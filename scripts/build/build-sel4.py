#!/usr/bin/env python3

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
from dataclasses import dataclass
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
SEL4_SOURCE = ROOT / "deps" / "sel4"
RUST_SEL4_SOURCE = ROOT / "deps" / "rust-sel4"
BUILD_ROOT = ROOT / "build"
CARGO_BUILD = BUILD_ROOT / "sel4-cargo"
ARTIFACTS = BUILD_ROOT / "sel4-artifacts"


@dataclass(frozen=True)
class Platform:
    """One seL4 build platform: the board or machine an image targets.

    Everything here is what makes two platforms differ. The rest of this
    script — variants, generations, the loader, packaging — is shared, because
    which board an image runs on is orthogonal to which generation it embeds.

    `qemu_dtb` distinguishes the two device-tree routes. `qemu-arm-virt` has no
    device tree until QEMU is asked to dump one, so the build extracts it
    deterministically and passes it in. `bcm2712` ships its description in
    tree (`tools/dts/rpi5b.dts` plus overlays), so passing a DTB would override
    the board's own facts with an emulator's.

    `pins_section` names this platform's `sel4/pins.toml` table, and
    `observed_prefix_section` its pinned artifact hashes: the two platforms
    build different kernels, so one set of hashes cannot describe both.
    """

    name: str
    config: Path
    build_dir: Path
    prefix_dir: Path
    target_profile: str
    pins_section: str
    observed_prefix_section: str
    random_seed: str
    qemu_dtb: bool
    architecture: str
    root_target_key: str
    child_target_name: str
    loader_target_key: str
    cross_compiler_environment: str


QEMU_ARM_VIRT = Platform(
    name="qemu-arm-virt",
    config=ROOT / "sel4" / "config" / "qemu-arm-virt.cmake",
    build_dir=BUILD_ROOT / "sel4-qemu",
    prefix_dir=BUILD_ROOT / "sel4-prefix",
    target_profile="aarch64-sel4-qemu-virt",
    pins_section="qemu_arm_virt",
    observed_prefix_section="observed_prefix",
    # Unchanged from when this was the only platform: the pinned
    # `[observed_prefix]` hashes were observed with this exact seed, and
    # changing it would move `kernel.elf` and report as toolchain drift.
    random_seed="slime-sel4-qemu-arm-virt",
    qemu_dtb=True,
    architecture="aarch64",
    root_target_key="root_target",
    child_target_name="aarch64-sel4-minimal.json",
    loader_target_key="loader_target",
    cross_compiler_environment="CROSS_COMPILER_PREFIX",
)

# P4's physical target. The kernel is a different build from the one above, so
# it installs into its own prefix and pins its own artifact hashes.
BCM2712_RPI5 = Platform(
    name="bcm2712-rpi5",
    config=ROOT / "sel4" / "config" / "bcm2712-rpi5.cmake",
    build_dir=BUILD_ROOT / "sel4-rpi5",
    prefix_dir=BUILD_ROOT / "sel4-rpi5-prefix",
    target_profile="aarch64-rpi5",
    pins_section="bcm2712_rpi5",
    observed_prefix_section="observed_prefix_bcm2712_rpi5",
    random_seed="slime-sel4-bcm2712-rpi5",
    qemu_dtb=False,
    architecture="aarch64",
    root_target_key="root_target",
    child_target_name="aarch64-sel4-minimal.json",
    loader_target_key="loader_target",
    cross_compiler_environment="CROSS_COMPILER_PREFIX",
)

QEMU_RISCV_VIRT = Platform(
    name="qemu-riscv-virt",
    config=ROOT / "sel4" / "config" / "qemu-riscv-virt.cmake",
    build_dir=BUILD_ROOT / "sel4-riscv64-qemu",
    prefix_dir=BUILD_ROOT / "sel4-riscv64-prefix",
    target_profile="riscv64-sel4-qemu-virt",
    pins_section="qemu_riscv_virt",
    observed_prefix_section="observed_prefix_qemu_riscv_virt",
    random_seed="slime-sel4-qemu-riscv-virt",
    qemu_dtb=True,
    architecture="riscv64",
    root_target_key="riscv64_root_target",
    child_target_name="riscv64imac-sel4-minimal.json",
    loader_target_key="riscv64_loader_target",
    cross_compiler_environment="RISCV64_CROSS_COMPILER_PREFIX",
)

CV1800B_DUO = Platform(
    name="cv1800b-duo",
    config=ROOT / "sel4" / "config" / "cv1800b-duo.cmake",
    build_dir=BUILD_ROOT / "sel4-cv1800b-duo",
    prefix_dir=BUILD_ROOT / "sel4-cv1800b-duo-prefix",
    target_profile="riscv64-sel4-milkv-duo",
    pins_section="cv1800b_duo",
    observed_prefix_section="observed_prefix_cv1800b_duo",
    random_seed="slime-sel4-cv1800b-duo",
    qemu_dtb=False,
    architecture="riscv64",
    root_target_key="riscv64_root_target",
    child_target_name="riscv64imac-sel4-minimal.json",
    loader_target_key="riscv64_loader_target",
    cross_compiler_environment="RISCV64_CROSS_COMPILER_PREFIX",
)

# P6's physical target: the Novatek NT98690 (NS02201) H1V1. Its kernel is a
# different build again -- Cortex-A73, 40-bit physical addresses, a GIC-400
# above 4 GiB -- installed into its own prefix with its own pinned hashes. The
# device tree is in-tree in the fork, so there is no QEMU to dump one from.
NS02201_H1V1 = Platform(
    name="ns02201-h1v1",
    config=ROOT / "sel4" / "config" / "ns02201-h1v1.cmake",
    build_dir=BUILD_ROOT / "sel4-ns02201-h1v1",
    prefix_dir=BUILD_ROOT / "sel4-ns02201-h1v1-prefix",
    target_profile="aarch64-sel4-nt98690-h1v1",
    pins_section="ns02201_h1v1",
    observed_prefix_section="observed_prefix_ns02201_h1v1",
    random_seed="slime-sel4-ns02201-h1v1",
    qemu_dtb=False,
    architecture="aarch64",
    root_target_key="root_target",
    child_target_name="aarch64-sel4-minimal.json",
    loader_target_key="loader_target",
    cross_compiler_environment="CROSS_COMPILER_PREFIX",
)

# The physical boards whose product image polls a real UART, and the serial
# kind their pinned `serial` string must name. Every entry pins `reg-shift 2`
# / `reg-io-width 4`, so the root's one 16550 adapter serves each; the kind in
# the pin string is asserted so a swapped board profile fails loudly.
PRODUCT_UART_KINDS: "dict[Platform, str]" = {
    CV1800B_DUO: "dw-apb",
    NS02201_H1V1: "ns16550a",
}

PLATFORMS = {
    platform.name: platform
    for platform in (QEMU_ARM_VIRT, BCM2712_RPI5, QEMU_RISCV_VIRT, CV1800B_DUO, NS02201_H1V1)
}

