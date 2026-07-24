#!/usr/bin/env python3

from __future__ import annotations

import hashlib
import sys
from pathlib import Path

from harness import SECTOR_SIZE

SECTORS = 8
MESSAGE = b"Slime OS M5.2 read-only virtio block fixture\n"


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: build-storage-fixture.py <output>")
    output = Path(sys.argv[1])
    image = bytearray(SECTOR_SIZE * SECTORS)
    image[: len(MESSAGE)] = MESSAGE
    for index in range(len(MESSAGE), SECTOR_SIZE):
        image[index] = (index * 37 + 11) & 0xFF
    output.write_bytes(image)
    digest = hashlib.sha256(image[:SECTOR_SIZE]).hexdigest()
    print(f"Built {output} ({len(image)} bytes, sector0 sha256:{digest})")


if __name__ == "__main__":
    main()
