#!/usr/bin/env python3

"""RP1 exact-profile executable closure and deterministic artifact gate."""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import json
import os
import subprocess
from boot_contracts import (
    GENERATION_HEADER_IDENTITY_END,
    GENERATION_HEADER_IDENTITY_OFFSET,
    GENERATION_HEADER_OBJECT_COUNT_OFFSET,
    GENERATION_HEADER_OBJECT_OFFSET_OFFSET,
    GENERATION_HEADER_STRING_LEN_OFFSET,
    GENERATION_HEADER_STRING_OFFSET_OFFSET,
    FEATURE_AARCH64_BASELINE,
    FEATURE_AARCH64_GENERIC_TIMER,
    FEATURE_AARCH64_GICV2,
    FEATURE_AARCH64_GICV3,
    GENERATION_OBJECT,
    TARGET_PROFILES_BY_NAME,
    sha256,
)
from harness import ROOT, load_script
from release_trust import build_release
from zutai_cli import STDLIB, binary

BUILD_GENERATION = load_script("rpi5_artifact_builder", "build/build-generation.py")
CHECK_GENERATION = load_script("rpi5_artifact_generation_check", "check/check-generation.py")
ARCHITECTURE_CONTRACT = load_script(
    "rpi5_artifact_architecture_contract", "check/check-architecture-contract.py"
)
# The RP0 contract that names the demo's executable roles. Format 2 carries the
# transport family as data, so the runtime's name follows the selected transport
# (`ros.transportRuntime`) rather than a transport-specific field.
DEMO_FIXTURE = ROOT / "contracts" / "rpi5-ros2-demo" / "v2" / "fixtures" / "valid.zti"
RPI5_PROFILE_NAME = "aarch64-rpi5"
QEMU_PROFILE_NAME = "aarch64-qemu-virt"
X86_PROFILE_NAME = "x86_64-qemu-virtio"
NEUTRAL_RESOURCE = b"rpi5-architecture-neutral-resource-v1"
NODE_COMPONENTS = ("ros2-demo-publisher", "ros2-demo-subscriber")
TRANSPORT_COMPONENT = "slime-zenoh-profile-0"


def fail(message: str) -> None:
    raise SystemExit(f"rpi5 artifact check: {message}")


