"""Judge the encoder of the Zenoh service grants table, `contracts/zenoh-service-grants-table/v1`.

`zenoh_grants_table.encode` turns the rows a system spec declares into the bytes the root serves. This module holds
three kinds of fact about it, and none of them reads the encoder's own constants:

* the schema's layouts add up to the header and row sizes it states, list the fields of their record types in
  order, and equal a copy of the layouts held here, so a field cannot move by editing the schema alone;
* for a set of honest row sets, the encoder's bytes equal bytes written again here with `struct` from the pinned
  layout, and the bytes decode back to the rows;
* the encoder refuses each input the contract forbids, and a decoder here refuses each corruption of an honest table.

Like the other judges it is trusted only after it refuses a mutation of an honest input.
"""

from __future__ import annotations

import re
import struct

import zenoh_client_exam as client
import zenoh_grants_table as table
from harness import ROOT

# Every field of both records: name, width in bytes, signed, byte array. The schema is compared with these and not with
# its own record types.
HEADER_PIN = [
    ("magic", 8, False, True), ("format_version", 4, False, False), ("header_size", 4, False, False),
    ("required_flags", 8, False, False), ("row_count", 4, False, False), ("total_len", 4, False, False),
]
ENTRY_PIN = [
    ("holder_name", 32, False, True), ("holder_name_len", 1, False, False), ("rights", 1, False, False),
    ("key_len", 2, False, False), ("key", 129, False, True), ("reserved", 3, False, True),
]
PINNED = {
    "formatVersion": 1, "tableMagic": int.from_bytes(b"SLIMEZG\0", "little"), "headerBytes": 32, "entryBytes": 168,
    "maxRows": 65535, "holderNameBytes": 32, "maxKeyBytes": 129, "rightPublish": 1, "rightSubscribe": 2, "knownRights": 3,
}
MAGIC = b"SLIMEZG\0"
DEMO_KEY = "0/slime_demo/extra/second"
LONGEST_KEY = "k" * 129


class TableExamError(Exception):
    pass


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise TableExamError(message)


def schema_facts(text: str | None = None) -> list[str]:
    """The facts about the schema, as one line each; raises for the first that does not hold."""
    text = table.SCHEMA.read_text(encoding="utf-8") if text is None else text
    try:
        found = client.constants(text)
        header, entry = client.layout(text, "headerLayout"), client.layout(text, "entryLayout")
        header_fields, entry_fields = client.record_fields(text, "GrantsHeader"), client.record_fields(text, "GrantsEntry")
    except client.ClientError as error:
        raise TableExamError(str(error)) from error
    out = []
    for name, wanted in PINNED.items():
        _need(found.get(name) == wanted, f"grants table: {name} is {found.get(name)}, this version requires {wanted}")
        out.append(f"constant {name} {wanted}")
    for label, fields, pin, names, size in (("header", header, HEADER_PIN, header_fields, "headerBytes"), ("entry", entry, ENTRY_PIN, entry_fields, "entryBytes")):
        _need([name for name, *_ in fields] == names, f"grants table: the {label} layout does not list the fields of its record in order")
        _need(sum(width for _, width, _, _ in fields) == found[size], f"grants table: the {label} layout does not add up to {size}")
        _need(fields == pin, f"grants table: the {label} layout differs from the pinned layout")
        out.append(f"layout {label} {found[size]}")
    _need(found["knownRights"] == found["rightPublish"] | found["rightSubscribe"], "grants table: knownRights is not the two rights together")
    _need(found["rightPublish"] & found["rightSubscribe"] == 0, "grants table: the two rights share a bit")
    _need(found["maxKeyBytes"] + 3 + 1 + 1 + 2 + found["holderNameBytes"] == found["entryBytes"], "grants table: the row does not hold a full key and name")
    out.append("rights distinct")
    # A window of 1024 bytes holds whole rows; the rows read in one call are what fit.
    out.append(f"rows-per-read {1024 // found['entryBytes']}")
    return out


def _row(holder: str, key: str, publish: bool, subscribe: bool) -> dict[str, object]:
    return {"holder": holder, "key": key, "publish": publish, "subscribe": subscribe}


# Honest row sets: name, rows.
HONEST = (
    ("one publisher", [_row("pub", DEMO_KEY, True, False)]),
    ("a publisher and a subscriber of one key", [_row("pub", DEMO_KEY, True, False), _row("sub", DEMO_KEY, False, True)]),
    ("both rights on one row", [_row("both", DEMO_KEY, True, True)]),
    ("the longest key and the longest name", [_row("n" * 32, LONGEST_KEY, True, False)]),
    ("the shortest key and name", [_row("a", "k", False, True)]),
    ("rows given out of order are written sorted", [_row("sub", DEMO_KEY, False, True), _row("pub", DEMO_KEY, True, False), _row("pub", "0/a", False, True)]),
    ("one holder on several keys", [_row("h", "a/b", True, False), _row("h", "a/c", False, True), _row("h", "d", True, True)]),
    ("a name of hyphens, underscores and digits", [_row("a-b_c9", "0/k_1-2:3.4~5", True, False)]),
)


def reference(rows: list[dict[str, object]], text: str | None = None) -> bytes:
    """The table written again with `struct` from the pinned layouts, rows sorted by holder name and then key."""
    found = client.constants(table.SCHEMA.read_text(encoding="utf-8") if text is None else text)
    entries = b""
    for row in sorted(rows, key=lambda row: (str(row["holder"]).encode(), str(row["key"]).encode())):
        name, key = str(row["holder"]).encode(), str(row["key"]).encode()
        rights = (1 if row["publish"] else 0) | (2 if row["subscribe"] else 0)
        entries += struct.pack("<32sBBH129s3s", name, len(name), rights, len(key), key, bytes(3))
    total = found["headerBytes"] + len(entries)
    return struct.pack("<8sIIQII", MAGIC, found["formatVersion"], found["headerBytes"], 0, len(rows), total) + entries


