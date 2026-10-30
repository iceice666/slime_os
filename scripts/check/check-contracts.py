#!/usr/bin/env python3

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import os
import subprocess
import sys
from zutai_cli import STDLIB, binary

from harness import GENERATION_CONTRACT, ROOT, load_script

BLOCK_CONTRACT = ROOT / "contracts" / "block" / "v1"
BLOCK_V2_CONTRACT = ROOT / "contracts" / "block" / "v2"
COMPONENT_CONTRACT = ROOT / "contracts" / "component" / "v1"
COMPONENT_V2_CONTRACT = ROOT / "contracts" / "component" / "v2"
STORE_CONTRACT = ROOT / "contracts" / "store" / "v1"
IO_QUEUE_CONTRACT = ROOT / "contracts" / "io-queue" / "v1"
LINK_DEVICE_CONTRACT = ROOT / "contracts" / "link-device" / "v1"
NETWORK_SERVICE_CONTRACT = ROOT / "contracts" / "network-service" / "v1"
ZENOH_PROFILE_CONTRACT = ROOT / "contracts" / "zenoh-profile" / "v1"
FS_CONTRACT = ROOT / "contracts" / "fs" / "v1"
GENERATION_MANAGEMENT_CONTRACT = ROOT / "contracts" / "generation-management" / "v1"
POWERBOX_CONTRACT = ROOT / "contracts" / "powerbox" / "v1"
BOOT_BINDING_GENERATOR = ROOT / "scripts" / "generate" / "generate-boot-bindings.py"
# Retained wire-format history. Nothing generates from these — `boot_gen` wires
# only v5 — but the format's history is part of the contract, so they are
# type-checked rather than left to rot.
# B64: v4 was the one retained version no gate touched, so it could have stopped
# parsing without anything noticing.
GENERATION_V2_CONTRACT = ROOT / "contracts" / "generation" / "v2"
GENERATION_V3_CONTRACT = ROOT / "contracts" / "generation" / "v3"
GENERATION_V4_CONTRACT = ROOT / "contracts" / "generation" / "v4"
KERNEL_IMAGE_CONTRACT = ROOT / "contracts" / "kernel-image" / "v1"
KERNEL_IMAGE_V2_CONTRACT = ROOT / "contracts" / "kernel-image" / "v2"
TARGET_PROFILE_CONTRACT = ROOT / "contracts" / "target-profile" / "v1"
BOOTSTATE_CONTRACT = ROOT / "contracts" / "bootstate" / "v1"
BOOTSTATE_TRACE_CONTRACT = ROOT / "contracts" / "bootstate" / "trace" / "v1"
RECOVERY_CONTRACT = ROOT / "contracts" / "recovery" / "v1"
TRANSFER_CONTRACT = ROOT / "contracts" / "transfer" / "v1"
STORE_DISK_CONTRACT = ROOT / "contracts" / "store" / "disk" / "v1"
RELEASE_CONTRACT = ROOT / "contracts" / "release" / "v1"
SHARED_BUFFER_BUDGET_CONTRACT = ROOT / "contracts" / "shared-buffer-budget" / "v1"
SAMPLE_DESCRIPTOR_CONTRACT = ROOT / "contracts" / "sample-descriptor" / "v1"
INTERFACE_SCHEMA_CONTRACT = ROOT / "contracts" / "interface-schema" / "v1"
INTERFACE_SCHEMA_BINDING_GENERATOR = (
    ROOT / "scripts" / "generate" / "generate-interface-schema-bindings.py"
)
COMPONENT_SPEC_CONTRACT = ROOT / "contracts" / "component-spec" / "v1"
SYSTEM_SPEC_CONTRACT = ROOT / "contracts" / "system-spec" / "v1"
COMPOSITION_INVENTORY_CONTRACT = ROOT / "contracts" / "composition-inventory" / "v1"
COMPONENT_SDK_RELEASE_CONTRACT = ROOT / "contracts" / "component-sdk-release" / "v1"
BOOT_MEDIA_CONTRACT = ROOT / "contracts" / "boot-media" / "v1"
CPU_BOOT_OBSERVATION_CONTRACT = ROOT / "contracts" / "cpu-boot-observation" / "v1"
SYSTEM_IMAGE_CLOSURE_CONTRACT = ROOT / "contracts" / "system-image-closure" / "v2"
SYSTEM_TEST_RUN_CONTRACT = ROOT / "contracts" / "system-test-run" / "v2"
FABRIC_GRAPH_CONTRACT = ROOT / "contracts" / "fabric-graph" / "v1"
CAPABILITY_TRANSFER_CONTRACT = ROOT / "contracts" / "capability-transfer" / "v1"
FABRIC_STREAM_CONTRACT = ROOT / "contracts" / "fabric-stream" / "v1"
FABRIC_QOS_CONTRACT = ROOT / "contracts" / "fabric-qos" / "v1"
FABRIC_TIME_CONTRACT = ROOT / "contracts" / "fabric-time" / "v1"
FABRIC_CALL_CONTRACT = ROOT / "contracts" / "fabric-call" / "v1"
FABRIC_OPERATION_CONTRACT = ROOT / "contracts" / "fabric-operation" / "v1"
FABRIC_VISIBILITY_CONTRACT = ROOT / "contracts" / "fabric-visibility" / "v1"
FABRIC_TRACE_CONTRACT = ROOT / "contracts" / "fabric-trace" / "v1"
DATA_FABRIC_PROFILE_CONTRACT = ROOT / "contracts" / "data-fabric-profile" / "v1"
NORMALIZED_INTERFACE_SCHEMAS_CONTRACT = ROOT / "contracts" / "normalized-interface-schemas" / "v1"

