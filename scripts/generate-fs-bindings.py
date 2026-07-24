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

GENERATOR = ROOT / "contracts" / "fs" / "v1" / "schema.zt"
OUTPUT = ROOT / "components" / "proto" / "src" / "fs.rs"
PYTHON_OUTPUT = ROOT / "scripts" / "fs_contracts.py"
INVALID_SCHEMA = "INVALID_FS_SCHEMA"


def render() -> tuple[str, str]:
    with tempfile.TemporaryDirectory(prefix="slime-fs-bindings-") as temporary:
        staging = Path(temporary)
        staged = staging / "components" / "proto" / "src" / "fs.rs"
        staged_python = staging / "fs_contracts.py"
        staged.parent.mkdir(parents=True)
        environment = os.environ.copy()
        environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
        environment["SLIME_FS_BINDINGS_ROOT"] = str(staging)
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
        if not staged.exists() or not staged_python.exists():
            raise SystemExit("filesystem generator did not write all binding surfaces")
        generated = staged.read_text(encoding="utf-8")
        generated_python = staged_python.read_text(encoding="utf-8")
        if INVALID_SCHEMA in generated or INVALID_SCHEMA in generated_python:
            raise SystemExit("filesystem schema reflection/layout validation failed")
        return generated, generated_python


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
    rendered_rust, rendered_python = render()
    generated = format_rust(rendered_rust)
    if arguments.check:
        if not OUTPUT.exists() or OUTPUT.read_text(encoding="utf-8") != generated:
            raise SystemExit("generated filesystem bindings are stale; run `just fs_gen`")
        if (
            not PYTHON_OUTPUT.exists()
            or PYTHON_OUTPUT.read_text(encoding="utf-8") != rendered_python
        ):
            raise SystemExit("generated filesystem bindings are stale; run `just fs_gen`")
        print("Filesystem protocol bindings are current")
        return
    write_atomic(OUTPUT, generated)
    print(f"Generated {OUTPUT.relative_to(ROOT)}")
    write_atomic(PYTHON_OUTPUT, rendered_python)
    print(f"Generated {PYTHON_OUTPUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
