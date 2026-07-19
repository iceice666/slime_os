#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SECTOR_SIZE = 512
DEVICE_ARGS = [
    "-display",
    "none",
    "-drive",
    "if=none,id=slime-storage,format=raw,file={image}",
    "-device",
    "virtio-blk-pci,drive=slime-storage,disable-legacy=on,queue-size=8",
]


def run_guest(image: Path, mode: int) -> str:
    arguments = [value.format(image=image) for value in DEVICE_ARGS]
    environment = os.environ.copy()
    environment["SLIME_GENERATION_NUMBER"] = str(mode + 1)
    process = subprocess.run(
        ["cargo", "run", "--", *arguments],
        cwd=ROOT / "kernel",
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


def sector_hash(image: Path, lba: int) -> str:
    with image.open("rb") as source:
        source.seek(lba * SECTOR_SIZE)
        data = source.read(SECTOR_SIZE)
    if len(data) != SECTOR_SIZE:
        raise SystemExit(f"short sector {lba} in {image}")
    return hashlib.sha256(data).hexdigest()


def write_check(image: Path) -> None:
    subprocess.run([ROOT / "scripts" / "build-storage-fixture.py", image], check=True)
    initial = sector_hash(image, 2)
    first = run_guest(image, 1)
    written = sector_hash(image, 2)
    if written == initial or "[storage-writer] durable sector verified" not in first:
        raise SystemExit("first boot did not persist the expected sector")
    second = run_guest(image, 1)
    if sector_hash(image, 2) != written or "[storage-writer] durable sector verified" not in second:
        raise SystemExit("fresh boot did not verify the persisted sector")
    print("storage write persistence check: ok")


def fault_check(image: Path) -> None:
    subprocess.run([ROOT / "scripts" / "build-storage-fixture.py", image], check=True)
    before = hashlib.sha256(image.read_bytes()).hexdigest()
    output = run_guest(image, 2)
    after = hashlib.sha256(image.read_bytes()).hexdigest()
    required = [
        "[storage-fault-probe] recovery and replay verified",
        "[block-flight] record",
        "[block-flight] replay",
    ]
    if before != after:
        raise SystemExit("fault injection changed the disposable image")
    if any(marker not in output for marker in required):
        raise SystemExit("fault check did not observe every recorder/recovery marker")
    print("storage fault recovery check: ok")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["write", "fault"])
    parser.add_argument("image", type=Path)
    arguments = parser.parse_args()
    arguments.image.unlink(missing_ok=True)
    if arguments.mode == "write":
        write_check(arguments.image)
    else:
        fault_check(arguments.image)


if __name__ == "__main__":
    main()
