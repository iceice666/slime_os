#!/usr/bin/env python3

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import json
import os
import struct
import subprocess

from boot_contracts import (
    COMPONENT_DEFAULT_STACK_BYTES,
    COMPONENT_IMAGE_HEADER,
    COMPONENT_IMAGE_KERNEL_ABI,
    COMPONENT_IMAGE_MAGIC,
    COMPONENT_IMAGE_SEGMENT,
    COMPONENT_IMAGE_VERSION,
    COMPONENT_SEGMENT_FLAG_EXEC,
    GENERATION_HEADER_OBJECT_COUNT_OFFSET,
    GENERATION_HEADER_OBJECT_OFFSET_OFFSET,
    GENERATION_HEADER_STRING_OFFSET_OFFSET,
    GENERATION_OBJECT,
    KERNEL_ABI_VERSION,
    KERNEL_HEADER,
    KERNEL_LEGACY_HEADER_LEN,
    KERNEL_LEGACY_MAGIC,
    KERNEL_LEGACY_VERSION,
    KERNEL_MAGIC,
    KERNEL_SEGMENT,
    KERNEL_VERSION,
    SEGMENT_EXEC,
    TARGET_PROFILE_MAX_NAME_BYTES,
    TARGET_PROFILES,
    TARGET_PROFILES_BY_NAME,
    sha256,
)
from harness import GENERATION_FIXTURES, ROOT, load_script
from release_trust import build_release
from zutai_cli import STDLIB, binary

BUILD_GENERATION = load_script("architecture_contract_builder", "build/build-generation.py")
CHECK_GENERATION = load_script("architecture_contract_generation_check", "check/check-generation.py")

EXPECTED_PROFILES = {
    "x86_64-qemu-virtio",
    "aarch64-qemu-virt",
    "aarch64-rpi5",
    "riscv64-qemu-virt",
}
UNKNOWN_TARGET = "unknown-architecture-profile"
NEUTRAL_RESOURCE = b"architecture-neutral-resource-v1"
MANIFEST = GENERATION_FIXTURES / "valid.zti"


def fail(message: str) -> None:
    raise SystemExit(f"architecture contract check: {message}")