def decode(data: bytes) -> list[dict[str, object]]:
    """The rows a table holds. Every field is checked, padding and reserved bytes included."""
    _need(len(data) >= 32, "table: shorter than the header")
    magic, version, header_size, flags, count, total = struct.unpack_from("<8sIIQII", data, 0)
    _need(magic == MAGIC, "table: wrong magic")
    _need(version == 1, "table: unknown format version")
    _need(header_size == 32, "table: header size is not 32")
    _need(flags == 0, "table: a required flag is set")
    _need(1 <= count <= 65535, "table: row count outside 1 to 65535")
    _need(total == 32 + count * 168 and len(data) == total, "table: total length does not match the rows")
    rows, previous = [], None
    for index in range(count):
        name, name_len, rights, key_len, key, reserved = struct.unpack_from("<32sBBH129s3s", data, 32 + index * 168)
        _need(1 <= name_len <= 32 and not any(name[name_len:]), "table: a name length is out of range or its padding is not zero")
        _need(1 <= key_len <= 129 and not any(key[key_len:]), "table: a key length is out of range or its padding is not zero")
        _need(not any(reserved), "table: reserved bytes are not zero")
        _need(rights in (1, 2, 3), "table: a row grants nothing or sets an unknown right")
        holder, text = name[:name_len].decode("ascii", "replace"), key[:key_len].decode("ascii", "replace")
        _need(table.HOLDER_PATTERN.fullmatch(holder) is not None, "table: a holder name holds a character outside its alphabet")
        _need(table.KEY_PATTERN.fullmatch(text) is not None and all(part for part in text.split("/")), "table: a key holds a character outside its alphabet or an empty segment")
        current = (name[:name_len], key[:key_len])
        _need(previous is None or current > previous, "table: rows are not in strictly ascending order of holder and key")
        previous = current
        rows.append(_row(holder, text, bool(rights & 1), bool(rights & 2)))
    return rows


def honest_cases() -> int:
    """The encoder equals the reference bytes for each honest set, and the bytes decode back to the rows."""
    for name, rows in HONEST:
        written = table.encode(rows)
        _need(written == reference(rows), f"grants table: the encoder's bytes for {name} differ from the reference")
        _need(decode(written) == table.canonical(rows), f"grants table: the bytes for {name} do not decode back to the rows in table order")
    return len(HONEST)


def _edit(old: str, new: str, count: int = 1):
    def apply(text: str) -> str:
        _need(text.count(old) == count, f"control: {old!r} found {text.count(old)} times, expected {count}")
        return text.replace(old, new)
    return apply


def _field(name: str, width: int, signed: str = "false", array: str = "false") -> str:
    return f'{{ name = "{name}"; width = {width}; signed = {signed}; byteArray = {array}; }};'


# Each edit corrupts the schema in one place. The facts must refuse it.
SCHEMA_EDITS = (
    ("the magic changed", _edit("tableMagic :: Int = 20083977042349139;", "tableMagic :: Int = 20083977042349138;")),
    ("the format version raised", _edit("formatVersion :: Int = 1;", "formatVersion :: Int = 2;")),
    ("the header size changed", _edit("headerBytes :: Int = 32;", "headerBytes :: Int = 33;")),
    ("the row size changed", _edit("entryBytes :: Int = 168;", "entryBytes :: Int = 169;")),
    ("the row bound raised", _edit("maxRows :: Int = 65535;", "maxRows :: Int = 65536;")),
    ("the row bound lowered", _edit("maxRows :: Int = 65535;", "maxRows :: Int = 24;")),
    ("the key bound raised", _edit("maxKeyBytes :: Int = 129;", "maxKeyBytes :: Int = 130;")),
    ("the name bound raised", _edit("holderNameBytes :: Int = 32;", "holderNameBytes :: Int = 33;")),
    ("the publish right changed", _edit("rightPublish :: Int = 1;", "rightPublish :: Int = 2;")),
    ("the subscribe right changed", _edit("rightSubscribe :: Int = 2;", "rightSubscribe :: Int = 1;")),
    ("the known rights changed", _edit("knownRights :: Int = 3;", "knownRights :: Int = 7;")),
    ("a header field widened", _edit(_field("row_count", 4), _field("row_count", 8))),
    ("a header field made signed", _edit(_field("total_len", 4), _field("total_len", 4, "true"))),
    ("the magic made a number", _edit(_field("magic", 8, array="true"), _field("magic", 8))),
    ("a row field widened", _edit(_field("key_len", 2), _field("key_len", 4))),
    ("the key shortened", _edit(_field("key", 129, array="true"), _field("key", 128, array="true"))),
    ("the name lengthened", _edit(_field("holder_name", 32, array="true"), _field("holder_name", 33, array="true"))),
    ("the reserved field dropped from the layout", _edit("  " + _field("reserved", 3, array="true") + "\n};\n", "};\n")),
    ("two row fields swapped", _edit(_field("holder_name_len", 1) + "\n  " + _field("rights", 1), _field("rights", 1) + "\n  " + _field("holder_name_len", 1))),
    ("a row field renamed", _edit(_field("rights", 1), _field("grants", 1))),
    ("a field removed from the record only", _edit("  rights : Int;\n", "")),
    ("a field added to the record only", _edit("  key_len : Int;\n", "  key_len : Int;\n  extra : Int;\n")),
    ("a constant stated twice", _edit("maxRows :: Int = 65535;", "maxRows :: Int = 65535;\nmaxRows :: Int = 65535;")),
    ("a line that is not a field", _edit(_field("reserved", 3, array="true"), "not a field")),
)


def schema_controls() -> int:
    schema_facts()
    refused = 0
    for label, edit in SCHEMA_EDITS:
        mutated = edit(table.SCHEMA.read_text(encoding="utf-8"))
        try:
            schema_facts(mutated)
        except TableExamError:
            refused += 1
            continue
        raise TableExamError(f"control: the schema facts accepted {label}")
    return refused


def _try(rows: list[dict[str, object]]) -> bool:
    try:
        table.encode(rows)
    except table.GrantsTableError:
        return False
    return True


GOOD = _row("pub", DEMO_KEY, True, False)


def _with(**changes: object) -> dict[str, object]:
    return {**GOOD, **changes}


