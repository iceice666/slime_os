"""Judge `contracts/zenoh-service-client/v1`: the protocol a local holder speaks to the Zenoh service.

The contract is a set of constants and four wire layouts. Nothing decodes a record against it, so this module
reads the schema text and derives every fact the protocol relies on. Each fact is checked against a value
that comes from another contract, so the schema cannot agree with itself and still be wrong:

* a request and a completion fit the IO0 payloads (`contracts/io-queue/v1`);
* each layout adds up to the size the schema declares, and lists the fields of its record type in order;
* no field names a key, a holder, a session or a peer, because the service learns the holder from the
  endpoint a request arrives on;
* the row index is as wide as `maxRows` needs, and the sample header and the largest payload fill the data
  page exactly;
* operations, statuses, items and loan roles are distinct, success is zero and every failure is negative;
* the rows a holder declares in the grants record fit the row index, and its sample size fits the page;
* the Rust constants file the implementation commits is the generator's output, byte for byte.

The judge is trusted only after it refuses a corrupted copy of the schema for each fact.
"""

from __future__ import annotations

import re

from harness import ROOT

SCHEMA = ROOT / "contracts" / "zenoh-service-client" / "v1" / "schema.zt"
IO_QUEUE = ROOT / "contracts" / "io-queue" / "v1" / "schema.zt"
GRANTS = ROOT / "contracts" / "zenoh-service-grants" / "v1" / "schema.zt"

# The words no field may use. A request names a grant row by its index, and the endpoint names the holder.
FORBIDDEN_FIELDS = ("key", "holder", "session", "peer", "link", "topic", "name")
LAYOUTS = {
    "ServiceRequest": ("requestLayout", "requestBytes"),
    "ServiceCompletion": ("completionLayout", "completionBytes"),
    "SampleHeader": ("sampleHeaderLayout", "sampleHeaderBytes"),
    "ServiceLoan": ("loanLayout", "loanBytes"),
}
# The values the contract states, held here so a change to either shows. They are constants of this
# protocol and not of another contract.
# The exact size of each record. A size and its layout can be edited together and still add up, so each size is
# held here and compared after the layout has been summed.
SIZES = {"requestBytes": 32, "completionBytes": 24, "sampleHeaderBytes": 16, "loanBytes": 64}
PINNED = {
    "formatVersion": 1,
    "serviceMagic": int.from_bytes(b"ZSVC", "big"),
    "queueSlots": 4,
    "ringPages": 1,
    "dataPages": 1,
    "ringBytes": 4096,
    "dataBytes": 4096,
    "statusSuccess": 0,
}
OPERATIONS = ("opAttach", "opPublish", "opTake", "opDetach", "opAbort")
STATUSES = ("statusSuccess", "statusDenied", "statusMalformed", "statusUnsupported", "statusTooLarge", "statusLinkClosed")
ITEMS = ("itemNone", "itemSample", "itemLoss")
ROLES = ("loanRoleRing", "loanRoleData")
FLAGS = ("flagPeerSent",)


class ClientError(Exception):
    pass


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise ClientError(message)


def constants(text: str) -> dict[str, int]:
    """Every `name :: Int = n;` at the start of a line. A name stated twice is refused."""
    found: dict[str, int] = {}
    for match in re.finditer(r"^(\w+) :: Int = (-?\d+);", text, re.M):
        name = match.group(1)
        _need(name not in found, f"client: {name} is stated twice")
        found[name] = int(match.group(2))
    return found


def _constant(found: dict[str, int], name: str) -> int:
    _need(name in found, f"client: the schema does not state {name}")
    return found[name]


def layout(text: str, name: str) -> list[tuple[str, int, bool, bool]]:
    block = re.search(rf"^{name} :: List WireField = \{{\n(.*?)^\}};", text, re.M | re.S)
    _need(block is not None, f"client: the schema does not state {name}")
    fields = []
    for line in block.group(1).splitlines():
        entry = re.fullmatch(r'\s*\{ name = "(\w+)"; width = (\d+); signed = (true|false); byteArray = (true|false); \};', line)
        _need(entry is not None, f"client: {name} has a line that is not a field: {line.strip()!r}")
        fields.append((entry.group(1), int(entry.group(2)), entry.group(3) == "true", entry.group(4) == "true"))
    return fields


