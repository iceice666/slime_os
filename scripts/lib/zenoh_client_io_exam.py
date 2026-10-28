"""Judge the Zenoh service client adapter from what the Rust tests print.

The adapter (`components/lib/src/zenoh_service_client.rs`) speaks the protocol of `contracts/zenoh-service-client/v1`
over one explicitly provisioned IO0 ring. The Rust test `the_client_exam` plays the service's end of the ring and the
control endpoint with a scripted stand-in runtime, and prints one `[zenoh-exam] clientio ...` line for each thing the
adapter did. This module owns what those lines must be, and nothing here reads the Rust:

* the bytes of every request the adapter wrote, which `struct` writes again from the schema's layout;
* the IO0 slice each request used: its direction, its length and the lease it named;
* each refusal, by name and class, and what the adapter did with the session afterwards;
* the loans: an answer outside the protocol returns both, and every loan the adapter imported is returned exactly
  once on a failed attach, whatever the failure was.

Like the other judges it is trusted only after it refuses a mutation of an honest transcript.
"""

from __future__ import annotations

import re

import zenoh_client_exam as client
import zenoh_client_wire_exam as wire_exam
from harness import ROOT

# The four request shapes the adapter writes, from the exam's scripted values. A control request carries only an
# operation. A publish carries the row and the holder's sequence number.
OP = {"attach": 1, "publish": 2, "take": 3, "detach": 4, "abort": 5}
PUBLISH_ROW = 2
PUBLISH_SEQUENCE = -7
PUBLISH_PAYLOAD = bytes([1, 2, 3])
TAKE_BUFFER = 16
TAKE_ROW, TAKE_SEQUENCE, TAKE_TIMESTAMP, TAKE_PAYLOAD = 1, 41, -9, bytes([7, 8, 9, 10])
LOSS = 16
HEADER_BYTES = 16
STATUS_DENIED, STATUS_LINK_CLOSED, STATUS_SUCCESS = -1, -5, 0
# The service's data page: the page the adapter maps, and the largest payload that page leaves after the header.
MAX_PAYLOAD = 4080

# IO0 directions, from `contracts/io-queue/v1`.
DIRECTION_DEVICE_READ, DIRECTION_DEVICE_WRITE = 1, 2
# The identities the exam gives the data loan. A request that names the data page names these two.
DATA_BUFFER, DATA_LEASE = 3, 4


class ClientIoError(Exception):
    pass


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise ClientIoError(message)


def request_bytes(op: int, row: int = 0, sequence: int = 0, text: str | None = None) -> bytes:
    """A request, written with `struct` from the schema's layout and nothing generated."""
    values = {
        "magic": client.PINNED["serviceMagic"], "version": 1, "op": op, "flags": 0, "row": row,
        "reserved": bytes(2), "sequence": sequence, "reserved2": bytes(12),
    }
    return wire_exam.pack("ServiceRequest", values, text=text)


def _loan_cases() -> list[str]:
    # The honest descriptor is valid. Each corruption of one field is refused, and the data-role descriptor is
    # valid only for the data role.
    out = ["clientio loan honest valid"]
    for name in ("wrong-magic", "wrong-version", "wrong-role", "reserved0-set", "reserved-set", "wrong-length", "zero-buffer", "zero-lease"):
        out.append(f"clientio loan {name} refused")
    out.append("clientio loan data-role checked")
    return out


