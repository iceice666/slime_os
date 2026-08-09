from __future__ import annotations

import base64
import hashlib
import struct
import subprocess
from pathlib import Path

from boot_contracts import (
    MAX_RELEASE_SIGNATURES,
    MAX_TARGET_BYTES,
    RELEASE_BYTES,
    RELEASE_HEADER_AUTHORITY_MANIFEST_END,
    RELEASE_HEADER_AUTHORITY_MANIFEST_OFFSET,
    RELEASE_HEADER_BYTES,
    RELEASE_HEADER_GENERATION_IDENTITY_END,
    RELEASE_HEADER_GENERATION_IDENTITY_OFFSET,
    RELEASE_HEADER_BOOT_BUNDLE_IDENTITY_END,
    RELEASE_HEADER_BOOT_BUNDLE_IDENTITY_OFFSET,
    RELEASE_HEADER_PARENT_IDENTITY_END,
    RELEASE_HEADER_PARENT_IDENTITY_OFFSET,
    RELEASE_HEADER_RELEASE_SEQUENCE_OFFSET,
    RELEASE_HEADER_SIGNATURE_COUNT_OFFSET,
    RELEASE_HEADER_TARGET_OFFSET,
    RELEASE_MAGIC,
    RELEASE_SIGNATURE_BYTES,
    RELEASE_SIGNATURE_KEY_ID_END,
    RELEASE_SIGNATURE_KEY_ID_OFFSET,
    RELEASE_SIGNATURE_SIGNATURE_END,
    RELEASE_SIGNATURE_SIGNATURE_OFFSET,
    RELEASE_VERSION,
    SIGN_NAMESPACE,
    GENERATION_EXECUTABLE,
    GENERATION_GRANT,
    GENERATION_HEADER_EXECUTABLE_COUNT_OFFSET,
    GENERATION_HEADER_EXECUTABLE_OFFSET_OFFSET,
    GENERATION_HEADER_GRANT_COUNT_OFFSET,
    GENERATION_HEADER_GRANT_OFFSET_OFFSET,
    GENERATION_HEADER_INSTANCE_COUNT_OFFSET,
    GENERATION_HEADER_INSTANCE_OFFSET_OFFSET,
    GENERATION_HEADER_STRING_OFFSET_OFFSET,
    GENERATION_INSTANCE,
)

from harness import ROOT

KEY_DIR = ROOT / "contracts" / "release" / "v1" / "test-keys"
KEY_PATHS = tuple(KEY_DIR / f"key{index}" for index in range(1, 4))


def sha256(data: bytes) -> bytes:
    return hashlib.sha256(data).digest()


def ssh_public_key(path: Path) -> bytes:
    public = subprocess.run(
        ["ssh-keygen", "-y", "-f", str(path)], check=True, text=True, stdout=subprocess.PIPE
    ).stdout.split()
    blob = base64.b64decode(public[1])
    algorithm_len = struct.unpack_from(">I", blob, 0)[0]
    offset = 4 + algorithm_len
    key_len = struct.unpack_from(">I", blob, offset)[0]
    key = blob[offset + 4 : offset + 4 + key_len]
    if len(key) != 32:
        raise ValueError("unexpected Ed25519 public key length")
    return key


def ssh_string(value: bytes) -> bytes:
    return struct.pack(">I", len(value)) + value


def ssh_signed_payload(payload: bytes) -> bytes:
    return (
        b"SSHSIG"
        + ssh_string(SIGN_NAMESPACE.encode())
        + ssh_string(b"")
        + ssh_string(b"sha256")
        + ssh_string(hashlib.sha256(payload).digest())
    )


