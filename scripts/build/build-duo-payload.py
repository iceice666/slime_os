#!/usr/bin/env python3

"""P3.D: build the Milk-V Duo bring-up payload and wrap it as a bootable FIT.

This is the board's equivalent of `build-rpi5-media.py`: it turns pinned source
into the exact bytes the board's firmware will accept, and writes an identity
manifest so a later gate can prove the deployed image is this build's.

Two board facts shape the output, both pinned in `sel4/pins.toml
[cv1800b_duo]`:

  * The payload is a flat binary linked at an absolute address, because the
    vendor U-Boot has no ELF loader (`bootelf` is not compiled in) and FIT
    `/incbin/` does not parse program headers.
  * The wrapper is a FIT whose kernel subimage is `type = "kernel"` and which
    carries a `flat_dt`. `kernel_noload` would be wrong: this U-Boot rewrites a
    noload image's entry to a FIT-relative offset, and its RISC-V `bootm` path
    hangs outright when the FIT carries no device tree.

The toolchain comes from `nix`, by attribute, so the emitted bytes do not depend
on whatever cross compiler happens to be on PATH.
"""

from __future__ import annotations

import json
import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import sha256_file  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
SOURCE_DIR = ROOT / "tools" / "duo" / "payload"
OUT_DIR = ROOT / "build" / "duo-payload"

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
    for key in ("board", "payload_load_address", "dram_base", "fit_staging_address"):
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


def build_fit(profile: dict[str, object]) -> Path:
    """Render the FIT source with pinned addresses, then assemble it.

    The `.its` in `tools/duo/payload` carries `@LOAD@` placeholders rather than a
    literal address so this script, and therefore `sel4/pins.toml`, remains the
    single source of truth for where the payload runs.
    """
    template = SOURCE_DIR / "smoke.its"
    if not template.is_file():
        fail(f"{template.relative_to(ROOT)} is missing")
    dtb = SOURCE_DIR / "duo.dtb"
    if not dtb.is_file():
        fail(
            f"{dtb.relative_to(ROOT)} is missing; it is the board's own device "
            "tree, captured from /sys/firmware/fdt on the running board"
        )
    load = str(profile["payload_load_address"])
    rendered = (
        template.read_text()
        .replace("@LOAD@", load)
        .replace("@BINARY@", str((OUT_DIR / "smoke.bin").resolve()))
        .replace("@DTB@", str(dtb.resolve()))
    )
    its = OUT_DIR / "smoke.its"
    its.write_text(rendered)
    fit = OUT_DIR / "smoke.itb"
    relative_out = OUT_DIR.relative_to(ROOT)
    # mkimage shells out to `dtc`, so dtc must be on PATH beside it.
    result = nix_shell(
        [UBOOT_ATTR, DTC_ATTR],
        f"""
        mkimage -f {relative_out}/smoke.its {relative_out}/smoke.itb
        mkimage -l {relative_out}/smoke.itb
        """,
    )
    if result.returncode != 0:
        fail(f"assembling the FIT failed:\n{result.stdout}\n{result.stderr}")
    if not fit.is_file():
        fail("the FIT was not produced")
    print(result.stdout.rstrip())
    return fit


def main() -> None:
    profile = load_profile()
    check_link_address(profile)
    binary, entry = build_binary()
    pinned = str(profile["payload_load_address"])
    if int(entry, 16) != int(pinned, 16):
        fail(
            f"the built payload's entry point is {entry} but "
            f"sel4/pins.toml pins {pinned}"
        )
    fit = build_fit(profile)

    identity = {
        "board": profile["board"],
        "soc": profile["soc"],
        "target_profile": "riscv64-duo-bringup",
        "march": MARCH,
        "mabi": MABI,
        "load_address": pinned,
        "entry_address": entry,
        "payload_bytes": binary.stat().st_size,
        "payload_sha256": sha256_file(binary, fail),
        "fit_bytes": fit.stat().st_size,
        "fit_sha256": sha256_file(fit, fail),
        "dtb_sha256": sha256_file(SOURCE_DIR / "duo.dtb", fail),
    }
    manifest = OUT_DIR / "identity.json"
    manifest.write_text(json.dumps(identity, indent=2, sort_keys=True) + "\n")

    print(
        f"duo payload build: {identity['payload_bytes']} byte payload at {pinned}, "
        f"{identity['fit_bytes']} byte FIT, manifest at "
        f"{manifest.relative_to(ROOT)}"
    )


if __name__ == "__main__":
    main()
