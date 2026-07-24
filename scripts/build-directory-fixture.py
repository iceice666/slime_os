#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import struct
import sys
from pathlib import Path

from harness import load_script

store = load_script("build_store_fixture", "build-store-fixture.py")

from fs_contracts import (
    FS_MAX_ENTRIES as MAX_ENTRIES,
    FS_MAX_NAME_BYTES as MAX_NAME,
    SNAPSHOT_BYTES,
    SNAPSHOT_COUNT_OFFSET,
    SNAPSHOT_ENTRY_BYTES as ENTRY_BYTES,
    SNAPSHOT_ENTRY_HASH_END,
    SNAPSHOT_ENTRY_HASH_OFFSET,
    SNAPSHOT_ENTRY_KIND_OFFSET,
    SNAPSHOT_ENTRY_NAME_LEN_OFFSET,
    SNAPSHOT_ENTRY_NAME_OFFSET,
    SNAPSHOT_ENTRY_OBJECT_TYPE_OFFSET,
    SNAPSHOT_ENTRY_PAYLOAD_LEN_OFFSET,
    SNAPSHOT_HEADER,
    SNAPSHOT_MAGIC,
    SNAPSHOT_OBJECT_TYPE as SNAPSHOT_TYPE,
    SNAPSHOT_VERSION,
    SNAPSHOT_VERSION_OFFSET,
)

PAYLOAD_TYPE = 7
PAYLOAD = b"Slime OS directory payload v1\n"


def entry(kind: int, name: bytes, obj_type: int, payload_len: int, digest: bytes) -> bytes:
    encoded = bytearray(ENTRY_BYTES)
    encoded[SNAPSHOT_ENTRY_KIND_OFFSET] = kind
    encoded[SNAPSHOT_ENTRY_NAME_LEN_OFFSET] = len(name)
    encoded[SNAPSHOT_ENTRY_NAME_OFFSET : SNAPSHOT_ENTRY_NAME_OFFSET + len(name)] = name
    struct.pack_into("<I", encoded, SNAPSHOT_ENTRY_OBJECT_TYPE_OFFSET, obj_type)
    struct.pack_into("<I", encoded, SNAPSHOT_ENTRY_PAYLOAD_LEN_OFFSET, payload_len)
    encoded[SNAPSHOT_ENTRY_HASH_OFFSET:SNAPSHOT_ENTRY_HASH_END] = digest
    return bytes(encoded)


def snapshot(entries: list[bytes]) -> bytes:
    encoded = bytearray(SNAPSHOT_BYTES)
    encoded[:8] = SNAPSHOT_MAGIC
    struct.pack_into("<I", encoded, SNAPSHOT_VERSION_OFFSET, SNAPSHOT_VERSION)
    struct.pack_into("<I", encoded, SNAPSHOT_COUNT_OFFSET, len(entries))
    for index, value in enumerate(entries):
        encoded[SNAPSHOT_HEADER + index * ENTRY_BYTES : SNAPSHOT_HEADER + (index + 1) * ENTRY_BYTES] = value
    return bytes(encoded)


def build() -> tuple[bytearray, bytes]:
    image = store.build("happy")
    payload_hash = hashlib.sha256(PAYLOAD).digest()
    docs = snapshot([entry(1, b"note", PAYLOAD_TYPE, len(PAYLOAD), payload_hash)])
    docs_hash = hashlib.sha256(docs).digest()
    root = snapshot(
        [
            entry(2, b"docs", SNAPSHOT_TYPE, len(docs), docs_hash),
            entry(1, b"note", PAYLOAD_TYPE, len(PAYLOAD), payload_hash),
        ]
    )
    root_hash = hashlib.sha256(root).digest()

    records = [
        store.record(PAYLOAD_TYPE, PAYLOAD),
        store.record(SNAPSHOT_TYPE, docs),
        store.record(SNAPSHOT_TYPE, root),
    ]
    cursor = store.RECORD_AREA_START
    for record in records:
        store.place(image, store.STORE_FIRST + cursor, record)
        cursor += len(record) // store.SECTOR
    store.place(image, store.STORE_FIRST, store.superblock(2, cursor, len(records)))
    store.place(
        image,
        store.STORE_FIRST + 1,
        store.superblock(1, store.RECORD_AREA_START, 0),
    )
    return image, root_hash


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: build-directory-fixture.py <output>")
    image, root_hash = build()
    output = Path(sys.argv[1])
    output.write_bytes(image)
    print(f"Built {output} ({len(image)} bytes, root sha256:{root_hash.hex()})")


if __name__ == "__main__":
    main()
