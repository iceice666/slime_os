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
GENERATOR = ROOT / "contracts" / "component" / "v1" / "schema.zt"
RUST_OUTPUT = ROOT / "kernel" / "src" / "component" / "gen.rs"
INVALID_SCHEMA = "INVALID_COMPONENT_SCHEMA"


def render() -> str:
    with tempfile.TemporaryDirectory(prefix="slime-component-bindings-") as temporary:
        staging = Path(temporary)
        staged_rust = staging / "kernel" / "src" / "component" / "gen.rs"
        staged_rust.parent.mkdir(parents=True)

        environment = os.environ.copy()
        environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
        environment["SLIME_COMPONENT_BINDINGS_ROOT"] = str(staging)
        process = subprocess.run(
            [str(binary()), "run", str(GENERATOR)],
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
        if not staged_rust.exists():
            raise SystemExit("component generator did not write the Rust binding surface")

        generated = staged_rust.read_text(encoding="utf-8")
        if INVALID_SCHEMA in generated:
            raise SystemExit("component schema reflection/layout validation failed")
        return generated


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
        help="fail when the checked-in Rust bindings are stale",
    )
    arguments = parser.parse_args()
    generated = format_rust(render())

    if arguments.check:
        if not RUST_OUTPUT.exists():
            raise SystemExit(
                f"missing generated component bindings: {RUST_OUTPUT.relative_to(ROOT)}"
            )
        if RUST_OUTPUT.read_text(encoding="utf-8") != generated:
            raise SystemExit(
                "generated component bindings are stale; run `just component_gen`"
            )
        print("Component image bindings are current")
        return

    write_atomic(RUST_OUTPUT, generated)
    print(f"Generated {RUST_OUTPUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