IMAGE = BUILD_ROOT / "slime-sel4.elf"
MANIFEST = BUILD_ROOT / "slime-sel4.identity.json"
# P5.2's component-graph image is written beside the P5.1 one rather than
# over it, so each gate boots the artifact it asserts about and neither
# invalidates the other's evidence by being built last.
GRAPH_IMAGE = BUILD_ROOT / "slime-sel4-graph.elf"
GRAPH_MANIFEST = BUILD_ROOT / "slime-sel4-graph.identity.json"
SAMPLE_IMAGE = BUILD_ROOT / "slime-sel4-sample.elf"
SAMPLE_MANIFEST = BUILD_ROOT / "slime-sel4-sample.identity.json"
ROLLBACK_IMAGE = BUILD_ROOT / "slime-sel4-rollback.elf"
ROLLBACK_MANIFEST = BUILD_ROOT / "slime-sel4-rollback.identity.json"
GENERATION_IMAGE = BUILD_ROOT / "slime-sel4-generation.elf"
GENERATION_MANIFEST = BUILD_ROOT / "slime-sel4-generation.identity.json"
BOOT_SELECTION_IMAGE = BUILD_ROOT / "slime-sel4-boot-selection.elf"
BOOT_SELECTION_MANIFEST = BUILD_ROOT / "slime-sel4-boot-selection.identity.json"
DEMO_IMAGE = BUILD_ROOT / "slime-sel4-demo.elf"
DEMO_MANIFEST = BUILD_ROOT / "slime-sel4-demo.identity.json"

# Which generation the root task embeds. That is the only difference between the
# images this script builds; see `build_application`. Every other plane now
# builds by closure identity (`scripts/lib/closure_image.py`); these seven
# remain because a legacy or SDK gate still selects them directly.
FIXTURE_VARIANT = "fixture"
GRAPH_VARIANT = "graph"
SAMPLE_VARIANT = "sample"
DEMO_VARIANT = "demo"

# B40 child-CSpace mutations, one per failure mode the capability-layout gate
# asserts the audit refuses.
B40_MUTATIONS = (
    "missing",
    "extra",
    "aliased",
    "wrong_slot",
    "wrong_type",
    "wrong_rights",
)
ROLLBACK_VARIANT = "rollback"
GENERATION_VARIANT = "generation"
BOOT_SELECTION_VARIANT = "boot-selection"
VARIANT_MANIFESTS = {
    GRAPH_VARIANT: "sel4",
    DEMO_VARIANT: "sel4-demo",
    SAMPLE_VARIANT: "sel4-sample",
    ROLLBACK_VARIANT: "sel4-rollback",
    GENERATION_VARIANT: "sel4-generation",
    BOOT_SELECTION_VARIANT: "sel4",
}
VARIANT_TARGET_DIRS = {
    FIXTURE_VARIANT: "root",
    GRAPH_VARIANT: "root-graph",
    DEMO_VARIANT: "root-demo",
    SAMPLE_VARIANT: "root-sample",
    ROLLBACK_VARIANT: "root-rollback",
    GENERATION_VARIANT: "root-generation",
    BOOT_SELECTION_VARIANT: "root-boot-selection",
}
VARIANT_IMAGES = {
    FIXTURE_VARIANT: (IMAGE, MANIFEST),
    GRAPH_VARIANT: (GRAPH_IMAGE, GRAPH_MANIFEST),
    DEMO_VARIANT: (DEMO_IMAGE, DEMO_MANIFEST),
    SAMPLE_VARIANT: (SAMPLE_IMAGE, SAMPLE_MANIFEST),
    ROLLBACK_VARIANT: (ROLLBACK_IMAGE, ROLLBACK_MANIFEST),
    GENERATION_VARIANT: (GENERATION_IMAGE, GENERATION_MANIFEST),
    BOOT_SELECTION_VARIANT: (BOOT_SELECTION_IMAGE, BOOT_SELECTION_MANIFEST),
}

CHILD_MANIFEST = ROOT / "slime-root" / "child" / "Cargo.toml"
CHUNK_SIZE = 1024 * 1024


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 image build: {message}")


def load_pins() -> dict[str, object]:
    if not PINS_PATH.is_file():
        fail(f"missing pin manifest: {PINS_PATH.relative_to(ROOT)}")
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


def require_file(path: Path, description: str) -> Path:
    if not path.is_file():
        fail(f"missing {description}: {path.relative_to(ROOT)}")
    return path


def require_tool(name: str) -> str:
    value = shutil.which(name)
    if value is None:
        fail(f"required tool is not on PATH: {name}")
    return value


