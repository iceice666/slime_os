#!/usr/bin/env python3

"""P5.4.2c gate: M5.9's recovery reconstruction, in userspace (M5.9).

Recovery is what happens when both BootState slots are gone. M5.9's exit
condition is that reconstruction produces a *verified* bootable root "without
modifying any device not named by an explicit capability", so this gate proves
both halves.

The rebuild: both slots are corrupt so selection refuses; a signed recovery
index decodes; every state object it names is retrieved from the object store
with its payload re-hashed; the reconstructed root is written to both slots at
sequences 1 and 2, each flushed; and the result is re-selected off the device
and must be the index's target. Running it twice must converge.

The containment: a **second disk** is attached and exposed only through a
read-only block capability. The component tries to write it, the root refuses
on rights, and the gate hashes the guard image before and after. That comparison
is the assertion M5.9 names — a serial marker alone cannot prove no mutation.
"""

from __future__ import annotations

import argparse
import hashlib
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

from harness import GENERATION_COMPOSITIONS, profile_text, profile_integer  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE_SCRIPT = ROOT / "scripts" / "build" / "build-store-fixture.py"
IMAGE = ROOT / "build" / "slime-sel4-recovery.elf"
FIXTURE = GENERATION_COMPOSITIONS / "sel4-recovery.zti"
BOOT_TIMEOUT_SECONDS = 240

# The guard disk: attached and granted read-only. Signed so a comparison
# distinguishes "unchanged" from "both empty".
GUARD_BYTES = 1 << 20
GUARD_SIGNATURE = b"GUARDDSK"

# Sequence 2 is the higher of the two slots recovery writes, so it is what a
# fresh boot selects. Release 3 is the fixture's accepted release sequence.
RECONSTRUCTED_SEQUENCE = 2
RECONSTRUCTED_RELEASE = 3
# The fixture's state closure: the store's seeded object.
CLOSURE_OBJECTS = 1

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "the spawned instance received its declared device authority",
        r"SLIME_GRAPH declared placed task=\d+ child=\d+ slot=\d+ kind=block",
    ),
    (
        "init spawned the probe",
        r"\[init\] recovery probe spawned",
    ),
    (
        # Both slots present and bad, not merely absent: selection must refuse
        # rather than treat the region as empty. Nothing is executed on an
        # unverified root, which is the state recovery exists for.
        "two corrupt slots produced no root",
        r"\[sel4-recovery-probe\] dual corruption refused",
    ),
    (
        # Bounds, ascending binding order, and a content-addressed state root
        # over every binding — checked before any read the index describes.
        "the recovery index decoded with its declared closure",
        rf"\[sel4-recovery-probe\] index states={CLOSURE_OBJECTS} "
        rf"release={RECONSTRUCTED_RELEASE}",
    ),
    (
        # Every named object retrieved and re-hashed. A closure with a missing
        # or corrupted object fails here, before anything is written.
        "every state object in the closure verified against the store",
        rf"\[sel4-recovery-probe\] closure verified objects={CLOSURE_OBJECTS} "
        rf"of={CLOSURE_OBJECTS}",
    ),
    (
        # Re-selected off the device rather than assumed from what was written.
        "the reconstructed root is what a fresh boot would select",
        rf"\[sel4-recovery-probe\] reconstructed seq={RECONSTRUCTED_SEQUENCE} "
        rf"release={RECONSTRUCTED_RELEASE}",
    ),
    (
        # Two writes, not one: an interruption after the first still leaves a
        # fully verified root.
        "reconstruction left both slots decodable",
        r"\[sel4-recovery-probe\] both slots decode",
    ),
    (
        "rerunning recovery from the durable index converged",
        r"\[sel4-recovery-probe\] recovery rerun from durable index converged",
    ),
    (
        # A real read-only block capability names the guard disk; the attempted
        # write must be rejected by rights, not by an unrelated endpoint kind.
        "the reachable guard disk refused a write",
        r"\[sel4-recovery-probe\] reachable guard disk write refused",
    ),
    (
        "the probe ran every arm and exited cleanly",
        r"\[sel4-recovery-probe\] recovery plane complete",
    ),
    (
        "init observed the clean exit",
        r"\[init\] recovery plane complete",
    ),
)

TERMINAL_MARKER = r"\[init\] recovery plane complete"

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    r"\[init\] recovery plane fail: .*",
    r"\[sel4-recovery-probe\] fail: .*",
    r"SLIME_ROOT block bring-up failed",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)

def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 recovery plane check: {message}")


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
    command = [sys.executable, str(BUILD_SCRIPT), "--recovery-plane"]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot run the seL4 image build: {error}")
    if process.returncode != 0:
        fail(f"seL4 image build failed with exit status {process.returncode}")


