"""The frame-store exam: what `contracts/frame-store/v1` means, held independently of its implementation.

The exam owns four judgements, and nothing here imports the converter, the generated bindings or
the Rust reader it judges:

* the contract: the schema's constants, layouts and refusal classes equal the values pinned here;
* the format: a strict decoder and a canonical encoder written from the schema's comment, and a
  corpus of stores each made to pass, or to fail exactly one check, in the contract's order;
* the Rust reader: the transcript its exam test writes for every corpus store, compared with this
  decoder's reading, payload by payload;
* the converter: its store from the pinned bag equals the store this module encodes from its own
  MCAP reading (`mcap_format`), and each bag it must refuse is refused by class.

Each judgement is trusted only after it refuses the corrupted evidence in `controls()`.
"""

from __future__ import annotations

import hashlib
import re
import struct
import zlib
from collections.abc import Callable, Iterator
from dataclasses import dataclass, replace

import mcap_format
from harness import ROOT

SCHEMA = ROOT / "contracts" / "frame-store" / "v1" / "schema.zt"
FIXTURE = ROOT / "components" / "lib" / "src" / "frame_store" / "fixture.bin"
CONVERTER = ROOT / "tools" / "frame-store" / "convert-mcap.py"
RUST_FILTER = "frame_store::exam"
LINE_TAG = "[frame-store]"
FIXTURE_MAX_BYTES = 4096

# The bag is a host artifact named by SLIME_LIDAR_BAG and never committed.
BAG_ENV = "SLIME_LIDAR_BAG"
BAG_SHA256 = "9ddece1e03367bd7c5cf1b6f9b977be35b97f00a36dfb4d55990429f0e1558ad"
BAG_BYTES = 98_999_859
BAG_CHANNEL = "/velodyne_points"
BAG_TYPE = "sensor_msgs/msg/PointCloud2"
BAG_TYPE_HASH = "RIHS01_9198cabf7da3796ae6fe19c4cb3bdd3525492988c70522628af5daa124bae2b5"
BAG_FRAMES = 200
BAG_FRAME_BYTES = 494_889
BAG_PERIOD_NS = 100_000_000
BAG_CHUNKS = 67
BAG_SCHEMA_BYTES = 724

CONVERTER_REFUSALS = ("bag-hash", "compressed-chunk", "channel")

# The contract, as this exam reads it. `schema_findings` requires the schema to state exactly these.
CONSTANTS = {
    "formatVersion": 1,
    "frameStoreMagic": int.from_bytes(b"SLFS", "little"),
    "hashVersionRihs01": 1,
    "headerBytes": 64,
    "indexEntryBytes": 24,
    "typeHashBytes": 32,
    "alignment": 8,
    "maxFrames": 4096,
    "maxPayloadBytes": 16 * 1024 * 1024,
    "maxStoreBytes": 128 * 1024 * 1024,
    "maxTopicBytes": 256,
    "maxTypeNameBytes": 256,
    "maxSchemaBytes": 65536,
}
REFUSAL_CLASSES = (
    "store-bound", "truncated", "magic", "version", "header", "frame-count",
    "payload-bound", "order", "range", "overlap", "padding", "trailing",
)
HEADER_LAYOUT = (
    ("magic", 4, False), ("version", 2, False), ("hashVersion", 1, False), ("reserved0", 1, True),
    ("frameCount", 4, False), ("topicBytes", 2, False), ("typeNameBytes", 2, False),
    ("schemaBytes", 4, False), ("reserved1", 4, True), ("framePeriodNs", 8, False), ("typeHash", 32, True),
)
INDEX_ENTRY_LAYOUT = (
    ("payloadOffset", 8, False), ("payloadBytes", 4, False), ("reserved", 4, True), ("logTimeNs", 8, False),
)

C = CONSTANTS
HEADER = struct.Struct("<IHBBIHHIIQ32s")
ENTRY = struct.Struct("<QIIQ")
assert HEADER.size == C["headerBytes"] and ENTRY.size == C["indexEntryBytes"]


class ExamError(Exception):
    """The evidence does not meet the exam."""


class Refused(Exception):
    """The decoder refuses a store; `kind` is one of `REFUSAL_CLASSES`."""

    def __init__(self, kind: str, detail: str):
        super().__init__(f"{kind}: {detail}")
        assert kind in REFUSAL_CLASSES, kind
        self.kind = kind


def align(value: int) -> int:
    return (value + C["alignment"] - 1) // C["alignment"] * C["alignment"]


def rihs01(text: str) -> bytes:
    match = re.fullmatch(r"RIHS01_([0-9a-f]{64})", text)
    if match is None:
        raise ExamError(f"{text!r} is not a RIHS01 type hash")
    return bytes.fromhex(match.group(1))


# The contract.

def schema_findings(text: str) -> list[str]:
    """Where the schema text differs from the contract this exam pins."""
    findings = []
    for name, value in CONSTANTS.items():
        found = re.findall(rf"^{name} :: Int = (\d+);", text, re.M)
        if found != [str(value)]:
            findings.append(f"{name} must be stated once as {value}, found {found}")
    classes = re.search(r"^refusalClasses :: List Text = \{(.*?)^\};", text, re.M | re.S)
    if classes is None or tuple(re.findall(r'"([a-z-]+)";', classes.group(1))) != REFUSAL_CLASSES:
        findings.append(f"refusalClasses must be {list(REFUSAL_CLASSES)} in that order")
    for list_name, layout in (("headerLayout", HEADER_LAYOUT), ("indexEntryLayout", INDEX_ENTRY_LAYOUT)):
        block = re.search(rf"^{list_name} :: List WireField = \{{(.*?)^\}};", text, re.M | re.S)
        rows = () if block is None else tuple(
            (name, int(width), array == "true")
            for name, width, array in re.findall(
                r'\{ name = "(\w+)"; width = (\d+); signed = false; byteArray = (true|false); \};', block.group(1)
            )
        )
        if block is None or rows != layout or block.group(1).count("{") != len(layout):
            findings.append(f"{list_name} must be {layout}")
    return findings


# The format.

@dataclass(frozen=True)
class Store:
    topic: str
    type_name: str
    type_hash: bytes
    schema: str
    period_ns: int
    frames: tuple[tuple[int, bytes], ...]  # (log time, payload)


@dataclass(frozen=True)
class Frame:
    offset: int
    length: int
    log_time: int


@dataclass(frozen=True)
class Reading:
    """What a reader exposes for an accepted store. Payloads are named by offset and length."""

    topic: bytes
    type_name: bytes
    type_hash: bytes
    schema: bytes
    period_ns: int
    frames: tuple[Frame, ...]


