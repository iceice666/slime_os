#!/usr/bin/env python3
"""M5.8 signed-release authorization and replay scenarios."""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import os
import shutil
import struct
import subprocess
import sys
from pathlib import Path

from harness import ROOT, load_script

BUILD = ROOT / "scripts" / "build" / "build-generation.py"
BUILD_COMPONENT = ROOT / "scripts" / "build" / "build-c-component.py"
WORK = Path("/tmp/slime-os-release-trust")

CHECK = load_script("check_generation", "check/check-generation.py")
BUILD_GENERATION = load_script("release_trust_builder", "build/build-generation.py")
TRUST = load_script("release_trust", "lib/release_trust.py")
# The generated Zutai constants, loaded directly rather than re-exported through
# `release_trust`: that module only imports the names its own body uses, and
# widening it to forward these would be an unused-import the linter is right to
# flag.
CONTRACTS = load_script("boot_contracts", "lib/boot_contracts.py")


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


def build(name: str) -> tuple[Path, Path]:
    output = WORK / name
    shutil.rmtree(output, ignore_errors=True)
    WORK.mkdir(parents=True, exist_ok=True)
    slisp = WORK / "slisp.elf"
    run(
        [
            sys.executable,
            str(BUILD_COMPONENT),
            str(ROOT / "components" / "slisp" / "slisp.c"),
            str(ROOT / "components" / "slisp" / "main.c"),
            str(slisp),
        ]
    )
    environment = os.environ.copy()
    environment["SLIME_TARGET_PROFILE"] = "aarch64-sel4-qemu-virt"
    run(
        [
            str(BUILD),
            "--external-component",
            f"slisp-external={slisp}",
            str(output),
        ],
        environment=environment,
    )
    return output / "generation.bin", output / "boot-store.bin"


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

def variant_generation(generation: bytes, number: int) -> bytes:
    variant = bytearray(generation)
    struct.pack_into("<Q", variant, 56, number)
    variant[24:56] = bytes(32)
    variant[24:56] = CONTRACTS.generation_identity(variant)
    return bytes(variant)


def bootstore(generations: list[bytes], **settings: str) -> bytes:
    names = (
        "SLIME_PENDING_RELEASE_SEQUENCE",
        "SLIME_KNOWN_GOOD_FIRST",
        "SLIME_PENDING_GENERATION",
        "SLIME_PENDING_ATTEMPTS",
        "SLIME_ACCEPTED_RELEASE_SEQUENCE",
    )
    previous = {name: os.environ.get(name) for name in names}
    try:
        for name in names:
            os.environ.pop(name, None)
        os.environ.update(settings)
        return BUILD_GENERATION.build_bootstore(generations)
    finally:
        for name, value in previous.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value


def test_durable_bootstores(generation: bytes) -> None:
    first = variant_generation(generation, 1)
    second = variant_generation(generation, 2)
    pending = CHECK.check_bootstore(
        bootstore(
            [first, second],
            SLIME_PENDING_GENERATION="1",
            SLIME_PENDING_ATTEMPTS="3",
            SLIME_ACCEPTED_RELEASE_SEQUENCE="1",
        )
    )["state"]
    if pending["known_good"] != first[24:56] or pending["pending"] != second[24:56]:
        raise SystemExit("pending store did not retain known-good and candidate identities")
    if pending["accepted_release_sequence"] != 1 or pending["remaining_attempts"] != 3:
        raise SystemExit("staging changed accepted sequence or pending attempt budget")

    stale = bootstore(
        [first, second],
        SLIME_PENDING_GENERATION="1",
        SLIME_PENDING_RELEASE_SEQUENCE="1",
        SLIME_ACCEPTED_RELEASE_SEQUENCE="1",
    )
    expect_error("stale pending release", "StalePendingRelease", lambda: CHECK.check_bootstore(stale))

    exhausted = CHECK.check_bootstore(
        bootstore(
            [first, second],
            SLIME_PENDING_GENERATION="1",
            SLIME_PENDING_ATTEMPTS="0",
            SLIME_ACCEPTED_RELEASE_SEQUENCE="1",
        )
    )["state"]
    if exhausted["known_good"] != first[24:56] or exhausted["pending"] != second[24:56]:
        raise SystemExit("exhausted pending store lost retained identities")

    promoted = CHECK.check_bootstore(bootstore([first, second]))
    if promoted["state"]["known_good"] != second[24:56] or promoted["state"]["pending"] is not None:
        raise SystemExit("promoted store did not select and clear the candidate")
    directory = {item["identity"]: item["release_sequence"] for item in promoted["generations"]}
    if directory != {first[24:56]: 1, second[24:56]: 2}:
        raise SystemExit("boot-store directory did not bind generations to releases")

    conflicting = bytearray(bootstore([first, second]))
    slot_b = bytearray(conflicting[CHECK.BOOTSTATE_SLOT_BYTES : 2 * CHECK.BOOTSTATE_SLOT_BYTES])
    struct.pack_into("<Q", slot_b, 24, 2)
    slot_b[CHECK.BOOTSTATE_KNOWN_GOOD_OFFSET] ^= 1
    slot_b[CHECK.BOOTSTATE_CHECKSUM_OFFSET : CHECK.BOOTSTATE_CHECKSUM_END] = CONTRACTS.bootstate_checksum(slot_b)
    conflicting[CHECK.BOOTSTATE_SLOT_BYTES : 2 * CHECK.BOOTSTATE_SLOT_BYTES] = slot_b
    expect_error(
        "conflicting BootState slots",
        "ConflictingBootStateSlots",
        lambda: CHECK.check_bootstore(bytes(conflicting)),
    )


