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

# Which generation the root task embeds. That is the only difference between the
# images this script builds; see `build_application`.
FIXTURE_VARIANT = "fixture"
GRAPH_VARIANT = "graph"
CHANNEL_VARIANT = "channel"
LOAN_VARIANT = "loan"
SPAWN_VARIANT = "spawn"
VARIANT_MANIFESTS = {
    GRAPH_VARIANT: "sel4",
    CHANNEL_VARIANT: "sel4-channel",
    LOAN_VARIANT: "sel4-loan",
    SPAWN_VARIANT: "sel4-spawn",
}
VARIANT_TARGET_DIRS = {
    FIXTURE_VARIANT: "root",
    GRAPH_VARIANT: "root-graph",
    CHANNEL_VARIANT: "root-channel",
    LOAN_VARIANT: "root-loan",
    SPAWN_VARIANT: "root-spawn",
}
VARIANT_IMAGES = {
    FIXTURE_VARIANT: (IMAGE, MANIFEST),
    GRAPH_VARIANT: (GRAPH_IMAGE, GRAPH_MANIFEST),
    CHANNEL_VARIANT: (CHANNEL_IMAGE, CHANNEL_MANIFEST),
    LOAN_VARIANT: (LOAN_IMAGE, LOAN_MANIFEST),
    SPAWN_VARIANT: (SPAWN_IMAGE, SPAWN_MANIFEST),
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


def git_commit(path: Path) -> str:
    commit = run_output(["git", "rev-parse", "HEAD"], cwd=path, description="read submodule pin")
    if re.fullmatch(r"[0-9a-f]{40}", commit) is None:
        fail(f"unexpected commit identity for {path.relative_to(ROOT)}: {commit!r}")
    return commit


def cross_compiler_prefix() -> str:
    """The exact cross toolchain the pinned kernel and loader were built with.

    `CROSS_COMPILER_PREFIX` overrides it for hosts whose GNU AArch64 toolchain
    carries a different triple; the default matches the `nix develop` shell.
    """
    prefix = os.environ.get("CROSS_COMPILER_PREFIX") or "aarch64-unknown-linux-gnu-"
    require_tool(f"{prefix}gcc")
    return prefix


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
    # Two reproducibility leaks in the upstream build, both closed here so the
    # observed prefix hashes in `sel4/pins.toml` mean something:
    #  * QEMU's `virt` machine seeds `rng-seed`/`kaslr-seed` into every dumped
    #    device tree, so letting the kernel extract its own DTB produces a
    #    different platform description on every configure. `dump_device_tree`
    #    dumps it once with `dtb-randomness=off` and passes it in.
    #  * The assembler records absolute source and build paths in the kernel's
    #    `.debug_line`, so the ELF depends on where the checkout lives. Both
    #    roots are mapped to fixed logical prefixes.
    prefix_map = (
        f"-ffile-prefix-map={SEL4_SOURCE}=/slime/sel4 "
        f"-ffile-prefix-map={SEL4_BUILD}=/slime/build"
    )
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
            f"-DCMAKE_C_FLAGS={prefix_map}",
            f"-DCMAKE_ASM_FLAGS={prefix_map}",
        ],
        description="configure seL4",
    )
    run(
        ["cmake", "--build", str(SEL4_BUILD), "--parallel"],
        description="build seL4",
    )
    run(
        ["cmake", "--install", str(SEL4_BUILD)],
        description="install seL4",
    )


def build_sel4_generation(manifest: str = "sel4") -> Path:
    """Build an `aarch64-sel4-qemu-virt` generation the root task launches.

    The root task embeds its generation at compile time, so this must run before
    the root task is built. Producing it here rather than committing a fixture
    keeps the graph the root boots equal to the manifest by construction — a
    component added to `sel4.zti` is in the next boot without a blob being
    regenerated by hand.

    `manifest` names which seL4 graph: `sel4` is P5.2's five-component service
    graph, `sel4-channel` is P5.3.1's channel plane. Each gets its own output
    directory, because they are different generations for the same target and
    sharing one would make each build overwrite the other's artifact. P5.2's
    keeps its existing path so the artifact its gate already asserts about does
    not move.
    """
    output = BUILD_ROOT / ("sel4-generation" if manifest == "sel4" else f"{manifest}-generation")
    output.mkdir(parents=True, exist_ok=True)
    environment = dict(os.environ)
    environment["SLIME_TARGET_PROFILE"] = "aarch64-sel4-qemu-virt"
    environment["SLIME_SEL4_MANIFEST"] = manifest
    run(
        [
            sys.executable,
            str(ROOT / "scripts" / "build" / "build-generation.py"),
            # The seL4 path builds no Slime kernel image: seL4 is the kernel,
            # and the argument is positional only.
            os.devnull,
            str(output),
        ],
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
    # Which generation the root task embeds is what distinguishes the images
    # this script produces, and it is the only thing that does: the root chooses
    # its startup path by what the generation carries — loadable payloads or not
    # — rather than by a flag it was built with. `fixture` embeds the retained
    # x86 generation and runs P5.1's native fixture path; `graph` embeds P5.2's
    # five-component service graph; `channel` embeds P5.3.1's channel plane.
    if variant != FIXTURE_VARIANT:
        manifest = VARIANT_MANIFESTS[variant]
        root_environment["SLIME_GENERATION"] = str(
            build_sel4_generation(manifest).resolve()
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
    arguments = parser.parse_args()
    selected = [
        variant
        for variant, chosen in (
            (GRAPH_VARIANT, arguments.component_graph),
            (CHANNEL_VARIANT, arguments.channel_plane),
            (LOAN_VARIANT, arguments.loan_plane),
            (SPAWN_VARIANT, arguments.spawn_plane),
        )
        if chosen
    ]
    if len(selected) > 1:
        fail("--component-graph, --channel-plane, --loan-plane, and --spawn-plane select different generations")
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
