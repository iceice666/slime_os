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
import struct
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4-bcm2712-rpi5.elf"
MANIFEST = ROOT / "build" / "slime-sel4-bcm2712-rpi5.identity.json"
MEDIA = ROOT / "build" / "rpi5-media"

PT_LOAD = 1
ELF_MAGIC = b"\x7fELF"
# `mrs x0, mpidr_el1`, the first instruction of `_start` in
# `deps/rust-sel4/crates/sel4-kernel-loader/asm/aarch64/head.S`.
MRS_MPIDR_EL1 = 0xD53800A0


def fail(message: str) -> NoReturn:
    raise SystemExit(f"rpi5 media build: {message}")


@dataclass(frozen=True)
class Segment:
    offset: int
    paddr: int
    file_size: int
    mem_size: int


def read_load_segments(path: Path) -> list[Segment]:
    """Every PT_LOAD segment of a little-endian AArch64 ELF64, by physical address."""
    data = path.read_bytes()
    if data[:4] != ELF_MAGIC:
        fail(f"{path.relative_to(ROOT)} is not an ELF file")
    if data[4] != 2 or data[5] != 1:
        fail(f"{path.relative_to(ROOT)} is not a little-endian 64-bit ELF")
    e_machine = struct.unpack_from("<H", data, 18)[0]
    if e_machine != 183:  # EM_AARCH64
        fail(f"{path.relative_to(ROOT)} is not an AArch64 ELF (e_machine={e_machine})")
    e_phoff, = struct.unpack_from("<Q", data, 32)
    e_phentsize, e_phnum = struct.unpack_from("<HH", data, 54)
    segments = []
    for index in range(e_phnum):
        base = e_phoff + index * e_phentsize
        p_type, = struct.unpack_from("<I", data, base)
        if p_type != PT_LOAD:
            continue
        p_offset, _p_vaddr, p_paddr, p_filesz, p_memsz = struct.unpack_from(
            "<QQQQQ", data, base + 8
        )
        segments.append(Segment(p_offset, p_paddr, p_filesz, p_memsz))
    if not segments:
        fail(f"{path.relative_to(ROOT)} declares no PT_LOAD segment")
    return sorted(segments, key=lambda segment: segment.paddr)


def flatten(path: Path, segments: list[Segment], entry: int) -> tuple[bytes, int]:
    """One contiguous image, and the physical address it must be loaded at.

    Gaps between segments are zero-filled, which is what a raw image means: the
    firmware copies these bytes to `kernel_address` and jumps to the *first
    byte*, so anything the loader expects to be zero must actually be zero here
    rather than left to whatever the previous boot stage wrote.

    Two facts make this more than a concatenation, and getting either wrong
    yields an image that loads and then does nothing observable:

      * The firmware branches to the start of what it loaded, but the ELF's
        entry point is 0x4838 into the lowest segment — `.rodata` precedes
        `.text` in the loader's layout. The first instruction executed would
        therefore not be `_start`.
      * The lowest segment begins at file offset 0, so it *contains the 64-byte
        ELF header*. Copying it verbatim put `7f454c46` — `ELF` — at the address
        the firmware branches to, and the CPU executed the header as AArch64.
        That is precisely a board that boots and prints nothing.

    Both are fixed by keeping every segment at its true physical address —
    contents are position-dependent, so they cannot be slid — and overwriting
    the ELF header's own bytes with one `b` to the entry point. The header is
    dead weight at run time, occupies the image's first 64 bytes, and sits at a
    page-aligned address, so the branch costs nothing and keeps
    `kernel_address` page-aligned. Trimming the image to `entry` instead would
    have worked too, but would have handed the firmware an address aligned only
    to 4 bytes.
    """
    data = path.read_bytes()
    base = segments[0].paddr
    end = max(segment.paddr + segment.mem_size for segment in segments)
    if not base <= entry < end:
        fail(f"entry {entry:#x} falls outside the loaded span")
    if base % 4096 != 0:
        fail(f"lowest segment address {base:#x} is not page-aligned")
    image = bytearray(end - base)
    for segment in segments:
        start = segment.paddr - base
        image[start : start + segment.file_size] = data[
            segment.offset : segment.offset + segment.file_size
        ]
    image[:4] = encode_branch(base, entry)
    return bytes(image), base


def encode_branch(source: int, target: int) -> bytes:
    """An AArch64 unconditional `b` from `source` to `target`.

    `b` is `0b000101` followed by a signed 26-bit immediate counting
    instructions, so it reaches +/-128 MiB — far more than the 0x4838 needed
    here, but the range is checked rather than assumed.
    """
    delta = target - source
    if delta % 4 != 0:
        fail(f"branch target {target:#x} is not instruction-aligned")
    imm26 = delta // 4
    if not -(1 << 25) <= imm26 < (1 << 25):
        fail(f"branch from {source:#x} to {target:#x} is out of range for `b`")
    return struct.pack("<I", (0b000101 << 26) | (imm26 & ((1 << 26) - 1)))


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
    landing = entry - base
    if landing + 4 > len(image):
        fail("the branch target lies outside the flattened image")
    first = struct.unpack_from("<I", image, landing)[0]
    if first != MRS_MPIDR_EL1:
        fail(
            f"the branch lands on {first:#010x}, not the loader's "
            f"`mrs x0, mpidr_el1` ({MRS_MPIDR_EL1:#010x}); the entry point is "
            "not _start"
        )


def elf_entry(path: Path) -> int:
    return struct.unpack_from("<Q", path.read_bytes(), 24)[0]


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
