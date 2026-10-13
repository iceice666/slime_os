"""Host reference for Zenoh transport fragmentation, written independently of the Rust codec.

A Put larger than a batch is sent as FRAGMENT batches. Each carries the reliable flag, a "more
follows" flag on every fragment but the last, a sequence number, and a slice of the serialized
network message. Only the first slice starts the message, so a receiver decodes nothing until the
last fragment arrives and the slices are joined.

    FRAGMENT   <hdr 0x26 | 0x40 if more><sn:leb128><slice>

The bytes belong to eclipse-zenoh. The captures below were taken from a stock
`eclipse-zenoh==1.0.0` peer through a logging TCP proxy, one run each, on one x86_64 Linux host
(wheel sha256 `1763912951ecd2a3706f668faf0541245387096b37a4fefb73af7848dadd46ff`, peer mode,
scouting and gossip off, `transport/link/tx/batch_size` set per run). The peer was given one Put of
a payload of the stated size with the demo's 33-byte attachment; each capture is the sequence of
FRAGMENT bodies it wrote, pinned by length, header and sha256. `reference_split` is the rule the
captures obey, and `self_test` proves the rule reproduces every pinned body, so nothing here is
derived from the Rust implementation.

What this does not cover: a peer other than this one, a batch size other than 512 and 2048,
unreliable fragments, extensions on a fragment, or fragments of anything but one Put with the
33-byte attachment.
"""

from __future__ import annotations

import hashlib
import struct
from dataclasses import dataclass

import zenoh_wire

ID_FRAGMENT = 0x06
FLAG_R = 0x20
FLAG_M = 0x40
FLAG_Z = 0x80
GID = bytes(0xE0 + i for i in range(16))
STREAM_LENGTH_BYTES = 2


class FragmentError(ValueError):
    """Input the reference refuses to encode or decode."""