FIELD = re.compile(r"(\w+)\s*:\s*([^;]+);")


def record_fields(text: str, name: str) -> list[str]:
    """The field names of a record type, in order. A field of any type but `Int` is refused, because only an
    integer has a width in a layout, and so is anything in the body that is not a field or a comment."""
    block = re.search(rf"^{name} :: type \{{\n(.*?)^\}};", text, re.M | re.S)
    _need(block is not None, f"client: the schema does not state the type {name}")
    body = block.group(1)
    fields = FIELD.findall(body)
    leftover = "".join(line for line in FIELD.sub("", body).splitlines() if not line.strip().startswith("--"))
    _need(leftover.strip() == "", f"client: the type {name} has text that is not a field: {leftover.strip()[:40]!r}")
    for field, kind in fields:
        _need(kind.strip() == "Int", f"client: {field} in {name} has the type {kind.strip()!r}, which has no width in a layout")
    return [field for field, _ in fields]


def _distinct(found: dict[str, int], names: tuple[str, ...], label: str) -> None:
    values = [_constant(found, name) for name in names]
    _need(len(set(values)) == len(values), f"client: two {label} share a value")


def io_limits() -> tuple[int, int]:
    text = IO_QUEUE.read_text(encoding="utf-8")
    request = re.search(r"^requestPayloadBytes :: Int = (\d+);", text, re.M)
    completion = re.search(r"^completionPayloadBytes :: Int = (\d+);", text, re.M)
    _need(request is not None and completion is not None, "client: the IO0 contract does not state its payload sizes")
    return int(request.group(1)), int(completion.group(1))


def grants_sample_limit() -> int:
    text = GRANTS.read_text(encoding="utf-8")
    found = re.search(r"^maxSampleBytes :: Int = (\d+);", text, re.M)
    _need(found is not None, "client: the grants contract does not state maxSampleBytes")
    return int(found.group(1))


def servable(sample_bytes: int, rows: int, found: dict[str, int]) -> None:
    """Whether a holder declared with `rows` rows and `sample_bytes` samples can be served by this protocol."""
    _need(0 < rows <= _constant(found, "maxRows"), f"client: {rows} rows do not fit the row index")
    _need(0 < sample_bytes <= _constant(found, "maxSamplePayloadBytes"), f"client: a sample of {sample_bytes} bytes does not fit the data page")


