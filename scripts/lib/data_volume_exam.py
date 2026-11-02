"""The expectation `just data_volume_check` holds a data volume to.

`contracts/data-volume/v1/schema.zt` is the format. This module is an independent encoder and decoder for it,
the disks the exam boots, and the judgement of what the guest probe prints. It shares no code with the
volume builder or the guest reader; it reuses the frame-store exam's independent frame-store encoder and
decoder, which the frame-store item already landed.
"""

from __future__ import annotations

import hashlib
import re
import struct
import uuid
import zlib
from collections.abc import Iterator
from dataclasses import dataclass

import frame_store_exam as frames

SCHEMA = "contracts/data-volume/v1/schema.zt"
BUILDER = "scripts/build/build-data-volume.py"
CLOSURE = "sel4-data-volume"
PROBE = "data-volume-probe"
BUILDER_REFUSALS = ("kind", "output", "payload")

SECTOR = 512
FIRST_LBA = 2048
ENTRY_COUNT = 128
ENTRY_BYTES = 128
ENTRY_SECTORS = ENTRY_COUNT * ENTRY_BYTES // SECTOR
MAX_PAYLOAD_BYTES = 134_217_728
VOLUME_MAGIC = int.from_bytes(b"SLDV", "little")
FORMAT_VERSION = 1
KIND_FRAME_STORE = 1
TYPE_GUID = "8f4c2a91-3b7e-4d6a-9c15-5e2f7a0b4d38"
PARTITION_NAME = "slime-data-volume"
DISK_DOMAIN = b"slime-data-volume-v1:disk:"
PARTITION_DOMAIN = b"slime-data-volume-v1:partition:"
REFUSAL_CLASSES = ("gpt", "partition", "header", "length", "padding", "hash", "frame-store")
HEAP_BOUND = 65536

CONSTANTS = {
    "formatVersion": FORMAT_VERSION, "volumeMagic": VOLUME_MAGIC, "kindFrameStore": KIND_FRAME_STORE,
    "sectorBytes": SECTOR, "firstLba": FIRST_LBA, "gptEntryCount": ENTRY_COUNT, "gptEntryBytes": ENTRY_BYTES,
    "maxPayloadBytes": MAX_PAYLOAD_BYTES,
}
TEXTS = {
    "partitionTypeGuid": TYPE_GUID, "partitionName": PARTITION_NAME,
    "diskGuidDomain": DISK_DOMAIN.decode(), "partitionGuidDomain": PARTITION_DOMAIN.decode(),
}
HEADER_LAYOUT = (("magic", 4, False), ("version", 2, False), ("kind", 2, False), ("payloadBytes", 8, False),
                 ("payloadSha256", 32, True))
VOLUME_HEADER = struct.Struct("<IHHQ32s")
GPT_HEADER = struct.Struct("<8sIIIIQQQQ16sQIII")
GPT_ENTRY = struct.Struct("<16s16sQQQ72s")


class Refused(Exception):
    """The decoder refuses a disk; `kind` is one of `REFUSAL_CLASSES`."""

    def __init__(self, kind: str, detail: str):
        super().__init__(f"{kind}: {detail}")
        assert kind in REFUSAL_CLASSES, kind
        self.kind = kind


