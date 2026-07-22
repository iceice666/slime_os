#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import struct
import sys
from pathlib import Path

import importlib.util

STORE_BUILDER = Path(__file__).with_name("build-store-fixture.py")
SPEC = importlib.util.spec_from_file_location("build_store_fixture", STORE_BUILDER)
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("cannot load store fixture builder")
store = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(store)

SNAPSHOT_MAGIC = b"SLIMEDIR"
SNAPSHOT_VERSION = 1
SNAPSHOT_HEADER = 16
ENTRY_BYTES = 64
MAX_ENTRIES = 16
MAX_NAME = 16
SNAPSHOT_TYPE = 0x44524953
PAYLOAD_TYPE = 7
PAYLOAD = b"Slime OS directory payload v1\n"


def entry(kind: int, name: bytes, obj_type: int, payload_len: int, digest: bytes) -> bytes:
    encoded = bytearray(ENTRY_BYTES)
    encoded[0] = kind
    encoded[1] = len(name)
    encoded[4 : 4 + len(name)] = name
    struct.pack_into("<I", encoded, 20, obj_type)
    struct.pack_into("<I", encoded, 24, payload_len)
    encoded[28:60] = digest
    return bytes(encoded)


def snapshot(entries: list[bytes]) -> bytes:
    encoded = bytearray(SNAPSHOT_HEADER + MAX_ENTRIES * ENTRY_BYTES)
    encoded[:8] = SNAPSHOT_MAGIC
    struct.pack_into("<I", encoded, 8, SNAPSHOT_VERSION)
    struct.pack_into("<I", encoded, 12, len(entries))
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