def facts(text: str | None = None) -> list[str]:
    """Every fact the schema states, as one line each; raises `ClientError` for the first that does not hold."""
    text = SCHEMA.read_text(encoding="utf-8") if text is None else text
    found = constants(text)
    out: list[str] = []
    for name, wanted in PINNED.items():
        _need(_constant(found, name) == wanted, f"client: {name} is {found.get(name)}, this version requires {wanted}")
        out.append(f"constant {name} {wanted}")
    request_limit, completion_limit = io_limits()
    sizes: dict[str, int] = {}
    for record, (layout_name, size_name) in LAYOUTS.items():
        fields = layout(text, layout_name)
        declared = _constant(found, size_name)
        total = sum(width for _, width, _, _ in fields)
        _need(total == declared, f"client: {layout_name} adds up to {total} bytes and {size_name} says {declared}")
        names = [name for name, _, _, _ in fields]
        _need(names == record_fields(text, record), f"client: {layout_name} does not list the fields of {record} in order")
        _need(all(width > 0 for _, width, _, _ in fields), f"client: {layout_name} has a field of no width")
        _need(len(set(names)) == len(names), f"client: {layout_name} names a field twice")
        for field, _width, signed, array in fields:
            _need(not any(word in field.lower() for word in FORBIDDEN_FIELDS), f"client: {layout_name} has a field named {field}, which names what the endpoint decides")
            if field.startswith("reserved"):
                _need(array and not signed, f"client: {field} in {layout_name} is not an unsigned byte array")
            else:
                _need(not array, f"client: {field} in {layout_name} is a byte array but is not reserved")
        sizes[size_name] = declared
        out.append(f"layout {record} {declared}")
    _need(sizes["requestBytes"] <= request_limit, f"client: a request of {sizes['requestBytes']} bytes exceeds the IO0 payload of {request_limit}")
    _need(sizes["completionBytes"] <= completion_limit, f"client: a completion of {sizes['completionBytes']} bytes exceeds the IO0 payload of {completion_limit}")
    out.append(f"fits-io0 {sizes['requestBytes']} {sizes['completionBytes']}")
    for size_name, wanted in SIZES.items():
        _need(sizes[size_name] == wanted, f"client: {size_name} is {sizes[size_name]}, this version requires {wanted}")
    out.append("sizes " + " ".join(str(SIZES[name]) for name in SIZES))
    request = {name: (width, signed) for name, width, signed, _ in layout(text, "requestLayout")}
    completion = {name: (width, signed) for name, width, signed, _ in layout(text, "completionLayout")}
    header = {name: (width, signed) for name, width, signed, _ in layout(text, "sampleHeaderLayout")}
    _need(request["sequence"] == (8, True), "client: the request's sequence is not a signed 8-byte field")
    _need(header["sequence"] == (8, True) and header["timestamp"] == (8, True), "client: the sample header is not two signed 8-byte fields")
    _need(completion["status"] == (4, True), "client: the completion's status is not a signed 4-byte field")
    out.append("signed sequence timestamp status")
    row_width = request["row"][0]
    _need(completion["row"][0] == row_width, "client: the request and the completion carry the row in different widths")
    _need(_constant(found, "maxRows") == 256**row_width - 1, f"client: maxRows is not what a {row_width}-byte row index holds")
    out.append(f"rows {found['maxRows']}")
    _need(_constant(found, "maxSamplePayloadBytes") == found["dataBytes"] - found["sampleHeaderBytes"], "client: the largest payload and the sample header do not fill the data page")
    _need(found["maxSamplePayloadBytes"] <= grants_sample_limit(), "client: the largest payload exceeds what the grants contract allows")
    out.append(f"data-page {found['sampleHeaderBytes']} {found['maxSamplePayloadBytes']}")
    for label, names in (("operations", OPERATIONS), ("statuses", STATUSES), ("items", ITEMS), ("loan roles", ROLES)):
        _distinct(found, names, label)
        out.append(f"distinct {label} {len(names)}")
    _need(all(found[name] > 0 for name in OPERATIONS), "client: an operation is not positive")
    _need(all(found[name] < 0 for name in STATUSES[1:]), "client: a failure status is not negative")
    _need(found["itemNone"] == 0 and found["itemSample"] > 0 and found["itemLoss"] > 0, "client: the items are not none, then two positive values")
    out.append("signs operations statuses items")
    _need(found["knownCompletionFlags"] == sum(found[name] for name in FLAGS), "client: knownCompletionFlags is not the sum of the flags")
    _need(all(found[name] & (found[name] - 1) == 0 and found[name] > 0 for name in FLAGS), "client: a flag is not one bit")
    out.append("flags known")
    _need(all(found[name] <= 65535 for name in ("queueSlots",)) and found["queueSlots"] > 0, "client: the queue has no slots")
    servable(12, 5, found)
    out.append("servable 12 5")
    return out


def _edit(old: str, new: str, count: int = 1):
    def apply(text: str) -> str:
        _need(text.count(old) == count, f"control: {old!r} found {text.count(old)} times, expected {count}")
        return text.replace(old, new)

    return apply


