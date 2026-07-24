#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import os
import subprocess
from pathlib import Path

from harness import ROOT, SECTOR_SIZE, run_qemu

DEVICE_ARGS = [
    "-display",
    "none",
    "-drive",
    "if=none,id=slime-storage,format=raw,file={image}",
    "-device",
    "virtio-blk-pci,drive=slime-storage,disable-legacy=on,queue-size=8",
]
# Store scenarios reboot a mutated image and assert host-side on sectors the
# guest did not write, so guest and host must stay coherent. `cache=directsync`
# bypasses the host writeback cache that otherwise lets a clean cached copy of
# an unwritten sector overwrite the on-disk fixture at guest teardown.
STORE_DEVICE_ARGS = [
    "-display",
    "none",
    "-drive",
    "if=none,id=slime-storage,format=raw,cache=directsync,file={image}",
    "-device",
    "virtio-blk-pci,drive=slime-storage,disable-legacy=on,queue-size=8",
]


def run_guest(image: Path, mode: int, device_args: list[str] = DEVICE_ARGS) -> str:
    arguments = [value.format(image=image) for value in device_args]
    environment = os.environ.copy()
    environment["SLIME_GENERATION_NUMBER"] = str(mode + 1)
    return run_qemu(
        ["cargo", "run", "--", *arguments],
        environment=environment,
        cwd=ROOT / "kernel",
        timeout=None,
    )


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


STORE_OPENED_HAPPY = "[store] opened partition first=40 last=2014 seq=2 objects=1"
STORE_MARKERS_HAPPY = [
    "[store-probe] stat-seeded 0",
    "[store-probe] get-seeded 0 hash-ok",
    "[store-probe] put-new 0",
    "[store-probe] get-new 0 bytes-ok",
    "[store-probe] stat-unknown -3",
    "[store-probe] done",
]
STORE_MARKERS_EMPTY_ROOT = [
    "[store-probe] stat-seeded -3",
    "[store-probe] get-seeded -3",
    "[store-probe] put-new 0",
    "[store-probe] get-new 0 bytes-ok",
    "[store-probe] stat-unknown -3",
    "[store-probe] done",
]
STORE_MARKERS_REJECTED = [
    "[store-probe] stat-seeded -7",
    "[store-probe] get-seeded -7",
    "[store-probe] put-new -7",
    "[store-probe] get-new -7",
    "[store-probe] stat-unknown -7",
    "[store-probe] done",
]


def require_markers(output: str, markers: list[str], label: str) -> None:
    missing = [marker for marker in markers if marker not in output]
    if missing:
        raise SystemExit(f"{label}: missing markers {missing}")


def store_fixture(image: Path, variant: str) -> None:
    subprocess.run(
        [ROOT / "scripts" / "build-store-fixture.py", image, variant],
        check=True,
    )


def store_check(image: Path) -> None:
    # Happy path: GPT resolves to the store partition, the seeded object
    # verifies, and a new object commits. Mode 3 selects generation 4 (the
    # store probe with the ObjectStore capability).
    store_fixture(image, "happy")
    before = image.read_bytes()
    first = run_guest(image, 3, STORE_DEVICE_ARGS)
    require_markers(first, [STORE_OPENED_HAPPY, *STORE_MARKERS_HAPPY], "happy boot")
    if image.read_bytes() == before:
        raise SystemExit("first store boot did not commit the new object")

    # Fresh boot: the committed root now carries the appended object; the
    # deduplicated put must not rewrite the store.
    committed = image.read_bytes()
    second = run_guest(image, 3, STORE_DEVICE_ARGS)
    require_markers(
        second,
        ["[store] opened partition first=40 last=2014 seq=3 objects=2", *STORE_MARKERS_HAPPY],
        "durability boot",
    )
    if image.read_bytes() != committed:
        raise SystemExit("deduplicated put rewrote the store")

    # GPT copy-recovery (one damaged header rebuilt from the other) is covered
    # by kernel/tests/object_store.rs, not here: UEFI firmware (OVMF) itself
    # auto-repairs a disk whose primary GPT header is damaged before our kernel
    # runs, so a QEMU boot cannot observe the kernel's own recovery path for a
    # damaged LBA-1 header. Superblock recovery below lives inside the
    # partition, which firmware never rewrites, so it is exercised here.

    # Conflicting valid copies are rejected, not guessed.
    store_fixture(image, "gpt-conflict")
    before = hashlib.sha256(image.read_bytes()).hexdigest()
    output = run_guest(image, 3, STORE_DEVICE_ARGS)
    require_markers(
        output,
        ["[gpt] store partition rejected: ConflictingCopies", *STORE_MARKERS_REJECTED],
        "conflicting GPT boot",
    )
    if hashlib.sha256(image.read_bytes()).hexdigest() != before:
        raise SystemExit("a rejected store image was modified")

    # A damaged newest superblock falls back to the older committed root.
    store_fixture(image, "superblock-newest-damaged")
    output = run_guest(image, 3, STORE_DEVICE_ARGS)
    require_markers(
        output,
        [
            "[store] opened partition first=40 last=2014 seq=1 objects=0",
            *STORE_MARKERS_EMPTY_ROOT,
        ],
        "older-root boot",
    )

    # With no valid superblock the store rejects every operation.
    store_fixture(image, "superblock-both-damaged")
    before = hashlib.sha256(image.read_bytes()).hexdigest()
    output = run_guest(image, 3, STORE_DEVICE_ARGS)
    require_markers(
        output,
        ["[store] open failed: NoValidSuperblock", *STORE_MARKERS_REJECTED],
        "no-root boot",
    )
    if hashlib.sha256(image.read_bytes()).hexdigest() != before:
        raise SystemExit("a rejected store image was modified")

    # An interrupted append beyond the committed root is ignored.
    store_fixture(image, "interrupted-append")
    output = run_guest(image, 3, STORE_DEVICE_ARGS)
    require_markers(output, [STORE_OPENED_HAPPY, *STORE_MARKERS_HAPPY], "interrupted-append boot")

    print("storage object store check: ok")



def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["write", "fault", "store"])
    parser.add_argument("image", type=Path)
    arguments = parser.parse_args()
    arguments.image.unlink(missing_ok=True)
    if arguments.mode == "write":
        write_check(arguments.image)
    elif arguments.mode == "fault":
        fault_check(arguments.image)
    else:
        store_check(arguments.image)


if __name__ == "__main__":
    main()