def test_release_rejections(image: bytes) -> None:
    generation, release = release_by_sequence(image, 1)

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

    expect_error("stale release", "StaleRelease", lambda: CHECK.check_release(release, generation, 1))

    unsigned = bytearray(release)
    unsigned[:8] = bytes(8)
    expect_error("missing release", "BadReleaseMagic", lambda: CHECK.check_release(bytes(unsigned), generation))


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
    rotation = bytearray(CONTRACTS.ROTATION_BYTES)
    rotation[:8] = CONTRACTS.ROTATION_MAGIC
    struct.pack_into("<IIQ", rotation, 8, CONTRACTS.ROTATION_VERSION, CONTRACTS.ROTATION_HEADER_BYTES, 0)
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
        offset = CONTRACTS.ROTATION_HEADER_BYTES + index * 32
        rotation[offset : offset + 32] = public
    previous_offset = CONTRACTS.ROTATION_HEADER_BYTES + CONTRACTS.MAX_TRUST_KEYS * 32
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
    if len(rotation) != CONTRACTS.ROTATION_BYTES or rotation[:8] != CONTRACTS.ROTATION_MAGIC:
        raise CHECK.CheckError("BadRotation")
    version, header, flags = struct.unpack_from("<IIQ", rotation, 8)
    previous, replacement, threshold, key_count, previous_count, replacement_count = struct.unpack_from("<IIIIII", rotation, 24)
    if (
        version != CONTRACTS.ROTATION_VERSION
        or header != CONTRACTS.ROTATION_HEADER_BYTES
        or flags != 0
        or previous != current_version
        or replacement != current_version + 1
        or threshold == 0
        or threshold > key_count
        or key_count > CONTRACTS.MAX_TRUST_KEYS
        or previous_count < 2
        or replacement_count < threshold
        or previous_count > TRUST.MAX_RELEASE_SIGNATURES
        or replacement_count > TRUST.MAX_RELEASE_SIGNATURES
        or any(rotation[48:CONTRACTS.ROTATION_HEADER_BYTES])
    ):
        raise CHECK.CheckError("BadRotation")
    replacement_keys = []
    for index in range(key_count):
        offset = CONTRACTS.ROTATION_HEADER_BYTES + index * 32
        replacement_keys.append(rotation[offset : offset + 32])
    if any(not key for key in replacement_keys) or len(set(replacement_keys)) != len(replacement_keys):
        raise CHECK.CheckError("BadRotation")
    if any(rotation[CONTRACTS.ROTATION_HEADER_BYTES + key_count * 32 : CONTRACTS.ROTATION_HEADER_BYTES + CONTRACTS.MAX_TRUST_KEYS * 32]):
        raise CHECK.CheckError("BadRotation")
    current_by_id = {TRUST.sha256(key): key for key in TRUST.initial_public_keys()}
    replacement_by_id = {TRUST.sha256(key): key for key in replacement_keys}
    previous_offset = CONTRACTS.ROTATION_HEADER_BYTES + CONTRACTS.MAX_TRUST_KEYS * 32
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