def mutations() -> list[tuple[str, object]]:
    pinned = [(f"{name} changed", _edit(f"{name} :: Int = {value};", f"{name} :: Int = {value + 1};")) for name, value in PINNED.items() if name != "statusSuccess"]
    pinned.append(("statusSuccess made nonzero", _edit("statusSuccess :: Int = 0;", "statusSuccess :: Int = 1;")))
    out = [
        *pinned,
        ("a request size that the layout does not add up to", _edit("requestBytes :: Int = 32;", "requestBytes :: Int = 33;")),
        ("a completion size that the layout does not add up to", _edit("completionBytes :: Int = 24;", "completionBytes :: Int = 23;")),
        ("a sample header size that the layout does not add up to", _edit("sampleHeaderBytes :: Int = 16;", "sampleHeaderBytes :: Int = 17;")),
        ("a loan size that the layout does not add up to", _edit("loanBytes :: Int = 64;", "loanBytes :: Int = 65;")),
        ("a request wider than the IO0 payload", lambda t: _edit("requestBytes :: Int = 32;", "requestBytes :: Int = 64;")(_edit('{ name = "reserved2"; width = 12;', '{ name = "reserved2"; width = 44;')(t))),
        ("a completion wider than the IO0 payload", lambda t: _edit("completionBytes :: Int = 24;", "completionBytes :: Int = 32;")(_edit('{ name = "flags"; width = 4; signed = false; byteArray = false; };\n};\n\nsampleHeaderLayout', '{ name = "flags"; width = 12; signed = false; byteArray = false; };\n};\n\nsampleHeaderLayout')(t))),
        ("a request layout that drops the row", _edit('  { name = "row"; width = 2; signed = false; byteArray = false; };\n  { name = "reserved"; width = 2; signed = false; byteArray = true; };\n  { name = "sequence"', '  { name = "reserved"; width = 2; signed = false; byteArray = true; };\n  { name = "sequence"')),
        ("two request fields swapped", _edit('  { name = "op"; width = 1; signed = false; byteArray = false; };\n  { name = "flags"; width = 1; signed = false; byteArray = false; };\n  { name = "row"', '  { name = "flags"; width = 1; signed = false; byteArray = false; };\n  { name = "op"; width = 1; signed = false; byteArray = false; };\n  { name = "row"')),
        ("a layout field renamed and its record type left", _edit('{ name = "length"; width = 4; signed = false; byteArray = false; };\n  { name = "flags"; width = 4;', '{ name = "size"; width = 4; signed = false; byteArray = false; };\n  { name = "flags"; width = 4;')),
        ("a request field that names a key", lambda t: _edit("  row : Int;\n  reserved : Int;\n  sequence : Int;", "  key : Int;\n  reserved : Int;\n  sequence : Int;")(_edit('{ name = "row"; width = 2; signed = false; byteArray = false; };\n  { name = "reserved"; width = 2; signed = false; byteArray = true; };\n  { name = "sequence"', '{ name = "key"; width = 2; signed = false; byteArray = false; };\n  { name = "reserved"; width = 2; signed = false; byteArray = true; };\n  { name = "sequence"')(t))),
        ("a request field that names a holder", lambda t: _edit("  sequence : Int;\n  reserved2 : Int;", "  holder : Int;\n  reserved2 : Int;")(_edit('{ name = "sequence"; width = 8; signed = true; byteArray = false; };\n  { name = "reserved2"', '{ name = "holder"; width = 8; signed = true; byteArray = false; };\n  { name = "reserved2"')(t))),
        ("a request sequence that is unsigned", _edit('{ name = "sequence"; width = 8; signed = true; byteArray = false; };\n  { name = "reserved2"', '{ name = "sequence"; width = 8; signed = false; byteArray = false; };\n  { name = "reserved2"')),
        ("a completion status that is unsigned", _edit('{ name = "status"; width = 4; signed = true; byteArray = false; };', '{ name = "status"; width = 4; signed = false; byteArray = false; };')),
        ("a sample timestamp that is unsigned", _edit('{ name = "timestamp"; width = 8; signed = true; byteArray = false; };', '{ name = "timestamp"; width = 8; signed = false; byteArray = false; };')),
        ("a reserved field that is not a byte array", _edit('{ name = "reserved2"; width = 12; signed = false; byteArray = true; };', '{ name = "reserved2"; width = 12; signed = false; byteArray = false; };')),
        ("a named field that is a byte array", _edit('{ name = "op"; width = 1; signed = false; byteArray = false; };\n  { name = "flags"; width = 1; signed = false; byteArray = false; };', '{ name = "op"; width = 1; signed = false; byteArray = false; };\n  { name = "flags"; width = 1; signed = false; byteArray = true; };')),
        ("a request row narrower than the completion row", lambda t: _edit('{ name = "row"; width = 2; signed = false; byteArray = false; };\n  { name = "reserved"; width = 2; signed = false; byteArray = true; };\n  { name = "sequence"', '{ name = "row"; width = 1; signed = false; byteArray = false; };\n  { name = "reserved"; width = 3; signed = false; byteArray = true; };\n  { name = "sequence"')(t)),
        ("a row limit past the row index", _edit("maxRows :: Int = 65535;", "maxRows :: Int = 65536;")),
        ("a row limit below the row index", _edit("maxRows :: Int = 65535;", "maxRows :: Int = 65534;")),
        ("a largest payload that overflows the data page", _edit("maxSamplePayloadBytes :: Int = 4080;", "maxSamplePayloadBytes :: Int = 4081;")),
        ("a largest payload that leaves the data page unfilled", _edit("maxSamplePayloadBytes :: Int = 4080;", "maxSamplePayloadBytes :: Int = 4079;")),
        ("two operations that share a value", _edit("opTake :: Int = 3;", "opTake :: Int = 2;")),
        ("an operation of zero", _edit("opAttach :: Int = 1;", "opAttach :: Int = 0;")),
        ("two statuses that share a value", _edit("statusTooLarge :: Int = -4;", "statusTooLarge :: Int = -3;")),
        ("a failure status that is positive", _edit("statusLinkClosed :: Int = -5;", "statusLinkClosed :: Int = 5;")),
        ("two items that share a value", _edit("itemLoss :: Int = 2;", "itemLoss :: Int = 1;")),
        ("an item of none that is not zero", _edit("itemNone :: Int = 0;", "itemNone :: Int = 3;")),
        ("two loan roles that share a value", _edit("loanRoleData :: Int = 2;", "loanRoleData :: Int = 1;")),
        ("known flags that are not the flags", _edit("knownCompletionFlags :: Int = 1;", "knownCompletionFlags :: Int = 3;")),
        ("a flag of more than one bit", lambda t: _edit("flagPeerSent :: Int = 1;", "flagPeerSent :: Int = 3;")(_edit("knownCompletionFlags :: Int = 1;", "knownCompletionFlags :: Int = 3;")(t))),
        ("a queue of no slots", _edit("queueSlots :: Int = 4;", "queueSlots :: Int = 0;")),
        ("a constant stated twice", lambda t: t + "opTake :: Int = 9;\n"),
        ("a constant commented out", _edit("opAbort :: Int = 5;", "-- opAbort :: Int = 5;")),
        ("a layout missing", _edit("sampleHeaderLayout :: List WireField = {", "sampleHeaderLayoutGone :: List WireField = {")),
        ("a record type missing", _edit("SampleHeader :: type {", "SampleHeaderGone :: type {")),
        ("a layout line that is not a field", _edit('  { name = "timestamp"; width = 8; signed = true; byteArray = false; };\n};', '  { name = "timestamp"; width = 8; signed = true; byteArray = false; }\n};')),
        ("a field of no width", lambda t: _edit('{ name = "reserved"; width = 2; signed = false; byteArray = true; };\n  { name = "sequence"; width = 8; signed = true; byteArray = false; };\n  { name = "reserved2"; width = 12;', '{ name = "reserved"; width = 0; signed = false; byteArray = true; };\n  { name = "sequence"; width = 8; signed = true; byteArray = false; };\n  { name = "reserved2"; width = 14;')(t)),
        ("a Text field added to the request type", _edit("  reserved2 : Int;\n};", "  reserved2 : Int;\n  key : Text;\n};")),
        ("a Bool field added to the completion type", _edit("  flags : Int;\n};\n\n-- The first bytes", "  flags : Int;\n  done : Bool;\n};\n\n-- The first bytes")),
        ("a field type of List Int in the sample header", _edit("  timestamp : Int;\n};", "  timestamp : List Int;\n};")),
        ("text in a record type that is not a field", _edit("  reserved2 : Int;\n};", "  reserved2 : Int;\n  junk\n};")),
        ("a request of 31 bytes with its layout cut to match", lambda t: _edit("requestBytes :: Int = 32;", "requestBytes :: Int = 31;")(_edit('{ name = "reserved2"; width = 12;', '{ name = "reserved2"; width = 11;')(t))),
        ("a completion of 23 bytes with its layout cut to match", lambda t: _edit("completionBytes :: Int = 24;", "completionBytes :: Int = 23;")(_edit('{ name = "flags"; width = 4; signed = false; byteArray = false; };\n};\n\nsampleHeaderLayout', '{ name = "flags"; width = 3; signed = false; byteArray = false; };\n};\n\nsampleHeaderLayout')(t))),
        ("a sample header of 15 bytes with its layout cut to match", lambda t: _edit("sampleHeaderBytes :: Int = 16;", "sampleHeaderBytes :: Int = 15;")(_edit('{ name = "timestamp"; width = 8; signed = true; byteArray = false; };\n};\n\nloanLayout', '{ name = "timestamp"; width = 7; signed = true; byteArray = false; };\n};\n\nloanLayout')(t))),
        ("a loan of 63 bytes with its layout cut to match", lambda t: _edit("loanBytes :: Int = 64;", "loanBytes :: Int = 63;")(_edit('{ name = "reserved"; width = 32; signed = false; byteArray = true; };', '{ name = "reserved"; width = 31; signed = false; byteArray = true; };')(t))),
        ("a field named twice", lambda t: _edit('  { name = "reserved2"; width = 12; signed = false; byteArray = true; };', '  { name = "reserved"; width = 12; signed = false; byteArray = true; };')(_edit("  reserved2 : Int;\n};", "  reserved : Int;\n};")(t))),
    ]
    return out


