#!/usr/bin/env python3
"""M5.8 signed-release authorization and replay scenarios."""

from __future__ import annotations

import importlib.util
import os
import shutil
import struct
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BUILD = ROOT / "scripts" / "build-generation.py"
CHECK_PATH = ROOT / "scripts" / "check-generation.py"
RELEASE_TRUST_PATH = ROOT / "scripts" / "release_trust.py"
KERNEL = ROOT / "kernel" / "target" / "x86_64-unknown-none" / "debug" / "slime_os-kernel"
WORK = Path("/tmp/slime-os-release-trust")


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot load {path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


CHECK = load_module("check_generation", CHECK_PATH)
TRUST = load_module("release_trust", RELEASE_TRUST_PATH)


def run(arguments: list[str], *, environment: dict[str, str] | None = None) -> str:
    process = subprocess.run(
        arguments,
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    sys.stdout.write(process.stdout)
    if process.returncode != 0:
        raise SystemExit(f"command failed with {process.returncode}: {' '.join(arguments)}")
    return process.stdout


def build(name: str, **variables: str) -> Path:
    output = WORK / name
    shutil.rmtree(output, ignore_errors=True)
    environment = os.environ.copy()
    environment.update(variables)
    run([str(BUILD), str(KERNEL), str(output)], environment=environment)
    return output / "boot-store.bin"


def expect_error(name: str, expected: str, action) -> None:
    try:
        action()
    except CHECK.CheckError as error:
        if str(error) != expected:
            raise SystemExit(f"{name}: expected {expected}, got {error}") from error
    else:
        raise SystemExit(f"{name}: unexpectedly accepted")


def entry(image: bytes, index: int) -> tuple[int, int, int, int]:
    offset = CHECK.BOOTSTORE_DIRECTORY_OFFSET + CHECK.BOOTSTORE_HEADER.size + index * CHECK.BOOTSTORE_ENTRY.size
    _, generation_offset, generation_len, release_offset, release_len = CHECK.BOOTSTORE_ENTRY.unpack_from(image, offset)
    return generation_offset, generation_len, release_offset, release_len


def release_parts(image: bytes, index: int) -> tuple[bytes, bytes]:
    generation_offset, generation_len, release_offset, release_len = entry(image, index)
    return (
        image[generation_offset : generation_offset + generation_len],
        image[release_offset : release_offset + release_len],
    )


def release_by_sequence(image: bytes, sequence: int) -> tuple[bytes, bytes]:
    count = struct.unpack_from("<I", image, CHECK.BOOTSTORE_DIRECTORY_OFFSET + 24)[0]
    for index in range(count):
        generation, release = release_parts(image, index)
        if struct.unpack_from("<Q", release, 88)[0] == sequence:
            return generation, release
    raise SystemExit(f"missing release sequence {sequence}")


def test_release_rejections(image: bytes) -> None:
    generation, release = release_by_sequence(image, 2)

    one_signature = bytearray(release)
    struct.pack_into("<I", one_signature, 200, 1)
    one_signature[CHECK.RELEASE_HEADER_BYTES + CHECK.RELEASE_SIGNATURE_BYTES :] = bytes(
        CHECK.RELEASE_BYTES - CHECK.RELEASE_HEADER_BYTES - CHECK.RELEASE_SIGNATURE_BYTES
    )
    expect_error("below threshold", "BadReleaseSignatures", lambda: CHECK.check_release(bytes(one_signature), generation))

    missing = bytearray(release)
    struct.pack_into("<I", missing, 200, 0)
    missing[CHECK.RELEASE_HEADER_BYTES:] = bytes(CHECK.RELEASE_BYTES - CHECK.RELEASE_HEADER_BYTES)
    expect_error("missing signatures", "BadReleaseSignatures", lambda: CHECK.check_release(bytes(missing), generation))

    duplicate = bytearray(release)
    duplicate[CHECK.RELEASE_HEADER_BYTES + CHECK.RELEASE_SIGNATURE_BYTES : CHECK.RELEASE_HEADER_BYTES + 2 * CHECK.RELEASE_SIGNATURE_BYTES] = duplicate[
        CHECK.RELEASE_HEADER_BYTES : CHECK.RELEASE_HEADER_BYTES + CHECK.RELEASE_SIGNATURE_BYTES
    ]
    duplicate[CHECK.RELEASE_HEADER_BYTES + 2 * CHECK.RELEASE_SIGNATURE_BYTES :] = bytes(CHECK.RELEASE_BYTES - CHECK.RELEASE_HEADER_BYTES - 2 * CHECK.RELEASE_SIGNATURE_BYTES)
    expect_error("duplicate key", "DuplicateOrUnknownReleaseKey", lambda: CHECK.check_release(bytes(duplicate), generation))

    malformed = bytearray(release)
    malformed[CHECK.RELEASE_HEADER_BYTES + 32] ^= 0x80
    expect_error("malformed signature", "BadReleaseSignature", lambda: CHECK.check_release(bytes(malformed), generation))

    excessive = bytearray(release)
    struct.pack_into("<I", excessive, 200, CHECK.MAX_RELEASE_SIGNATURES + 1)
    expect_error("excessive signatures", "BadReleaseSignatures", lambda: CHECK.check_release(bytes(excessive), generation))

    wrong_target = bytearray(release)
    wrong_target[104] ^= 1
    expect_error("wrong target", "WrongReleaseTarget", lambda: CHECK.check_release(bytes(wrong_target), generation))

    expect_error("stale release", "StaleRelease", lambda: CHECK.check_release(release, generation, 2))

    unsigned = bytearray(image)
    count = struct.unpack_from("<I", image, CHECK.BOOTSTORE_DIRECTORY_OFFSET + 24)[0]
    release_offset = next(
        entry(image, index)[2]
        for index in range(count)
        if struct.unpack_from("<Q", image, entry(image, index)[2] + 88)[0] == 2
    )
    unsigned[release_offset : release_offset + CHECK.RELEASE_BYTES] = bytes(CHECK.RELEASE_BYTES)
    unsigned[CHECK.BOOTSTORE_DIRECTORY_OFFSET + 48 : CHECK.BOOTSTORE_DIRECTORY_OFFSET + 80] = bytes(32)
    unsigned[CHECK.BOOTSTORE_DIRECTORY_OFFSET + 48 : CHECK.BOOTSTORE_DIRECTORY_OFFSET + 80] = CHECK.bootstore_checksum(unsigned)
    expect_error("missing release", "BadReleaseMagic", lambda: CHECK.check_bootstore(bytes(unsigned)))


def ssh_rotation_payload(payload: bytes) -> bytes:
    return (
        b"SSHSIG"
        + TRUST.ssh_string(TRUST.SIGN_NAMESPACE.encode())
        + TRUST.ssh_string(b"")
        + TRUST.ssh_string(b"sha256")
        + TRUST.ssh_string(TRUST.sha256(payload))
    )


def build_rotation(
    current_keys: tuple[Path, ...],
    replacement_keys: tuple[Path, ...],
    previous_version: int,
    replacement_version: int,
) -> bytes:
    rotation = bytearray(TRUST.ROTATION_BYTES)
    rotation[:8] = TRUST.ROTATION_MAGIC
    struct.pack_into("<IIQ", rotation, 8, TRUST.ROTATION_VERSION, TRUST.ROTATION_HEADER_BYTES, 0)
    struct.pack_into(
        "<IIIIII",
        rotation,
        24,
        previous_version,
        replacement_version,
        2,
        len(replacement_keys),
        len(current_keys),
        len(replacement_keys),
    )
    replacement_public = tuple(TRUST.ssh_public_key(path) for path in replacement_keys)
    for index, public in enumerate(replacement_public):
        offset = TRUST.ROTATION_HEADER_BYTES + index * 32
        rotation[offset : offset + 32] = public
    previous_offset = TRUST.ROTATION_HEADER_BYTES + TRUST.MAX_TRUST_KEYS * 32
    replacement_offset = previous_offset + TRUST.MAX_RELEASE_SIGNATURES * TRUST.RELEASE_SIGNATURE_BYTES
    payload = bytes(rotation[:previous_offset])
    for base, paths in ((previous_offset, current_keys), (replacement_offset, replacement_keys)):
        entries = sorted((TRUST.sha256(TRUST.ssh_public_key(path)), TRUST.ssh_signature(path, payload)) for path in paths)
        for index, (key_id, signature) in enumerate(entries):
            offset = base + index * TRUST.RELEASE_SIGNATURE_BYTES
            rotation[offset : offset + 32] = key_id
            rotation[offset + 32 : offset + TRUST.RELEASE_SIGNATURE_BYTES] = signature
    return bytes(rotation)


def verify_rotation(rotation: bytes, *, current_version: int = 1) -> None:
    if len(rotation) != TRUST.ROTATION_BYTES or rotation[:8] != TRUST.ROTATION_MAGIC:
        raise CHECK.CheckError("BadRotation")
    version, header, flags = struct.unpack_from("<IIQ", rotation, 8)
    previous, replacement, threshold, key_count, previous_count, replacement_count = struct.unpack_from("<IIIIII", rotation, 24)
    if (
        version != TRUST.ROTATION_VERSION
        or header != TRUST.ROTATION_HEADER_BYTES
        or flags != 0
        or previous != current_version
        or replacement != current_version + 1
        or threshold == 0
        or threshold > key_count
        or key_count > TRUST.MAX_TRUST_KEYS
        or previous_count < 2
        or replacement_count < threshold
        or previous_count > TRUST.MAX_RELEASE_SIGNATURES
        or replacement_count > TRUST.MAX_RELEASE_SIGNATURES
        or any(rotation[48:TRUST.ROTATION_HEADER_BYTES])
    ):
        raise CHECK.CheckError("BadRotation")
    replacement_keys = []
    for index in range(key_count):
        offset = TRUST.ROTATION_HEADER_BYTES + index * 32
        replacement_keys.append(rotation[offset : offset + 32])
    if any(not key for key in replacement_keys) or len(set(replacement_keys)) != len(replacement_keys):
        raise CHECK.CheckError("BadRotation")
    if any(rotation[TRUST.ROTATION_HEADER_BYTES + key_count * 32 : TRUST.ROTATION_HEADER_BYTES + TRUST.MAX_TRUST_KEYS * 32]):
        raise CHECK.CheckError("BadRotation")
    current_by_id = {TRUST.sha256(key): key for key in TRUST.initial_public_keys()}
    replacement_by_id = {TRUST.sha256(key): key for key in replacement_keys}
    previous_offset = TRUST.ROTATION_HEADER_BYTES + TRUST.MAX_TRUST_KEYS * 32
    replacement_offset = previous_offset + TRUST.MAX_RELEASE_SIGNATURES * TRUST.RELEASE_SIGNATURE_BYTES
    payload = rotation[:previous_offset]
    for name, base, count, keys in (
        ("previous", previous_offset, previous_count, current_by_id),
        ("replacement", replacement_offset, replacement_count, replacement_by_id),
    ):
        previous_key_id = bytes(32)
        for index in range(count):
            offset = base + index * TRUST.RELEASE_SIGNATURE_BYTES
            key_id = rotation[offset : offset + 32]
            signature = rotation[offset + 32 : offset + TRUST.RELEASE_SIGNATURE_BYTES]
            if key_id <= previous_key_id or key_id not in keys:
                raise CHECK.CheckError(f"BadRotation{name}")
            process = subprocess.run(
                [
                    "cargo", "run", "--quiet", "--manifest-path", str(ROOT / "boot-contracts" / "Cargo.toml"),
                    "--features", "release-crypto", "--example", "verify_release", "--",
                    "signature",
                    keys[key_id].hex(), ssh_rotation_payload(payload).hex(), signature.hex(),
                ],
                cwd=ROOT,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            if process.returncode != 0:
                raise CHECK.CheckError(f"BadRotation{name}")
            previous_key_id = key_id
    if any(rotation[replacement_offset + replacement_count * TRUST.RELEASE_SIGNATURE_BYTES :]):
        raise CHECK.CheckError("BadRotation")


def test_rotation() -> None:
    valid = build_rotation(TRUST.KEY_PATHS[:2], TRUST.KEY_PATHS, 1, 2)
    rotation_path = WORK / "valid-rotation.bin"
    rotation_path.write_bytes(valid)
    run(
        [
            "cargo", "run", "--quiet", "--manifest-path", str(ROOT / "boot-contracts" / "Cargo.toml"),
            "--features", "release-crypto", "--example", "verify_release", "--",
            "rotation", str(rotation_path),
        ]
    )
    verify_rotation(valid)

    skipped = build_rotation(TRUST.KEY_PATHS[:2], TRUST.KEY_PATHS, 1, 3)
    expect_error("rotation version skip", "BadRotation", lambda: verify_rotation(skipped))

    no_previous_continuity = build_rotation(TRUST.KEY_PATHS[:1], TRUST.KEY_PATHS, 1, 2)
    expect_error("rotation previous continuity", "BadRotation", lambda: verify_rotation(no_previous_continuity))

    no_replacement_continuity = build_rotation(TRUST.KEY_PATHS[:2], TRUST.KEY_PATHS[:1], 1, 2)
    expect_error("rotation replacement continuity", "BadRotation", lambda: verify_rotation(no_replacement_continuity))


def main() -> None:
    image_path = build("authorized")
    image = image_path.read_bytes()
    CHECK.check_bootstore(image)
    test_release_rejections(image)
    test_rotation()

    pending_path = build("pending", SLIME_PENDING_GENERATION="1", SLIME_PENDING_ATTEMPTS="1")
    pending = CHECK.check_bootstore(pending_path.read_bytes())
    if pending["state"]["accepted_release_sequence"] != 1:
        raise SystemExit("staging advanced accepted release sequence")

    stale_path = build(
        "stale-pending",
        SLIME_PENDING_GENERATION="1",
        SLIME_PENDING_ATTEMPTS="1",
        SLIME_PENDING_RELEASE_SEQUENCE="1",
    )
    expect_error("stale pending bootstore", "StalePendingRelease", lambda: CHECK.check_bootstore(stale_path.read_bytes()))

    failed_path = build("failed-pending", SLIME_PENDING_GENERATION="1", SLIME_PENDING_ATTEMPTS="0")
    failed = CHECK.check_bootstore(failed_path.read_bytes())
    if failed["state"]["accepted_release_sequence"] != 1 or failed["selected"]["identity"] != failed["state"]["known_good"]:
        raise SystemExit("failed pending generation changed accepted sequence or local known-good selection")

    promoted_path = build("promoted", SLIME_ACCEPTED_RELEASE_SEQUENCE="2")
    promoted = CHECK.check_bootstore(promoted_path.read_bytes())
    identities = {generation["identity"] for generation in promoted["generations"]}
    if promoted["state"]["known_good"] not in identities or len(identities) < 2:
        raise SystemExit("promotion invalidated retained local rollback generation")

    print("release trust check: signed staging, replay, rotation, rollback, and promotion passed")


if __name__ == "__main__":
    main()
