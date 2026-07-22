#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location("check_generation", ROOT / "scripts" / "check-generation.py")
if SPEC is None or SPEC.loader is None:
    raise SystemExit("cannot load generation checker")
CHECK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK)

MAGIC = b"SLIMETR\0"
VERSION = 1
HEADER_BYTES = 320
OBJECT_BYTES = 64
STATE_BYTES = 80
SECTOR = 512
POLICIES = {1, 3, 4}


def binding_identity(name: str) -> bytes:
    encoded = name.encode()
    return hashlib.sha256(b"slime-state-binding-v1" + struct.pack("<H", len(encoded)) + encoded).digest()


def authority_manifest_identity(generation: bytes) -> bytes:
    fields = CHECK.GENERATION_HEADER.unpack_from(generation)
    component_count, grant_count = fields[12], fields[14]
    component_offset, grant_offset = fields[18], fields[20]
    strings_offset, strings_len = fields[23:25]
    components = []
    for index in range(component_count):
        row = CHECK.GENERATION_COMPONENT.unpack_from(
            generation, component_offset + index * CHECK.GENERATION_COMPONENT.size
        )
        components.append(CHECK.read_string(generation, strings_offset, strings_len, row[0]))
    hasher = hashlib.sha256()
    hasher.update(b"slime-authority-manifest-v1")
    for index in range(grant_count):
        row = CHECK.GENERATION_GRANT.unpack_from(
            generation, grant_offset + index * CHECK.GENERATION_GRANT.size
        )
        for value in (
            CHECK.read_string(generation, strings_offset, strings_len, row[0]),
            components[row[1]],
            components[row[2]],
        ):
            encoded = value.encode()
            hasher.update(struct.pack("<H", len(encoded)))
            hasher.update(encoded)
        hasher.update(struct.pack("<II", row[3], row[4]))
    return hasher.digest()


def parse_generation(data: bytes) -> dict:
    checked = CHECK.check_generation(data)
    fields = CHECK.GENERATION_HEADER.unpack_from(data)
    objects, states = fields[11], fields[15]
    object_offset, state_offset = fields[17], fields[21]
    strings_offset, strings_len, payload_offset = fields[23:26]
    object_rows = []
    for index in range(objects):
        id_offset, kind, offset, length, digest = CHECK.GENERATION_OBJECT.unpack_from(
            data, object_offset + index * CHECK.GENERATION_OBJECT.size
        )
        object_rows.append(
            {
                "id": CHECK.read_string(data, strings_offset, strings_len, id_offset),
                "kind": kind,
                "payload_offset": offset,
                "length": length,
                "digest": digest,
            }
        )
    state_rows = []
    for index in range(states):
        name_offset, _, schema_version, policy = CHECK.GENERATION_STATE.unpack_from(
            data, state_offset + index * CHECK.GENERATION_STATE.size
        )
        state_rows.append(
            {
                "name": CHECK.read_string(data, strings_offset, strings_len, name_offset),
                "schema_version": schema_version,
                "policy": policy,
            }
        )
    checked.update(
        {
            "objects": object_rows,
            "states": state_rows,
            "metadata_len": payload_offset,
            "authority_manifest": authority_manifest_identity(data),
        }
    )
    return checked


def build_bundle(receiver: bytes, source: bytes, release: bytes, state_root: bytes) -> bytes:
    receiver_generation = parse_generation(receiver)
    source_generation = parse_generation(source)
    if source_generation["parent"] != receiver_generation["identity"]:
        raise SystemExit("source generation parent does not match receiver generation")
    release_sequence = CHECK.check_release(release, source)

    objects = []
    payloads = bytearray()
    receiver_objects = {obj["digest"]: obj for obj in receiver_generation["objects"]}
    for obj in source_generation["objects"]:
        payload = None
        if receiver_objects.get(obj["digest"], {}).get("length") != obj["length"]:
            payload = source[obj["payload_offset"] : obj["payload_offset"] + obj["length"]]
        objects.append((obj, payload))

    metadata = source[: source_generation["metadata_len"]]
    states = [state for state in source_generation["states"] if state["policy"] in POLICIES]
    object_table_offset = HEADER_BYTES
    state_table_offset = object_table_offset + len(objects) * OBJECT_BYTES
    release_offset = state_table_offset + len(states) * STATE_BYTES
    metadata_offset = release_offset + len(release)
    payload_offset = metadata_offset + len(metadata)

    object_records = bytearray()
    cursor = payload_offset
    for obj, payload in objects:
        record_offset = 0
        flags = 0
        if payload is not None:
            record_offset = cursor
            flags = 1
            payloads.extend(payload)
            cursor += len(payload)
        object_records.extend(
            struct.pack("<32sQQIIQ", obj["digest"], obj["length"], record_offset, obj["kind"], flags, 0)
        )

    state_records = bytearray()
    for state in states:
        flags = 1 | (2 if state["policy"] == 1 else 0)
        state_records.extend(
            struct.pack(
                "<32s32sIIII",
                binding_identity(state["name"]),
                state_root,
                state["schema_version"],
                state["policy"],
                flags,
                0,
            )
        )

    total_len = (cursor + SECTOR - 1) // SECTOR * SECTOR
    header = bytearray(HEADER_BYTES)
    header[:8] = MAGIC
    struct.pack_into("<IIQ", header, 8, VERSION, HEADER_BYTES, 0)
    header[24:56] = source_generation["identity"]
    header[56:88] = source_generation["parent"]
    header[88:120] = state_root
    header[120:152] = source_generation["authority_manifest"]
    struct.pack_into("<QQ", header, 152, release_sequence, len(source))
    struct.pack_into("<IIII", header, 176, len(objects), len(states), 0, 0)
    struct.pack_into(
        "<QQQQQQ",
        header,
        184,
        object_table_offset,
        state_table_offset,
        release_offset,
        metadata_offset,
        len(metadata),
        payload_offset,
    )
    struct.pack_into("<Q", header, 232, total_len)
    bundle = bytearray(header + object_records + state_records + release + metadata + payloads)
    bundle.extend(b"\0" * (total_len - len(bundle)))
    bundle[248:280] = hashlib.sha256(bundle[:248] + b"\0" * 32 + bundle[280:]).digest()
    return bytes(bundle)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("receiver", type=Path)
    parser.add_argument("source", type=Path)
    parser.add_argument("release", type=Path)
    parser.add_argument("--state-root", required=True)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    state_root = bytes.fromhex(args.state_root)
    if len(state_root) != 32:
        raise SystemExit("state root must be 32 bytes")
    args.output.write_bytes(
        build_bundle(
            args.receiver.read_bytes(),
            args.source.read_bytes(),
            args.release.read_bytes(),
            state_root,
        )
    )
if __name__ == "__main__":
    main()