def schema_findings(text: str) -> list[str]:
    """Where the schema text differs from the contract this exam pins."""
    findings = []
    for name, value in CONSTANTS.items():
        found = re.findall(rf"^{name} :: Int = (\d+);", text, re.M)
        if found != [str(value)]:
            findings.append(f"{name} must be stated once as {value}, found {found}")
    for name, value in TEXTS.items():
        found = re.findall(rf'^{name} :: Text = "([^"]*)";', text, re.M)
        if found != [value]:
            findings.append(f"{name} must be stated once as {value!r}, found {found}")
    block = re.search(r"^refusalClasses :: List Text = \{(.*?)^\};", text, re.M | re.S)
    if block is None or tuple(re.findall(r'"([a-z-]+)";', block.group(1))) != REFUSAL_CLASSES:
        findings.append(f"refusalClasses must be {list(REFUSAL_CLASSES)} in that order")
    block = re.search(r"^volumeHeaderLayout :: List WireField = \{(.*?)^\};", text, re.M | re.S)
    rows = () if block is None else tuple(
        (name, int(width), array == "true") for name, width, array in re.findall(
            r'\{ name = "(\w+)"; width = (\d+); signed = false; byteArray = (true|false); \};', block.group(1)))
    if block is None or rows != HEADER_LAYOUT or block.group(1).count("{") != len(HEADER_LAYOUT):
        findings.append(f"volumeHeaderLayout must be {HEADER_LAYOUT}")
    return findings


# The format.

def guid(text_or_bytes: str | bytes) -> bytes:
    """A GUID in the GPT mixed-endian encoding, from its text or from 16 raw bytes read as big-endian."""
    value = uuid.UUID(text_or_bytes) if isinstance(text_or_bytes, str) else uuid.UUID(bytes=text_or_bytes)
    return value.bytes_le


def payload_sectors(size: int) -> int:
    return (size + SECTOR - 1) // SECTOR


def geometry(size: int) -> tuple[int, int]:
    """(total sectors, last usable LBA) of the disk for a payload of `size` bytes."""
    last_usable = FIRST_LBA + payload_sectors(size)
    return last_usable + 1 + ENTRY_SECTORS + 1, last_usable


def _protective_mbr(total: int) -> bytes:
    mbr = bytearray(SECTOR)
    mbr[446:462] = bytes((0x00, 0x00, 0x02, 0x00, 0xEE, 0xFF, 0xFF, 0xFF)) + struct.pack(
        "<II", 1, min(total - 1, 0xFFFFFFFF))
    mbr[510:512] = b"\x55\xaa"
    return bytes(mbr)


def _gpt_header(own: int, alternate: int, entries_lba: int, last_usable: int, disk_guid: bytes,
                entries_crc: int) -> bytes:
    fields = [b"EFI PART", 0x00010000, 92, 0, 0, own, alternate, ENTRY_SECTORS + 2, last_usable, disk_guid,
              entries_lba, ENTRY_COUNT, ENTRY_BYTES, entries_crc]
    raw = GPT_HEADER.pack(*fields)
    fields[3] = zlib.crc32(raw)
    return GPT_HEADER.pack(*fields).ljust(SECTOR, b"\0")


def encode(payload: bytes, *, kind: int = KIND_FRAME_STORE) -> bytes:
    """The one disk image of `payload`."""
    digest = hashlib.sha256(payload).digest()
    total, last_usable = geometry(len(payload))
    disk_guid = guid(hashlib.sha256(DISK_DOMAIN + digest).digest()[:16])
    part_guid = guid(hashlib.sha256(PARTITION_DOMAIN + digest).digest()[:16])
    name = PARTITION_NAME.encode("utf-16-le").ljust(72, b"\0")
    entries = GPT_ENTRY.pack(guid(TYPE_GUID), part_guid, FIRST_LBA, last_usable, 0, name).ljust(
        ENTRY_COUNT * ENTRY_BYTES, b"\0")
    crc = zlib.crc32(entries)
    out = bytearray(total * SECTOR)
    out[0:SECTOR] = _protective_mbr(total)
    out[SECTOR:2 * SECTOR] = _gpt_header(1, total - 1, 2, last_usable, disk_guid, crc)
    out[2 * SECTOR:(2 + ENTRY_SECTORS) * SECTOR] = entries
    header = VOLUME_HEADER.pack(VOLUME_MAGIC, FORMAT_VERSION, kind, len(payload), digest)
    out[FIRST_LBA * SECTOR:FIRST_LBA * SECTOR + len(header)] = header
    start = (FIRST_LBA + 1) * SECTOR
    out[start:start + len(payload)] = payload
    backup_entries = total - 1 - ENTRY_SECTORS
    out[backup_entries * SECTOR:(total - 1) * SECTOR] = entries
    out[(total - 1) * SECTOR:] = _gpt_header(total - 1, 1, backup_entries, last_usable, disk_guid, crc)
    return bytes(out)