def ssh_signature(path: Path, payload: bytes) -> bytes:
    work = Path("/tmp/slime-release-signing.bin")
    signature_path = work.with_suffix(".bin.sig")
    work.write_bytes(payload)
    signature_path.unlink(missing_ok=True)
    subprocess.run(
        ["ssh-keygen", "-Y", "sign", "-q", "-O", "hashalg=sha256", "-f", str(path), "-n", SIGN_NAMESPACE, str(work)],
        check=True,
    )
    lines = signature_path.read_text(encoding="ascii").splitlines()
    blob = base64.b64decode("".join(lines[1:-1]))
    offset = 6
    version = struct.unpack_from(">I", blob, offset)[0]
    offset += 4
    if version != 1:
        raise ValueError("unexpected SSH signature version")
    public_len = struct.unpack_from(">I", blob, offset)[0]
    offset += 4 + public_len
    namespace_len = struct.unpack_from(">I", blob, offset)[0]
    offset += 4
    namespace = blob[offset : offset + namespace_len]
    offset += namespace_len
    reserved_len = struct.unpack_from(">I", blob, offset)[0]
    offset += 4 + reserved_len
    hash_len = struct.unpack_from(">I", blob, offset)[0]
    offset += 4 + hash_len
    signature_blob_len = struct.unpack_from(">I", blob, offset)[0]
    offset += 4
    signature_blob = blob[offset : offset + signature_blob_len]
    algorithm_len = struct.unpack_from(">I", signature_blob, 0)[0]
    signature_offset = 4 + algorithm_len
    signature_len = struct.unpack_from(">I", signature_blob, signature_offset)[0]
    signature = signature_blob[signature_offset + 4 : signature_offset + 4 + signature_len]
    if namespace != SIGN_NAMESPACE.encode() or len(signature) != 64:
        raise ValueError("unexpected SSH signature encoding")
    return signature


def authority_manifest_identity(generation: bytes) -> bytes:
    version = struct.unpack_from("<I", generation, 8)[0]
    if version >= 4:
        executable_count = struct.unpack_from("<I", generation, GENERATION_HEADER_EXECUTABLE_COUNT_OFFSET)[0]
        instance_count = struct.unpack_from("<I", generation, GENERATION_HEADER_INSTANCE_COUNT_OFFSET)[0]
        grant_count = struct.unpack_from("<I", generation, GENERATION_HEADER_GRANT_COUNT_OFFSET)[0]
        executable_offset = struct.unpack_from("<Q", generation, GENERATION_HEADER_EXECUTABLE_OFFSET_OFFSET)[0]
        instance_offset = struct.unpack_from("<Q", generation, GENERATION_HEADER_INSTANCE_OFFSET_OFFSET)[0]
        grant_offset = struct.unpack_from("<Q", generation, GENERATION_HEADER_GRANT_OFFSET_OFFSET)[0]
        string_offset = struct.unpack_from("<Q", generation, GENERATION_HEADER_STRING_OFFSET_OFFSET)[0]
        executable = GENERATION_EXECUTABLE
        instance = GENERATION_INSTANCE
    else:
        header = struct.Struct("<8sIIQ32sQ32sIIIIIIIIIIQQQQQQQQQQ40x")
        component = struct.Struct("<IIIII12x")
        fields = header.unpack_from(generation)
        executable_count = fields[12]
        instance_count = executable_count
        grant_count = fields[14]
        executable_offset = fields[18]
        instance_offset = executable_offset
        grant_offset = fields[20]
        string_offset = fields[23]
        executable = component
        instance = component

    def text(offset: int) -> bytes:
        length = struct.unpack_from("<H", generation, string_offset + offset)[0]
        return generation[string_offset + offset + 2 : string_offset + offset + 2 + length]

    executable_names = [
        text(executable.unpack_from(generation, executable_offset + index * executable.size)[0])
        for index in range(executable_count)
    ]
    instance_names = [
        text(instance.unpack_from(generation, instance_offset + index * instance.size)[0])
        for index in range(instance_count)
    ]
    hasher = hashlib.sha256()
    hasher.update(b"slime-authority-manifest-v1")
    for index in range(grant_count):
        name_offset, source, target, rights, transferable = GENERATION_GRANT.unpack_from(
            generation, grant_offset + index * GENERATION_GRANT.size
        )
        target_names = executable_names if version >= 4 and rights & (1 << 3) else instance_names
        for value in (text(name_offset), instance_names[source], target_names[target]):
            hasher.update(struct.pack("<H", len(value)))
            hasher.update(value)
        hasher.update(struct.pack("<I" if version <= 2 else "<Q", rights))
        hasher.update(struct.pack("<I", transferable))
    return hasher.digest()


