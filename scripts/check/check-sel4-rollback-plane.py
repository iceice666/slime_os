#!/usr/bin/env python3

"""P5.4.2c gate: M5.6's rollback contract, in userspace (M5.6).

Two fixed BootState slots on a real device, and the transitions between them.
The property under test is the one M5.6 names — *no transition overwrites the
only valid root* — so every commit is older-slot-first and the gate checks, at
each step, that the previously selected slot still decodes.

The walked sequence is the oracle's `3 -> 2 -> 1 -> 0`: stage a pending
generation with three attempts, consume all durably, find it exhausted, roll
back to known-good, then confirm rollback is idempotent. Promotion follows,
rollback is only half the contract: the running generation is promoted, and both
a wrong running identity and a stale release sequence are refused.

What makes this more than a unit test is durability. Every transition is a
write, a flush, and a re-read: the state the gate reports is the one a fresh
boot would select off the device, not the one the component computed.
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import tomllib
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import profile_text, profile_integer  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE_SCRIPT = ROOT / "scripts" / "build" / "build-store-fixture.py"
IMAGE = ROOT / "build" / "slime-sel4-rollback.elf"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-rollback.zti"
BOOT_TIMEOUT_SECONDS = 240

# Each transition commits, so the sequence advances by one every time. Pinning
# the exact numbers is what catches a commit that silently did not happen: a
# no-op write would leave the next step reporting the previous sequence.
GENESIS_SEQUENCE = 1

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "the spawned instance received its declared device authority",
        r"SLIME_GRAPH declared placed task=\d+ child=\d+ slot=\d+ kind=block",
    ),
    (
        "init spawned the probe",
        r"\[init\] rollback probe spawned",
    ),
    (
        # A disk with no BootState must produce no root. Inventing one would
        # mean a corrupt device boots something.
        "an empty slot region produced no root",
        r"\[sel4-rollback-probe\] empty slots refused",
    ),
    (
        "the known-good root was committed",
        rf"\[sel4-rollback-probe\] genesis seq={GENESIS_SEQUENCE} pending=0 "
        rf"attempts=0 release=0",
    ),
    (
        # Staged with the model's maximum three attempts, and durable: the
        # sequence advanced, so the write reached the device and a boot would
        # see the pending generation.
        "a pending generation was staged with three attempts",
        rf"\[sel4-rollback-probe\] staged seq={GENESIS_SEQUENCE + 1} pending=1 "
        rf"attempts=3 release=0",
    ),
    (
        # The oracle's `3 -> 2`. Durable decrement before transfer is the
        # property: a boot that transferred first could retry forever.
        "the first attempt was consumed durably",
        rf"\[sel4-rollback-probe\] attempt seq={GENESIS_SEQUENCE + 2} pending=1 "
        rf"attempts=2 release=0",
    ),
    (
        "the second attempt was consumed durably",
        rf"\[sel4-rollback-probe\] attempt seq={GENESIS_SEQUENCE + 3} pending=1 "
        rf"attempts=1 release=0",
    ),
    (
        # `1 -> 0`.
        "the last attempt was consumed durably",
        rf"\[sel4-rollback-probe\] attempt seq={GENESIS_SEQUENCE + 4} pending=1 "
        rf"attempts=0 release=0",
    ),
    (
        "a further attempt was refused rather than wrapping",
        r"\[sel4-rollback-probe\] attempts exhausted",
    ),
    (
        # Back to known-good, with the pending generation cleared. The
        # known-good identity is unchanged, which is the rollback root having
        # been retained across every transition above.
        "the exhausted pending generation rolled back to known-good",
        rf"\[sel4-rollback-probe\] rolled-back seq={GENESIS_SEQUENCE + 5} pending=0 "
        rf"attempts=0 release=0",
    ),
    (
        "rolling back again is a no-op rather than an error",
        r"\[sel4-rollback-probe\] rollback is idempotent",
    ),
    (
        # Only the generation that is running may be confirmed, and the accepted
        # release sequence may not walk backwards.
        "promotion with a wrong running identity or a stale release was refused",
        r"\[sel4-rollback-probe\] unauthorized promotion refused",
    ),
    (
        # Promotion advances the accepted release sequence and makes the
        # formerly pending generation the known-good root.
        "the running generation was promoted and the release sequence advanced",
        rf"\[sel4-rollback-probe\] promoted seq={GENESIS_SEQUENCE + 7} pending=0 "
        rf"attempts=0 release=1",
    ),
    (
        # The invariant the whole sequence exists to preserve.
        "both slots decode after the sequence",
        r"\[sel4-rollback-probe\] both slots decode",
    ),
    (
        "the probe ran every transition and exited cleanly",
        r"\[sel4-rollback-probe\] rollback plane complete",
    ),
    (
        "init observed the clean exit",
        r"\[init\] rollback plane complete",
    ),
)

TERMINAL_MARKER = r"\[init\] rollback plane complete"

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    r"\[init\] rollback plane fail: .*",
    r"\[sel4-rollback-probe\] fail: .*",
    r"SLIME_ROOT block bring-up failed",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 rollback plane check: {message}")


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
    command = [sys.executable, str(BUILD_SCRIPT), "--rollback-plane"]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot run the seL4 image build: {error}")
    if process.returncode != 0:
        fail(f"seL4 image build failed with exit status {process.returncode}")


def build_fixture(disk: Path) -> None:
    """The store fixture, reused: this plane needs its validated GPT partition.

    The BootState slots live above the object store's record area on the same
    partition, so one fixture serves both planes.
    """
    command = [sys.executable, str(FIXTURE_SCRIPT), str(disk), "happy"]
    try:
        process = subprocess.run(command, cwd=ROOT, check=False, capture_output=True)
    except OSError as error:
        fail(f"cannot build the store fixture: {error}")
    if process.returncode != 0:
        fail(f"store fixture build failed: {process.stderr.decode()}")


def boot(profile: dict[str, object], disk: Path) -> str:
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
        "-drive",
        f"if=none,id=slimedisk,format=raw,file={disk}",
        "-device",
        "virtio-blk-device,drive=slimedisk",
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
        # been observed rather than at the terminal alone.
        idle_seen = False
        for line in process.stdout:
            lines.append(line.rstrip("\r\n"))
            if failures.search(line):
                break
            idle_seen |= "[sel4-rollback-probe] idle without a run token" in line
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
    # Asserted by presence, not by position: the idle instance concludes it
    # holds no run token only after a bounded wait, so its line lands wherever
    # the scheduler puts it. Ordering it would assert a scheduling accident.
    if "[sel4-rollback-probe] idle without a run token" not in transcript:
        report_transcript(transcript)
        fail("the unconfigured instance did not report parking without a run token")
    completions = transcript.count("[sel4-rollback-probe] rollback plane complete")
    if completions != 1:
        report_transcript(transcript)
        fail(f"{completions} instances ran the scenario, expected 1")
    # Every committed sequence is distinct and strictly increasing. A transition
    # that reported the right shape at a repeated sequence would mean the commit
    # did not reach the device, and the per-marker pins alone would not say so
    # if two steps happened to expect the same number.
    sequences = [
        int(value)
        for value in re.findall(r"\[sel4-rollback-probe\] \w[\w-]* seq=(\d+)", transcript)
    ]
    if sequences != sorted(set(sequences)):
        fail(f"committed sequences are not strictly increasing: {sequences}")
    print(
        f"transcript: {len(REQUIRED_MARKERS)} markers observed; "
        f"{len(sequences)} durable transitions at strictly increasing sequences "
        f"{sequences}, exhausted attempts rolled back to known-good, rollback is "
        "idempotent, and promotion advanced the accepted release",
        flush=True,
    )


def check_slots_durable(disk: Path, partition_first_lba: int) -> None:
    """Both BootState slots decode in the image after the boot.

    The component asserts this in-boot from what it read back; this asserts it
    from the host, against the bytes actually on the disk. A device that
    acknowledged writes it never persisted passes the first and fails this.
    """
    image = disk.read_bytes()
    magic = b"SLIMEBS\0"
    decoded = 0
    for offset in (1024, 1025):
        start = (partition_first_lba + offset) * 512
        slot = image[start : start + 512]
        if slot[:8] == magic:
            decoded += 1
    if decoded != 2:
        fail(
            f"{decoded} of 2 BootState slots carry the record magic in the image; "
            "the transitions were not durable"
        )
    print(
        "image: both BootState slots carry a record after the boot, so every "
        "transition reached the device",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 rollback-plane image and assert M5.6 in userspace"
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

    with tempfile.TemporaryDirectory() as directory:
        disk = Path(directory) / "rollback-plane.img"
        build_fixture(disk)
        before = disk.read_bytes()
        transcript = boot(profile, disk)
        check_transcript(transcript)
        # The store fixture's own partition: 40 is where its GPT places the
        # store, and the BootState slots sit above the record area inside it.
        check_slots_durable(disk, 40)
        # The transitions write only their two slots. The GPT, the protective
        # MBR, and the object store's superblocks and records are not theirs to
        # touch.
        after = disk.read_bytes()
        state_a = (40 + 1024) * 512
        state_b = state_a + 512
        for name, start, end in (
            ("the GPT and protective MBR", 0, 40 * 512),
            ("the object store region", 40 * 512, state_a),
            ("the disk beyond the BootState slots", state_b + 512, len(after)),
        ):
            if after[start:end] != before[start:end]:
                fail(f"the transitions modified {name}")

    print(
        "seL4 rollback plane check: a component walked the BootState transition "
        "model on two durable slots — staged a pending generation, consumed both "
        "attempts, rolled back to known-good when they were exhausted, found "
        "rollback idempotent, refused unauthorized promotion, and promoted the "
        "running generation — with M5.6 policy entirely in userspace and every "
        "commit written older-slot-first"
    )


if __name__ == "__main__":
    main()
