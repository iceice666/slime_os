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

GENERATOR = ROOT / "contracts" / "generation-management" / "v1" / "schema.zt"
KERNEL_OUTPUT = ROOT / "kernel" / "src" / "generation_proto" / "gen.rs"
COMPONENT_OUTPUT = ROOT / "components" / "proto" / "src" / "generation.rs"
INVALID_SCHEMA = "INVALID_GENERATION_MANAGEMENT_SCHEMA"


def render() -> dict[Path, str]:
    with tempfile.TemporaryDirectory(prefix="slime-generation-management-bindings-") as temporary:
        staging = Path(temporary)
        staged_kernel = staging / "kernel" / "src" / "generation_proto" / "gen.rs"
        staged_component = staging / "components" / "proto" / "src" / "generation.rs"
        staged_kernel.parent.mkdir(parents=True)
        staged_component.parent.mkdir(parents=True)

        environment = os.environ.copy()
        environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
        environment["SLIME_GENERATION_MANAGEMENT_BINDINGS_ROOT"] = str(staging)
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
        if not staged_kernel.exists() or not staged_component.exists():
            raise SystemExit("generation-management generator did not write all binding surfaces")
        generated = {
            KERNEL_OUTPUT: staged_kernel.read_text(encoding="utf-8"),
            COMPONENT_OUTPUT: staged_component.read_text(encoding="utf-8"),
        }
        if INVALID_SCHEMA in generated.values():
            raise SystemExit("generation-management schema reflection/layout validation failed")
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
    rendered = render()
    generated = {path: format_rust(contents) for path, contents in rendered.items()}

    if arguments.check:
        for path, contents in generated.items():
            if not path.exists() or path.read_text(encoding="utf-8") != contents:
                raise SystemExit(
                    "generated generation-management bindings are stale; run `just generation_management_gen`"
                )
        print("Generation-management protocol bindings are current")
        return

    for path, contents in generated.items():
        write_atomic(path, contents)
        print(f"Generated {path.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
