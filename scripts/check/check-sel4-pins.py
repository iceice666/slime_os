#!/usr/bin/env python3

from __future__ import annotations

import argparse
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
CONFIG_PATHS = {
    "qemu-arm-virt": ROOT / "sel4" / "config" / "qemu-arm-virt.cmake",
    "qemu-riscv-virt": ROOT / "sel4" / "config" / "qemu-riscv-virt.cmake",
    "cv1800b-duo": ROOT / "sel4" / "config" / "cv1800b-duo.cmake",
    "ns02201-h1v1": ROOT / "sel4" / "config" / "ns02201-h1v1.cmake",
}
RPI5_CONFIG_PATH = ROOT / "sel4" / "config" / "bcm2712-rpi5.cmake"
SEL4_PATH = ROOT / "deps" / "sel4"
RUST_SEL4_PATH = ROOT / "deps" / "rust-sel4"
# Each platform installs its own prefix and pins its own artifact hashes: the
# platforms build different kernels, so one hash set cannot describe all.
PREFIX_PATHS = {
    "qemu-arm-virt": (ROOT / "build" / "sel4-prefix", "observed_prefix"),
    "qemu-riscv-virt": (
        ROOT / "build" / "sel4-riscv64-prefix",
        "observed_prefix_qemu_riscv_virt",
    ),
    "bcm2712-rpi5": (
        ROOT / "build" / "sel4-rpi5-prefix",
        "observed_prefix_bcm2712_rpi5",
    ),
    "cv1800b-duo": (
        ROOT / "build" / "sel4-cv1800b-duo-prefix",
        "observed_prefix_cv1800b_duo",
    ),
    "ns02201-h1v1": (
        ROOT / "build" / "sel4-ns02201-h1v1-prefix",
        "observed_prefix_ns02201_h1v1",
    ),
}
HEX_SHA256 = re.compile(r"[0-9a-f]{64}\Z")
DATED_NIGHTLY = re.compile(r"nightly-\d{4}-\d{2}-\d{2}")
# `FORCE` is matched, not ignored: `bcm2712-rpi5.cmake` needs it to override a
# value its included verified profile already cached, and a `set(...)` this
# regex failed to match would be a pinned option the gate silently stopped
# reading.
CMAKE_SET = re.compile(
    r'^set\(\s*([A-Za-z0-9_]+)\s+(?:"([^"]*)"|([^\s\)]+))'
    r'(?:\s+CACHE\s+\w+\s+"[^"]*")?(?:\s+FORCE)?\s*\)$'
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 pin check: {message}")


def require_file(path: Path, description: str) -> Path:
    if not path.is_file():
        fail(f"missing {description}: {path.relative_to(ROOT)}")
    return path


def require_directory(path: Path, description: str) -> Path:
    if not path.is_dir():
        fail(
            f"missing initialized {description}: {path.relative_to(ROOT)}; "
            "run `git submodule update --init --recursive`"
        )
    return path


def load_pins() -> dict[str, object]:
    require_file(PINS_PATH, "pin manifest")
    try:
        pins = tomllib.loads(PINS_PATH.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse {PINS_PATH.relative_to(ROOT)}: {error}")
    if pins.get("schema") != 1:
        fail("unsupported sel4/pins.toml schema (expected 1)")
    return pins


def table(pins: dict[str, object], name: str) -> dict[str, object]:
    value = pins.get(name)
    if not isinstance(value, dict):
        fail(f"sel4/pins.toml is missing [{name}]")
    return value


def text(entry: dict[str, object], key: str, section: str) -> str:
    value = entry.get(key)
    if not isinstance(value, str) or not value:
        fail(f"sel4/pins.toml [{section}].{key} must be non-empty text")
    return value


def integer(entry: dict[str, object], key: str, section: str) -> int:
    value = entry.get(key)
    if not isinstance(value, int) or isinstance(value, bool):
        fail(f"sel4/pins.toml [{section}].{key} must be an integer")
    return value


def boolean(entry: dict[str, object], key: str, section: str) -> bool:
    value = entry.get(key)
    if not isinstance(value, bool):
        fail(f"sel4/pins.toml [{section}].{key} must be true or false")
    return value


def run_output(command: list[str], *, cwd: Path = ROOT) -> str:
    try:
        process = subprocess.run(
            command,
            cwd=cwd,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
    except OSError as error:
        fail(f"cannot run {command[0]}: {error}")
    if process.returncode != 0:
        detail = process.stdout.strip() or f"exit status {process.returncode}"
        fail(f"command failed: {' '.join(command)}\n{detail}")
    return process.stdout.strip()


def git_commit(path: Path) -> str:
    require_directory(path, f"submodule {path.relative_to(ROOT)}")
    commit = run_output(["git", "rev-parse", "HEAD"], cwd=path)
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        fail(f"unexpected git commit identity for {path.relative_to(ROOT)}: {commit!r}")
    return commit


def git_origin(path: Path) -> str:
    return run_output(["git", "config", "--get", "remote.origin.url"], cwd=path)


def git_dirty(path: Path) -> str:
    """Uncommitted modifications inside a pinned submodule.

    A commit hash alone does not identify the sources that were built: a
    tracked file edited in place, or an untracked file the build picks up,
    leaves `rev-parse HEAD` unchanged while the artifacts differ. The pin gate
    fails closed on either.
    """
    return run_output(["git", "status", "--porcelain", "--untracked-files=normal"], cwd=path)


def normalized_repository(value: str) -> str:
    normalized = value.removesuffix("/").removesuffix(".git")
    if normalized.startswith("git@github.com:"):
        normalized = "https://github.com/" + normalized.removeprefix("git@github.com:")
    return normalized


def require_sha256(value: str, description: str) -> str:
    if not HEX_SHA256.fullmatch(value):
        fail(f"invalid SHA-256 pin for {description}: {value!r}")
    return value


def parse_toolchain(path: Path) -> str:
    require_file(path, "Rust toolchain manifest")
    try:
        manifest = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse {path.relative_to(ROOT)}: {error}")
    toolchain = manifest.get("toolchain")
    if not isinstance(toolchain, dict):
        fail(f"{path.relative_to(ROOT)} has no [toolchain] table")
    channel = toolchain.get("channel")
    if not isinstance(channel, str) or not channel:
        fail(f"{path.relative_to(ROOT)} has no pinned toolchain channel")
    return channel


def parse_cmake_cache(path: Path) -> dict[str, str]:
    require_file(path, "seL4 CMake configuration")
    values: dict[str, str] = {}
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        fail(f"cannot read {path.relative_to(ROOT)}: {error}")
    for line_number, raw_line in enumerate(lines, 1):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("include(") and line.endswith(")"):
            continue
        match = CMAKE_SET.fullmatch(line)
        if match is None:
            fail(f"unsupported statement in {path.relative_to(ROOT)}:{line_number}: {line!r}")
        key = match.group(1)
        value = match.group(2) if match.group(2) is not None else match.group(3)
        if key in values:
            fail(f"duplicate {key} in {path.relative_to(ROOT)}")
        values[key] = value
    return values


def expected_cmake_values(profile: dict[str, object], section: str) -> dict[str, str]:
    values = {
        "KernelPlatform": text(profile, "platform", section),
        "KernelSel4Arch": text(profile, "sel4_arch", section),
        "KernelIsMCS": "ON" if boolean(profile, "mcs", section) else "OFF",
        "KernelMaxNumNodes": str(integer(profile, "nodes", section)),
        "KernelVerificationBuild": "ON"
        if boolean(profile, "verification_build", section)
        else "OFF",
        "KernelDebugBuild": "ON" if boolean(profile, "debug_build", section) else "OFF",
        "KernelPrinting": "ON" if boolean(profile, "printing", section) else "OFF",
    }
    if section in ("qemu_arm_virt", "ns02201_h1v1"):
        values.update(
            {
                "KernelArmHypervisorSupport": "ON"
                if boolean(profile, "hypervisor", section)
                else "OFF",
                "KernelArmExportPCNTUser": "ON"
                if boolean(profile, "export_pcnt_user", section)
                else "OFF",
                "KernelArmExportPTMRUser": "ON"
                if boolean(profile, "export_ptmr_user", section)
                else "OFF",
            }
        )
    if section == "cv1800b_duo":
        values["KernelRiscvExportTimeUser"] = (
            "ON" if boolean(profile, "export_time_user", section) else "OFF"
        )
    return values


def check_submodules(pins: dict[str, object]) -> None:
    for section_name, path in (("sel4", SEL4_PATH), ("rust_sel4", RUST_SEL4_PATH)):
        section = table(pins, section_name)
        expected_commit = text(section, "commit", section_name)
        if not re.fullmatch(r"[0-9a-f]{40}", expected_commit):
            fail(f"[{section_name}].commit is not a full 40-character commit")
        actual_commit = git_commit(path)
        if actual_commit != expected_commit:
            fail(f"{path.relative_to(ROOT)} is at {actual_commit}, expected {expected_commit}")
        expected_repository = normalized_repository(text(section, "repository", section_name))
        actual_repository = normalized_repository(git_origin(path))
        if actual_repository != expected_repository:
            fail(
                f"{path.relative_to(ROOT)} origin is {actual_repository}, "
                f"expected {expected_repository}"
            )
        dirty = git_dirty(path)
        if dirty:
            fail(
                f"{path.relative_to(ROOT)} has uncommitted changes, so its pinned "
                f"commit does not identify the built sources:\n{dirty}"
            )

    sel4_release = text(table(pins, "sel4"), "release", "sel4")
    version = (
        require_file(SEL4_PATH / "VERSION", "seL4 VERSION").read_text(encoding="utf-8").strip()
    )
    if version != sel4_release:
        fail(f"deps/sel4/VERSION is {version!r}, expected {sel4_release!r}")


def check_toolchain_and_targets(pins: dict[str, object]) -> None:
    rust_sel4 = table(pins, "rust_sel4")
    expected_toolchain = text(rust_sel4, "toolchain", "rust_sel4")
    # The seL4 artifacts build with the pinned rust-sel4 toolchain, which is
    # not the toolchain the retained legacy gates use. Both must be exact
    # dated nightlies, and the dev shell must install the seL4 one.
    submodule_toolchain = parse_toolchain(RUST_SEL4_PATH / "rust-toolchain.toml")
    if submodule_toolchain != expected_toolchain:
        fail(
            f"deps/rust-sel4/rust-toolchain.toml pins {submodule_toolchain!r}, "
            f"expected {expected_toolchain!r}"
        )
    workspace_toolchain = parse_toolchain(ROOT / "rust-toolchain.toml")
    if not DATED_NIGHTLY.fullmatch(workspace_toolchain):
        fail(
            f"rust-toolchain.toml pins {workspace_toolchain!r}, which is not an exact dated nightly"
        )
    # The dev shell must install both toolchains. It reads the seL4 pin from
    # this file rather than repeating it, so the assertion is that the flake
    # installs that interpolated value, not a duplicated literal.
    flake = (ROOT / "flake.nix").read_text(encoding="utf-8")
    if "rustup toolchain install ${sel4RustToolchain}" not in flake:
        fail(
            "flake.nix does not install the seL4 toolchain from sel4/pins.toml; "
            "`nix develop` would not provide it"
        )
    if "sel4Pins.rust_sel4.toolchain" not in flake:
        fail("flake.nix does not read the seL4 toolchain pin from sel4/pins.toml")
    if f'"{workspace_toolchain}"' not in flake:
        fail(
            f"flake.nix does not install the workspace toolchain {workspace_toolchain} "
            "named by rust-toolchain.toml"
        )
    # `flake.nix` must export `CROSS_COMPILER_PREFIX` as an absolute path into
    # the wrapper's `bin/`, not a bare triple prefix. A bare prefix resolves
    # against `PATH`, and nixpkgs' `pkgsCross.aarch64-multiplatform.stdenv.cc`
    # is a *native* wrapper on `aarch64-linux` that exports no prefixed `gcc`,
    # so the lookup silently reaches the unwrapped compiler and a different
    # `as` — a different `kernel.elf` from the same pinned inputs (B21). The
    # prefix pin cannot catch that itself: it only reports "toolchain drift"
    # without naming which host is odd.
    if 'CROSS_COMPILER_PREFIX = "${crossCC}/bin/${crossCC.targetPrefix}"' not in flake:
        fail(
            "flake.nix must export CROSS_COMPILER_PREFIX as an absolute "
            '"${crossCC}/bin/${crossCC.targetPrefix}" path; a bare triple '
            "prefix resolves per-host and silently changes kernel.elf (B21)"
        )
    if (
        'RISCV64_CROSS_COMPILER_PREFIX = "${riscvCrossCC}/bin/${riscvCrossCC.targetPrefix}"'
        not in flake
    ):
        fail("flake.nix must export an absolute RISCV64_CROSS_COMPILER_PREFIX")

    target_specs = (
        ("root_target", "root_target_sha256", "aarch64-unknown-none"),
        ("riscv64_root_target", "riscv64_root_target_sha256", "riscv64"),
    )
    for target_key, hash_key, llvm_target in target_specs:
        target_text = text(rust_sel4, target_key, "rust_sel4")
        target_path = (ROOT / target_text).resolve()
        try:
            target_path.relative_to(ROOT)
        except ValueError:
            fail(f"[rust_sel4].{target_key} escapes the repository root")
        require_file(target_path, "root-task target specification")
        expected_hash = require_sha256(text(rust_sel4, hash_key, "rust_sel4"), target_key)
        actual_hash = sha256_file(target_path, fail)
        if actual_hash != expected_hash:
            fail(
                f"{target_path.relative_to(ROOT)} SHA-256 is {actual_hash}, "
                f"expected {expected_hash}"
            )
        try:
            target = json.loads(target_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            fail(f"cannot parse {target_path.relative_to(ROOT)}: {error}")
        if target.get("llvm-target") != llvm_target:
            fail(f"{target_key} llvm-target must be {llvm_target}")
        if target.get("panic-strategy") != "abort" or target.get("exe-suffix") != ".elf":
            fail(f"{target_key} must use panic=abort and the .elf executable suffix")

    if text(rust_sel4, "loader_target", "rust_sel4") != "aarch64-unknown-none":
        fail("unsupported AArch64 loader target pin")
    if text(rust_sel4, "riscv64_loader_target", "rust_sel4") != "riscv64imac-unknown-none-elf":
        fail("unsupported RISC-V loader target pin")


def check_profile(pins: dict[str, object]) -> None:
    arm = table(pins, "qemu_arm_virt")
    arm_expected = expected_cmake_values(arm, "qemu_arm_virt")
    arm_actual = parse_cmake_cache(CONFIG_PATHS["qemu-arm-virt"])
    if arm_actual != arm_expected:
        details = [
            f"{key}: expected {arm_expected.get(key)!r}, got {arm_actual.get(key)!r}"
            for key in sorted(set(arm_expected) | set(arm_actual))
            if arm_expected.get(key) != arm_actual.get(key)
        ]
        fail("qemu-arm-virt CMake config disagrees with pins.toml:\n" + "\n".join(details))
    if arm_expected["KernelArmHypervisorSupport"] != "ON":
        fail("qemu-arm-virt product profile must enable hypervisor support")
    if arm_expected["KernelIsMCS"] != "OFF" or arm_expected["KernelMaxNumNodes"] != "1":
        fail("qemu-arm-virt product profile must be non-MCS and single-node")
    if text(arm, "machine", "qemu_arm_virt") != "virt,virtualization=on":
        fail("qemu-arm-virt machine pin must be virt,virtualization=on")
    if text(arm, "cpu", "qemu_arm_virt") != "cortex-a53":
        fail("qemu-arm-virt CPU pin must be cortex-a53")
    if (
        integer(arm, "cpus", "qemu_arm_virt") != 1
        or integer(arm, "memory_mib", "qemu_arm_virt") != 2048
    ):
        fail("qemu-arm-virt QEMU shape must be one CPU and 2048 MiB")

    riscv = table(pins, "qemu_riscv_virt")
    riscv_expected = expected_cmake_values(riscv, "qemu_riscv_virt")
    riscv_actual = parse_cmake_cache(CONFIG_PATHS["qemu-riscv-virt"])
    if riscv_actual != riscv_expected:
        details = [
            f"{key}: expected {riscv_expected.get(key)!r}, got {riscv_actual.get(key)!r}"
            for key in sorted(set(riscv_expected) | set(riscv_actual))
            if riscv_expected.get(key) != riscv_actual.get(key)
        ]
        fail("qemu-riscv-virt CMake config disagrees with pins.toml:\n" + "\n".join(details))
    if riscv_expected["KernelIsMCS"] != "OFF" or riscv_expected["KernelMaxNumNodes"] != "1":
        fail("qemu-riscv-virt product profile must be non-MCS and single-node")
    if (
        text(riscv, "machine", "qemu_riscv_virt") != "virt"
        or text(riscv, "cpu", "qemu_riscv_virt") != "rv64"
    ):
        fail("qemu-riscv-virt machine/CPU pins must be virt/rv64")
    if (
        integer(riscv, "cpus", "qemu_riscv_virt") != 1
        or integer(riscv, "memory_mib", "qemu_riscv_virt") != 3072
    ):
        fail("qemu-riscv-virt QEMU shape must be one CPU and 3072 MiB")

    duo = table(pins, "cv1800b_duo")
    duo_expected = expected_cmake_values(duo, "cv1800b_duo")
    duo_actual = parse_cmake_cache(CONFIG_PATHS["cv1800b-duo"])
    if duo_actual != duo_expected:
        details = [
            f"{key}: expected {duo_expected.get(key)!r}, got {duo_actual.get(key)!r}"
            for key in sorted(set(duo_expected) | set(duo_actual))
            if duo_expected.get(key) != duo_actual.get(key)
        ]
        fail("cv1800b-duo CMake config disagrees with pins.toml:\n" + "\n".join(details))
    if duo_expected["KernelIsMCS"] != "OFF" or duo_expected["KernelMaxNumNodes"] != "1":
        fail("cv1800b-duo product profile must be non-MCS and single-node")
    if text(duo, "cpu", "cv1800b_duo") != "thead-c906":
        fail("cv1800b-duo CPU pin must name the T-Head C906")
    if text(duo, "mmu", "cv1800b_duo") != "sv39":
        fail("cv1800b-duo MMU pin must be Sv39")
    if integer(duo, "timer_frequency_hz", "cv1800b_duo") != 25_000_000:
        fail("cv1800b-duo timer frequency must match the observed DT")
    if text(duo, "serial", "cv1800b_duo") != "uart0-dw-apb-0x04140000":
        fail("cv1800b-duo UART0 must match the observed DW APB MMIO identity")
    duo_dts = (ROOT / "deps" / "sel4" / "tools" / "dts" / "cv1800b-duo.dts").read_text(
        encoding="utf-8"
    )
    uart_node = re.search(
        r"serial@04140000\s*\{(?P<body>.*?)\n\s*\};",
        duo_dts,
        flags=re.DOTALL,
    )
    if uart_node is None:
        fail("cv1800b-duo DT is missing UART0 at 0x04140000")
    uart_body = uart_node.group("body")
    for fact in (
        'compatible = "snps,dw-apb-uart";',
        "reg = <0x00 0x4140000 0x00 0x1000>;",
        "reg-shift = <0x02>;",
        "reg-io-width = <0x04>;",
    ):
        if fact not in uart_body:
            fail(f"cv1800b-duo UART0 DT fact missing: {fact}")
    if integer(duo, "timer_irq", "cv1800b_duo") != 17:
        fail("cv1800b-duo timer IRQ must match the observed RTC alarm source")
    if integer(duo, "max_irq", "cv1800b_duo") != 101:
        fail("cv1800b-duo PLIC source count must match riscv,ndev")
    if integer(duo, "usable_memory_bytes", "cv1800b_duo") != 0x03F00000:
        fail("cv1800b-duo usable memory must exclude the OpenSBI reservation")
    expected_duo_boot_files = [
        "slime-sel4-cv1800b-duo.itb",
        "slime-sel4-sample-cv1800b-duo.itb",
        "slime-sel4-sample-cv1800b-duo-early-fault.itb",
    ]
    if duo.get("boot_files") != expected_duo_boot_files:
        fail("cv1800b-duo boot files must pin the product, sample, and fault FITs")

    # P6.B: the H1V1's product half must agree with its CMake config, with the
    # facts the P6.A probe measured on the board, and with the seL4 fork's own
    # device tree for the platform -- three sources that can drift apart
    # independently, none of which is allowed to be a Slime-side assertion.
    h1v1 = table(pins, "ns02201_h1v1")
    h1v1_expected = expected_cmake_values(h1v1, "ns02201_h1v1")
    h1v1_actual = parse_cmake_cache(CONFIG_PATHS["ns02201-h1v1"])
    if h1v1_actual != h1v1_expected:
        details = [
            f"{key}: expected {h1v1_expected.get(key)!r}, got {h1v1_actual.get(key)!r}"
            for key in sorted(set(h1v1_expected) | set(h1v1_actual))
            if h1v1_expected.get(key) != h1v1_actual.get(key)
        ]
        fail("ns02201-h1v1 CMake config disagrees with pins.toml:\n" + "\n".join(details))
    if h1v1_expected["KernelIsMCS"] != "OFF" or h1v1_expected["KernelMaxNumNodes"] != "1":
        fail("ns02201-h1v1 product profile must be non-MCS and single-node")
    if h1v1_expected["KernelArmHypervisorSupport"] != "ON":
        fail("ns02201-h1v1 must be a hypervisor build: firmware enters at EL2 and the loader asserts it")
    if text(h1v1, "cpu", "ns02201_h1v1") != "cortex-a73":
        fail("ns02201-h1v1 CPU pin must name the Cortex-A73 the board reported")
    if integer(h1v1, "entry_el_observed", "ns02201_h1v1") != 2:
        fail("ns02201-h1v1 observed entry EL must be 2, the level the hypervisor build assumes")
    if integer(h1v1, "physical_address_bits", "ns02201_h1v1") != 40:
        fail("ns02201-h1v1 physical address width must be the 40 bits PARange 2 encodes")
    frequency = integer(h1v1, "timer_frequency_hz", "ns02201_h1v1")
    if frequency != integer(h1v1, "cntfrq_el0_primary_hz", "ns02201_h1v1"):
        fail("ns02201-h1v1 timer frequency must be the CNTFRQ_EL0 the board reported")
    if frequency != 12_000_000:
        fail("ns02201-h1v1 timer frequency must be 12 MHz")
    if integer(h1v1, "timer_irq", "ns02201_h1v1") != 30:
        fail("ns02201-h1v1 timer IRQ must be CNTP's PPI 30, the line a hypervisor seL4 leaves to userspace")
    if integer(h1v1, "max_irq", "ns02201_h1v1") != integer(h1v1, "gic_irqs", "ns02201_h1v1"):
        fail("ns02201-h1v1 max_irq must be the line count GICD_TYPER reported")
    if text(h1v1, "serial", "ns02201_h1v1") != "uart0-ns16550a-0x2f0130000":
        fail("ns02201-h1v1 UART0 must match the observed 16550 MMIO identity")
    h1v1_dts = (ROOT / "deps" / "sel4" / "tools" / "dts" / "ns02201-h1v1.dts").read_text(
        encoding="utf-8"
    )
    uart_node = re.search(
        r"uart@2f0130000\s*\{(?P<body>.*?)\n\s*\};",
        h1v1_dts,
        flags=re.DOTALL,
    )
    if uart_node is None:
        fail("ns02201-h1v1 DT is missing UART0 at 0x2f0130000")
    uart_body = uart_node.group("body")
    for fact in (
        'compatible = "ns16550a";',
        "reg = <0x02 0xf0130000 0x00 0x1000>;",
        "reg-shift = <0x02>;",
        "reg-io-width = <0x04>;",
    ):
        if fact not in uart_body:
            fail(f"ns02201-h1v1 UART0 DT fact missing: {fact}")
    memory_base = int(text(h1v1, "sel4_memory_base", "ns02201_h1v1"), 16)
    memory_size = int(text(h1v1, "sel4_memory_size", "ns02201_h1v1"), 16)
    overlay = (
        ROOT / "deps" / "sel4" / "src" / "plat" / "ns02201" / "overlay-ns02201-h1v1.dts"
    ).read_text(encoding="utf-8")
    memory_node = re.search(
        rf"memory@{memory_base:x}\s*\{{(?P<body>.*?)\n\s*\}};", overlay, flags=re.DOTALL
    )
    if memory_node is None or (
        f"reg = <0x0 {memory_base:#x} 0x0 {memory_size:#x}>;" not in memory_node.group("body")
    ):
        fail(
            "ns02201-h1v1 overlay memory node must declare the pinned window "
            f"{memory_base:#x}+{memory_size:#x}; it is what places the kernel"
        )
    if memory_base % 0x20_0000 != 0:
        fail("ns02201-h1v1 memory base must be 2 MiB-aligned")
    expected_h1v1_boot_files = [
        "slime-nt98690-probe.bin",
        "slime-sel4-sample-ns02201-h1v1.bin",
        "slime-sel4-ns02201-h1v1-test-terminator.bin",
    ]
    if h1v1.get("boot_files") != expected_h1v1_boot_files:
        fail(
            "ns02201-h1v1 boot files must pin the P6.A probe, the P6.B sample "
            "image, and the P6.C resident-product gate image"
        )

    rpi5 = parse_cmake_cache(RPI5_CONFIG_PATH)
    include = "${CMAKE_CURRENT_LIST_DIR}/../../deps/sel4/configs/AARCH64_bcm2712_verified.cmake"
    # The inherited verified profile supplies platform/architecture, turns
    # printing off, and sets `KernelVerificationBuild ON`; this product overlay
    # must explicitly restore every runtime mechanism slime-root consumes.
    #
    # The three printing-related entries are pinned here so the board's
    # observability cannot be lost silently: without them the kernel emits no
    # UART output and P4/RP3 have no evidence path. They also record that this
    # kernel is deliberately outside the verified set — see the config's own
    # comment for the cost.
    required_rpi5 = {
        "KernelIsMCS": "OFF",
        "KernelMaxNumNodes": "1",
        "KernelVerificationBuild": "OFF",
        "KernelDebugBuild": "ON",
        "KernelPrinting": "ON",
        "KernelArmExportPCNTUser": "ON",
        "KernelArmExportPTMRUser": "ON",
    }
    source = RPI5_CONFIG_PATH.read_text(encoding="utf-8").splitlines()
    if not source or source[0].strip() != f"include({include})":
        fail("bcm2712-rpi5 CMake config does not inherit the pinned verified profile")
    if rpi5 != required_rpi5:
        details = [
            f"{key}: expected {required_rpi5.get(key)!r}, got {rpi5.get(key)!r}"
            for key in sorted(set(required_rpi5) | set(rpi5))
            if required_rpi5.get(key) != rpi5.get(key)
        ]
        fail("bcm2712-rpi5 CMake config is incomplete:\n" + "\n".join(details))


def check_rustup_policy(pins: dict[str, object]) -> None:
    """The pinned toolchain must already be installed, with `rust-src`.

    Nothing here fetches: `build-std` needs the `rust-src` component present
    locally, and discovering that mid-build wastes a full kernel configure.
    The rustc version string is not compared against the channel date — a
    dated nightly ships the previous day's commit, so the name is the pin.
    """
    expected = text(table(pins, "rust_sel4"), "toolchain", "rust_sel4")
    rustup = shutil.which("rustup")
    if rustup is None:
        fail("rustup is not on PATH")
    installed = run_output([rustup, "toolchain", "list"])
    if not any(line.split()[0].startswith(f"{expected}-") for line in installed.splitlines()):
        fail(
            f"required Rust toolchain {expected} is not installed; "
            f"run `rustup toolchain install {expected} --profile minimal "
            "--component rust-src` or enter `nix develop`"
        )
    components = run_output([rustup, "component", "list", "--toolchain", expected, "--installed"])
    if not any(line.startswith("rust-src") for line in components.splitlines()):
        fail(
            f"toolchain {expected} lacks the rust-src component required by "
            f"-Z build-std; run `rustup component add rust-src --toolchain {expected}`"
        )


def check_qemu_version(pins: dict[str, object]) -> None:
    for section, executable in (
        ("qemu_arm_virt", "qemu-system-aarch64"),
        ("qemu_riscv_virt", "qemu-system-riscv64"),
    ):
        expected = text(table(pins, section), "qemu_version", section)
        qemu = shutil.which(executable)
        if qemu is None:
            fail(f"{executable} is not on PATH")
        output = run_output([qemu, "--version"])
        match = re.search(r"QEMU emulator version ([0-9]+(?:\.[0-9]+)+)", output)
        if match is None:
            fail(f"cannot parse QEMU version from: {output!r}")
        if match.group(1) != expected:
            fail(f"{executable} version is {match.group(1)}, expected {expected}")


def check_prefix(pins: dict[str, object], platform: str) -> None:
    prefix, section = PREFIX_PATHS[platform]
    observed = table(pins, section)
    files = {
        "kernel_sha256": prefix / "bin" / "kernel.elf",
        "kernel_config_sha256": prefix / "libsel4" / "include" / "kernel" / "gen_config.json",
        "libsel4_config_sha256": prefix / "libsel4" / "include" / "sel4" / "gen_config.json",
        "dtb_sha256": prefix / "support" / "kernel.dtb",
        "platform_info_sha256": prefix / "support" / "platform_gen.yaml",
    }
    rebuild = {
        "qemu-arm-virt": "just sel4_qemu_image_check",
        "qemu-riscv-virt": "just riscv64_qemu_image_check",
        "bcm2712-rpi5": "just sel4_rpi5_image_check",
        "cv1800b-duo": "just sel4_duo_image_check",
        "ns02201-h1v1": "just sel4_nt98690_image_check",
    }[platform]
    for key, path in files.items():
        require_file(path, f"installed seL4 prefix artifact ({key})")
        expected = require_sha256(text(observed, key, section), key)
        actual = sha256_file(path, fail)
        if actual != expected:
            fail(
                f"{path.relative_to(ROOT)} SHA-256 is {actual}, expected {expected}; "
                f"rebuild with `{rebuild}` or inspect toolchain drift"
            )


def main() -> None:
    parser = argparse.ArgumentParser(description="Verify pinned standalone seL4 inputs")
    parser.add_argument(
        "--prefix",
        action="store_true",
        help="also require the installed seL4 prefix to match observed artifact hashes",
    )
    parser.add_argument(
        "--platform",
        choices=sorted(PREFIX_PATHS),
        default="qemu-arm-virt",
        help="which platform's installed prefix --prefix validates",
    )
    parser.add_argument(
        "--skip-host-tools",
        action="store_true",
        help="skip installed rustup and QEMU version checks (for static CI pin validation)",
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    pins = load_pins()
    check_submodules(pins)
    check_toolchain_and_targets(pins)
    check_profile(pins)
    if not arguments.skip_host_tools:
        check_rustup_policy(pins)
        check_qemu_version(pins)
    if arguments.prefix:
        check_prefix(pins, arguments.platform)
    print("seL4 pin check: exact source, toolchain, target, config, and host pins verified")


if __name__ == "__main__":
    main()