def servable_controls() -> int:
    """A holder that the protocol cannot serve is refused for that reason."""
    found = constants(SCHEMA.read_text(encoding="utf-8"))
    refused = 0
    for label, sample, rows in (
        ("a sample of one byte over the data page", found["maxSamplePayloadBytes"] + 1, 5),
        ("a sample of no bytes", 0, 5),
        ("a holder of no rows", 12, 0),
        ("a holder of one row over the row index", 12, found["maxRows"] + 1),
    ):
        try:
            servable(sample, rows, found)
        except ClientError:
            refused += 1
            continue
        raise ClientError(f"control: the client judge served {label}")
    servable(found["maxSamplePayloadBytes"], found["maxRows"], found)
    return refused


def schema_controls() -> int:
    honest = SCHEMA.read_text(encoding="utf-8")
    facts(honest)
    refused = 0
    for label, apply in mutations():
        changed = apply(honest)
        if changed == honest:
            raise ClientError(f"control: {label} did not change the schema")
        try:
            facts(changed)
        except ClientError as error:
            if str(error).startswith("control:"):
                raise
            refused += 1
            continue
        raise ClientError(f"control: the client judge accepted {label}")
    return refused


GENERATED = ROOT / "components" / "proto" / "src" / "zenoh_service_client.rs"
GENERATED_HEADER = (
    "// @generated by scripts/generate/generate-zenoh-service-client-bindings.py; do not edit.\n"
    "// Source contract: contracts/zenoh-service-client/v1/schema.zt\n"
)


