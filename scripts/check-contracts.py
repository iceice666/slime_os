#!/usr/bin/env python3

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path
import tempfile

ROOT = Path(__file__).resolve().parent.parent
ZUTAI_MANIFEST = ROOT / "deps" / "zutai" / "Cargo.toml"
STDLIB = ROOT / "deps" / "zutai" / "stdlib"
GENERATION_CONTRACT = ROOT / "contracts" / "generation" / "v1"
BLOCK_CONTRACT = ROOT / "contracts" / "block" / "v1"
BLOCK_BINDING_GENERATOR = ROOT / "scripts" / "generate-block-bindings.py"
COMPONENT_CONTRACT = ROOT / "contracts" / "component" / "v1"
COMPONENT_BINDING_GENERATOR = ROOT / "scripts" / "generate-component-bindings.py"


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


run("check", str(GENERATION_CONTRACT / "schema.zt"))

valid = run("run", str(GENERATION_CONTRACT / "check-valid.zt"))
if not valid.startswith("#valid"):
    raise SystemExit("valid generation fixture did not decode as #valid")

invalid = run("run", str(GENERATION_CONTRACT / "check-invalid.zt"))
if not invalid.startswith("#invalid") or "formatVersion" not in invalid:
    raise SystemExit("invalid generation fixture did not report formatVersion")

run("check", str(BLOCK_CONTRACT / "schema.zt"))
run("check", str(BLOCK_CONTRACT / "gen_rust.zt"))
subprocess.run(
    [sys.executable, str(BLOCK_BINDING_GENERATOR), "--check"],
    cwd=ROOT,
    check=True,
)

with tempfile.TemporaryDirectory(prefix="slime-block-contract-") as temporary:
    source = Path(temporary) / "block-binding-check.S"
    obj = Path(temporary) / "block-binding-check.o"
    source.write_text(
        ".intel_syntax noprefix\n"
        '.include "block_proto.inc"\n'
        ".section .text\n"
        ".global _start\n"
        "_start:\n"
        "    BLOCK_VALIDATE_REQUEST rdi, rsi, invalid\n"
        "    BLOCK_VALIDATE_REPLY rdi, rsi, invalid\n"
        "invalid:\n"
        "    ret\n",
        encoding="utf-8",
    )
    subprocess.run(
        [
            "as",
            "--64",
            "-I",
            str(ROOT / "components" / "include"),
            "-o",
            str(obj),
            str(source),
        ],
        check=True,
    )

run("check", str(COMPONENT_CONTRACT / "schema.zt"))
run("check", str(COMPONENT_CONTRACT / "gen_rust.zt"))
subprocess.run(
    [sys.executable, str(COMPONENT_BINDING_GENERATOR), "--check"],
    cwd=ROOT,
    check=True,
)

print("Generation manifest, block protocol, and component image contracts passed")
