#!/usr/bin/env python3
"""IO2 gate: computed virtio-blk operations, refusals, and async settlement."""

from __future__ import annotations

import argparse
import re
import sys
import tempfile
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from closure_image import ClosureImageError, build as build_closure_image  # noqa: E402
from harness import sha256_file  # noqa: E402
from sel4_gate_markers import match_marker_contract  # noqa: E402
from sel4_plane import run_plane  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS = ROOT / "sel4" / "pins.toml"
# The closure identity names the build's inputs and is re-resolved from repository
# state before building, so stale input is refused instead of silently changing the image.
CLOSURE = "sel4-io-block"
IMAGE: Path | None = None
FIXTURE = ROOT / "contracts" / "generation-manifest" / "v1" / "compositions" / "sel4-io-block.zti"
TIMEOUT = 300
SECTOR_BYTES = 512
DISK_BYTES = 1 << 20
FRESH_LBA = 3
FRESH_MARKER = b"SLIMEIO2"
WRITTEN_PREFIX = b"SLIMEIO2-WRITTEN"
WRITTEN_FILL = 0xA5

CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "generation and driver authority",
        (
            r"SLIME_ROOT generation admitted number=51 ",
            r"\[virtio-blk-driver\] mmio mechanism=mediated-bounded-read32-write32",
            r"\[virtio-blk-driver\] ready capacity=\d+ epoch=\d+",
        ),
    ),
    (
        "bounded asynchronous identity",
        (
            r"\[io-block-probe\] backpressure full_refusals=1 overwrite=0",
            r"\[io-block-probe\] async queued=8 completed=8 identities=8 overwrite=0",
        ),
    ),
    (
        "observed block operations",
        (
            r"\[io-block-probe\] operations read=2 write=1 flush=1 geometry=1",
            r"\[io-block-probe\] byte-verification readback=512 mismatches=0",
        ),
    ),
    (
        "observed refusal arms",
        (
            r"\[io-block-probe\] refusals out_of_range=1 malformed=1 short_buffer=1 unsupported=1 missing_right=1",
            r"\[io-block-probe\] io block plane complete observed_operations=5 observed_refusals=5",
            r"SLIME_GRAPH HEALTHY generation=51 required=4 live=0 completed=4 failed=0",
        ),
    ),
)

FAILURE_MARKERS = (
    r"SLIME_ROOT FATAL",
    r"SLIME_GRAPH FAIL",
    r"\[virtio-blk-driver\] fail: ",
    r"\[io-block-probe\] fail: ",
    r"Caught cap fault",
    r"Caught vm fault",
    r"panicked at ",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 I/O block plane check: {message}")


def build_image() -> None:
    global IMAGE
    try:
        built = build_closure_image(CLOSURE)
    except ClosureImageError as error:
        fail(str(error))
    IMAGE = built.image
    actual = sha256_file(IMAGE, fail)
    if actual != built.digest():
        fail(f"{IMAGE} SHA-256 is {actual}, but the build result records {built.digest()}; the image changed after it was built")


def check_fixture() -> None:
    text = FIXTURE.read_text(encoding="utf-8")
    for declaration in (
        "generation = 51;",
        'name = "virtio-blk-driver";',
        'name = "io-block-probe";',
        'capabilityKind = "device";',
        'capabilityKind = "mmioRegion";',
        'capabilityKind = "interruptSource";',
        'capabilityKind = "dmaAccount";',
    ):
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
    with tempfile.TemporaryDirectory(prefix="slime-io-block-") as temporary:
        disk = Path(temporary) / "disk.img"
        readonly_disk = Path(temporary) / "readonly-disk.img"
        image = bytearray(DISK_BYTES)
        image[FRESH_LBA * SECTOR_BYTES : FRESH_LBA * SECTOR_BYTES + len(FRESH_MARKER)] = (
            FRESH_MARKER
        )
        disk.write_bytes(image)
        readonly_disk.write_bytes(bytearray(DISK_BYTES))
        terminal = re.compile(CHAINS[-1][1][-1] + "|" + "|".join(FAILURE_MARKERS))
        transcript = run_plane(
            image=IMAGE,
            timeout=TIMEOUT,
            terminal_condition=terminal,
            fail=fail,
            pins_path=PINS,
            additional_arguments=(
                "-drive",
                f"if=none,id=slimeio2,format=raw,file={disk}",
                "-device",
                "virtio-blk-device,drive=slimeio2",
                "-drive",
                f"if=none,id=slimeio2ro,format=raw,file={readonly_disk},readonly=on",
                "-device",
                "virtio-blk-device,drive=slimeio2ro",
            ),
        )
        match_marker_contract(transcript, CHAINS, FAILURE_MARKERS, fail)
        written = disk.read_bytes()[FRESH_LBA * SECTOR_BYTES : (FRESH_LBA + 1) * SECTOR_BYTES]
        expected = WRITTEN_PREFIX + bytes([WRITTEN_FILL]) * (SECTOR_BYTES - len(WRITTEN_PREFIX))
        if written != expected:
            fail("flushed write did not reach the backing disk byte-for-byte")
    print(
        "seL4 I/O block plane check: computed operations, byte readback, async identity, and refusal arms proved"
    )


if __name__ == "__main__":
    main()