def _rust_name(name: str) -> str:
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).upper()


def render_bindings(text: str | None = None) -> str:
    """The generated Rust constants file: every integer constant of the schema, in the schema's order."""
    text = SCHEMA.read_text(encoding="utf-8") if text is None else text
    return GENERATED_HEADER + "\n" + "".join(f"pub const {_rust_name(name)}: i64 = {value};\n" for name, value in constants(text).items())


def read_exact(path) -> str:
    """The file's text with every byte kept, so a CRLF or a lone CR is seen and not translated to LF."""
    try:
        return path.read_bytes().decode("utf-8")
    except UnicodeDecodeError as error:
        raise ClientError(f"{path.name} is not UTF-8 text: {error.reason}") from error


def check_generated_text(text: str, name: str) -> int:
    """The file equals the generator's output byte for byte; returns the number of constants it holds."""
    expected = render_bindings()
    if text != expected:
        seen, wanted = text.splitlines(keepends=True), expected.splitlines(keepends=True)
        for number in range(max(len(seen), len(wanted))):
            have = seen[number] if number < len(seen) else ""
            want = wanted[number] if number < len(wanted) else ""
            if have != want:
                raise ClientError(f"{name} differs from the generator's output at line {number + 1}: has {have.rstrip()[:60]!r}, wants {want.rstrip()[:60]!r}")
        raise ClientError(f"{name} differs from the generator's output")
    return expected.count("pub const ")


