#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import importlib.util
import os
import struct
import shutil
import subprocess
import sys
import zlib
from pathlib import Path

from boot_contracts import BOOTSTATE_SLOT_BYTES, BOOTSTORE_CAPACITY

ROOT = Path(__file__).resolve().parent.parent
SECTOR = 512
STATE_SECTORS = 128
STATE_FIRST_LBA = BOOTSTORE_CAPACITY // SECTOR
TARGET_BDF = "0x18"

CHECK_SPEC = importlib.util.spec_from_file_location(
    "check_generation", ROOT / "scripts" / "check-generation.py"
)
if CHECK_SPEC is None or CHECK_SPEC.loader is None:
    raise SystemExit("cannot load generation checker")
CHECK = importlib.util.module_from_spec(CHECK_SPEC)
CHECK_SPEC.loader.exec_module(CHECK)


def run(arguments: list[str], *, environment: dict[str, str] | None = None) -> str:
    process = subprocess.run(
        arguments,
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    sys.stdout.write(process.stdout)
    if process.returncode != 0:
        raise SystemExit(process.returncode)
    return process.stdout


def superblock(sequence: int) -> bytes:
    sector = bytearray(SECTOR)
    struct.pack_into("<8sIIQQI", sector, 0, b"SLIMESB\0", 1, 64, sequence, 2, 0)
    struct.pack_into("<QQ", sector, 40, 2, STATE_SECTORS)
    struct.pack_into("<I", sector, 60, zlib.crc32(sector[:60]))
    return bytes(sector)


def prepare_target(source: bytes, path: Path) -> None:
    image = bytearray(source)
    image.extend(bytes(STATE_SECTORS * SECTOR))
    image[STATE_FIRST_LBA * SECTOR : (STATE_FIRST_LBA + 1) * SECTOR] = superblock(2)
    image[(STATE_FIRST_LBA + 1) * SECTOR : (STATE_FIRST_LBA + 2) * SECTOR] = superblock(1)
    image[: BOOTSTATE_SLOT_BYTES * 2] = bytes(BOOTSTATE_SLOT_BYTES * 2)
    path.write_bytes(image)


def valid_states(image: Path) -> list[dict]:
    data = image.read_bytes()
    states = []
    for index in range(2):
        slot = data[index * BOOTSTATE_SLOT_BYTES : (index + 1) * BOOTSTATE_SLOT_BYTES]
        try:
            states.append(CHECK.decode_bootstate(slot))
        except SystemExit:
            pass
    return states


def main() -> None:
    kernel = ROOT / "kernel" / "target" / "x86_64-unknown-none" / "release" / "slime_os-kernel"
    build = Path("/tmp/slime-os-recovery-build")
    media = Path("/tmp/slime-os-recovery-media.img")
    target = Path("/tmp/slime-os-recovery-target.img")
    guard = Path("/tmp/slime-os-recovery-guard.img")
    for path in (media, target, guard):
        path.unlink(missing_ok=True)
    shutil.rmtree(build, ignore_errors=True)

    environment = os.environ.copy()
    environment["SLIME_RECOVERY_TARGET_BDF"] = TARGET_BDF
    run([str(ROOT / "scripts" / "build-generation.py"), str(kernel), str(build)], environment=environment)
    source = (build / "boot-store.bin").read_bytes()
    prepare_target(source, target)
    guard.write_bytes(source)
    guard_before = hashlib.sha256(source).digest()

    media_environment = environment.copy()
    media_environment["SLIME_RECOVERY_IMAGE"] = "1"
    media_environment["SLIME_GENERATION_DIR"] = str(build)
    run(
        [
            str(ROOT / "kernel" / "scripts" / "build-iso.sh"),
            str(kernel),
            str(media),
            "64",
        ],
        environment=media_environment,
    )

    boot_environment = os.environ.copy()
    boot_environment["SLIME_BOOT_IMAGE"] = str(media)
    boot_environment["SLIME_REUSE_BOOT_IMAGE"] = "1"
    output = run(
        [
            str(ROOT / "kernel" / "scripts" / "run-kernel.sh"),
            str(kernel),
            "-display",
            "none",
            "-drive",
            f"if=none,id=repair,format=raw,cache=directsync,file={target}",
            "-device",
            "virtio-blk-pci,drive=repair,disable-legacy=on,queue-size=8",
            "-drive",
            f"if=none,id=guard,format=raw,cache=directsync,file={guard}",
            "-device",
            "virtio-blk-pci,drive=guard,disable-legacy=on,queue-size=8",
        ],
        environment=boot_environment,
    )
    if "[recovery] reconstruction complete" not in output:
        raise SystemExit("recovery component did not complete reconstruction")
    states = valid_states(target)
    if len(states) != 2 or {state["sequence"] for state in states} != {1, 2}:
        raise SystemExit("recovery did not reconstruct both BootState slots")
    if any(state["known_good"] != states[0]["known_good"] for state in states[1:]):
        raise SystemExit("reconstructed slots disagree on known-good generation")
    if hashlib.sha256(guard.read_bytes()).digest() != guard_before:
        raise SystemExit("recovery modified the ungranted guard disk")
    print("recovery check: reconstructed verified BootState and preserved guard disk")


if __name__ == "__main__":
    main()