def _header(disk: bytes, lba: int) -> tuple:
    if len(disk) < (lba + 1) * SECTOR:
        raise Refused("gpt", f"the disk ends before LBA {lba}")
    sector = disk[lba * SECTOR:(lba + 1) * SECTOR]
    fields = list(GPT_HEADER.unpack_from(sector))
    stored = fields[3]
    fields[3] = 0
    if fields[0] != b"EFI PART" or fields[1] != 0x00010000 or fields[2] != 92 or fields[4] != 0:
        raise Refused("gpt", f"the GPT header at LBA {lba} is not a revision 1.0 header")
    if zlib.crc32(GPT_HEADER.pack(*fields)) != stored or any(sector[GPT_HEADER.size:]):
        raise Refused("gpt", f"the GPT header at LBA {lba} fails its CRC32 or holds trailing bytes")
    fields[3] = stored
    return tuple(fields)


def decode(disk: bytes) -> bytes:
    """Validate a disk in the contract's order and return its payload. Raises Refused with the first class."""
    if len(disk) % SECTOR or len(disk) < (FIRST_LBA + 2 + ENTRY_SECTORS + 1) * SECTOR:
        raise Refused("gpt", f"{len(disk)} bytes is not a whole disk of the minimum size")
    total = len(disk) // SECTOR
    mbr = disk[:SECTOR]
    if mbr[510:512] != b"\x55\xaa" or mbr[450] != 0xEE or struct.unpack_from("<II", mbr, 454) != (
            1, min(total - 1, 0xFFFFFFFF)):
        raise Refused("gpt", "the protective MBR is not one 0xEE partition from LBA 1 over the disk")
    primary = _header(disk, 1)
    backup = _header(disk, total - 1)
    if primary[5:7] != (1, total - 1) or backup[5:7] != (total - 1, 1):
        raise Refused("gpt", "the headers do not name their own and each other's LBAs")
    if primary[10] != 2 or backup[10] != total - 1 - ENTRY_SECTORS:
        raise Refused("gpt", "an entry array is not where the layout puts it")
    if primary[7:10] != backup[7:10] or primary[11:] != backup[11:] or primary[11:13] != (ENTRY_COUNT, ENTRY_BYTES):
        raise Refused("gpt", "the two GPT headers disagree or name another entry geometry")
    if primary[7] != ENTRY_SECTORS + 2 or primary[8] >= total - 1 - ENTRY_SECTORS:
        raise Refused("gpt", "the usable LBA range does not fit the disk")
    arrays = []
    for header in (primary, backup):
        start = header[10] * SECTOR
        array = disk[start:start + ENTRY_COUNT * ENTRY_BYTES]
        if zlib.crc32(array) != header[13]:
            raise Refused("gpt", f"the entry array at LBA {header[10]} fails its CRC32")
        arrays.append(array)
    if arrays[0] != arrays[1]:
        raise Refused("gpt", "the two entry arrays differ")
    type_guid, _, first, last, attributes, name = GPT_ENTRY.unpack_from(arrays[0])
    if type_guid != guid(TYPE_GUID):
        raise Refused("partition", "entry 0 is not a data volume")
    if any(arrays[0][ENTRY_BYTES:]):
        raise Refused("partition", "an entry after entry 0 is not all zero")
    if first != FIRST_LBA or last != primary[8] or attributes or name != PARTITION_NAME.encode("utf-16-le").ljust(72, b"\0"):
        raise Refused("partition", "the volume does not span firstLba to the last usable LBA as named")
    header = disk[FIRST_LBA * SECTOR:(FIRST_LBA + 1) * SECTOR]
    magic, version, kind, size, digest = VOLUME_HEADER.unpack_from(header)
    if magic != VOLUME_MAGIC or version != FORMAT_VERSION or kind != KIND_FRAME_STORE or any(header[VOLUME_HEADER.size:]):
        raise Refused("header", "the volume header's magic, version, kind or a reserved byte")
    if not 1 <= size <= MAX_PAYLOAD_BYTES or payload_sectors(size) != last - FIRST_LBA:
        raise Refused("length", f"payload length {size} does not fill the partition's {last - FIRST_LBA} sectors")
    start = (FIRST_LBA + 1) * SECTOR
    payload = disk[start:start + size]
    if any(disk[start + size:(last + 1) * SECTOR]):
        raise Refused("padding", "a byte after the payload in its last sector is not zero")
    if hashlib.sha256(payload).digest() != digest:
        raise Refused("hash", "the payload's SHA-256 is not the header's")
    try:
        frames.decode(payload)
    except frames.Refused as error:
        raise Refused("frame-store", f"the payload is not a frame store: {error}") from error
    return payload


