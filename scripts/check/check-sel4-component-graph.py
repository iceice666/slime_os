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

# Components the `aarch64-sel4-qemu-virt` generation declares, and the number of
# executables each one's outbound `exec | spawn` grants name. Pinned as a table
# rather than as a count, because the distinguishing claim of required check 1
# is *which* component holds *which* authority: `spawn-service` holds exactly
# the two executables the generation grants it and every other component holds
# none. A regression that granted everything to everyone would still produce
# five staged components, so the counts are what make the claim non-vacuous.
DECLARED_COMPONENTS: tuple[tuple[int, str, int], ...] = (
    (0, "console", 0),
    (1, "echo-agent", 0),
    (2, "init", 0),
    (3, "spawn-service", 2),
    (4, "sysinfo", 0),
)

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "generation admitted",
        r"SLIME_ROOT generation admitted number=\d+ components=5 grants=\d+",
    ),
    ("authority manifest reported", r"SLIME_ROOT authority manifest=\["),
    (
        # The inverse of P5.1's assertion. There `slimecm` had to be non-zero to
        # prove the "not activated" claim was not vacuous; here `elf=5` with
        # `slimecm=0` proves every payload this generation carries is a native
        # image, so the components launched below could only have come from
        # them.
        "every payload is a native ELF image",
        r"SLIME_ROOT graph admitted; legacy SLIMECM images not activated "
        r"components=5 slimecm=0 elf=5 unrecognized=0",
    ),
    # -- required check 1: declared components, declared grants --
    *(
        (
            f"{name} staged with its declared grants",
            rf"SLIME_GRAPH staged task={task} component={name} grants=\d+ "
            rf"executables={executables} window=0x[0-9a-f]+ "
            rf"frames=[1-9]\d* tables=[1-9]\d* entry=0x[0-9a-f]+",
        )
        for task, name, executables in DECLARED_COMPONENTS
    ),
    (
        "no payload was refused or unrecognized",
        r"SLIME_GRAPH staged components=5 loadable=5 slimecm=0 "
        r"wrong_target=0 unrecognized=0",
    ),
    ("every component activated", r"SLIME_GRAPH activated components=5"),
    # -- required check 2: the root answers the surface components invoke --
    #
    # The window bind is the load-bearing one: `recv`, `spawn`, and `wait` all
    # stage through the transfer window and refuse to truncate, so a component
    # that could not bind one could issue none of them. It is answered against
    # the mapping the loader actually made, not the caller's word.
    (
        "spawn-service bound the window the loader mapped for it",
        r"SLIME_GRAPH window bound task=3 base=0x237000 len=4096",
    ),
    ("spawn-service reached its service loop", r"\[spawn-service\] ready"),
    (
        "a real shared region was allocated against the declared quota",
        r"SLIME_GRAPH buffer created task=3 slot=\d+ id=\d+ pages=1",
    ),
    (
        # spawn-service runs create/map/write/seal/unmap/release at startup and
        # exits non-zero if any step fails, so this single line is the whole
        # shared-buffer lifecycle observed end to end through real seL4 frames.
        "the full shared-buffer lifecycle completed",
        r"\[spawn-service\] shared-buffer quota live",
    ),
    # -- required check 3: an unanswered operation is bounded, not fatal --
    #
    # Two distinct reasons an operation goes unanswered, and the root task keeps
    # them apart rather than reporting both as the same thing:
    #
    #   `unsupported`   the plane has no seL4 mechanism owner in this cutover
    #                   (storage, directory, input, generation management,
    #                   recovery). This is the designed answer.
    #   `unimplemented` the operation IS root-mediated and this slice has no
    #                   handler for it yet.
    #
    # Until P5.3.1 this gate asserted at least one `unimplemented` marker,
    # because `send`, `recv`, and `wait` had no handler and every declared
    # component reached one. P5.3.1 implemented them, so this boot no longer
    # emits that line -- the components now get real answers instead of bounded
    # errors, which is the point of that slice. Asserting the marker still
    # appears would be asserting that the channel plane is *missing*.
    #
    # Relaxing an assertion in the same change that alters the behaviour it
    # covered is how evidence gets lost, so the property is re-evidenced rather
    # than dropped. Four things assert it now, and each names a different half:
    #
    #   - the `spawn refused` / `spawn failed slot=N error=-4` pair below is a
    #     *live* bounded refusal on this boot -- an operation the root declines
    #     and the caller survives, observed rather than argued;
    #   - `check_operation_surface` asserts the nine unmediated planes are still
    #     classified `Unavailable`, statically, against `Operation::mediation`;
    #   - the terminal marker pins `unimplemented=0` exactly, so an operation
    #     losing its handler fails this gate rather than passing quietly;
    #   - FAILURE_MARKERS fails on any fault, panic, or abort, which is what
    #     "bounded rather than fatal" means.
    ("init ran and drove the graph", r"\[init\] launching component graph"),
    (
        # The negative half of required check 1: authority is resolved from the
        # caller's own table, so a component asking for an executable its
        # generation did not grant it is refused rather than served.
        "an ungranted executable slot is refused",
        r"SLIME_GRAPH spawn refused task=2 slot=\d+ ungranted",
    ),
    (
        "the refusal reached the component as an ordinary Slime error",
        r"\[init\] spawn failed slot=\d+ error=-4",
    ),
    (
        # `unimplemented=0` is pinned exactly rather than left open: with the
        # channel plane landed, every operation these five components reach now
        # has a handler, and an operation losing one would show up here.
        "the graph drained with every window and table reclaimed",
        r"SLIME_GRAPH served live=0 unsupported=0 unimplemented=0 "
        r"buffers=[1-9]\d* windows=0 tables=0",
    ),
    (
        # P5.3.1. The channel plane's own accounting for this graph. `parked=0`
        # and `queues=0` are the teardown property: no component is still
        # blocked on a reply the root owes it, and no queue still believes it
        # has a live peer -- either would be a graph that drained only because
        # the loop hit its iteration bound. `replies` counts every saved reply
        # CSlot handed back, so the parking path is shown not to leak.
        "every channel and held reply was reclaimed",
        r"SLIME_GRAPH channels served sends=\d+ receives=\d+ parks=\d+ "
        r"settled=\d+ parked=0 queues=0 replies=\d+",
    ),
)

