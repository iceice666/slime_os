#!/usr/bin/env python3

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path
from zutai_cli import STDLIB, binary

ROOT = Path(__file__).resolve().parent.parent
GENERATION_CONTRACT = ROOT / "contracts" / "generation" / "v1"
BLOCK_CONTRACT = ROOT / "contracts" / "block" / "v1"
BLOCK_BINDING_GENERATOR = ROOT / "scripts" / "generate-block-bindings.py"
COMPONENT_CONTRACT = ROOT / "contracts" / "component" / "v1"
COMPONENT_BINDING_GENERATOR = ROOT / "scripts" / "generate-component-bindings.py"
STORE_CONTRACT = ROOT / "contracts" / "store" / "v1"
STORE_BINDING_GENERATOR = ROOT / "scripts" / "generate-store-bindings.py"
BOOT_BINDING_GENERATOR = ROOT / "scripts" / "generate-boot-bindings.py"
GENERATION_V2_CONTRACT = ROOT / "contracts" / "generation" / "v2"
KERNEL_IMAGE_CONTRACT = ROOT / "contracts" / "kernel-image" / "v1"
BOOTSTATE_CONTRACT = ROOT / "contracts" / "bootstate" / "v1"
BOOTSTATE_TRACE_CONTRACT = ROOT / "contracts" / "bootstate" / "trace" / "v1"
RECOVERY_CONTRACT = ROOT / "contracts" / "recovery" / "v1"


def run(*arguments: str) -> str:
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    process = subprocess.run(
        [str(binary()), *arguments],
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

run("check", str(COMPONENT_CONTRACT / "schema.zt"))
run("check", str(COMPONENT_CONTRACT / "gen_rust.zt"))
subprocess.run(
    [sys.executable, str(COMPONENT_BINDING_GENERATOR), "--check"],
    cwd=ROOT,
    check=True,
)

run("check", str(STORE_CONTRACT / "schema.zt"))
run("check", str(STORE_CONTRACT / "gen_rust.zt"))
subprocess.run(
    [sys.executable, str(STORE_BINDING_GENERATOR), "--check"],
    cwd=ROOT,
    check=True,
)

for contract in (
    GENERATION_V2_CONTRACT,
    KERNEL_IMAGE_CONTRACT,
    BOOTSTATE_CONTRACT,
    BOOTSTATE_TRACE_CONTRACT,
    RECOVERY_CONTRACT,
):
    run("check", str(contract / "schema.zt"))
    run("check", str(contract / "gen_rust.zt"))

invalid_boot_layout = run("run", str(GENERATION_V2_CONTRACT / "check-invalid-layout.zt"))
if "INVALID_GENERATION_SCHEMA" not in invalid_boot_layout:
    raise SystemExit("generation wire-layout mismatch was not rejected")
subprocess.run(
    [sys.executable, str(BOOT_BINDING_GENERATOR), "--check"],
    cwd=ROOT,
    check=True,
)

print(
    "Generation source/binary, kernel image, BootState, BootState trace, recovery, "
    "block, component, and store contracts passed"
)
