#!/usr/bin/env python3

"""P5.4.3 gate: `InputRead` mediation.

Small on purpose. M6.4's Dango session is the consumer of key events and a large
composition with its own failure modes; this asserts the mechanism underneath
it, so a defect in the authority path is distinguishable from a defect in the
shell.

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

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
IMAGE = ROOT / "build" / "slime-sel4-input.elf"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-input.zti"
BOOT_TIMEOUT_SECONDS = 180

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "the unconfigured instance parked without a run token",
        r"\[sel4-input-probe\] idle without a run token",
    ),
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


def build_image() -> None:
    command = [sys.executable, str(BUILD_SCRIPT), "--input-plane"]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot run the seL4 image build: {error}")
    if process.returncode != 0:
        fail(f"seL4 image build failed with exit status {process.returncode}")




def boot(profile: dict[str, object]) -> str:
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
        for line in process.stdout:
            lines.append(line.rstrip("\r\n"))
            if failures.search(line):
                break
            if terminal.search(line):
                reached = True
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
    if not IMAGE.is_file():
        fail(f"missing packaged image {IMAGE.relative_to(ROOT)}")
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
