#!/usr/bin/env python3

"""P5.3.1 gate: two components rendezvous over a generation-declared native
seL4 Endpoint.

Boots `build/slime-sel4-channel.elf` -- the image whose root task embeds the
channel-plane generation, `contracts/generation-manifest/v1/compositions/sel4-channel.zti` --
and asserts ordered evidence that root installs the statically attenuated
Endpoint capabilities before activation, the blocking send completes only
after its receiver runs and accepts the exact payload, both components complete
through an explicit userspace/supervision lifecycle, and every task-owned native
capability and root export ticket is reclaimed.

Modelled on `check-sel4-component-graph.py`, which guards P5.2 against a
different image. The seL4 images are separate artifacts on purpose: each gate
boots the one it asserts about, so none invalidates another's evidence by being
built last.
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
import threading
import tomllib
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from closure_image import ClosureImageError, build as build_closure_image  # noqa: E402
from harness import profile_text, profile_integer, sha256_file  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"

# CP15: this plane builds by closure identity rather than by a plane flag. The
# identity names the build's inputs and is re-resolved from repository state
# before the build, so a stale or hand-edited input is a refusal instead of a
# silently different image. `IMAGE` is filled in by the build below rather than
# being a fixed path a stale artifact could satisfy.
CLOSURE = "sel4-channel"
IMAGE: Path | None = None

BOOT_TIMEOUT_SECONDS = 120

# The bytes `init` sends to `console`, pinned so the transcript proves the
# receiver observed the complete message rather than merely some successful
# Endpoint rendezvous.
PAYLOAD_BYTES = 42

TERMINAL_MARKER = (
    r"SLIME_GRAPH HEALTHY generation=1 required=2 live=0 completed=2 failed=0"
)

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "the channel generation was admitted",
        r"SLIME_ROOT generation admitted number=\d+ executables=2 instances=2 grants=2 ",
    ),
    (
        "both payloads are native ELF images",
        r"SLIME_ROOT graph admitted executables=2 instances=2 slimecm=0 elf=2 unrecognized=0",
    ),
    (
        "console made unrelated progress while init remained blocked",
        r"\[console\] unrelated progress while sender blocked",
    ),
    # CP2: the runtime binding query's denial arm, asserted from the root's own
    # line as well as the component's, so a component that simply never asked
    # could not satisfy it. The grant arm is proved by `init` resolving the
    # channel edge it sends on below — a wrong answer there is a failed
    # rendezvous, not a passing boot.
    (
        "a binding this instance was not granted was refused rather than answered",
        r"SLIME_GRAPH binding unresolved task=1 instance=0 len=26",
    ),
    (
        "console observed that denial as a denial",
        r"\[console\] ungranted binding denied",
    ),
    ("init entered the blocking native send", r"\[init\] rendezvous send entering"),
    (
        "init observed rendezvous completion",
        r"\[init\] rendezvous send completed",
    ),
    (
        "console printed the exact rendezvous payload",
        r"\[console\] channel plane carried this line",
    ),
    ("console accepted the explicit close message", r"\[console\] channel close received"),
    ("console completed its channel role", r"\[console\] channel plane complete"),
    ("console exited cleanly", r"SLIME_GRAPH component exit task=1 status=0"),
    (
        "init observed console termination through supervision",
        r"\[init\] channel receiver completed",
    ),
    ("init completed the scenario", r"\[init\] channel plane complete"),
    ("init exited cleanly", r"SLIME_GRAPH component exit task=0 status=0"),
    (
        "the graph drained its task-owned authority and window tables",
        r"SLIME_GRAPH served live=0 unsupported=0 buffers=0 windows=0 tasks=0",
    ),
    (
        "every task arena and native capability was reclaimed",
        r"SLIME_GRAPH tasks reclaimed live=0 slots=[1-9]\d*",
    ),
    (
        "no task-owned native authority or root export ticket leaked",
        r"SLIME_GRAPH native task_caps=0 exports=0 tickets=0",
    ),
    ("the supervisor certified the completed graph", TERMINAL_MARKER),
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL .*",
    r"SLIME_GRAPH FAIL .*",
    r"SLIME_GRAPH component exit .*status=-?[1-9]\d*",
    r"\[init\] channel plane fail: .*",
    r"\[slime-rt\] transfer window bind failed",
    r"SLIME_GRAPH window bind refused",
    r"SLIME_GRAPH endpoint unplaced .*",
    r"SLIME_GRAPH service budget exhausted",
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
    raise SystemExit(f"seL4 channel plane check: {message}")


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
    """Build this plane's image from its closure and bind `IMAGE` to the result."""
    global IMAGE
    try:
        built = build_closure_image(CLOSURE)
    except ClosureImageError as error:
        fail(str(error))
    IMAGE = built.image
    # The digest the build recorded, against the bytes on disk. The closure
    # identity is already verified against repository state by the builder; this
    # is the second half, proving the file about to be booted is the one that
    # build produced.
    actual = sha256_file(IMAGE, fail)
    if actual != built.digest():
        fail(
            f"{IMAGE} SHA-256 is {actual}, but the build result records "
            f"{built.digest()}; the image changed after it was built"
        )
    print(
        f"[closure] {CLOSURE} resolved to {built.identity[:12]} "
        f"and produced {actual[:12]}",
        flush=True,
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
        description="Boot the seL4 channel-plane image and assert ordered markers"
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built image instead of rebuilding it first",
    )
    parser.add_argument(
        "--image",
        type=Path,
        help="boot this verified closure image (requires --no-build)",
    )
    arguments = parser.parse_args()
    global IMAGE
    if arguments.image is not None:
        if not arguments.no_build:
            fail("--image requires --no-build")
        IMAGE = arguments.image.resolve()
        if not IMAGE.is_file():
            fail(f"missing image: {IMAGE}")
    elif arguments.no_build:
        fail("--no-build requires --image naming the already-built closure image")

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    pins = load_pins()
    if not arguments.no_build:
        build_image()
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    check_transcript(boot(profile))
    print(
        "seL4 channel plane check: the declared native Endpoint was installed with "
        "static direction, its blocking rendezvous carried the exact payload, both "
        "components completed explicitly, and no task-owned native/root resource leaked"
    )


if __name__ == "__main__":
    main()
