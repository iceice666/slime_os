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
        digest, offset, length, _ = OBJECT.unpack_from(data, object_start + index * OBJECT.size)
        blob = data[blobs_start + offset : blobs_start + offset + length]
        assert hashlib.sha256(blob).digest() == digest
    print("Generation binary passed")


if __name__ == "__main__":
    main()
