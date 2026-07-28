#!/usr/bin/env python3

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import os
import subprocess
import sys
from pathlib import Path
from zutai_cli import STDLIB, binary

from harness import ROOT

GENERATION_CONTRACT = ROOT / "contracts" / "generation" / "v1"
BLOCK_CONTRACT = ROOT / "contracts" / "block" / "v1"
BLOCK_BINDING_GENERATOR = ROOT / "scripts" / "generate" / "generate-block-bindings.py"
COMPONENT_CONTRACT = ROOT / "contracts" / "component" / "v1"
COMPONENT_BINDING_GENERATOR = ROOT / "scripts" / "generate" / "generate-component-bindings.py"
STORE_CONTRACT = ROOT / "contracts" / "store" / "v1"
STORE_BINDING_GENERATOR = ROOT / "scripts" / "generate" / "generate-store-bindings.py"
FS_CONTRACT = ROOT / "contracts" / "fs" / "v1"
FS_BINDING_GENERATOR = ROOT / "scripts" / "generate" / "generate-fs-bindings.py"
GENERATION_MANAGEMENT_CONTRACT = ROOT / "contracts" / "generation-management" / "v1"
GENERATION_MANAGEMENT_BINDING_GENERATOR = ROOT / "scripts" / "generate" / "generate-generation-management-bindings.py"
POWERBOX_CONTRACT = ROOT / "contracts" / "powerbox" / "v1"
POWERBOX_BINDING_GENERATOR = ROOT / "scripts" / "generate" / "generate-powerbox-bindings.py"
BOOT_BINDING_GENERATOR = ROOT / "scripts" / "generate" / "generate-boot-bindings.py"
GENERATION_V2_CONTRACT = ROOT / "contracts" / "generation" / "v2"
GENERATION_V3_CONTRACT = ROOT / "contracts" / "generation" / "v3"
KERNEL_IMAGE_CONTRACT = ROOT / "contracts" / "kernel-image" / "v1"
BOOTSTATE_CONTRACT = ROOT / "contracts" / "bootstate" / "v1"
BOOTSTATE_TRACE_CONTRACT = ROOT / "contracts" / "bootstate" / "trace" / "v1"
RECOVERY_CONTRACT = ROOT / "contracts" / "recovery" / "v1"
TRANSFER_CONTRACT = ROOT / "contracts" / "transfer" / "v1"
STORE_DISK_CONTRACT = ROOT / "contracts" / "store" / "disk" / "v1"
HANDOFF_CONTRACT = ROOT / "contracts" / "handoff" / "v1"
RELEASE_CONTRACT = ROOT / "contracts" / "release" / "v1"
SHARED_BUFFER_BUDGET_CONTRACT = ROOT / "contracts" / "shared-buffer-budget" / "v1"
SAMPLE_DESCRIPTOR_CONTRACT = ROOT / "contracts" / "sample-descriptor" / "v1"
SAMPLE_DESCRIPTOR_BINDING_GENERATOR = ROOT / "scripts" / "generate" / "generate-sample-descriptor-bindings.py"
INTERFACE_SCHEMA_CONTRACT = ROOT / "contracts" / "interface-schema" / "v1"
INTERFACE_SCHEMA_BINDING_GENERATOR = (
    ROOT / "scripts" / "generate" / "generate-interface-schema-bindings.py"
)
FABRIC_GRAPH_CONTRACT = ROOT / "contracts" / "fabric-graph" / "v1"
CAPABILITY_TRANSFER_CONTRACT = ROOT / "contracts" / "capability-transfer" / "v1"
CAPABILITY_TRANSFER_BINDING_GENERATOR = (
    ROOT / "scripts" / "generate" / "generate-capability-transfer-bindings.py"
)
FABRIC_STREAM_CONTRACT = ROOT / "contracts" / "fabric-stream" / "v1"
FABRIC_STREAM_BINDING_GENERATOR = (
    ROOT / "scripts" / "generate" / "generate-fabric-stream-bindings.py"
)


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

