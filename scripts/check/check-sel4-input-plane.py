#!/usr/bin/env python3

"""P5.4.3 gate: `InputRead` mediation.

Small on purpose. This gate isolates input mediation from any interactive
language, so authority-path defects remain distinguishable from shell or
evaluator defects.

Three claims: a granted capability yields the generation's scripted keys in
order and decoded, an exhausted script terminates its reader rather than
blocking it, and a slot holding no input capability is refused.

The second is not hypothetical. `WAIT_KIND_INPUT` resolved to a wait target that
is never ready, so a component waiting on input parked forever — a defect that
survived because no plane had ever waited on input.
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

from harness import GENERATION_COMPOSITIONS, profile_text, profile_integer, sha256_file  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
# CP15: the closure identity names the build's inputs and is re-resolved from
# repository state before the build, so stale input is refused rather than
# silently producing a different image.
CLOSURE = "sel4-input"
IMAGE: Path | None = None
FIXTURE = GENERATION_COMPOSITIONS / "sel4-input.zti"
BOOT_TIMEOUT_SECONDS = 180

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "the spawned instance received its declared input authority",
        r"SLIME_GRAPH declared placed task=\d+ child=\d+ slot=\d+ kind=input",
    ),
    (
        "init spawned the probe",
        r"\[init\] input probe spawned",
    ),
    (
        # Checked before the positive arms, so a mechanism ignoring the
        # capability entirely could not pass them by accident.
        "a slot holding no input capability was refused",
        r"\[sel4-input-probe\] ungranted slot refused",
    ),
    (
        # Order, the character encoding, the named-key encoding, and the
        # `pressed` bit — all four compared against the script rather than
        # assumed. Two of them were wrong when this mechanism was written.
        "the scripted keys decoded in order",
        r"\[sel4-input-probe\] script decoded in order",
    ),
    (
        # The arm that would have caught `WaitTarget::Unmediated`.
        "an exhausted script ends its reader rather than blocking",
        r"\[sel4-input-probe\] exhausted script ends the reader",
    ),
    (
        "the probe ran every arm and exited cleanly",
        r"\[sel4-input-probe\] input plane complete",
    ),
    (
        "init observed the clean exit",
        r"\[init\] input plane complete",
    ),
)

TERMINAL_MARKER = r"\[init\] input plane complete"

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    r"\[init\] input plane fail: .*",
    r"\[sel4-input-probe\] fail: .*",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)

def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 input plane check: {message}")


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
    global IMAGE
    try:
        built = build_closure_image(CLOSURE)
    except ClosureImageError as error:
        fail(str(error))
    IMAGE = built.image
    actual = sha256_file(IMAGE, fail)
    if actual != built.digest():
        fail(
            f"{IMAGE} SHA-256 is {actual}, but the build result records "
            f"{built.digest()}; the image changed after it was built"
        )




def boot(profile: dict[str, object]) -> str:
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
    failures = re.compile("|".join(FAILURE_MARKERS))
    terminal = re.compile(TERMINAL_MARKER)
    lines: list[str] = []
    reached = False
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
    watchdog = threading.Timer(BOOT_TIMEOUT_SECONDS, process.kill)
    watchdog.start()
    try:
        assert process.stdout is not None
        # The root-owned idle instance runs concurrently with init and reports
        # holding no run token only after a bounded wait, so its line can land
        # before or after init's completion marker. Stop once *both* facts have
        # been observed. A fixed line tail waited forever on a quiescent graph
        # that had nothing else to print; this waits for evidence, not output.
        idle_seen = False
        for line in process.stdout:
            lines.append(line.rstrip("\r\n"))
            if failures.search(line):
                break
            idle_seen |= "[sel4-input-probe] idle without a run token" in line
            reached |= terminal.search(line) is not None
            if reached and idle_seen:
                break
    finally:
        watchdog.cancel()
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    transcript = "\n".join(lines)
    if not reached:
        report_transcript(transcript)
        fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without completing the plane")
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
    for label, pattern in REQUIRED_MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            report_transcript(transcript)
            if re.search(pattern, transcript) is not None:
                fail(f"marker out of order: {label} ({pattern})")
            fail(f"missing marker: {label} ({pattern})")
        position = match.end()
    # Asserted by count, not by position.
    #
    # The plane declares this executable twice: the instance init spawns, and a
    # root-owned idle one holding the same input authority with no session. The
    # idle instance concludes it holds no run token only after a bounded wait,
    # which is what distinguishes "nothing will ever arrive" from "the sender has
    # not spoken yet" — so its marker lands wherever the scheduler puts it,
    # including after the terminal line. Ordering it would be asserting a
    # scheduling accident.
    if "[sel4-input-probe] idle without a run token" not in transcript:
        report_transcript(transcript)
        fail("the unconfigured instance did not report parking without a run token")
    completions = transcript.count("[sel4-input-probe] input plane complete")
    if completions != 1:
        report_transcript(transcript)
        fail(f"{completions} instances ran the scenario, expected 1")
    print(
        f"transcript: {len(REQUIRED_MARKERS)} markers observed; a granted "
        "capability decoded the scripted keys in order, an exhausted script "
        "ended its reader, and an ungranted slot was refused",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 input-plane image and assert InputRead mediation"
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built image instead of rebuilding it first",
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    if not FIXTURE.is_file():
        fail(f"missing generation fixture {FIXTURE.relative_to(ROOT)}")
    pins = load_pins()
    if not arguments.no_build:
        build_image()
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    check_transcript(boot(profile))
    print(
        "seL4 input plane check: a component read the generation's scripted "
        "keys through a granted capability, in order and correctly decoded, was "
        "ended rather than blocked when the script ran out, and was refused a "
        "slot holding no input capability"
    )


if __name__ == "__main__":
    main()
