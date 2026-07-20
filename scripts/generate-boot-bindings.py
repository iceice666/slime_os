#!/usr/bin/env python3

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "scripts" / "boot_contracts.py"
ZUTAI_MANIFEST = ROOT / "deps" / "zutai" / "Cargo.toml"
STDLIB = ROOT / "deps" / "zutai" / "stdlib"
GENERATORS = (
    (ROOT / "contracts" / "generation" / "v2" / "schema.zt", "generation.py"),
    (ROOT / "contracts" / "kernel-image" / "v1" / "schema.zt", "kernel_image.py"),
    (ROOT / "contracts" / "bootstate" / "v1" / "schema.zt", "bootstate.py"),
    (ROOT / "contracts" / "release" / "v1" / "schema.zt", "release.py"),
)
INVALID_SCHEMA = "INVALID_"
HEADER = """# @generated from boot contract schemas; do not edit.
from __future__ import annotations

import hashlib
import struct

"""
TRACE = """BOOTSTATE_TRACE_VERSION = 1
BOOTSTATE_TRACE_MAX_LINE = 640

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
        [
            "cargo",
            "run",
            "--manifest-path",
            str(ZUTAI_MANIFEST),
            "-q",
            "-p",
            "zutai-cli",
            "--",
            "run",
            str(generator),
        ],
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


def render() -> str:
    with tempfile.TemporaryDirectory(prefix="slime-boot-bindings-") as temporary:
        staging = Path(temporary)
        for generator, _ in GENERATORS:
            run_generator(generator, staging)

        fragments = []
        for _, name in GENERATORS:
            path = staging / name
            if not path.exists():
                raise SystemExit(f"boot generator did not write {name}")
            fragment = path.read_text(encoding="utf-8")
            if INVALID_SCHEMA in fragment:
                raise SystemExit(f"boot schema reflection/layout validation failed in {name}")
            fragments.append(fragment.rstrip() + "\n\n")
        return HEADER + "".join(fragments) + TRACE + HELPERS


def write_atomic(path: Path, contents: str) -> None:
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(contents, encoding="utf-8")
    temporary.replace(path)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail when the checked-in Python bindings are stale",
    )
    arguments = parser.parse_args()
    generated = render()
    if arguments.check:
        if not OUTPUT.exists() or OUTPUT.read_text(encoding="utf-8") != generated:
            raise SystemExit("generated boot bindings are stale; run `just boot_gen`")
        print("Boot contract bindings are current")
        return
    write_atomic(OUTPUT, generated)
    print(f"Generated {OUTPUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