def run(
    command: list[str],
    *,
    cwd: Path = ROOT,
    environment: dict[str, str] | None = None,
    description: str,
) -> None:
    rendered = " ".join(command)
    print(f"[{description}] {rendered}", flush=True)
    try:
        process = subprocess.run(
            command,
            cwd=cwd,
            env=environment,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
    except OSError as error:
        fail(f"cannot run {command[0]} for {description}: {error}")
    if process.stdout:
        sys.stdout.write(process.stdout)
        sys.stdout.flush()
    if process.returncode != 0:
        fail(f"{description} failed with exit status {process.returncode}")


def run_output(command: list[str], *, cwd: Path = ROOT, description: str) -> str:
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
        fail(f"cannot run {command[0]} for {description}: {error}")
    if process.returncode != 0:
        detail = process.stdout.strip() or f"exit status {process.returncode}"
        fail(f"{description} failed:\n{detail}")
    return process.stdout.strip()


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as handle:
            while chunk := handle.read(CHUNK_SIZE):
                digest.update(chunk)
    except OSError as error:
        fail(f"cannot hash {path.relative_to(ROOT)}: {error}")
    return digest.hexdigest()


def file_record(path: Path) -> dict[str, object]:
    require_file(path, "build artifact")
    try:
        recorded = path.relative_to(ROOT)
    except ValueError:
        recorded = Path(path.name)
    return {
        "path": str(recorded),
        "bytes": path.stat().st_size,
        "sha256": sha256_file(path),
    }


def directory_digest(directory: Path) -> str:
    """A path-sensitive digest over every file under `directory`.

    Records which Slime sources produced the image without embedding a file
    list that grows with the tree. Paths are relative and sorted, so the digest
    is stable across checkouts and independent of directory iteration order.
    """
    if not directory.is_dir():
        fail(f"missing source directory: {directory.relative_to(ROOT)}")
    digest = hashlib.sha256()
    for path in sorted(p for p in directory.rglob("*") if p.is_file()):
        digest.update(str(path.relative_to(directory)).encode("utf-8"))
        digest.update(b"\0")
        digest.update(bytes.fromhex(sha256_file(path)))
    return digest.hexdigest()


def boot_bundle_identity(platform: Platform) -> str:
    """Versioned identity of the immutable seL4 kernel and loader."""
    kernel = require_file(platform.prefix_dir / "bin" / "kernel.elf", "installed seL4 kernel")
    digest = hashlib.sha256()
    digest.update(b"slime-sel4-boot-bundle-v1\0")
    digest.update(bytes.fromhex(sha256_file(kernel)))
    digest.update(
        bytes.fromhex(directory_digest(RUST_SEL4_SOURCE / "crates" / "sel4-kernel-loader"))
    )
    return digest.hexdigest()


def git_commit(path: Path) -> str:
    commit = run_output(["git", "rev-parse", "HEAD"], cwd=path, description="read submodule pin")
    if re.fullmatch(r"[0-9a-f]{40}", commit) is None:
        fail(f"unexpected commit identity for {path.relative_to(ROOT)}: {commit!r}")
    return commit


def cross_compiler_prefix(platform: Platform) -> str:
    """Resolve the GNU cross toolchain selected for one seL4 architecture."""
    defaults = {
        "aarch64": "aarch64-unknown-linux-gnu-",
        "riscv64": "riscv64-unknown-linux-gnu-",
    }
    prefix = os.environ.get(platform.cross_compiler_environment) or defaults[platform.architecture]
    require_tool(f"{prefix}gcc")
    return prefix


# nixpkgs' compiler wrappers inject host-varying flags through the environment:
# `-frandom-seed=<the dev shell's derivation hash>`, which GCC uses to seed the
# symbol and section names that must differ per file, plus `-isystem` and
# `-fmacro-prefix-map` entries for every package in the shell. Inheriting them
# makes `[observed_prefix]` a function of the dev shell as much as of the
# toolchain: adding a tool to `flake.nix`, or reordering that list, changes the
# derivation hash, changes the seed, and changes `kernel.elf` byte-for-byte —
# reported as toolchain drift by a gate that cannot tell the two apart. They are
# dropped here and the seed is set from a fixed repo-controlled value instead.
#
# The hardening set goes with them. It is the ambient shell's policy for hosted
# userspace, and this is a freestanding kernel that states its own flags in
# `deps/sel4/CMakeLists.txt` and asks for none of it. Most of the set was
# already inert here — `-fno-stack-protector` is appended after the wrapper's
# `-fstack-protector-strong` and wins, and `_FORTIFY_SOURCE` is a libc macro
# with nothing to attach to under `-nostdinc -ffreestanding`. The one that
# reached codegen is `-fzero-call-used-regs=used-gpr`, which adds a
# `mov x16, 0` / `mov x17, 0` pair before every `ret`.
#
# Matching by prefix rather than by exact name is required: the wrappers read
# target- and role-mangled spellings — `NIX_CFLAGS_COMPILE_aarch64_unknown_linux_gnu`,
# `_FOR_BUILD`, `_FOR_TARGET` — rather than the base names.
ENVIRONMENT_FLAG_PREFIXES = (
    "NIX_CFLAGS",
    "NIX_CXXSTDLIB",
    "NIX_FFLAGS",
    "NIX_GNATFLAGS",
    "NIX_HARDENING_ENABLE",
    "NIX_LDFLAGS",
)
# The two groups below reach the build by routes the flag prefixes do not cover,
# and they are classified separately because only one of them is a live leak.
#
# Defense in depth. The bintools wrapper's boolean switches carry no flags
# themselves, so they match none of the prefixes above. Its `ld` reads
# `NIX_SET_BUILD_ID` and appends `--build-id=<NIX_BUILD_ID_STYLE>` *after* the
# kernel's own `-Wl,--build-id=none`, which the linker honours over it — but
# `linker.lds_pp` then discards `.note.gnu.build-id` outright, so no note
# survives into `kernel.elf` either way. Fault-injected with the scrub disabled:
# byte-identical. Dropped anyway, because the protection is the kernel's linker
# script rather than anything this build states, and the loader links through
# `rust-lld` with no such script.
ENVIRONMENT_SWITCH_NAMES = ("NIX_BUILD_ID_STYLE", "NIX_SET_BUILD_ID")
# CMake seeds `CMAKE_<LANG>_FLAGS_INIT` and `CMAKE_EXE_LINKER_FLAGS_INIT` from
# these. The assembler variable is `ASMFLAGS`, not `ASFLAGS`:
# `deps/sel4/CMakeLists.txt` declares `project(seL4 C ASM)`, so `ASM_DIALECT` is
# empty and `CMakeASMInformation.cmake` reads `$ENV{ASMFLAGS}`. The configure
# line passes an explicit `-D` for each of these, and a cache entry beats
# `_INIT` seeding, so today they are belt and braces.
ENVIRONMENT_FLAG_NAMES = ("ASMFLAGS", "CFLAGS", "CXXFLAGS", "LDFLAGS")
# A real redirection, not defense in depth: these carry one store path per
# package in the shell and are prepended to `find_file`/`find_path` search
# order, which no `-D` on the configure line protects. seL4 resolves
# `KERNEL_HELPERS_PATH` (`deps/sel4/CMakeLists.txt`) and `UMM_TYPES` that way,
# so a shell package shipping a `helpers.cmake` or `umm_types.txt` would win
# over the in-tree file and silently change what gets built.
ENVIRONMENT_SEARCH_NAMES = ("CMAKE_INCLUDE_PATH", "CMAKE_LIBRARY_PATH", "CMAKE_PREFIX_PATH")

# GCC's symbol-naming seed replaces the dev shell's derivation hash; each
# platform carries its own in `Platform.random_seed`. One value per platform is
# safe: the seed only suffixes file-scope static and section names, and
# `kernel.elf` links five objects — `kernel_all.c` plus `head.S`, `traps.S`,
# `idle.S`, and `machine_asm.S` — so there is nothing for a shared seed to
# collide with. Thirteen compile edges share these flags in all; the other
# eight are bitfield/pruning scaffolding and libsel4, never co-linked into the
# pinned artifact.


ENVIRONMENT_DROPPED_NAMES = (
    ENVIRONMENT_FLAG_NAMES + ENVIRONMENT_SWITCH_NAMES + ENVIRONMENT_SEARCH_NAMES
)


def sel4_build_environment() -> dict[str, str]:
    """The kernel's build environment, with the ambient shell's build inputs removed.

    Everything the build genuinely needs is kept: `PATH`, the wrappers' own
    `NIX_CC`/`NIX_BINTOOLS` and `NIX_CC_WRAPPER_TARGET_*` role markers, the
    Darwin SDK variables, and the store/SSL/temp settings. The cross compiler
    finds its libc, crt, and include paths through its `nix-support` files
    rather than through the environment, so dropping the flag variables cannot
    strand it.
    """
    return {
        name: value
        for name, value in os.environ.items()
        if not name.startswith(ENVIRONMENT_FLAG_PREFIXES) and name not in ENVIRONMENT_DROPPED_NAMES
    }


def cargo_environment(toolchain: str, platform: Platform) -> dict[str, str]:
    environment = dict(os.environ)
    environment["RUSTUP_TOOLCHAIN"] = toolchain
    # Selects which kernel the loader and root task compile against: the
    # loader's platform module, its link address, and the embedded platform
    # info all come from this prefix.
    environment["SEL4_PREFIX"] = str(platform.prefix_dir)
    # `sel4-sys` runs bindgen over the installed libsel4 headers, and bindgen
    # resolves libclang at run time. Failing here beats a build-script panic
    # minutes into the kernel-loader build.
    libclang = environment.get("LIBCLANG_PATH")
    if not libclang:
        fail(
            "LIBCLANG_PATH is unset, so bindgen cannot generate the libsel4 bindings; "
            "enter the pinned shell with `nix develop` or export LIBCLANG_PATH"
        )
    if not Path(libclang).is_dir():
        fail(f"LIBCLANG_PATH does not name a directory: {libclang}")
    compiler = f"{cross_compiler_prefix(platform)}gcc"
    target_env = platform.architecture.replace("-", "_") + "_unknown_none"
    environment[f"CC_{target_env}"] = compiler
    environment["CC"] = compiler
    return environment


def cargo_build(
    *,
    manifest: Path,
    package: str,
    target: Path | str,
    target_dir: Path,
    environment: dict[str, str],
    description: str,
    cwd: Path = ROOT,
    features: tuple[str, ...] = (),
) -> None:
    command = [
        "cargo",
        "build",
        "--locked",
        "--offline",
        "--release",
        "--manifest-path",
        str(manifest),
        "--package",
        package,
        "--target",
        str(target),
        "--target-dir",
        str(target_dir),
    ]
    if isinstance(target, Path):
        # Only a JSON target specification needs the unstable path-target flag.
        command.extend(["-Z", "json-target-spec"])
    command.extend(
        [
            "-Z",
            "build-std=core,alloc,compiler_builtins",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ]
    )
    if features:
        command.extend(["--features", ",".join(features)])
    run(command, cwd=cwd, environment=environment, description=description)


def child_features() -> tuple[str, ...]:
    require_file(CHILD_MANIFEST, "root child manifest")
    try:
        manifest = tomllib.loads(CHILD_MANIFEST.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse {CHILD_MANIFEST.relative_to(ROOT)}: {error}")
    features = manifest.get("features")
    if features is None:
        return ()
    if not isinstance(features, dict):
        fail("slime-root child [features] must be a table")
    return ("sel4",) if "sel4" in features else ()


# The kernel's CMake rules drive host Python generators (bitfield, invocation,
# hardware/DTS) through a bare `python3`. Missing modules surface as a generator
# traceback several minutes into the build, so they are checked up front.
SEL4_PYTHON_MODULES = ("jinja2", "yaml", "lxml", "ply", "pyfdt", "jsonschema")


def require_sel4_python_modules() -> None:
    python3 = require_tool("python3")
    probe = ";".join(f"import {module}" for module in SEL4_PYTHON_MODULES)
    try:
        process = subprocess.run(
            [python3, "-c", probe],
            cwd=ROOT,
            check=False,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
    except OSError as error:
        fail(f"cannot run {python3}: {error}")
    if process.returncode != 0:
        fail(
            "the seL4 build's host Python generators are missing modules "
            f"({', '.join(SEL4_PYTHON_MODULES)}); enter `nix develop` so `python3` "
            f"provides them.\n{process.stdout.strip()}"
        )


# The exact QEMU machine parameters used to dump each emulator-only platform.
# Random firmware seeds are disabled because the installed DTB is a pinned build
# input, not runtime entropy.
QEMU_DTB_PARAMETERS = {
    "qemu-arm-virt": (
        "qemu-system-aarch64",
        "virt,secure=off,virtualization=on,gic-version=2,dtb-randomness=off",
        "cortex-a53",
        "1024",
    ),
    "qemu-riscv-virt": ("qemu-system-riscv64", "virt", "rv64", "3072"),
}


def dump_device_tree(platform: Platform) -> Path:
    """Dump the platform device tree once, deterministically.

    Only for platforms whose description does not exist until an emulator is
    asked for it (`Platform.qemu_dtb`). A real board ships its device tree in
    the kernel source tree, and dumping QEMU's over it would replace the
    board's own memory map, interrupt controller, and console with a machine
    that is not the target.

    Memory size is the kernel's own `QEMU_MEMORY` default for this platform,
    not the 2048 MiB the product boots with: the kernel derives its physical
    memory window from this description, and the pinned prefix was produced
    with the default. Widening it is a platform change, not a harness knob.
    """
    dtb = platform.build_dir / f"slime-{platform.name}.dtb"
    dtb.parent.mkdir(parents=True, exist_ok=True)
    qemu, machine, cpu, memory = QEMU_DTB_PARAMETERS[platform.name]
    run(
        [
            require_tool(qemu),
            "-machine",
            f"{machine},dumpdtb={dtb}",
            "-cpu",
            cpu,
            "-smp",
            "1",
            "-m",
            memory,
            "-nographic",
            *(["-bios", "none"] if platform.architecture == "riscv64" else []),
        ],
        description="dump platform device tree",
    )
    if platform.architecture == "riscv64":
        # RISC-V `virt` has no `dtb-randomness` machine property. It injects
        # only `/chosen/rng-seed`; delete that runtime entropy from the build
        # input after dumping rather than pinning a different DTB each build.
        run(
            [require_tool("fdtput"), "-d", str(dtb), "/chosen", "rng-seed"],
            description="normalize RISC-V platform device tree",
        )
    return require_file(dtb, "dumped platform device tree")


def configure_and_install_sel4(platform: Platform) -> None:
    require_file(platform.config, f"{platform.name} seL4 configuration")
    require_tool("cmake")
    require_tool("ninja")
    require_tool("dtc")
    # `tools/xmllint.sh` shells out to `xmllint` to validate the syscall and
    # invocation XML before generating headers; its absence surfaces as a bare
    # exit-127 ninja failure several steps into the build.
    require_tool("xmllint")
    require_sel4_python_modules()
    cross_prefix = cross_compiler_prefix(platform)
    platform.build_dir.mkdir(parents=True, exist_ok=True)
    platform.prefix_dir.mkdir(parents=True, exist_ok=True)
    dtb = None
    if platform.qemu_dtb:
        dtb = dump_device_tree(platform)
    # Four reproducibility leaks in the upstream build, all closed here so the
    # observed prefix hashes in `sel4/pins.toml` mean something:
    #  * QEMU's `virt` machine seeds `rng-seed`/`kaslr-seed` into every dumped
    #    device tree, so letting the kernel extract its own DTB produces a
    #    different platform description on every configure. `dump_device_tree`
    #    dumps it once with `dtb-randomness=off` and passes it in.
    #  * The assembler records absolute source and build paths in the kernel's
    #    `.debug_line`, so the ELF depends on where the checkout lives. Both
    #    roots are mapped to fixed logical prefixes.
    #  * GCC's symbol-naming seed and the hardening set arrive from the ambient
    #    dev shell, so the ELF depends on that shell's derivation hash.
    #    `sel4_build_environment` drops them and the fixed `-frandom-seed`
    #    below replaces the seed. See `ENVIRONMENT_FLAG_PREFIXES`.
    #  * Frame-pointer policy is stated rather than inherited. AArch64's GCC
    #    backend needs both switches to erase the wrapper's explicit frame
    #    policy; RISC-V has no leaf-only companion switch.
    frame_flags = (
        "-fomit-frame-pointer -momit-leaf-frame-pointer"
        if platform.architecture == "aarch64"
        else "-fomit-frame-pointer"
    )
    common_flags = (
        f"-ffile-prefix-map={SEL4_SOURCE}=/slime/sel4 "
        f"-ffile-prefix-map={platform.build_dir}=/slime/build "
        f"-frandom-seed={platform.random_seed} {frame_flags}"
    )
    environment = sel4_build_environment()
    configure = [
        "cmake",
        "-S",
        str(SEL4_SOURCE),
        "-B",
        str(platform.build_dir),
        "-G",
        "Ninja",
        "-C",
        str(platform.config),
        f"-DCMAKE_INSTALL_PREFIX={platform.prefix_dir}",
        f"-DCROSS_COMPILER_PREFIX={cross_prefix}",
        f"-DCMAKE_C_FLAGS={common_flags}",
        f"-DCMAKE_ASM_FLAGS={common_flags}",
        # Empty rather than absent: an explicit cache entry is what stops a
        # tree configured under a shell that exported `LDFLAGS` from
        # retaining it. seL4 appends its own linker flags itself.
        "-DCMAKE_EXE_LINKER_FLAGS=",
    ]
    if dtb is not None:
        configure.append(f"-DQEMU_DTB={dtb}")
    run(configure, environment=environment, description="configure seL4")
    run(
        ["cmake", "--build", str(platform.build_dir), "--parallel"],
        environment=environment,
        description="build seL4",
    )
    run(
        ["cmake", "--install", str(platform.build_dir)],
        environment=environment,
        description="install seL4",
    )


def build_sel4_generation(
    manifest: str = "sel4",
    *,
    platform: Platform = QEMU_ARM_VIRT,
    output_name: str | None = None,
    environment: dict[str, str] | None = None,
    component_spec_root: Path | None = None,
    external_components: list[str] | None = None,
    prebuilt_generation: Path | None = None,
) -> Path:
    """Build one aarch64-sel4 generation and return its generation bytes.

    `environment` overrides the ambient one, which C8.14's fault plane uses to
    enable the proxy early-death injection for its variant alone.
    """
    if prebuilt_generation is not None:
        return require_file(prebuilt_generation.resolve(), "prebuilt seL4 generation")
    name = output_name or ("sel4-generation" if manifest == "sel4" else f"{manifest}-generation")
    # Per platform: the components inside are admitted for one exact target
    # profile, so a board generation is not interchangeable with the QEMU one
    # of the same name.
    output = BUILD_ROOT / name if platform is QEMU_ARM_VIRT else BUILD_ROOT / platform.name / name
    output.mkdir(parents=True, exist_ok=True)
    environment = dict(environment if environment is not None else os.environ)
    # The components link against libsel4, so their build scripts need this
    # platform's prefix. Set here rather than inherited: the callers that pass
    # an `environment` build it from `os.environ`, which carries whatever
    # prefix — or none — the ambient shell had.
    environment["SEL4_PREFIX"] = str(platform.prefix_dir)
    # Every executable byte in the generation is admitted for exactly this
    # profile, so a board image cannot embed QEMU-qualified components.
    environment["SLIME_TARGET_PROFILE"] = platform.target_profile
    environment["SLIME_SEL4_MANIFEST"] = manifest
    command = [sys.executable, str(ROOT / "scripts" / "build" / "build-generation.py")]
    if component_spec_root is not None:
        command += ["--component-spec-root", str(component_spec_root)]
    for mapping in external_components or []:
        command += ["--external-component", mapping]
    command.append(str(output))
    run(command, environment=environment, description="build seL4 generation")
    return require_file(output / "generation.bin", "seL4 generation")


def build_product_slisp(platform: Platform) -> tuple[Path, str]:
    """Build the in-tree freestanding Slisp ELF for external admission."""
    output = BUILD_ROOT / f"slisp-product-{platform.architecture}.elf"
    run(
        [
            sys.executable,
            str(ROOT / "scripts" / "build" / "build-c-component.py"),
            "--architecture",
            platform.architecture,
            str(ROOT / "components" / "slisp" / "slisp.c"),
            str(ROOT / "components" / "slisp" / "main.c"),
            str(output),
        ],
        description="build product Slisp component",
    )
    return require_file(output, "product Slisp ELF"), sha256_file(output)


def build_application(
    pins: dict[str, object],
    *,
    variant: str = FIXTURE_VARIANT,
    platform: Platform = QEMU_ARM_VIRT,
    component_spec_root: Path | None = None,
    external_components: list[str] | None = None,
    prebuilt_generation: Path | None = None,
    resolved_generation: Path | None = None,
    duo_early_fault: bool = False,
    test_terminator: bool = False,
    toolchain: str | None = None,
    root_target: Path | None = None,
    child_target: Path | None = None,
    # CP14: when a closure drives the build, the root's role and its declared
    # parameters come from closure data rather than from `variant`. `None`
    # keeps the legacy variant-derived behavior every existing caller relies
    # on, so the two paths coexist until CP15 migrates the last checker.
    closure_root_role: str | None = None,
    closure_root_parameters: tuple[str, ...] = (),
    closure_target_name: str | None = None,
    # CP14: one closed B40 child-CSpace mutation, from a negative build case.
    # Only the closure path accepts it, so an ambient environment variable can
    # no longer make any build produce a deliberately invalid root.
    closure_root_mutation: str | None = None,
) -> tuple[Path, Path, Path | None]:
    rust_sel4 = table(pins, "rust_sel4")
    toolchain = toolchain or text(rust_sel4, "toolchain", "rust_sel4")
    environment = cargo_environment(toolchain, platform)
    root_target = root_target or ROOT / text(
        rust_sel4, platform.root_target_key, "rust_sel4"
    )
    child_target = child_target or RUST_SEL4_SOURCE / "support" / "targets" / platform.child_target_name
    require_file(root_target, "root target specification")
    require_file(child_target, "child target specification")

    child_target_dir = CARGO_BUILD / platform.name / "child"
    child_environment = environment.copy()
    child_remap = f"--remap-path-prefix={child_target_dir}=./target/sel4/{platform.name}/child"
    child_environment["RUSTFLAGS"] = f"{child_environment.get('RUSTFLAGS', '')} {child_remap}".strip()
    cargo_build(
        manifest=CHILD_MANIFEST,
        package="slime-root-child",
        target=child_target,
        target_dir=child_target_dir,
        environment=child_environment,
        description="build root child",
        features=child_features(),
    )
    child_elf = child_target_dir / child_target.stem / "release" / "slime-root-child.elf"
    require_file(child_elf, "root child ELF")

    root_environment = environment.copy()
    root_environment["SLIME_TARGET_PROFILE"] = platform.target_profile
    root_environment["CHILD_ELF"] = str(child_elf.resolve())
    if platform.name == CV1800B_DUO.name:
        frequency = table(pins, platform.pins_section).get("timer_frequency_hz")
        if not isinstance(frequency, int) or isinstance(frequency, bool) or frequency <= 0:
            fail(f"sel4/pins.toml [{platform.pins_section}].timer_frequency_hz must be positive")
        root_environment["SLIME_DUO_TIMEBASE_HZ"] = str(frequency)
    if closure_root_role is not None:
        # Closure-driven: the role and its parameters are the whole selection,
        # and the resolver has already refused a wrong-platform parameter.
        if "qemuKeyboard" in closure_root_parameters:
            root_environment["SLIME_QEMU_KEYBOARD"] = "1"
        if "duoTestTerminator" in closure_root_parameters:
            root_environment["SLIME_DUO_TEST_TERMINATOR"] = "1"
        if closure_root_role == "boot-selector":
            root_environment["SLIME_BOOT_SELECTOR"] = "1"
            root_environment["SLIME_BOOT_BUNDLE_IDENTITY"] = boot_bundle_identity(platform)
            generation = None
        else:
            if resolved_generation is None:
                fail(f"root role {closure_root_role!r} requires a resolved generation")
            generation = resolved_generation.resolve()
            root_environment["SLIME_GENERATION"] = str(generation)
            if closure_root_role == "root-fixture":
                root_environment["SLIME_ROOT_FIXTURE"] = "1"
        if closure_root_role == "reclamation-unwind":
            rustflags = root_environment.get("RUSTFLAGS", "")
            root_environment["RUSTFLAGS"] = (
                f"{rustflags} --cfg slime_b38_force_unwind".strip()
            )
    elif platform.name == QEMU_ARM_VIRT.name and variant == GRAPH_VARIANT:
        # Temporary interactive product path: the root polls QEMU virt's PL011
        # RX FIFO and feeds those bytes through the existing input capability.
        # Plane images keep deterministic scripts, and physical targets do not
        # compile a QEMU address into their root task.
        root_environment["SLIME_QEMU_KEYBOARD"] = "1"
    if closure_root_role is None and platform in PRODUCT_UART_KINDS and variant == GRAPH_VARIANT:
        serial = text(table(pins, platform.pins_section), "serial", platform.pins_section)
        kind = PRODUCT_UART_KINDS[platform]
        match = re.fullmatch(rf"uart0-{kind}-(0x[0-9a-fA-F]+)", serial)
        if match is None:
            fail(
                f"sel4/pins.toml [{platform.pins_section}].serial must name "
                f"uart0-{kind}-<hex-address>"
            )
        root_environment["SLIME_PRODUCT_UART_PADDR"] = match.group(1)
        if test_terminator:
            root_environment["SLIME_PRODUCT_TEST_TERMINATOR"] = "1"
    if duo_early_fault:
        root_environment["SLIME_DUO_EARLY_FAULT"] = "1"
    if closure_root_role is not None:
        pass
    elif variant == BOOT_SELECTION_VARIANT:
        bundle_identity = boot_bundle_identity(platform)
        root_environment["SLIME_BOOT_SELECTOR"] = "1"
        root_environment["SLIME_BOOT_BUNDLE_IDENTITY"] = bundle_identity
        generation = None
    elif resolved_generation is not None:
        generation = resolved_generation.resolve()
        root_environment["SLIME_GENERATION"] = str(generation)
    else:
        manifest = VARIANT_MANIFESTS.get(variant, "sel4")
        generation_environment = None
        if (
            variant in (FIXTURE_VARIANT, GRAPH_VARIANT)
            and component_spec_root is None
            and not external_components
        ):
            slisp_elf, slisp_digest = build_product_slisp(platform)
            generation_environment = dict(os.environ)
            generation_environment["SLIME_PRODUCT_SLISP_SHA256"] = slisp_digest
            external_components = [f"slisp-external={slisp_elf}"]

        generation = build_sel4_generation(
            manifest,
            platform=platform,
            environment=generation_environment,
            component_spec_root=component_spec_root,
            external_components=external_components,
            prebuilt_generation=prebuilt_generation,
        ).resolve()
        root_environment["SLIME_GENERATION"] = str(generation)
        if variant == FIXTURE_VARIANT and platform.name != CV1800B_DUO.name:
            root_environment["SLIME_ROOT_FIXTURE"] = "1"
    if closure_root_mutation is not None:
        if closure_root_role is None:
            fail("a root mutation is only selectable through a closure")
        if closure_root_mutation not in B40_MUTATIONS:
            fail(f"unknown B40 mutation {closure_root_mutation!r}")
        rustflags = root_environment.get("RUSTFLAGS", "")
        root_environment["RUSTFLAGS"] = (
            f"{rustflags} --cfg slime_b40_mutate_{closure_root_mutation}".strip()
        )
    # B40 negative mutations: perturb one child CSpace in exactly one way so
    # the capability-layout audit's refusal is observed rather than assumed.
    # Never set for a product variant.
    mutation = None if closure_root_role is not None else os.environ.get("SLIME_B40_MUTATION")
    if mutation:
        if mutation not in B40_MUTATIONS:
            fail(f"unknown B40 mutation {mutation!r}")
        rustflags = root_environment.get("RUSTFLAGS", "")
        root_environment["RUSTFLAGS"] = f"{rustflags} --cfg slime_b40_mutate_{mutation}".strip()
    # Separate target directories: the images embed different generations, so
    # sharing one would make each build invalidate the others' artifacts and
    # whichever gate ran last would boot a rebuilt image. Keyed by platform for
    # the same reason one level up — the root task compiles against a specific
    # `SEL4_PREFIX`, so a QEMU and a board build of the same variant are
    # different binaries.
    target_name = closure_target_name or VARIANT_TARGET_DIRS[variant]
    root_target_dir = CARGO_BUILD / platform.name / target_name
    rustflags = root_environment.get("RUSTFLAGS", "")
    remap = f"--remap-path-prefix={root_target_dir}=./target/sel4/{platform.name}/{target_name}"
    root_environment["RUSTFLAGS"] = f"{rustflags} {remap}".strip()
    cargo_build(
        manifest=ROOT / "Cargo.toml",
        package="slime-root",
        target=root_target,
        target_dir=root_target_dir,
        environment=root_environment,
        description="build root task",
    )
    root_elf = root_target_dir / root_target.stem / "release" / "slime-root.elf"
    require_file(root_elf, "root task ELF")
    if closure_root_role is not None:
        return child_elf, root_elf, generation
    return child_elf, root_elf, None if variant == BOOT_SELECTION_VARIANT else generation


def build_loader(
    pins: dict[str, object],
    platform: Platform,
    *,
    toolchain: str | None = None,
    loader_target: str | None = None,
) -> tuple[Path, Path]:
    """Build the kernel loader and its host packaging tool.

    Both run with `deps/rust-sel4` as the working directory: that workspace's
    `.cargo/config.toml` supplies the `rust-lld` linker selection and the
    `RUST_TARGET_PATH` its crates expect. Closure callers pass the toolchain and
    loader target explicitly; legacy callers retain the pinned manifest route.
    """
    rust_sel4 = table(pins, "rust_sel4")
    toolchain = toolchain or text(rust_sel4, "toolchain", "rust_sel4")
    loader_target = loader_target or text(
        rust_sel4, platform.loader_target_key, "rust_sel4"
    )
    environment = cargo_environment(toolchain, platform)

    loader_target_dir = CARGO_BUILD / platform.name / "loader"
    cargo_build(
        manifest=RUST_SEL4_SOURCE / "Cargo.toml",
        package="sel4-kernel-loader",
        target=loader_target,
        target_dir=loader_target_dir,
        environment=environment,
        cwd=RUST_SEL4_SOURCE,
        description="build seL4 kernel loader",
    )
    loader = loader_target_dir / loader_target / "release" / "sel4-kernel-loader"
    require_file(loader, "seL4 kernel loader")

    host_target_dir = CARGO_BUILD / "host-tools"
    run(
        [
            "cargo",
            "build",
            "--locked",
            "--offline",
            "--release",
            "--manifest-path",
            str(RUST_SEL4_SOURCE / "Cargo.toml"),
            "--package",
            "sel4-kernel-loader-add-payload",
            "--target-dir",
            str(host_target_dir),
        ],
        cwd=RUST_SEL4_SOURCE,
        environment=environment,
        description="build loader payload tool",
    )
    payload_tool = host_target_dir / "release" / "sel4-kernel-loader-add-payload"
    require_file(payload_tool, "loader payload tool")
    return loader, payload_tool


def package_image(
    payload_tool: Path, loader: Path, root_elf: Path, image: Path, platform: Platform
) -> None:
    run(
        [
            str(payload_tool),
            "--sel4-prefix",
            str(platform.prefix_dir),
            "--loader",
            str(loader),
            "--app",
            str(root_elf),
            "-o",
            str(image),
        ],
        description="package seL4 image",
    )
    require_file(image, "packaged seL4 image")


def copy_artifact(source: Path, name: str, platform: Platform = QEMU_ARM_VIRT) -> Path:
    # Board and QEMU artifacts of the same name are different binaries, so the
    # board's live in their own subdirectory rather than overwriting the ones
    # every existing seL4 gate reads.
    directory = ARTIFACTS if platform is QEMU_ARM_VIRT else ARTIFACTS / platform.name
    directory.mkdir(parents=True, exist_ok=True)
    destination = directory / name
    try:
        shutil.copyfile(source, destination)
    except OSError as error:
        fail(f"cannot copy {source.relative_to(ROOT)} to {destination.relative_to(ROOT)}: {error}")
    return destination


def write_manifest(
    pins: dict[str, object],
    *,
    child_elf: Path,
    root_elf: Path,
    loader: Path,
    payload_tool: Path,
    image: Path = IMAGE,
    manifest_path: Path = MANIFEST,
    variant: str = FIXTURE_VARIANT,
    platform: Platform = QEMU_ARM_VIRT,
    generation: Path | None = None,
    duo_early_fault: bool = False,
    test_terminator: bool = False,
    source_platform: Platform | None = None,
) -> None:
    source_platform = source_platform or platform
    prefix = platform.prefix_dir
    kernel = require_file(prefix / "bin" / "kernel.elf", "installed seL4 kernel")
    kernel_config = require_file(
        prefix / "libsel4" / "include" / "kernel" / "gen_config.json",
        "installed seL4 kernel config",
    )
    libsel4_config = require_file(
        prefix / "libsel4" / "include" / "sel4" / "gen_config.json",
        "installed libsel4 config",
    )
    dtb = require_file(prefix / "support" / "kernel.dtb", "installed seL4 DTB")
    platform_info = require_file(
        prefix / "support" / "platform_gen.yaml", "installed platform metadata"
    )
    root_target = ROOT / text(table(pins, "rust_sel4"), source_platform.root_target_key, "rust_sel4")
    child_target = RUST_SEL4_SOURCE / "support" / "targets" / source_platform.child_target_name
    suffix = "" if variant == FIXTURE_VARIANT else f"-{variant}"
    if duo_early_fault:
        suffix += "-early-fault"
    if test_terminator:
        suffix += "-test-terminator"
    stable_child = copy_artifact(child_elf, f"slime-root-child{suffix}.elf", source_platform)
    stable_root = copy_artifact(root_elf, f"slime-root{suffix}.elf", source_platform)
    stable_loader = copy_artifact(loader, "sel4-kernel-loader", source_platform)
    stable_payload_tool = copy_artifact(payload_tool, "sel4-kernel-loader-add-payload", source_platform)

    manifest_platform = table(pins, source_platform.pins_section)
    manifest = {
        "schema": 1,
        "kind": "slime-sel4-image-identity",
        "source": {
            "sel4": {
                "commit": git_commit(SEL4_SOURCE),
                "release": require_file(SEL4_SOURCE / "VERSION", "seL4 VERSION")
                .read_text(encoding="utf-8")
                .strip(),
            },
            "rust_sel4": {
                "commit": git_commit(RUST_SEL4_SOURCE),
                "toolchain": text(table(pins, "rust_sel4"), "toolchain", "rust_sel4"),
            },
            "slime_root": directory_digest(ROOT / "slime-root"),
            "boot_contracts": directory_digest(ROOT / "boot-contracts" / "src"),
        },
        "config": {
            "pins": file_record(PINS_PATH),
            "cmake": file_record(source_platform.config),
            "root_target": file_record(root_target),
            "child_target": file_record(child_target),
            "kernel_config": file_record(kernel_config),
            "libsel4_config": file_record(libsel4_config),
            "dtb": file_record(dtb),
            "platform_info": file_record(platform_info),
        },
        "elf": {
            "kernel": file_record(kernel),
            "child": file_record(stable_child),
            "root": file_record(stable_root),
            "loader": file_record(stable_loader),
            "payload_tool": file_record(stable_payload_tool),
        },
        "image": file_record(image),
        "component_graph": variant == GRAPH_VARIANT,
        "variant": variant,
        "platform": source_platform.name,
        "target_profile": source_platform.target_profile,
    }
    if duo_early_fault:
        manifest["duo_early_fault"] = True
    if test_terminator:
        manifest["test_terminator"] = True
    if source_platform.qemu_dtb:
        manifest["qemu"] = {
            "machine": text(manifest_platform, "machine", source_platform.pins_section),
            "cpu": text(manifest_platform, "cpu", source_platform.pins_section),
            "cpus": manifest_platform["cpus"],
            "memory_mib": manifest_platform["memory_mib"],
            "version": text(manifest_platform, "qemu_version", source_platform.pins_section),
        }
    else:
        manifest["board"] = {
            "platform": text(manifest_platform, "platform", source_platform.pins_section),
            "soc": text(manifest_platform, "soc", source_platform.pins_section),
            "serial": text(manifest_platform, "serial", source_platform.pins_section),
            "serial_baud": manifest_platform["serial_baud"],
            "boot_files": manifest_platform["boot_files"],
        }
    if generation is not None:
        generation_bytes = require_file(generation, "embedded generation").read_bytes()
        if len(generation_bytes) < 56:
            fail("embedded generation is truncated")
        manifest["generation"] = {
            "bytes": len(generation_bytes),
            "identity": generation_bytes[24:56].hex(),
            "sha256": hashlib.sha256(generation_bytes).hexdigest(),
        }
    if variant == BOOT_SELECTION_VARIANT:
        manifest["boot_bundle_identity"] = boot_bundle_identity(platform)
    encoded = json.dumps(manifest, indent=2, sort_keys=True) + "\n"
    try:
        manifest_path.write_text(encoded, encoding="utf-8")
    except OSError as error:
        fail(f"cannot write {manifest_path.relative_to(ROOT)}: {error}")


def main() -> None:
    parser = argparse.ArgumentParser(description="Build the pinned standalone Slime seL4 image")
    parser.add_argument(
        "--skip-pin-check",
        action="store_true",
        help="skip the initial static source/tool/config pin validation",
    )
    parser.add_argument(
        "--boot-selection",
        action="store_true",
        help="build the immutable disk-backed generation selector as the sole loader app",
    )
    parser.add_argument(
        "--component-graph",
        action="store_true",
        help=(
            "embed the aarch64-sel4-qemu-virt generation so the root task launches "
            "its declared component graph (P5.2), writing a separate image"
        ),
    )
    parser.add_argument(
        "--sample-plane",
        action="store_true",
        help="embed the sample-plane generation (P5.3.4), writing a separate image",
    )
    parser.add_argument(
        "--duo-early-fault",
        action="store_true",
        help="build a Duo-only bounded post-timer fault diagnostic image",
    )
    parser.add_argument(
        "--test-terminator",
        action="store_true",
        help=(
            "build a distinct resident-product image for a physical UART board "
            "with the gate-only reset trigger"
        ),
    )
    parser.add_argument(
        "--demo-plane",
        action="store_true",
        help=(
            "embed the RP2 demo-scoped generation, which both launches the "
            "product component graph and runs the bounded data path, writing a "
            "separate image"
        ),
    )
    parser.add_argument(
        "--generation-plane",
        action="store_true",
        help=(
            "embed the M6.5 generation-command generation (P5.4.3): a "
            "management service holding the only block capability and an "
            "unprivileged client that must ask, writing a separate image"
        ),
    )
    parser.add_argument(
        "--rollback-plane",
        action="store_true",
        help=(
            "embed the M5.6 rollback generation (P5.4.2c): a probe walking the "
            "BootState transition model on two durable slots, writing a "
            "separate image"
        ),
    )
    parser.add_argument(
        "--component-spec-root",
        type=Path,
        help="load component specifications from this directory",
    )
    parser.add_argument(
        "--external-component",
        action="append",
        default=[],
        metavar="NAME=ELF",
        help="forward one external component mapping to the generation builder",
    )
    parser.add_argument(
        "--prebuilt-generation",
        type=Path,
        help="embed an already built generation instead of rebuilding it",
    )
    parser.add_argument(
        "--platform",
        choices=sorted(PLATFORMS),
        default=QEMU_ARM_VIRT.name,
        help=(
            "which board or machine to build for; every artifact, the embedded "
            "generation's target profile, and the loader's console follow from it"
        ),
    )
    arguments = parser.parse_args()
    selected = [
        variant
        for variant, chosen in (
            (GRAPH_VARIANT, arguments.component_graph),
            (DEMO_VARIANT, arguments.demo_plane),
            (SAMPLE_VARIANT, arguments.sample_plane),
            (ROLLBACK_VARIANT, arguments.rollback_plane),
            (GENERATION_VARIANT, arguments.generation_plane),
            (BOOT_SELECTION_VARIANT, arguments.boot_selection),
        )
        if chosen
    ]
    if len(selected) > 1:
        fail("each --*-plane flag selects a different generation; pass one")
    variant = selected[0] if selected else FIXTURE_VARIANT
    if arguments.duo_early_fault and arguments.platform != CV1800B_DUO.name:
        fail("--duo-early-fault requires --platform cv1800b-duo")
    if arguments.duo_early_fault and variant != SAMPLE_VARIANT:
        fail("--duo-early-fault requires --sample-plane")
    if arguments.test_terminator and not any(
        arguments.platform == platform.name for platform in PRODUCT_UART_KINDS
    ):
        names = ", ".join(platform.name for platform in PRODUCT_UART_KINDS)
        fail(f"--test-terminator requires a physical UART platform ({names})")
    if arguments.test_terminator and variant != GRAPH_VARIANT:
        fail("--test-terminator requires --component-graph")
    if arguments.test_terminator and arguments.duo_early_fault:
        fail("--test-terminator cannot be combined with --duo-early-fault")

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    pins = load_pins()
    if not arguments.skip_pin_check:
        run(
            [sys.executable, str(ROOT / "scripts" / "check" / "check-sel4-pins.py")],
            description="verify seL4 pins",
        )
    if arguments.prebuilt_generation is not None and (
        arguments.component_spec_root is not None or arguments.external_component
    ):
        fail(
            "--prebuilt-generation cannot be combined with --component-spec-root "
            "or --external-component"
        )
    if arguments.prebuilt_generation is not None and variant == BOOT_SELECTION_VARIANT:
        fail("--prebuilt-generation cannot be combined with --boot-selection")
    platform = PLATFORMS[arguments.platform]
    BUILD_ROOT.mkdir(parents=True, exist_ok=True)
    configure_and_install_sel4(platform)
    run(
        [
            sys.executable,
            str(ROOT / "scripts" / "check" / "check-sel4-pins.py"),
            "--prefix",
            "--platform",
            platform.name,
        ],
        description="verify installed seL4 prefix",
    )
    child_elf, root_elf, generation = build_application(
        pins,
        variant=variant,
        platform=platform,
        component_spec_root=arguments.component_spec_root,
        external_components=arguments.external_component,
        prebuilt_generation=arguments.prebuilt_generation,
        duo_early_fault=arguments.duo_early_fault,
        test_terminator=arguments.test_terminator,
    )
    loader, payload_tool = build_loader(pins, platform)
    image, manifest_path = VARIANT_IMAGES[variant]
    if platform is not QEMU_ARM_VIRT:
        # A board image is a different artifact from the QEMU image of the same
        # variant, so it never overwrites it: every existing seL4 gate reads the
        # QEMU path by name.
        image = image.with_name(f"{image.stem}-{platform.name}{image.suffix}")
        manifest_path = manifest_path.with_name(
            manifest_path.name.replace(".identity.json", f"-{platform.name}.identity.json")
        )
    if arguments.duo_early_fault:
        image = image.with_name(image.name.replace(".elf", "-early-fault.elf"))
        manifest_path = manifest_path.with_name(
            manifest_path.name.replace(".identity.json", "-early-fault.identity.json")
        )
    if arguments.test_terminator:
        image = image.with_name(image.name.replace(".elf", "-test-terminator.elf"))
        manifest_path = manifest_path.with_name(
            manifest_path.name.replace(
                ".identity.json", "-test-terminator.identity.json"
            )
        )
    package_image(payload_tool, loader, root_elf, image, platform)
    write_manifest(
        pins,
        child_elf=child_elf,
        root_elf=root_elf,
        loader=loader,
        payload_tool=payload_tool,
        image=image,
        manifest_path=manifest_path,
        variant=variant,
        platform=platform,
        generation=generation,
        duo_early_fault=arguments.duo_early_fault,
        test_terminator=arguments.test_terminator,
    )
    print(
        f"seL4 image build: wrote {image.relative_to(ROOT)} and {manifest_path.relative_to(ROOT)}"
    )


if __name__ == "__main__":
    main()
