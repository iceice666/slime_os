#!/usr/bin/env python3

"""Turn P4's packaged RPi5 ELF into the flat boot image the firmware loads.

The BCM2712 boot ROM reads the SPI EEPROM, which reads a FAT32 partition and
loads the 64-bit kernel named by `config.txt` as a *raw* image at a fixed
address. `sel4-kernel-loader-add-payload` only ever emits a patched ELF
(`main.rs` finalises an `ElfFile`; there is no raw-output flag), so something
has to flatten it.

`objcopy -O binary` cannot: it works from sections, and the loader's payload
arrives in program headers that carry no sections at all — the payload segment
and its `PT_SEL4_KERNEL_LOADER_PAYLOAD` alias map to zero sections, so objcopy
silently drops them and produces a 38 KB image out of a 789 KB one. That image
boots nothing, and nothing about it looks wrong. This script therefore reads
the program headers directly, which is also what the firmware effectively does.

It writes the exact `boot_files` that `sel4/pins.toml [bcm2712_rpi5]` pins, and
nothing else: `config.txt` and `kernel8.img`. Media layout, partitioning, and
copying are deliberately not done here — writing to a block device is the one
step that can destroy a user's disk, so the operator does it with the commands
this script prints.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import tomllib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

import arm64_image  # noqa: E402
from arm64_image import ELF_MAGIC, Arm64ImageError, Segment, elf_entry  # noqa: E402,F401
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4-bcm2712-rpi5.elf"
MANIFEST = ROOT / "build" / "slime-sel4-bcm2712-rpi5.identity.json"
MEDIA = ROOT / "build" / "rpi5-media"

def fail(message: str) -> NoReturn:
    raise SystemExit(f"rpi5 media build: {message}")


def flatten(path: Path, segments: list[Segment], entry: int) -> tuple[bytes, int]:
    """One contiguous image whose first word branches to the entry point.

    The BCM2712 firmware copies the file to `kernel_address` and jumps to its
    *first byte*, and the ELF's entry is not its first byte: `.rodata` precedes
    `.text`, and the lowest segment begins with the ELF header. The shared
    flattener supplies the zero-filled span; the branch installed here is what
    makes its first word executable, and `check_entry_is_code` proves it.
    """
    try:
        image, base = arm64_image.flatten(path, segments)
    except Arm64ImageError as error:
        fail(str(error))
    end = base + len(image)
    if not base <= entry < end:
        fail(f"entry {entry:#x} falls outside the loaded span")
    image[:4] = encode_branch(base, entry)
    return bytes(image), base


def encode_branch(source: int, target: int) -> bytes:
    try:
        return arm64_image.encode_branch(source, target)
    except Arm64ImageError as error:
        fail(str(error))


def check_entry_is_code(image: bytes, base: int, entry: int) -> None:
    """The first word must be the branch this script installed, not data.

    This guard exists because the first version of this script shipped an image
    beginning with `7f454c46` — the ELF magic — which the firmware dutifully
    loaded and branched into, executing the header as AArch64. The board booted
    and printed nothing, which is indistinguishable from a wiring fault. Four
    bytes of comparison are worth more than the time that ambiguity costs.

    Both ends are checked: the first word is the exact expected branch, and the
    word it lands on is `mrs x0, mpidr_el1` — the first instruction of `_start`
    in `deps/rust-sel4/crates/sel4-kernel-loader/asm/aarch64/head.S`. Pinning
    the destination too catches an entry that drifted into a literal pool or the
    middle of a function, which a "not ELF magic" check would miss.
    """
    if len(image) < 4:
        fail("flattened image is too small to contain an instruction")
    if image[:4] == ELF_MAGIC:
        fail(
            "the flattened image starts with ELF magic, so the firmware would "
            "execute the ELF header as code"
        )
    expected = encode_branch(base, entry)
    if image[:4] != expected:
        fail(
            f"the flattened image starts with {image[:4].hex()}, not the "
            f"expected branch {expected.hex()} to the entry point"
        )
    try:
        arm64_image.check_branch_lands_on_start(image, base, entry)
    except Arm64ImageError as error:
        fail(str(error))


def read_load_segments(path: Path) -> list[Segment]:
    try:
        return arm64_image.read_load_segments(path)
    except Arm64ImageError as error:
        fail(str(error))


def render_config(load_addr: int, pins: dict[str, object]) -> str:
    """`config.txt` for a bare AArch64 image, with every line load-bearing.

    `arm_64bit=1` selects AArch64 rather than the 32-bit entry path.

    `kernel_address` is where the firmware places the raw image *and* the
    address it branches to. It is therefore the ELF's **entry point**, not its
    lowest segment address: `flatten` trims the image to start at `_start` for
    exactly this reason, and the two values must agree or the loader runs from
    the wrong place with no diagnostic.

    `enable_uart=1` plus `uart_2ndstage=1` keep the debug UART powered and put
    the firmware's own diagnostics on it, so a boot that dies before Slime
    prints anything is still diagnosable — and, usefully, so silence on the wire
    distinguishes a wiring fault from an image fault.

    `disable_commandline_tags=1` stops the firmware appending ATAGs over the
    image, and `device_tree=` disables firmware device-tree loading: this kernel
    carries its own description compiled from `deps/sel4/tools/dts/rpi5b.dts`.
    """
    board = pins["bcm2712_rpi5"]
    return "\n".join(
        (
            "# Generated by scripts/build/build-rpi5-media.py. Do not edit.",
            f"# Slime OS seL4 image for {board['board']} ({board['soc']}).",
            f"# Serial evidence path: {board['serial']} at {board['serial_baud']} baud.",
            "arm_64bit=1",
            "kernel=kernel8.img",
            f"kernel_address={load_addr:#x}",
            "disable_commandline_tags=1",
            "device_tree=",
            "enable_uart=1",
            "uart_2ndstage=1",
            "",
        )
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--image",
        type=Path,
        default=IMAGE,
        help="packaged RPi5 loader ELF to flatten",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=MEDIA,
        help="directory to write the boot files into",
    )
    arguments = parser.parse_args()

    if not arguments.image.is_file():
        fail(
            f"missing {arguments.image}; build it with "
            "`python3 scripts/build/build-sel4.py --platform bcm2712-rpi5`"
        )
    pins = tomllib.loads(PINS_PATH.read_text(encoding="utf-8"))
    expected = list(pins["bcm2712_rpi5"]["boot_files"])

    segments = read_load_segments(arguments.image)
    entry = elf_entry(arguments.image)
    image, load_addr = flatten(arguments.image, segments, entry)
    check_entry_is_code(image, load_addr, entry)

    arguments.output.mkdir(parents=True, exist_ok=True)
    kernel = arguments.output / "kernel8.img"
    config = arguments.output / "config.txt"
    kernel.write_bytes(image)
    config.write_text(render_config(load_addr, pins), encoding="utf-8")

    written = sorted(path.name for path in arguments.output.iterdir())
    if written != sorted(expected):
        fail(
            f"wrote {written}, but sel4/pins.toml pins boot_files as {sorted(expected)}"
        )

    digest = hashlib.sha256(image).hexdigest()
    if MANIFEST.is_file():
        identity = json.loads(MANIFEST.read_text(encoding="utf-8"))
        if identity.get("platform") != "bcm2712-rpi5":
            fail(
                f"{MANIFEST.relative_to(ROOT)} is for platform "
                f"{identity.get('platform')!r}, not bcm2712-rpi5"
            )
    print(f"rpi5 media build: wrote {kernel.relative_to(ROOT)} ({len(image)} bytes)")
    print(f"  load address {load_addr:#x}, entry {entry:#x}, sha256 {digest}")
    print(f"  wrote {config.relative_to(ROOT)}")
    print()
    print("Copy these onto the FAT32 boot partition of the removable media, e.g.:")
    print(f"  cp {kernel.relative_to(ROOT)} {config.relative_to(ROOT)} /Volumes/<BOOT>/")
    print("Nothing here writes to a block device; do that step yourself.")


if __name__ == "__main__":
    sys.exit(main())