# The disks the exam boots.

def payload() -> bytes:
    """The honest payload: a frame store with a LiDAR-sized frame, an empty frame, and frames that straddle
    sector and 4 KiB request boundaries, its bytes a fixed pseudo-random stream."""
    def stream(seed: int, length: int) -> bytes:
        out = bytearray()
        counter = 0
        while len(out) < length:
            out += hashlib.sha256(b"slime-data-volume-exam:%d:%d" % (seed, counter)).digest()
            counter += 1
        return bytes(out[:length])
    sizes = (494_889, 4097, 1, 0, 70_001, 131_072, 513, 203)
    store = frames.Store(
        topic="/exam/velodyne_points", type_name=frames.BAG_TYPE, type_hash=frames.HASH, schema="uint8 x\n",
        period_ns=frames.BAG_PERIOD_NS,
        frames=tuple((1_790_000_000_000_000_000 + index * frames.BAG_PERIOD_NS, stream(index, size))
                     for index, size in enumerate(sizes)),
    )
    return frames.encode(store)


def _sector(disk: bytes, lba: int, change) -> bytes:
    data = bytearray(disk)
    sector = bytearray(data[lba * SECTOR:(lba + 1) * SECTOR])
    change(sector)
    data[lba * SECTOR:(lba + 1) * SECTOR] = sector
    return bytes(data)


def _entries(disk: bytes, offset: int, value: bytes, *, fix_crc: bool = True) -> bytes:
    """The same disk with `value` written at `offset` of both entry arrays, their CRC32s recomputed or not."""
    total = len(disk) // SECTOR
    data = bytearray(disk)
    for header_lba, entries_lba in ((1, 2), (total - 1, total - 1 - ENTRY_SECTORS)):
        base = entries_lba * SECTOR
        data[base + offset:base + offset + len(value)] = value
        if not fix_crc:
            continue
        fields = list(GPT_HEADER.unpack_from(data, header_lba * SECTOR))
        fields[13] = zlib.crc32(bytes(data[base:base + ENTRY_COUNT * ENTRY_BYTES]))
        fields[3] = 0
        fields[3] = zlib.crc32(GPT_HEADER.pack(*fields))
        data[header_lba * SECTOR:header_lba * SECTOR + GPT_HEADER.size] = GPT_HEADER.pack(*fields)
    return bytes(data)


def not_a_store(store: bytes) -> bytes:
    """`store` with its magic broken: a payload whose header hash matches but the frame-store reader refuses."""
    broken = bytearray(store)
    broken[0] ^= 0xFF
    return bytes(broken)