def generation_release_fields(generation: bytes) -> tuple[bytes, bytes, str, bytes]:
    identity = generation[24:56]
    parent = generation[64:96]
    target_offset = struct.unpack_from("<I", generation, 96)[0]
    version = struct.unpack_from("<I", generation, 8)[0]
    string_offset = struct.unpack_from("<Q", generation, 208 if version >= 4 else 184)[0]
    target_len = struct.unpack_from("<H", generation, string_offset + target_offset)[0]
    target = generation[string_offset + target_offset + 2 : string_offset + target_offset + 2 + target_len].decode()
    return identity, parent, target, authority_manifest_identity(generation)


def build_release(
    generation: bytes,
    sequence: int,
    key_paths: tuple[Path, ...] = KEY_PATHS,
    boot_bundle_identity: bytes | None = None,
) -> bytes:
    identity, parent, target, authority = generation_release_fields(generation)
    if boot_bundle_identity is None:
        boot_bundle_identity = sha256(b"slime-test-boot-bundle-v2")
    if len(boot_bundle_identity) != 32 or boot_bundle_identity == bytes(32):
        raise ValueError("boot bundle identity must be a nonzero SHA-256 digest")
    target_bytes = target.encode()
    if not 1 <= len(target_bytes) <= MAX_TARGET_BYTES or len(key_paths) > MAX_RELEASE_SIGNATURES:
        raise ValueError("release bound exceeded")
    release = bytearray(RELEASE_BYTES)
    release[:8] = RELEASE_MAGIC
    struct.pack_into("<IIQ", release, 8, RELEASE_VERSION, RELEASE_HEADER_BYTES, 0)
    release[RELEASE_HEADER_GENERATION_IDENTITY_OFFSET:RELEASE_HEADER_GENERATION_IDENTITY_END] = identity
    release[RELEASE_HEADER_PARENT_IDENTITY_OFFSET:RELEASE_HEADER_PARENT_IDENTITY_END] = parent
    struct.pack_into("<QII", release, RELEASE_HEADER_RELEASE_SEQUENCE_OFFSET, sequence, len(target_bytes), 1)
    release[RELEASE_HEADER_TARGET_OFFSET : RELEASE_HEADER_TARGET_OFFSET + len(target_bytes)] = target_bytes
    release[RELEASE_HEADER_BOOT_BUNDLE_IDENTITY_OFFSET:RELEASE_HEADER_BOOT_BUNDLE_IDENTITY_END] = boot_bundle_identity
    release[RELEASE_HEADER_AUTHORITY_MANIFEST_OFFSET:RELEASE_HEADER_AUTHORITY_MANIFEST_END] = authority
    struct.pack_into("<I", release, RELEASE_HEADER_SIGNATURE_COUNT_OFFSET, len(key_paths))
    payload = bytes(release[:RELEASE_HEADER_BYTES])
    entries = sorted((sha256(ssh_public_key(path)), ssh_signature(path, payload)) for path in key_paths)
    for index, (key_id, signature) in enumerate(entries):
        offset = RELEASE_HEADER_BYTES + index * RELEASE_SIGNATURE_BYTES
        release[offset + RELEASE_SIGNATURE_KEY_ID_OFFSET : offset + RELEASE_SIGNATURE_KEY_ID_END] = key_id
        release[offset + RELEASE_SIGNATURE_SIGNATURE_OFFSET : offset + RELEASE_SIGNATURE_SIGNATURE_END] = signature
    return bytes(release)


def initial_public_keys() -> tuple[bytes, ...]:
    return tuple(ssh_public_key(path) for path in KEY_PATHS)