def read_leb32(data: bytes, at: int, bits: int = 32) -> tuple[int, int]:
    """A canonical LEB128 that fits `bits` bits, read the way the Rust reader reads it.

    At most `ceil(bits / 7)` bytes; a value above `bits` bits is an overflow; a multi-byte value
    whose last byte is zero is not canonical. `zenoh_wire.read_leb128` accepts all three, so the
    fragment reference does not use it for a sequence number, a scope or a length. A key-expression
    scope is read with 16 bits, as the Rust decoder reads it (`LEB128_SHORT_BITS`)."""
    value = 0
    for count in range((bits + 6) // 7):
        if at + count >= len(data):
            raise FragmentError("truncated LEB128")
        byte = data[at + count]
        value |= (byte & 0x7F) << (7 * count)
        if not byte & 0x80:
            if value >> bits:
                raise FragmentError(f"LEB128 over {bits} bits")
            if count > 0 and byte & 0x7F == 0:
                raise FragmentError("noncanonical LEB128")
            return value, at + count + 1
    raise FragmentError(f"LEB128 over {bits} bits")


def payload_bytes(size: int) -> bytes:
    """The payload the captures carried: a fixed pattern, so a reassembled message is checkable."""
    return bytes((i * 7 + 1) % 251 for i in range(size))


def attachment() -> bytes:
    """The 33-byte attachment the captures carried: sequence 0, timestamp 1000, the demo GID."""
    return struct.pack("<qq", 0, 1000) + bytes([16]) + GID


def message(size: int) -> bytes:
    """The serialized network message the captures split: a PUSH with a Put, the alias form.

    Header 0x1d, scope 1 (the alias the stock peer declared), then the Put with the attachment
    extension, its length, the attachment, the payload length and the payload."""
    att = attachment()
    return (
        bytes([0x1D])
        + zenoh_wire.leb128(1)
        + bytes([zenoh_wire.ID_PUT | zenoh_wire.FLAG_Z, zenoh_wire.EXT_ATTACHMENT])
        + zenoh_wire.leb128(len(att))
        + att
        + zenoh_wire.leb128(size)
        + payload_bytes(size)
    )


def reference_split(msg: bytes, batch: int, first_sn: int) -> list[bytes]:
    """Split `msg` into FRAGMENT bodies for a link whose batch is `batch` bytes.

    A fragment body is at most `batch - 2` bytes, the stream length being the other two. Sequence
    numbers are consecutive from `first_sn`. Every body but the last carries the "more" flag."""
    if batch <= STREAM_LENGTH_BYTES + 6:
        raise FragmentError("a batch cannot hold a fragment header and a slice")
    bodies: list[bytes] = []
    at, sn = 0, first_sn
    while at < len(msg):
        header_len = 1 + len(zenoh_wire.leb128(sn))
        chunk = msg[at : at + (batch - STREAM_LENGTH_BYTES - header_len)]
        at += len(chunk)
        more = at < len(msg)
        bodies.append(bytes([ID_FRAGMENT | FLAG_R | (FLAG_M if more else 0)]) + zenoh_wire.leb128(sn) + chunk)
        sn = (sn + 1) & 0xFFFFFFFF
    return bodies


@dataclass(frozen=True)
class Fragment:
    more: bool
    sn: int
    data: bytes


def decode_fragment(body: bytes, batch: int | None = None) -> Fragment:
    """Decode one FRAGMENT body; refuse anything that is not exactly the reliable form.

    When the batch the link announced is given, a body longer than the batch less the two-byte
    stream length is refused: the peer was told that size and must not send more."""
    if not body:
        raise FragmentError("empty fragment")
    if batch is not None and len(body) > batch - STREAM_LENGTH_BYTES:
        raise FragmentError("a fragment longer than the announced batch")
    header = body[0]
    if header & 0x1F != ID_FRAGMENT:
        raise FragmentError("not a FRAGMENT")
    if header & FLAG_R == 0:
        raise FragmentError("an unreliable fragment")
    if header & FLAG_Z:
        raise FragmentError("a fragment with extensions")
    sn, at = read_leb32(body, 1)
    if at >= len(body):
        raise FragmentError("a fragment with no data")
    return Fragment(more=bool(header & FLAG_M), sn=sn, data=body[at:])


def reassemble(bodies: list[bytes], expected_sn: int | None = None, batch: int | None = None) -> bytes:
    """Join a run of FRAGMENT bodies into the message, refusing a run that is not a whole message.

    The run must be consecutive sequence numbers, every fragment but the last marked "more", and
    the last not. The sequence number wraps at 32 bits, as the transport's does. When the session's
    next expected sequence number is given, the first fragment must carry exactly it: a run replayed
    from an earlier point, or one that skips ahead, is refused, as a FRAME with the wrong sequence
    number is."""
    if not bodies:
        raise FragmentError("no fragments")
    parts = [decode_fragment(body, batch) for body in bodies]
    if expected_sn is not None and parts[0].sn != expected_sn & 0xFFFFFFFF:
        raise FragmentError("the first fragment is not the session's next sequence number")
    for earlier, later in zip(parts, parts[1:], strict=False):
        if later.sn != (earlier.sn + 1) & 0xFFFFFFFF:
            raise FragmentError("fragment sequence numbers are not consecutive")
    if any(not part.more for part in parts[:-1]):
        raise FragmentError("a fragment marked final is followed by another")
    if parts[-1].more:
        raise FragmentError("the last fragment still says more follows")
    return b"".join(part.data for part in parts)


def next_sn(first_sn: int, count: int) -> int:
    """The session's next expected sequence number after a run of `count` fragments: each fragment
    uses one, and the number wraps at 32 bits."""
    return (first_sn + count) & 0xFFFFFFFF


PUSH_ALIAS = 0x1D


def full_key_for_exam(size: int, key: str = "0/slime_demo/counter/slime_demo_msgs::msg::dds_::Counter_/RIHS01_a82fd5ffcb96d0a197a5ad3680d1c4e6ba43a962928ecd592fb565eb8129595b") -> bytes:
    """A Put naming the demo key in full, with the 33-byte attachment and `size` payload bytes."""
    return zenoh_wire.push_put(key, attachment(), payload_bytes(size), True)

PUSH_FULL_KEY = zenoh_wire.ID_PUSH | zenoh_wire.FLAG_N | zenoh_wire.FLAG_M


def key_expression(raw: bytes) -> str:
    """The Rust decoder's `key_expression`, written again: valid UTF-8, no wildcard, no `?` or `#`, no
    leading or trailing `/` and no `//`. A wildcard is its own class; the rest are invalid."""
    try:
        key = raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise FragmentError("invalid-keyexpr: the key is not UTF-8") from error
    if "*" in key or "$" in key:
        raise FragmentError("wildcard-keyexpr: the key holds a wildcard")
    if "?" in key or "#" in key or key.startswith("/") or key.endswith("/") or "//" in key:
        raise FragmentError("invalid-keyexpr: the key is not a valid key expression")
    return key


def decode_message(
    msg: bytes,
    alias: int | None = None,
    max_key: int | None = None,
    max_payload: int | None = None,
    subscribed: str | None = None,
) -> dict:
    """Decode the reassembled PUSH with a Put: the key, the attachment and the payload.

    The key is either the alias a stock peer declared (header 0x1d, a nonzero 16-bit scope and no
    key bytes) or the key in full (header 0x7d, scope 0, a 16-bit length and the bytes). Given the
    session's declared alias, a scope that is another alias is refused: it was never declared. Given
    the configuration's key and payload sizes, a longer key or payload is refused. A full key must be a
    valid key expression, and given the key the session's subscriber declared, a Put for another key
    is refused as the whole-frame session refuses it."""
    if not msg or msg[0] not in (PUSH_ALIAS, PUSH_FULL_KEY):
        raise FragmentError("not a PUSH in the alias or full-key form")
    named = msg[0] == PUSH_FULL_KEY
    scope, at = read_leb32(msg, 1, 16)
    key = None
    if named:
        if scope != 0:
            raise FragmentError("a full key in a nonzero scope")
        length, at = read_leb32(msg, at, 16)
        if length == 0:
            raise FragmentError("an empty key")
        if max_key is not None and length > max_key:
            raise FragmentError("the key is longer than the configuration declares")
        if at + length > len(msg):
            raise FragmentError("a truncated key")
        key = msg[at : at + length]
        at += length
        text = key_expression(key)
        if subscribed is not None and text != subscribed:
            raise FragmentError("undeclared-key: the Put names a key the subscriber did not declare")
    else:
        if scope == 0:
            raise FragmentError("alias scope 0 is the global scope and names no alias")
        if alias is not None and scope != alias:
            raise FragmentError("an alias the session never declared")
    if at + 2 > len(msg) or msg[at] != zenoh_wire.ID_PUT | zenoh_wire.FLAG_Z or msg[at + 1] != zenoh_wire.EXT_ATTACHMENT:
        raise FragmentError("a Put without the attachment extension")
    at += 2
    att_len, at = read_leb32(msg, at)
    if att_len != zenoh_wire.ATTACHMENT_BYTES or at + att_len > len(msg):
        raise FragmentError("the attachment is not 33 bytes")
    att = msg[at : at + att_len]
    if att[16] != zenoh_wire.GID_BYTES:
        raise FragmentError("the attachment's GID length byte is not 16")
    at += att_len
    size, at = read_leb32(msg, at)
    if max_payload is not None and size > max_payload:
        raise FragmentError("the payload is longer than the configuration declares")
    if at + size != len(msg):
        raise FragmentError("the payload length disagrees with the message")
    return {"scope": scope, "key": key, "attachment": att, "payload": msg[at:]}


# Captured from the stock peer: (payload bytes, batch bytes, first sequence number, fragment
# lengths, fragment headers, sha256 of each fragment body). A fragment's length is the batch less
# the two-byte stream length, except the last.
CAPTURES = (
    (
        5000,
        2048,
        250917212,
        (2046, 2046, 963),
        (0x66, 0x66, 0x26),
        (
            "3a204a426aa81bfd43d1993883cd26ad288bb10cd21f0e9e8da310a7aff8acb4",
            "115c8ad2bfa2bb2ae78a932cc6d26b592f621e4256f980f7a38407a3cc556c79",
            "77922590635a1ec6191f82ad1272fc0995585c5547fc0210b604c4a2f1fbc8fb",
        ),
    ),
    (
        2200,
        2048,
        228976026,
        (2046, 204),
        (0x66, 0x26),
        (
            "9bbc893d83b3354f2e3c89b09d307545e3f7f538f200f7bf763d0bafefde6384",
            "2b7ea030692c9a4f13bb70adf972aab18669c10ad6959c07c190e4c979aac6bc",
        ),
    ),
    (
        5000,
        512,
        26343361,
        (510, 510, 510, 510, 510, 510, 510, 510, 510, 500),
        (0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x26),
        (
            "6a60b9186829556cfd5fc0fdfab10593142abeadd37a892e8212039d18925a41",
            "5abc503a4bbaf5b3aa2cb5a4f3ec9e4ef65ea68f8c7c614d125e25e6fa2a965f",
            "8326441f5a41c8ab8f300fe3364189a0fdebb25d0c8a825def86aaffbd0ed945",
            "2abdfba4fe45211b4bdd614acb1c3aca21a85b9dee14621872f7b60755817331",
            "aa020b56f6bf33e35844067d5bda2308b1cba283036315b7f47348edf1371c85",
            "c0f5bd6b2aaa0cb06c55b686dce03334860afacaa3bfee0f14dd78c417d34ac5",
            "04fcf50f3044e353d1d59849ebca133bb2faf854bdebdd88494cf4cecea000ec",
            "297bf4144dd72a828dcdadce16e8f0908a1f8b3840144308b11383d85d80172c",
            "3c6e6a38c4df129edf7b0ba9aeb2df0d9f25e9fedfdb812b629ea08909b93d83",
            "25617eb4aa9e787983cdc17edc7fd357d6d3bd1db4bdd80649c67c8e72696ac9",
        ),
    ),
)

def fits_one_frame(msg: bytes, batch: int, frame_sn: int) -> bool:
    """Whether a message travels whole: a FRAME is its header byte, its sequence number and the
    message, and the batch holds `batch - 2` bytes behind the stream length."""
    return 1 + len(zenoh_wire.leb128(frame_sn)) + len(msg) <= batch - STREAM_LENGTH_BYTES


# Observed, not derived: at a 2048-byte batch a Put of 2001 payload bytes was delivered with no
# FRAGMENT on the wire, and one of 2002 payload bytes became two fragments of 2046 and 6 bytes. The
# message is 2041 and 2042 bytes, and with a FRAME's header byte and four-byte sequence number
# (the sequence numbers in these runs were between 2**21 and 2**28) that is 2046 and 2047 against a
# limit of 2046. The 2001-byte run's own frame was not recorded, so its sequence number is taken to
# be in the same range as the others.
BOUNDARY = (
    (2001, 2048, 206290105, False, ()),
    (2002, 2048, 206290106, True, (2046, 6)),
)


def self_test() -> int:
    """Prove the reference reproduces every captured body and reassembles every capture.

    Returns the number of fragment bodies checked."""
    checked = 0
    for size, batch, first_sn, lengths, headers, digests in CAPTURES:
        msg = message(size)
        bodies = reference_split(msg, batch, first_sn)
        got = tuple((len(b), b[0], hashlib.sha256(b).hexdigest()) for b in bodies)
        want = tuple(zip(lengths, headers, digests, strict=True))
        if got != want:
            raise FragmentError(f"the reference does not reproduce the {size}-byte, {batch}-byte capture")
        if reassemble(bodies) != msg:
            raise FragmentError(f"the {size}-byte capture does not reassemble to its message")
        decoded = decode_message(reassemble(bodies))
        if decoded["payload"] != payload_bytes(size) or decoded["attachment"] != attachment():
            raise FragmentError(f"the {size}-byte capture does not decode to the sent payload")
        checked += len(bodies)
    for size, batch, first_sn, fragmented, lengths in BOUNDARY:
        msg = message(size)
        whole = fits_one_frame(msg, batch, first_sn)
        if whole == fragmented:
            raise FragmentError(f"the {size}-byte boundary case is on the wrong side of the rule")
        if fragmented:
            if tuple(len(b) for b in reference_split(msg, batch, first_sn)) != lengths:
                raise FragmentError(f"the {size}-byte boundary case splits differently")
    return checked