def corrupted(honest: bytes, store: bytes) -> dict[str, tuple[str, bytes]]:
    """Disks named by the check each is built to fail first, with that check's refusal class. A class has a
    disk for each of its distinct checks, so a reader that skipped one check is caught by its disk."""
    start = (FIRST_LBA + 1) * SECTOR
    size = len(store)
    last = len(honest) // SECTOR - 1

    def flip(offset: int):
        return lambda sector: sector.__setitem__(offset, sector[offset] ^ 0x01)

    return {
        "gpt-header-field": ("gpt", _sector(honest, 1, flip(40))),
        "gpt-header-crc": ("gpt", _sector(honest, 1, flip(16))),
        "gpt-entries-crc": ("gpt", _entries(honest, ENTRY_BYTES + 3, b"\x01", fix_crc=False)),
        "gpt-backup-header": ("gpt", _sector(honest, last, flip(56))),
        "gpt-mbr": ("gpt", _sector(honest, 0, lambda sector: sector.__setitem__(450, 0x83))),
        "partition-type": ("partition", _entries(honest, 0, guid("0fc63daf-8483-4772-8e79-3d69d8477de4"))),
        "partition-second-entry": ("partition", _entries(honest, ENTRY_BYTES + 3, b"\x01")),
        "header-magic": ("header", _sector(honest, FIRST_LBA, flip(0))),
        "header-reserved": ("header", _sector(honest, FIRST_LBA, lambda sector: sector.__setitem__(100, 1))),
        "length": ("length", _sector(honest, FIRST_LBA, lambda sector: sector.__setitem__(
            slice(8, 16), struct.pack("<Q", size + SECTOR)))),
        "padding": ("padding", bytes(bytearray(honest[:start + size]) + b"\x01" + honest[start + size + 1:])),
        "hash": ("hash", bytes(bytearray(honest[:start + 7]) + bytes([honest[start + 7] ^ 0x01]) + honest[start + 8:])),
        "frame-store": ("frame-store", encode(not_a_store(store))),
    }


# What the probe prints.

TAG = f"[{PROBE}] "
VOLUME_LINE = re.compile(r"volume kind=frame-store bytes=(\d+) sha256=([0-9a-f]{64})")
FRAMES_LINE = re.compile(r"frames count=(\d+) period=(\d+) first=(\d+) last=(\d+) digest=([0-9a-f]{64})")
HEAP_LINE = re.compile(r"heap used=(\d+)")


def probe_lines(transcript: str) -> list[str]:
    return [line.split(TAG, 1)[1].strip() for line in transcript.splitlines() if TAG in line]


def volume_line(payload: bytes) -> str:
    return f"volume kind=frame-store bytes={len(payload)} sha256={hashlib.sha256(payload).hexdigest()}"


def expected_volume(store: bytes) -> tuple[str, str]:
    reading = frames.decode(store)
    payloads = b"".join(store[frame.offset:frame.offset + frame.length] for frame in reading.frames)
    return (
        volume_line(store),
        f"frames count={len(reading.frames)} period={reading.period_ns} first={reading.frames[0].log_time} "
        f"last={reading.frames[-1].log_time} digest={hashlib.sha256(payloads).hexdigest()}",
    )


def honest_findings(lines: list[str], store: bytes) -> list[str]:
    volume, frame_line = expected_volume(store)
    if len(lines) != 5:
        return [f"the probe printed {lines}, not the five lines of an accepted volume"]
    findings = []
    for got, want in ((lines[0], volume), (lines[1], frame_line), (lines[2], "write refused"),
                      (lines[4], "complete")):
        if got != want:
            findings.append(f"the probe printed {got!r}, not {want!r}")
    heap = HEAP_LINE.fullmatch(lines[3])
    if heap is None or int(heap.group(1)) > HEAP_BOUND:
        findings.append(f"the probe's heap line {lines[3]!r} is not at most {HEAP_BOUND} bytes")
    return findings


def refusal_findings(lines: list[str], kind: str, store: bytes) -> list[str]:
    wanted = ([volume_line(not_a_store(store))] if kind == "frame-store" else []) + [f"refused {kind}"]
    return [] if lines == wanted else [f"the probe printed {lines}, not {wanted}"]


