#!/usr/bin/env python3
"""Render the entropy-source/v1 and entropy-service/v1 protocol bindings."""

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

PROTO = Path("components") / "proto" / "src"
# (contract, generated Rust module, invalid-schema sentinel)
CONTRACTS = (
    ("entropy-source", PROTO / "entropy_source.rs", "INVALID_ENTROPY_SOURCE_SCHEMA"),
    ("entropy-service", PROTO / "entropy_service.rs", "INVALID_ENTROPY_SERVICE_SCHEMA"),
)


def render(contract: str, output: Path, invalid: str) -> str:
    generator = ROOT / "contracts" / contract / "v1" / "schema.zt"
    with tempfile.TemporaryDirectory(prefix=f"slime-{contract}-bindings-") as temporary:
        staging = Path(temporary)
        staged = staging / output
        staged.parent.mkdir(parents=True)
        environment = os.environ.copy()
        environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
        environment["SLIME_ENTROPY_BINDINGS_ROOT"] = str(staging)
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
        if not staged.exists():
            raise SystemExit(f"{contract} generator did not write {output}")
        generated = staged.read_text(encoding="utf-8")
        if invalid in generated:
            raise SystemExit(f"{contract} schema reflection/layout validation failed")
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
    parser.add_argument("--check", action="store_true")
    arguments = parser.parse_args()
    outputs = [(ROOT / output, format_rust(render(contract, output, invalid))) for contract, output, invalid in CONTRACTS]
    if arguments.check:
        for output, generated in outputs:
            if not output.exists() or output.read_text(encoding="utf-8") != generated:
                raise SystemExit("generated entropy bindings are stale; run `just entropy_gen`")
        print("entropy-source and entropy-service protocol bindings are current")
        return
    for output, generated in outputs:
        write_atomic(output, generated)
        print(f"Generated {output.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
