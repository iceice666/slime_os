#!/usr/bin/env python3

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from zutai_cli import STDLIB, binary

from harness import ROOT

GENERATOR = ROOT / "contracts" / "store" / "v1" / "schema.zt"
RUST_OUTPUT = ROOT / "kernel" / "src" / "store_proto" / "gen.rs"
COMPONENT_RUST_OUTPUT = ROOT / "components" / "proto" / "src" / "store.rs"
INVALID_SCHEMA = "INVALID_STORE_SCHEMA"


def render() -> dict[Path, str]:
    with tempfile.TemporaryDirectory(prefix="slime-store-bindings-") as temporary:
        staging = Path(temporary)
        staged_rust = staging / "kernel" / "src" / "store_proto" / "gen.rs"
        staged_component_rust = staging / "components" / "proto" / "src" / "store.rs"
        staged_rust.parent.mkdir(parents=True)
        staged_component_rust.parent.mkdir(parents=True)

        environment = os.environ.copy()
        environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
        environment["SLIME_STORE_BINDINGS_ROOT"] = str(staging)
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
        if not staged_rust.exists() or not staged_component_rust.exists():
            raise SystemExit("store generator did not write all binding surfaces")

        generated = {
            RUST_OUTPUT: staged_rust.read_text(encoding="utf-8"),
            COMPONENT_RUST_OUTPUT: staged_component_rust.read_text(encoding="utf-8"),
        }
        if INVALID_SCHEMA in generated.values():
            raise SystemExit("store schema reflection/layout validation failed")
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
    rendered = render()
    generated = {
        RUST_OUTPUT: format_rust(rendered[RUST_OUTPUT]),
        COMPONENT_RUST_OUTPUT: format_rust(rendered[COMPONENT_RUST_OUTPUT]),
    }

    if arguments.check:
        for path, contents in generated.items():
            if not path.exists():
                raise SystemExit(f"missing generated store bindings: {path.relative_to(ROOT)}")
            if path.read_text(encoding="utf-8") != contents:
                raise SystemExit("generated store bindings are stale; run `just store_gen`")
        print("Store protocol bindings are current")
        return

    for path, contents in generated.items():
        write_atomic(path, contents)
        print(f"Generated {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