DRIVER_TAG = "[virtio-blk-driver] "
DRIVER_AUTHORITY = "authority rings=1 rights=read source=generation"
DRIVER_REFUSAL = "refused write ring=0 rights=read"


def driver_findings(transcript: str) -> list[str]:
    """The block driver, which enforces the ring's rights, must report the read-only authority it read and
    the one write it refused, before the probe reports the refusal."""
    authority = refused = probe_refusal = None
    for index, line in enumerate(transcript.splitlines()):
        if DRIVER_TAG in line:
            text = line.split(DRIVER_TAG, 1)[1].strip()
            if text.startswith("authority ") and authority is None:
                authority = (index, text)
            if text.startswith("refused "):
                refused = (index, text) if refused is None else (-1, "more than one refusal")
        elif TAG in line and line.split(TAG, 1)[1].strip() == "write refused":
            probe_refusal = index
    findings = []
    if authority is None or authority[1] != DRIVER_AUTHORITY:
        findings.append(f"the driver reported authority {authority and authority[1]!r}, not {DRIVER_AUTHORITY!r}")
    if refused is None or refused[1] != DRIVER_REFUSAL:
        findings.append(f"the driver reported {refused and refused[1]!r}, not one {DRIVER_REFUSAL!r}")
    elif probe_refusal is None or refused[0] > probe_refusal:
        findings.append("the driver's refusal does not precede the probe's")
    return findings


BUILDER_REFUSAL = re.compile(r"build-data-volume: refused ([a-z-]+)")


def builder_refusal(stderr: str) -> str | None:
    lines = [line for line in stderr.splitlines() if line.strip()]
    match = BUILDER_REFUSAL.fullmatch(lines[-1].strip()) if lines else None
    return match.group(1) if match else None


# Controls.

@dataclass(frozen=True)
class Corpus:
    store: bytes
    honest: bytes
    broken: dict[str, tuple[str, bytes]]


def corpus() -> Corpus:
    store = payload()
    honest = encode(store)
    return Corpus(store, honest, corrupted(honest, store))


def _verdict(disk: bytes) -> str:
    try:
        decode(disk)
    except Refused as refusal:
        return refusal.kind
    return "accepted"


def _decoder_controls(data: Corpus) -> Iterator[tuple[str, bool]]:
    yield "decode-honest", _verdict(data.honest) == "accepted" and decode(data.honest) == data.store
    for name, (kind, disk) in data.broken.items():
        yield f"decode-{name}", _verdict(disk) == kind
    yield "decode-every-class-covered", {kind for kind, _ in data.broken.values()} == set(REFUSAL_CLASSES)
    small = frames.encode(frames.BASE)
    yield "decode-small-store", decode(encode(small)) == small
    yield "decode-truncated-disk", _verdict(data.honest[:-SECTOR]) == "gpt"
    yield "decode-backup-header", _verdict(_sector(data.honest, len(data.honest) // SECTOR - 1,
                                                   lambda s: s.__setitem__(24, s[24] ^ 1))) == "gpt"
    yield "decode-mbr", _verdict(_sector(data.honest, 0, lambda s: s.__setitem__(450, 0x83))) == "gpt"
    yield "decode-header-reserved", _verdict(_sector(data.honest, FIRST_LBA, lambda s: s.__setitem__(100, 1))) == "header"
    yield "decode-zero-length", _verdict(_sector(data.honest, FIRST_LBA, lambda s: s.__setitem__(
        slice(8, 16), struct.pack("<Q", 0)))) == "length"
    yield "decode-short-length", _verdict(_sector(data.honest, FIRST_LBA, lambda s: s.__setitem__(
        slice(8, 16), struct.pack("<Q", len(data.store) - SECTOR)))) == "length"
    yield "decode-kind", _verdict(encode(data.store, kind=2)) == "header"
    yield "encode-deterministic", encode(data.store) == data.honest
    other = bytearray(data.store)
    other[-1] ^= 1
    yield "encode-guid-follows-payload", encode(bytes(other))[SECTOR + 56:SECTOR + 72] != data.honest[SECTOR + 56:SECTOR + 72]


