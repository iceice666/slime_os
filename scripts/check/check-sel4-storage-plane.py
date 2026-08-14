#!/usr/bin/env python3

"""P5.4.2c gate: a userspace component reaches a real disk (M5.2, M5.3).

`just sel4_device_check` proves the root can drive a virtio block device.
This gate proves the layer that matters: a *component* moving sectors through
nothing but a capability its generation granted it, mediated by
`BlockTransact`.

Six arms, and each fails differently:

* a read returns the fixture's own signature, so the sector crossed rather than
  being fabricated;
* a write, a flush, and a read-back agree byte for byte, and the write is
  confirmed durable in the host image after the boot;
* a sector past the device's capacity is refused;
* a malformed request is refused before any sector moves;
* a slot holding no block capability is refused;
* the root-launched instance of the same component, holding the same device
  capability, parks — so the arms above are the spawned instance's.

That last one is the authority claim. Both copies of the component hold the
block capability, because a generation grant names a component rather than a
task; what distinguishes them is a run token only `init` hands to the instance
it spawns.
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

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
IMAGE = ROOT / "build" / "slime-sel4-storage.elf"
MANIFEST = ROOT / "build" / "slime-sel4-storage.identity.json"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-storage.zti"
IMAGE_VARIANT = "storage"
BOOT_TIMEOUT_SECONDS = 180

DISK_BYTES = 1 << 20
# Written at sector 0 by the fixture, and required back from the read.
DISK_SIGNATURE = b"SLIMEDSK"
# Written at sector 1 by the probe, and required in the image afterwards.
SCRATCH_LBA = 1
SCRATCH_SIGNATURE = b"SLIMEWR1"
SECTOR_BYTES = 512

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        # The spawned instance's own block capability, placed by the root above
        # the parent's grants. Before P5.4.2c a spawned child received only what
        # its parent handed it, so this line did not exist and the arms below
        # could not run.
        "the spawned instance received its declared device authority",
        r"SLIME_GRAPH declared placed task=\d+ child=\d+ slot=\d+ kind=block",
    ),
    (
        "init spawned the probe",
        r"\[init\] storage probe spawned",
    ),
    (
        # M5.2: a read through the capability, returning the fixture's bytes.
        "a sector was read through the capability and carries the fixture's bytes",
        r"\[sel4-storage-probe\] sector 0 verified",
    ),
    (
        # M5.3: write, flush, and a read-back compared byte for byte.
        "a sector was written, flushed, and read back identical",
        r"\[sel4-storage-probe\] write flushed and verified",
    ),
    (
        "a sector past the device's capacity was refused",
        r"\[sel4-storage-probe\] out-of-range refused",
    ),
    (
        "a malformed request was refused",
        r"\[sel4-storage-probe\] malformed refused",
    ),
    (
        "a slot holding no block capability was refused",
        r"\[sel4-storage-probe\] ungranted slot refused",
    ),
    (
        "the probe ran every arm and exited cleanly",
        r"\[sel4-storage-probe\] storage plane complete",
    ),
    (
        "init observed the clean exit",
        r"\[init\] storage plane complete",
    ),
)

TERMINAL_MARKER = r"\[init\] storage plane complete"

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    r"\[init\] storage plane fail: .*",
    r"\[sel4-storage-probe\] fail: .*",
    r"SLIME_ROOT block bring-up failed",
    r"SLIME_ROOT block read failed",
    r"SLIME_ROOT block write failed",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 storage plane check: {message}")


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
    command = [sys.executable, str(BUILD_SCRIPT), "--storage-plane"]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot run the seL4 image build: {error}")
    if process.returncode != 0:
        fail(f"seL4 image build failed with exit status {process.returncode}")


def boot(profile: dict[str, object], disk: Path) -> str:
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
            idle_seen |= "[sel4-storage-probe] idle without a run token" in line
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
        fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without completing the storage plane")
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
    if "[sel4-storage-probe] idle without a run token" not in transcript:
        report_transcript(transcript)
        fail("the unconfigured instance did not report parking without a run token")
    # Exactly one instance ran the scenario. Two would mean the run-token
    # discrimination failed and both copies raced on the scratch sector.
    completions = transcript.count("[sel4-storage-probe] storage plane complete")
    if completions != 1:
        report_transcript(transcript)
        fail(f"{completions} instances ran the scenario, expected 1")
    # The root's own record of what it served, so the component's claims are
    # corroborated by the mediation rather than only self-reported.
    served = re.findall(r"SLIME_GRAPH block served task=\d+ device=\d+ op=(\d+) lba=\d+ status=(-?\d+)", transcript)
    if not any(op == "1" and status == "0" for op, status in served):
        fail("the root served no successful read")
    if not any(op == "2" and status == "0" for op, status in served):
        fail("the root served no successful write")
    if not any(op == "3" and status == "0" for op, status in served):
        fail("the root served no successful flush")
    print(
        f"transcript: {len(REQUIRED_MARKERS)} markers observed; a component read, wrote, "
        "flushed, and verified a sector through a granted block capability, and "
        "three refusal arms held",
        flush=True,
    )


def check_durability(disk: Path) -> None:
    """The write reached the image, not just the device's cache.

    Read after the boot, from the host side. A flush the device acknowledged but
    never honoured would pass every in-boot assertion and fail here.
    """
    image = disk.read_bytes()
    start = SCRATCH_LBA * SECTOR_BYTES
    written = image[start : start + len(SCRATCH_SIGNATURE)]
    if written != SCRATCH_SIGNATURE:
        fail(
            f"sector {SCRATCH_LBA} holds {written!r} after the boot, "
            f"expected {SCRATCH_SIGNATURE!r}"
        )
    if image[: len(DISK_SIGNATURE)] != DISK_SIGNATURE:
        fail("the fixture's own sector 0 was modified")
    print(
        f"image: sector 0 still holds {DISK_SIGNATURE!r} and sector {SCRATCH_LBA} "
        f"holds {SCRATCH_SIGNATURE!r}, so the flushed write is durable",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 storage-plane image and assert M5.2/M5.3"
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
        disk = Path(directory) / "storage-plane.img"
        image = bytearray(DISK_BYTES)
        image[: len(DISK_SIGNATURE)] = DISK_SIGNATURE
        disk.write_bytes(bytes(image))
        transcript = boot(profile, disk)
        check_transcript(transcript)
        check_durability(disk)
    print(
        "seL4 storage plane check: a userspace component read, wrote, flushed, and "
        "verified sectors on a real device through a capability its generation "
        "granted, three refusal arms held, and the write survived to the image"
    )


if __name__ == "__main__":
    main()