def build_fixture(disk: Path) -> None:
    """The `recovery` fixture: a valid store, two corrupt BootState slots, and a
    signed recovery index naming the seeded object as the state closure."""
    command = [sys.executable, str(FIXTURE_SCRIPT), str(disk), "recovery"]
    try:
        process = subprocess.run(command, cwd=ROOT, check=False, capture_output=True)
    except OSError as error:
        fail(f"cannot build the store fixture: {error}")
    if process.returncode != 0:
        fail(f"store fixture build failed: {process.stderr.decode()}")


def boot(profile: dict[str, object], disk: Path, guard: Path) -> str:
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
        # The guard disk. Attached to the machine and named only by a read-only
        # capability, so the attempted write reaches the correct object and is
        # refused on authority. The image comparison proves containment.
        "-drive",
        f"if=none,id=guarddisk,format=raw,file={guard}",
        "-device",
        "virtio-blk-device,drive=guarddisk",
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
            idle_seen |= "[sel4-recovery-probe] idle without a run token" in line
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
    if "[sel4-recovery-probe] idle without a run token" not in transcript:
        report_transcript(transcript)
        fail("the unconfigured instance did not report parking without a run token")
    completions = transcript.count("[sel4-recovery-probe] recovery plane complete")
    if completions != 1:
        report_transcript(transcript)
        fail(f"{completions} instances ran the scenario, expected 1")
    print(
        f"transcript: {len(REQUIRED_MARKERS)} markers observed; two corrupt slots "
        f"refused, a signed index decoded, {CLOSURE_OBJECTS} state object verified "
        f"against the store, and a reconstructed root selected at sequence "
        f"{RECONSTRUCTED_SEQUENCE}",
        flush=True,
    )


def check_reconstructed(disk: Path, partition_first_lba: int) -> None:
    """Both slots carry a BootState record in the image after the boot."""
    image = disk.read_bytes()
    magic = b"SLIMEBS\0"
    present = sum(
        1
        for offset in (1024, 1025)
        if image[
            (partition_first_lba + offset) * 512 : (partition_first_lba + offset) * 512 + 8
        ]
        == magic
    )
    if present != 2:
        fail(f"{present} of 2 slots carry a record after reconstruction")
    print(
        "image: both BootState slots carry a record, so reconstruction left "
        "redundancy rather than a single root",
        flush=True,
    )


def check_guard_untouched(guard: Path, before: str) -> None:
    """The read-only guard device is byte-identical.

    This is M5.9's containment requirement. A serial refusal is insufficient
    without comparing the reachable device before and after.
    """
    after = hashlib.sha256(guard.read_bytes()).hexdigest()
    if after != before:
        fail(
            "the read-only guard disk changed during recovery: "
            f"{before[:16]} -> {after[:16]}"
        )
    if guard.read_bytes()[: len(GUARD_SIGNATURE)] != GUARD_SIGNATURE:
        fail("the guard disk lost its signature")
    print(
        "guard: the second disk, reachable only through a read-only capability, "
        "is byte-identical after the boot",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 recovery-plane image and assert M5.9 in userspace"
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
        disk = Path(directory) / "recovery-plane.img"
        build_fixture(disk)
        before = disk.read_bytes()

        guard = Path(directory) / "guard.img"
        guard_image = bytearray(GUARD_BYTES)
        guard_image[: len(GUARD_SIGNATURE)] = GUARD_SIGNATURE
        guard.write_bytes(bytes(guard_image))
        guard_digest = hashlib.sha256(guard.read_bytes()).hexdigest()

        transcript = boot(profile, disk, guard)
        check_transcript(transcript)
        check_reconstructed(disk, 40)
        check_guard_untouched(guard, guard_digest)

        # Reconstruction writes the two BootState slots and nothing else. The
        # GPT, the object store's superblocks and records, and the recovery
        # index it read are not its to modify.
        after = disk.read_bytes()
        slot_a = (40 + 1024) * 512
        slot_b = slot_a + 512
        for name, start, end in (
            ("the GPT and protective MBR", 0, 40 * 512),
            ("the object store region", 40 * 512, slot_a),
            ("the recovery index and beyond", slot_b + 512, len(after)),
        ):
            if after[start:end] != before[start:end]:
                fail(f"reconstruction modified {name}")

    print(
        "seL4 recovery plane check: a component refused two corrupt BootState "
        "slots, decoded a signed recovery index, verified its whole state "
        "closure against the content-addressed store, reconstructed a bootable "
        "root into both slots idempotently, and left an attached disk that no "
        "capability names byte-identical"
    )


if __name__ == "__main__":
    main()
