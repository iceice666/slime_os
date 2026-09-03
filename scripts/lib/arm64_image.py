"""The arm64 Linux `Image` header, as firmware reads it.

A boot loader implementing the arm64 boot protocol reads sixty-four bytes at
the address it loaded a file to, checks the magic, and decides from
`text_offset` and `image_size` where the image runs and how much memory it
owns. Slime builds such images for boards whose firmware speaks that protocol
and nothing else -- vendor U-Boots with `booti` compiled in and `go`,
`bootelf`, and `bootm` compiled out -- so the header is a wire format this
repository produces and must therefore be able to read back.

Reading it back is the point. The values that matter are the ones firmware acts
on, and a header whose `text_offset` disagrees with where the file was linked
produces a boot that either silently relocates or executes the wrong bytes.
Both look, on a serial console, exactly like a dead board.

The module also flattens an ELF into the raw image such a header fronts: the
PT_LOAD walk, the zero-filled span, the `b` to the entry point, and the check
that the branch lands on the loader's first instruction. Those began in
`scripts/build/build-rpi5-media.py` and live here so a second board can share
them: the Pi 5 firmware wants a bare branch as the first word, a `booti`
firmware wants this header, and both want the same bytes behind it.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass
from pathlib import Path

PT_LOAD = 1
ELF_MAGIC = b"\x7fELF"

#: `mrs x0, mpidr_el1`, the first instruction of `_start` in
#: `deps/rust-sel4/crates/sel4-kernel-loader/asm/aarch64/head.S`. An image
#: whose entry branch lands anywhere else is not entering the loader.
MRS_MPIDR_EL1 = 0xD53800A0

#: `"ARM\\x64"` little-endian, at byte offset 56.
MAGIC = 0x644D5241

#: Every arm64 `Image` header is exactly this long.
HEADER_BYTES = 64

_HEADER = struct.Struct("<IIQQQQQQII")

#: `flags` bit 0 clear selects little-endian; bits 2:1 select the kernel's page
#: size, where 1 means 4 KiB; bit 3 says the image may be placed at any
#: 2 MiB-aligned address rather than only at the base of memory.
FLAG_PAGE_SIZE_4K = 1 << 1
FLAG_PLACE_ANYWHERE = 1 << 3


class Arm64ImageError(Exception):
    """A header that firmware would reject, or act on differently than intended."""


@dataclass(frozen=True)
class Header:
    code0: int
    code1: int
    text_offset: int
    image_size: int
    flags: int
    res2: int
    res3: int
    res4: int
    magic: int
    res5: int


def parse_header(image: bytes) -> Header:
    """Read the header from the first 64 bytes of a flat image."""
    if len(image) < HEADER_BYTES:
        raise Arm64ImageError(
            f"image is {len(image)} bytes, too short to carry a {HEADER_BYTES}-byte header"
        )
    return Header(*_HEADER.unpack(image[:HEADER_BYTES]))


def pack_header(
    *,
    code0: int,
    text_offset: int,
    image_size: int,
    flags: int = FLAG_PAGE_SIZE_4K | FLAG_PLACE_ANYWHERE,
) -> bytes:
    """Build a header. `code0` must be an executable instruction: firmware
    branches to the load address, so the first word runs before anything has
    looked at the magic."""
    return _HEADER.pack(code0, 0, text_offset, image_size, flags, 0, 0, 0, MAGIC, 0)


def decode_branch(word: int) -> int:
    """Byte displacement of an AArch64 unconditional `b`, relative to itself.

    `b` is `0b000101` in the top six bits followed by a signed 26-bit immediate
    counting instructions. Raises if `word` is not a `b` at all, which is the
    interesting case: a header whose `code0` is data rather than a branch is one
    firmware will happily execute as code.
    """
    if word >> 26 != 0b000101:
        raise Arm64ImageError(
            f"first word {word:#010x} is not an unconditional `b`, so firmware "
            "would execute whatever it is as code"
        )
    imm26 = word & ((1 << 26) - 1)
    if imm26 >= (1 << 25):
        imm26 -= 1 << 26
    return imm26 * 4


@dataclass(frozen=True)
class Segment:
    offset: int
    paddr: int
    file_size: int
    mem_size: int


def read_load_segments(path: Path) -> list[Segment]:
    """Every PT_LOAD segment of a little-endian AArch64 ELF64, by physical address."""
    data = path.read_bytes()
    if len(data) < 64:
        raise Arm64ImageError(f"{path} is shorter than an ELF64 header")
    if data[:4] != ELF_MAGIC:
        raise Arm64ImageError(f"{path} is not an ELF file")
    if data[4] != 2 or data[5] != 1:
        raise Arm64ImageError(f"{path} is not a little-endian 64-bit ELF")
    e_machine = struct.unpack_from("<H", data, 18)[0]
    if e_machine != 183:  # EM_AARCH64
        raise Arm64ImageError(f"{path} is not an AArch64 ELF (e_machine={e_machine})")
    (e_phoff,) = struct.unpack_from("<Q", data, 32)
    e_phentsize, e_phnum = struct.unpack_from("<HH", data, 54)
    if e_phentsize < 56 or e_phoff + e_phnum * e_phentsize > len(data):
        raise Arm64ImageError(f"{path} has a truncated ELF program-header table")
    segments = []
    for index in range(e_phnum):
        header = e_phoff + index * e_phentsize
        (p_type,) = struct.unpack_from("<I", data, header)
        if p_type != PT_LOAD:
            continue
        p_offset, _p_vaddr, p_paddr, p_filesz, p_memsz = struct.unpack_from(
            "<QQQQQ", data, header + 8
        )
        if p_filesz > p_memsz:
            raise Arm64ImageError(
                f"{path} PT_LOAD {index} has file size {p_filesz:#x} larger than memory size {p_memsz:#x}"
            )
        if p_offset > len(data) or p_filesz > len(data) - p_offset:
            raise Arm64ImageError(f"{path} PT_LOAD {index} extends past the end of the file")
        segments.append(Segment(p_offset, p_paddr, p_filesz, p_memsz))
    if not segments:
        raise Arm64ImageError(f"{path} declares no PT_LOAD segment")
    return sorted(segments, key=lambda segment: segment.paddr)


def elf_entry(path: Path) -> int:
    data = path.read_bytes()
    if len(data) < 64 or data[:4] != ELF_MAGIC or data[4] != 2 or data[5] != 1:
        raise Arm64ImageError(f"{path} is not a complete little-endian ELF64 file")
    return struct.unpack_from("<Q", data, 24)[0]


def flatten(path: Path, segments: list[Segment]) -> tuple[bytearray, int]:
    """One contiguous image spanning every segment, and its physical base.

    Gaps between segments are zero-filled, which is what a raw image means:
    firmware copies these bytes to one address and nothing else initialises
    them, so anything the loader expects to be zero must actually be zero here
    rather than left to whatever the previous boot stage wrote. The first
    bytes are whatever the lowest segment holds -- for a loader linked with
    its ELF header in the first segment, that header -- and it is the caller's
    job to overwrite them with something firmware may execute.
    """
    data = path.read_bytes()
    base = segments[0].paddr
    end = max(segment.paddr + segment.mem_size for segment in segments)
    if base % 4096 != 0:
        raise Arm64ImageError(f"lowest segment address {base:#x} is not page-aligned")
    image = bytearray(end - base)
    for segment in segments:
        start = segment.paddr - base
        image[start : start + segment.file_size] = data[
            segment.offset : segment.offset + segment.file_size
        ]
    return image, base


def encode_branch(source: int, target: int) -> bytes:
    """An AArch64 unconditional `b` from `source` to `target`.

    `b` is `0b000101` followed by a signed 26-bit immediate counting
    instructions, so it reaches +/-128 MiB; the range is checked rather than
    assumed.
    """
    delta = target - source
    if delta % 4 != 0:
        raise Arm64ImageError(f"branch target {target:#x} is not instruction-aligned")
    imm26 = delta // 4
    if not -(1 << 25) <= imm26 < (1 << 25):
        raise Arm64ImageError(f"branch from {source:#x} to {target:#x} is out of range for `b`")
    return struct.pack("<I", (0b000101 << 26) | (imm26 & ((1 << 26) - 1)))


def check_branch_lands_on_start(image: bytes, base: int, entry: int) -> None:
    """The word at `entry` must be the loader's first instruction.

    Pinning the destination catches an entry that drifted into a literal pool
    or the middle of a function, which no check on the branch itself would.
    """
    landing = entry - base
    if landing < 0 or landing + 4 > len(image):
        raise Arm64ImageError("the branch target lies outside the flattened image")
    first = struct.unpack_from("<I", image, landing)[0]
    if first != MRS_MPIDR_EL1:
        raise Arm64ImageError(
            f"the branch lands on {first:#010x}, not the loader's "
            f"`mrs x0, mpidr_el1` ({MRS_MPIDR_EL1:#010x}); the entry point is not _start"
        )
