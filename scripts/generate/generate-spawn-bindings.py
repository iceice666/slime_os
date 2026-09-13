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
from zutai_cli import STDLIB, binary

from harness import ROOT

GENERATOR = ROOT / "contracts" / "spawn" / "v1" / "schema.zt"
RUST_OUTPUT = ROOT / "components" / "proto" / "src" / "spawn.rs"
C_OUTPUT = ROOT / "components" / "runtime" / "include" / "slime" / "spawn.h"
INVALID_SCHEMA = "INVALID_SPAWN_SCHEMA"


def render() -> tuple[str, str]:
    with tempfile.TemporaryDirectory(prefix="slime-spawn-bindings-") as temporary:
        staging = Path(temporary)
        staged_rust = staging / RUST_OUTPUT.relative_to(ROOT)
        staged_c = staging / C_OUTPUT.relative_to(ROOT)
        staged_rust.parent.mkdir(parents=True)
        staged_c.parent.mkdir(parents=True)
        environment = os.environ.copy()
        environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
        environment["SLIME_SPAWN_BINDINGS_ROOT"] = str(staging)
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
        if not staged_rust.exists() or not staged_c.exists():
            raise SystemExit("spawn generator did not write both bindings")
        generated_rust = staged_rust.read_text(encoding="utf-8")
        generated_c = staged_c.read_text(encoding="utf-8")
        if INVALID_SCHEMA in generated_rust or INVALID_SCHEMA in generated_c:
            raise SystemExit("spawn schema reflection/layout validation failed")
        return generated_rust, generated_c


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
    generated_rust, generated_c = render()
    generated_rust = format_rust(generated_rust)
    outputs = ((RUST_OUTPUT, generated_rust), (C_OUTPUT, generated_c))
    if arguments.check:
        for output, generated in outputs:
            if not output.exists() or output.read_text(encoding="utf-8") != generated:
                raise SystemExit("generated spawn bindings are stale; run `just spawn_gen`")
        print("Spawn protocol bindings are current")
        return
    for output, generated in outputs:
        write_atomic(output, generated)
        print(f"Generated {output.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