def rust_rotation(rotation: bytes, name: str) -> subprocess.CompletedProcess[str]:
    """Run `apply_rotation` over one rotation blob and return the outcome.

    The Python `verify_rotation` below reimplements the same rules, so on its own
    it can only prove the *fixture* is malformed — not that `release.rs` refuses
    it. Every refusal therefore goes through both.
    """
    path = WORK / f"rotation-{name}.bin"
    path.write_bytes(rotation)
    return subprocess.run(
        [
            "cargo", "run", "--quiet", "--manifest-path", str(ROOT / "boot-contracts" / "Cargo.toml"),
            "--features", "release-crypto", "--example", "verify_release", "--",
            "rotation", str(path),
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )


def expect_rust_rotation_refused(name: str, rotation: bytes) -> None:
    """`apply_rotation` must refuse this blob, not merely the Python mirror.

    Without this the three continuity cases below asserted only against
    `verify_rotation`, so deleting the replacement-version check from
    `release.rs` left the whole gate green.
    """
    outcome = rust_rotation(rotation, name)
    if outcome.returncode == 0:
        raise CHECK.CheckError(f"apply_rotation accepted {name}")
    if "rotation verification failed" not in outcome.stderr:
        detail = outcome.stderr.strip() or outcome.stdout.strip() or f"exit {outcome.returncode}"
        raise SystemExit(f"apply_rotation infrastructure/build failure for {name}: {detail}")


def test_rotation() -> None:
    valid = build_rotation(TRUST.KEY_PATHS[:2], TRUST.KEY_PATHS, 1, 2)
    if rust_rotation(valid, "valid").returncode != 0:
        raise CHECK.CheckError("apply_rotation refused a well-formed rotation")
    verify_rotation(valid)

    skipped = build_rotation(TRUST.KEY_PATHS[:2], TRUST.KEY_PATHS, 1, 3)
    expect_error("rotation version skip", "BadRotation", lambda: verify_rotation(skipped))
    expect_rust_rotation_refused("version-skip", skipped)

    no_previous_continuity = build_rotation(TRUST.KEY_PATHS[:1], TRUST.KEY_PATHS, 1, 2)
    expect_error("rotation previous continuity", "BadRotation", lambda: verify_rotation(no_previous_continuity))
    expect_rust_rotation_refused("previous-continuity", no_previous_continuity)

    no_replacement_continuity = build_rotation(TRUST.KEY_PATHS[:2], TRUST.KEY_PATHS[:1], 1, 2)
    expect_error("rotation replacement continuity", "BadRotation", lambda: verify_rotation(no_replacement_continuity))
    expect_rust_rotation_refused("replacement-continuity", no_replacement_continuity)

    # A rotation naming a `previous_version` other than the root it is applied
    # against must be refused, or an old rotation could be replayed to advance
    # the trust root a second time.
    #
    # `(2, 2)` and not `(2, 3)`: the replacement version must stay at
    # `current.version + 1` so the *replacement* branch does not fire first and
    # mask the one under test. The two continuity cases above vary the signature
    # counts instead, so neither reaches this branch — deleting
    # `previous_version != current.version` from `release.rs` left the whole gate
    # green until this fixture existed.
    stale_previous = build_rotation(TRUST.KEY_PATHS[:2], TRUST.KEY_PATHS, 2, 2)
    expect_rust_rotation_refused("stale-previous", stale_previous)


def main() -> None:
    generation_path, image_path = build("authorized")
    image = image_path.read_bytes()
    generation, release = release_by_sequence(image, 1)
    if CHECK.check_release(release, generation) != 1:
        raise SystemExit("release verifier did not retain sequence one")
    test_release_rejections(image)
    test_durable_bootstores(generation_path.read_bytes())
    test_rotation()

    print(
        "release trust check: signed release, pending/stale/exhausted/promotion/directory/conflict "
        "stores, threshold refusals, replay refusal, and trust-root rotation passed"
    )


if __name__ == "__main__":
    main()
