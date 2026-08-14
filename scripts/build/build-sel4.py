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
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
SEL4_SOURCE = ROOT / "deps" / "sel4"
RUST_SEL4_SOURCE = ROOT / "deps" / "rust-sel4"
SEL4_CONFIG = ROOT / "sel4" / "config" / "qemu-arm-virt.cmake"
BUILD_ROOT = ROOT / "build"
SEL4_BUILD = BUILD_ROOT / "sel4-qemu"
SEL4_PREFIX = BUILD_ROOT / "sel4-prefix"
CARGO_BUILD = BUILD_ROOT / "sel4-cargo"
ARTIFACTS = BUILD_ROOT / "sel4-artifacts"
IMAGE = BUILD_ROOT / "slime-sel4.elf"
MANIFEST = BUILD_ROOT / "slime-sel4.identity.json"
# P5.2's component-graph image is written beside the P5.1 one rather than
# over it, so each gate boots the artifact it asserts about and neither
# invalidates the other's evidence by being built last.
GRAPH_IMAGE = BUILD_ROOT / "slime-sel4-graph.elf"
GRAPH_MANIFEST = BUILD_ROOT / "slime-sel4-graph.identity.json"
CHANNEL_IMAGE = BUILD_ROOT / "slime-sel4-channel.elf"
CHANNEL_MANIFEST = BUILD_ROOT / "slime-sel4-channel.identity.json"
LOAN_IMAGE = BUILD_ROOT / "slime-sel4-loan.elf"
LOAN_MANIFEST = BUILD_ROOT / "slime-sel4-loan.identity.json"
SPAWN_IMAGE = BUILD_ROOT / "slime-sel4-spawn.elf"
SPAWN_MANIFEST = BUILD_ROOT / "slime-sel4-spawn.identity.json"
SAMPLE_IMAGE = BUILD_ROOT / "slime-sel4-sample.elf"
SAMPLE_MANIFEST = BUILD_ROOT / "slime-sel4-sample.identity.json"
STREAM_IMAGE = BUILD_ROOT / "slime-sel4-stream.elf"
STREAM_MANIFEST = BUILD_ROOT / "slime-sel4-stream.identity.json"
SUPERVISION_IMAGE = BUILD_ROOT / "slime-sel4-supervision.elf"
SUPERVISION_MANIFEST = BUILD_ROOT / "slime-sel4-supervision.identity.json"
RECLAMATION_IMAGE = BUILD_ROOT / "slime-sel4-reclamation.elf"
RECLAMATION_MANIFEST = BUILD_ROOT / "slime-sel4-reclamation.identity.json"
CROSSING_IMAGE = BUILD_ROOT / "slime-sel4-crossing.elf"
CROSSING_MANIFEST = BUILD_ROOT / "slime-sel4-crossing.identity.json"
CALL_IMAGE = BUILD_ROOT / "slime-sel4-call.elf"
CALL_MANIFEST = BUILD_ROOT / "slime-sel4-call.identity.json"
QOS_IMAGE = BUILD_ROOT / "slime-sel4-qos.elf"
STRESS_IMAGE = BUILD_ROOT / "slime-sel4-stress.elf"
STRESS_MANIFEST = BUILD_ROOT / "slime-sel4-stress.identity.json"
QOS_MANIFEST = BUILD_ROOT / "slime-sel4-qos.identity.json"
OPERATION_IMAGE = BUILD_ROOT / "slime-sel4-operation.elf"
OPERATION_MANIFEST = BUILD_ROOT / "slime-sel4-operation.identity.json"
VISIBILITY_IMAGE = BUILD_ROOT / "slime-sel4-visibility.elf"
VISIBILITY_MANIFEST = BUILD_ROOT / "slime-sel4-visibility.identity.json"
BOOT_IMAGE = BUILD_ROOT / "slime-sel4-boot.elf"
BOOT_MANIFEST = BUILD_ROOT / "slime-sel4-boot.identity.json"
STORAGE_IMAGE = BUILD_ROOT / "slime-sel4-storage.elf"
STORAGE_MANIFEST = BUILD_ROOT / "slime-sel4-storage.identity.json"
STORE_IMAGE = BUILD_ROOT / "slime-sel4-store.elf"
STORE_MANIFEST = BUILD_ROOT / "slime-sel4-store.identity.json"
ROLLBACK_IMAGE = BUILD_ROOT / "slime-sel4-rollback.elf"
ROLLBACK_MANIFEST = BUILD_ROOT / "slime-sel4-rollback.identity.json"
RECOVERY_IMAGE = BUILD_ROOT / "slime-sel4-recovery.elf"
RECOVERY_MANIFEST = BUILD_ROOT / "slime-sel4-recovery.identity.json"
GENERATION_IMAGE = BUILD_ROOT / "slime-sel4-generation.elf"
GENERATION_MANIFEST = BUILD_ROOT / "slime-sel4-generation.identity.json"
DIRECTORY_IMAGE = BUILD_ROOT / "slime-sel4-directory.elf"
DIRECTORY_MANIFEST = BUILD_ROOT / "slime-sel4-directory.identity.json"
FILESYSTEM_IMAGE = BUILD_ROOT / "slime-sel4-filesystem.elf"
FILESYSTEM_MANIFEST = BUILD_ROOT / "slime-sel4-filesystem.identity.json"
DANGO_IMAGE = BUILD_ROOT / "slime-sel4-dango.elf"
DANGO_MANIFEST = BUILD_ROOT / "slime-sel4-dango.identity.json"
INPUT_IMAGE = BUILD_ROOT / "slime-sel4-input.elf"
INPUT_MANIFEST = BUILD_ROOT / "slime-sel4-input.identity.json"
POWERBOX_IMAGE = BUILD_ROOT / "slime-sel4-powerbox.elf"
POWERBOX_MANIFEST = BUILD_ROOT / "slime-sel4-powerbox.identity.json"
TRANSFER_IMAGE = BUILD_ROOT / "slime-sel4-transfer.elf"
TRANSFER_MANIFEST = BUILD_ROOT / "slime-sel4-transfer.identity.json"
BOOT_SELECTION_IMAGE = BUILD_ROOT / "slime-sel4-boot-selection.elf"
BOOT_SELECTION_MANIFEST = BUILD_ROOT / "slime-sel4-boot-selection.identity.json"