# Each input the contract forbids: a label and the rows. The encoder must refuse each.
REFUSED_INPUTS = (
    ("no rows", []),
    ("a row that grants nothing", [_with(publish=False, subscribe=False)]),
    ("a holder name of zero bytes", [_with(holder="")]),
    ("a holder name of 33 bytes", [_with(holder="n" * 33)]),
    ("a holder name with a capital", [_with(holder="Pub")]),
    ("a holder name with a dot", [_with(holder="p.b")]),
    ("a holder name with a space", [_with(holder="p b")]),
    ("a holder name outside ASCII", [_with(holder="pub\u00e9")]),
    ("a key of zero bytes", [_with(key="")]),
    ("a key of 130 bytes", [_with(key="k" * 130)]),
    ("a key with a star", [_with(key="0/slime_demo/*")]),
    ("a key with a double star", [_with(key="0/slime_demo/**")]),
    ("a key with a dollar", [_with(key="0/slime_demo/$*")]),
    ("a key with a question mark", [_with(key="0/slime_demo/a?b")]),
    ("a key with a hash", [_with(key="0/slime_demo/a#b")]),
    ("a key with a space", [_with(key="0/slime demo")]),
    ("a key outside ASCII", [_with(key="0/slime_demo/\u00e9")]),
    ("a key with a leading slash", [_with(key="/0/slime_demo")]),
    ("a key with a trailing slash", [_with(key="0/slime_demo/")]),
    ("a key with an empty segment", [_with(key="0//slime_demo")]),
    ("a rights field that is not a boolean", [_with(publish=1)]),
    ("a missing right", [{"holder": "pub", "key": DEMO_KEY, "publish": True}]),
    ("a holder that is not text", [_with(holder=7)]),
    ("a key that is not text", [_with(key=None)]),
    ("a holder naming a key twice", [GOOD, _with(subscribe=True)]),
    ("a row with a field the grants record does not have", [{**GOOD, "future_right": True}]),
    ("a row missing a field the grants record has", [{k: v for k, v in GOOD.items() if k != "key"}]),
)

# Each corruption of an honest table: a label and a function of its bytes. The decoder must refuse each.
HONEST_TABLE_ROWS = [_row("pub", DEMO_KEY, True, False), _row("sub", DEMO_KEY, False, True)]


def _flip(offset: int, value: int):
    def apply(data: bytes) -> bytes:
        return data[:offset] + bytes([value]) + data[offset + 1 :]
    return apply


def _row_at(index: int, field_offset: int) -> int:
    return 32 + index * 168 + field_offset


def _header(data: bytes, count: int, total: int) -> bytes:
    """The honest header with a new row count and total length. The rows that follow are the caller's."""
    return data[:24] + count.to_bytes(4, "little") + total.to_bytes(4, "little")


def _zero_name(data: bytes) -> bytes:
    """The first row's name made empty: its length is zero and its bytes are zero, so only the length guard refuses it."""
    return data[: _row_at(0, 0)] + bytes(33) + data[_row_at(0, 33) :]


def _zero_key(data: bytes) -> bytes:
    """The first row's key made empty: its length is zero and its bytes are zero."""
    return data[: _row_at(0, 34)] + bytes(2) + bytes(129) + data[_row_at(0, 165) :]


CORRUPTIONS = (
    ("a table shorter than its header", lambda data: data[:31]),
    ("a table with a byte missing", lambda data: data[:-1]),
    ("a table with a byte added", lambda data: data + b"\0"),
    ("a wrong magic", _flip(0, ord("X"))),
    ("a wrong format version", _flip(8, 2)),
    ("a wrong header size", _flip(12, 33)),
    ("a required flag set", _flip(16, 1)),
    ("a row count of zero", lambda data: data[:24] + bytes(4) + data[28:]),
    ("a row count above the rows present", _flip(24, 3)),
    ("a row count below the rows present", _flip(24, 1)),
    ("a total length that is wrong", _flip(28, 0x71)),
    ("a name length of zero", _flip(_row_at(0, 32), 0)),
    ("a name length of 33", _flip(_row_at(0, 32), 33)),
    ("a nonzero byte after the name", _flip(_row_at(0, 31), 1)),
    ("a table of no rows with a matching length", lambda data: _header(data, count=0, total=32) + b""),
    ("a table whose header claims 65536 rows", lambda data: _header(data, count=65536, total=32 + 65536 * 168) + data[32:]),
    ("a name of length zero whose bytes are all zero", lambda data: _zero_name(data)),
    ("a key of length zero whose bytes are all zero", lambda data: _zero_key(data)),
    ("a rights field of zero", _flip(_row_at(0, 33), 0)),
    ("a rights field with an unknown bit", _flip(_row_at(0, 33), 5)),
    ("a key length of zero", lambda data: data[: _row_at(0, 34)] + bytes(2) + data[_row_at(0, 36) :]),
    ("a key length of 130", lambda data: data[: _row_at(0, 34)] + (130).to_bytes(2, "little") + data[_row_at(0, 36) :]),
    ("a nonzero byte after the key", _flip(_row_at(0, 36 + 128), 1)),
    ("a nonzero reserved byte", _flip(_row_at(0, 165), 1)),
    ("a stored holder name with a capital", _flip(_row_at(0, 0), ord("P"))),
    ("a stored key with a star", _flip(_row_at(0, 36), ord("*"))),
    ("a stored key with a space", _flip(_row_at(0, 37), ord(" "))),
    ("a stored key with an empty segment", _flip(_row_at(0, 38), ord("/"))),
    ("two rows swapped", lambda data: data[:32] + data[_row_at(1, 0) :] + data[_row_at(0, 0) : _row_at(1, 0)]),
    ("a second row repeating the first", lambda data: data[: _row_at(1, 0)] + data[_row_at(0, 0) : _row_at(1, 0)]),
    ("a second row naming the same key under the same holder", lambda data: data[: _row_at(1, 0)] + data[_row_at(0, 0) : _row_at(0, 33)] + bytes([2]) + data[_row_at(0, 34) : _row_at(1, 0)]),
)


def input_controls() -> int:
    refused = 0
    for label, rows in REFUSED_INPUTS:
        if _try(rows):
            raise TableExamError(f"control: the encoder accepted {label}")
        refused += 1
    return refused


def table_controls() -> int:
    honest = table.encode(HONEST_TABLE_ROWS)
    _need(decode(honest) == HONEST_TABLE_ROWS, "control: the honest table does not decode back to its rows")
    refused = 0
    for label, corrupt in CORRUPTIONS:
        mutated = corrupt(honest)
        if mutated == honest:
            raise TableExamError(f"control: {label} did not change the table")
        try:
            decode(mutated)
        except TableExamError:
            refused += 1
            continue
        raise TableExamError(f"control: the decoder accepted {label}")
    return refused