# An attach that fails after its loans arrived returns exactly the loans it imported. A loan is imported once the
# descriptor is valid, so a descriptor refused before import returns nothing.
ATTACH_REFUSALS = (
    # (name, class, loans imported)
    ("one-endpoint-for-both", "BadRequest", None),
    ("an-unaligned-ring-base", "BadRequest", None),
    ("an-unaligned-data-base", "BadRequest", None),
    ("the-same-base-twice", "BadRequest", None),
    ("a-zero-ring-base", "BadRequest", None),
    ("a-ring-base-that-wraps", "BadRequest", None),
)
ATTACH_FAILURES = (
    ("data-before-ring", "Malformed", 0),
    ("one-buffer-for-both", "Malformed", 2),
    ("one-lease-for-both", "Malformed", 2),
    ("no-rows", "Malformed", 2),
    ("more-rows-than-the-index-holds", "Malformed", 2),
    ("an-attach-the-service-refused", "Setup", 0),
    ("no-attach-answer", "Lost", 2),
    ("no-loans-at-all", "Lost", 0),
)
PUBLISH_MALFORMED = (
    "wrong-row", "wrong-operation", "an-item-on-a-publish", "short-transfer", "an-unknown-flag",
    "a-status-below-the-set", "a-wrong-magic", "a-wrong-version", "a-nonzero-reserved", "a-failure-carrying-a-count",
)
TAKE_MALFORMED = (
    "a-sample-from-a-row-the-holder-lacks", "a-sample-longer-than-the-buffer",
    "a-consistent-sample-longer-than-the-buffer", "a-sample-whose-transfer-does-not-match",
    "a-loss-of-zero", "a-loss-that-moved-bytes", "an-empty-ring-with-a-row", "a-take-failure-carrying-a-count",
    "a-take-with-a-wrong-magic", "a-take-with-a-wrong-version", "a-flag-on-a-take",
)
REFUSED_BEFORE_PUBLISH = (
    ("row-past-the-holders-rows", "BadRequest"),
    ("payload-over-the-data-page", "BadRequest"),
    ("take-buffer-over-the-data-page", "BadRequest"),
)


def good_lines(text: str | None = None) -> list[str]:
    """The honest transcript, in the order the Rust test prints it, for the layouts in `text`."""
    out = _loan_cases()
    # Attach refused before anything is sent.
    for name, error, _ in ATTACH_REFUSALS:
        out.append(f"clientio attach-refused {name} {error}")
    # Attach refused after the service answered: the loans imported are the loans returned.
    for name, error, returned in ATTACH_FAILURES:
        out.append(f"clientio attach-refused {name} {error} returns {returned}")
    out.append("clientio attach-refused unformatted-ring Malformed returns 2")
    out.append("clientio attach rows 3 imports 2")
    out.append(f"clientio attach request {request_bytes(OP['attach'], text=text).hex()}")
    # Publish.
    out.append(f"clientio publish reply {STATUS_SUCCESS} 2 true")
    out.append(f"clientio publish request {request_bytes(OP['publish'], PUBLISH_ROW, PUBLISH_SEQUENCE, text).hex()}")
    out.append(f"clientio publish slice 0 {len(PUBLISH_PAYLOAD)} {DIRECTION_DEVICE_READ} {DATA_BUFFER} {DATA_LEASE} {PUBLISH_PAYLOAD.hex()}")
    # Take.
    out.append(f"clientio take sample {TAKE_ROW} {TAKE_SEQUENCE} {TAKE_TIMESTAMP} {len(TAKE_PAYLOAD)} {TAKE_PAYLOAD.hex()}")
    out.append(f"clientio take request {request_bytes(OP['take'], text=text).hex()}")
    out.append(f"clientio take slice 0 {HEADER_BYTES + TAKE_BUFFER} {DIRECTION_DEVICE_WRITE} {DATA_BUFFER} {DATA_LEASE}")
    out.append(f"clientio take loss {LOSS}")
    out.append("clientio take none")
    # Refused before publishing: nothing is queued.
    for name, error in REFUSED_BEFORE_PUBLISH:
        out.append(f"clientio refused {name} {error}")
    out.append(f"clientio take largest-buffer {STATUS_SUCCESS}")
    # A service refusal is a status. The session stays usable.
    out.append(f"clientio status {STATUS_DENIED} {STATUS_LINK_CLOSED} {STATUS_SUCCESS}")
    # An answer outside the protocol poisons the session and returns both loans.
    for name in PUBLISH_MALFORMED:
        out.append(f"clientio malformed {name} Malformed true 2")
    for name in TAKE_MALFORMED:
        out.append(f"clientio malformed {name} Malformed true 2")
    out.append("clientio lost publish Lost true 2")
    out.append("clientio lost later-requests Lost")
    # Teardown.
    out.append("clientio teardown detach ok returns 2 before-send [2] sent 1 unread 0")
    out.append(f"clientio teardown request {request_bytes(OP['detach'], text=text).hex()}")
    out.append("clientio teardown abort ok returns 2 before-send [2] sent 1 unread 0")
    out.append(f"clientio teardown request {request_bytes(OP['abort'], text=text).hex()}")
    out.append("clientio teardown unacknowledged Lost returns 2")
    out.append("clientio teardown wrong-acknowledgement Malformed")
    out.append("clientio poisoned finish Lost")
    out.append("clientio poisoned abort ok")
    out.append("clientio ok every-request-is-bounded-and-every-answer-is-checked")
    return out


