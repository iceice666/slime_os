#!/usr/bin/env python3

"""Build either Milk-V Duo payload and wrap it in the board's bootable FIT.

P3.D's minimal S-mode probe and P3.E's packaged seL4 image deliberately share
one FIT builder. Both use the board's pinned USB-NCM/U-Boot handoff and captured
device tree; only the input bytes, load address, entry address, configuration,
and identity differ.

The seL4 image is the loader ELF flattened from PT_LOAD program headers. FIT
`/incbin/` does not parse ELF, and section-based `objcopy -O binary` can drop the
loader's sectionless payload segment. The first loaded word is replaced by a
RISC-V `jal x0` to the ELF entry while every segment remains at its linked
physical address.
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import struct
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import sha256_file  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
SOURCE_DIR = ROOT / "tools" / "duo" / "payload"
OUT_DIR = ROOT / "build" / "duo-payload"
DEFAULT_SEL4_IMAGE = ROOT / "build" / "slime-sel4-cv1800b-duo.elf"
DEFAULT_SEL4_IMAGE_IDENTITY = ROOT / "build" / "slime-sel4-cv1800b-duo.identity.json"

PT_LOAD = 1
ELF_MAGIC = b"\x7fELF"
EM_RISCV = 243
PAGE_SIZE = 4096
RISCV_AUIPC_GP = (3 << 7) | 0x17


@dataclass(frozen=True)
class Segment:
    offset: int
    paddr: int
    file_size: int
    mem_size: int

# M, A and C only: seL4's own RV64 build emits M/A/C unconditionally, and this
# payload deliberately avoids F/D so it cannot depend on FPU state the firmware
# may not have enabled.
MARCH = "rv64imac_zicsr_zifencei"
MABI = "lp64"

GCC_ATTR = "nixpkgs#pkgsCross.riscv64-embedded.buildPackages.gcc"
BINUTILS_ATTR = "nixpkgs#pkgsCross.riscv64-embedded.buildPackages.binutils"
UBOOT_ATTR = "nixpkgs#ubootTools"
DTC_ATTR = "nixpkgs#dtc"


def fail(message: str) -> NoReturn:
    raise SystemExit(f"duo payload build: {message}")


def load_profile() -> dict[str, object]:
    if not PINS_PATH.is_file():
        fail(f"{PINS_PATH.relative_to(ROOT)} is missing")
    with PINS_PATH.open("rb") as handle:
        pins = tomllib.load(handle)
    profile = pins.get("cv1800b_duo")
    if not isinstance(profile, dict):
        fail("sel4/pins.toml has no [cv1800b_duo] table")
    for key in (
        "board",
        "payload_load_address",
        "dram_base",
        "dram_size",
        "sbi_reservation_bytes",
        "fit_staging_address",
        "fit_config",
    ):
        if key not in profile:
            fail(f"sel4/pins.toml [cv1800b_duo] does not pin {key!r}")
    return profile


def nix_shell(attributes: list[str], script: str) -> subprocess.CompletedProcess[str]:
    if shutil.which("nix") is None:
        fail("`nix` is not on PATH; this build takes its cross toolchain by attribute")
    return subprocess.run(
        ["nix", "shell", *attributes, "--command", "bash", "-euo", "pipefail", "-c", script],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )


def check_link_address(profile: dict[str, object]) -> None:
    """The linker script's base must be the address the board pins.

    Comparing here rather than after the build means a mismatch is reported as a
    pin disagreement, not as a mysterious hang on the board.
    """
    linker_script = SOURCE_DIR / "smoke.ld"
    if not linker_script.is_file():
        fail(f"{linker_script.relative_to(ROOT)} is missing")
    match = re.search(
        r"PAYLOAD_BASE\s*=\s*(0x[0-9A-Fa-f]+)\s*;", linker_script.read_text()
    )
    if match is None:
        fail(f"{linker_script.relative_to(ROOT)} does not define PAYLOAD_BASE")
    linked = match.group(1).lower()
    pinned = str(profile["payload_load_address"]).lower()
    if int(linked, 16) != int(pinned, 16):
        fail(
            f"{linker_script.relative_to(ROOT)} links at {linked} but "
            f"sel4/pins.toml [cv1800b_duo].payload_load_address pins {pinned}"
        )


def build_binary() -> tuple[Path, str]:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    binary = OUT_DIR / "smoke.bin"
    relative_out = OUT_DIR.relative_to(ROOT)
    result = nix_shell(
        [GCC_ATTR, BINUTILS_ATTR],
        f"""
        riscv64-none-elf-gcc -x assembler-with-cpp \
            -march={MARCH} -mabi={MABI} -mcmodel=medany \
            -nostdlib -nostartfiles -ffreestanding \
            -Wl,--build-id=none \
            -T {SOURCE_DIR.relative_to(ROOT)}/smoke.ld \
            -o {relative_out}/smoke.elf {SOURCE_DIR.relative_to(ROOT)}/smoke.S
        riscv64-none-elf-objcopy -O binary {relative_out}/smoke.elf {relative_out}/smoke.bin
        riscv64-none-elf-readelf -h {relative_out}/smoke.elf | grep 'Entry point'
        """,
    )
    if result.returncode != 0:
        fail(f"assembling the payload failed:\n{result.stdout}\n{result.stderr}")
    if not binary.is_file():
        fail("the payload binary was not produced")
    entry = re.search(r"Entry point address:\s*(0x[0-9a-fA-F]+)", result.stdout)
    if entry is None:
        fail(f"could not read the payload's entry point:\n{result.stdout}")
    return binary, entry.group(1).lower()


def build_fit(
    profile: dict[str, object],
    *,
    binary: Path,
    fit: Path,
    load: int,
    entry: int,
    config: str,
    description: str,
) -> Path:
    """Render one pinned FIT source, then assemble it.

    `type = "kernel"` is required because this vendor U-Boot treats a
    `kernel_noload` entry as an offset into the FIT. The `flat_dt` node and
    `os = "linux"` select its RISC-V `(hart_id, fdt_addr)` S-mode handoff; the
    payload is not claiming to be Linux.
    """
    dtb = SOURCE_DIR / "duo.dtb"
    if not dtb.is_file():
        fail(
            f"{dtb.relative_to(ROOT)} is missing; it is the board's own device "
            "tree, captured from /sys/firmware/fdt on the running board"
        )
    rendered = f'''/dts-v1/;

/ {{
    description = "{description}";
    #address-cells = <2>;
    images {{
        kernel-1 {{
            description = "{description}";
            data = /incbin/("{binary.resolve()}");
            type = "kernel";
            arch = "riscv";
            os = "linux";
            compression = "none";
            load = <0x0 {load:#x}>;
            entry = <0x0 {entry:#x}>;
            hash-1 {{ algo = "crc32"; }};
        }};
        fdt-duo {{
            description = "Milk-V Duo device tree, captured from the board";
            data = /incbin/("{dtb.resolve()}");
            type = "flat_dt";
            arch = "riscv";
            compression = "none";
            hash-1 {{ algo = "crc32"; }};
        }};
    }};
    configurations {{
        default = "{config}";
        {config} {{
            description = "{description}";
            kernel = "kernel-1";
            fdt = "fdt-duo";
        }};
    }};
}};
'''
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    its = fit.with_suffix(".its")
    its.write_text(rendered)
    relative_its = its.relative_to(ROOT)
    relative_fit = fit.relative_to(ROOT)
    result = nix_shell(
        [UBOOT_ATTR, DTC_ATTR],
        f"mkimage -f {relative_its} {relative_fit}\nmkimage -l {relative_fit}",
    )
    if result.returncode != 0:
        fail(f"assembling the FIT failed:\n{result.stdout}\n{result.stderr}")
    if not fit.is_file():
        fail("the FIT was not produced")
    print(result.stdout.rstrip())
    return fit


def read_load_segments(path: Path) -> tuple[list[Segment], int]:
    data = path.read_bytes()
    if data[:4] != ELF_MAGIC or data[4] != 2 or data[5] != 1:
        fail(f"{path.relative_to(ROOT)} is not a little-endian ELF64 image")
    machine = struct.unpack_from("<H", data, 18)[0]
    if machine != EM_RISCV:
        fail(f"{path.relative_to(ROOT)} is not RISC-V (e_machine={machine})")
    entry = struct.unpack_from("<Q", data, 24)[0]
    phoff = struct.unpack_from("<Q", data, 32)[0]
    phentsize, phnum = struct.unpack_from("<HH", data, 54)
    segments: list[Segment] = []
    for index in range(phnum):
        base = phoff + index * phentsize
        if struct.unpack_from("<I", data, base)[0] != PT_LOAD:
            continue
        offset, _vaddr, paddr, file_size, mem_size = struct.unpack_from(
            "<QQQQQ", data, base + 8
        )
        if file_size > mem_size or offset + file_size > len(data):
            fail(f"PT_LOAD {index} exceeds the ELF bounds")
        segments.append(Segment(offset, paddr, file_size, mem_size))
    if not segments:
        fail(f"{path.relative_to(ROOT)} declares no PT_LOAD segment")
    return sorted(segments, key=lambda segment: segment.paddr), entry


def encode_jal(source: int, target: int) -> bytes:
    delta = target - source
    if delta % 2 != 0 or not -(1 << 20) <= delta < (1 << 20):
        fail(f"entry {target:#x} is outside one RISC-V JAL from {source:#x}")
    immediate = delta & ((1 << 21) - 1)
    instruction = (
        ((immediate >> 20) & 0x1) << 31
        | ((immediate >> 1) & 0x3FF) << 21
        | ((immediate >> 11) & 0x1) << 20
        | ((immediate >> 12) & 0xFF) << 12
        | 0x6F
    )
    return struct.pack("<I", instruction)


def check_flattened_entry(image: bytes, base: int, entry: int) -> None:
    """The flattened image starts with our JAL and lands on loader `_start`."""
    if len(image) < 4:
        fail("flattened seL4 image is too small to contain an instruction")
    expected = encode_jal(base, entry)
    if image[:4] != expected:
        fail(
            f"flattened seL4 image starts with {image[:4].hex()}, not the "
            f"expected JAL {expected.hex()} to {entry:#x}"
        )
    landing = entry - base
    if landing < 0 or landing + 4 > len(image):
        fail("the seL4 entry lies outside the flattened image")
    first = struct.unpack_from("<I", image, landing)[0]
    if first & 0xFFF != RISCV_AUIPC_GP:
        fail(
            f"the seL4 entry starts with {first:#010x}, not the loader's "
            "`auipc gp` prologue; the ELF entry is not `_start`"
        )


def check_fit_staging_overlap(
    profile: dict[str, object], *, base: int, end: int, fit_bytes: int
) -> None:
    staging = int(str(profile["fit_staging_address"]), 16)
    staging_end = staging + fit_bytes
    if staging < end and base < staging_end:
        fail(
            f"seL4 image span {base:#x}..{end:#x} overlaps the staged FIT "
            f"span {staging:#x}..{staging_end:#x}"
        )


def flatten_sel4(
    path: Path, profile: dict[str, object], *, output: Path
) -> tuple[int, int, int]:
    segments, entry = read_load_segments(path)
    base = segments[0].paddr
    end = max(segment.paddr + segment.mem_size for segment in segments)
    if segments[0].offset != 0:
        fail(
            "the lowest seL4 PT_LOAD does not contain the ELF header; "
            "installing the entry JAL would overwrite live payload bytes"
        )
    if base % PAGE_SIZE != 0:
        fail(f"lowest seL4 segment address {base:#x} is not page-aligned")
    if not base <= entry < end:
        fail(f"entry {entry:#x} lies outside the PT_LOAD span {base:#x}..{end:#x}")
    dram_base = int(str(profile["dram_base"]), 16)
    usable_base = dram_base + int(profile["sbi_reservation_bytes"])
    dram_end = dram_base + int(str(profile["dram_size"]), 16)
    if base < usable_base or end > dram_end:
        fail(
            f"seL4 image span {base:#x}..{end:#x} exceeds the usable Duo "
            f"DRAM window {usable_base:#x}..{dram_end:#x}"
        )
    data = path.read_bytes()
    if data[:4] != ELF_MAGIC:
        fail("the lowest seL4 PT_LOAD does not begin with the ELF header")
    image = bytearray(end - base)
    for segment in segments:
        start = segment.paddr - base
        image[start : start + segment.file_size] = data[
            segment.offset : segment.offset + segment.file_size
        ]
    image[:4] = encode_jal(base, entry)
    check_flattened_entry(image, base, entry)
    output.write_bytes(image)
    return base, entry, end


def build_smoke(profile: dict[str, object]) -> None:
    check_link_address(profile)
    binary, entry_text = build_binary()
    entry = int(entry_text, 16)
    pinned = int(str(profile["payload_load_address"]), 16)
    if entry != pinned:
        fail(f"the built payload entry is {entry:#x}, expected {pinned:#x}")
    fit = build_fit(
        profile,
        binary=binary,
        fit=OUT_DIR / "smoke.itb",
        load=pinned,
        entry=entry,
        config="config-duo",
        description="Slime OS Milk-V Duo bring-up payload",
    )
    identity = {
        "board": profile["board"],
        "soc": profile["soc"],
        "target_profile": "riscv64-duo-bringup",
        "march": MARCH,
        "mabi": MABI,
        "load_address": str(profile["payload_load_address"]),
        "entry_address": entry_text,
        "payload_bytes": binary.stat().st_size,
        "payload_sha256": sha256_file(binary, fail),
        "fit_bytes": fit.stat().st_size,
        "fit_sha256": sha256_file(fit, fail),
        "dtb_sha256": sha256_file(SOURCE_DIR / "duo.dtb", fail),
    }
    manifest = OUT_DIR / "identity.json"
    manifest.write_text(json.dumps(identity, indent=2, sort_keys=True) + "\n")
    print(
        f"duo payload build: {identity['payload_bytes']} byte payload at "
        f"{profile['payload_load_address']}, {identity['fit_bytes']} byte FIT, "
        f"manifest at {manifest.relative_to(ROOT)}"
    )


def build_sel4(
    profile: dict[str, object],
    *,
    image: Path,
    image_identity_path: Path,
    output_stem: str,
) -> None:
    if not image.is_file() or not image_identity_path.is_file():
        fail(
            "the Duo seL4 image is missing; run the matching "
            "`python3 scripts/build/build-sel4.py --platform cv1800b-duo` build first"
        )
    try:
        image_identity = json.loads(image_identity_path.read_text())
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse {image_identity_path.relative_to(ROOT)}: {error}")
    if image_identity.get("target_profile") != "riscv64-sel4-milkv-duo":
        fail("the packaged seL4 image identity names the wrong target profile")
    image_record = image_identity.get("image")
    if not isinstance(image_record, dict) or image_record.get("sha256") != sha256_file(
        image, fail
    ):
        fail("the Duo seL4 ELF does not match its image identity manifest")
    generation = image_identity.get("generation")
    if not isinstance(generation, dict) or not isinstance(generation.get("identity"), str):
        fail("the Duo seL4 identity does not record its embedded generation")

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    binary = OUT_DIR / f"{output_stem}.bin"
    fit_path = OUT_DIR / f"{output_stem}.itb"
    identity_path = OUT_DIR / f"{output_stem}.identity.json"
    load, entry, end = flatten_sel4(image, profile, output=binary)
    config = str(profile["fit_config"])
    fit = build_fit(
        profile,
        binary=binary,
        fit=fit_path,
        load=load,
        entry=entry,
        config=config,
        description="Slime OS seL4 on Milk-V Duo",
    )
    check_fit_staging_overlap(
        profile,
        base=load,
        end=end,
        fit_bytes=fit.stat().st_size,
    )
    identity = {
        "board": profile["board"],
        "soc": profile["soc"],
        "target_profile": image_identity["target_profile"],
        "variant": image_identity.get("variant"),
        "duo_early_fault": image_identity.get("duo_early_fault", False),
        "test_terminator": image_identity.get("test_terminator", False),
        "fit_config": config,
        "load_address": f"{load:#x}",
        "entry_address": f"{entry:#x}",
        "image_end": f"{end:#x}",
        "payload_bytes": binary.stat().st_size,
        "payload_sha256": sha256_file(binary, fail),
        "fit_bytes": fit.stat().st_size,
        "fit_sha256": sha256_file(fit, fail),
        "dtb_sha256": sha256_file(SOURCE_DIR / "duo.dtb", fail),
        "elf_sha256": sha256_file(image, fail),
        "generation_identity": generation["identity"],
        "generation_sha256": generation.get("sha256"),
    }
    identity_path.write_text(json.dumps(identity, indent=2, sort_keys=True) + "\n")
    print(
        f"duo seL4 FIT build: {identity['payload_bytes']} byte image "
        f"{identity['load_address']}..{identity['image_end']}, "
        f"generation {identity['generation_identity']}, manifest at "
        f"{identity_path.relative_to(ROOT)}"
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--sel4",
        action="store_true",
        help="package an already-built cv1800b-duo seL4 loader image",
    )
    parser.add_argument("--image", type=Path, default=DEFAULT_SEL4_IMAGE)
    parser.add_argument("--identity", type=Path, default=DEFAULT_SEL4_IMAGE_IDENTITY)
    parser.add_argument("--output-stem", default="slime-sel4-cv1800b-duo")
    arguments = parser.parse_args()
    profile = load_profile()
    if arguments.sel4:
        build_sel4(
            profile,
            image=arguments.image,
            image_identity_path=arguments.identity,
            output_stem=arguments.output_stem,
        )
    else:
        build_smoke(profile)


if __name__ == "__main__":
    main()