GRANTS_SCHEMA = ROOT / "contracts" / "zenoh-service-grants" / "v1" / "schema.zt"


def _alphabet(text: str, name: str) -> str:
    block = re.search(rf"^{name} :: List Text = \{{(.*?)\}};", text, re.M | re.S)
    _need(block is not None, f"grants table: the grants contract does not state {name}")
    return "".join(re.findall(r'"(.)"', block.group(1)))


def alphabet_facts(grants_text: str | None = None) -> int:
    """The encoder accepts exactly the characters the grants contract lists for a holder name and for a key, over
    all of ASCII, so the table cannot take a grant the record refuses or refuse one it allows."""
    text = GRANTS_SCHEMA.read_text(encoding="utf-8") if grants_text is None else grants_text
    for label, name, pattern in (("holder", "nameChars", table.HOLDER_PATTERN), ("key", "keyChars", table.KEY_PATTERN)):
        listed = set(_alphabet(text, name))
        accepted = {chr(code) for code in range(128) if pattern.fullmatch(chr(code))}
        _need(accepted == listed, f"grants table: the {label} alphabet differs from the grants contract's {name} by {sorted(accepted ^ listed)}")
    return 2


ALPHABET_EDITS = (
    ("a key character added", '"~"; };', '"~"; "*"; };'),
    ("a key character removed", '"~"; };', "};"),
    ("a holder character added", '"9"; "-"; "_"; };', '"9"; "-"; "_"; "."; };'),
    ("a holder character removed", '"9"; "-"; "_"; };', '"9"; "-"; };'),
)


def alphabet_controls() -> int:
    alphabet_facts()
    text = GRANTS_SCHEMA.read_text(encoding="utf-8")
    refused = 0
    for label, old, new in ALPHABET_EDITS:
        _need(text.count(old) == 1, f"control: {label} anchor found {text.count(old)} times")
        try:
            alphabet_facts(text.replace(old, new))
        except TableExamError:
            refused += 1
            continue
        raise TableExamError(f"control: the alphabet check accepted {label}")
    return refused


# Each edit moves or resizes a field of the schema the encoder reads. The pinned reference is written from the real
# layout, so the encoder's bytes under the edit must differ from it, or the encoder must refuse the layout.
ENCODER_EDITS = (
    ("the name length and the name swapped", _edit(_field("holder_name", 32, array="true") + "\n  " + _field("holder_name_len", 1), _field("holder_name_len", 1) + "\n  " + _field("holder_name", 32, array="true"))),
    ("the key length widened with the key shortened", _edit(_field("key_len", 2) + "\n  " + _field("key", 129, array="true"), _field("key_len", 4) + "\n  " + _field("key", 127, array="true"))),
    ("the version widened", _edit(_field("format_version", 4), _field("format_version", 8))),
    ("the row count and total length swapped", _edit(_field("row_count", 4) + "\n  " + _field("total_len", 4), _field("total_len", 4) + "\n  " + _field("row_count", 4))),
)


def encoder_controls() -> int:
    text = table.SCHEMA.read_text(encoding="utf-8")
    rows = HONEST_TABLE_ROWS
    want = reference(rows)
    refused = 0
    for label, edit in ENCODER_EDITS:
        mutated = edit(text)
        try:
            got = table.encode(rows, mutated)
        except (table.GrantsTableError, KeyError, client.ClientError):
            refused += 1
            continue
        if got == want:
            raise TableExamError(f"control: the encoder wrote the reference bytes under a schema with {label}")
        refused += 1
    return refused


def distinct_controls() -> None:
    """No two controls share a label, and no two corrupt the same input the same way, so the pin counts distinct
    corruptions. A corruption is identified by what it corrupts and the result: two families that edit the same file
    are compared together, so a schema edit and an encoder edit that give the same text are one corruption."""
    text = table.SCHEMA.read_text(encoding="utf-8")
    honest = table.encode(HONEST_TABLE_ROWS)
    grants = GRANTS_SCHEMA.read_text(encoding="utf-8")
    generated = render_bindings()
    lib_edits, test_edits = export_edits()
    # (family, base it corrupts, label, the corrupted value)
    items: list[tuple[str, str, str, object]] = []
    items += [("schema edit", "table schema", label, edit(text)) for label, edit in SCHEMA_EDITS]
    items += [("encoder edit", "table schema", label, edit(text)) for label, edit in ENCODER_EDITS]
    items += [("alphabet edit", "grants schema", label, grants.replace(old, new)) for label, old, new in ALPHABET_EDITS]
    items += [("refused input", "rows", label, repr(rows)) for label, rows in REFUSED_INPUTS]
    items += [("table corruption", "table bytes", label, corrupt(honest)) for label, corrupt in CORRUPTIONS]
    items += [("transcript edit", "transcript", label, "\n".join(lines)) for label, lines in transcript_mutations()]
    items += [("generated-file edit", "generated file", label, value) for label, value in generated_edits(generated)]
    items += [("lib.rs edit", "lib.rs", label, value) for label, value in lib_edits]
    items += [("test edit", "test file", label, value) for label, value in test_edits]
    items += [("probe edit", "probe run", label, "\n".join(lines)) for label, lines in probe_mutations()]
    labels: dict[str, str] = {}
    values: dict[tuple[str, object], tuple[str, str]] = {}
    for family, base, label, value in items:
        _need(label not in labels, f"control: the label {label!r} is used by a {labels.get(label)} and a {family}")
        labels[label] = family
        key = (base, value)
        _need(key not in values, f"control: {family} {label!r} corrupts {base} the same way as {values.get(key, ('', ''))[0]} {values.get(key, ('', ''))[1]!r}")
        values[key] = (family, label)
    _need(len(items) == len(labels) == len(values), "control: the controls are not all distinct")


# The Rust the implementation commits. The generated file holds the layout: lengths, offsets and the two records with
# `encode` and `decode`. The decoder that validates a whole table is written by hand beside it and reads these offsets.
import subprocess  # noqa: E402

import zenoh_client_wire_exam as wire  # noqa: E402

