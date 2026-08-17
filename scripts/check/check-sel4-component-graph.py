#!/usr/bin/env python3

"""P5.2 gate: a generation of native ELF component images boots its declared
graph on seL4.

Boots `build/slime-sel4-graph.elf` and asserts that init launches its declared
services with generation-derived native Endpoint capabilities, the services
exercise their bounded operation surface, and their explicit userspace shutdown
and supervision protocol completes before the graph is certified healthy. Raw
Endpoint closure is deliberately not lifecycle evidence: the kernel object
supplies rendezvous transport, not a service-termination protocol.

Modelled on `check-sel4-root-boot.py`, which guards P5.1 against the other
image. The images are separate artifacts on purpose: each gate boots the one it
asserts about, so neither invalidates the other's evidence by being built last.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import threading
import tomllib
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import profile_text, profile_integer, sha256_file  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4-graph.elf"
MANIFEST = ROOT / "build" / "slime-sel4-graph.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"

BOOT_TIMEOUT_SECONDS = 120

# The v5 generation carries five executable catalogue entries and five instance
# declarations, but root owns and autostarts only init. Init spawns console and
# spawn-service, drives their scenario, explicitly shuts them down, and observes
# their termination through supervision before completing itself.
TERMINAL_MARKER = (
    r"SLIME_GRAPH HEALTHY generation=1 required=3 live=0 completed=3 failed=0"
)

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    ("generation admitted", r"SLIME_ROOT generation admitted number=1 executables=5 instances=5 grants=\d+ "),
    ("authority manifest reported", r"SLIME_ROOT authority manifest=\["),
    ("all catalogue payloads are native ELF images", r"SLIME_ROOT graph admitted executables=5 instances=5 slimecm=0 elf=5 unrecognized=0"),
    ("only root-owned init was staged", r"SLIME_GRAPH staged task=0 instance=init executable=init grants=7 bindings=7 window=0x[0-9a-f]+ frames=[1-9]\d* tables=[1-9]\d* entry=0x[0-9a-f]+"),
    ("the executable catalogue remained available to spawn", r"SLIME_GRAPH staged instances=1 root_autostart=1 loadable_executables=5 slimecm=0 wrong_target=0 unrecognized=0"),
    (
        "console's generation-owned Endpoint was installed",
        r"SLIME_GRAPH endpoint grant=console-output producer_instance=0 consumer_instance=2",
    ),
    (
        "spawn-service's generation-owned Endpoint was installed",
        r"SLIME_GRAPH endpoint grant=spawn-service-rpc producer_instance=3 consumer_instance=2",
    ),
    ("only init was root-activated", r"SLIME_GRAPH activated instances=1"),
    ("init began the declared graph", r"\[init\] launching component graph"),
    ("init authorized console through its executable binding", r"SLIME_GRAPH spawn authorized task=0 slot=1 component=console grants=0"),
    (
        "console received its installed native Endpoint capability",
        r"SLIME_GRAPH native endpoint task=1 slot=33 side=both",
    ),
    ("init spawned console as instance task 1", r"SLIME_GRAPH spawned task=0 child=1 component=console grants=0 endpoints=1 notifications=0 handle=\d+ supervision_grants=0 buffer_factory_grants=0"),
    ("init authorized spawn-service through its executable binding", r"SLIME_GRAPH spawn authorized task=0 slot=5 component=spawn-service grants=3"),
    (
        "spawn-service received its installed native Endpoint capability",
        r"SLIME_GRAPH native endpoint task=2 slot=33 side=both",
    ),
    ("init spawned spawn-service as instance task 2", r"SLIME_GRAPH spawned task=0 child=2 component=spawn-service grants=3 endpoints=1 notifications=0 handle=\d+ supervision_grants=0 buffer_factory_grants=1"),
    ("spawn-service reached its service loop", r"\[spawn-service\] ready"),
    ("spawn-service allocated against its quota", r"SLIME_GRAPH buffer created task=2 slot=\d+ id=\d+ pages=1 writable=1"),
    ("the shared-buffer lifecycle became live", r"\[spawn-service\] shared-buffer quota live"),
    ("spawn-service received explicit shutdown", r"\[spawn-service\] shutdown received"),
    ("spawn-service completed its protocol", r"\[spawn-service\] complete"),
    ("spawn-service exited cleanly", r"SLIME_GRAPH component exit task=2 status=0"),
    ("console received explicit shutdown", r"\[console\] channel close received"),
    ("console completed its protocol", r"\[console\] channel plane complete"),
    ("console exited cleanly", r"SLIME_GRAPH component exit task=1 status=0"),
    (
        "init observed both service terminations through supervision",
        r"\[init\] component services completed",
    ),
    ("init completed the causal launch", r"\[init\] spawn graph launched"),
    ("init completed cleanly", r"SLIME_GRAPH component exit task=0 status=0"),
    ("every task arena was reclaimed", r"SLIME_GRAPH tasks reclaimed live=0 slots=[1-9]\d*"),
    (
        "no task-owned native authority or root export ticket leaked",
        r"SLIME_GRAPH native task_caps=0 exports=0 tickets=0",
    ),
    ("the supervisor certified the graph", TERMINAL_MARKER),
)

# B50 is a repository-wide cutover. Guard every surviving implementation source
# that could reintroduce the universal dispatcher or product-plane selection;
# generated outputs, historical contracts, docs, and negative-test selectors
# are intentionally outside this exact-source check.
COMPATIBILITY_SOURCE_ROOTS = (
    ROOT / "slime-root" / "src",
    ROOT / "components" / "runtime" / "src",
    ROOT / "components" / "bins" / "src",
)
# The files that actually *held* the deleted model symbols: the manifest and
# wire schemas that named `endpointCreate` as a right, the builder that encoded
# it, and the decoder plus independent checker that read it back. A guard over
# the Rust component tree alone could not fail on any of them, so the symbols
# would have been unenforced exactly where they lived.
COMPATIBILITY_SOURCE_FILES = (
    ROOT / "components" / "bins" / "build.rs",
    ROOT / "scripts" / "build" / "build-generation.py",
    ROOT / "scripts" / "check" / "check-generation.py",
    ROOT / "contracts" / "generation" / "v1" / "schema.zt",
    ROOT / "contracts" / "generation" / "v5" / "schema.zt",
    ROOT / "contracts" / "generation" / "v5" / "gen_rust.zt",
    ROOT / "boot-contracts" / "src" / "generation.rs",
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL .*",
    r"SLIME_GRAPH FAIL .*",
    r"SLIME_GRAPH component exit .*status=-?[1-9]\d*",
    # A component that could not bind its transfer window would issue no
    # windowed operation at all, and the graph would look quiet rather than
    # broken.
    r"\[slime-rt\] transfer window bind failed",
    # A component image refused after admission, or a payload that reached the
    # loader without being admitted.
    r"SLIME_GRAPH window bind refused",
    # The root task's service loop never draining is a livelock, not a slow
    # component.
    r"SLIME_GRAPH service budget exhausted",
    # seL4's own complaints. `read-only endpoint cap` in particular means a
    # component cannot invoke the root at all, which is silent from the Slime
    # side: the component simply never speaks.
    r"Attempted to invoke a read-only endpoint",
    r"seL4 called fail",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 component graph check: {message}")


def load_pins() -> dict[str, object]:
    if not PINS_PATH.is_file():
        fail(f"missing pin manifest: {PINS_PATH.relative_to(ROOT)}")
    try:
        pins = tomllib.loads(PINS_PATH.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse {PINS_PATH.relative_to(ROOT)}: {error}")
    if pins.get("schema") != 1:
        fail("unsupported sel4/pins.toml schema (expected 1)")
    if not isinstance(pins.get("qemu_arm_virt"), dict):
        fail("sel4/pins.toml is missing [qemu_arm_virt]")
    return pins


def build_image() -> None:
    command = [sys.executable, str(BUILD_SCRIPT), "--component-graph"]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot run the seL4 image build: {error}")
    if process.returncode != 0:
        fail(f"seL4 image build failed with exit status {process.returncode}")


def check_manifest() -> None:
    if not MANIFEST.is_file():
        fail(
            f"missing identity manifest {MANIFEST.relative_to(ROOT)}; "
            "run `just sel4_component_graph_check`"
        )
    try:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse {MANIFEST.relative_to(ROOT)}: {error}")
    if not isinstance(manifest, dict) or manifest.get("kind") != "slime-sel4-image-identity":
        fail(f"{MANIFEST.relative_to(ROOT)} is not a Slime seL4 identity manifest")
    # The two images are built from the same sources and differ only in which
    # generation the root task embeds, so booting the wrong one would fail on
    # markers rather than on identity. Checking the flag reports the actual
    # cause instead.
    if manifest.get("component_graph") is not True:
        fail(
            f"{MANIFEST.relative_to(ROOT)} does not record a component-graph image; "
            "rebuild with `--component-graph`"
        )
    image = manifest.get("image")
    if not isinstance(image, dict) or not isinstance(image.get("sha256"), str):
        fail("identity manifest does not record the packaged image digest")
    if not IMAGE.is_file():
        fail(f"missing packaged image {IMAGE.relative_to(ROOT)}")
    actual = sha256_file(IMAGE, fail)
    if actual != image["sha256"]:
        fail(
            f"{IMAGE.relative_to(ROOT)} SHA-256 is {actual}, but the identity manifest "
            f"records {image['sha256']}; rebuild before booting"
        )


def boot(profile: dict[str, object]) -> str:
    """Boot the image and return the serial transcript.

    The root task suspends itself once the graph has drained, so QEMU stays
    alive afterwards and waiting for an exit would always time out. Serial
    output is read line by line and the guest is killed as soon as the terminal
    or any failure marker appears.
    """
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    command = [
        qemu,
        "-machine",
        profile_text(profile, "machine", fail),
        "-cpu",
        profile_text(profile, "cpu", fail),
        "-smp",
        str(profile_integer(profile, "cpus", fail)),
        "-m",
        f"size={profile_integer(profile, 'memory_mib', fail)}M",
        "-nographic",
        "-serial",
        "mon:stdio",
        "-kernel",
        str(IMAGE),
    ]
    print(f"[boot] {' '.join(command)}", flush=True)
    terminal = re.compile(TERMINAL_MARKER)
    failures = re.compile("|".join(FAILURE_MARKERS))
    lines: list[str] = []
    try:
        process = subprocess.Popen(
            command,
            cwd=ROOT,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
    except OSError as error:
        fail(f"cannot run QEMU: {error}")
    # A wedged guest emits nothing, so the deadline cannot live in the read
    # loop; a watchdog kills QEMU, which closes the pipe and ends the loop.
    watchdog = threading.Timer(BOOT_TIMEOUT_SECONDS, process.kill)
    watchdog.start()
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\n"))
            if terminal.search(line) or failures.search(line):
                break
    finally:
        timed_out = not watchdog.is_alive()
        watchdog.cancel()
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    transcript = "\n".join(lines)
    if timed_out and terminal.search(transcript) is None:
        report_transcript(transcript)
        fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without reaching the final marker")
    return transcript


def report_transcript(transcript: str) -> None:
    tail = transcript.splitlines()[-40:]
    if tail:
        sys.stdout.write("--- serial transcript (tail) ---\n")
        sys.stdout.write("\n".join(tail) + "\n")
        sys.stdout.write("--- end transcript ---\n")
        sys.stdout.flush()


def check_deleted_compatibility_surface() -> None:
    forbidden = (
        "enum Operation",
        "Operation::",
        "enum Mediation",
        "Mediation::",
        "MAX_OPERATION_LABEL",
        "RETIRED_POLICY_LABELS",
        "RETIRED_INPUT_READ_LABEL",
        "RETIRED_BLOCK_TRANSACT_LABEL",
        "RETIRED_STORE_TRANSACT_LABEL",
        "RETIRED_DIRECTORY_LABEL",
        "GraphTables",
        "SERVICE_ROOT_DISPATCH",
        "endpointCreate",
        "right_roles",
        "channel_aliases",
        "SLIME_SEL4_CHANNEL_CHECK",
        "SLIME_SEL4_CALL_CHECK",
        "SLIME_SEL4_OPERATION_CHECK",
        "SLIME_SEL4_STREAM_CHECK",
        "SLIME_SEL4_QOS_CHECK",
        "SLIME_SEL4_VISIBILITY_CHECK",
    )
    guarded = list(COMPATIBILITY_SOURCE_FILES)
    for source_root in COMPATIBILITY_SOURCE_ROOTS:
        guarded.extend(sorted(source_root.rglob("*.rs")))
    root_only = ("enum Operation", "Operation::")
    for path in guarded:
        if not path.is_file():
            fail(f"missing {path.relative_to(ROOT)}")
        source = path.read_text(encoding="utf-8")
        symbols = forbidden
        if path.parent != ROOT / "slime-root" / "src":
            symbols = tuple(symbol for symbol in forbidden if symbol not in root_only)
        for symbol in symbols:
            if symbol in source:
                fail(
                    f"{path.relative_to(ROOT)} retains deleted compatibility "
                    f"surface {symbol!r} (B50)"
                )
    print("repository service surface: compatibility model deleted", flush=True)



def check_transcript(transcript: str) -> None:
    for pattern in FAILURE_MARKERS:
        match = re.search(pattern, transcript)
        if match is not None:
            report_transcript(transcript)
            fail(f"failure marker in serial transcript: {match.group(0)!r}")
    position = 0
    for description, pattern in REQUIRED_MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            report_transcript(transcript)
            if re.search(pattern, transcript) is not None:
                fail(f"marker out of order: {description} ({pattern})")
            fail(f"missing marker: {description} ({pattern})")
        position = match.end()
    # Each task's window is its own region, which is the property the pinned
    # addresses used to carry before component code size made them brittle. Two
    # tasks bound at one base would mean one staging area serving both, and a
    # payload one wrote appearing in the other's `recv`.
    bases = dict(re.findall(r"SLIME_GRAPH window bound task=(\d+) base=(0x[0-9a-f]+)", transcript))
    if len(set(bases.values())) != len(bases):
        fail(f"two tasks bound the same transfer window base: {bases}")
    print(
        f"windows: {len(bases)} tasks each bound a distinct region "
        f"({', '.join(f'task {t}@{b}' for t, b in sorted(bases.items()))})",
        flush=True,
    )

    terminals = re.findall(TERMINAL_MARKER, transcript)
    if len(terminals) != 1:
        fail(f"expected exactly one healthy supervisor terminal, saw {len(terminals)}")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 component-graph image and assert ordered markers"
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built image instead of rebuilding it first",
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    pins = load_pins()
    if not arguments.no_build:
        build_image()
    check_manifest()
    check_deleted_compatibility_surface()
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    check_transcript(boot(profile))
    print(
        "seL4 component graph check: init launched the two required services with "
        "native Endpoint authority, the services exercised their bounded operation "
        "surface and completed explicit supervised shutdown, and no task-owned "
        "native/root resource leaked"
    )


if __name__ == "__main__":
    main()
