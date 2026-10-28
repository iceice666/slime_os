"""A reader and a writer of the public MCAP container format, held by the frame-store exam.

MCAP (https://mcap.dev/spec) is an external format that Slime OS does not own, so this module reads
it by the specification's layout. It is the exam's independent reading of a bag: nothing here
imports the converter it judges.

The reader is strict on every boundary the exam claims. It refuses a missing or wrong magic, a
record that runs past its container, an unknown record in a chunk, a chunk whose compression is not
empty, a chunk whose records do not fill `uncompressed_size`, a nonzero CRC that does not match, a
message on an undeclared channel, a schema or channel id declared twice with different content,
and a footer that does not end the file. It reads every message in file order and never consults
the summary section for message data.

The writer builds small bags for the converter's cases: chunks with their message indexes, a data
end record that declares no data-section CRC, and a summary of schemas, channels, statistics and chunk
indexes. A chunk may be compressed as a zstd frame of raw blocks, which every zstd decoder accepts.
"""

from __future__ import annotations

import struct
import zlib
from dataclasses import dataclass, field

MAGIC = b"\x89MCAP0\r\n"

OP_HEADER = 0x01
OP_FOOTER = 0x02
OP_SCHEMA = 0x03
OP_CHANNEL = 0x04
OP_MESSAGE = 0x05
OP_CHUNK = 0x06
OP_MESSAGE_INDEX = 0x07
OP_CHUNK_INDEX = 0x08
OP_ATTACHMENT = 0x09
OP_ATTACHMENT_INDEX = 0x0A
OP_STATISTICS = 0x0B
OP_METADATA = 0x0C
OP_METADATA_INDEX = 0x0D
OP_SUMMARY_OFFSET = 0x0E
OP_DATA_END = 0x0F

DATA_SECTION_OPS = frozenset(
    {OP_SCHEMA, OP_CHANNEL, OP_MESSAGE, OP_CHUNK, OP_MESSAGE_INDEX, OP_ATTACHMENT, OP_METADATA}
)
SUMMARY_SECTION_OPS = frozenset(
    {
        OP_SCHEMA, OP_CHANNEL, OP_CHUNK_INDEX, OP_ATTACHMENT_INDEX, OP_STATISTICS,
        OP_METADATA_INDEX, OP_SUMMARY_OFFSET,
    }
)
CHUNK_OPS = frozenset({OP_SCHEMA, OP_CHANNEL, OP_MESSAGE})


class McapError(Exception):
    """The bag is not a well-formed uncompressed MCAP file. `kind` names the boundary."""

    def __init__(self, kind: str, detail: str):
        super().__init__(f"{kind}: {detail}")
        self.kind = kind


@dataclass(frozen=True)
class Schema:
    id: int
    name: str
    encoding: str
    data: bytes


@dataclass(frozen=True)
class Channel:
    id: int
    schema_id: int
    topic: str
    message_encoding: str
    metadata: tuple[tuple[str, str], ...]


@dataclass(frozen=True)
class Message:
    channel_id: int
    sequence: int
    log_time: int
    publish_time: int
    data: bytes


@dataclass
class Bag:
    profile: str
    library: str
    schemas: dict[int, Schema] = field(default_factory=dict)
    channels: dict[int, Channel] = field(default_factory=dict)
    messages: list[Message] = field(default_factory=list)
    chunks: int = 0
    chunk_compressions: list[str] = field(default_factory=list)

    def channel_by_topic(self, topic: str) -> Channel | None:
        found = [channel for channel in self.channels.values() if channel.topic == topic]
        if len(found) > 1:
            raise McapError("channel", f"topic {topic!r} is declared by {len(found)} channels")
        return found[0] if found else None