GENERATED = ROOT / "components" / "proto" / "src" / "zenoh_service_grants_table_wire.rs"
GENERATED_HEADER = (
    "// @generated by scripts/generate/generate-zenoh-service-grants-table.py; do not edit.\n"
    "// Source contract: contracts/zenoh-service-grants-table/v1/schema.zt\n"
)
RECORDS = (("header", "headerLayout", "WireGrantsHeader", "HEADER"), ("entry", "entryLayout", "WireGrantsEntry", "ENTRY"))


def render_raw(text: str | None = None) -> str:
    """The Rust file before rustfmt: the constants, then one struct for each record with offsets taken from the layout."""
    text = table.SCHEMA.read_text(encoding="utf-8") if text is None else text
    found = client.constants(text)
    out = [GENERATED_HEADER, "\n"]
    for name in ("formatVersion", "headerBytes", "entryBytes", "maxRows", "holderNameBytes", "maxKeyBytes", "rightPublish", "rightSubscribe", "knownRights"):
        rust = re.sub(r"([A-Z])", r"_\1", name).upper()
        # Sizes and counts index buffers, so they are `usize`. The version and the rights are carried in fields.
        kind = "usize" if name in ("headerBytes", "entryBytes", "maxRows", "holderNameBytes", "maxKeyBytes") else "u32"
        out.append(f"pub const {rust}: {kind} = {found[name]};\n")
    magic = found["tableMagic"].to_bytes(8, "little")
    out.append("pub const TABLE_MAGIC: [u8; 8] = [" + ", ".join(f"0x{byte:02x}" for byte in magic) + "];\n")
    out.append("\n")
    for _word, layout_name, struct_name, prefix in RECORDS:
        fields = client.layout(text, layout_name)
        total = sum(width for _, width, _, _ in fields)
        out.append(f"pub const {prefix}_LEN: usize = {total};\n")
        offset = 0
        for name, width, _, _ in fields:
            out.append(f"pub const OFF_{prefix}_{name.upper()}: usize = {offset};\n")
            offset += width
        out.append("\n#[derive(Debug, Clone, Copy, PartialEq, Eq)]\n")
        out.append(f"pub struct {struct_name} {{\n")
        for name, width, signed, array in fields:
            out.append(f"    pub {name}: {wire.rust_type(width, signed, array)},\n")
        out.append("}\n\n")
        out.append(f"impl {struct_name} {{\n")
        out.append(f"    pub fn decode(buf: &[u8]) -> Option<Self> {{\n        if buf.len() < {prefix}_LEN {{\n            return None;\n        }}\n        Some(Self {{\n")
        for name, width, signed, array in fields:
            out.append(f"            {name}: {wire._decode_expr(prefix, name, width, signed, array)},\n")
        out.append("        })\n    }\n\n")
        out.append(f"    pub fn encode(self) -> [u8; {prefix}_LEN] {{\n        let mut buf = [0u8; {prefix}_LEN];\n")
        for name, width, signed, array in fields:
            out.append(f"        {wire._encode_stmt(prefix, name, width, signed, array)}\n")
        out.append("        buf\n    }\n}\n\n")
    return "".join(out).rstrip("\n") + "\n"


def render_bindings(text: str | None = None) -> str:
    raw = render_raw(text)
    process = subprocess.run(["rustfmt", "--edition", "2024", "--emit", "stdout"], input=raw, text=True, capture_output=True, check=False)
    _need(process.returncode == 0, f"grants table: rustfmt refused the rendered Rust: {process.stderr[:200]}")
    start = process.stdout.find("// @generated")
    _need(start >= 0, "grants table: rustfmt output has no generated header")
    return process.stdout[start:]


# The transcript. The Rust test reads each table as hex from the lines the judge supplies to it through the test file,
# decodes it with the hand-written validating decoder in `slime-proto` and prints a verdict. Honest tables must decode
# to the rows, and each corrupted table must be refused. The verdicts equal what `decode` above says for the same bytes.
def tables() -> list[tuple[str, bytes, list[dict[str, object]] | None]]:
    """Every table the Rust test decodes: its name, its bytes and the rows it must decode to, or None to be refused."""
    out: list[tuple[str, bytes, list[dict[str, object]] | None]] = []
    for name, rows in HONEST:
        written = table.encode(rows)
        # The bytes the transcript carries are the encoder's, so they are compared with the independent writer here,
        # on every path that builds the transcript. An encoder that drifted from the layout cannot print a line.
        _need(written == reference(rows), f"grants table: the encoder's bytes for {name} differ from the reference")
        out.append((f"honest-{name.replace(' ', '-')}", written, table.canonical(rows)))
    honest = table.encode(HONEST_TABLE_ROWS)
    _need(honest == reference(HONEST_TABLE_ROWS), "grants table: the encoder's bytes for the corruption base differ from the reference")
    for label, corrupt in CORRUPTIONS:
        out.append((f"corrupt-{label.replace(' ', '-')}", corrupt(honest), None))
    return out


def verdict(data: bytes, rows: list[dict[str, object]] | None) -> str:
    """What the Python decoder says of `data`: `rows N` with the rows' digest, or `refused`."""
    try:
        found = decode(data)
    except TableExamError:
        return "refused"
    _need(rows is not None and found == rows, "grants table: the decoder accepted a table the exam expects it to refuse")
    return f"rows {len(found)} {row_digest(found)}"


def row_digest(rows: list[dict[str, object]]) -> str:
    """The rows as one line the Rust decoder prints for what it read: holder, key and rights of every row."""
    return ";".join(f"{row['holder']}:{row['key']}:{'p' if row['publish'] else '-'}{'s' if row['subscribe'] else '-'}" for row in rows)


def good_lines() -> list[str]:
    out = []
    for name, data, rows in tables():
        out.append(f"grantstable {name} {data.hex()} {verdict(data, rows)}")
    out.append("grantstable ok every-table-decodes-to-its-rows-or-is-refused")
    return out


# The probe. A test that prints its lines from constants ignores the file it is given, so the slice runs it a second time on
# a file in which one honest table is replaced by another. The probe table is honest and none of the transcript's tables, so
# a test that read the file prints its line and a test that did not cannot. The judge writes the line from its own decoder.
PROBE_NAME = "honest-the-probe"
PROBE_ROWS = [_row("probe", "0/probe/second", False, True), _row("probe", "0/probe/first", True, False)]


