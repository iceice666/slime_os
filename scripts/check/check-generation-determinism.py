#!/usr/bin/env python3
"""Build the product seL4 generation twice and verify identical admitted bytes."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BUILD = ROOT / "scripts" / "build" / "build-generation.py"
sys.path.insert(0, str(ROOT / "scripts" / "lib"))

from harness import load_script  # noqa: E402

CHECK = load_script("check_generation", "check/check-generation.py")


def fail(message: str) -> None:
    raise SystemExit(f"generation determinism check: {message}")


def build(output: Path, target_dir: Path) -> None:
    environment = os.environ.copy()
    environment["SLIME_TARGET_PROFILE"] = "aarch64-sel4-qemu-virt"
    environment["SLIME_SEL4_MANIFEST"] = "sel4"
    environment["CARGO_TARGET_DIR"] = str(target_dir)
    process = subprocess.run(
        [str(BUILD), str(output)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    sys.stdout.write(process.stdout)
    if process.returncode != 0:
        fail(f"builder exited with status {process.returncode}")


def compare(first: Path, second: Path, name: str) -> bytes:
    first_bytes = (first / name).read_bytes()
    second_bytes = (second / name).read_bytes()
    if first_bytes != second_bytes:
        fail(f"two identical builds produced different {name} bytes")
    return first_bytes


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="slime-generation-determinism-") as directory:
        root = Path(directory)
        first = root / "first"
        second = root / "second"
        build(first, root / "target-first")
        build(second, root / "target-second")
        generation = compare(first, second, "generation.bin")
        first_generation_one = (first / "generation-1.bin").read_bytes()
        second_generation_one = (second / "generation-1.bin").read_bytes()
        if first_generation_one != generation or second_generation_one != generation:
            fail("generation-1.bin is not the independently written generation.bin alias")
        bootstore = compare(first, second, "boot-store.bin")
        checked_generation = CHECK.check_generation(generation)
        checked_store = CHECK.check_bootstore(bootstore)
        if checked_store["selected"]["identity"] != checked_generation["identity"]:
            fail("boot store did not select the independently admitted generation")

    print(
        "generation determinism check: two isolated builds forced the sel4 manifest "
        "and produced byte-identical generation.bin and boot-store.bin; each build's "
        "independently written generation-1.bin alias matched its generation, and "
        "generation and boot store admission passed"
    )


if __name__ == "__main__":
    main()
