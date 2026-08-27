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

SCHEMA = ROOT / "contracts" / "component-runtime-abi" / "v1" / "schema.zt"
RUST_OUTPUT = ROOT / "boot-contracts" / "src" / "generated" / "component_runtime_abi.rs"
C_OUTPUT = ROOT / "components" / "runtime" / "include" / "slime" / "component_runtime_abi.h"
INVALID_SCHEMA = "INVALID_COMPONENT_RUNTIME_ABI_SCHEMA"


def render() -> tuple[str, str]:
    with tempfile.TemporaryDirectory(prefix="slime-component-runtime-abi-") as temporary:
        staging = Path(temporary)
        environment = os.environ.copy()
        environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
        environment["SLIME_COMPONENT_RUNTIME_ABI_BINDINGS_ROOT"] = str(staging)
        process = subprocess.run(
            [str(binary()), "run", str(SCHEMA)],
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
        rust = staging / "component_runtime_abi.rs"
        c = staging / "component_runtime_abi.h"
        if not rust.exists() or not c.exists():
            raise SystemExit("component-runtime-abi generator did not write Rust and C bindings")
        rust_text = rust.read_text(encoding="utf-8")
        c_text = c.read_text(encoding="utf-8")
        if INVALID_SCHEMA in rust_text or INVALID_SCHEMA in c_text:
            raise SystemExit("component-runtime-abi schema validation failed")
        return rust_text, c_text


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
    rust_text, c_text = render()
    rust_text = format_rust(rust_text)
    outputs = ((RUST_OUTPUT, rust_text), (C_OUTPUT, c_text))
    if arguments.check:
        for path, contents in outputs:
            if not path.exists() or path.read_text(encoding="utf-8") != contents:
                raise SystemExit(
                    f"generated {path.relative_to(ROOT)} is stale; "
                    "run `just component_runtime_abi_gen`"
                )
        print("Component runtime ABI bindings are current")
        return
    for path, contents in outputs:
        write_atomic(path, contents)
        print(f"Generated {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