def require_command(arguments: list[str], failure: str, *, cwd: _Path = ROOT) -> None:
    process = subprocess.run(
        arguments,
        cwd=cwd,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        _sys.stderr.write(process.stdout)
        _sys.stderr.write(process.stderr)
        fail(failure)
    if process.stdout:
        print(process.stdout, end="")


def check_profiles() -> None:
    ids = [profile.id for profile in TARGET_PROFILES]
    names = [profile.name for profile in TARGET_PROFILES]
    if len(ids) != len(set(ids)):
        fail("target profile ids are not unique")
    if len(names) != len(set(names)):
        fail("target profile names are not unique")
    for profile in TARGET_PROFILES:
        name_bytes = len(profile.name.encode("utf-8"))
        if name_bytes == 0:
            fail(f"target profile {profile.id} has an empty name")
        if name_bytes > TARGET_PROFILE_MAX_NAME_BYTES:
            fail(
                f"target profile {profile.name!r} exceeds "
                f"{TARGET_PROFILE_MAX_NAME_BYTES} UTF-8 bytes"
            )
    missing = EXPECTED_PROFILES.difference(names)
    if missing:
        fail(f"required target profiles are missing: {', '.join(sorted(missing))}")


def check_unknown_target() -> None:
    if TARGET_PROFILES_BY_NAME.get(UNKNOWN_TARGET) is not None:
        fail("an unknown target name resolved to an admitted profile")


def check_cross_profile_rejection() -> None:
    same_isa_pairs = 0
    for left in TARGET_PROFILES:
        for right in TARGET_PROFILES:
            if left.id >= right.id:
                continue
            same_qualification = (
                left.architecture == right.architecture
                and left.abi == right.abi
                and left.page_profile == right.page_profile
            )
            if not same_qualification:
                continue
            same_isa_pairs += 1
            try:
                CHECK_GENERATION.check_component_image(
                    component_image(left), right, f"{left.name}-under-{right.name}"
                )
            except CHECK_GENERATION.CheckError:
                pass
            else:
                fail(f"same-ISA profile {right.name!r} admitted {left.name!r} image")
    if same_isa_pairs == 0:
        fail("no same-ISA profiles exercise exact profile-id separation")



def kernel_image(profile) -> bytes:
    payload_offset = KERNEL_HEADER.size + KERNEL_SEGMENT.size
    payload = b"\x90" * 8
    header = KERNEL_HEADER.pack(
        KERNEL_MAGIC,
        KERNEL_VERSION,
        KERNEL_HEADER.size,
        KERNEL_ABI_VERSION,
        0,
        profile.architecture,
        profile.abi,
        profile.page_profile,
        profile.id,
        profile.required_features,
        profile.kernel_preferred_base,
        0,
        1,
        0,
        payload_offset,
        payload_offset + len(payload),
    )
    segment = KERNEL_SEGMENT.pack(0, len(payload), payload_offset, len(payload), SEGMENT_EXEC, 0)
    return header + segment + payload


def component_image(profile) -> bytes:
    payload = b"\x90" * 8
    header = COMPONENT_IMAGE_HEADER.pack(
        COMPONENT_IMAGE_MAGIC,
        COMPONENT_IMAGE_VERSION,
        COMPONENT_IMAGE_HEADER.size,
        COMPONENT_IMAGE_KERNEL_ABI,
        profile.architecture,
        profile.abi,
        profile.page_profile,
        0,
        1,
        0,
        COMPONENT_DEFAULT_STACK_BYTES,
        profile.id,
        profile.required_features,
    )
    segment = COMPONENT_IMAGE_SEGMENT.pack(0, len(payload), 0, len(payload), COMPONENT_SEGMENT_FLAG_EXEC, 0)
    return header + segment + payload


def target_manifest(name: str) -> dict:
    return {
        "target": name,
        "bootAction": "graph",
        "kernelObject": "kernel",
        "bootstrapInstance": "init",
        "objects": [
            {"id": "init-image", "kind": "bootstrap"},
            {"id": "kernel", "kind": "kernel"},
            {"id": "neutral", "kind": "resource"},
        ],
        # v5 splits the v4 `components` list into an executable catalogue and
        # the instances constructed from it. This manifest still used the v4
        # key, so the builder raised `KeyError: 'instances'` and this gate
        # could not run at all -- the cutover moved every real fixture and
        # left the one synthesized here behind.
        "executables": [
            {"name": "init", "object": "init-image", "role": "init", "spawnBudget": 0}
        ],
        "instances": [
            {
                "name": "init",
                "executable": "init",
                "owner": "root",
                "autostart": True,
                "dependencies": [],
                "health": "required",
                "bindings": [],
            }
        ],
        "grants": [],
        "state": [],
        "health": {"bootAttempts": 2, "requiredInstances": ["init"]},
    }


def object_payload(generation: bytes, object_id: str) -> bytes:
    object_count = struct.unpack_from(
        "<I", generation, GENERATION_HEADER_OBJECT_COUNT_OFFSET
    )[0]
    object_offset = struct.unpack_from(
        "<Q", generation, GENERATION_HEADER_OBJECT_OFFSET_OFFSET
    )[0]
    string_offset = struct.unpack_from(
        "<Q", generation, GENERATION_HEADER_STRING_OFFSET_OFFSET
    )[0]
    for index in range(object_count):
        name_offset, _kind, payload_offset, payload_len, _digest = GENERATION_OBJECT.unpack_from(
            generation, object_offset + index * GENERATION_OBJECT.size
        )
        name_len = struct.unpack_from("<H", generation, string_offset + name_offset)[0]
        name_start = string_offset + name_offset + 2
        name = generation[name_start : name_start + name_len].decode()
        if name == object_id:
            return generation[payload_offset : payload_offset + payload_len]
    fail(f"generation omitted object {object_id!r}")


def check_target_identity_and_neutral_resources() -> None:
    profiles = [
        TARGET_PROFILES_BY_NAME["aarch64-qemu-virt"],
        TARGET_PROFILES_BY_NAME["aarch64-rpi5"],
    ]
    generations = []
    releases = []
    resources = []
    for profile in profiles:
        payloads = {
            "kernel": kernel_image(profile),
            "init-image": component_image(profile),
            "neutral": NEUTRAL_RESOURCE,
        }
        generation = BUILD_GENERATION.build_generation(
            target_manifest(profile.name), payloads, None, 1, profile
        )
        generations.append(generation)
        releases.append(build_release(generation, 1, key_paths=()))
        resources.append(object_payload(generation, "neutral"))
    if generations[0][24:56] == generations[1][24:56]:
        fail("changing only the exact target did not change generation identity")
    if sha256(releases[0]) == sha256(releases[1]):
        fail("changing only the exact target did not change release identity")
    if resources != [NEUTRAL_RESOURCE, NEUTRAL_RESOURCE]:
        fail("architecture-neutral resource bytes changed across target builds")

def check_generated_bindings() -> None:
    require_command(
        [_sys.executable, "scripts/generate/generate-boot-bindings.py", "--check"],
        "generated boot bindings are not in sync with their contracts",
    )
    require_command(
        [_sys.executable, "scripts/generate/generate-component-bindings.py", "--check"],
        "generated component bindings are not in sync with their contract",
    )


def check_rollback_window() -> None:
    if KERNEL_LEGACY_MAGIC != b"SLIMEKRN":
        fail("kernel-image v1 legacy magic is not retained")
    if KERNEL_LEGACY_VERSION != 1:
        fail("kernel-image v1 legacy version is not retained")
    if KERNEL_LEGACY_HEADER_LEN != 64:
        fail("kernel-image v1 legacy header length is not retained")


def manifest_target() -> str:
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    process = subprocess.run(
        [str(binary()), "json", str(MANIFEST)],
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
        fail("generation manifest did not project to JSON")
    try:
        value = json.loads(process.stdout)
    except json.JSONDecodeError as error:
        fail(f"generation manifest JSON is invalid: {error}")
    if not isinstance(value, dict):
        fail("generation manifest JSON is not a record")
    target = value.get("target")
    if not isinstance(target, str):
        fail("generation manifest target is not text")
    return target


def check_manifest_target() -> None:
    target = manifest_target()
    if target not in TARGET_PROFILES_BY_NAME:
        fail(f"generation manifest names unknown target {target!r}")


def check_rust_admission() -> None:
    require_command(
        ["cargo", "test", "-p", "boot-contracts", "--lib"],
        "host Rust target and executable-image admission tests failed",
        cwd=ROOT / "boot-contracts",
    )


def run() -> None:
    check_profiles()
    check_unknown_target()
    check_cross_profile_rejection()
    check_target_identity_and_neutral_resources()
    check_generated_bindings()
    check_rollback_window()
    check_manifest_target()
    check_rust_admission()
    print("Architecture target and executable-artifact contracts passed")


if __name__ == "__main__":
    run()