def probe_replaces() -> str:
    """The name of the honest table the probe file replaces."""
    return "honest-one-publisher"


def probe_file_text() -> str:
    """The tables file for the second run: the first file with the replaced table swapped for the probe table."""
    out = []
    for name, data, _rows in tables():
        if name == probe_replaces():
            out.append(f"{PROBE_NAME} {table.encode(PROBE_ROWS).hex()}\n")
        else:
            out.append(f"{name} {data.hex()}\n")
    return "".join(out)


def probe_lines() -> list[str]:
    """What a test that reads the file prints on the second run: the first run's lines with the replaced table's line
    swapped for the probe table's line, written from the judge's own encoder and decoder."""
    probe_data = table.encode(PROBE_ROWS)
    probe_rows = table.canonical(PROBE_ROWS)
    out = []
    for line in good_lines():
        fields = line.split(" ")
        if len(fields) > 1 and fields[1] == probe_replaces():
            out.append(f"grantstable {PROBE_NAME} {probe_data.hex()} {verdict(probe_data, probe_rows)}")
        else:
            out.append(line)
    return out


def judge_probe(lines: list[str]) -> None:
    """The second run's lines equal the probe lines, and differ from the first run's in exactly one line."""
    wanted = probe_lines()
    first = good_lines()
    _need(len(wanted) == len(first), "grants table: the probe changes the number of lines")
    changed = [index for index, (a, b) in enumerate(zip(wanted, first, strict=True)) if a != b]
    _need(len(changed) == 1, f"grants table: the probe must change exactly one line, it changes {len(changed)}")
    for index, (seen, want) in enumerate(zip(lines, wanted, strict=False)):
        _need(seen == want, f"grants table probe line {index + 1} is {seen[:90]!r}, the probe requires {want[:90]!r}")
    _need(len(lines) == len(wanted), f"grants table: the probe run printed {len(lines)} lines, the probe requires {len(wanted)}")


def input_file_text() -> str:
    """What the Rust test reads: one table a line, its name and its bytes in hex. The bytes come from here and not from
    the Rust, so the decoder under test never judges bytes it wrote."""
    return "".join(f"{name} {data.hex()}\n" for name, data, _rows in tables())


def judge(lines: list[str]) -> tuple[int, int]:
    """Check a transcript against the exam; returns (cases, refusals). A refusal is a corrupted table the decoder refused."""
    wanted = good_lines()
    shapes: dict[tuple[str, str], set[int]] = {}
    for line in wanted:
        fields = line.split()
        shapes.setdefault((fields[1], fields[-1] if fields[-1] == "refused" else ""), set()).add(len(fields))
    counts = {count for sizes in shapes.values() for count in sizes}
    for line in lines:
        fields = line.split()
        _need(fields[:1] == ["grantstable"], f"grants table: a record that is not a grantstable exam line: {line[:60]!r}")
        _need(len(fields) in counts, f"grants table: a line with {len(fields)} fields: {line[:60]}")
    for index, (seen, want) in enumerate(zip(lines, wanted, strict=False)):
        _need(seen == want, f"grants table line {index + 1} is {seen[:90]!r}, the exam requires {want[:90]!r}")
    if len(lines) < len(wanted):
        raise TableExamError(f"grants table: the transcript stops after {len(lines)} of {len(wanted)} lines; the next is {wanted[len(lines)][:70]!r}")
    if len(lines) > len(wanted):
        raise TableExamError(f"grants table: {len(lines) - len(wanted)} lines beyond the exam, the first {lines[len(wanted)][:70]!r}")
    refusals = sum(1 for line in lines if line.endswith(" refused"))
    return len(lines) - refusals, refusals


def _change_field(line: str, position: int) -> str:
    fields = line.split(" ")
    token = fields[position]
    if re.fullmatch(r"[0-9a-f]{16,}", token):
        fields[position] = token[:-1] + ("0" if token[-1] != "0" else "1")
    else:
        fields[position] = token + "x"
    return " ".join(fields)


def transcript_mutations() -> list[tuple[str, list[str]]]:
    good = good_lines()
    out: list[tuple[str, list[str]]] = [
        ("an empty transcript", []),
        ("a line that is not a grantstable line", [*good, "clientio ok x"]),
        ("a line with an extra field", [*good[:-1], good[-1] + " extra"]),
        ("a line repeated", [*good, good[-1]]),
        ("the last two lines swapped", [*good[:-2], good[-1], good[-2]]),
        ("the first two lines swapped", [good[1], good[0], *good[2:]]),
    ]
    for index, line in enumerate(good):
        out.append((f"line {index + 1} ({line[12:50]}) missing", good[:index] + good[index + 1 :]))
    # Every field of every line, changed alone.
    for index, line in enumerate(good):
        for position in range(len(line.split(" "))):
            changed = _change_field(line, position)
            out.append((f"line {index + 1} field {position + 1} changed", [*good[:index], changed, *good[index + 1 :]]))
    # A wrong decoder, by name: accepts a corrupted table, refuses an honest one, or reads rows wrongly.
    corrupt = next(i for i, line in enumerate(good) if " corrupt-" in line and line.endswith(" refused"))
    honest = next(i for i, line in enumerate(good) if " honest-" in line)
    out.append(("a corrupted table accepted", [*good[:corrupt], good[corrupt].rsplit(" ", 1)[0] + " rows 1 pub:0/slime_demo/extra/second:p-", *good[corrupt + 1 :]]))
    out.append(("an honest table refused", [*good[:honest], good[honest].split(" rows ")[0] + " refused", *good[honest + 1 :]]))
    out.append(("an honest table read with the rights swapped", [*good[:honest], good[honest].replace(":p-", ":-s"), *good[honest + 1 :]]))
    two = next(i for i, line in enumerate(good) if " rows 2 " in line)
    first, second = good[two].split(" rows 2 ")[1].split(";")
    out.append(("an honest table read with its rows reordered", [*good[:two], good[two].split(" rows 2 ")[0] + f" rows 2 {second};{first}", *good[two + 1 :]]))
    return out


