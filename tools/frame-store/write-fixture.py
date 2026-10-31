#!/usr/bin/env python3
"""Write the frame-store fixture the Rust reader's tests read, or check that it is current.

    python3 tools/frame-store/write-fixture.py [--check]

The fixture holds two `std_msgs/msg/String` messages in CDR, 14 and 15 bytes long, so both payloads
are padded. Its RIHS01 hash is the one `ros2 topic info -v` reports for that type.
"""

from __future__ import annotations

import argparse
import sys

import frame_store

FIXTURE = frame_store.ROOT / "components" / "lib" / "src" / "frame_store" / "fixture.bin"
STRING_HASH = "df668c740482bbd48fb39d76a70dfd4bd59db1288021743503259e948f6b1a18"


def cdr_string(text: str) -> bytes:
    """A little-endian CDR `std_msgs/msg/String`: encapsulation header, length with NUL, bytes, NUL."""
    raw = text.encode("utf-8") + b"\x00"
    return b"\x00\x01\x00\x00" + len(raw).to_bytes(4, "little") + raw


STORE = frame_store.Store(
    topic="/fixture/chatter",
    type_name="std_msgs/msg/String",
    schema="string data\n",
    period_ns=100_000_000,
    frames=((1_000_000_000, cdr_string("hello")), (1_100_000_000, cdr_string("world!"))),
)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--check", action="store_true", help="fail if the committed fixture differs")
    arguments = parser.parse_args()
    if frame_store.rihs01(STORE.type_name, STORE.schema).hex() != STRING_HASH:
        print("write-fixture: the computed std_msgs/msg/String hash is not the published one", file=sys.stderr)
        return 1
    data = frame_store.encode(STORE)
    if arguments.check:
        if not FIXTURE.is_file() or FIXTURE.read_bytes() != data:
            print(f"write-fixture: {FIXTURE.relative_to(frame_store.ROOT)} is not current", file=sys.stderr)
            return 1
        print(f"write-fixture: {FIXTURE.relative_to(frame_store.ROOT)} is current ({len(data)} bytes)")
        return 0
    FIXTURE.parent.mkdir(parents=True, exist_ok=True)
    FIXTURE.write_bytes(data)
    print(f"write-fixture: wrote {FIXTURE.relative_to(frame_store.ROOT)} ({len(data)} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