def encode(store: Store) -> bytes:
    """The one canonical encoding of `store`."""
    texts = [store.topic.encode(), store.type_name.encode(), store.schema.encode()]
    out = bytearray(
        HEADER.pack(
            C["frameStoreMagic"], C["formatVersion"], C["hashVersionRihs01"], 0, len(store.frames),
            len(texts[0]), len(texts[1]), len(texts[2]), 0, store.period_ns, store.type_hash,
        )
    )
    out += b"".join(texts)
    out += bytes(align(len(out)) - len(out))
    cursor = len(out) + ENTRY.size * len(store.frames)
    for log_time, payload in store.frames:
        out += ENTRY.pack(cursor, len(payload), 0, log_time)
        cursor = align(cursor + len(payload))
    for _, payload in store.frames:
        out += payload + bytes(align(len(payload)) - len(payload))
    return bytes(out)


def _text(raw: bytes, what: str) -> None:
    try:
        raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise Refused("header", f"the {what} is not UTF-8") from error
    if b"\x00" in raw:
        raise Refused("header", f"the {what} holds a NUL byte")


def decode(data: bytes) -> Reading:
    """Validate a whole store in the contract's order. Raises Refused with the first failing class."""
    size = len(data)
    if size > C["maxStoreBytes"]:
        raise Refused("store-bound", f"{size} bytes is above {C['maxStoreBytes']}")
    if size < C["headerBytes"]:
        raise Refused("truncated", f"{size} bytes is shorter than the header")
    (magic, version, hash_version, reserved0, count, topic_len, type_len, schema_len, reserved1, period,
     type_hash) = HEADER.unpack_from(data)
    if magic != C["frameStoreMagic"]:
        raise Refused("magic", f"magic 0x{magic:08x}")
    if version != C["formatVersion"]:
        raise Refused("version", f"version {version}")
    if hash_version != C["hashVersionRihs01"] or reserved0 or reserved1 or period == 0:
        raise Refused("header", "hash version, a reserved field or the period")
    for length, bound, what in (
        (topic_len, C["maxTopicBytes"], "topic"),
        (type_len, C["maxTypeNameBytes"], "type name"),
        (schema_len, C["maxSchemaBytes"], "schema"),
    ):
        if not 1 <= length <= bound:
            raise Refused("header", f"the {what} length {length} is outside 1..{bound}")
    if not 1 <= count <= C["maxFrames"]:
        raise Refused("frame-count", f"{count} frames")
    text_start = C["headerBytes"]
    text_end = text_start + topic_len + type_len + schema_len
    index_start = align(text_end)
    data_start = index_start + ENTRY.size * count
    if size < data_start:
        raise Refused("truncated", f"the store ends at {size}, before its index ends at {data_start}")
    _text(data[text_start:text_start + topic_len], "topic")
    _text(data[text_start + topic_len:text_end - schema_len], "type name")
    _text(data[text_end - schema_len:text_end], "schema")
    if any(data[text_end:index_start]):
        raise Refused("header", "a byte between the text and the index is not zero")
    frames = []
    cursor = data_start
    previous_end = data_start
    previous_time = None
    for index in range(count):
        offset, length, reserved, log_time = ENTRY.unpack_from(data, index_start + ENTRY.size * index)
        if reserved:
            raise Refused("header", f"entry {index} has a nonzero reserved field")
        if length > C["maxPayloadBytes"]:
            raise Refused("payload-bound", f"entry {index} holds {length} bytes")
        if previous_time is not None and log_time < previous_time:
            raise Refused("order", f"entry {index} log time {log_time} is below {previous_time}")
        if offset > size or length > size - offset:
            raise Refused("range", f"entry {index} ends past the store")
        if offset < cursor:
            raise Refused("overlap", f"entry {index} starts at {offset}, before {cursor}")
        if offset > cursor or any(data[previous_end:offset]):
            raise Refused("padding", f"entry {index} padding before {offset}")
        frames.append(Frame(offset, length, log_time))
        previous_time = log_time
        previous_end = offset + length
        cursor = align(previous_end)
    if any(data[previous_end:min(cursor, size)]):
        raise Refused("padding", "a byte after the last payload is not zero")
    if size < cursor:
        raise Refused("range", f"the store ends at {size}, before the aligned end {cursor}")
    if size > cursor:
        raise Refused("trailing", f"{size - cursor} bytes after the aligned end {cursor}")
    return Reading(
        data[text_start:text_start + topic_len],
        data[text_start + topic_len:text_end - schema_len],
        type_hash,
        data[text_end - schema_len:text_end],
        period,
        tuple(frames),
    )


# The corpus. Every case is a store this decoder accepts, or one built to fail exactly one check.

HASH = rihs01(BAG_TYPE_HASH)
BASE = Store(
    topic="/exam/points",
    type_name=BAG_TYPE,
    type_hash=HASH,
    schema="uint8 x\n",
    period_ns=50_000_000,
    frames=((10, b"\x01\x02\x03\x04\x05"), (20, bytes(range(16))), (20, b"\xaa\xbb\xcc")),
)


def _layout(store: Store) -> tuple[int, int, int]:
    """(text end, index start, data start) of `store`'s encoding."""
    text_end = C["headerBytes"] + sum(len(t.encode()) for t in (store.topic, store.type_name, store.schema))
    index_start = align(text_end)
    return text_end, index_start, index_start + ENTRY.size * len(store.frames)


def _patch(data: bytes, offset: int, form: str, value: int) -> bytes:
    out = bytearray(data)
    struct.pack_into("<" + form, out, offset, value)
    return bytes(out)


def _entry(data: bytes, index: int, field: str, value: int) -> bytes:
    _, index_start, _ = _layout(BASE)
    position, form = {"offset": (0, "Q"), "length": (8, "I"), "reserved": (12, "I"), "log": (16, "Q")}[field]
    return _patch(data, index_start + ENTRY.size * index + position, form, value)


def _byte(data: bytes, offset: int, value: int) -> bytes:
    out = bytearray(data)
    out[offset] = value
    return bytes(out)


