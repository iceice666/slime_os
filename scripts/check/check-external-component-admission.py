#!/usr/bin/env python3
"""CP4: external ELF admission, rejection, signing, and mixed-source generation."""

from __future__ import annotations

import copy
import hashlib
import os
import shutil
import subprocess
import struct
import sys
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts" / "lib"))

from component_paths import crate_path  # noqa: E402
from component_spec import admit_specs  # noqa: E402
from harness import load_script  # noqa: E402
SEL4_BUILDER = ROOT / "scripts" / "build" / "build-sel4.py"
GRAPH_CHECK = ROOT / "scripts" / "check" / "check-sel4-component-graph.py"

BUILDER = ROOT / "scripts" / "build" / "build-generation.py"
CHECK = load_script("external_component_generation_check", "check/check-generation.py")
EXTERNAL_CRATE_FILES = (
    "Cargo.toml",
    "build.rs",
    "src/main.rs",
)


def fail(message: str) -> None:
    raise SystemExit(f"external component admission check: {message}")


def zti(value: object, indent: int = 0) -> str:
    import json

    padding = " " * indent
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=True)
    if isinstance(value, list):
        if not value:
            return "[]"
        rows = "".join(f"{padding}  {zti(item, indent + 2)};\n" for item in value)
        return "[\n" + rows + padding + "]"
    if isinstance(value, dict):
        rows = "".join(
            f"{padding}  {key} = {zti(item, indent + 2)};\n"
            for key, item in value.items()
        )
        return "{\n" + rows + padding + "}"
    raise TypeError(type(value))


def cargo_target_directory_name(target: Path) -> str:
    return target.stem


def isolated_console_source(source: Path) -> None:
    source.mkdir()
    crate = crate_path("console")
    for relative in EXTERNAL_CRATE_FILES:
        destination = source / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(crate / relative, destination)
    manifest = tomllib.loads((source / "Cargo.toml").read_text(encoding="utf-8"))
    manifest["package"]["name"] = "cp4-external-console"
    manifest.pop("lints", None)
    dependency_paths = {
        "boot-contracts": ROOT / "boot-contracts",
        "slime-components": ROOT / "components" / "lib",
        "slime-rt": ROOT / "components" / "runtime",
    }
    for name, path in dependency_paths.items():
        manifest["dependencies"][name]["path"] = str(path)
    manifest["build-dependencies"]["slime-build-support"]["path"] = str(
        ROOT / "components" / "build-support"
    )
    (source / "Cargo.toml").write_text(external_manifest(manifest), encoding="utf-8")


def external_manifest(value: dict) -> str:
    package = value["package"]
    binary = value["bin"][0]
    rows = [
        "[package]",
        f'name = "{package["name"]}"',
        f'version = "{package["version"]}"',
        f'edition = "{package["edition"]}"',
        "publish = false",
        f'rust-version = "{package["rust-version"]}"',
        f'build = "{package["build"]}"',
        "",
        "[[bin]]",
        f'name = "{binary["name"]}"',
        f'path = "{binary["path"]}"',
        "test = false",
        "",
        "[dependencies]",
    ]
    rows.extend(
        f'{name} = {{ path = "{dependency["path"]}" }}'
        for name, dependency in value["dependencies"].items()
    )
    rows += ["", "[build-dependencies]"]
    rows.extend(
        f'{name} = {{ path = "{dependency["path"]}" }}'
        for name, dependency in value["build-dependencies"].items()
    )
    rows += [
        "",
        "[profile.release]",
        'opt-level = "s"',
        "codegen-units = 1",
        "debug = false",
        'panic = "abort"',
        "",
    ]
    return "\n".join(rows)