class _Cursor:
    def __init__(self, data: bytes, start: int, end: int, where: str):
        self.data = data
        self.position = start
        self.end = end
        self.where = where

    def take(self, count: int) -> bytes:
        if count < 0 or self.position + count > self.end:
            raise McapError("truncated", f"{self.where} needs {count} bytes at {self.position}, ends at {self.end}")
        value = self.data[self.position:self.position + count]
        self.position += count
        return value

    def unpack(self, form: str) -> tuple:
        return struct.unpack("<" + form, self.take(struct.calcsize("<" + form)))

    def u8(self) -> int:
        return self.unpack("B")[0]

    def u16(self) -> int:
        return self.unpack("H")[0]

    def u32(self) -> int:
        return self.unpack("I")[0]

    def u64(self) -> int:
        return self.unpack("Q")[0]

    def string(self) -> str:
        raw = self.take(self.u32())
        try:
            return raw.decode("utf-8")
        except UnicodeDecodeError as error:
            raise McapError("string", f"{self.where} holds a string that is not UTF-8") from error

    def u32_bytes(self) -> bytes:
        return self.take(self.u32())

    def string_map(self) -> tuple[tuple[str, str], ...]:
        inner = _Cursor(self.data, self.position, self.position + 4, self.where)
        length = inner.u32()
        self.position += 4
        inner = _Cursor(self.data, self.position, self.position + length, self.where + " map")
        self.take(length)
        pairs = []
        while inner.position < inner.end:
            pairs.append((inner.string(), inner.string()))
        return tuple(pairs)

    def done(self) -> None:
        if self.position != self.end:
            raise McapError("record", f"{self.where} leaves {self.end - self.position} unread bytes")


def _records(data: bytes, start: int, end: int, where: str):
    """Yield (opcode, record start, body start, body end) for consecutive records in [start, end)."""
    position = start
    while position < end:
        cursor = _Cursor(data, position, end, where)
        opcode = cursor.u8()
        length = cursor.u64()
        body = cursor.position
        if length > end - body:
            raise McapError("truncated", f"{where} record 0x{opcode:02x} at {position} runs past {end}")
        yield opcode, position, body, body + length
        position = body + length


def _schema(data: bytes, start: int, end: int) -> Schema:
    cursor = _Cursor(data, start, end, "schema")
    schema = Schema(cursor.u16(), cursor.string(), cursor.string(), cursor.u32_bytes())
    cursor.done()
    if schema.id == 0:
        raise McapError("schema", "schema id 0 is reserved")
    return schema


def _channel(data: bytes, start: int, end: int) -> Channel:
    cursor = _Cursor(data, start, end, "channel")
    channel = Channel(cursor.u16(), cursor.u16(), cursor.string(), cursor.string(), cursor.string_map())
    cursor.done()
    return channel


def _message(data: bytes, start: int, end: int) -> Message:
    cursor = _Cursor(data, start, end, "message")
    channel_id, sequence, log_time, publish_time = cursor.unpack("HIQQ")
    return Message(channel_id, sequence, log_time, publish_time, data[cursor.position:end])


def _declare(table: dict, item, kind: str) -> None:
    previous = table.get(item.id)
    if previous is not None and previous != item:
        raise McapError(kind, f"{kind} id {item.id} is declared twice with different content")
    table[item.id] = item


def _take_definition(bag: Bag, opcode: int, data: bytes, start: int, end: int) -> None:
    if opcode == OP_SCHEMA:
        _declare(bag.schemas, _schema(data, start, end), "schema")
    elif opcode == OP_CHANNEL:
        channel = _channel(data, start, end)
        if channel.schema_id != 0 and channel.schema_id not in bag.schemas:
            raise McapError("channel", f"channel {channel.id} names undeclared schema {channel.schema_id}")
        _declare(bag.channels, channel, "channel")
    elif opcode == OP_MESSAGE:
        message = _message(data, start, end)
        if message.channel_id not in bag.channels:
            raise McapError("message", f"a message names undeclared channel {message.channel_id}")
        bag.messages.append(message)