def transcript_controls() -> int:
    good = good_lines()
    judge(good)
    refused, seen = 0, {}
    for label, lines in transcript_mutations():
        if lines == good:
            raise TableExamError(f"control: {label} did not change the transcript")
        if tuple(lines) in seen:
            raise TableExamError(f"control: {label} repeats {seen[tuple(lines)]}")
        seen[tuple(lines)] = label
        try:
            judge(lines)
        except TableExamError:
            refused += 1
            continue
        raise TableExamError(f"control: the grants table judge accepted {label}")
    return refused


def probe_mutations() -> list[tuple[str, list[str]]]:
    """Second runs a wrong test would print. The probe judge must refuse each."""
    probe, first = probe_lines(), good_lines()
    changed = next(i for i, (a, b) in enumerate(zip(probe, first, strict=True)) if a != b)
    other = 5
    out: list[tuple[str, list[str]]] = [
        ("a test that prints constants", first),
        ("an empty second run", []),
        ("a second run that stops early", probe[:-1]),
        ("a second run with a line added", [*probe, probe[-1]]),
        ("a second run with the probe line missing", probe[:changed] + probe[changed + 1 :]),
        ("a second run that changes a different line", [*first[:other], first[other].replace("rows", "rowz", 1), *first[other + 1 :]]),
        ("a second run that changes two lines", [*probe[:other], probe[other].rsplit(" ", 1)[0] + " refused", *probe[other + 1 :]]),
        ("a second run that prints the probe line with a wrong verdict", [*probe[:changed], probe[changed].rsplit(" ", 1)[0] + " refused", *probe[changed + 1 :]]),
        ("a second run that prints the probe line with the rows reordered", [*probe[:changed], probe[changed].replace("probe:0/probe/first:p-;probe:0/probe/second:-s", "probe:0/probe/second:-s;probe:0/probe/first:p-"), *probe[changed + 1 :]]),
        ("a second run that prints the probe line with other bytes", [*probe[:changed], probe[changed].replace("honest-the-probe 534c", "honest-the-probe 544c", 1), *probe[changed + 1 :]]),
        ("a second run that prints the probe line under the replaced name", [*probe[:changed], probe[changed].replace("honest-the-probe", probe_replaces(), 1), *probe[changed + 1 :]]),
    ]
    return out


def probe_controls() -> int:
    judge_probe(probe_lines())
    refused, seen = 0, {}
    for label, lines in probe_mutations():
        if lines == probe_lines():
            raise TableExamError(f"control: {label} did not change the probe lines")
        if tuple(lines) in seen:
            raise TableExamError(f"control: {label} repeats {seen[tuple(lines)]}")
        seen[tuple(lines)] = label
        try:
            judge_probe(lines)
        except TableExamError:
            refused += 1
            continue
        raise TableExamError(f"control: the probe judge accepted {label}")
    return refused


def all_controls() -> int:
    distinct_controls()
    return schema_controls() + input_controls() + table_controls() + alphabet_controls() + encoder_controls() + transcript_controls() + generated_controls() + export_controls() + probe_controls()


# The generated file is the renderer's output, byte for byte, so no offset is written by hand.
def check_generated_text(text: str, name: str) -> None:
    expected = render_bindings()
    if text != expected:
        seen, wanted = text.splitlines(keepends=True), expected.splitlines(keepends=True)
        for number in range(max(len(seen), len(wanted))):
            have = seen[number] if number < len(seen) else ""
            want = wanted[number] if number < len(wanted) else ""
            if have != want:
                raise TableExamError(f"grants table: {name} differs from the renderer's output at line {number + 1}: has {have.rstrip()[:50]!r}, wants {want.rstrip()[:50]!r}")
        raise TableExamError(f"grants table: {name} differs from the renderer's output")


def check_generated() -> None:
    if not GENERATED.is_file():
        raise TableExamError(f"{GENERATED.relative_to(ROOT)} does not exist; the grants table has not landed, run `just zenoh_service_grants_table_gen`")
    try:
        text = GENERATED.read_bytes().decode("utf-8")
    except UnicodeDecodeError as error:
        raise TableExamError(f"{GENERATED.name} is not UTF-8 text: {error.reason}") from error
    check_generated_text(text, GENERATED.name)


def generated_edits(honest: str) -> tuple[tuple[str, str], ...]:
    return (
        ("an offset moved", honest.replace("OFF_ENTRY_KEY: usize = 36", "OFF_ENTRY_KEY: usize = 35", 1)),
        ("a length changed", honest.replace("ENTRY_LEN: usize = 168", "ENTRY_LEN: usize = 169", 1)),
        ("the row bound raised in the generated file", honest.replace("MAX_ROWS: usize = 65535", "MAX_ROWS: usize = 65536", 1)),
        ("the key bound raised in the generated file", honest.replace("MAX_KEY_BYTES: usize = 129", "MAX_KEY_BYTES: usize = 130", 1)),
        ("the magic changed in the generated file", honest.replace("0x53, 0x4c", "0x54, 0x4c", 1)),
        ("a field type narrowed", honest.replace("pub key_len: u16", "pub key_len: u8", 1)),
        ("a decode length check dropped", honest.replace("if buf.len() < ENTRY_LEN {", "if false {", 1)),
        ("an encode statement dropped", honest.replace("        buf[OFF_ENTRY_RIGHTS] = self.rights;\n", "", 1)),
        ("the header comment removed", honest.replace("// @generated", "// generated", 1)),
        ("a trailing newline removed", honest.rstrip("\n")),
        ("a line added", honest + "// extra\n"),
        ("an empty file", ""),
    )


def generated_controls() -> int:
    """A generated file that differs from the renderer's output by one edit is refused."""
    honest = render_bindings()
    refused = 0
    for label, text in generated_edits(honest):
        if text == honest:
            raise TableExamError(f"control: {label} did not change the file")
        try:
            check_generated_text(text, "the file")
        except TableExamError:
            refused += 1
            continue
        raise TableExamError(f"control: the generated-file check accepted {label}")
    return refused


# The product crate exports the decoder and the wire records. The Rust test could compile either by path, which passes
# without the export, so the exports are read as text, and the test must reach the decoder through the crate.
PROTO_LIB = ROOT / "components" / "proto" / "src" / "lib.rs"
TABLE_TEST = ROOT / "components" / "proto" / "tests" / "zenoh_service_grants_table.rs"
EXPORT_MODS = ("pub mod zenoh_service_grants_table;", "pub mod zenoh_service_grants_table_wire;")
TEST_IMPORT = "use slime_proto::zenoh_service_grants_table::{Row, Table};"


