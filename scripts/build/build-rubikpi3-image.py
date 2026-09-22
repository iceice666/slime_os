#!/usr/bin/env python3
"""Package a fixed-link Rubik Pi 3 seL4 loader ELF for legacy arm64 kexec."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts" / "lib"))

import arm64_image  # noqa: E402
from arm64_image import Arm64ImageError  # noqa: E402

TWO_MIB = 2 * 1024 * 1024
SEL4_RAM_START = 0xA020_0000
SEL4_RAM_END = 0xB970_0000
#: Room reserved above the image for the segments `kexec` places after it: the
#: device tree it hands the payload, which the boot protocol caps at 2 MiB, and
#: the purgatory. Both are transient -- the loader has entered seL4 before that
#: memory is used for anything else -- but `--mem-max` bounds every segment, so
#: the window must hold them or the load fails outright.
MINIMUM_AUXILIARY_BYTES = 2 * TWO_MIB + 64 * 1024
PAGE_BYTES = 4096
DEFAULT_INPUT = ROOT / "build" / "slime-sel4-rubikpi3.elf"
DEFAULT_OUTPUT = ROOT / "build" / "rubikpi3" / "slime-sel4-rubikpi3.img"


def fail(message: str) -> NoReturn:
    raise SystemExit(f"rubikpi3 image build: {message}")


def align_down(value: int, alignment: int) -> int:
    return value & -alignment


def align_up(value: int, alignment: int) -> int:
    return -(-value // alignment) * alignment


def build_image(source: Path) -> tuple[bytes, dict[str, object]]:
    try:
        segments = arm64_image.read_load_segments(source)
        entry = arm64_image.elf_entry(source)
    except Arm64ImageError as error:
        fail(str(error))

    lowest = segments[0].paddr
    highest = max(segment.paddr + segment.mem_size for segment in segments)
    image_base = align_down(lowest, TWO_MIB)
    if image_base < SEL4_RAM_START:
        fail(f"image base {image_base:#x} falls below seL4 RAM {SEL4_RAM_START:#x}")
    if highest > SEL4_RAM_END:
        fail(f"ELF end {highest:#x} exceeds qualified RAM end {SEL4_RAM_END:#x}")
    if lowest - image_base < arm64_image.HEADER_BYTES:
        fail("lowest PT_LOAD leaves no room for an executable arm64 Image header")
    if not lowest <= entry < highest:
        fail(f"entry {entry:#x} falls outside PT_LOAD span {lowest:#x}..{highest:#x}")

    data = source.read_bytes()
    image = bytearray(highest - image_base)
    previous_end = 0
    for segment in segments:
        start = segment.paddr - image_base
        end = start + segment.mem_size
        if start < previous_end:
            fail(f"overlapping PT_LOAD segment at {segment.paddr:#x}")
        image[start : start + segment.file_size] = data[
            segment.offset : segment.offset + segment.file_size
        ]
        previous_end = end

    try:
        branch = arm64_image.encode_branch(image_base, entry)
        header = arm64_image.pack_header(
            code0=int.from_bytes(branch, "little"),
            text_offset=0,
            image_size=len(image),
            flags=arm64_image.FLAG_PAGE_SIZE_4K,
        )
    except Arm64ImageError as error:
        fail(str(error))
    image[: arm64_image.HEADER_BYTES] = header

    try:
        parsed = arm64_image.parse_header(image)
        if parsed.magic != arm64_image.MAGIC:
            raise Arm64ImageError(f"read-back magic is {parsed.magic:#x}")
        if parsed.text_offset != 0 or parsed.image_size != len(image):
            raise Arm64ImageError("read-back text_offset/image_size differs from emitted image")
        if parsed.flags != arm64_image.FLAG_PAGE_SIZE_4K:
            raise Arm64ImageError("fixed-link image is incorrectly marked relocatable")
        if arm64_image.decode_branch(parsed.code0) != entry - image_base:
            raise Arm64ImageError("read-back branch does not land on the ELF entry")
        arm64_image.check_branch_lands_on_start(image, image_base, entry)
    except Arm64ImageError as error:
        fail(str(error))

    # `add_segment_phys_virt` rounds every segment's memory size up to a page
    # before validating it against `mem_max`, so the bound must cover the
    # rounded image rather than its exact length.
    rounded_image_end = align_up(image_base + len(image), PAGE_BYTES)
    kexec_window_end = align_up(rounded_image_end + MINIMUM_AUXILIARY_BYTES, TWO_MIB)
    if kexec_window_end > SEL4_RAM_END:
        fail(f"kexec window end {kexec_window_end:#x} exceeds RAM end {SEL4_RAM_END:#x}")
    auxiliary_capacity = kexec_window_end - rounded_image_end
    metadata: dict[str, object] = {
        "schema": "slime-rubikpi3-kexec-image/v2",
        "source": str(source),
        "sourceSha256": hashlib.sha256(data).hexdigest(),
        "imageBase": f"0x{image_base:x}",
        "imageEndExclusive": f"0x{image_base + len(image):x}",
        "imagePageEndExclusive": f"0x{rounded_image_end:x}",
        "imageSize": len(image),
        "entry": f"0x{entry:x}",
        "branchDisplacement": entry - image_base,
        "lowestLoad": f"0x{lowest:x}",
        "highestLoadEnd": f"0x{highest:x}",
        "fixedPlacement": True,
        "placementContract": (
            "header forbids arbitrary placement; legacy kexec mem-min pins the absolute base"
        ),
        "kexecMemMin": f"0x{image_base:x}",
        "kexecMemMax": f"0x{kexec_window_end - 1:x}",
        "kexecAuxiliaryCapacity": auxiliary_capacity,
        "minimumKexecAuxiliaryBytes": MINIMUM_AUXILIARY_BYTES,
        "requiredKexecMode": "legacy-kexec-load",
        "requiredKexecArguments": [
            "-c",
            f"--mem-min=0x{image_base:x}",
            f"--mem-max=0x{kexec_window_end - 1:x}",
        ],
        "sel4RamStart": f"0x{SEL4_RAM_START:x}",
        "sel4RamEnd": f"0x{SEL4_RAM_END:x}",
    }
    return bytes(image), metadata


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", type=Path, default=DEFAULT_INPUT)
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    arguments = parser.parse_args()

    if not arguments.input.is_file():
        fail(f"input does not exist: {arguments.input}")
    image, metadata = build_image(arguments.input)
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    arguments.output.write_bytes(image)
    metadata["imageSha256"] = hashlib.sha256(image).hexdigest()
    metadata_path = arguments.output.with_suffix(arguments.output.suffix + ".json")
    metadata_path.write_text(json.dumps(metadata, indent=2, sort_keys=True) + "\n")
    print(arguments.output)
    print(metadata_path)


if __name__ == "__main__":
    main()
