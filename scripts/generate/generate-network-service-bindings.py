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

GENERATOR = ROOT / "contracts" / "network-service" / "v1" / "schema.zt"
OUTPUT = ROOT / "components" / "proto" / "src" / "network_service.rs"
PYTHON_OUTPUT = ROOT / "scripts" / "lib" / "network_launch.py"
INVALID_SCHEMA = "INVALID_NETWORK_SERVICE_SCHEMA"


def render() -> tuple[str, str]:
    with tempfile.TemporaryDirectory(prefix="slime-network-service-bindings-") as temporary:
        staging = Path(temporary)
        staged = staging / "components" / "proto" / "src" / "network_service.rs"
        staged.parent.mkdir(parents=True)
        environment = os.environ.copy()
        environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
        environment["SLIME_NETWORK_SERVICE_BINDINGS_ROOT"] = str(staging)
        process = subprocess.run(
            [str(binary()), "run", str(GENERATOR)], cwd=ROOT, env=environment,
            check=False, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        if process.returncode != 0:
            sys.stderr.write(process.stdout)
            sys.stderr.write(process.stderr)
            raise SystemExit(process.returncode)
        if not staged.exists():
            raise SystemExit("network-service generator did not write bindings")
        generated = staged.read_text(encoding="utf-8")
        if INVALID_SCHEMA in generated:
            raise SystemExit("network-service schema reflection/layout validation failed")
        python = (staging / "network_launch.py").read_text(encoding="utf-8")
        if INVALID_SCHEMA in python:
            raise SystemExit("network launch schema validation failed")
        return generated, python


def format_rust(source: str) -> str:
    process = subprocess.run(
        ["rustfmt", "--edition", "2024", "--emit", "stdout"], cwd=ROOT,
        input=source, check=False, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
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
    generated = format_rust(rust)
    if arguments.check:
        if not OUTPUT.exists() or OUTPUT.read_text(encoding="utf-8") != generated:
            raise SystemExit("generated network-service bindings are stale")
        if not PYTHON_OUTPUT.exists() or PYTHON_OUTPUT.read_text(encoding="utf-8") != python:
            raise SystemExit("generated network launch bindings are stale")
        print("Network service protocol bindings are current")
        return
    write_atomic(OUTPUT, generated)
    write_atomic(PYTHON_OUTPUT, python)
    print(f"Generated {OUTPUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
