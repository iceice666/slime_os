from __future__ import annotations

import base64
import hashlib
import struct
import subprocess
from pathlib import Path

RELEASE_MAGIC = b"SLIMERL\0"
RELEASE_VERSION = 1
RELEASE_BYTES = 512
RELEASE_HEADER_BYTES = 208
RELEASE_SIGNATURE_BYTES = 96
MAX_RELEASE_SIGNATURES = 3
MAX_TARGET_BYTES = 32
ROTATION_MAGIC = b"SLIMERT\0"
ROTATION_VERSION = 1
ROTATION_BYTES = 1024
ROTATION_HEADER_BYTES = 64
MAX_TRUST_KEYS = 4
SIGN_NAMESPACE = "slime-release"

ROOT = Path(__file__).resolve().parent.parent
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
    header = struct.Struct("<8sIIQ32sQ32sIIIIIIIIIIQQQQQQQQQQ40x")
    component = struct.Struct("<IIIII12x")
    grant = struct.Struct("<IIIII12x")
    fields = header.unpack_from(generation)
    component_count = fields[12]
    grant_count = fields[14]
    component_offset = fields[18]
    grant_offset = fields[20]
    string_offset = fields[23]

    def text(offset: int) -> bytes:
        length = struct.unpack_from("<H", generation, string_offset + offset)[0]
        return generation[string_offset + offset + 2 : string_offset + offset + 2 + length]

    component_names = []
    for index in range(component_count):
        name_offset = component.unpack_from(generation, component_offset + index * component.size)[0]
        component_names.append(text(name_offset))
    hasher = hashlib.sha256()
    hasher.update(b"slime-authority-manifest-v1")
    for index in range(grant_count):
        name_offset, source, target, rights, transferable = grant.unpack_from(
            generation, grant_offset + index * grant.size
        )
        for value in (text(name_offset), component_names[source], component_names[target]):
            hasher.update(struct.pack("<H", len(value)))
            hasher.update(value)
        hasher.update(struct.pack("<II", rights, transferable))
    return hasher.digest()


def generation_release_fields(generation: bytes) -> tuple[bytes, bytes, str, bytes, bytes]:
    identity = generation[24:56]
    parent = generation[64:96]
    target_offset = struct.unpack_from("<I", generation, 96)[0]
    kernel_index = struct.unpack_from("<I", generation, 100)[0]
    object_offset = struct.unpack_from("<Q", generation, 136)[0]
    string_offset = struct.unpack_from("<Q", generation, 184)[0]
    target_len = struct.unpack_from("<H", generation, string_offset + target_offset)[0]
    target = generation[string_offset + target_offset + 2 : string_offset + target_offset + 2 + target_len].decode()
    object_record = object_offset + kernel_index * 64
    kernel = generation[object_record + 24 : object_record + 56]
    return identity, parent, target, kernel, authority_manifest_identity(generation)


def build_release(generation: bytes, sequence: int, key_paths: tuple[Path, ...] = KEY_PATHS) -> bytes:
    identity, parent, target, kernel, authority = generation_release_fields(generation)
    target_bytes = target.encode()
    if not 1 <= len(target_bytes) <= MAX_TARGET_BYTES or len(key_paths) > MAX_RELEASE_SIGNATURES:
        raise ValueError("release bound exceeded")
    release = bytearray(RELEASE_BYTES)
    release[:8] = RELEASE_MAGIC
    struct.pack_into("<IIQ", release, 8, RELEASE_VERSION, RELEASE_HEADER_BYTES, 0)
    release[24:56] = identity
    release[56:88] = parent
    struct.pack_into("<QII", release, 88, sequence, len(target_bytes), 1)
    release[104 : 104 + len(target_bytes)] = target_bytes
    release[136:168] = kernel
    release[168:200] = authority
    struct.pack_into("<I", release, 200, len(key_paths))
    payload = bytes(release[:RELEASE_HEADER_BYTES])
    entries = sorted((sha256(ssh_public_key(path)), ssh_signature(path, payload)) for path in key_paths)
    for index, (key_id, signature) in enumerate(entries):
        offset = RELEASE_HEADER_BYTES + index * RELEASE_SIGNATURE_BYTES
        release[offset : offset + 32] = key_id
        release[offset + 32 : offset + RELEASE_SIGNATURE_BYTES] = signature
    return bytes(release)


def initial_public_keys() -> tuple[bytes, ...]:
    return tuple(ssh_public_key(path) for path in KEY_PATHS)