def shapes() -> dict[str, set[int]]:
    """The field counts each kind of line has in the honest transcript. A line of another count is not a line of
    the exam, whatever its words say, and is refused before the transcript is compared."""
    found: dict[str, set[int]] = {}
    for line in good_lines():
        fields = line.split()
        found.setdefault(fields[1], set()).add(len(fields))
    return found


def judge(lines: list[str], text: str | None = None) -> tuple[int, int]:
    """Check a transcript against the exam for the layouts in `text`; returns (cases, refusals). A refusal is a line
    that names an error."""
    known = shapes()
    for line in lines:
        fields = line.split()
        _need(fields[:1] == ["clientio"], f"clientio: a record that is not a clientio exam line: {line!r}")
        _need(len(fields) > 1 and fields[1] in known, f"clientio: unrecognised line: {line}")
        _need(len(fields) in known[fields[1]], f"clientio: a {fields[1]} line has {len(fields)} fields: {line}")
    wanted = good_lines(text)
    for index, (seen, want) in enumerate(zip(lines, wanted, strict=False)):
        _need(seen == want, f"clientio line {index + 1} is {seen!r}, the exam requires {want!r}")
    if len(lines) < len(wanted):
        raise ClientIoError(f"clientio: the transcript stops after {len(lines)} of {len(wanted)} lines; the next is {wanted[len(lines)]!r}")
    if len(lines) > len(wanted):
        raise ClientIoError(f"clientio: {len(lines) - len(wanted)} lines beyond the exam, the first {lines[len(wanted)]!r}")
    # A refusal is a line that names the error the adapter returned for a request it did not accept: a request
    # refused before it was published, an attach refused, or an answer outside the protocol.
    kinds = ("clientio refused ", "clientio attach-refused ", "clientio malformed ", "clientio lost ")
    refusals = sum(1 for line in lines if line.startswith(kinds))
    return len(lines) - refusals, refusals


def _change_last(line: str) -> str:
    head, last = line.rsplit(" ", 1)
    if re.fullmatch(r"[0-9a-f]+", last) and len(last) > 4:
        return f"{head} {last[:-1]}{'0' if last[-1] != '0' else '1'}"
    return f"{head} {last}x"


def _label(index: int, line: str) -> str:
    return f"line {index + 1} ({line[9:60]})"


def transcript_mutations() -> list[tuple[str, list[str]]]:
    good = good_lines()
    out: list[tuple[str, list[str]]] = [
        ("an empty transcript", []),
        ("a line that is not a clientio exam line", [*good, "service ok static-declaration-at-open"]),
        ("an unrecognised kind of line", [*good, "clientio dropped request"]),
        ("a line with an extra field", [*good[:-1], good[-1] + " extra"]),
        ("a line repeated", [*good, good[-1]]),
        ("the last line missing", good[:-1]),
        ("the first two lines swapped", [good[1], good[0], *good[2:]]),
    ]
    for index, line in enumerate(good):
        out.append((f"{_label(index, line)} missing", good[:index] + good[index + 1 :]))
    for index, line in enumerate(good):
        out.append((f"{_label(index, line)} with its last field changed", [*good[:index], _change_last(line), *good[index + 1 :]]))
    return out