def _transcript_controls(data: Corpus) -> Iterator[tuple[str, bool]]:
    volume, frame_line = expected_volume(data.store)
    honest = [volume, frame_line, "write refused", "heap used=4096", "complete"]
    yield "probe-honest", not honest_findings(honest, data.store)
    cases = {
        "probe-missing-complete": honest[:4],
        "probe-extra-line": [*honest, "complete"],
        "probe-wrong-bytes": [volume.replace(f"bytes={len(data.store)}", f"bytes={len(data.store) + 1}"), *honest[1:]],
        "probe-wrong-sha": [volume[:-1] + ("0" if volume[-1] != "0" else "1"), *honest[1:]],
        "probe-wrong-digest": [volume, frame_line[:-1] + ("0" if frame_line[-1] != "0" else "1"), *honest[2:]],
        "probe-wrong-count": [volume, frame_line.replace("count=8", "count=7"), *honest[2:]],
        "probe-no-write-refusal": [volume, frame_line, "write accepted", *honest[3:]],
        "probe-heap-over": [volume, frame_line, "write refused", f"heap used={HEAP_BOUND + 1}", "complete"],
        "probe-reordered": [frame_line, volume, *honest[2:]],
    }
    for label, lines in cases.items():
        yield label, bool(honest_findings(lines, data.store))
    for kind in REFUSAL_CLASSES:
        good = [volume_line(not_a_store(data.store)), "refused frame-store"] if kind == "frame-store" else [f"refused {kind}"]
        yield f"probe-refusal-{kind}", not refusal_findings(good, kind, data.store)
        other = REFUSAL_CLASSES[(REFUSAL_CLASSES.index(kind) + 1) % len(REFUSAL_CLASSES)]
        yield f"probe-refusal-{kind}-wrong-class", bool(refusal_findings([f"refused {other}"], kind, data.store))
        yield f"probe-refusal-{kind}-then-complete", bool(refusal_findings([*good, "complete"], kind, data.store))
    yield "probe-refusal-hash-after-volume", bool(refusal_findings([volume, "refused hash"], "hash", data.store))
    yield "probe-refusal-frame-store-honest-volume", bool(refusal_findings([volume, "refused frame-store"], "frame-store", data.store))
    good = (f"{DRIVER_TAG}{DRIVER_AUTHORITY}\n{DRIVER_TAG}{DRIVER_REFUSAL}\n{TAG}write refused\n")
    yield "driver-honest", not driver_findings(good)
    for label, text in {
        "driver-read-write": good.replace("rights=read source", "rights=read,write source"),
        "driver-no-refusal": good.replace(f"{DRIVER_TAG}{DRIVER_REFUSAL}\n", ""),
        "driver-two-refusals": good.replace(f"{TAG}write", f"{DRIVER_TAG}{DRIVER_REFUSAL}\n{TAG}write"),
        "driver-after-probe": f"{DRIVER_TAG}{DRIVER_AUTHORITY}\n{TAG}write refused\n{DRIVER_TAG}{DRIVER_REFUSAL}\n",
        "driver-probe-only": f"{TAG}write refused\n",
        "driver-other-ring": good.replace("ring=0 rights=read\n", "ring=1 rights=read\n"),
    }.items():
        yield label, bool(driver_findings(text))
    yield "builder-refusal-honest", builder_refusal("x\nbuild-data-volume: refused payload\n") == "payload"
    yield "builder-refusal-not-last", builder_refusal("build-data-volume: refused payload\ndone\n") is None
    yield "builder-refusal-other-tool", builder_refusal("convert-mcap: refused payload\n") is None


def controls() -> list[tuple[str, bool]]:
    """Each control and whether it held."""
    data = corpus()
    return [*_decoder_controls(data), *_transcript_controls(data)]
