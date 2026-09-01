#!/usr/bin/env python3

"""Build P6.A's Novatek NT98690 H1V1 probe into the flat image `booti` loads.

The board's vendor U-Boot has `booti` and nothing else: `go`, `bootelf`, and
`bootm` are compiled out of `nvt-ns02201_a64_pci_emmc_defconfig`. So the only
way to hand this board our own code is an arm64 `Image` -- a flat binary
carrying the sixty-four byte header the Linux boot protocol defines -- and the
only way to get one is to emit it ourselves.

The header is assembled into the payload rather than prepended here, because
its `text_offset` and `image_size` are linker facts and belong to the linker
script that establishes them. What this script does is *check* them, which
matters more than it sounds: this board's U-Boot carries a Novatek patch that
relocates every image to `ALIGN(gd->ram_base, 2 MiB) + text_offset`,
unconditionally, ignoring the header's "place anywhere" flag
(`arch/arm/lib/image.c` under `CONFIG_ARCH_NOVATEK`). With `ram_base` zero on
this board the destination is exactly `text_offset` -- so a `text_offset` that
disagreed with the link address would relocate the image out from under its own
code, and the board would print nothing at all. That failure is indisputable on
a bench and invisible in a diff, which is why three separate agreements are
asserted here: the linker script's base, the pinned load address, and the
header the assembler actually emitted.

Writing to removable media is deliberately not done here. Copying onto a card
is the one step that can destroy an unrelated disk, so this prints the command
and the operator runs it.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from arm64_image import (  # noqa: E402
    FLAG_PAGE_SIZE_4K,
    FLAG_PLACE_ANYWHERE,
    HEADER_BYTES,
    MAGIC,
    Arm64ImageError,
    decode_branch,
    parse_header,
)

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
PINS_SECTION = "ns02201_h1v1"
SOURCE_DIR = ROOT / "tools" / "nt98690" / "payload"
PROBE_SOURCE = SOURCE_DIR / "probe.S"
PROBE_LINKER_SCRIPT = SOURCE_DIR / "probe.ld"
OUT_DIR = ROOT / "build" / "nt98690-payload"

TARGET_PROFILE = "aarch64-nt98690-bringup"

#: The probe keeps its whole state in registers and a 4 KiB stack, so its
#: memory footprint is its file plus that stack plus alignment. A declared
#: `image_size` far larger than the file would reserve memory nothing uses; one
#: smaller than the file would let U-Boot's own FDT relocation land on the code.
MAX_RESERVED_BEYOND_FILE = 0x2000

#: Regions of this board's DRAM that belong to the vendor firmware while our
#: payload runs, from `nvt-mem-tbl.dtsi` and `ModelConfig.mk`. A payload
#: overlapping any of them corrupts the firmware that has to survive to reset
#: the board.
RESERVED_REGIONS: tuple[tuple[int, int, str], ...] = (
    (0x0000_0000, 0x0200_0000, "vendor core-entry stubs, loader device tree, SHMINFO, loader, and TF-A BL31"),
    (0x0480_0000, 0x0AC0_0000, "vendor media CMA pools"),
    (0x7C00_0000, 0x8000_0000, "U-Boot, its heap, and the device tree it relocates for us"),
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"nt98690 payload build: {message}")


def load_profile() -> dict[str, object]:
    if not PINS_PATH.is_file():
        fail(f"missing pins: {PINS_PATH.relative_to(ROOT)}")
    try:
        pins = tomllib.loads(PINS_PATH.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot read {PINS_PATH.relative_to(ROOT)}: {error}")
    profile = pins.get(PINS_SECTION)
    if not isinstance(profile, dict):
        fail(f"sel4/pins.toml has no [{PINS_SECTION}] table")
    for key in (
        "board",
        "soc",
        "payload_load_address",
        "dram_base",
        "firmware_memory_size",
        "boot_files",
        "serial_baud",
    ):
        if key not in profile:
            fail(f"sel4/pins.toml [{PINS_SECTION}] must pin {key}")
    return profile


def hex_pin(profile: dict[str, object], key: str) -> int:
    value = profile[key]
    if not isinstance(value, str) or not value.startswith("0x"):
        fail(f"sel4/pins.toml [{PINS_SECTION}].{key} must be a hexadecimal string")
    try:
        return int(value, 16)
    except ValueError:
        fail(f"sel4/pins.toml [{PINS_SECTION}].{key} is not a hexadecimal integer")


def boot_file(profile: dict[str, object]) -> str:
    files = profile["boot_files"]
    if not isinstance(files, list) or len(files) != 1 or not isinstance(files[0], str):
        fail(f"sel4/pins.toml [{PINS_SECTION}].boot_files must name exactly one file")
    return files[0]


def check_link_address(profile: dict[str, object]) -> int:
    """The linker script's base, the pinned load address, and the vendor memory
    map must agree before anything is compiled."""
    load = hex_pin(profile, "payload_load_address")

    source = PROBE_LINKER_SCRIPT.read_text(encoding="utf-8")
    match = re.search(r"^\s*PAYLOAD_BASE\s*=\s*(0x[0-9a-fA-F]+)\s*;", source, re.MULTILINE)
    if match is None:
        fail(f"{PROBE_LINKER_SCRIPT.relative_to(ROOT)} does not define PAYLOAD_BASE")
    linked = int(match.group(1), 16)
    if linked != load:
        fail(
            f"{PROBE_LINKER_SCRIPT.relative_to(ROOT)} links at {linked:#x} but "
            f"[{PINS_SECTION}].payload_load_address is {load:#x}; this U-Boot "
            "relocates to the header's text_offset, so the two must be equal"
        )

    if load % 0x20_0000 != 0:
        fail(
            f"payload load address {load:#x} is not 2 MiB-aligned, which the "
            "board's relocation arithmetic requires"
        )

    limit = hex_pin(profile, "firmware_memory_size")
    if load >= limit:
        fail(
            f"payload load address {load:#x} is outside the {limit:#x} bytes of "
            "DRAM the vendor device tree declares to firmware"
        )

    for start, end, what in RESERVED_REGIONS:
        if start <= load < end:
            fail(f"payload load address {load:#x} lies inside the {what} ({start:#x}..{end:#x})")
    return load


def cross_prefix() -> str:
    """The dev shell's pinned AArch64 cross toolchain.

    `flake.nix` exports this from `pkgsCross.aarch64-multiplatform.stdenv.cc`
    and `check-sel4-pins.py` asserts the exported path, so the compiler that
    builds this payload is the same pinned one the seL4 product is built with.
    A bare-metal `aarch64-none-elf` toolchain fetched by attribute would be the
    more obvious choice and is deliberately not used: this host resolves
    `nixpkgs` through a rolling channel, so an attribute reference would make
    the payload's bytes depend on the week it was built.
    """
    prefix = os.environ.get("CROSS_COMPILER_PREFIX")
    if not prefix:
        fail(
            "CROSS_COMPILER_PREFIX is unset; run inside the dev shell "
            "(`nix develop`), which exports the pinned AArch64 cross toolchain"
        )
    if shutil.which(f"{prefix}gcc") is None:
        fail(f"{prefix}gcc is not executable")
    return prefix


def run(command: list[str]) -> str:
    try:
        completed = subprocess.run(command, check=True, capture_output=True, text=True)
    except FileNotFoundError:
        fail(f"missing tool: {command[0]}")
    except subprocess.CalledProcessError as error:
        detail = (error.stderr or error.stdout or "").strip()
        fail(f"`{' '.join(command)}` failed: {detail}")
    return completed.stdout


def build_binary(prefix: str, *, output_name: str, qemu_variant: bool) -> tuple[Path, Path, int, str]:
    """Assemble and flatten the probe. Returns (binary, elf, entry, toolchain)."""
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    elf = OUT_DIR / ("probe-qemu.elf" if qemu_variant else "probe.elf")
    binary = OUT_DIR / output_name

    command = [
        f"{prefix}gcc",
        "-x",
        "assembler-with-cpp",
        "-march=armv8-a",
        "-nostdlib",
        "-nostartfiles",
        "-ffreestanding",
        # The wrapper defaults to PIE, which cannot hold the absolute
        # relocation the probe uses to compare its link address against where
        # it is actually running.
        "-no-pie",
        "-static",
        "-Wl,--build-id=none",
        f"-Wl,-T,{PROBE_LINKER_SCRIPT}",
    ]
    if qemu_variant:
        command.append("-DSLIME_PROBE_QEMU_VIRT")
    command += ["-o", str(elf), str(PROBE_SOURCE)]
    run(command)
    run([f"{prefix}objcopy", "-O", "binary", str(elf), str(binary)])

    header = run([f"{prefix}readelf", "-h", str(elf)])
    match = re.search(r"Entry point address:\s*(0x[0-9a-fA-F]+)", header)
    if match is None:
        fail("could not read the ELF entry point")
    entry = int(match.group(1), 16)

    toolchain = run([f"{prefix}gcc", "--version"]).splitlines()[0].strip()
    return binary, elf, entry, toolchain


def check_image_header(image: bytes, *, load: int, entry: int) -> None:
    """Assert the header the assembler emitted is the one firmware needs.

    Every field here is one firmware acts on, and the checks are ordered by how
    silently a wrong value fails: a bad magic is refused at the prompt with a
    message, while a wrong `text_offset` boots nothing and says nothing.
    """
    try:
        header = parse_header(image)
    except Arm64ImageError as error:
        fail(str(error))

    if header.magic != MAGIC:
        fail(
            f"image magic is {header.magic:#010x}, not {MAGIC:#010x}; `booti` "
            "would refuse this with `Bad Linux ARM64 Image magic!`"
        )
    if header.text_offset != load:
        fail(
            f"header text_offset is {header.text_offset:#x} but the payload is "
            f"linked and loaded at {load:#x}; this board relocates to "
            "text_offset, so it would run code from the wrong address"
        )
    if entry != load:
        fail(f"ELF entry {entry:#x} is not the link address {load:#x}")

    try:
        displacement = decode_branch(header.code0)
    except Arm64ImageError as error:
        fail(str(error))
    target = load + displacement
    if not load + HEADER_BYTES <= target < load + len(image):
        fail(
            f"the header's branch lands at {target:#x}, outside the image body "
            f"({load + HEADER_BYTES:#x}..{load + len(image):#x}); firmware would "
            "execute the header or off the end of the payload"
        )

    if header.code1 != 0 or header.res2 or header.res3 or header.res4 or header.res5:
        fail("a reserved header field is non-zero")

    expected_flags = FLAG_PAGE_SIZE_4K | FLAG_PLACE_ANYWHERE
    if header.flags != expected_flags:
        fail(f"header flags are {header.flags:#x}, expected {expected_flags:#x}")

    if header.image_size < len(image):
        fail(
            f"header image_size {header.image_size:#x} is smaller than the "
            f"{len(image):#x}-byte image, so firmware would not reserve all of it"
        )
    if header.image_size > len(image) + MAX_RESERVED_BEYOND_FILE:
        fail(
            f"header image_size {header.image_size:#x} reserves more than "
            f"{MAX_RESERVED_BEYOND_FILE:#x} bytes beyond the {len(image):#x}-byte "
            "image; the probe's only uninitialised memory is its stack"
        )


def write_identity(
    profile: dict[str, object],
    *,
    binary: Path,
    load: int,
    entry: int,
    toolchain: str,
) -> Path:
    image = binary.read_bytes()
    header = parse_header(image)
    identity = {
        "schema": 1,
        "board": profile["board"],
        "soc": profile["soc"],
        "target_profile": TARGET_PROFILE,
        "march": "armv8-a",
        "boot_file": binary.name,
        "load_address": f"{load:#x}",
        "entry_address": f"{entry:#x}",
        "text_offset": f"{header.text_offset:#x}",
        "image_size": f"{header.image_size:#x}",
        "flags": f"{header.flags:#x}",
        "payload_bytes": len(image),
        "payload_sha256": hashlib.sha256(image).hexdigest(),
        "toolchain": toolchain,
    }
    path = OUT_DIR / "identity.json"
    path.write_text(json.dumps(identity, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--qemu-variant",
        action="store_true",
        help=(
            "also build the QEMU `virt` retarget of the same source, so the "
            "instruction stream can be executed before it reaches a board"
        ),
    )
    arguments = parser.parse_args()

    profile = load_profile()
    load = check_link_address(profile)
    prefix = cross_prefix()

    binary, elf, entry, toolchain = build_binary(
        prefix, output_name=boot_file(profile), qemu_variant=False
    )
    image = binary.read_bytes()
    check_image_header(image, load=load, entry=entry)
    identity = write_identity(profile, binary=binary, load=load, entry=entry, toolchain=toolchain)

    print(f"probe:    {binary.relative_to(ROOT)} ({len(image)} bytes)")
    print(f"elf:      {elf.relative_to(ROOT)}")
    print(f"identity: {identity.relative_to(ROOT)}")
    print(f"load:     {load:#x} (link address, header text_offset, and fatload address)")

    if arguments.qemu_variant:
        qemu_binary, qemu_elf, _, _ = build_binary(
            prefix, output_name="probe-qemu.bin", qemu_variant=True
        )
        print(f"qemu:     {qemu_binary.relative_to(ROOT)} ({qemu_binary.stat().st_size} bytes)")
        print(f"          from {qemu_elf.relative_to(ROOT)}")

    print()
    print("This wrote no block device. To stage the probe for a board run:")
    print(f"  cp {binary.relative_to(ROOT)} <mounted FAT32 partition>/")
    print(
        f"  # then boot the board with SW18 = {profile.get('sw18_boot_position', '?')} "
        "and run `just nt98690_boot_check <serial>`"
    )


if __name__ == "__main__":
    main()
