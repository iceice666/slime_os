#!/usr/bin/env python3

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import os
import subprocess
import sys
import tempfile
from pathlib import Path

from harness import ROOT
from zutai_cli import STDLIB, binary

GENERATOR = ROOT / "contracts" / "fabric-visibility" / "v1" / "schema.zt"
RUST_OUTPUT = ROOT / "components" / "proto" / "src" / "fabric_visibility.rs"
PYTHON_OUTPUT = ROOT / "scripts" / "lib" / "fabric_visibility_contract.py"
INVALID_SCHEMA = "INVALID_FABRIC_VISIBILITY_SCHEMA"


def render() -> tuple[str, str]:
    with tempfile.TemporaryDirectory(prefix="slime-fabric-visibility-bindings-") as temporary:
        staging = Path(temporary)
        staged_rust = staging / "components" / "proto" / "src" / "fabric_visibility.rs"
        staged_python = staging / "scripts" / "lib" / "fabric_visibility_contract.py"
        staged_rust.parent.mkdir(parents=True)
        staged_python.parent.mkdir(parents=True)
        environment = os.environ.copy()
        environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
        environment["SLIME_FABRIC_VISIBILITY_BINDINGS_ROOT"] = str(staging)
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
        if not staged_rust.exists() or not staged_python.exists():
            raise SystemExit("fabric-visibility generator did not write both bindings")
        rust = staged_rust.read_text(encoding="utf-8")
        python = staged_python.read_text(encoding="utf-8")
        if INVALID_SCHEMA in rust or INVALID_SCHEMA in python:
            raise SystemExit("fabric-visibility schema reflection/layout validation failed")
        return rust, python


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
    parser.add_argument("--check", action="store_true")
    arguments = parser.parse_args()
    rust, python = render()
    outputs = ((RUST_OUTPUT, format_rust(rust)), (PYTHON_OUTPUT, python))
    if arguments.check:
        for path, generated in outputs:
            if not path.exists() or path.read_text(encoding="utf-8") != generated:
                raise SystemExit(
                    f"generated {path.name} is stale; run `just fabric_visibility_gen`"
                )
        print("Fabric-visibility protocol bindings are current")
        return
    for path, generated in outputs:
        write_atomic(path, generated)
        print(f"Generated {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