def build_isolated_console(root: Path) -> Path:
    source = root / "external-source"
    isolated_console_source(source)
    pins = tomllib.loads((ROOT / "sel4" / "pins.toml").read_text(encoding="utf-8"))
    target = ROOT / "deps" / "rust-sel4" / "support" / "targets" / "aarch64-sel4-minimal.json"
    target_dir = root / "external-target"
    environment = os.environ.copy()
    environment["RUSTUP_TOOLCHAIN"] = pins["rust_sel4"]["toolchain"]
    environment["SEL4_PREFIX"] = str(ROOT / "build" / "sel4-prefix")
    environment["SLIME_TARGET_PROFILE"] = "aarch64-sel4-qemu-virt"
    environment["CARGO_TARGET_DIR"] = str(target_dir)
    environment["RUSTFLAGS"] = " ".join(
        (
            "-C link-arg=--build-id=none",
            f"--remap-path-prefix={source}=./external-component",
            f"--remap-path-prefix={ROOT}=./slime-sdk",
        )
    )
    built = subprocess.run(
        [
            "cargo",
            "build",
            "--release",
            "--target",
            str(target),
            "-Z",
            "json-target-spec",
            "-Z",
            "build-std=core,alloc,compiler_builtins",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ],
        cwd=source,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if built.returncode != 0:
        fail(f"isolated external console build failed:\n{built.stdout}")
    elf = target_dir / cargo_target_directory_name(target) / "release" / "console.elf"
    if not elf.is_file():
        fail("isolated external console build produced no console ELF")
    return elf


def external_specs(root: Path, digest: str) -> None:
    for entry in admit_specs():
        spec = copy.deepcopy(entry.spec)
        if entry.name == "console":
            spec["implementation"] = {
                "provider": "external",
                "binary": "console-external",
                "contentHash": digest,
            }
        (root / f"{entry.name}.zti").write_text(zti(spec) + "\n", encoding="utf-8")


def build(output: Path, specs: Path, elf: Path) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment["SLIME_TARGET_PROFILE"] = "aarch64-sel4-qemu-virt"
    environment["SLIME_SEL4_MANIFEST"] = "sel4"
    return subprocess.run(
        [
            sys.executable,
            str(BUILDER),
            "--component-spec-root",
            str(specs),
            "--external-component",
            f"console-external={elf}",
            str(output),
        ],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )


def malformed_external(
    root: Path, baseline: bytes, label: str, mutate
) -> tuple[Path, Path]:
    data = bytearray(baseline)
    mutate(data)
    elf = root / f"{label}.elf"
    elf.write_bytes(data)
    specs = root / f"{label}-specs"
    specs.mkdir()
    external_specs(specs, hashlib.sha256(data).hexdigest())
    return specs, elf


def refused_before_signing(root: Path, label: str, specs: Path, elf: Path, marker: str) -> None:
    output = root / f"{label}-build"
    refused = build(output, specs, elf)
    if refused.returncode == 0 or marker not in refused.stdout:
        fail(f"{label} external ELF was not refused before generation signing:\n{refused.stdout}")
    if (output / "generation.bin").exists() or (output / "boot-store.bin").exists():
        fail(f"{label} refusal left a signed generation artifact")


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="slime-external-component-") as temporary:
        root = Path(temporary)
        external_elf = build_isolated_console(root)
        elf = external_elf.read_bytes()
        digest = hashlib.sha256(elf).hexdigest()
        workspace_console = (
            ROOT
            / "target"
            / "components"
            / "aarch64-sel4-qemu-virt"
            / "generation-1"
            / "aarch64-sel4-minimal"
            / "release"
            / "console.elf"
        )
        if workspace_console.is_file() and workspace_console.read_bytes() == elf:
            fail("isolated external console ELF is byte-identical to the workspace artifact")
        specs = root / "specs"
        specs.mkdir()
        external_specs(specs, digest)

        mixed = build(root / "mixed", specs, external_elf)
        if mixed.returncode != 0:
            fail(f"mixed-source build failed:\n{mixed.stdout}")
        marker = "implementation=console-external provider=external"
        if marker not in mixed.stdout:
            fail("builder output did not identify console as externally sourced")
        generation = (root / "mixed" / "generation.bin").read_bytes()
        bootstore = (root / "mixed" / "boot-store.bin").read_bytes()
        checked_generation = CHECK.check_generation(generation)
        checked_store = CHECK.check_bootstore(bootstore)
        if checked_store["selected"]["identity"] != checked_generation["identity"]:
            fail("signed boot store did not select the mixed-source generation")

        selector_with_prebuilt = subprocess.run(
            [
                sys.executable,
                str(SEL4_BUILDER),
                "--skip-pin-check",
                "--boot-selection",
                "--prebuilt-generation",
                str(root / "mixed" / "generation.bin"),
            ],
            cwd=ROOT,
            env=os.environ.copy(),
            check=False,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        if (
            selector_with_prebuilt.returncode == 0
            or "cannot be combined with --boot-selection" not in selector_with_prebuilt.stdout
        ):
            fail("boot-selection accepted a prebuilt generation it would not embed")

        boot_build = subprocess.run(
            [
                sys.executable,
                str(SEL4_BUILDER),
                "--component-graph",
                "--prebuilt-generation",
                str(root / "mixed" / "generation.bin"),
            ],
            cwd=ROOT,
            env=os.environ.copy(),
            check=False,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        if boot_build.returncode != 0:
            fail(f"mixed-source seL4 image build failed:\n{boot_build.stdout}")
        manifest = __import__("json").loads(
            (ROOT / "build" / "slime-sel4-graph.identity.json").read_text(encoding="utf-8")
        )
        embedded = manifest.get("generation")
        if not isinstance(embedded, dict) or embedded.get("identity") != checked_generation[
            "identity"
        ].hex():
            fail("seL4 image did not record the release-verified mixed generation identity")
        boot = subprocess.run(
            [sys.executable, str(GRAPH_CHECK), "--no-build"],
            cwd=ROOT,
            env=os.environ.copy(),
            check=False,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        if boot.returncode != 0:
            fail(f"mixed-source generation did not pass the component graph gate:\n{boot.stdout}")

        wrong_hash = root / "wrong-hash.elf"
        wrong_hash.write_bytes(elf + b"\0")
        refused_hash = build(root / "wrong-hash", specs, wrong_hash)
        if refused_hash.returncode == 0 or "does not match declared" not in refused_hash.stdout:
            fail("external bytes whose content hash disagreed were not refused")

        def bad_program_header(data: bytearray) -> None:
            struct.pack_into("<Q", data, 32, len(data) - 8)

        def bad_entry(data: bytearray) -> None:
            struct.pack_into("<Q", data, 24, 0)

        def writable_executable(data: bytearray) -> None:
            phoff = struct.unpack_from("<Q", data, 32)[0]
            phentsize, phnum = struct.unpack_from("<HH", data, 54)
            for index in range(phnum):
                offset = phoff + index * phentsize
                if struct.unpack_from("<I", data, offset)[0] == 1:
                    struct.pack_into("<I", data, offset + 4, 7)
                    return
            fail("isolated external console ELF has no loadable segment to mutate")

        def oversized_mapped_segment(data: bytearray) -> None:
            phoff = struct.unpack_from("<Q", data, 32)[0]
            phentsize, phnum = struct.unpack_from("<HH", data, 54)
            for index in range(phnum):
                offset = phoff + index * phentsize
                if struct.unpack_from("<I", data, offset)[0] == 1:
                    struct.pack_into("<Q", data, offset + 40, 1 << 40)
                    return
            fail("isolated external console ELF has no loadable segment to enlarge")

        def oversized_program_header(data: bytearray) -> None:
            struct.pack_into("<H", data, 54, 64)

        for label, mutate, marker in (
            ("bad-program-header", bad_program_header, "truncated program header"),
            ("bad-entry", bad_entry, "entry point is not executable"),
            ("writable-executable", writable_executable, "writable executable page"),
            (
                "oversized-mapped-segment",
                oversized_mapped_segment,
                "mapped component image exceeds the component image bound",
            ),
            (
                "oversized-program-header",
                oversized_program_header,
                "invalid program header table",
            ),
        ):
            malformed_specs, malformed_elf = malformed_external(root, elf, label, mutate)
            refused_before_signing(root, label, malformed_specs, malformed_elf, marker)

    print(
        "external component admission check: one independently built external ELF "
        "was mixed with workspace components, signed, admitted, booted through the "
        "component-graph gate, and reported by source; hash-mismatched and structurally "
        "invalid external ELFs were refused before signing"
    )

if __name__ == "__main__":
    main()
