#!/usr/bin/env python3

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from zutai_cli import STDLIB, binary

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "scripts" / "boot_contracts.py"
RUST_OUTPUT_DIR = ROOT / "boot-contracts" / "src" / "generated"
GENERATORS = (
    (ROOT / "contracts" / "generation" / "v2" / "schema.zt", "generation.py", "generation.rs"),
    (ROOT / "contracts" / "kernel-image" / "v1" / "schema.zt", "kernel_image.py", "kernel_image.rs"),
    (ROOT / "contracts" / "bootstate" / "v1" / "schema.zt", "bootstate.py", "bootstate.rs"),
    (
        ROOT / "contracts" / "bootstate" / "trace" / "v1" / "schema.zt",
        "bootstate_trace.py",
        "bootstate_trace.rs",
    ),
    (ROOT / "contracts" / "release" / "v1" / "schema.zt", "release.py", "release.rs"),
    (ROOT / "contracts" / "recovery" / "v1" / "schema.zt", "recovery.py", "recovery.rs"),
    (ROOT / "contracts" / "transfer" / "v1" / "schema.zt", "transfer.py", "transfer.rs"),
    (ROOT / "contracts" / "store" / "disk" / "v1" / "schema.zt", "store_disk.py", "store_disk.rs"),
    (ROOT / "contracts" / "handoff" / "v1" / "schema.zt", "handoff.py", "handoff.rs"),
)
INVALID_SCHEMA = "INVALID_"
HEADER = """# @generated from boot contract schemas; do not edit.
from __future__ import annotations

import hashlib
import struct

"""
HELPERS = """def sha256(data: bytes) -> bytes:
    return hashlib.sha256(data).digest()


def generation_identity(data: bytes) -> bytes:
    return sha256(
        data[:GENERATION_HEADER_IDENTITY_OFFSET]
        + bytes(GENERATION_HEADER_IDENTITY_END - GENERATION_HEADER_IDENTITY_OFFSET)
        + data[GENERATION_HEADER_IDENTITY_END:]
    )


def bootstate_checksum(slot: bytes) -> bytes:
    return sha256(
        slot[:BOOTSTATE_CHECKSUM_OFFSET]
        + bytes(BOOTSTATE_CHECKSUM_END - BOOTSTATE_CHECKSUM_OFFSET)
        + slot[BOOTSTATE_CHECKSUM_END:]
    )


def bootstore_checksum(data: bytes) -> bytes:
    offset = BOOTSTORE_DIRECTORY_OFFSET + BOOTSTORE_HEADER_CHECKSUM_OFFSET
    end = BOOTSTORE_DIRECTORY_OFFSET + BOOTSTORE_HEADER_CHECKSUM_END
    return sha256(data[BOOTSTATE_SLOT_BYTES * BOOTSTATE_SLOT_COUNT : offset] + bytes(end - offset) + data[end:])
"""


def run_generator(generator: Path, staging: Path) -> None:
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    environment["SLIME_BOOT_BINDINGS_ROOT"] = str(staging)
    process = subprocess.run(
        [str(binary()), "run", str(generator)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        sys.stderr.write(process.stdout)
        sys.stderr.write(process.stderr)
        raise SystemExit(process.returncode)


def format_rust(source: str) -> str:
    process = subprocess.run(
        ["rustfmt", "--edition", "2024", "--emit", "stdout"],
        cwd=ROOT,
        input=source,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        sys.stderr.write(process.stderr)
        raise SystemExit(process.returncode)
    return process.stdout


def render() -> tuple[str, dict[Path, str]]:
    with tempfile.TemporaryDirectory(prefix="slime-boot-bindings-") as temporary:
        staging = Path(temporary)
        for generator, _, _ in GENERATORS:
            run_generator(generator, staging)

        fragments = []
        rust_outputs: dict[Path, str] = {}
        for _, python_name, rust_name in GENERATORS:
            path = staging / python_name
            if not path.exists():
                raise SystemExit(f"boot generator did not write {python_name}")
            fragment = path.read_text(encoding="utf-8")
            if INVALID_SCHEMA in fragment:
                raise SystemExit(f"boot schema reflection/layout validation failed in {python_name}")
            fragments.append(fragment.rstrip() + "\n\n")

            rust_path = staging / rust_name
            if not rust_path.exists():
                raise SystemExit(f"boot generator did not write {rust_name}")
            rust_fragment = rust_path.read_text(encoding="utf-8")
            if INVALID_SCHEMA in rust_fragment:
                raise SystemExit(f"boot schema reflection/layout validation failed in {rust_name}")
            rust_outputs[RUST_OUTPUT_DIR / rust_name] = format_rust(rust_fragment)

        return HEADER + "".join(fragments) + HELPERS, rust_outputs


def write_atomic(path: Path, contents: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(contents, encoding="utf-8")
    temporary.replace(path)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail when the checked-in bindings are stale",
    )
    arguments = parser.parse_args()
    generated, rust_outputs = render()
    if arguments.check:
        if not OUTPUT.exists() or OUTPUT.read_text(encoding="utf-8") != generated:
            raise SystemExit("generated boot bindings are stale; run `just boot_gen`")
        for path, contents in rust_outputs.items():
            if not path.exists() or path.read_text(encoding="utf-8") != contents:
                raise SystemExit(
                    f"generated {path.relative_to(ROOT)} is stale; run `just boot_gen`"
                )
        print("Boot contract bindings are current")
        return
    write_atomic(OUTPUT, generated)
    print(f"Generated {OUTPUT.relative_to(ROOT)}")
    for path, contents in rust_outputs.items():
        write_atomic(path, contents)
        print(f"Generated {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
