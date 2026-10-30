"""Encode the Zenoh service grants table of `contracts/zenoh-service-grants-table/v1`.

The generation builder calls `encode` with the rows a system spec declares and stores the bytes as a resource object.
The layout comes from the contract's schema text, so no offset is written here. `scripts/lib/zenoh_grants_table_exam.py`
writes the same bytes again with `struct` and compares them, and refuses the same inputs.
"""

from __future__ import annotations

import re
import struct

import zenoh_client_exam as client
from harness import ROOT

SCHEMA = ROOT / "contracts" / "zenoh-service-grants-table" / "v1" / "schema.zt"

# The characters of a holder name and of a key, as `zenoh-service-grants/v1` states them.
HOLDER_PATTERN = re.compile(r"[a-z0-9_-]+")
KEY_PATTERN = re.compile(r"[A-Za-z0-9/_:.~-]+")


# The fields of a `Grant` in `zenoh-service-grants/v1`. The record is closed, so a row holds exactly these.
ROW_FIELDS = {"holder", "key", "publish", "subscribe"}


class GrantsTableError(Exception):
    pass


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise GrantsTableError(message)


def read_schema(text: str | None = None) -> tuple[dict[str, int], list[tuple[str, int, bool, bool]], list[tuple[str, int, bool, bool]]]:
    text = SCHEMA.read_text(encoding="utf-8") if text is None else text
    return client.constants(text), client.layout(text, "headerLayout"), client.layout(text, "entryLayout")


def _pack(fields: list[tuple[str, int, bool, bool]], values: dict[str, object]) -> bytes:
    out = b""
    for name, width, signed, array in fields:
        value = values[name]
        if array:
            data = bytes(value)
            _need(len(data) == width, f"grants table: {name} is {len(data)} bytes, the layout says {width}")
            out += data
        else:
            code = {1: "b", 2: "h", 4: "i", 8: "q"}[width]
            try:
                out += struct.pack("<" + (code if signed else code.upper()), value)
            except struct.error as error:
                raise GrantsTableError(f"grants table: {name} does not fit {width} bytes: {error}") from error
    return out


def validate_row(row: dict[str, object], key_limit: int, name_limit: int) -> None:
    _need(set(row) == ROW_FIELDS, f"grants table: a row holds the fields {sorted(row)}, the grants record closes a row to {sorted(ROW_FIELDS)}")
    holder, key = row.get("holder"), row.get("key")
    _need(isinstance(holder, str) and 1 <= len(holder) <= name_limit and HOLDER_PATTERN.fullmatch(holder) is not None, f"grants table: holder {holder!r} is not 1 to {name_limit} bytes of a-z, 0-9, hyphen and underscore")
    _need(isinstance(key, str) and 1 <= len(key) <= key_limit, f"grants table: the key of {holder!r} is not 1 to {key_limit} bytes")
    _need(KEY_PATTERN.fullmatch(key) is not None, f"grants table: the key of {holder!r} holds a character outside the grants alphabet")
    _need(all(part for part in key.split("/")), f"grants table: the key of {holder!r} has an empty segment")
    publish, subscribe = row.get("publish"), row.get("subscribe")
    _need(isinstance(publish, bool) and isinstance(subscribe, bool), f"grants table: the rights of {holder!r} are not two booleans")
    _need(publish or subscribe, f"grants table: {holder!r} holds a row that grants nothing")


def canonical(rows: list[dict[str, object]]) -> list[dict[str, object]]:
    """The rows in table order: by holder name and then key, each as bytes. Valid rows are ASCII."""
    return sorted(rows, key=lambda row: (str(row["holder"]).encode("ascii"), str(row["key"]).encode("ascii")))


def encode(rows: list[dict[str, object]], text: str | None = None) -> bytes:
    """The table for `rows`, each `{holder, key, publish, subscribe}`, in the order given."""
    found, header, entry = read_schema(text)
    _need(1 <= len(rows) <= found["maxRows"], f"grants table: {len(rows)} rows, the table holds 1 to {found['maxRows']}")
    pairs = set()
    out = bytearray()
    for row in rows:
        validate_row(row, found["maxKeyBytes"], found["holderNameBytes"])
    for row in canonical(rows):
        pair = (row["holder"], row["key"])
        _need(pair not in pairs, f"grants table: {row['holder']!r} names a key twice")
        pairs.add(pair)
        name, key = str(row["holder"]).encode("ascii"), str(row["key"]).encode("ascii")
        rights = (found["rightPublish"] if row["publish"] else 0) | (found["rightSubscribe"] if row["subscribe"] else 0)
        out.extend(_pack(entry, {
            "holder_name": name.ljust(found["holderNameBytes"], b"\0"), "holder_name_len": len(name), "rights": rights,
            "key_len": len(key), "key": key.ljust(found["maxKeyBytes"], b"\0"), "reserved": bytes(3),
        }))
    total = found["headerBytes"] + len(out)
    head = _pack(header, {
        "magic": int(found["tableMagic"]).to_bytes(8, "little"), "format_version": found["formatVersion"],
        "header_size": found["headerBytes"], "required_flags": 0, "row_count": len(rows), "total_len": total,
    })
    _need(len(head) == found["headerBytes"], "grants table: the header is not the size the schema says")
    return head + bytes(out)