def read(data: bytes) -> Bag:
    """Read a whole uncompressed MCAP bag. Raises `McapError` on any boundary above."""
    if len(data) < 2 * len(MAGIC) or data[:len(MAGIC)] != MAGIC:
        raise McapError("magic", "the file does not start with the MCAP magic")
    if data[-len(MAGIC):] != MAGIC:
        raise McapError("magic", "the file does not end with the MCAP magic")
    end = len(data) - len(MAGIC)
    records = list(_records(data, len(MAGIC), end, "file"))
    if not records or records[0][0] != OP_HEADER:
        raise McapError("header", "the first record is not a header")
    if records[-1][0] != OP_FOOTER or records[-1][3] != end:
        raise McapError("footer", "the last record is not a footer that ends at the closing magic")
    header = _Cursor(data, records[0][2], records[0][3], "header")
    bag = Bag(header.string(), header.string())
    header.done()

    data_end_index = next((i for i, record in enumerate(records) if record[0] == OP_DATA_END), None)
    if data_end_index is None:
        raise McapError("data-end", "the file has no data end record")
    for opcode, start, body, body_end in records[1:data_end_index]:
        if opcode not in DATA_SECTION_OPS:
            raise McapError("record", f"record 0x{opcode:02x} at {start} is not a data-section record")
        if opcode == OP_CHUNK:
            _read_chunk(bag, data, body, body_end)
        elif opcode in CHUNK_OPS:
            _take_definition(bag, opcode, data, body, body_end)

    _, data_end_start, body, body_end = records[data_end_index]
    data_end = _Cursor(data, body, body_end, "data end")
    data_crc = data_end.u32()
    data_end.done()
    # A nonzero data-section CRC covers the file from its first byte to the data end record.
    if data_crc and zlib.crc32(data[:data_end_start]) != data_crc:
        raise McapError("crc", "the data section CRC does not match")

    summary = records[data_end_index + 1:-1]
    for opcode, start, _, _ in summary:
        if opcode not in SUMMARY_SECTION_OPS:
            raise McapError("record", f"record 0x{opcode:02x} at {start} is not a summary-section record")
    footer_start, footer_body, footer_end = records[-1][1:]
    footer = _Cursor(data, footer_body, footer_end, "footer")
    summary_start, summary_offset_start, summary_crc = footer.unpack("QQI")
    footer.done()
    expected_summary = summary[0][1] if summary else 0
    if summary_start != expected_summary:
        raise McapError("footer", f"summary start {summary_start} is not {expected_summary}")
    if summary_offset_start:
        offsets = [record[1] for record in summary if record[0] == OP_SUMMARY_OFFSET]
        if not offsets or offsets[0] != summary_offset_start:
            raise McapError("footer", "the summary offset start does not name the first summary offset")
    if summary_crc:
        covered = data[summary_start or footer_start:footer_end - 4]
        if zlib.crc32(covered) != summary_crc:
            raise McapError("crc", "the summary CRC does not match")
    for opcode, _, body, body_end in summary:
        if opcode == OP_SCHEMA:
            _declare(bag.schemas, _schema(data, body, body_end), "schema")
        elif opcode == OP_CHANNEL:
            _declare(bag.channels, _channel(data, body, body_end), "channel")
    return bag


def _read_chunk(bag: Bag, data: bytes, start: int, end: int) -> None:
    cursor = _Cursor(data, start, end, "chunk")
    _start_time, _end_time, uncompressed_size, uncompressed_crc = cursor.unpack("QQQI")
    compression = cursor.string()
    records_length = cursor.u64()
    records_start = cursor.position
    cursor.take(records_length)
    cursor.done()
    bag.chunks += 1
    bag.chunk_compressions.append(compression)
    if compression != "":
        raise McapError("compressed-chunk", f"chunk {bag.chunks} is compressed with {compression!r}")
    if records_length != uncompressed_size:
        raise McapError("chunk", f"chunk {bag.chunks} holds {records_length} bytes, declares {uncompressed_size}")
    records = data[records_start:records_start + records_length]
    if uncompressed_crc and zlib.crc32(records) != uncompressed_crc:
        raise McapError("crc", f"chunk {bag.chunks} CRC does not match")
    for opcode, record_start, body, body_end in _records(data, records_start, records_start + records_length, "chunk"):
        if opcode not in CHUNK_OPS:
            raise McapError("chunk", f"record 0x{opcode:02x} at {record_start} may not sit in a chunk")
        _take_definition(bag, opcode, data, body, body_end)


# The writer.

def _string(text: str) -> bytes:
    raw = text.encode("utf-8")
    return struct.pack("<I", len(raw)) + raw


def _string_map(pairs: tuple[tuple[str, str], ...]) -> bytes:
    body = b"".join(_string(key) + _string(value) for key, value in pairs)
    return struct.pack("<I", len(body)) + body


def record(opcode: int, body: bytes) -> bytes:
    return struct.pack("<BQ", opcode, len(body)) + body


def schema_record(schema: Schema) -> bytes:
    body = struct.pack("<H", schema.id) + _string(schema.name) + _string(schema.encoding)
    return record(OP_SCHEMA, body + struct.pack("<I", len(schema.data)) + schema.data)


def channel_record(channel: Channel) -> bytes:
    body = struct.pack("<HH", channel.id, channel.schema_id) + _string(channel.topic)
    return record(OP_CHANNEL, body + _string(channel.message_encoding) + _string_map(channel.metadata))


def message_record(message: Message) -> bytes:
    head = struct.pack("<HIQQ", message.channel_id, message.sequence, message.log_time, message.publish_time)
    return record(OP_MESSAGE, head + message.data)