def _base_mutations() -> dict[str, tuple[Callable[[bytes], bytes], str]]:
    """Each edit of the base store and the class it must be refused with. The base pads its text by
    one byte, its first payload by three and its last by five."""
    text_end, index_start, d = _layout(BASE)
    assert index_start > text_end, "the base store must pad its text"
    return {
        "empty": (lambda b: b"", "truncated"),
        "header-short": (lambda b: b[:63], "truncated"),
        "magic": (lambda b: _byte(b, 0, b[0] ^ 1), "magic"),
        "version-0": (lambda b: _patch(b, 4, "H", 0), "version"),
        "version-2": (lambda b: _patch(b, 4, "H", 2), "version"),
        "hash-version-0": (lambda b: _patch(b, 6, "B", 0), "header"),
        "hash-version-2": (lambda b: _patch(b, 6, "B", 2), "header"),
        "reserved0": (lambda b: _patch(b, 7, "B", 1), "header"),
        "reserved1": (lambda b: _patch(b, 20, "I", 1), "header"),
        "period-zero": (lambda b: _patch(b, 24, "Q", 0), "header"),
        "topic-empty": (lambda b: _patch(b, 12, "H", 0), "header"),
        "topic-over": (lambda b: _patch(b, 12, "H", C["maxTopicBytes"] + 1), "header"),
        "type-empty": (lambda b: _patch(b, 14, "H", 0), "header"),
        "type-over": (lambda b: _patch(b, 14, "H", C["maxTypeNameBytes"] + 1), "header"),
        "schema-empty": (lambda b: _patch(b, 16, "I", 0), "header"),
        "schema-over": (lambda b: _patch(b, 16, "I", C["maxSchemaBytes"] + 1), "header"),
        "frames-zero": (lambda b: _patch(b, 8, "I", 0), "frame-count"),
        "frames-over": (lambda b: _patch(b, 8, "I", C["maxFrames"] + 1), "frame-count"),
        "index-short": (lambda b: b[:d - 1], "truncated"),
        "topic-not-utf8": (lambda b: _byte(b, 65, 0xFF), "header"),
        "schema-nul": (lambda b: _byte(b, text_end - 2, 0), "header"),
        "text-padding": (lambda b: _byte(b, text_end, 1), "header"),
        "entry-reserved": (lambda b: _entry(b, 1, "reserved", 1), "header"),
        "payload-over": (lambda b: _entry(b, 1, "length", C["maxPayloadBytes"] + 1), "payload-bound"),
        "log-backwards": (lambda b: _entry(b, 1, "log", 9), "order"),
        "entries-swapped": (lambda b: _entry(_entry(b, 0, "offset", d + 8), 1, "offset", d), "padding"),
        "payload-past-end": (lambda b: _entry(b, 2, "length", 9), "range"),
        "offset-wraps": (lambda b: _entry(b, 2, "offset", 2**64 - 8), "range"),
        "offset-past-end": (lambda b: _entry(b, 2, "offset", len(b) + 8), "range"),
        "overlap-start": (lambda b: _entry(b, 1, "offset", d), "overlap"),
        "overlap-unaligned": (lambda b: _entry(b, 1, "offset", d + 4), "overlap"),
        "first-before-data": (lambda b: _entry(b, 0, "offset", d - 8), "overlap"),
        "gap": (lambda b: _entry(b, 1, "offset", d + 16), "padding"),
        "gap-unaligned": (lambda b: _entry(b, 1, "offset", d + 9), "padding"),
        "padding-nonzero": (lambda b: _byte(b, d + 5, 1), "padding"),
        "end-padding-nonzero": (lambda b: _byte(b, len(b) - 1, 1), "padding"),
        "cut-in-end-padding": (lambda b: b[:-2], "range"),
        "cut-in-payload": (lambda b: b[:d + 26], "range"),
        "trailing-zeros": (lambda b: b + bytes(8), "trailing"),
        "trailing-byte": (lambda b: b + b"\x01", "trailing"),
        # Two faults each: the class must be the earlier check's, which pins the contract's order.
        "short-and-magic": (lambda b: bytes(63), "truncated"),
        "magic-and-version": (lambda b: _patch(_byte(b, 0, b[0] ^ 1), 4, "H", 2), "magic"),
        "version-and-header": (lambda b: _patch(_patch(b, 4, "H", 2), 20, "I", 1), "version"),
        "header-and-frame-count": (lambda b: _patch(_patch(b, 24, "Q", 0), 8, "I", 0), "header"),
        "frame-count-and-text": (lambda b: _patch(_byte(b, 65, 0xFF), 8, "I", 0), "frame-count"),
        "index-short-and-text": (lambda b: _byte(b, 65, 0xFF)[:d - 1], "truncated"),
        "text-and-entry": (lambda b: _entry(_byte(b, text_end, 1), 0, "length", C["maxPayloadBytes"] + 1), "header"),
        "reserved-and-bound": (
            lambda b: _entry(_entry(b, 1, "reserved", 1), 1, "length", C["maxPayloadBytes"] + 1), "header"
        ),
        "bound-and-order": (lambda b: _entry(_entry(b, 1, "length", C["maxPayloadBytes"] + 1), 1, "log", 9), "payload-bound"),
        "bound-and-offset-past-end": (
            lambda b: _entry(_entry(b, 1, "length", C["maxPayloadBytes"] + 1), 1, "offset", 2**40), "payload-bound"
        ),
        "order-and-range": (lambda b: _entry(_entry(b, 1, "log", 9), 1, "offset", 2**40), "order"),
        "range-and-overlap": (lambda b: _entry(_entry(b, 1, "offset", d), 1, "length", 4096), "range"),
        "earlier-entry-first": (lambda b: _entry(_entry(b, 0, "offset", d + 8), 1, "log", 9), "padding"),
        "end-padding-and-trailing": (lambda b: _byte(b, len(b) - 1, 1) + bytes(8), "padding"),
    }


@dataclass(frozen=True)
class Case:
    name: str
    build: Callable[[], bytes]
    expect: str  # "accept" or a refusal class




def _max_store() -> bytes:
    """A store of exactly `maxStoreBytes`: seven payloads at the bound and one that fills the rest."""
    store = Store("/exam/max", BAG_TYPE, HASH, "uint8[] data\n", 1, ())
    frames = [(index, bytes([index]) * C["maxPayloadBytes"]) for index in range(7)]
    _, _, data_start = _layout(replace(store, frames=(*frames, (7, b""))))
    remaining = C["maxStoreBytes"] - data_start - 7 * C["maxPayloadBytes"]
    assert 8 <= remaining <= C["maxPayloadBytes"] and remaining % 8 == 0
    frames.append((7, b"\x07" * (remaining - 5)))
    data = encode(replace(store, frames=tuple(frames)))
    assert len(data) == C["maxStoreBytes"]
    return data