def check_export_text(text: str) -> None:
    lines = text.splitlines()
    _need(not wire._block_comment(text), "grants table: lib.rs holds a block comment, which could hide an export")
    for mod in EXPORT_MODS:
        at = [index for index, line in enumerate(lines) if line == mod]
        _need(len(at) == 1, f"grants table: lib.rs holds {len(at)} lines `{mod}`, the export needs exactly one")
        _need(at[0] >= 1 and not lines[at[0] - 1].lstrip().startswith("#["), f"grants table: an attribute stands directly above `{mod}`")
    wrappers = [line for line in lines if re.match(r"^(pub(\([a-z]+\))? )?mod \w+ *\{", line)]
    _need(not wrappers, f"grants table: lib.rs opens an inline module at the top level: {wrappers[:1]}")


def check_test_text(text: str) -> None:
    _need(not wire._block_comment(text), "grants table: the test holds a block comment, which could hide its import")
    lines = text.splitlines()
    _need(sum(1 for line in lines if line == TEST_IMPORT) == 1, f"grants table: the test needs exactly one line `{TEST_IMPORT}`")
    _need(not any(re.match(r"^\s*#\[path\b", line) for line in lines), "grants table: the test includes a source file by path")
    _need(not any(re.match(r"^\s*(pub )?mod \w+;", line) for line in lines), "grants table: the test declares a module from a file")
    _need("SLIME_GRANTS_TABLES" in text, "grants table: the test does not read the tables the slice writes")


def check_exports() -> None:
    for path, check in ((PROTO_LIB, check_export_text), (TABLE_TEST, check_test_text)):
        _need(path.is_file(), f"grants table: {path.relative_to(ROOT)} does not exist")
        check(path.read_text(encoding="utf-8"))


_HONEST_LIB = "\n".join(["#![no_std]", "", "pub mod syscall_abi;", EXPORT_MODS[0], EXPORT_MODS[1], "pub mod zenoh_service_grants;", ""])
_HONEST_TEST = "\n".join([TEST_IMPORT, "", "fn tables() { let _ = std::env::var(\"SLIME_GRANTS_TABLES\"); }", ""])


def export_edits() -> tuple[list[tuple[str, str]], list[tuple[str, str]]]:
    lib_edits = []
    for mod in EXPORT_MODS:
        short = mod.split()[-1].rstrip(";")
        lib_edits += [
            (f"{short} removed", _HONEST_LIB.replace(mod + "\n", "")),
            (f"{short} commented out", _HONEST_LIB.replace(mod, "// " + mod)),
            (f"{short} in a block comment", _HONEST_LIB.replace(mod, "/* " + mod + " */")),
            (f"{short} under a cfg attribute", _HONEST_LIB.replace(mod, "#[cfg(test)]\n" + mod)),
            (f"{short} under a never-true attribute", _HONEST_LIB.replace(mod, "#[cfg(any())]\n" + mod)),
            (f"{short} indented", _HONEST_LIB.replace(mod, "    " + mod)),
            (f"{short} in an inline module", _HONEST_LIB.replace(mod, "mod hidden {\n" + mod + "\n}")),
            (f"{short} made private", _HONEST_LIB.replace(mod, mod.replace("pub ", ""))),
            (f"{short} renamed", _HONEST_LIB.replace(mod, mod.replace(";", "s;"))),
            (f"{short} twice", _HONEST_LIB.replace(mod, mod + "\n" + mod)),
        ]
    lib_edits.append(("an empty lib.rs", ""))
    test_edits = [
        ("the import removed", _HONEST_TEST.replace(TEST_IMPORT, "use std::fmt::{Debug};")),
        ("the decoder included by path", '#[path = "../src/zenoh_service_grants_table.rs"]\nmod by_path;\n' + _HONEST_TEST),
        ("a path attribute beside the import", '    #[path = "x.rs"]\n' + _HONEST_TEST),
        ("a module declared from a file", "mod decoder;\n" + _HONEST_TEST),
        ("the import in a block comment", _HONEST_TEST.replace(TEST_IMPORT, "/* " + TEST_IMPORT + " */")),
        ("the import twice", _HONEST_TEST.replace(TEST_IMPORT, TEST_IMPORT + "\n" + TEST_IMPORT)),
        ("the tables file not read", _HONEST_TEST.replace("SLIME_GRANTS_TABLES", "OTHER")),
        ("an empty test", ""),
    ]
    return lib_edits, test_edits


def export_controls() -> int:
    check_export_text(_HONEST_LIB)
    check_test_text(_HONEST_TEST)
    lib_edits, test_edits = export_edits()
    refused = 0
    for edits, honest, check in ((lib_edits, _HONEST_LIB, check_export_text), (test_edits, _HONEST_TEST, check_test_text)):
        for label, text in edits:
            if text == honest:
                raise TableExamError(f"control: {label} did not change the file")
            try:
                check(text)
            except TableExamError:
                refused += 1
                continue
            raise TableExamError(f"control: the grants table export check accepted {label}")
    return refused


# What the admitted item fixes, held here as well so the gate stays correct after the item is completed and retired to
# .tasks/terminal. Both counts are exact.
PINNED_CASES = 40
PINNED_CONTROLS = 367
ADMISSION = "slime-zenoh-service-grants-table-"


def item_pins() -> tuple[int, int]:
    """The case count and the control count the admitted item fixes, read from its stored body, or (0, 0)."""
    for path in sorted((ROOT / ".tasks" / "items").glob("*.md")):
        text = path.read_text(encoding="utf-8")
        if "zenoh_service_grants_table_check" not in text or ADMISSION not in text or re.search(r"^state: cancelled$", text, re.M):
            continue
        found = re.search(r'id = "all-cases-observed";.*?intValue = (\d+);', text, re.S)
        controls = re.search(r'id = "exactly-\d+-controls";.*?observation = "negativeControlsRefused";.*?intValue = (\d+);', text, re.S)
        if found and controls:
            return int(found.group(1)), int(controls.group(1))
    return 0, 0
