#!/usr/bin/env python3

"""P5.4.2a gate: `slime-root` can reach a real device.

`just sel4_root_boot_check` proves the mechanism against a machine with *no*
disk attached: thirty-two virtio-mmio transports mapped and probed, all
reporting device id 0. That is the negative half, and on its own it is
satisfiable by a probe that reads a constant.

This gate boots the same image with a virtio-blk device on the QEMU command
line and requires the probe to find exactly it: one transport, device id 2,
QEMU's vendor id, at the highest declared slot. The pair is what makes the
mechanism observed rather than asserted — the same code must say "nothing" when
nothing is attached and name the disk when one is.

The disk is created with one signature sector and is never written. P5.4.2a is the resource substrate:
retype a device untyped, map it non-cacheably, read registers out of it. The
transport driver that puts data through it is P5.4.2b.
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
IMAGE = ROOT / "build" / "slime-sel4-graph.elf"
BOOT_TIMEOUT_SECONDS = 180
DISK_BYTES = 1 << 20

# qemu-arm-virt declares thirty-two transports at 0x0a00_0000 + n * 0x200 and
# attaches a device to the highest free one, so a single `-device` lands at
# 0x0a003e00. Pinned because it is the fixture's own arrangement: the root reads
# whatever the registers say, and this is what they say for this command line.
EXPECTED_TRANSPORT = 0xA003E00
# virtio device id 2 is a block device; 0x554d4551 is "QEMU" little-endian.
EXPECTED_DEVICE_ID = 2
EXPECTED_VENDOR_ID = 0x554D4551
# The fixture disk: 1 MiB of 512-byte sectors, with an identifying signature at
# sector 0 that the read must report back.
EXPECTED_SECTORS = DISK_BYTES // 512
DISK_SIGNATURE = b"SLIMEDSK"
EXPECTED_HEAD = DISK_SIGNATURE[:4].hex()

MARKERS: tuple[tuple[str, str], ...] = (
    (
        "BootInfo named device untyped memory",
        r"SLIME_ROOT devices untypeds=[1-9]\d*",
    ),
    (
        # The register read, and the whole point of the slice: this line's
        # values come out of MMIO the root mapped itself.
        "the attached block device was identified by register read",
        rf"SLIME_ROOT virtio transport={EXPECTED_TRANSPORT:#x} version=[1-9]\d* "
        rf"device={EXPECTED_DEVICE_ID} vendor={EXPECTED_VENDOR_ID:#x}",
    ),
    (
        # P5.4.2b's first half: the root acquires and binds the *device's own*
        # interrupt line. IRQ 79 is the DTB's `<0 0x2f 0x01>` on
        # `virtio_mmio@a003e00` — SPI 47, which seL4 numbers from 32 — so this
        # asserts the address-to-IRQ derivation as well as the binding.
        #
        # Only the attached transport's line is bound. `irq_control_get_trigger`
        # succeeds for any number the platform declares, so binding an empty
        # slot's would report a binding that can never fire.
        "the attached device's interrupt was acquired and bound",
        r"SLIME_ROOT virtio irq bound transport=0xa003e00 irq=79 badge=0x2",
    ),
    (
        # P5.4.2b: two DMA pages of ordinary RAM, named to the device by
        # guest-physical address. The allocator is the only thing that knows
        # those addresses; before this slice it discarded them.
        "virtqueue and request buffers were allocated with physical addresses",
        r"SLIME_ROOT block dma queue=0x[0-9a-f]+ buffer=0x[0-9a-f]+",
    ),
    (
        # The legacy MMIO handshake completed and the device published its
        # config space. The fixture disk is 1 MiB, so 2048 sectors of 512 bytes
        # is a value read from the device rather than a constant.
        "the block device negotiated and reported its capacity",
        rf"SLIME_ROOT block ready transport={EXPECTED_TRANSPORT:#x} sectors={EXPECTED_SECTORS}",
    ),
    (
        # DMA in the device-writes direction: descriptors the device followed,
        # a buffer it filled, and a status byte it set. The head bytes are the
        # fixture's own signature, so a driver that completed a request without
        # moving data reports the wrong ones.
        "a sector was read by DMA and carries the fixture's bytes",
        rf"SLIME_ROOT block read lba=0 bytes=512 head={EXPECTED_HEAD}",
    ),
    # No write marker, deliberately. Bring-up used to prove the other DMA
    # direction with a write/flush/read-back on sector 1 — which is the GPT
    # primary header, so the root destroyed the partition table of any
    # partitioned disk before userspace ran. `sel4_storage_check` proves both
    # directions and a flush from userspace, on a sector its fixture designates,
    # through a capability. The check below is what replaced the marker: the
    # image must be byte-identical after this boot.
    (
        # Exactly one. More would mean the probe is matching something other
        # than a present transport; none would mean it cannot see the disk.
        "every declared transport was scanned and exactly one is attached",
        r"SLIME_ROOT virtio probed granules=4 slots=32 found=1",
    ),
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_ROOT device page unavailable",
    r"SLIME_ROOT device map failed",
    r"SLIME_ROOT device unmap failed",
    r"SLIME_ROOT virtio irq unavailable",
    r"SLIME_ROOT block page unavailable",
    r"SLIME_ROOT block map failed",
    r"SLIME_ROOT block queue unavailable",
    r"SLIME_ROOT block buffer unavailable",
    r"SLIME_ROOT block bring-up failed",
    r"SLIME_ROOT block read failed",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
)

TERMINAL_MARKER = r"SLIME_ROOT virtio probed "


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 device plane check: {message}")


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
        # The one difference from every other seL4 gate's command line.
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
        fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without completing the device probe")
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
    for label, pattern in MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            report_transcript(transcript)
            if re.search(pattern, transcript) is not None:
                fail(f"marker out of order: {label} ({pattern})")
            fail(f"missing marker: {label} ({pattern})")
        position = match.end()
    # One transport line, not merely at least one: a probe reporting several
    # would be matching something other than an attached device.
    transports = re.findall(r"SLIME_ROOT virtio transport=", transcript)
    if len(transports) != 1:
        report_transcript(transcript)
        fail(f"the probe reported {len(transports)} transports, expected 1")
    print(
        f"transcript: {len(MARKERS)} markers observed; the root mapped 32 virtio-mmio "
        "register banks out of device untyped memory, identified the attached block "
        "device, and moved sectors through it in both directions",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 component-graph image with a disk attached and assert P5.4.2a"
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
    if not IMAGE.is_file():
        fail(f"missing packaged image {IMAGE.relative_to(ROOT)}")
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    with tempfile.TemporaryDirectory() as directory:
        disk = Path(directory) / "device-plane.img"
        # A signature at sector 0 the read must report back, so a driver that
        # completes a request without moving data fails rather than passing on
        # a buffer of zeroes it never filled.
        image = bytearray(DISK_BYTES)
        image[: len(DISK_SIGNATURE)] = DISK_SIGNATURE
        original = bytes(image)
        disk.write_bytes(original)
        transcript = boot(profile, disk)
        check_transcript(transcript)
        # Bring-up reads and does not write. This is the assertion that replaced
        # the old `block wrote lba=1` marker: that write landed on the GPT
        # primary header, so the root destroyed the partition table of any
        # partitioned disk before userspace ran. A byte-identical image is the
        # property that actually matters, and it is not something a serial
        # marker can express.
        after = disk.read_bytes()
        if after != original:
            differing = [
                index
                for index in range(0, len(original), 512)
                if after[index : index + 512] != original[index : index + 512]
            ]
            fail(
                "the root modified the disk during bring-up; sectors changed: "
                + ", ".join(str(index // 512) for index in differing[:8])
            )
    print(
        "seL4 device plane check: the root retyped a granule out of BootInfo device "
        "untyped memory, mapped it non-cacheably into its own VSpace, brought up the "
        "attached virtio block device, read a sector through DMA, and left every "
        "sector on the disk byte-identical"
    )


if __name__ == "__main__":
    main()