# Every operation the root task must answer with a bounded error rather than a
# handler, checked against `slime-root/src/ipc.rs` itself.
#
# The runtime half of required check 3 can only observe the operations these
# five components happen to invoke. This half asserts the property for the whole
# unmediated surface: each of these labels is classified `Unavailable`, so
# `unmediated_response` returns a bounded error for it and the dispatcher's
# catch-all cannot turn one into a fault. Pinning the list here means a plane
# silently reclassified as `RootService` -- and therefore falling through to the
# unimplemented path -- fails this gate rather than passing quietly.
UNMEDIATED_OPERATIONS: tuple[tuple[str, int], ...] = (
    ("BlockTransact", 6),
    ("StoreTransact", 7),
    ("RecoveryReconstruct", 10),
    ("DirectoryInspect", 14),
    ("DirectoryDerive", 15),
    ("DirectoryCommit", 16),
    ("InputRead", 17),
    ("GenerationTransact", 18),
    ("GenerationReceive", 19),
)

IPC_SOURCE = ROOT / "slime-root" / "src" / "ipc.rs"

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL .*",
    r"SLIME_GRAPH FAIL .*",
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
    terminal = re.compile(REQUIRED_MARKERS[-1][1])
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

    Asserts that every plane this cutover does not own is still classified
    `Mediation::Unavailable`, so `unmediated_response` yields a bounded error
    for it. A plane quietly reclassified `RootService` would fall through the
    dispatcher's unimplemented path instead — the same visible result today,
    but a different claim, and one that would stop being true the moment a
    handler landed.
    """
    if not IPC_SOURCE.is_file():
        fail(f"missing {IPC_SOURCE.relative_to(ROOT)}")
    source = IPC_SOURCE.read_text(encoding="utf-8")
    start = source.find("pub const fn mediation(self) -> Mediation {")
    if start < 0:
        fail(f"{IPC_SOURCE.relative_to(ROOT)} declares no mediation table")
    table = source[start:]
    unavailable = table.find("Mediation::Unavailable")
    if unavailable < 0:
        fail("the mediation table declares no Unavailable plane")
    # The arm listing the unavailable operations is whatever precedes the
    # `Mediation::Unavailable` result.
    arm = table[:unavailable]
    arm = arm[arm.rfind("Mediation::RootService") :]
    for name, label in UNMEDIATED_OPERATIONS:
        if f"Self::{name}" not in arm:
            fail(
                f"operation {name} (label {label}) is no longer classified "
                "Unavailable; required check 3 covers a different surface than "
                "this gate asserts"
            )
    print(
        f"operation surface: {len(UNMEDIATED_OPERATIONS)} unmediated planes "
        "answer a bounded Slime error",
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
        "seL4 component graph check: 5 native ELF components launched with their "
        "declared grants, the root answered their operation surface, and an "
        "unanswered operation returned a bounded error with the caller running"
    )


if __name__ == "__main__":
    main()
