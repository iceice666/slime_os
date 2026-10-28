#!/usr/bin/env python3
"""Convert one channel of an MCAP bag into a `contracts/frame-store/v1` store.

    SLIME_LIDAR_BAG=<bag> python3 tools/frame-store/convert-mcap.py \\
        --sha256 <hex> --channel <topic> --output <store>

The bag is a host artifact that is never committed. The converter refuses it unless its SHA-256
equals `--sha256`, and refuses a bag with any compressed chunk. Its output is a deterministic function
of the bag and the channel. A refusal exits 1 with `convert-mcap: refused <class>: <detail>` as its
last stderr line and writes no store. The classes are in `frame_store.REFUSAL_CLASSES`.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import sys
import tempfile
from pathlib import Path

import frame_store

BAG_ENV = "SLIME_LIDAR_BAG"


def convert(bag: Path, digest: str, channel: str) -> bytes:
    data = bag.read_bytes()
    found = hashlib.sha256(data).hexdigest()
    if found != digest.lower():
        raise frame_store.Refused("bag-hash", f"{bag} has SHA-256 {found}")
    return frame_store.encode(frame_store.store_from_bag(frame_store.read_bag(data), channel))


def write_atomically(path: Path, data: bytes) -> None:
    """Write `data` to `path` so that a reader sees the old file or the whole new one, never part."""
    with tempfile.NamedTemporaryFile(dir=path.parent, prefix=f".{path.name}.", delete=False) as handle:
        handle.write(data)
        temporary = Path(handle.name)
    os.replace(temporary, path)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--sha256", required=True, help="the SHA-256 the bag must have, in hex")
    parser.add_argument("--channel", required=True, help="the topic of the channel to convert")
    parser.add_argument("--output", required=True, type=Path, help="where to write the store")
    arguments = parser.parse_args()
    named = os.environ.get(BAG_ENV)
    if not named:
        parser.error(f"{BAG_ENV} must name the bag")
    try:
        store = convert(Path(named), arguments.sha256, arguments.channel)
    except frame_store.Refused as refusal:
        print(f"convert-mcap: refused {refusal.kind}: {refusal}", file=sys.stderr)
        return 1
    write_atomically(arguments.output, store)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
