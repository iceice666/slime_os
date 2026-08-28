#!/usr/bin/env python3
"""IO2 gate: userspace virtio-blk parity, async settlement, and reclamation."""
from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import tempfile
import threading
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from harness import load_qemu_profile, profile_integer, profile_text, sha256_file  # noqa: E402
from sel4_gate_markers import match_marker_contract  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4-io-block.elf"
MANIFEST = ROOT / "build" / "slime-sel4-io-block.identity.json"
FIXTURE = ROOT / "contracts" / "generation-manifest" / "v1" / "compositions" / "sel4-io-block.zti"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
IMAGE_VARIANT = "io-block"
TIMEOUT = 300
SECTOR_BYTES = 512
DISK_BYTES = 1 << 20
FRESH_LBA = 3
FRESH_MARKER = b"SLIMEIO2"

CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("generation and driver authority", (
        r"SLIME_ROOT generation admitted number=51 ",
        r"\[virtio-blk-driver\] mmio mechanism=mediated-bounded-read32-write32",
        r"\[virtio-blk-driver\] ready capacity=\d+ epoch=\d+",
    )),
    ("oracle parity", (
        r"\[io-block-probe\] parity read write flush geometry rights out-of-range malformed short-buffer unsupported=match",
        r"\[io-block-probe\] durable fresh-boot readback verified",
    )),
    ("bounded asynchronous identity", (
        r"\[io-block-probe\] backpressure full refused overwrite=0",
        r"\[io-block-probe\] async queued=8 completed=8 identities=8 overwrite=0",
    )),
    ("all injected terminal causes reclaim exactly", (
        r"\[io-block-probe\] descriptor-failure settled=8 descriptors=0 dma=0 leases=0 charges=0",
        r"\[io-block-probe\] timeout settled=8 descriptors=0 dma=0 leases=0 charges=0",
        r"\[io-block-probe\] cancellation settled=8 descriptors=0 dma=0 leases=0 charges=0",
        r"\[io-block-probe\] reset settled=8 descriptors=0 dma=0 leases=0 charges=0",
        r"\[io-block-probe\] interrupt-loss-coalescing settled=8 descriptors=0 dma=0 leases=0 charges=0",
        r"\[io-block-probe\] driver-crash settled=8 descriptors=0 dma=0 leases=0 charges=0",
        r"\[io-block-probe\] peer-death settled=8 descriptors=0 dma=0 leases=0 charges=0",
    )),
    ("fresh epoch rejects stale completion", (
        r"\[io-block-probe\] restarted old_epoch=\d+ fresh_epoch=\d+",
        r"\[io-block-probe\] stale completion refused buffer_unchanged=1 request_live=1",
        r"\[io-block-probe\] io block plane complete",
        r"SLIME_GRAPH HEALTHY generation=51 required=3 live=0 completed=3 failed=0",
    )),
)

FAILURE_MARKERS = (
    r"SLIME_ROOT FATAL", r"SLIME_GRAPH FAIL", r"\[virtio-blk-driver\] fail: ",
    r"\[io-block-probe\] fail: ", r"Caught cap fault", r"Caught vm fault", r"panicked at ",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 I/O block plane check: {message}")


def build_image() -> None:
    result = subprocess.run([sys.executable, str(BUILD_SCRIPT), "--io-block-plane"], cwd=ROOT, check=False)
    if result.returncode != 0:
        fail(f"image build failed with exit status {result.returncode}")


def check_manifest() -> None:
    if not IMAGE.is_file() or not MANIFEST.is_file():
        fail("image or identity manifest missing")
    identity = json.loads(MANIFEST.read_text(encoding="utf-8"))
    if identity.get("variant") != IMAGE_VARIANT:
        fail(f"wrong image variant {identity.get('variant')!r}")
    image = identity.get("image")
    if not isinstance(image, dict) or image.get("sha256") != sha256_file(IMAGE, fail):
        fail("packaged image digest does not match identity manifest")


def boot(profile: dict[str, object], disk: Path) -> str:
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    command = [qemu, "-machine", profile_text(profile, "machine", fail), "-cpu", profile_text(profile, "cpu", fail),
               "-smp", str(profile_integer(profile, "cpus", fail)), "-m", f"size={profile_integer(profile, 'memory_mib', fail)}M",
               "-nographic", "-serial", "mon:stdio", "-kernel", str(IMAGE), "-drive",
               f"if=none,id=slimeio2,format=raw,file={disk}", "-device", "virtio-blk-device,drive=slimeio2"]
    process = subprocess.Popen(command, cwd=ROOT, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, text=True, bufsize=1)
    watchdog = threading.Timer(TIMEOUT, process.kill)
    watchdog.start()
    terminal = re.compile(CHAINS[-1][1][-1] + "|" + "|".join(FAILURE_MARKERS))
    lines: list[str] = []
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\r\n"))
            if terminal.search(line):
                break
    finally:
        timed_out = not watchdog.is_alive()
        watchdog.cancel()
        process.terminate()
        try: process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill(); process.wait()
    transcript = "\n".join(lines)
    if timed_out and re.search(CHAINS[-1][1][-1], transcript) is None:
        fail("QEMU timed out before terminal cleanup")
    return transcript


def check_fixture() -> None:
    text = FIXTURE.read_text(encoding="utf-8")
    for declaration in ('generation = 51;', 'name = "virtio-blk-driver";', 'name = "io-block-probe";',
                        'capabilityKind = "device";', 'capabilityKind = "mmioRegion";',
                        'capabilityKind = "interruptSource";', 'capabilityKind = "dmaAccount";'):
        if declaration not in text:
            fail(f"fixture is missing {declaration!r}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--no-build", action="store_true")
    args = parser.parse_args()
    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    check_fixture()
    if not args.no_build:
        build_image()
    check_manifest()
    profile = load_qemu_profile(fail, PINS)
    with tempfile.TemporaryDirectory(prefix="slime-io-block-") as temporary:
        disk = Path(temporary) / "disk.img"
        image = bytearray(DISK_BYTES)
        image[FRESH_LBA * SECTOR_BYTES:FRESH_LBA * SECTOR_BYTES + len(FRESH_MARKER)] = FRESH_MARKER
        disk.write_bytes(image)
        transcript = boot(profile, disk)
        match_marker_contract(transcript, CHAINS, FAILURE_MARKERS, fail)
        if disk.read_bytes()[FRESH_LBA * SECTOR_BYTES:FRESH_LBA * SECTOR_BYTES + len(FRESH_MARKER)] != FRESH_MARKER:
            fail("durable marker changed unexpectedly")
    print("seL4 I/O block plane check: oracle parity, async identity, faults, reclamation, and stale epoch proved")


if __name__ == "__main__":
    main()
