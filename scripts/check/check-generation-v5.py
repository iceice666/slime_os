#!/usr/bin/env python3
"""Every generation this repository builds is v5, and nothing still writes v4.

B50's exit condition asks that "every fixture uses v5". The wire format cut
over at `8745d18`, and the v4 binding generator is still on disk under
`contracts/generation/v4/` because the format's history is part of the
contract. What must not survive is a *producer*: a manifest the builder still
encodes as v4, or a second `GENERATION_VERSION` that some path selects.

Checked by building, not by reading. A manifest can declare whatever it likes
in `formatVersion` -- that field is the *manifest* schema's version, not the
wire format's, and the two are easy to confuse. The authority is the magic and
version word in the bytes the root actually decodes.
"""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

sys.path.insert(0, str(ROOT / "scripts" / "lib"))
sys.path.insert(0, str(ROOT / "scripts" / "build"))

EXPECTED_MAGIC = b"SLIMEG5\0"
EXPECTED_VERSION = 5


def fail(message: str) -> None:
    print(f"generation v5 check: {message}", file=sys.stderr)
    raise SystemExit(1)


def load_builder():
    import importlib.util

    path = ROOT / "scripts" / "build" / "build-generation.py"
    spec = importlib.util.spec_from_file_location("build_generation_v5", path)
    if spec is None or spec.loader is None:
        fail(f"cannot load {path.relative_to(ROOT)}")
    module = importlib.util.module_from_spec(spec)
    sys.modules["build_generation_v5"] = module
    spec.loader.exec_module(module)
    return module


def check_single_version(builder) -> None:
    """One version constant, and it is the one the decoder expects.

    A second constant is how a v4 path survives a cutover: the format stays
    described in one place while some manifest quietly selects the other.
    """
    if builder.GENERATION_VERSION != EXPECTED_VERSION:
        fail(
            f"builder writes generation version {builder.GENERATION_VERSION}, "
            f"expected {EXPECTED_VERSION}"
        )
    magic = builder.GENERATION_MAGIC
    if isinstance(magic, str):
        magic = magic.encode()
    if magic != EXPECTED_MAGIC:
        fail(f"builder writes magic {magic!r}, expected {EXPECTED_MAGIC!r}")


def built_header(manifest: str) -> tuple[bytes, int]:
    """Build one manifest and return the magic and version it encodes."""
    with tempfile.TemporaryDirectory() as directory:
        environment = os.environ.copy()
        environment["SLIME_TARGET_PROFILE"] = "aarch64-sel4-qemu-virt"
        environment["SLIME_SEL4_MANIFEST"] = manifest
        result = subprocess.run(
            [
                sys.executable,
                str(ROOT / "scripts" / "build" / "build-generation.py"),
                directory,
            ],
            cwd=ROOT,
            env=environment,
            capture_output=True,
            text=True,
            check=False,
        )
        if result.returncode != 0:
            fail(f"{manifest}: build failed: {result.stderr.strip().splitlines()[-1:]}")
        blob = Path(directory) / "generation.bin"
        if not blob.is_file():
            fail(f"{manifest}: build produced no generation.bin")
        header = blob.read_bytes()[:12]
        return header[:8], int.from_bytes(header[8:12], "little")


def main() -> None:
    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    builder = load_builder()
    check_single_version(builder)

    manifests = sorted(builder.SEL4_MANIFESTS)
    if not manifests:
        fail("the builder declares no seL4 manifests")
    for manifest in manifests:
        magic, version = built_header(manifest)
        if magic != EXPECTED_MAGIC or version != EXPECTED_VERSION:
            fail(
                f"{manifest} encodes magic {magic!r} version {version}, "
                f"expected {EXPECTED_MAGIC!r} version {EXPECTED_VERSION}"
            )
    label = EXPECTED_MAGIC.rstrip(b"\0").decode()
    print(
        f"generation v5 check: all {len(manifests)} seL4 manifests encode "
        f"{label} version {EXPECTED_VERSION}",
        flush=True,
    )


if __name__ == "__main__":
    main()