run("check", str(FS_CONTRACT / "schema.zt"))
run("check", str(FS_CONTRACT / "gen_rust.zt"))
subprocess.run(
    [sys.executable, str(FS_BINDING_GENERATOR), "--check"],
    cwd=ROOT,
    check=True,
)

run("check", str(GENERATION_MANAGEMENT_CONTRACT / "schema.zt"))
run("check", str(GENERATION_MANAGEMENT_CONTRACT / "gen_rust.zt"))
subprocess.run(
    [sys.executable, str(GENERATION_MANAGEMENT_BINDING_GENERATOR), "--check"],
    cwd=ROOT,
    check=True,
)

run("check", str(POWERBOX_CONTRACT / "schema.zt"))
run("check", str(POWERBOX_CONTRACT / "gen_rust.zt"))
subprocess.run(
    [sys.executable, str(POWERBOX_BINDING_GENERATOR), "--check"],
    cwd=ROOT,
    check=True,
)

run("check", str(TRANSFER_CONTRACT / "schema.zt"))
run("check", str(TRANSFER_CONTRACT / "gen_rust.zt"))

run("check", str(SAMPLE_DESCRIPTOR_CONTRACT / "schema.zt"))
run("check", str(SAMPLE_DESCRIPTOR_CONTRACT / "gen_rust.zt"))
subprocess.run(
    [sys.executable, str(SAMPLE_DESCRIPTOR_BINDING_GENERATOR), "--check"],
    cwd=ROOT,
    check=True,
)

run("check", str(CAPABILITY_TRANSFER_CONTRACT / "schema.zt"))
run("check", str(CAPABILITY_TRANSFER_CONTRACT / "gen_rust.zt"))
subprocess.run(
    [sys.executable, str(CAPABILITY_TRANSFER_BINDING_GENERATOR), "--check"],
    cwd=ROOT,
    check=True,
)

run("check", str(FABRIC_STREAM_CONTRACT / "schema.zt"))
run("check", str(FABRIC_STREAM_CONTRACT / "gen_rust.zt"))
subprocess.run(
    [sys.executable, str(FABRIC_STREAM_BINDING_GENERATOR), "--check"],
    cwd=ROOT,
    check=True,
)
run("check", str(INTERFACE_SCHEMA_CONTRACT / "schema.zt"))
run("check", str(INTERFACE_SCHEMA_CONTRACT / "check.zt"))
run("check", str(INTERFACE_SCHEMA_CONTRACT / "gen_python.zt"))
subprocess.run(
    [sys.executable, str(INTERFACE_SCHEMA_BINDING_GENERATOR), "--check"],
    cwd=ROOT,
    check=True,
)


for contract in (
    GENERATION_V2_CONTRACT,
    GENERATION_V3_CONTRACT,
    KERNEL_IMAGE_CONTRACT,
    BOOTSTATE_CONTRACT,
    BOOTSTATE_TRACE_CONTRACT,
    RECOVERY_CONTRACT,
    STORE_DISK_CONTRACT,
    HANDOFF_CONTRACT,
    RELEASE_CONTRACT,
    SHARED_BUFFER_BUDGET_CONTRACT,
    FABRIC_GRAPH_CONTRACT,
):
    run("check", str(contract / "schema.zt"))
    run("check", str(contract / "gen_rust.zt"))

for contract in (GENERATION_V2_CONTRACT, GENERATION_V3_CONTRACT):
    invalid_boot_layout = run("run", str(contract / "check-invalid-layout.zt"))
    if "INVALID_GENERATION_SCHEMA" not in invalid_boot_layout:
        raise SystemExit("generation wire-layout mismatch was not rejected")
subprocess.run(
    [sys.executable, str(BOOT_BINDING_GENERATOR), "--check"],
    cwd=ROOT,
    check=True,
)

subprocess.run(
    ["cargo", "test", "--quiet", "--lib", "-p", "boot-contracts"],
    cwd=ROOT,
    check=True,
)

print(
    "Generation source/binary, kernel image, BootState, BootState trace, recovery, "
    "block, component, store, spawn, filesystem, powerbox, generation-management, "
    "transfer, sample-descriptor, interface-schema, fabric-graph, "
    "capability-transfer, and fabric-stream contracts passed"
)