SLOT_PIN_REASON_CHECK = load_script(
    "generation_manifest_slot_pin_reasons", "check/contracts/slot_pin_reasons.py"
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
# Every binding in the shared table (`scripts/lib/zutai_bindings.py`): the
# committed outputs must equal what each contract's renderer produces.
subprocess.run(
    [sys.executable, str(ROOT / "scripts" / "generate" / "generate-bindings.py"), "--check", "--all"],
    cwd=ROOT,
    check=True,
)
run("check", str(BLOCK_V2_CONTRACT / "schema.zt"))
run("check", str(BLOCK_V2_CONTRACT / "gen_rust.zt"))

run("check", str(COMPONENT_CONTRACT / "schema.zt"))
run("check", str(COMPONENT_CONTRACT / "gen_rust.zt"))

run("check", str(IO_QUEUE_CONTRACT / "schema.zt"))
run("check", str(IO_QUEUE_CONTRACT / "gen_rust.zt"))
run("check", str(LINK_DEVICE_CONTRACT / "schema.zt"))
run("check", str(ZENOH_PROFILE_CONTRACT / "schema.zt"))
run("check", str(NETWORK_SERVICE_CONTRACT / "schema.zt"))
run("check", str(STORE_CONTRACT / "schema.zt"))
run("check", str(STORE_CONTRACT / "gen_rust.zt"))

run("check", str(FS_CONTRACT / "schema.zt"))
run("check", str(FS_CONTRACT / "gen_rust.zt"))

run("check", str(GENERATION_MANAGEMENT_CONTRACT / "schema.zt"))
run("check", str(GENERATION_MANAGEMENT_CONTRACT / "gen_rust.zt"))

run("check", str(POWERBOX_CONTRACT / "schema.zt"))
run("check", str(POWERBOX_CONTRACT / "gen_rust.zt"))
run("check", str(BOOT_MEDIA_CONTRACT / "schema.zt"))
run("check", str(CPU_BOOT_OBSERVATION_CONTRACT / "schema.zt"))

subprocess.run(
    [sys.executable, str(ROOT / "scripts/generate/generate-devloop-diagnostics-bindings.py"), "--check"],
    cwd=ROOT,
    check=True,
)

subprocess.run(
    [sys.executable, str(ROOT / "scripts/generate/generate-observation-capacity-bindings.py"), "--check"],
    cwd=ROOT,
    check=True,
)

run("check", str(TRANSFER_CONTRACT / "schema.zt"))
run("check", str(TRANSFER_CONTRACT / "gen_rust.zt"))

run("check", str(SAMPLE_DESCRIPTOR_CONTRACT / "schema.zt"))
run("check", str(SAMPLE_DESCRIPTOR_CONTRACT / "gen_rust.zt"))

run("check", str(CAPABILITY_TRANSFER_CONTRACT / "schema.zt"))
run("check", str(CAPABILITY_TRANSFER_CONTRACT / "gen_rust.zt"))

run("check", str(FABRIC_STREAM_CONTRACT / "schema.zt"))
run("check", str(FABRIC_STREAM_CONTRACT / "gen_rust.zt"))
run("check", str(FABRIC_QOS_CONTRACT / "schema.zt"))
run("check", str(FABRIC_QOS_CONTRACT / "gen_rust.zt"))
run("check", str(ROOT / "contracts/private-memory-probe/v1/schema.zt"))
run("check", str(ROOT / "contracts/private-memory-probe/v1/gen_rust.zt"))
run("check", str(FABRIC_TIME_CONTRACT / "schema.zt"))
run("check", str(FABRIC_TIME_CONTRACT / "gen_rust.zt"))
run("check", str(FABRIC_CALL_CONTRACT / "schema.zt"))
run("check", str(FABRIC_CALL_CONTRACT / "gen_rust.zt"))
run("check", str(FABRIC_OPERATION_CONTRACT / "schema.zt"))
run("check", str(FABRIC_OPERATION_CONTRACT / "gen_rust.zt"))
run("check", str(FABRIC_VISIBILITY_CONTRACT / "schema.zt"))
run("check", str(FABRIC_VISIBILITY_CONTRACT / "gen_rust.zt"))
run("check", str(FABRIC_TRACE_CONTRACT / "schema.zt"))
run("check", str(FABRIC_TRACE_CONTRACT / "gen_rust.zt"))
run("check", str(INTERFACE_SCHEMA_CONTRACT / "schema.zt"))
run("check", str(INTERFACE_SCHEMA_CONTRACT / "check.zt"))
run("check", str(INTERFACE_SCHEMA_CONTRACT / "gen_python.zt"))
subprocess.run(
    [sys.executable, str(INTERFACE_SCHEMA_BINDING_GENERATOR), "--check"],
    cwd=ROOT,
    check=True,
)
run("check", str(COMPONENT_SPEC_CONTRACT / "schema.zt"))
run("check", str(COMPONENT_SPEC_CONTRACT / "check.zt"))
run("check", str(COMPONENT_SPEC_CONTRACT / "gen_python.zt"))
run("check", str(SYSTEM_SPEC_CONTRACT / "schema.zt"))
run("check", str(SYSTEM_SPEC_CONTRACT / "check.zt"))
run("check", str(SYSTEM_SPEC_CONTRACT / "gen_python.zt"))
run("check", str(COMPOSITION_INVENTORY_CONTRACT / "schema.zt"))
run("check", str(COMPOSITION_INVENTORY_CONTRACT / "check.zt"))
run("check", str(COMPOSITION_INVENTORY_CONTRACT / "gen_python.zt"))
run("check", str(COMPONENT_SDK_RELEASE_CONTRACT / "schema.zt"))
run("check", str(COMPONENT_SDK_RELEASE_CONTRACT / "check.zt"))
run("check", str(COMPONENT_SDK_RELEASE_CONTRACT / "check-matrix.zt"))
run("check", str(COMPONENT_SDK_RELEASE_CONTRACT / "gen_python.zt"))
for contract in (SYSTEM_IMAGE_CLOSURE_CONTRACT, SYSTEM_TEST_RUN_CONTRACT):
    run("check", str(contract / "schema.zt"))
    run("check", str(contract / "check.zt"))
    run("check", str(contract / "gen_python.zt"))
# The negative-build-case module lives beside the closure contract it perturbs
# and shares its bindings, so it is type-checked here rather than getting its
# own generator entry.
run("check", str(SYSTEM_IMAGE_CLOSURE_CONTRACT / "check-negative.zt"))
# CP9's published matrix is a committed repository artifact, so it is checked
# unconditionally: there is no state in which it is legitimately absent, and a
# conditional check would let deleting it pass silently.
#
# Both halves are needed. The schema types every field as `Text`, so decoding
# proves shape only; `component_sdk.read_matrix` additionally binds the three
# files to one identity and enforces the closed vocabularies, the bounds, and the
# immutable-commit rule the contract declares.
matrix_path = ROOT / "sdk" / "compatibility-matrix.zti"
matrix_environment = os.environ.copy()
matrix_environment["SLIME_COMPONENT_SDK_MATRIX_PATH"] = str(matrix_path)
decoded_matrix = subprocess.run(
    [str(binary()), "run", str(COMPONENT_SDK_RELEASE_CONTRACT / "check-matrix.zt")],
    cwd=ROOT,
    env={**matrix_environment, "ZUTAI_STDLIB_ROOT": str(STDLIB)},
    check=False,
    text=True,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
)
if decoded_matrix.returncode != 0 or not decoded_matrix.stdout.startswith("#valid"):
    raise SystemExit(
        "published compatibility matrix did not decode as #valid:\n"
        + decoded_matrix.stdout
        + decoded_matrix.stderr
    )
_sys.path.insert(0, str(ROOT / "scripts" / "lib"))
import component_sdk  # noqa: E402

_matrix_table, _matrix_identity = component_sdk.read_matrix(matrix_path)
print(
    f"published compatibility matrix: {len(_matrix_table['rows'])} rows at "
    f"{_matrix_identity[:16]}"
)
run("check", str(DATA_FABRIC_PROFILE_CONTRACT / "schema.zt"))
run("check", str(DATA_FABRIC_PROFILE_CONTRACT / "check.zt"))
run("check", str(NORMALIZED_INTERFACE_SCHEMAS_CONTRACT / "schema.zt"))
run("check", str(NORMALIZED_INTERFACE_SCHEMAS_CONTRACT / "gen_rust.zt"))


for contract in (
    GENERATION_V2_CONTRACT,
    GENERATION_V3_CONTRACT,
    GENERATION_V4_CONTRACT,
    KERNEL_IMAGE_CONTRACT,
    KERNEL_IMAGE_V2_CONTRACT,
    COMPONENT_V2_CONTRACT,
    TARGET_PROFILE_CONTRACT,
    BOOTSTATE_CONTRACT,
    BOOTSTATE_TRACE_CONTRACT,
    RECOVERY_CONTRACT,
    STORE_DISK_CONTRACT,
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
SLOT_PIN_REASON_CHECK.main()

# Every contract version's schema must compile, named above or not. A planning
# pull request may land a schema before any generator, binding, or checker
# refers to it (`scripts/lib/pr_scope.py` INTERFACE), and this is the only
# gate that would otherwise never see it.
schemas = sorted((ROOT / "contracts").rglob("schema.zt"))
if not schemas:
    raise SystemExit("no contracts/**/schema.zt found")
for schema in schemas:
    run("check", str(schema))
print(f"{len(schemas)} contract schemas compiled")

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
    "Generation source/binary, kernel image v1/v2, target profile, BootState, "
    "BootState trace, recovery, block, component v1/v2, store, spawn, filesystem, "
    "link-device, network-service, powerbox, generation-management, transfer, "
    "fabric-graph, capability-transfer, fabric-stream, fabric-qos, fabric-time, "
    "fabric-call, fabric-operation, fabric-visibility, fabric-trace, "
    "component-spec, system-spec, component-sdk-release, system-image-closure, "
    "and system-test-run contracts passed"
)