def zstd_raw_frame(payload: bytes) -> bytes:
    """A zstd frame (RFC 8878) of raw blocks: single segment, an 8-byte content size, no checksum."""
    frame = struct.pack("<IB", 0xFD2FB528, 0xE0) + struct.pack("<Q", len(payload))
    blocks = [payload[i:i + 0x1FFFF] for i in range(0, len(payload), 0x1FFFF)] or [b""]
    for index, block in enumerate(blocks):
        last = 1 if index == len(blocks) - 1 else 0
        frame += (last | (len(block) << 3)).to_bytes(3, "little") + block
    return frame


@dataclass(frozen=True)
class ChunkPlan:
    """One chunk to write: definitions and messages in record order, and its compression."""

    items: tuple[Schema | Channel | Message, ...]
    compression: str = ""


def write(chunks: tuple[ChunkPlan, ...], *, profile: str = "ros2", library: str = "slime-frame-store-exam") -> bytes:
    """A complete MCAP file: header, chunks with message indexes, data end, summary, footer."""
    out = bytearray(MAGIC)
    out += record(OP_HEADER, _string(profile) + _string(library))
    schemas: dict[int, Schema] = {}
    channels: dict[int, Channel] = {}
    chunk_indexes = []
    channel_counts: dict[int, int] = {}
    all_times = []
    for plan in chunks:
        inner = bytearray()
        offsets: dict[int, list[tuple[int, int]]] = {}
        times = []
        for item in plan.items:
            if isinstance(item, Schema):
                schemas[item.id] = item
                inner += schema_record(item)
            elif isinstance(item, Channel):
                channels[item.id] = item
                inner += channel_record(item)
            else:
                offsets.setdefault(item.channel_id, []).append((item.log_time, len(inner)))
                channel_counts[item.channel_id] = channel_counts.get(item.channel_id, 0) + 1
                times.append(item.log_time)
                inner += message_record(item)
        raw = bytes(inner)
        stored = raw if plan.compression == "" else zstd_raw_frame(raw)
        start_time, end_time = (min(times), max(times)) if times else (0, 0)
        all_times.extend(times)
        chunk_body = struct.pack("<QQQI", start_time, end_time, len(raw), zlib.crc32(raw))
        chunk_body += _string(plan.compression) + struct.pack("<Q", len(stored)) + stored
        chunk_start = len(out)
        out += record(OP_CHUNK, chunk_body)
        chunk_length = len(out) - chunk_start
        index_offsets = {}
        index_start = len(out)
        for channel_id in sorted(offsets):
            entries = b"".join(struct.pack("<QQ", time, offset) for time, offset in offsets[channel_id])
            index_offsets[channel_id] = len(out)
            out += record(OP_MESSAGE_INDEX, struct.pack("<HI", channel_id, len(entries)) + entries)
        index_map = b"".join(struct.pack("<HQ", key, value) for key, value in sorted(index_offsets.items()))
        chunk_indexes.append(
            struct.pack("<QQQQ", start_time, end_time, chunk_start, chunk_length)
            + struct.pack("<I", len(index_map)) + index_map
            + struct.pack("<Q", len(out) - index_start)
            + _string(plan.compression)
            + struct.pack("<QQ", len(stored), len(raw))
        )
    # Zero declares no data-section CRC. The specification does not say whether the leading magic is
    # covered, so a nonzero value could make a converter that reads it the other way refuse the bag.
    out += record(OP_DATA_END, struct.pack("<I", 0))
    summary_start = len(out)
    for schema in schemas.values():
        out += schema_record(schema)
    for channel in channels.values():
        out += channel_record(channel)
    counts = b"".join(struct.pack("<HQ", key, value) for key, value in sorted(channel_counts.items()))
    start_time, end_time = (min(all_times), max(all_times)) if all_times else (0, 0)
    statistics = struct.pack("<QHIIII", sum(channel_counts.values()), len(schemas), len(channels), 0, 0, len(chunks))
    statistics += struct.pack("<QQ", start_time, end_time) + struct.pack("<I", len(counts)) + counts
    out += record(OP_STATISTICS, statistics)
    for body in chunk_indexes:
        out += record(OP_CHUNK_INDEX, body)
    footer_head = struct.pack("<BQ", OP_FOOTER, 20) + struct.pack("<QQ", summary_start, 0)
    summary_crc = zlib.crc32(bytes(out[summary_start:]) + footer_head)
    out += footer_head + struct.pack("<I", summary_crc)
    out += MAGIC
    return bytes(out)
