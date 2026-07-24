#!/usr/bin/env python3
from __future__ import annotations

import argparse
import hashlib
import struct
from pathlib import Path

from harness import SECTOR_SIZE, load_script

CHECK = load_script("check_generation", "check-generation.py")

from boot_contracts import (
    TRANSFER_HEADER_GENERATION_END,
    TRANSFER_HEADER_GENERATION_OFFSET,
    TRANSFER_HEADER_HASH_END,
    TRANSFER_HEADER_HASH_OFFSET,
    TRANSFER_HEADER_OBJECT_COUNT_OFFSET,
    TRANSFER_HEADER_OBJECT_OFFSET_OFFSET,
    TRANSFER_HEADER_RELEASE_SEQUENCE_OFFSET,
    TRANSFER_HEADER_TOTAL_LEN_OFFSET,
    TRANSFER_MAGIC,
    TRANSFER_VERSION,
)
from boot_contracts import (
    TRANSFER_HEADER_AUTHORITY_MANIFEST_END as AUTHORITY_END,
    TRANSFER_HEADER_AUTHORITY_MANIFEST_OFFSET as AUTHORITY_OFFSET,
    TRANSFER_HEADER_PARENT_END as PARENT_END,
    TRANSFER_HEADER_PARENT_OFFSET as PARENT_OFFSET,
    TRANSFER_HEADER_SOURCE_STATE_ROOT_END as STATE_ROOT_END,
    TRANSFER_HEADER_SOURCE_STATE_ROOT_OFFSET as STATE_ROOT_OFFSET,
    TRANSFER_HEADER_BYTES as HEADER_BYTES,
    TRANSFER_OBJECT_BYTES as OBJECT_BYTES,
    TRANSFER_STATE_BYTES as STATE_BYTES,
)

MAGIC = TRANSFER_MAGIC
VERSION = TRANSFER_VERSION
SECTOR = SECTOR_SIZE
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
    header[TRANSFER_HEADER_GENERATION_OFFSET:TRANSFER_HEADER_GENERATION_END] = source_generation[
        "identity"
    ]
    header[PARENT_OFFSET:PARENT_END] = source_generation["parent"]
    header[STATE_ROOT_OFFSET:STATE_ROOT_END] = state_root
    header[AUTHORITY_OFFSET:AUTHORITY_END] = source_generation["authority_manifest"]
    struct.pack_into(
        "<QQ", header, TRANSFER_HEADER_RELEASE_SEQUENCE_OFFSET, release_sequence, len(source)
    )
    struct.pack_into(
        "<IIII", header, TRANSFER_HEADER_OBJECT_COUNT_OFFSET, len(objects), len(states), 0, 0
    )
    struct.pack_into(
        "<QQQQQQ",
        header,
        TRANSFER_HEADER_OBJECT_OFFSET_OFFSET,
        object_table_offset,
        state_table_offset,
        release_offset,
        metadata_offset,
        len(metadata),
        payload_offset,
    )
    struct.pack_into("<Q", header, TRANSFER_HEADER_TOTAL_LEN_OFFSET, total_len)
    bundle = bytearray(header + object_records + state_records + release + metadata + payloads)
    bundle.extend(b"\0" * (total_len - len(bundle)))
    bundle[TRANSFER_HEADER_HASH_OFFSET:TRANSFER_HEADER_HASH_END] = hashlib.sha256(
        bundle[:TRANSFER_HEADER_HASH_OFFSET]
        + b"\0" * 32
        + bundle[TRANSFER_HEADER_HASH_END:]
    ).digest()
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
