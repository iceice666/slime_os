#!/usr/bin/env python3

"""P5.2 gate: a generation of native ELF component images boots its declared
graph on seL4.

Boots `build/slime-sel4-graph.elf` -- the image whose root task embeds the
`aarch64-sel4-qemu-virt` generation -- and asserts ordered markers for each of
P5.2's three required checks:

1. a generation whose payloads are native ELF images launches its declared
   components with their declared grants;
2. the root service answers the operation surface those components actually
   invoke, with the same errors and bounds as the legacy kernel;
3. an unsupported operation returns its bounded Slime error rather than
   faulting the caller.

Modelled on `check-sel4-root-boot.py`, which guards P5.1 against the other
image. The two are separate artifacts on purpose: each gate boots the one it
asserts about, so neither invalidates the other's evidence by being built last.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
import threading
import tomllib
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4-graph.elf"
MANIFEST = ROOT / "build" / "slime-sel4-graph.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"

BOOT_TIMEOUT_SECONDS = 120

# The v4 generation carries five executable catalogue entries and five instance
# declarations, but root owns and autostarts only init. Init then exercises its
# explicit executable bindings by spawning console and spawn-service.
TERMINAL_MARKER = (
    r"SLIME_GRAPH HEALTHY generation=1 required=3 live=2 completed=1 failed=0"
)

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    ("generation admitted", r"SLIME_ROOT generation admitted number=1 executables=5 instances=5 grants=\d+ "),
    ("authority manifest reported", r"SLIME_ROOT authority manifest=\["),
    ("all catalogue payloads are native ELF images", r"SLIME_ROOT graph admitted executables=5 instances=5 slimecm=0 elf=5 unrecognized=0"),
    ("only root-owned init was staged", r"SLIME_GRAPH staged task=0 instance=init executable=init grants=8 bindings=8 window=0x[0-9a-f]+ frames=[1-9]\d* tables=[1-9]\d* entry=0x[0-9a-f]+"),
    ("the executable catalogue remained available to spawn", r"SLIME_GRAPH staged instances=1 root_autostart=1 loadable_executables=5 slimecm=0 wrong_target=0 unrecognized=0"),
    ("only init was root-activated", r"SLIME_GRAPH activated instances=1"),
    # Init's window sits above its own image, so the address is a function of
    # how large `init.rs` compiles to and moves whenever its code changes. The
    # property under test is that init bound a one-page window at all; the two
    # child addresses below stay exact, because those images are not edited by
    # work on init's composition.
    ("init bound its transfer window", r"SLIME_GRAPH window bound task=0 base=0x[0-9a-f]+ len=4096"),
    ("init began the declared graph", r"\[init\] launching component graph"),
    ("init authorized console through its executable binding", r"SLIME_GRAPH spawn authorized task=0 slot=1 component=console grants=1"),
    ("init spawned console as instance task 1", r"SLIME_GRAPH spawned task=0 child=1 component=console grants=1 channels=1 handle=\d+"),
    ("init authorized spawn-service through its executable binding", r"SLIME_GRAPH spawn authorized task=0 slot=5 component=spawn-service grants=5"),
    ("init spawned spawn-service as instance task 2", r"SLIME_GRAPH spawned task=0 child=2 component=spawn-service grants=5 channels=1 handle=\d+"),
    ("init completed the causal launch", r"\[init\] spawn graph launched"),
    ("init completed cleanly", r"SLIME_GRAPH component exit task=0 status=0"),
    ("spawn-service bound its mapped window", r"SLIME_GRAPH window bound task=2 base=0x237000 len=4096"),
    ("spawn-service reached its service loop", r"\[spawn-service\] ready"),
    ("spawn-service allocated against its quota", r"SLIME_GRAPH buffer created task=2 slot=\d+ id=\d+ pages=1 writable=1"),
    ("the shared-buffer lifecycle became live", r"\[spawn-service\] shared-buffer quota live"),
    ("spawn-service parked live", r"SLIME_GRAPH parked task=2 reason=wait"),
    ("console bound its mapped window", r"SLIME_GRAPH window bound task=1 base=0x236000 len=4096"),
    ("console parked live", r"SLIME_GRAPH parked task=1 reason=wait"),
    ("the supervisor certified the graph", TERMINAL_MARKER),
)

# Every operation the root task must answer with a bounded error rather than a
# handler, checked against `slime-root/src/ipc.rs` itself.
#
# The runtime half of required check 3 can only observe the operations these
# five components happen to invoke. This half asserts the property for the
# whole surface, statically: after B43 and B44 no label is classified
# `Unavailable`, because every operation the root did not actually perform was
# removed from the ABI rather than left answering `UnsupportedOperation`.
# Block requests moved to the console thread with the device tables; store,
# generation, recovery, and health were deleted as userspace policy built over
# block authority. What remains must be refused, not resolved.

IPC_SOURCE = ROOT / "slime-root" / "src" / "ipc.rs"

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


def profile_text(profile: dict[str, object], key: str) -> str:
    value = profile.get(key)
    if not isinstance(value, str) or not value:
        fail(f"sel4/pins.toml [qemu_arm_virt].{key} must be non-empty text")
    return value


def profile_integer(profile: dict[str, object], key: str) -> int:
    value = profile.get(key)
    if not isinstance(value, int) or isinstance(value, bool):
        fail(f"sel4/pins.toml [qemu_arm_virt].{key} must be an integer")
    return value


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as handle:
            while chunk := handle.read(1024 * 1024):
                digest.update(chunk)
    except OSError as error:
        fail(f"cannot hash {path.relative_to(ROOT)}: {error}")
    return digest.hexdigest()


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
    actual = sha256_file(IMAGE)
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
        profile_text(profile, "machine"),
        "-cpu",
        profile_text(profile, "cpu"),
        "-smp",
        str(profile_integer(profile, "cpus")),
        "-m",
        f"size={profile_integer(profile, 'memory_mib')}M",
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


def check_operation_surface() -> None:
    """The static half of required check 3.

    Asserts that the mediation table has no unmediated class left. Until B44
    it asserted the opposite: that each plane the cutover did not own stayed
    `Mediation::Unavailable`, so a plane quietly reclassified `RootService`
    would fail here rather than fall through the dispatcher's unimplemented
    path.

    Every member of that class is now gone from the ABI instead of
    reclassified, because an operation whose only answer is
    `UnsupportedOperation` is surface for something the root does not do.
    So the claim inverts: every label the root still accepts must be one it
    actually performs, and a reintroduced `Unavailable` arm is a regression.
    """
    if not IPC_SOURCE.is_file():
        fail(f"missing {IPC_SOURCE.relative_to(ROOT)}")
    source = IPC_SOURCE.read_text(encoding="utf-8")
    start = source.find("pub const fn mediation(self) -> Mediation {")
    if start < 0:
        fail(f"{IPC_SOURCE.relative_to(ROOT)} declares no mediation table")
    table = source[start:]
    end = table.find("\n    }\n")
    if end < 0:
        fail("the mediation table has no discernible end")
    table = table[:end]
    if "Mediation::Unavailable" in table:
        fail(
            "the mediation table classifies a plane Unavailable again; such an "
            "operation answers UnsupportedOperation and nothing else, which is "
            "ABI surface for something the root does not perform (B44)"
        )
    # Every retired label must stay retired: `from_label` refuses it rather
    # than resolving it to whichever operation now sits at that number.
    retired = re.findall(r"pub const RETIRED_\w+: sel4::Word = (\d+);", source)
    retired += re.findall(r"pub const RETIRED_POLICY_LABELS: \[sel4::Word; \d+\] = \[([^\]]+)\]", source)
    holes = set()
    for entry in retired:
        holes.update(part.strip() for part in entry.split(",") if part.strip())
    if len(holes) < 6:
        fail(f"expected at least six retired labels, found {sorted(holes)}")
    resolver = source[source.find("const fn from_label"):]
    resolver = resolver[: resolver.find("\n    }\n")]
    for hole in sorted(holes, key=int):
        if re.search(rf"^\s*{hole} => Self::", resolver, re.M):
            fail(f"retired label {hole} resolves to an operation again")
    print(
        f"operation surface: no unmediated plane remains and {len(holes)} "
        "retired labels stay refused",
        flush=True,
    )



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
    check_operation_surface()
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    check_transcript(boot(profile))
    print(
        "seL4 component graph check: init launched the two required spawned instances; "
        "spawn-service exercised its bounded operation surface; console and spawn-service "
        "parked live; the supervisor certified the required graph"
    )


if __name__ == "__main__":
    main()