def project_fixture() -> dict:
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    process = subprocess.run(
        [str(binary()), "json", str(DEMO_FIXTURE)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        _sys.stderr.write(process.stdout)
        _sys.stderr.write(process.stderr)
        fail("RP0 fixture did not project to JSON")
    try:
        fixture = json.loads(process.stdout)
    except json.JSONDecodeError as error:
        fail(f"RP0 fixture JSON is invalid: {error}")
    if not isinstance(fixture, dict):
        fail("RP0 fixture is not a record")
    return fixture


def demo_artifact_names(fixture: dict) -> tuple[str, str, str]:
    try:
        target = fixture["targetProfile"]
        transport_runtime = fixture["ros"]["transportRuntime"]
        publisher = fixture["workload"]["publisher"]["component"]
        subscriber = fixture["workload"]["subscriber"]["component"]
    except (KeyError, TypeError) as error:
        fail(f"RP0 fixture omitted executable closure metadata: {error}")
    if target != RPI5_PROFILE_NAME:
        fail(f"RP0 fixture target {target!r} is not {RPI5_PROFILE_NAME!r}")
    names = (transport_runtime, publisher, subscriber)
    if names != (TRANSPORT_COMPONENT, *NODE_COMPONENTS) or len(set(names)) != len(names):
        fail("RP0 fixture transport/node executable names drifted")
    return names


def closure_manifest(profile_name: str, executable_names: tuple[str, ...]) -> dict:
    # B69: generation v5 splits v4's single `components` list into an executable
    # catalogue and the instances constructed from it, and renames
    # `bootstrapComponent`/`health.requiredComponents` to their instance
    # spellings. This manifest still used the v4 vocabulary, so
    # `build_generation` raised `KeyError: 'instances'` and the gate could not
    # reach a single assertion. `check-architecture-contract.py` was already
    # fixed for the same cutover and carries the same note; this synthesized
    # manifest was left behind.
    objects = [
        {"id": "init-image", "kind": "bootstrap"},
        {"id": "kernel", "kind": "kernel"},
        {"id": "neutral", "kind": "resource"},
    ]
    executables = [
        {"name": "init", "object": "init-image", "role": "init", "spawnBudget": 0}
    ]
    instances = [
        {
            "name": "init",
            "executable": "init",
            "owner": "root",
            "autostart": True,
            "dependencies": [],
            "health": "required",
            "bindings": [],
        }
    ]
    for name in executable_names:
        object_id = f"image:{name}"
        objects.append({"id": object_id, "kind": "component"})
        executables.append(
            {
                "name": name,
                "object": object_id,
                "role": "service" if name == TRANSPORT_COMPONENT else "application",
                "spawnBudget": 0,
            }
        )
        # Owned by `init`, not by root. Two reasons, and the second is a latent
        # validator bug this manifest is the first to expose:
        #
        # 1. It is the real topology. Root owns the bootstrap instance; every
        #    other component is spawned by it.
        # 2. `check-generation.py` encodes a root-owned instance as
        #    `(owner_kind=0, owner_index=0)` and then requires
        #    `owner_index != index` unconditionally, so a root-owned instance
        #    landing at index 0 is rejected as `BadInstanceOwner` even though
        #    `owner_index` is meaningless when `owner_kind` is 0. Instances are
        #    sorted by name, and every real fixture has a name sorting before
        #    `init` (`fabric-call-client`, `fabric-intruder`, ...), so `init` is
        #    never at index 0 there and the flaw stays hidden. Here `init` would
        #    be index 0. Recorded as part of B69 rather than worked around
        #    silently.
        instances.append(
            {
                "name": name,
                "executable": name,
                "owner": "init",
                "autostart": True,
                "dependencies": [],
                "health": "required",
                "bindings": [],
            }
        )
    return {
        "target": profile_name,
        # `check-generation.py`'s `BOOT_ACTIONS` is a closed set; this
        # synthesized closure is an ordinary product generation, so it must name
        # that action rather than invent one. Naming an unregistered action makes
        # every negative case below fail as `UnknownBootAction` before it can
        # reach the target-profile mismatch it is testing for.
        "bootAction": "product",
        "kernelObject": "kernel",
        "bootstrapInstance": "init",
        "objects": objects,
        "executables": executables,
        "instances": instances,
        "grants": [],
        "state": [],
        "health": {
            "bootAttempts": 2,
            "requiredInstances": ["init", *executable_names],
        },
    }


def closure_payloads(profile, executable_names: tuple[str, ...]) -> dict[str, bytes]:
    payloads = {
        "kernel": ARCHITECTURE_CONTRACT.kernel_image(profile),
        "init-image": ARCHITECTURE_CONTRACT.component_image(profile),
        "neutral": NEUTRAL_RESOURCE,
    }
    for name in executable_names:
        payloads[f"image:{name}"] = ARCHITECTURE_CONTRACT.component_image(profile)
    return payloads


def build_closure(profile, executable_names: tuple[str, ...]) -> bytes:
    return BUILD_GENERATION.build_generation(
        closure_manifest(profile.name, executable_names),
        closure_payloads(profile, executable_names),
        None,
        1,
        profile,
    )


def object_rows(generation: bytes) -> dict[str, tuple[int, bytes, bytes]]:
    object_count = int.from_bytes(
        generation[
            GENERATION_HEADER_OBJECT_COUNT_OFFSET : GENERATION_HEADER_OBJECT_COUNT_OFFSET + 4
        ],
        "little",
    )
    object_offset = int.from_bytes(
        generation[
            GENERATION_HEADER_OBJECT_OFFSET_OFFSET : GENERATION_HEADER_OBJECT_OFFSET_OFFSET + 8
        ],
        "little",
    )
    strings_offset = int.from_bytes(
        generation[
            GENERATION_HEADER_STRING_OFFSET_OFFSET : GENERATION_HEADER_STRING_OFFSET_OFFSET + 8
        ],
        "little",
    )
    strings_len = int.from_bytes(
        generation[
            GENERATION_HEADER_STRING_LEN_OFFSET : GENERATION_HEADER_STRING_LEN_OFFSET + 8
        ],
        "little",
    )
    rows = {}
    for index in range(object_count):
        name_offset, kind, payload_offset, payload_len, digest = GENERATION_OBJECT.unpack_from(
            generation, object_offset + index * GENERATION_OBJECT.size
        )
        name = CHECK_GENERATION.read_string(
            generation, strings_offset, strings_len, name_offset
        )
        payload = generation[payload_offset : payload_offset + payload_len]
        rows[name] = (kind, payload, digest)
    return rows


def expect_rejected(generation: bytes, marker: str) -> None:
    try:
        CHECK_GENERATION.check_generation(generation)
    except CHECK_GENERATION.CheckError as error:
        if marker not in str(error):
            fail(f"wrong rejection {error!s}; expected {marker}")
    else:
        fail(f"mismatched closure was admitted; expected {marker}")


def retarget_generation(
    source_profile,
    destination_profile,
    executable_names: tuple[str, ...],
    *,
    wrong_kernel: bool,
    wrong_component: str | None,
) -> bytes:
    payloads = closure_payloads(destination_profile, executable_names)
    if wrong_kernel:
        payloads["kernel"] = ARCHITECTURE_CONTRACT.kernel_image(source_profile)
    if wrong_component is not None:
        payloads[f"image:{wrong_component}"] = ARCHITECTURE_CONTRACT.component_image(
            source_profile
        )
    return BUILD_GENERATION.build_generation(
        closure_manifest(destination_profile.name, executable_names),
        payloads,
        None,
        1,
        destination_profile,
    )


def check_wrong_target_closures(executable_names: tuple[str, ...]) -> None:
    rpi5 = TARGET_PROFILES_BY_NAME[RPI5_PROFILE_NAME]
    for source_name in (X86_PROFILE_NAME, QEMU_PROFILE_NAME):
        source = TARGET_PROFILES_BY_NAME[source_name]
        expect_rejected(
            retarget_generation(
                source,
                rpi5,
                executable_names,
                wrong_kernel=True,
                wrong_component=None,
            ),
            "KernelTargetProfileMismatch",
        )
        for component in executable_names:
            expect_rejected(
                retarget_generation(
                    source,
                    rpi5,
                    executable_names,
                    wrong_kernel=False,
                    wrong_component=component,
                ),
                f"ComponentTargetProfileMismatch:{component}",
            )


def check_determinism_and_identity(executable_names: tuple[str, ...]) -> None:
    rpi5 = TARGET_PROFILES_BY_NAME[RPI5_PROFILE_NAME]
    qemu = TARGET_PROFILES_BY_NAME[QEMU_PROFILE_NAME]
    first = build_closure(rpi5, executable_names)
    second = build_closure(rpi5, executable_names)
    if first != second:
        fail("identical normalized RPi5 inputs produced different generations")
    first_store = BUILD_GENERATION.build_bootstore([first])
    second_store = BUILD_GENERATION.build_bootstore([second])
    if first_store != second_store:
        fail("identical normalized RPi5 inputs produced different boot stores")
    CHECK_GENERATION.check_bootstore(first_store)

    qemu_generation = build_closure(qemu, executable_names)
    if (
        first[GENERATION_HEADER_IDENTITY_OFFSET:GENERATION_HEADER_IDENTITY_END]
        == qemu_generation[GENERATION_HEADER_IDENTITY_OFFSET:GENERATION_HEADER_IDENTITY_END]
    ):
        fail("changing only the exact target did not change generation identity")
    if sha256(build_release(first, 1, key_paths=())) == sha256(
        build_release(qemu_generation, 1, key_paths=())
    ):
        fail("changing only the exact target did not change release identity")

    rpi5_rows = object_rows(first)
    qemu_rows = object_rows(qemu_generation)
    target_specific = ["kernel", "init-image", *(f"image:{name}" for name in executable_names)]
    for object_id in target_specific:
        _, rpi5_payload, rpi5_digest = rpi5_rows[object_id]
        _, qemu_payload, qemu_digest = qemu_rows[object_id]
        if rpi5_payload == qemu_payload or rpi5_digest == qemu_digest:
            fail(f"target-specific executable identity did not change: {object_id}")
    if rpi5_rows["neutral"][1:] != qemu_rows["neutral"][1:]:
        fail("architecture-neutral resource identity changed across targets")


def check_profile_isolation() -> None:
    root = _Path(os.environ.get("CARGO_TARGET_DIR") or ROOT / "target" / "components")
    qemu = BUILD_GENERATION.component_target_dir(
        root, TARGET_PROFILES_BY_NAME[QEMU_PROFILE_NAME], "generation-1"
    )
    rpi5 = BUILD_GENERATION.component_target_dir(
        root, TARGET_PROFILES_BY_NAME[RPI5_PROFILE_NAME], "generation-1"
    )
    if qemu == rpi5 or qemu.parent == rpi5.parent:
        fail("same-Cargo-target profiles share a component artifact directory")


def check_declared_profile_axes(fixture: dict) -> None:
    rpi5 = TARGET_PROFILES_BY_NAME[RPI5_PROFILE_NAME]
    qemu = TARGET_PROFILES_BY_NAME[QEMU_PROFILE_NAME]
    if rpi5.id == qemu.id or rpi5.name == qemu.name:
        fail("RPi5 and AArch64 QEMU profiles are not distinct")
    if rpi5.required_features != (
        FEATURE_AARCH64_BASELINE | FEATURE_AARCH64_GICV2 | FEATURE_AARCH64_GENERIC_TIMER
    ):
        fail("RPi5 profile does not declare its GICv2 board interrupt contract")
    if qemu.required_features != (
        FEATURE_AARCH64_BASELINE | FEATURE_AARCH64_GICV3 | FEATURE_AARCH64_GENERIC_TIMER
    ):
        fail("AArch64 QEMU profile does not declare its GICv3 machine contract")
    board = fixture.get("board")
    if not isinstance(board, dict):
        fail("RP0 fixture omitted its board contract")
    if board.get("interruptController") != "arm-gic-400-gicv2-from-device-tree":
        fail("RP0 board interrupt contract does not match the RPi5 target profile")
    if board.get("pageProfile") != "aarch64-4k":
        fail("RP0 board page profile does not match the RPi5 target profile")
    if rpi5.qemu_binary:
        fail("RPi5 profile unexpectedly names a QEMU launcher")
    if not qemu.qemu_binary:
        fail("AArch64 QEMU profile omitted its launcher")


def run() -> None:
    fixture = project_fixture()
    executable_names = demo_artifact_names(fixture)
    check_declared_profile_axes(fixture)
    check_profile_isolation()
    check_wrong_target_closures(executable_names)
    check_determinism_and_identity(executable_names)
    print("RPi5 target-qualified executable closure and artifacts passed")


if __name__ == "__main__":
    run()