def transcript_controls() -> int:
    good = good_lines()
    judge(good)
    refused = 0
    for label, lines in transcript_mutations():
        if lines == good:
            raise ClientIoError(f"control: {label} did not change the transcript")
        try:
            judge(lines)
        except ClientIoError:
            refused += 1
            continue
        raise ClientIoError(f"control: the clientio judge accepted {label}")
    return refused


def reference_controls() -> int:
    """The request bytes come from the schema's layout. For each corrupted layout of the request, the transcript an
    adapter of the real layout prints is refused, because the judge now requires the bytes of the corrupted one."""
    text = client.SCHEMA.read_text(encoding="utf-8")
    honest = good_lines()
    refused = 0
    edits = [
        ("the request op widened", '{ name = "op"; width = 1; signed = false; byteArray = false; };\n  { name = "flags"; width = 1; signed = false; byteArray = false; };\n  { name = "row"', '{ name = "op"; width = 2; signed = false; byteArray = false; };\n  { name = "flags"; width = 1; signed = false; byteArray = false; };\n  { name = "row"'),
        ("the request sequence made unsigned", '{ name = "sequence"; width = 8; signed = true; byteArray = false; };\n  { name = "reserved2"', '{ name = "sequence"; width = 8; signed = false; byteArray = false; };\n  { name = "reserved2"'),
        ("the request row moved after the sequence", '{ name = "row"; width = 2; signed = false; byteArray = false; };\n  { name = "reserved"; width = 2; signed = false; byteArray = true; };\n  { name = "sequence"; width = 8; signed = true; byteArray = false; };', '{ name = "reserved"; width = 2; signed = false; byteArray = true; };\n  { name = "sequence"; width = 8; signed = true; byteArray = false; };\n  { name = "row"; width = 2; signed = false; byteArray = false; };'),
    ]
    for label, old, new in edits:
        block = re.search(r"^requestLayout :: List WireField = \{\n(.*?)^\};", text, re.M | re.S)
        if block is None or block.group(0).count(old) != 1:
            raise ClientIoError(f"control: {label} anchor is not in requestLayout exactly once")
        mutated = text.replace(block.group(0), block.group(0).replace(old, new, 1))
        try:
            judge(honest, mutated)
        except (ClientIoError, wire_exam.WireError, KeyError):
            refused += 1
            continue
        raise ClientIoError(f"control: the clientio judge accepted the honest transcript under a schema with {label}")
    return refused


LIB = ROOT / "components" / "lib" / "src" / "lib.rs"
EXPORT_CFG = '#[cfg(feature = "component-runtime")]'
EXPORT_MOD = "pub mod zenoh_service_client;"


def check_export_text(text: str) -> None:
    """The product crate exports the adapter. The integration test compiles the adapter by path, so it passes
    without this export, and a consumer of `slime-components` would then find no adapter.

    This is a text check, so it matches the exact pair of lines the neighbouring modules use and rules out what
    could make the pair inert. The pair must stand alone at the start of its lines, the attribute directly above the
    module and no other attribute above that. No block comment may appear anywhere in the file, since one could
    swallow the pair, and no inline module may open at the top level, since one could wrap it."""
    lines = text.splitlines()
    _need("/*" not in text, "clientio: lib.rs holds a block comment, which could hide the export")
    at = [index for index, line in enumerate(lines) if line == EXPORT_MOD]
    _need(len(at) == 1, f"clientio: lib.rs holds {len(at)} lines `{EXPORT_MOD}`, the export needs exactly one")
    index = at[0]
    _need(index >= 2 and lines[index - 1] == EXPORT_CFG, f"clientio: the line above the export is not `{EXPORT_CFG}`")
    _need(not lines[index - 2].lstrip().startswith("#["), "clientio: another attribute stands above the export's attribute")
    wrappers = [line for line in lines if re.match(r"^(pub(\([a-z]+\))? )?mod \w+ *\{", line)]
    if wrappers:
        raise ClientIoError(f"clientio: lib.rs opens an inline module at the top level: {wrappers[0]!r}")