def cases() -> tuple[Case, ...]:
    """The exam corpus, in the order the Rust test must report it. Builds are lazy: two are 128 MiB."""
    base = encode(BASE)
    built = [
        Case("base", lambda: base, "accept"),
        Case("single-empty", lambda: encode(replace(BASE, frames=((7, b""),))), "accept"),
        Case("equal-times", lambda: encode(replace(BASE, frames=((5, b"a"), (5, b"bb"), (5, b"ccc")))), "accept"),
        Case(
            "max-frames",
            lambda: encode(replace(BASE, frames=tuple((i, bytes([i % 251]) * (i % 9)) for i in range(C["maxFrames"])))),
            "accept",
        ),
        Case(
            "max-text",
            lambda: encode(
                replace(
                    BASE,
                    topic="/" + "t" * (C["maxTopicBytes"] - 1),
                    type_name="p" * C["maxTypeNameBytes"],
                    schema="é" * (C["maxSchemaBytes"] // 2),
                )
            ),
            "accept",
        ),
        Case("max-payload", lambda: encode(replace(BASE, frames=((1, bytes(range(256)) * (C["maxPayloadBytes"] // 256)),))), "accept"),
        Case("max-store", _max_store, "accept"),
        Case("store-bound", lambda: _max_store() + bytes(8), "store-bound"),
    ]
    for name, (mutate, expect) in _base_mutations().items():
        built.append(Case(name, lambda mutate=mutate: mutate(base), expect))
    # Encoded with a text outside its bound, so the length check is the only check that fails.
    for name, fields in {
        "encoded-topic-empty": {"topic": ""},
        "encoded-type-empty": {"type_name": ""},
        "encoded-schema-empty": {"schema": ""},
        "encoded-topic-over": {"topic": "/" + "t" * C["maxTopicBytes"]},
        "encoded-type-over": {"type_name": "p" * (C["maxTypeNameBytes"] + 1)},
        "encoded-schema-over": {"schema": "s" * (C["maxSchemaBytes"] + 1)},
    }.items():
        built.append(Case(name, lambda fields=fields: encode(replace(BASE, **fields)), "header"))
    return tuple(built)


def expected_lines(case: str, data: bytes) -> list[str]:
    """The transcript lines the Rust exam test must write for one store, from this decoder's reading."""
    try:
        reading = decode(data)
    except Refused as refusal:
        return [f"{LINE_TAG} {case} refuse {refusal.kind}"]
    lines = [
        f"{LINE_TAG} {case} accept frames={len(reading.frames)} period={reading.period_ns} "
        f"hash={reading.type_hash.hex()} topic={reading.topic.hex()} type={reading.type_name.hex()} "
        f"schema={reading.schema.hex()}"
    ]
    for index, frame in enumerate(reading.frames):
        digest = hashlib.sha256(data[frame.offset:frame.offset + frame.length]).hexdigest()
        lines.append(
            f"{LINE_TAG} {case} frame {index} offset={frame.offset} bytes={frame.length} "
            f"log={frame.log_time} sha256={digest}"
        )
    lines.append(f"{LINE_TAG} {case} end")
    return lines


def case_lines(case: Case, data: bytes) -> list[str]:
    """The transcript lines for one corpus case, after the decoder reaches the case's declared verdict."""
    found = verdict(data)
    if found != case.expect:
        raise ExamError(f"the exam decoder reads corpus case {case.name} as {found}, the corpus declares {case.expect}")
    return expected_lines(case.name, data)


LINE = re.compile(
    rf"^{re.escape(LINE_TAG)} [a-z0-9-]+ (?:"
    r"refuse [a-z-]+"
    r"|accept frames=\d+ period=\d+ hash=[0-9a-f]{64} topic=[0-9a-f]+ type=[0-9a-f]+ schema=[0-9a-f]+"
    r"|frame \d+ offset=\d+ bytes=\d+ log=\d+ sha256=[0-9a-f]{64}"
    r"|end)$"
)


def judge_transcript(text: str, expected: list[str]) -> None:
    """The transcript the Rust test wrote must be exactly `expected`, in order, one well-formed line each."""
    if text and not text.endswith("\n"):
        raise ExamError("the transcript does not end with a newline")
    printed = text.splitlines()
    for line in printed:
        if LINE.fullmatch(line) is None:
            raise ExamError(f"the Rust exam printed a malformed line: {line[:200]!r}")
    if printed != expected:
        for index, (got, want) in enumerate(zip(printed, expected, strict=False)):
            if got != want:
                raise ExamError(f"line {index}: the Rust exam printed {got[:200]!r}, the exam expects {want[:200]!r}")
        raise ExamError(f"the Rust exam printed {len(printed)} lines, the exam expects {len(expected)}")


TEST_LINE = re.compile(r"^test (\S+) \.\.\. (ok|FAILED|ignored)$", re.M)


def judge_test_run(output: str) -> int:
    """At least one test under `frame_store::exam` passed, no test failed, and cargo reported success."""
    results = TEST_LINE.findall(output)
    ours = [(name, status) for name, status in results if RUST_FILTER in name]
    if not ours or any(status != "ok" for _, status in ours):
        raise ExamError(f"no passing test named {RUST_FILTER!r}: {ours}")
    if any(status == "FAILED" for _, status in results):
        raise ExamError("a test in the run failed")
    summaries = re.findall(r"test result: (\w+)\. (\d+) passed; (\d+) failed", output)
    if not summaries or any(status != "ok" or int(failed) for status, _, failed in summaries):
        raise ExamError("the cargo test run did not report success")
    return len(ours)


def judge_fixture(data: bytes) -> Reading:
    """The committed fixture is small, valid, and pads a payload in a store of more than one frame."""
    if len(data) > FIXTURE_MAX_BYTES:
        raise ExamError(f"the committed fixture is {len(data)} bytes, above {FIXTURE_MAX_BYTES}")
    try:
        reading = decode(data)
    except Refused as refusal:
        raise ExamError(f"the committed fixture is refused: {refusal}") from refusal
    if len(reading.frames) < 2:
        raise ExamError("the committed fixture holds fewer than 2 frames")
    if all(frame.length % C["alignment"] == 0 for frame in reading.frames):
        raise ExamError("no payload in the committed fixture needs padding")
    return reading


# The bag and the converter.

def judge_bag(bag: mcap_format.Bag) -> mcap_format.Channel:
    """The pinned bag holds what the item states, counted from the reading."""
    channel = bag.channel_by_topic(BAG_CHANNEL)
    if channel is None:
        raise ExamError(f"the bag has no {BAG_CHANNEL} channel")
    schema = bag.schemas.get(channel.schema_id)
    facts = {
        "chunks": (bag.chunks, BAG_CHUNKS),
        "compressed chunks": (sum(1 for c in bag.chunk_compressions if c), 0),
        "channels": (len(bag.channels), 1),
        "messages": (len(bag.messages), BAG_FRAMES),
        "channel messages": (sum(1 for m in bag.messages if m.channel_id == channel.id), BAG_FRAMES),
        "message encoding": (channel.message_encoding, "cdr"),
        "schema name": (schema and schema.name, BAG_TYPE),
        "schema encoding": (schema and schema.encoding, "ros2msg"),
        "schema bytes": (schema and len(schema.data), BAG_SCHEMA_BYTES),
        "message bytes": ({len(m.data) for m in bag.messages}, {BAG_FRAME_BYTES}),
        "log-time steps": (
            {b.log_time - a.log_time for a, b in zip(bag.messages, bag.messages[1:], strict=False)},
            {BAG_PERIOD_NS},
        ),
    }
    for what, (found, expected) in facts.items():
        if found != expected:
            raise ExamError(f"the bag's {what} is {found!r}, the item states {expected!r}")
    return channel


def expected_store(bag: mcap_format.Bag, topic: str, period_ns: int) -> Store:
    """The store a converter writes for `topic`: its messages in log-time order, file order on ties."""
    channel = bag.channel_by_topic(topic)
    if channel is None:
        raise ExamError(f"the bag has no {topic} channel")
    schema = bag.schemas[channel.schema_id]
    messages = sorted((m for m in bag.messages if m.channel_id == channel.id), key=lambda m: m.log_time)
    return Store(
        topic, schema.name, HASH, schema.data.decode("utf-8"), period_ns,
        tuple((m.log_time, m.data) for m in messages),
    )


def judge_converted(first: bytes, second: bytes, expected: Store, *, period_pinned: bool) -> Reading:
    """Two conversions are identical and equal the canonical encoding of `expected`, frame by frame.

    Without `period_pinned`, the declared period may be any nonzero value; everything else is pinned.
    """
    if first != second:
        raise ExamError("two conversions of the same bag and channel differ")
    try:
        reading = decode(first)
    except Refused as refusal:
        raise ExamError(f"the converted store is refused: {refusal}") from refusal
    if not period_pinned:
        expected = replace(expected, period_ns=reading.period_ns)
    checks = {
        "topic": (reading.topic, expected.topic.encode()),
        "type": (reading.type_name, expected.type_name.encode()),
        "type hash": (reading.type_hash, expected.type_hash),
        "schema": (reading.schema, expected.schema.encode()),
        "period": (reading.period_ns, expected.period_ns),
        "frame count": (len(reading.frames), len(expected.frames)),
    }
    for what, (found, want) in checks.items():
        if found != want:
            raise ExamError(f"the converted store's {what} is {str(found)[:120]}, the exam expects {str(want)[:120]}")
    for index, (frame, (log_time, payload)) in enumerate(zip(reading.frames, expected.frames, strict=True)):
        if frame.log_time != log_time or first[frame.offset:frame.offset + frame.length] != payload:
            raise ExamError(f"converted frame {index} is not message {index} of the channel in log-time order")
    if first != encode(expected):
        raise ExamError("the converted store is not the canonical encoding of its content")
    return reading


REFUSAL_LINE = re.compile(r"^convert-mcap: refused ([a-z-]+)(?:: .*)?$")


def judge_refusal(returncode: int, stderr: str, output_exists: bool, expected: str) -> None:
    """The converter exits nonzero, names the class on its last stderr line, and writes no store."""
    lines = stderr.strip().splitlines()
    match = REFUSAL_LINE.fullmatch(lines[-1]) if lines else None
    if returncode == 0:
        raise ExamError(f"the converter accepted a bag it must refuse as {expected}")
    if match is None or match.group(1) != expected:
        raise ExamError(f"the converter must end with 'convert-mcap: refused {expected}', printed {lines[-1:]!r}")
    if output_exists:
        raise ExamError(f"the converter wrote a store for a bag it refused as {expected}")


def synthetic_bags(bag: mcap_format.Bag) -> tuple[bytes, Store, bytes]:
    """Two small bags from the pinned bag's first six messages: (plain, its store, compressed).

    The requested channel is the second one declared, its messages arrive out of log-time order
    across two chunks, and another channel with the same schema interleaves with it. The compressed
    bag is the plain one with its second chunk stored as a zstd frame.
    """
    channel = bag.channel_by_topic(BAG_CHANNEL)
    schema = bag.schemas[channel.schema_id]
    other = mcap_format.Channel(1, schema.id, "/other_points", "cdr", channel.metadata)
    wanted = mcap_format.Channel(2, schema.id, BAG_CHANNEL, "cdr", channel.metadata)
    data = [message.data for message in bag.messages[:6]]
    base = 1_000_000_000

    def message(channel_id: int, sequence: int, offset_ms: int, payload: bytes) -> mcap_format.Message:
        time = base + offset_ms * 1_000_000
        return mcap_format.Message(channel_id, sequence, time, time, payload)

    first = (schema, other, wanted, message(2, 0, 300, data[0]), message(1, 0, 50, data[1]))
    second = (
        message(2, 1, 100, data[2]), message(1, 1, 150, data[3]),
        message(2, 2, 200, data[4]), message(1, 2, 250, data[5]),
    )
    plain = mcap_format.write((mcap_format.ChunkPlan(first), mcap_format.ChunkPlan(second)))
    compressed = mcap_format.write((mcap_format.ChunkPlan(first), mcap_format.ChunkPlan(second, "zstd")))
    expected = Store(
        BAG_CHANNEL, schema.name, HASH, schema.data.decode("utf-8"), 1,
        ((base + 100_000_000, data[2]), (base + 200_000_000, data[4]), (base + 300_000_000, data[0])),
    )
    return plain, expected, compressed


# Controls: each corrupts honest evidence and requires the judge above to refuse it.

def _refuses(action: Callable[[], object], exception: type[Exception] = ExamError) -> bool:
    try:
        action()
    except exception:
        return True
    return False


def _schema_controls(text: str) -> Iterator[tuple[str, bool]]:
    yield "schema-honest", not schema_findings(text)
    for name, value in CONSTANTS.items():
        yield f"schema-{name}", bool(schema_findings(text.replace(f"{name} :: Int = {value};", f"{name} :: Int = {value + 1};")))
    edits = {
        "schema-class-dropped": ('  "trailing";\n', ""),
        "schema-class-swapped": ('  "overlap";\n  "padding";', '  "padding";\n  "overlap";'),
        "schema-header-width": ('{ name = "frameCount"; width = 4;', '{ name = "frameCount"; width = 8;'),
        "schema-entry-field": ('{ name = "payloadBytes"; width = 4;', '{ name = "payloadLen"; width = 4;'),
        "schema-entry-extra": (
            '  { name = "logTimeNs"; width = 8; signed = false; byteArray = false; };\n',
            '  { name = "logTimeNs"; width = 8; signed = false; byteArray = false; };\n'
            '  { name = "spare"; width = 8, signed = false; byteArray = true; };\n',
        ),
    }
    for name, (old, new) in edits.items():
        assert old in text, name
        yield name, bool(schema_findings(text.replace(old, new)))


def _flip_hex(line: str, key: str) -> str:
    """`line` with the first hex digit after `key=` changed."""
    start = line.index(f"{key}=") + len(key) + 1
    return line[:start] + ("1" if line[start] == "0" else "0") + line[start + 1:]


def _transcript_controls() -> Iterator[tuple[str, bool]]:
    base = encode(BASE)
    expected = expected_lines("base", base) + expected_lines("magic", _base_mutations()["magic"][0](base))
    honest = "\n".join(expected) + "\n"
    yield "transcript-honest", not _refuses(lambda: judge_transcript(honest, expected))
    yield "transcript-unterminated", _refuses(lambda: judge_transcript(honest[:-1], expected))
    yield "transcript-empty", _refuses(lambda: judge_transcript("", expected))
    edits = {
        "transcript-missing-case": lambda lines: lines[:-1],
        "transcript-extra-line": lambda lines: [*lines, f"{LINE_TAG} extra refuse magic"],
        "transcript-reordered": lambda lines: lines[-1:] + lines[:-1],
        "transcript-wrong-class": lambda lines: [*lines[:-1], f"{LINE_TAG} magic refuse version"],
        "transcript-accepts-refused": lambda lines: [*lines[:-1], f"{LINE_TAG} magic end"],
        "transcript-frame-count": lambda lines: [lines[0].replace("frames=3", "frames=2"), *lines[1:]],
        "transcript-period": lambda lines: [lines[0].replace("period=50000000", "period=5"), *lines[1:]],
        "transcript-hash": lambda lines: [_flip_hex(lines[0], "hash"), *lines[1:]],
        "transcript-topic": lambda lines: [_flip_hex(lines[0], "topic"), *lines[1:]],
        "transcript-type": lambda lines: [_flip_hex(lines[0], "type"), *lines[1:]],
        "transcript-schema": lambda lines: [_flip_hex(lines[0], "schema"), *lines[1:]],
        "transcript-offset": lambda lines: [lines[0], re.sub(r"offset=(\d+)", lambda m: f"offset={int(m.group(1)) + 8}", lines[1]), *lines[2:]],
        "transcript-bytes": lambda lines: [lines[0], lines[1].replace("bytes=5", "bytes=8"), *lines[2:]],
        "transcript-log": lambda lines: [lines[0], lines[1].replace("log=10", "log=11"), *lines[2:]],
        "transcript-sha": lambda lines: [lines[0], _flip_hex(lines[1], "sha256"), *lines[2:]],
        "transcript-frame-dropped": lambda lines: lines[:2] + lines[3:],
        "transcript-end-missing": lambda lines: [line for line in lines if not line.endswith(" end")],
        "transcript-malformed": lambda lines: [*lines, f"{LINE_TAG} base accept frames=x"],
        "transcript-foreign-line": lambda lines: [lines[0], "debug output", *lines[1:]],
    }
    for name, edit in edits.items():
        stdout = "\n".join(edit(list(expected))) + "\n"
        yield name, _refuses(lambda stdout=stdout: judge_transcript(stdout, expected))
    passing = "running 1 test\ntest frame_store::exam::corpus ... ok\n\ntest result: ok. 1 passed; 0 failed;"
    yield "test-run-honest", not _refuses(lambda: judge_test_run(passing))
    for name, output in {
        "test-run-missing": "test other::test ... ok\ntest result: ok. 1 passed; 0 failed;",
        "test-run-failed": "test frame_store::exam::corpus ... FAILED\ntest result: FAILED. 0 passed; 1 failed;",
        "test-run-ignored": "test frame_store::exam::corpus ... ignored\ntest result: ok. 0 passed; 0 failed;",
        "test-run-other-failed": (
            "test frame_store::exam::corpus ... ok\ntest other::t ... FAILED\ntest result: FAILED. 1 passed; 1 failed;"
        ),
        "test-run-no-summary": "test frame_store::exam::corpus ... ok\n",
    }.items():
        yield name, _refuses(lambda output=output: judge_test_run(output))


def _fixture_controls() -> Iterator[tuple[str, bool]]:
    honest = encode(BASE)
    yield "fixture-honest", not _refuses(lambda: judge_fixture(honest))
    for name, data in {
        "fixture-large": encode(replace(BASE, frames=((1, bytes(FIXTURE_MAX_BYTES)), (2, b"x")))),
        "fixture-one-frame": encode(replace(BASE, frames=((1, b"abc"),))),
        "fixture-aligned": encode(replace(BASE, frames=((1, bytes(8)), (2, bytes(16))))),
        "fixture-invalid": honest + b"\x00",
    }.items():
        yield name, _refuses(lambda data=data: judge_fixture(data))


def _converter_controls() -> Iterator[tuple[str, bool]]:
    expected = replace(BASE, topic=BAG_CHANNEL)
    honest = encode(expected)
    frames = expected.frames
    yield "converted-honest", not _refuses(lambda: judge_converted(honest, honest, expected, period_pinned=True))
    single = {
        "converted-payload-byte": _byte(honest, _layout(expected)[2], 0x7F),
        "converted-frames-swapped": encode(replace(expected, frames=(frames[1], frames[0], frames[2]))),
        "converted-frame-dropped": encode(replace(expected, frames=frames[:2])),
        "converted-topic": encode(replace(expected, topic="/other_points")),
        "converted-type": encode(replace(expected, type_name="sensor_msgs/msg/PointCloud")),
        "converted-hash": encode(replace(expected, type_hash=bytes(32))),
        "converted-schema": encode(replace(expected, schema="uint8 y\n")),
        "converted-period": encode(replace(expected, period_ns=1)),
        "converted-log-time": encode(replace(expected, frames=((11, frames[0][1]), *frames[1:]))),
        "converted-trailing": honest + bytes(8),
    }
    yield "converted-nondeterministic", _refuses(
        lambda: judge_converted(honest, _byte(honest, len(honest) - 9, 0x55), expected, period_pinned=True)
    )
    for name, data in single.items():
        yield name, _refuses(lambda data=data: judge_converted(data, data, expected, period_pinned=True))
    loose = encode(replace(expected, period_ns=7))
    yield "converted-free-period", not _refuses(lambda: judge_converted(loose, loose, expected, period_pinned=False))
    yield "refusal-honest", not _refuses(lambda: judge_refusal(1, "convert-mcap: refused channel: /x\n", False, "channel"))
    for name, args in {
        "refusal-exit-zero": (0, "convert-mcap: refused channel\n", False, "channel"),
        "refusal-wrong-class": (1, "convert-mcap: refused bag-hash\n", False, "channel"),
        "refusal-not-last": (1, "convert-mcap: refused channel\ntraceback\n", False, "channel"),
        "refusal-silent": (1, "", False, "channel"),
        "refusal-wrote-store": (1, "convert-mcap: refused channel\n", True, "channel"),
    }.items():
        yield name, _refuses(lambda args=args: judge_refusal(*args))


def _foreign_record_bag(schema: mcap_format.Schema, channel: mcap_format.Channel) -> bytes:
    """A bag whose chunk holds a metadata record, which may sit only in the data section."""
    inner = mcap_format.schema_record(schema) + mcap_format.channel_record(channel)
    inner += mcap_format.record(mcap_format.OP_METADATA, struct.pack("<I", 1) + b"m" + struct.pack("<I", 0))
    body = struct.pack("<QQQI", 0, 0, len(inner), zlib.crc32(inner)) + struct.pack("<IQ", 0, len(inner)) + inner
    out = mcap_format.MAGIC + mcap_format.record(mcap_format.OP_HEADER, struct.pack("<I", 4) + b"ros2" + struct.pack("<I", 0))
    out += mcap_format.record(mcap_format.OP_CHUNK, body) + mcap_format.record(mcap_format.OP_DATA_END, bytes(4))
    return out + mcap_format.record(mcap_format.OP_FOOTER, bytes(20)) + mcap_format.MAGIC


def _mcap_controls() -> Iterator[tuple[str, bool]]:
    schema = mcap_format.Schema(1, BAG_TYPE, "ros2msg", b"uint8 x\n")
    channel = mcap_format.Channel(1, 1, BAG_CHANNEL, "cdr", ())
    messages = tuple(mcap_format.Message(1, i, 100 * i, 100 * i, bytes([i]) * 9) for i in range(3))
    honest = mcap_format.write(
        (mcap_format.ChunkPlan((schema, channel, *messages[:2])), mcap_format.ChunkPlan(messages[2:]))
    )
    reading = mcap_format.read(honest)
    yield "mcap-honest", [m.data for m in reading.messages] == [m.data for m in messages]
    message_at = honest.index(bytes([2]) * 9)
    compressed = mcap_format.write(
        (mcap_format.ChunkPlan((schema, channel, *messages)), mcap_format.ChunkPlan((), "zstd"))
    )
    undeclared = mcap_format.write((mcap_format.ChunkPlan((schema, channel, replace(messages[0], channel_id=2))),))
    redeclared = mcap_format.write((mcap_format.ChunkPlan((schema, channel, replace(channel, topic="/x"))),))
    corrupt = {
        "mcap-magic": ("magic", b"\x00" + honest[1:]),
        "mcap-closing-magic": ("magic", honest[:-1] + b"\x00"),
        "mcap-truncated": ("truncated", honest[:-20] + honest[-8:]),
        "mcap-compressed": ("compressed-chunk", compressed),
        "mcap-chunk-crc": ("crc", _byte(honest, message_at, 0x7F)),
        "mcap-undeclared-channel": ("message", undeclared),
        "mcap-redeclared": ("channel", redeclared),
        "mcap-chunk-foreign-record": ("chunk", _foreign_record_bag(schema, channel)),
    }
    for name, (kind, data) in corrupt.items():
        yield name, _mcap_refuses(data, kind)
    yield from _mcap_boundary_controls(schema, channel, messages[0])


def _mcap_refuses(data: bytes, kind: str, *, topic: str | None = None) -> bool:
    """The reader refuses `data` at the named boundary, not at a later one that happens to fail too."""
    try:
        bag = mcap_format.read(data)
        if topic is not None:
            bag.channel_by_topic(topic)
    except mcap_format.McapError as error:
        return error.kind == kind
    return False


def _assemble(
    data_records: bytes,
    *,
    summary: bytes = b"",
    data_crc: int | None = 0,
    footer: tuple[int, int, int] | None = None,
    header: bool = True,
    after_footer: bytes = b"",
) -> bytes:
    """A bag from raw records: header, data records, data end, summary, footer. Each part may be wrong."""
    out = bytearray(mcap_format.MAGIC)
    if header:
        out += mcap_format.record(mcap_format.OP_HEADER, struct.pack("<I", 4) + b"ros2" + struct.pack("<I", 0))
    out += data_records
    if data_crc is not None:
        out += mcap_format.record(mcap_format.OP_DATA_END, struct.pack("<I", data_crc))
    summary_start = len(out) if summary else 0
    out += summary
    start, offset_start, crc = footer if footer is not None else (summary_start, 0, 0)
    out += mcap_format.record(mcap_format.OP_FOOTER, struct.pack("<QQI", start, offset_start, crc))
    return bytes(out + after_footer + mcap_format.MAGIC)


def _chunk_record(inner: bytes, *, declared_size: int | None = None) -> bytes:
    size = len(inner) if declared_size is None else declared_size
    body = struct.pack("<QQQI", 0, 0, size, zlib.crc32(inner)) + struct.pack("<IQ", 0, len(inner)) + inner
    return mcap_format.record(mcap_format.OP_CHUNK, body)


def _mcap_boundary_controls(
    schema: mcap_format.Schema, channel: mcap_format.Channel, message: mcap_format.Message
) -> Iterator[tuple[str, bool]]:
    """One control per refusal in `mcap_format.read`, each on a bag that is otherwise well formed."""
    definitions = mcap_format.schema_record(schema) + mcap_format.channel_record(channel)
    plain = definitions + mcap_format.message_record(message)
    with_crc = _assemble(plain, data_crc=None)
    with_crc = _assemble(plain, data_crc=zlib.crc32(with_crc[: len(with_crc) - 8 - 29]))
    statistics = mcap_format.record(mcap_format.OP_STATISTICS, bytes(46))

    def summary_crc(summary: bytes) -> bytes:
        draft = _assemble(plain, summary=summary)
        start = len(draft) - 8 - 29 - len(summary)
        head = struct.pack("<BQ", mcap_format.OP_FOOTER, 20) + struct.pack("<QQ", start, 0)
        return _assemble(plain, summary=summary, footer=(start, 0, zlib.crc32(summary + head)))

    honest = {
        "mcap-records-outside-chunks": _assemble(plain),
        "mcap-data-crc": with_crc,
        "mcap-summary-crc": summary_crc(statistics),
    }
    for name, data in honest.items():
        yield name, len(mcap_format.read(data).messages) == 1
    bad_text = mcap_format.record(
        mcap_format.OP_CHANNEL,
        struct.pack("<HHI", 1, 1, 2) + b"\xff\xfe" + struct.pack("<I", 3) + b"cdr" + struct.pack("<I", 0),
    )
    schema_body = mcap_format.schema_record(schema)[9:]
    summary_start = len(_assemble(plain)) - 37  # before the 29-byte footer and the 8-byte magic
    past_end = struct.pack("<BQ", mcap_format.OP_METADATA, 1 << 20)
    after_footer = mcap_format.record(mcap_format.OP_DATA_END, bytes(4))
    corrupt = {
        "mcap-too-short": ("magic", mcap_format.MAGIC + mcap_format.MAGIC[:-1]),
        "mcap-record-past-end": ("truncated", mcap_format.MAGIC + past_end + bytes(16) + mcap_format.MAGIC),
        "mcap-field-past-record": ("truncated", _assemble(mcap_format.record(mcap_format.OP_SCHEMA, schema_body[:-1]))),
        "mcap-unread-bytes": ("record", _assemble(mcap_format.record(mcap_format.OP_SCHEMA, schema_body + b"\x00"))),
        "mcap-string-not-utf8": ("string", _assemble(mcap_format.schema_record(schema) + bad_text)),
        "mcap-schema-id-zero": ("schema", _assemble(mcap_format.schema_record(replace(schema, id=0)))),
        "mcap-undeclared-schema": ("channel", _assemble(mcap_format.channel_record(channel))),
        "mcap-no-header": ("header", _assemble(plain, header=False)),
        "mcap-record-after-footer": ("footer", _assemble(plain, after_footer=after_footer)),
        "mcap-no-data-end": ("data-end", _assemble(plain, data_crc=None)),
        "mcap-summary-record-in-data": ("record", _assemble(plain + statistics)),
        "mcap-data-crc-wrong": ("crc", _assemble(plain, data_crc=1)),
        "mcap-data-record-in-summary": ("record", _assemble(plain, summary=mcap_format.message_record(message))),
        "mcap-summary-start": ("footer", _assemble(plain, summary=statistics, footer=(0, 0, 0))),
        "mcap-summary-offset-start": ("footer", _assemble(plain, summary=statistics, footer=(summary_start, 1, 0))),
        "mcap-summary-crc-wrong": ("crc", _assemble(plain, summary=statistics, footer=(summary_start, 0, 1))),
        "mcap-chunk-size": ("chunk", _assemble(_chunk_record(plain, declared_size=len(plain) + 1))),
    }
    for name, (kind, data) in corrupt.items():
        yield name, _mcap_refuses(data, kind)
    twice = _assemble(
        definitions + mcap_format.channel_record(replace(channel, id=2)) + mcap_format.message_record(message)
    )
    yield "mcap-topic-twice", _mcap_refuses(twice, "channel", topic=BAG_CHANNEL)
    yield "mcap-topic-once", mcap_format.read(_assemble(plain)).channel_by_topic(BAG_CHANNEL) == channel


def _honest_bag() -> mcap_format.Bag:
    """A reading that holds every fact the item states. All messages share one payload object."""
    payload = bytes(BAG_FRAME_BYTES)
    start = 1_790_000_000_000_000_000
    return mcap_format.Bag(
        profile="ros2",
        library="exam",
        schemas={1: mcap_format.Schema(1, BAG_TYPE, "ros2msg", bytes(BAG_SCHEMA_BYTES))},
        channels={1: mcap_format.Channel(1, 1, BAG_CHANNEL, "cdr", ())},
        messages=[
            mcap_format.Message(1, 0, start + i * BAG_PERIOD_NS, start + i * BAG_PERIOD_NS, payload)
            for i in range(BAG_FRAMES)
        ],
        chunks=BAG_CHUNKS,
        chunk_compressions=[""] * BAG_CHUNKS,
    )


def _bag_controls() -> Iterator[tuple[str, bool]]:
    """One control per fact `judge_bag` asserts. Each corrupted reading breaks that fact and no other."""
    yield "bag-honest", not _refuses(lambda: judge_bag(_honest_bag()))

    def edit_channel(bag: mcap_format.Bag, **fields: object) -> None:
        bag.channels[1] = replace(bag.channels[1], **fields)

    def edit_schema(bag: mcap_format.Bag, **fields: object) -> None:
        bag.schemas[1] = replace(bag.schemas[1], **fields)

    def edit_message(bag: mcap_format.Bag, index: int, **fields: object) -> None:
        bag.messages[index] = replace(bag.messages[index], **fields)

    def extra_message(bag: mcap_format.Bag) -> None:
        last = bag.messages[-1]
        bag.messages.append(replace(last, channel_id=9, log_time=last.log_time + BAG_PERIOD_NS))

    edits: dict[str, Callable[[mcap_format.Bag], object]] = {
        "bag-no-channel": lambda bag: edit_channel(bag, topic="/velodyne"),
        "bag-chunks": lambda bag: setattr(bag, "chunks", BAG_CHUNKS - 1),
        "bag-compressed-chunk": lambda bag: bag.chunk_compressions.__setitem__(5, "zstd"),
        "bag-channels": lambda bag: bag.channels.__setitem__(2, mcap_format.Channel(2, 1, "/other", "cdr", ())),
        "bag-messages": extra_message,
        "bag-channel-messages": lambda bag: edit_message(bag, BAG_FRAMES - 1, channel_id=9),
        "bag-message-encoding": lambda bag: edit_channel(bag, message_encoding="json"),
        "bag-schema-name": lambda bag: edit_schema(bag, name="sensor_msgs/msg/PointCloud"),
        "bag-schema-encoding": lambda bag: edit_schema(bag, encoding="ros1msg"),
        "bag-schema-bytes": lambda bag: edit_schema(bag, data=bytes(BAG_SCHEMA_BYTES - 1)),
        "bag-message-bytes": lambda bag: edit_message(bag, 7, data=bytes(BAG_FRAME_BYTES - 1)),
        "bag-log-time-step": lambda bag: edit_message(bag, 100, log_time=bag.messages[100].log_time + 1),
    }
    for name, edit in edits.items():
        bag = _honest_bag()
        edit(bag)
        yield name, _refuses(lambda bag=bag: judge_bag(bag))


def _decoder_controls() -> Iterator[tuple[str, bool]]:
    """Every corpus case decodes to its declared verdict, and every accepted one re-encodes to itself."""
    for case in cases():
        data = case.build()
        yield f"corpus-{case.name}", verdict(data) == case.expect
        if case.expect == "accept":
            yield f"roundtrip-{case.name}", roundtrip(data)


def verdict(data: bytes) -> str:
    try:
        decode(data)
    except Refused as refusal:
        return refusal.kind
    return "accept"


def roundtrip(data: bytes) -> bool:
    reading = decode(data)
    store = Store(
        reading.topic.decode(), reading.type_name.decode(), reading.type_hash, reading.schema.decode(),
        reading.period_ns, tuple((f.log_time, data[f.offset:f.offset + f.length]) for f in reading.frames),
    )
    return encode(store) == data


def controls() -> list[tuple[str, bool]]:
    """(name, passed) for every control: the honest form accepted, or the corruption refused."""
    results: list[tuple[str, bool]] = []
    results.extend(_schema_controls(SCHEMA.read_text(encoding="utf-8")))
    results.extend(_decoder_controls())
    results.extend(_transcript_controls())
    results.extend(_fixture_controls())
    results.extend(_converter_controls())
    results.extend(_mcap_controls())
    results.extend(_bag_controls())
    return results