# Which generation the root task embeds. That is the only difference between the
# images this script builds; see `build_application`.
FIXTURE_VARIANT = "fixture"
GRAPH_VARIANT = "graph"
CHANNEL_VARIANT = "channel"
LOAN_VARIANT = "loan"
SPAWN_VARIANT = "spawn"
SAMPLE_VARIANT = "sample"
STREAM_VARIANT = "stream"
SUPERVISION_VARIANT = "supervision"
RECLAMATION_VARIANT = "reclamation"

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
CROSSING_VARIANT = "crossing"
CALL_VARIANT = "call"
QOS_VARIANT = "qos"
STRESS_VARIANT = "stress"
OPERATION_VARIANT = "operation"
VISIBILITY_VARIANT = "visibility"
BOOT_VARIANT = "boot"
STORAGE_VARIANT = "storage"
STORE_VARIANT = "store"
ROLLBACK_VARIANT = "rollback"
RECOVERY_VARIANT = "recovery"
GENERATION_VARIANT = "generation"
DIRECTORY_VARIANT = "directory"
FILESYSTEM_VARIANT = "filesystem"
DANGO_VARIANT = "dango"
INPUT_VARIANT = "input"
POWERBOX_VARIANT = "powerbox"
TRANSFER_VARIANT = "transfer"
BOOT_SELECTION_VARIANT = "boot-selection"
VARIANT_MANIFESTS = {
    GRAPH_VARIANT: "sel4",
    CHANNEL_VARIANT: "sel4-channel",
    LOAN_VARIANT: "sel4-loan",
    SPAWN_VARIANT: "sel4-spawn",
    SAMPLE_VARIANT: "sel4-sample",
    STREAM_VARIANT: "sel4-stream",
    SUPERVISION_VARIANT: "sel4-supervision",
    RECLAMATION_VARIANT: "sel4-reclamation",
    CROSSING_VARIANT: "sel4-crossing",
    CALL_VARIANT: "sel4-call",
    QOS_VARIANT: "sel4-qos",
    STRESS_VARIANT: "sel4-stress",
    OPERATION_VARIANT: "sel4-operation",
    VISIBILITY_VARIANT: "sel4-visibility",
    BOOT_VARIANT: "sel4-boot",
    STORAGE_VARIANT: "sel4-storage",
    STORE_VARIANT: "sel4-store",
    ROLLBACK_VARIANT: "sel4-rollback",
    RECOVERY_VARIANT: "sel4-recovery",
    GENERATION_VARIANT: "sel4-generation",
    DIRECTORY_VARIANT: "sel4-directory",
    FILESYSTEM_VARIANT: "sel4-filesystem",
    DANGO_VARIANT: "sel4-dango",
    INPUT_VARIANT: "sel4-input",
    POWERBOX_VARIANT: "sel4-powerbox",
    TRANSFER_VARIANT: "sel4-transfer",
    BOOT_SELECTION_VARIANT: "sel4",
}
VARIANT_TARGET_DIRS = {
    FIXTURE_VARIANT: "root",
    GRAPH_VARIANT: "root-graph",
    CHANNEL_VARIANT: "root-channel",
    LOAN_VARIANT: "root-loan",
    SPAWN_VARIANT: "root-spawn",
    SAMPLE_VARIANT: "root-sample",
    STREAM_VARIANT: "root-stream",
    SUPERVISION_VARIANT: "root-supervision",
    RECLAMATION_VARIANT: "root-reclamation",
    CROSSING_VARIANT: "root-crossing",
    CALL_VARIANT: "root-call",
    QOS_VARIANT: "root-qos",
    STRESS_VARIANT: "root-stress",
    OPERATION_VARIANT: "root-operation",
    VISIBILITY_VARIANT: "root-visibility",
    BOOT_VARIANT: "root-boot",
    STORAGE_VARIANT: "root-storage",
    STORE_VARIANT: "root-store",
    ROLLBACK_VARIANT: "root-rollback",
    RECOVERY_VARIANT: "root-recovery",
    GENERATION_VARIANT: "root-generation",
    DIRECTORY_VARIANT: "root-directory",
    FILESYSTEM_VARIANT: "root-filesystem",
    DANGO_VARIANT: "root-dango",
    INPUT_VARIANT: "root-input",
    POWERBOX_VARIANT: "root-powerbox",
    TRANSFER_VARIANT: "root-transfer",
    BOOT_SELECTION_VARIANT: "root-boot-selection",
}
VARIANT_IMAGES = {
    FIXTURE_VARIANT: (IMAGE, MANIFEST),
    GRAPH_VARIANT: (GRAPH_IMAGE, GRAPH_MANIFEST),
    CHANNEL_VARIANT: (CHANNEL_IMAGE, CHANNEL_MANIFEST),
    LOAN_VARIANT: (LOAN_IMAGE, LOAN_MANIFEST),
    SPAWN_VARIANT: (SPAWN_IMAGE, SPAWN_MANIFEST),
    SAMPLE_VARIANT: (SAMPLE_IMAGE, SAMPLE_MANIFEST),
    STREAM_VARIANT: (STREAM_IMAGE, STREAM_MANIFEST),
    SUPERVISION_VARIANT: (SUPERVISION_IMAGE, SUPERVISION_MANIFEST),
    RECLAMATION_VARIANT: (RECLAMATION_IMAGE, RECLAMATION_MANIFEST),
    CROSSING_VARIANT: (CROSSING_IMAGE, CROSSING_MANIFEST),
    CALL_VARIANT: (CALL_IMAGE, CALL_MANIFEST),
    QOS_VARIANT: (QOS_IMAGE, QOS_MANIFEST),
    STRESS_VARIANT: (STRESS_IMAGE, STRESS_MANIFEST),
    OPERATION_VARIANT: (OPERATION_IMAGE, OPERATION_MANIFEST),
    VISIBILITY_VARIANT: (VISIBILITY_IMAGE, VISIBILITY_MANIFEST),
    BOOT_VARIANT: (BOOT_IMAGE, BOOT_MANIFEST),
    STORAGE_VARIANT: (STORAGE_IMAGE, STORAGE_MANIFEST),
    STORE_VARIANT: (STORE_IMAGE, STORE_MANIFEST),
    ROLLBACK_VARIANT: (ROLLBACK_IMAGE, ROLLBACK_MANIFEST),
    RECOVERY_VARIANT: (RECOVERY_IMAGE, RECOVERY_MANIFEST),
    GENERATION_VARIANT: (GENERATION_IMAGE, GENERATION_MANIFEST),
    DIRECTORY_VARIANT: (DIRECTORY_IMAGE, DIRECTORY_MANIFEST),
    FILESYSTEM_VARIANT: (FILESYSTEM_IMAGE, FILESYSTEM_MANIFEST),
    DANGO_VARIANT: (DANGO_IMAGE, DANGO_MANIFEST),
    INPUT_VARIANT: (INPUT_IMAGE, INPUT_MANIFEST),
    POWERBOX_VARIANT: (POWERBOX_IMAGE, POWERBOX_MANIFEST),
    TRANSFER_VARIANT: (TRANSFER_IMAGE, TRANSFER_MANIFEST),
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
    return {
        "path": str(path.relative_to(ROOT)),
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


def boot_bundle_identity() -> str:
    """Versioned identity of the immutable seL4 kernel and loader."""
    kernel = require_file(SEL4_PREFIX / "bin" / "kernel.elf", "installed seL4 kernel")
    digest = hashlib.sha256()
    digest.update(b"slime-sel4-boot-bundle-v1\0")
    digest.update(bytes.fromhex(sha256_file(kernel)))
    digest.update(bytes.fromhex(directory_digest(RUST_SEL4_SOURCE / "crates" / "sel4-kernel-loader")))
    return digest.hexdigest()


def git_commit(path: Path) -> str:
    commit = run_output(["git", "rev-parse", "HEAD"], cwd=path, description="read submodule pin")
    if re.fullmatch(r"[0-9a-f]{40}", commit) is None:
        fail(f"unexpected commit identity for {path.relative_to(ROOT)}: {commit!r}")
    return commit


def cross_compiler_prefix() -> str:
    """The exact cross toolchain the pinned kernel and loader were built with.

    `CROSS_COMPILER_PREFIX` overrides it for hosts whose GNU AArch64 toolchain
    carries a different triple; the default matches the `nix develop` shell.

    The shell sets this to an **absolute** `.../bin/<triple>-` path rather than
    a bare `<triple>-`, because a bare prefix names whatever `PATH` resolves
    and that is a different derivation per system. On `aarch64-linux`
    nixpkgs' `pkgsCross.aarch64-multiplatform.stdenv.cc` is a *native* wrapper
    that exports no `aarch64-unknown-linux-gnu-gcc`, so the prefixed lookup
    reaches past it to the unwrapped GCC, while Darwin resolves the cross
    wrapper: different flag injection and a different `as`, hence a different
    `kernel.elf`. Absolute pinning is what makes `[observed_prefix]` a function
    of the toolchain rather than of `PATH` order (B21).

    A bare prefix is still accepted: `require_tool` resolves either form, and
    hosts outside the pinned shell may legitimately have only a `PATH` entry.
    """
    prefix = os.environ.get("CROSS_COMPILER_PREFIX") or "aarch64-unknown-linux-gnu-"
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

# Replaces the dev shell's derivation hash as GCC's symbol-naming seed. One
# value for the whole build is safe: the seed only suffixes file-scope static
# and section names, and `kernel.elf` links five objects — `kernel_all.c` plus
# `head.S`, `traps.S`, `idle.S`, and `machine_asm.S` — so there is nothing for a
# shared seed to collide with. Thirteen compile edges share these flags in all;
# the other eight are bitfield/pruning scaffolding and libsel4, never co-linked
# into the pinned artifact.
SEL4_RANDOM_SEED = "slime-sel4-qemu-arm-virt"


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
        if not name.startswith(ENVIRONMENT_FLAG_PREFIXES)
        and name not in ENVIRONMENT_DROPPED_NAMES
    }


def cargo_environment(toolchain: str) -> dict[str, str]:
    environment = dict(os.environ)
    environment["RUSTUP_TOOLCHAIN"] = toolchain
    environment["SEL4_PREFIX"] = str(SEL4_PREFIX)
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
    compiler = f"{cross_compiler_prefix()}gcc"
    environment["CC_aarch64_unknown_none"] = compiler
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


# The exact QEMU machine string the kernel's own platform config would build
# for this profile, plus `dtb-randomness=off`. Without that switch QEMU seeds
# `rng-seed` and `kaslr-seed` into the dump, and the installed device tree —
# and everything derived from it — differs on every configure.
QEMU_DTB_MACHINE = "virt,secure=off,virtualization=on,gic-version=2,dtb-randomness=off"
QEMU_DTB_CPU = "cortex-a53"
QEMU_DTB_MEMORY = "1024"


def dump_device_tree() -> Path:
    """Dump the platform device tree once, deterministically.

    Memory size is the kernel's own `QEMU_MEMORY` default for this platform,
    not the 2048 MiB the product boots with: the kernel derives its physical
    memory window from this description, and the pinned prefix was produced
    with the default. Widening it is a platform change, not a harness knob.
    """
    dtb = SEL4_BUILD / "slime-qemu-arm-virt.dtb"
    dtb.parent.mkdir(parents=True, exist_ok=True)
    run(
        [
            require_tool("qemu-system-aarch64"),
            "-machine",
            f"{QEMU_DTB_MACHINE},dumpdtb={dtb}",
            "-cpu",
            QEMU_DTB_CPU,
            "-smp",
            "1",
            "-m",
            QEMU_DTB_MEMORY,
            "-nographic",
        ],
        description="dump platform device tree",
    )
    return require_file(dtb, "dumped platform device tree")


def configure_and_install_sel4() -> None:
    require_file(SEL4_CONFIG, "qemu-arm-virt seL4 configuration")
    require_tool("cmake")
    require_tool("ninja")
    require_tool("dtc")
    # `tools/xmllint.sh` shells out to `xmllint` to validate the syscall and
    # invocation XML before generating headers; its absence surfaces as a bare
    # exit-127 ninja failure several steps into the build.
    require_tool("xmllint")
    # The kernel's qemu-arm-virt platform config extracts its device tree by
    # invoking QEMU, so the emulator is a build input, not only a boot input.
    require_tool("qemu-system-aarch64")
    require_sel4_python_modules()
    cross_prefix = cross_compiler_prefix()
    SEL4_BUILD.mkdir(parents=True, exist_ok=True)
    SEL4_PREFIX.mkdir(parents=True, exist_ok=True)
    dtb = dump_device_tree()
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
    #  * Frame-pointer policy is stated rather than inherited. The recorded
    #    cause for this (B20) was wrong and is corrected here: it claimed
    #    Darwin's wrapper injects `-fno-omit-frame-pointer` while
    #    `aarch64-linux` "forces neither". Both systems' wrappers ship the
    #    *same* `nix-support/cc-cflags-before`
    #    (`-fno-omit-frame-pointer -mno-omit-leaf-frame-pointer -march=armv8-a`)
    #    — verified by reading both files. The real divergence was which
    #    binary ran at all: with a bare `CROSS_COMPILER_PREFIX`, Darwin
    #    resolved the cross *wrapper* and `aarch64-linux` fell through to the
    #    *unwrapped* GCC, which injects nothing. `CROSS_COMPILER_PREFIX` is now
    #    absolute, so both run the wrapper and the injection is uniform (B21).
    #
    #    These flags remain load-bearing, for a reason B20 did not identify.
    #    With the toolchain pinned but the flags removed, the two hosts still
    #    disagree: every ALLOC section matches byte-for-byte and only
    #    `.debug_line` differs (observed `e8cbab4f…` vs `4c694979…`, both
    #    982208 bytes). The frame-pointer prologue makes GAS emit an extra
    #    line-table row at one address, and GAS's DWARF-5 "view" numbering for
    #    that row is not host-independent. Keeping the frame pointer omitted
    #    keeps that row from existing, so the flags close a real residual leak
    #    rather than merely restating a policy. Do not drop them.
    #
    #    They are also a policy this build *chooses*. GCC's aarch64 backend
    #    disables `-fomit-frame-pointer` at every `-O` level
    #    (`aarch_option_optimization_table`, `OPT_LEVELS_ALL`), so an aarch64
    #    kernel keeps its frame pointers at `-O2` unless the flag is passed
    #    explicitly. `-Q --help=optimizers` reports `-fomit-frame-pointer
    #    [enabled]` at `-O2` regardless, which is a reporting trap rather than
    #    the truth: `aarch64.cc` drives codegen off a tri-state where only an
    #    explicit flag counts. seL4 asks for no frame pointer, nothing in the
    #    tree walks one, and omitting it is worth a register and the prologue.
    #
    #    `-momit-leaf-frame-pointer` is belt and braces: under
    #    `-fomit-frame-pointer` no function gets a frame pointer, leaf or not,
    #    and the two flags together emit assembly identical to the first alone.
    #    It is kept because it names the second of the wrapper's two
    #    injections.
    common_flags = (
        f"-ffile-prefix-map={SEL4_SOURCE}=/slime/sel4 "
        f"-ffile-prefix-map={SEL4_BUILD}=/slime/build "
        f"-frandom-seed={SEL4_RANDOM_SEED} "
        "-fomit-frame-pointer -momit-leaf-frame-pointer"
    )
    environment = sel4_build_environment()
    run(
        [
            "cmake",
            "-S",
            str(SEL4_SOURCE),
            "-B",
            str(SEL4_BUILD),
            "-G",
            "Ninja",
            "-C",
            str(SEL4_CONFIG),
            f"-DCMAKE_INSTALL_PREFIX={SEL4_PREFIX}",
            f"-DCROSS_COMPILER_PREFIX={cross_prefix}",
            f"-DQEMU_DTB={dtb}",
            f"-DCMAKE_C_FLAGS={common_flags}",
            f"-DCMAKE_ASM_FLAGS={common_flags}",
            # Empty rather than absent: an explicit cache entry is what stops a
            # tree configured under a shell that exported `LDFLAGS` from
            # retaining it. seL4 appends its own linker flags itself.
            "-DCMAKE_EXE_LINKER_FLAGS=",
        ],
        environment=environment,
        description="configure seL4",
    )
    run(
        ["cmake", "--build", str(SEL4_BUILD), "--parallel"],
        environment=environment,
        description="build seL4",
    )
    run(
        ["cmake", "--install", str(SEL4_BUILD)],
        environment=environment,
        description="install seL4",
    )


def build_sel4_generation(manifest: str = "sel4", *, output_name: str | None = None) -> Path:
    """Build one aarch64-sel4 generation and return its generation bytes."""
    name = output_name or ("sel4-generation" if manifest == "sel4" else f"{manifest}-generation")
    output = BUILD_ROOT / name
    output.mkdir(parents=True, exist_ok=True)
    environment = dict(os.environ)
    environment["SLIME_TARGET_PROFILE"] = "aarch64-sel4-qemu-virt"
    environment["SLIME_SEL4_MANIFEST"] = manifest
    run(
        [sys.executable, str(ROOT / "scripts" / "build" / "build-generation.py"), str(output)],
        environment=environment,
        description="build seL4 generation",
    )
    return require_file(output / "generation.bin", "seL4 generation")

def build_application(
    pins: dict[str, object], *, variant: str = FIXTURE_VARIANT
) -> tuple[Path, Path]:
    rust_sel4 = table(pins, "rust_sel4")
    toolchain = text(rust_sel4, "toolchain", "rust_sel4")
    environment = cargo_environment(toolchain)
    root_target = ROOT / text(rust_sel4, "root_target", "rust_sel4")
    child_target = RUST_SEL4_SOURCE / "support" / "targets" / "aarch64-sel4-minimal.json"
    require_file(root_target, "root target specification")
    require_file(child_target, "child target specification")

    child_target_dir = CARGO_BUILD / "child"
    cargo_build(
        manifest=CHILD_MANIFEST,
        package="slime-root-child",
        target=child_target,
        target_dir=child_target_dir,
        environment=environment,
        description="build root child",
        features=child_features(),
    )
    child_elf = child_target_dir / "aarch64-sel4-minimal" / "release" / "slime-root-child.elf"
    require_file(child_elf, "root child ELF")

    root_environment = environment.copy()
    root_environment["CHILD_ELF"] = str(child_elf.resolve())
    if variant == BOOT_SELECTION_VARIANT:
        bundle_identity = boot_bundle_identity()
        root_environment["SLIME_BOOT_SELECTOR"] = "1"
        root_environment["SLIME_BOOT_BUNDLE_IDENTITY"] = bundle_identity
    else:
        manifest = VARIANT_MANIFESTS.get(variant, "sel4")
        root_environment["SLIME_GENERATION"] = str(build_sel4_generation(manifest).resolve())
        if variant == FIXTURE_VARIANT:
            root_environment["SLIME_ROOT_FIXTURE"] = "1"
    if variant == RECLAMATION_VARIANT:
        rustflags = root_environment.get("RUSTFLAGS", "")
        root_environment["RUSTFLAGS"] = f"{rustflags} --cfg slime_b38_force_unwind".strip()
    # B40 negative mutations: perturb one child CSpace in exactly one way so
    # the capability-layout audit's refusal is observed rather than assumed.
    # Never set for a product variant.
    mutation = os.environ.get("SLIME_B40_MUTATION")
    if mutation:
        if mutation not in B40_MUTATIONS:
            fail(f"unknown B40 mutation {mutation!r}")
        rustflags = root_environment.get("RUSTFLAGS", "")
        root_environment["RUSTFLAGS"] = (
            f"{rustflags} --cfg slime_b40_mutate_{mutation}".strip()
        )
    # Separate target directories: the images embed different generations, so
    # sharing one would make each build invalidate the others' artifacts and
    # whichever gate ran last would boot a rebuilt image.
    root_target_dir = CARGO_BUILD / VARIANT_TARGET_DIRS[variant]
    cargo_build(
        manifest=ROOT / "Cargo.toml",
        package="slime-root",
        target=root_target,
        target_dir=root_target_dir,
        environment=root_environment,
        description="build root task",
    )
    root_elf = (
        root_target_dir
        / "aarch64-sel4-roottask-minimal"
        / "release"
        / "slime-root.elf"
    )
    require_file(root_elf, "root task ELF")
    return child_elf, root_elf


def build_loader(pins: dict[str, object]) -> tuple[Path, Path]:
    """Build the kernel loader and its host packaging tool.

    Both run with `deps/rust-sel4` as the working directory: that workspace's
    `.cargo/config.toml` supplies the `rust-lld` linker selection and the
    `RUST_TARGET_PATH` its crates expect, and cargo discovers configuration
    from the working directory rather than from `--manifest-path`.
    """
    rust_sel4 = table(pins, "rust_sel4")
    toolchain = text(rust_sel4, "toolchain", "rust_sel4")
    loader_target = text(rust_sel4, "loader_target", "rust_sel4")
    environment = cargo_environment(toolchain)

    loader_target_dir = CARGO_BUILD / "loader"
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


def package_image(payload_tool: Path, loader: Path, root_elf: Path, image: Path) -> None:
    run(
        [
            str(payload_tool),
            "--sel4-prefix",
            str(SEL4_PREFIX),
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


def copy_artifact(source: Path, name: str) -> Path:
    ARTIFACTS.mkdir(parents=True, exist_ok=True)
    destination = ARTIFACTS / name
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
) -> None:
    kernel = require_file(SEL4_PREFIX / "bin" / "kernel.elf", "installed seL4 kernel")
    kernel_config = require_file(
        SEL4_PREFIX / "libsel4" / "include" / "kernel" / "gen_config.json",
        "installed seL4 kernel config",
    )
    libsel4_config = require_file(
        SEL4_PREFIX / "libsel4" / "include" / "sel4" / "gen_config.json",
        "installed libsel4 config",
    )
    dtb = require_file(SEL4_PREFIX / "support" / "kernel.dtb", "installed seL4 DTB")
    platform_info = require_file(
        SEL4_PREFIX / "support" / "platform_gen.yaml", "installed platform metadata"
    )
    root_target = ROOT / text(table(pins, "rust_sel4"), "root_target", "rust_sel4")
    child_target = RUST_SEL4_SOURCE / "support" / "targets" / "aarch64-sel4-minimal.json"

    suffix = "" if variant == FIXTURE_VARIANT else f"-{variant}"
    stable_child = copy_artifact(child_elf, f"slime-root-child{suffix}.elf")
    stable_root = copy_artifact(root_elf, f"slime-root{suffix}.elf")
    stable_loader = copy_artifact(loader, "sel4-kernel-loader")
    stable_payload_tool = copy_artifact(payload_tool, "sel4-kernel-loader-add-payload")

    qemu = table(pins, "qemu_arm_virt")
    manifest = {
        "schema": 1,
        "kind": "slime-sel4-image-identity",
        "source": {
            "sel4": {
                "commit": git_commit(SEL4_SOURCE),
                "release": require_file(
                    SEL4_SOURCE / "VERSION", "seL4 VERSION"
                ).read_text(encoding="utf-8").strip(),
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
            "cmake": file_record(SEL4_CONFIG),
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
        # Which startup path this image takes, so a gate cannot boot a different
        # one and assert against markers it will never emit.
        #
        # `component_graph` is retained beside `variant` rather than replaced by
        # it: P5.1's and P5.2's gates assert on that field, and a third image is
        # no reason to edit verification code those slices' evidence rests on. A
        # bool cannot name three images, so `variant` is what a new gate reads.
        "component_graph": variant == GRAPH_VARIANT,
        "variant": variant,
        "qemu": {
            "machine": text(qemu, "machine", "qemu_arm_virt"),
            "cpu": text(qemu, "cpu", "qemu_arm_virt"),
            "cpus": qemu["cpus"],
            "memory_mib": qemu["memory_mib"],
            "version": text(qemu, "qemu_version", "qemu_arm_virt"),
        },
    }
    if variant == BOOT_SELECTION_VARIANT:
        manifest["boot_bundle_identity"] = boot_bundle_identity()
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
        "--channel-plane",
        action="store_true",
        help=(
            "embed the channel-plane generation (P5.3.1), writing a separate image"
        ),
    )
    parser.add_argument(
        "--loan-plane",
        action="store_true",
        help="embed the loan-plane generation (P5.3.2), writing a separate image",
    )
    parser.add_argument(
        "--spawn-plane",
        action="store_true",
        help="embed the spawn-plane generation (P5.3.3), writing a separate image",
    )
    parser.add_argument(
        "--sample-plane",
        action="store_true",
        help="embed the sample-plane generation (P5.3.4), writing a separate image",
    )
    parser.add_argument(
        "--stream-plane",
        action="store_true",
        help="embed the stream-plane generation (P5.5.2), writing a separate image",
    )
    parser.add_argument(
        "--supervision-plane",
        action="store_true",
        help=(
            "embed the supervision-plane generation (B16), writing a separate image"
        ),
    )
    parser.add_argument(
        "--reclamation-plane",
        action="store_true",
        help="embed the B38 task-reclamation generation, writing a separate image",
    )
    parser.add_argument(
        "--crossing-plane",
        action="store_true",
        help=(
            "embed the channel-crossing generation (B22), writing a separate image"
        ),
    )
    parser.add_argument(
        "--call-plane",
        action="store_true",
        help="embed the bounded-call generation (C8.6), writing a separate image",
    )
    parser.add_argument(
        "--qos-plane",
        action="store_true",
        help=(
            "embed the timed-QoS generation (C8.5): the stream graph plus a "
            "monotonic-time channel, writing a separate image"
        ),
    )
    parser.add_argument(
        "--stress-plane",
        action="store_true",
        help=(
            "embed the 48-instance generation (B49): the admitted ceiling, so "
            "the largest graph admission accepts actually boots"
        ),
    )
    parser.add_argument(
        "--operation-plane",
        action="store_true",
        help="embed the native-operation generation (C8.7), writing a separate image",
    )
    parser.add_argument(
        "--visibility-plane",
        action="store_true",
        help=(
            "embed the filtered-introspection and declared-interposition "
            "generation (C8.8), writing a separate image"
        ),
    )
    parser.add_argument(
        "--boot-plane",
        action="store_true",
        help=(
            "embed the full-graph bootstrap generation (C8.10): every C8 role "
            "in one collision-free layout, writing a separate image"
        ),
    )
    parser.add_argument(
        "--storage-plane",
        action="store_true",
        help=(
            "embed the M5 storage generation (P5.4.2c): a probe holding a block "
            "capability, writing a separate image"
        ),
    )
    parser.add_argument(
        "--transfer-plane",
        action="store_true",
        help=(
            "embed the M6.7 transfer generation (P5.4.3): a probe holding a "
            "read-only source device and a writable receiver, writing a "
            "separate image"
        ),
    )
    parser.add_argument(
        "--powerbox-plane",
        action="store_true",
        help=(
            "embed the M6.6 powerbox generation (P5.4.3): a chooser holding "
            "directory authority the requester lacks, handing over one narrowed "
            "view on selection, writing a separate image"
        ),
    )
    parser.add_argument(
        "--input-plane",
        action="store_true",
        help=(
            "embed the input generation (P5.4.3): a probe reading the scripted "
            "key source through a granted capability, writing a separate image"
        ),
    )
    parser.add_argument(
        "--dango-plane",
        action="store_true",
        help=(
            "embed the M6.4 dango generation (P5.4.3): a scripted console "
            "session launching commands through the spawn service, writing a "
            "separate image"
        ),
    )
    parser.add_argument(
        "--filesystem-plane",
        action="store_true",
        help=(
            "embed the M6.3 filesystem generation (P5.4.3): a service resolving "
            "names in a snapshot tree over the object store, and a client that "
            "must ask it, writing a separate image"
        ),
    )
    parser.add_argument(
        "--directory-plane",
        action="store_true",
        help=(
            "embed the M6.3 directory generation (P5.4.3): a probe exercising "
            "scoped views, narrow-only derivation, and the atomic namespace "
            "commit, writing a separate image"
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
        "--recovery-plane",
        action="store_true",
        help=(
            "embed the M5.9 recovery generation (P5.4.2c): a probe "
            "reconstructing a verified root from a signed index while an "
            "ungranted disk stays untouched, writing a separate image"
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
        "--store-plane",
        action="store_true",
        help=(
            "embed the M5.4 object-store generation (P5.4.2c): a probe running "
            "GPT validation and the object store in userspace over a block "
            "capability, writing a separate image"
        ),
    )
    arguments = parser.parse_args()
    selected = [
        variant
        for variant, chosen in (
            (GRAPH_VARIANT, arguments.component_graph),
            (CHANNEL_VARIANT, arguments.channel_plane),
            (LOAN_VARIANT, arguments.loan_plane),
            (SPAWN_VARIANT, arguments.spawn_plane),
            (SAMPLE_VARIANT, arguments.sample_plane),
            (STREAM_VARIANT, arguments.stream_plane),
            (SUPERVISION_VARIANT, arguments.supervision_plane),
            (RECLAMATION_VARIANT, arguments.reclamation_plane),
            (CROSSING_VARIANT, arguments.crossing_plane),
            (CALL_VARIANT, arguments.call_plane),
            (QOS_VARIANT, arguments.qos_plane),
            (STRESS_VARIANT, arguments.stress_plane),
            (OPERATION_VARIANT, arguments.operation_plane),
            (VISIBILITY_VARIANT, arguments.visibility_plane),
            (BOOT_VARIANT, arguments.boot_plane),
            (STORAGE_VARIANT, arguments.storage_plane),
            (STORE_VARIANT, arguments.store_plane),
            (ROLLBACK_VARIANT, arguments.rollback_plane),
            (RECOVERY_VARIANT, arguments.recovery_plane),
            (GENERATION_VARIANT, arguments.generation_plane),
            (DIRECTORY_VARIANT, arguments.directory_plane),
            (FILESYSTEM_VARIANT, arguments.filesystem_plane),
            (DANGO_VARIANT, arguments.dango_plane),
            (INPUT_VARIANT, arguments.input_plane),
            (POWERBOX_VARIANT, arguments.powerbox_plane),
            (TRANSFER_VARIANT, arguments.transfer_plane),
            (BOOT_SELECTION_VARIANT, arguments.boot_selection),
        )
        if chosen
    ]
    if len(selected) > 1:
        fail("each --*-plane flag selects a different generation; pass one")
    variant = selected[0] if selected else FIXTURE_VARIANT

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    pins = load_pins()
    if not arguments.skip_pin_check:
        run(
            [sys.executable, str(ROOT / "scripts" / "check" / "check-sel4-pins.py")],
            description="verify seL4 pins",
        )
    BUILD_ROOT.mkdir(parents=True, exist_ok=True)
    configure_and_install_sel4()
    run(
        [
            sys.executable,
            str(ROOT / "scripts" / "check" / "check-sel4-pins.py"),
            "--prefix",
        ],
        description="verify installed seL4 prefix",
    )
    child_elf, root_elf = build_application(pins, variant=variant)
    loader, payload_tool = build_loader(pins)
    image, manifest_path = VARIANT_IMAGES[variant]
    package_image(payload_tool, loader, root_elf, image)
    write_manifest(
        pins,
        child_elf=child_elf,
        root_elf=root_elf,
        loader=loader,
        payload_tool=payload_tool,
        image=image,
        manifest_path=manifest_path,
        variant=variant,
    )
    print(
        f"seL4 image build: wrote {image.relative_to(ROOT)} and "
        f"{manifest_path.relative_to(ROOT)}"
    )


if __name__ == "__main__":
    main()