def check_export() -> None:
    _need(LIB.is_file(), f"clientio: {LIB.relative_to(ROOT)} does not exist")
    check_export_text(LIB.read_text(encoding="utf-8"))


_HONEST_LIB = "\n".join([
    "#![cfg_attr(not(test), no_std)]",
    "",
    "pub mod zenoh_service;",
    EXPORT_CFG,
    EXPORT_MOD,
    "#[cfg(test)]",
    "mod zenoh_service_link;",
    "",
])


def export_controls() -> int:
    """The export check refuses what would leave the adapter unexported or inert, and accepts the honest pair."""
    check_export_text(_HONEST_LIB)
    edits = [
        ("the export removed", _HONEST_LIB.replace(EXPORT_MOD + "\n", "")),
        ("the attribute removed", _HONEST_LIB.replace(EXPORT_CFG + "\n" + EXPORT_MOD, EXPORT_MOD)),
        ("another feature in the attribute", _HONEST_LIB.replace(EXPORT_CFG, '#[cfg(feature = "heap")]')),
        ("the attribute negated", _HONEST_LIB.replace(EXPORT_CFG, '#[cfg(not(feature = "component-runtime"))]')),
        ("a test-only attribute", _HONEST_LIB.replace(EXPORT_CFG, "#[cfg(test)]")),
        ("an attribute that is never true above the pair", _HONEST_LIB.replace(EXPORT_CFG, "#[cfg(any())]\n" + EXPORT_CFG)),
        ("the export commented out", _HONEST_LIB.replace(EXPORT_MOD, "// " + EXPORT_MOD)),
        ("the pair inside a block comment", _HONEST_LIB.replace(EXPORT_CFG, "/*\n" + EXPORT_CFG).replace(EXPORT_MOD, EXPORT_MOD + "\n*/")),
        ("the export indented", _HONEST_LIB.replace(EXPORT_MOD, "    " + EXPORT_MOD)),
        ("the pair inside an inline module", _HONEST_LIB.replace(EXPORT_CFG, "mod hidden {\n" + EXPORT_CFG).replace(EXPORT_MOD, EXPORT_MOD + "\n}")),
        ("a private module", _HONEST_LIB.replace(EXPORT_MOD, "mod zenoh_service_client;")),
        ("another module name", _HONEST_LIB.replace(EXPORT_MOD, "pub mod zenoh_service_clients;")),
        ("the export twice", _HONEST_LIB.replace(EXPORT_MOD, EXPORT_MOD + "\n" + EXPORT_CFG + "\n" + EXPORT_MOD)),
        ("an empty file", ""),
    ]
    refused = 0
    for label, text in edits:
        if text == _HONEST_LIB:
            raise ClientIoError(f"control: {label} did not change the file")
        try:
            check_export_text(text)
        except ClientIoError:
            refused += 1
            continue
        raise ClientIoError(f"control: the export check accepted {label}")
    return refused


def all_controls() -> int:
    return transcript_controls() + reference_controls() + export_controls()


# What the admitted item fixes, held here as well so the gate stays correct after the item is completed and
# retired to .tasks/terminal. Both counts are exact.
PINNED_CASES = 72
PINNED_CONTROLS = 168
ADMISSION = "slime-zenoh-service-client-io-"


def item_pins() -> tuple[int, int]:
    """The case count and the control count the admitted item fixes, read from its stored body, or (0, 0)."""
    for path in sorted((ROOT / ".tasks" / "items").glob("*.md")):
        text = path.read_text(encoding="utf-8")
        if "zenoh_service_client_io_check" not in text or ADMISSION not in text or re.search(r"^state: cancelled$", text, re.M):
            continue
        found = re.search(r'id = "all-cases-observed";.*?intValue = (\d+);', text, re.S)
        controls = re.search(r'id = "exactly-\d+-controls";.*?observation = "negativeControlsRefused";.*?intValue = (\d+);', text, re.S)
        if found and controls:
            return int(found.group(1)), int(controls.group(1))
    return 0, 0