def check_generated() -> int:
    """The Rust file the implementation commits is the generator's output."""
    if not GENERATED.is_file():
        raise ClientError(f"{GENERATED.relative_to(ROOT)} does not exist; the client bindings have not landed, run `just zenoh_service_client_gen`")
    return check_generated_text(read_exact(GENERATED), GENERATED.name)


def generated_controls() -> int:
    """A generated file that differs from the generator's output by one edit is refused."""
    honest = render_bindings()
    check_generated_text(honest, "honest")
    lines = honest.splitlines(keepends=True)
    edits = [
        ("a constant's value changed", honest.replace("MAX_ROWS: i64 = 65535", "MAX_ROWS: i64 = 65534")),
        ("a constant removed", "".join(line for line in lines if "OP_ABORT" not in line)),
        ("a constant added", honest + "pub const EXTRA: i64 = 1;\n"),
        ("two constants swapped", "".join(lines[:2] + [lines[3], lines[2]] + lines[4:])),
        ("a type changed", honest.replace(": i64 =", ": u32 =", 1)),
        ("the header removed", "".join(lines[2:])),
        ("a comment appended", honest + "// a comment\n"),
        ("a line ending changed to CRLF", honest.replace("\n", "\r\n")),
        ("the trailing newline removed", honest.rstrip("\n")),
        ("an empty file", ""),
    ]
    refused = 0
    for label, text in edits:
        if text == honest:
            raise ClientError(f"control: {label} did not change the generated text")
        try:
            check_generated_text(text, "mutated")
        except ClientError:
            refused += 1
            continue
        raise ClientError(f"control: the client judge accepted a generated file with {label}")
    return refused


def all_controls() -> int:
    return schema_controls() + servable_controls() + generated_controls()


# What the admitted item fixes, held here as well so the gate stays correct after the item is completed
# and retired to .tasks/terminal. Both counts are exact.
PINNED_CASES = 24
PINNED_CONTROLS = 68
ADMISSION = "slime-zenoh-service-client-"


def item_pins() -> tuple[int, int]:
    """The case count and the control count the admitted item fixes, read from its stored body, or (0, 0)."""
    for path in sorted((ROOT / ".tasks" / "items").glob("*.md")):
        text = path.read_text(encoding="utf-8")
        if "zenoh_service_client_check" not in text or ADMISSION not in text or re.search(r"^state: cancelled$", text, re.M):
            continue
        found = re.search(r'id = "all-cases-observed";.*?intValue = (\d+);', text, re.S)
        controls = re.search(r'id = "exactly-\d+-controls";.*?observation = "negativeControlsRefused";.*?intValue = (\d+);', text, re.S)
        if found and controls:
            return int(found.group(1)), int(controls.group(1))
    return 0, 0
