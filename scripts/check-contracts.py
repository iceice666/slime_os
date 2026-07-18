#!/usr/bin/env python3

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ZUTAI_MANIFEST = ROOT / "deps" / "zutai" / "Cargo.toml"
STDLIB = ROOT / "deps" / "zutai" / "stdlib"
CONTRACT = ROOT / "contracts" / "generation" / "v1"


def run(*arguments: str) -> str:
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    process = subprocess.run(
        [
            "cargo",
            "run",
            "--manifest-path",
            str(ZUTAI_MANIFEST),
            "-q",
            "-p",
            "zutai-cli",
            "--",
            *arguments,
        ],
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
    return process.stdout


run("check", str(CONTRACT / "schema.zt"))

valid = run("run", str(CONTRACT / "check-valid.zt"))
if not valid.startswith("#valid"):
    raise SystemExit("valid generation fixture did not decode as #valid")

invalid = run("run", str(CONTRACT / "check-invalid.zt"))
if not invalid.startswith("#invalid") or "formatVersion" not in invalid:
    raise SystemExit("invalid generation fixture did not report formatVersion")

print("Generation manifest contracts passed")
