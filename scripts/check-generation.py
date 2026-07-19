#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import struct
import sys
from pathlib import Path

HEADER = struct.Struct("<8sIIQ32sHHHHH2x")
OBJECT = struct.Struct("<32sQII")
COMPONENT = struct.Struct("<IIBBH")
GRANT = struct.Struct("<IIIBBH")

# Component image format (contracts/component/v1). Mirrors the kernel decoder
# in kernel/src/component.rs; drift is caught by `just generation_check`.
IMAGE_HEADER = struct.Struct("<8sIIIIHHI")
IMAGE_SEGMENT = struct.Struct("<IIIIHH")


def check_image(blob: bytes) -> None:
    magic, version, header_size, abi, entry, count, _, stack = IMAGE_HEADER.unpack_from(blob)
    assert magic == b"SLIMECMP" and version == 1 and header_size == IMAGE_HEADER.size
    assert abi == 1
    assert 1 <= count <= 16
    assert 0 < stack <= 1024 * 1024 and stack % 4096 == 0
    data_start = IMAGE_HEADER.size + count * IMAGE_SEGMENT.size
    assert len(blob) >= data_start
    previous_end = 0
    total_pages = 0
    entry_ok = False
    for index in range(count):
        vaddr, mem_len, file_offset, file_len, flags, _ = IMAGE_SEGMENT.unpack_from(
            blob, IMAGE_HEADER.size + index * IMAGE_SEGMENT.size
        )
        assert not flags & ~0b11 and flags & 0b11 != 0b11
        assert vaddr % 4096 == 0 and 0 < mem_len and file_len <= mem_len
        assert vaddr >= previous_end
        previous_end = vaddr + mem_len
        assert data_start + file_offset + file_len <= len(blob)
        total_pages += -(-mem_len // 4096)
        assert total_pages * 4096 <= 16 * 1024 * 1024
        if flags & 0b10 and vaddr <= entry < vaddr + mem_len:
            entry_ok = True
    assert entry_ok


def main() -> None:
    data = Path(sys.argv[1]).read_bytes()
    magic, version, header_size, _, expected, objects, components, grants, bootstrap, _ = HEADER.unpack_from(data)
    assert magic == b"SLIMEGEN" and version == 1 and header_size == HEADER.size
    assert bootstrap < components
    calculated = hashlib.sha256(data[:24] + bytes(32) + data[56:]).digest()
    assert calculated == expected
    object_start = HEADER.size
    component_start = object_start + objects * OBJECT.size
    grant_start = component_start + components * COMPONENT.size
    strings_start = grant_start + grants * GRANT.size
    string_end = 0
    for index in range(components):
        offset, object_index, _, _, _ = COMPONENT.unpack_from(data, component_start + index * COMPONENT.size)
        assert object_index < objects
        length = struct.unpack_from("<H", data, strings_start + offset)[0]
        string_end = max(string_end, offset + 2 + length)
    for index in range(grants):
        offset, source, target, _, _, _ = GRANT.unpack_from(data, grant_start + index * GRANT.size)
        assert source < components and target < components
        length = struct.unpack_from("<H", data, strings_start + offset)[0]
        string_end = max(string_end, offset + 2 + length)
    blobs_start = strings_start + string_end
    for index in range(objects):
        digest, offset, length, kind = OBJECT.unpack_from(data, object_start + index * OBJECT.size)
        blob = data[blobs_start + offset : blobs_start + offset + length]
        assert hashlib.sha256(blob).digest() == digest
        if kind in (2, 3):  # bootstrap and component objects carry component images
            check_image(blob)
    print("Generation binary passed")


if __name__ == "__main__":
    main()
