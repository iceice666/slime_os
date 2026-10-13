"""Judge the Zenoh Profile 0 link-limits and fragmentation exam from evidence the Rust tests print.

The Rust tests under the module path `zenoh_links::` print one `[zenoh-exam] links ...` line for
each thing they did. This module owns what those lines must be, and nothing here reads the Rust:

* the three configurations a link is declared with, pinned field by field and validated through
  `contracts/zenoh-link-limits/v1` before they are accepted;
* the fragments a stock eclipse-zenoh 1.0.0 peer put on the wire for one large Put, pinned by
  length, header and sha256 in `zenoh_fragment.CAPTURES`, and the payload each reassembles to;
* the fragments of our own Put, which uses the full key form where the stock peer used its alias,
  so they cannot be the captured bytes and are computed here by the host reference instead;
* the edge between a message that travels whole and one that is fragmented;
* every refusal, by name and class.

Like the other judges it is trusted only after it refuses a mutation of an honest transcript.
"""

from __future__ import annotations

import hashlib
import shutil
import tempfile
from collections.abc import Callable
from functools import lru_cache
from pathlib import Path

import zenoh_exchange
import zenoh_fragment
import zenoh_wire
import zutai_cli
from harness import ROOT

LIMITS_SCHEMA = ROOT / "contracts" / "zenoh-link-limits" / "v1" / "schema.zt"

# name, mode, batch, payload, key, attachment, keys, subscribers, samples, stack, timeout, heap
CONFIG_FIELDS = (
    "name", "mode", "batchBytes", "payloadBytes", "keyBytes", "attachmentBytes", "maxKeys",
    "maxSubscribers", "outstandingSamples", "stackBytes", "reassemblyTimeoutMs", "heapBytes",
)
CONFIGS = (
    ("demo", "fixed", 512, 12, 129, 33, 1, 1, 4, 16384, 0, 0),
    ("large", "fixed", 2048, 1024, 129, 33, 4, 4, 4, 32768, 0, 0),
    ("heap", "heap", 2048, 16384, 129, 33, 4, 4, 4, 32768, 5000, 149202),
)

# The edge scenarios: our own first frame on the link carries this sequence number, and the Put
# in it is the demo key with the 33-byte attachment.
FULL_KEY_SN = 1000
FULL_KEY_PAYLOAD = 5000
FULL_KEY_BATCH = 2048
EDGE_BATCH = 2048

OK_SCENARIOS = (
    "encode-stock-5000-2048",
    "encode-stock-2200-2048",
    "encode-stock-5000-512",
    "reassemble-stock-5000-2048",
    "reassemble-stock-2200-2048",
    "reassemble-stock-5000-512",
    "boundary-whole-then-fragmented",
    "two-messages-in-a-row",
    "reassembly-after-timeout-reuses-storage",
    "reassembly-after-refusals-reuses-storage",
    "put-roundtrip-across-sequence-width",
    "put-roundtrip-at-the-declared-payload",
    "heap-reassembles-after-queue-drained",
    "reassembly-completes-at-the-timeout-edge",
    "heap-declared-payload-boundary",
    "heap-fills-notice-queue",
    "stack-covers-measured-sizes",
    "heap-storage-from-component",
    "put-roundtrip-5000-2048",
    "reassemble-sender-mapped-alias",
    "fragment-run-advances-session-sequence",
    "refused-fragment-run-leaves-session-unchanged",
    "demo-config-unchanged",
    "large-delivers-1024-whole",
    "four-keys-and-subscribers",
)
REFUSED_SCENARIOS = (
    ("fixed-refuses-fragment", "unsupported-message"),
    # A fixed link refuses every fragment as unsupported-message whatever its form, so the malformed
    # forms are exercised in fixed mode too and cannot be satisfied by a heap-mode refusal.
    ("fixed-refuses-best-effort-fragment", "unsupported-message"),
    ("fixed-refuses-fragment-with-extension", "unsupported-message"),
    ("fixed-refuses-empty-fragment", "unsupported-message"),
    # The sequence number is read before the mode is consulted nowhere: a fixed link classifies a
    # fragment with a malformed sequence number as unsupported-message too, not as the LEB class.
    ("fixed-refuses-fragment-sequence-noncanonical", "unsupported-message"),
    ("fixed-refuses-fragment-sequence-over-32-bits", "unsupported-message"),
    ("fragment-best-effort", "unsupported-message"),
    ("fragment-extension", "unsupported-extension"),
    ("empty-fragment", "malformed-message"),
    ("fragment-before-open", "unexpected-message"),
    ("fragment-sequence-gap", "fragment-sequence"),
    # The more flag decides where a message ends, not the payload length: an interior fragment that
    # says no more follows ends the message early, so what was joined is cut off inside its payload;
    # a last fragment that still says more follows leaves the run open, and with nothing further it
    # is refused once the declared timeout has passed, without a sample having been delivered.
    ("fragment-interior-marked-final", "truncated"),
    ("fragment-final-marked-more", "reassembly-timeout"),
    # A run is bound to the session: its first fragment must be the next expected sequence number,
    # as a FRAME's must, and the session's expectation advances only when the whole run is accepted.
    ("fragment-stale-first-sequence", "sequence-mismatch"),
    ("fragment-skipped-first-sequence", "sequence-mismatch"),
    ("frame-during-reassembly", "unexpected-message"),
    # A fragment carries a sequence number and no message id, so a second message cannot be told from a
    # continuation on the wire. What can be refused is what interleaving leaves behind: two messages
    # joined in one run, which reassemble to a message with data after it, and a fragment of the other
    # (best-effort) channel arriving mid-run.
    ("fragments-of-two-messages-in-one-run", "trailing-bytes"),
    ("best-effort-fragment-during-reassembly", "unsupported-message"),
    # The Put inside a fragmented message is held to what a single-batch Put is held to, in heap mode.
    ("fragmented-put-without-attachment", "malformed-message"),
    ("fragmented-put-with-short-attachment", "malformed-message"),
    ("fragmented-put-gid-length-15", "malformed-message"),
    ("fragmented-put-gid-length-17", "malformed-message"),
    ("fragmented-put-attachment-34-bytes", "over-bound"),
    ("fragmented-put-alias-scope-zero", "invalid-keyexpr"),
    ("fragmented-put-alias-scope-65536", "leb128-overflow"),
    ("message-over-declared-heap", "over-bound"),
    # The declared payload is enforced as a limit of its own, not only through the heap: with storage
    # large enough for it, a payload one byte over what the configuration declares is refused.
    ("payload-over-declared-with-enough-storage", "over-bound"),
    # The declared payload is a limit of its own in fixed mode too, on the configuration's side of the
    # batch: both fixed configurations refuse one byte over what they declare although it would fit.
    ("demo-refuses-payload-13-bytes", "over-bound"),
    ("large-refuses-payload-1025-bytes", "over-bound"),
    # The Put inside a fragmented message is held to the declared key: the demo key reassembles at 129
    # bytes (the full-key positive), and a 130-byte one is refused whatever the storage; and to the
    # session's aliases: only alias 1 is declared, so a canonical scope of 2 names nothing.
    ("fragmented-put-key-130-bytes", "over-bound"),
    # The key inside a reassembled Put is held to what a whole-frame key is held to: a valid key
    # expression, with the class the corpus gives each form, and the subscriber's own key.
    ("fragmented-put-key-wildcard", "wildcard-keyexpr"),
    ("fragmented-put-key-leading-slash", "invalid-keyexpr"),
    ("fragmented-put-key-double-slash", "invalid-keyexpr"),
    ("fragmented-put-key-invalid-utf8", "invalid-keyexpr"),
    ("fragmented-put-key-not-subscribed", "undeclared-key"),
    # An alias resolves to the key it was declared for, and that key is then held to the subscription;
    # and a peer that declared an alias addresses it sender-mapped, so the receiver mapping, which
    # names an alias this side declared, is refused where the stock captures use it only because a
    # stock publisher was addressing a stock listener's alias.
    ("fragmented-put-alias-for-another-key", "undeclared-key"),
    ("fragmented-put-receiver-mapped-alias", "invalid-keyexpr"),
    # The widths: a key length is 16 bits and an attachment length 32, and each is refused as an
    # overflow of its own width, not later as a bound or a malformed attachment.
    ("fragmented-put-key-length-over-16-bits", "leb128-overflow"),
    ("fragmented-put-attachment-length-over-32-bits", "leb128-overflow"),
    # Canonical values inside each field's declared width are read, so a reader that is too narrow
    # fails: an 8-bit reader would call a scope or key length of 256 an overflow, and a 16-bit one
    # would call an attachment or payload length of 65536 one. Each is refused for what it says.
    # A payload shorter than its length says is truncated, and the reassembly timeout has an edge.
    ("fragmented-put-scope-256", "invalid-keyexpr"),
    ("fragmented-put-key-length-256", "over-bound"),
    ("fragmented-put-attachment-length-65536", "over-bound"),
    ("fragmented-put-payload-length-65536", "over-bound"),
    ("fragmented-put-payload-truncated", "truncated"),
    # The top bit of each field's width: a value canonical in its field at bit 15 or bit 31 is refused
    # for what it says, so a reader one bit narrow fails; and a Put cut off inside its attachment is
    # truncated, as the whole-frame parser's `take` reports it.
    ("fragmented-put-scope-32768", "invalid-keyexpr"),
    ("fragmented-put-key-length-32768", "over-bound"),
    ("fragmented-put-attachment-length-2147483648", "over-bound"),
    ("fragmented-put-payload-length-2147483648", "over-bound"),
    ("fragmented-put-attachment-truncated", "truncated"),
    ("reassembly-one-ms-after-the-timeout", "reassembly-timeout"),
    # Every variable-length integer in a fragment run is read the way the Rust reader reads it: at most
    # the field's width, and canonically. The fragment's own sequence number and the four lengths and
    # scope inside the reassembled Put are each graded on the link, not only in the host oracle.
    ("fragment-sequence-noncanonical", "leb128-noncanonical"),
    ("fragment-sequence-over-32-bits", "leb128-overflow"),
    ("fragmented-put-scope-noncanonical", "leb128-noncanonical"),
    ("fragmented-put-key-length-noncanonical", "leb128-noncanonical"),
    ("fragmented-put-attachment-length-noncanonical", "leb128-noncanonical"),
    ("fragmented-put-payload-length-noncanonical", "leb128-noncanonical"),
    ("fragmented-put-payload-length-over-32-bits", "leb128-overflow"),
    ("fragmented-put-undeclared-alias", "invalid-keyexpr"),
    # The link announced its batch, so a fragment longer than the batch less the stream length is
    # refused, as a stream prefix longer than the batch is.
    ("fragment-over-the-announced-batch", "length-exceeds-batch"),
    ("reassembly-timeout", "reassembly-timeout"),
    ("storage-refused", "out-of-memory"),
    ("fifth-subscriber", "subscriber-limit"),
    ("fifth-alias", "over-bound"),
)
# 3 configurations, 13 scenarios, 15 stock fragments, 3 reassemblies, 3 fragments of our own Put and
# 2 edge lines make 39 positive cases; 12 refusals make 51. The control floor is the number the
# judge, the host reference and the limits contract refuse today.
# The session's next expected sequence number after a whole run, for our own 5000-byte Put at a
# 2048-byte batch: (label, the number before the run). Each line reports before, count and after.
ADVANCE = (
    ("fullkey-5000-2048", 1000),
    ("fullkey-wraps-32-bits", 0xFFFFFFFE),
)
# Storage of exactly one message of this payload: a timed-out reassembly that kept its buffer leased
# leaves no room for the next, so a successful reassembly into it proves the storage was released.
REUSE_SIZE = 5000
# The heap configuration's declared payload: a Put of exactly this size reassembles, and one byte
# more is refused whatever storage it is given.
HEAP_PAYLOAD = CONFIGS[2][3]
# The heap configuration's reassembly timeout, and a message that completes exactly at it.
TIMEOUT_MS = CONFIGS[2][10]
DEADLINE_PAYLOAD = 5000


def deadline_line(timeout: int, first: int, last: int, digest: str) -> str:
    return f"links deadline {timeout} {first} {last} {digest}"


def reuse_capacity() -> int:
    return REUSE_SIZE + 129 + 33 + FRAMING


# The heap configuration queues two notices for each outstanding sample and every one holds a whole
# message, so the owner that has drained none holds this many; a message in progress is one more.
def queue_held() -> int:
    return NOTICES * CONFIGS[2][8]


def distinct_payload(size: int, index: int) -> bytes:
    """A payload of `size` bytes that differs from every other `index`, so an overwrite is noticed."""
    return bytes((i * 7 + 1 + index * 13) % 251 for i in range(size))


def queue_digest() -> str:
    """The digest of the held payloads and the one in progress, each at the declared size."""
    return sha("\n".join(sha(distinct_payload(HEAP_PAYLOAD, i)) for i in range(queue_held() + 1)).encode())


def queue_line(held: int, in_progress: int, size: int, sent: str, back: str) -> str:
    return f"links queue {held} {in_progress} {size} {sent} {back}"


# Sequence numbers at which the width of the encoded number changes, one for each step from one byte to
# five and the step back from five bytes to one at the 32-bit wrap: a run that starts one short of the
# step has fragments whose headers differ in size, so a link that sizes every slice from its first
# fragment's header overruns the announced batch, and one that does not wrap emits 4294967296.
WIDTH_STARTS = (127, 16383, 2097151, 268435455, 0xFFFFFFFE)
# Each way a reassembly ends without delivering a sample, after which the one-message storage must be
# free again: three that end before the message is complete and three that refuse the completed message.
REUSE_CAUSES = (
    "sequence-gap",
    "whole-frame-during-reassembly",
    "best-effort-fragment-during-reassembly",
    "completed-malformed-put",
    "completed-undeclared-key",
    "completed-payload-over-declared",
)


def width_split(start: int) -> list[bytes]:
    return zenoh_fragment.reference_split(full_key_message(FULL_KEY_PAYLOAD), FULL_KEY_BATCH, start)


def heap_max_bodies() -> list[bytes]:
    """Our own Put of the heap configuration's declared payload, as the sender splits it."""
    return zenoh_fragment.reference_split(full_key_message(HEAP_PAYLOAD), FULL_KEY_BATCH, FULL_KEY_SN)


def drained_line(held: int, size: int, digest: str) -> str:
    return f"links drained {held} {size} {digest}"


def drained_digest() -> str:
    return sha(distinct_payload(HEAP_PAYLOAD, queue_held() + 1))


PINNED_CASES = 155
PINNED_CONTROLS = 332


class LinksError(ValueError):
    """Evidence the judge refuses."""


def capture_name(size: int, batch: int) -> str:
    return f"{size}-{batch}"


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


@lru_cache(maxsize=None)
def key() -> str:
    return zenoh_exchange.demo()["key"]


def full_key_message(size: int) -> bytes:
    """Our own Put: a PUSH naming the demo key in full, sender mapping, with the 33-byte attachment."""
    return zenoh_wire.push_put(key(), zenoh_fragment.attachment(), zenoh_fragment.payload_bytes(size), True)


def full_key_bodies() -> list[bytes]:
    return zenoh_fragment.reference_split(full_key_message(FULL_KEY_PAYLOAD), FULL_KEY_BATCH, FULL_KEY_SN)


@lru_cache(maxsize=None)
def whole_edge(batch: int, sn: int) -> int:
    """The largest payload whose full-key Put travels whole in one FRAME at this batch size."""
    limit = batch - zenoh_fragment.STREAM_LENGTH_BYTES
    size = 0
    while 1 + len(zenoh_wire.leb128(sn)) + len(full_key_message(size + 1)) <= limit:
        size += 1
    return size


def expected_fragments() -> dict[str, list[tuple[int, int, str]]]:
    """Per stock capture: (length, header, sha256) of each fragment body, from the pinned table."""
    out = {}
    for size, batch, _sn, lengths, headers, digests in zenoh_fragment.CAPTURES:
        out[capture_name(size, batch)] = list(zip(lengths, headers, digests, strict=True))
    return out


def stock_references() -> None:
    """Prove the host reference reproduces every captured body, so the pins are the stock peer's."""
    try:
        zenoh_fragment.self_test()
    except zenoh_fragment.FragmentError as error:
        raise LinksError(f"the host fragment reference disagrees with the captures: {error}") from error


def good_lines() -> list[str]:
    stock_references()
    lines = ["links config " + " ".join(str(v) for v in config) for config in CONFIGS]
    expected = expected_fragments()
    for name in OK_SCENARIOS:
        if name.startswith("encode-stock-"):
            capture = name[len("encode-stock-"):]
            lines += [
                f"links fragment {capture} {index} {length} {header:02x} {digest}"
                for index, (length, header, digest) in enumerate(expected[capture])
            ]
        if name.startswith("reassemble-stock-"):
            capture = name[len("reassemble-stock-"):]
            size = int(capture.split("-")[0])
            lines.append(f"links reassembled {capture} {size} {sha(zenoh_fragment.payload_bytes(size))}")
        if name == "boundary-whole-then-fragmented":
            edge = whole_edge(EDGE_BATCH, FULL_KEY_SN)
            lines.append(f"links boundary {edge} whole 1")
            lines.append(f"links boundary {edge + 1} fragmented 2")
        if name == "put-roundtrip-5000-2048":
            lines += [
                f"links fullkey {index} {len(body)} {body[0]:02x} {sha(body)}"
                for index, body in enumerate(full_key_bodies())
            ]
        if name == "reassembly-completes-at-the-timeout-edge":
            lines.append(deadline_line(TIMEOUT_MS, 0, TIMEOUT_MS, sha(zenoh_fragment.payload_bytes(DEADLINE_PAYLOAD))))
        if name == "put-roundtrip-at-the-declared-payload":
            lines += [
                f"links heapfrag {index} {len(body)} {body[0]:02x} {sha(body)}"
                for index, body in enumerate(heap_max_bodies())
            ]
        if name == "heap-reassembles-after-queue-drained":
            lines.append(drained_line(queue_held(), HEAP_PAYLOAD, drained_digest()))
        if name == "reassembly-after-refusals-reuses-storage":
            lines += [
                f"links reuseafter {cause} {reuse_capacity()} {REUSE_SIZE} {sha(zenoh_fragment.payload_bytes(REUSE_SIZE))}"
                for cause in REUSE_CAUSES
            ]
        if name == "put-roundtrip-across-sequence-width":
            lines += [
                f"links widthsplit {start} {index} {len(body)} {body[0]:02x} {sha(body)}"
                for start in WIDTH_STARTS
                for index, body in enumerate(width_split(start))
            ]
        if name == "reassembly-after-timeout-reuses-storage":
            lines.append(f"links reuse {reuse_capacity()} {REUSE_SIZE} {sha(zenoh_fragment.payload_bytes(REUSE_SIZE))}")
        if name == "reassemble-sender-mapped-alias":
            lines.append(f"links aliased 5d 5000 {sha(zenoh_fragment.payload_bytes(5000))}")
        if name == "heap-declared-payload-boundary":
            lines.append(f"links payload {HEAP_PAYLOAD} {heap_needed(pinned_configs()[2])} {sha(zenoh_fragment.payload_bytes(HEAP_PAYLOAD))}")
        if name == "heap-fills-notice-queue":
            lines.append(queue_line(queue_held(), 1, HEAP_PAYLOAD, queue_digest(), queue_digest()))
        if name == "stack-covers-measured-sizes":
            lines += [f"links size {c['name']} {required_stack(c)} {c['stackBytes']}" for c in pinned_configs()]
        if name == "fragment-run-advances-session-sequence":
            count = len(full_key_bodies())
            lines += [
                f"links advance {label} {before} {count} {zenoh_fragment.next_sn(before, count)}"
                for label, before in ADVANCE
            ]
        lines.append(f"links ok {name}")
    lines += [f"links refused {name} {cls}" for name, cls in REFUSED_SCENARIOS]
    return lines


SHAPES = {
    ("links", "config"): 14,
    ("links", "ok"): 3,
    ("links", "refused"): 4,
    ("links", "fragment"): 7,
    ("links", "reassembled"): 5,
    ("links", "fullkey"): 6,
    ("links", "boundary"): 5,
    ("links", "advance"): 6,
    ("links", "reuse"): 5,
    ("links", "payload"): 5,
    ("links", "size"): 5,
    ("links", "queue"): 7,
    ("links", "aliased"): 5,
    ("links", "deadline"): 6,
    ("links", "reuseafter"): 6,
    ("links", "heapfrag"): 6,
    ("links", "drained"): 5,
    ("links", "widthsplit"): 7,
}


def judge(lines: list[str]) -> tuple[int, int]:
    """Returns (positive cases, refusals); raises LinksError on any evidence it does not accept."""
    stock_references()
    split = [line.split() for line in lines]
    for line, fields in zip(lines, split, strict=True):
        if fields[:1] != ["links"]:
            raise LinksError(f"a record that is not a links exam line: {line!r}")
        kind = tuple(fields[:2])
        if kind not in SHAPES:
            raise LinksError(f"unrecognised line: {line}")
        if len(fields) != SHAPES[kind]:
            raise LinksError(f"a {kind[1]} line has {len(fields)} fields, expected {SHAPES[kind]}: {line}")

    def only(kind: str) -> list[list[str]]:
        return [f for f in split if f[1] == kind]

    configs = [tuple(f[2:]) for f in only("config")]
    wanted_configs = [tuple(str(v) for v in c) for c in CONFIGS]
    if configs != wanted_configs:
        raise LinksError(f"configurations reported {configs}, the exam pins {wanted_configs}")

    done = [f[2] for f in only("ok")]
    if done != list(OK_SCENARIOS):
        raise LinksError(f"scenarios reported {done}, the exam runs them as {list(OK_SCENARIOS)}")

    expected = expected_fragments()
    sent = [(f[2], int(f[3]), int(f[4]), f[5], f[6]) for f in only("fragment")]
    wanted_sent = [
        (capture, index, length, f"{header:02x}", digest)
        for capture, rows in expected.items()
        for index, (length, header, digest) in enumerate(rows)
    ]
    if sent != wanted_sent:
        raise LinksError("the fragments encoded for the stock captures differ from the captured bodies")

    reassembled = [(f[2], int(f[3]), f[4]) for f in only("reassembled")]
    wanted_reassembled = [
        (capture, int(capture.split("-")[0]), sha(zenoh_fragment.payload_bytes(int(capture.split("-")[0]))))
        for capture in expected
    ]
    if reassembled != wanted_reassembled:
        raise LinksError("a stock capture did not reassemble to the payload that was sent")

    fullkey = [(int(f[2]), int(f[3]), f[4], f[5]) for f in only("fullkey")]
    wanted_full = [(i, len(b), f"{b[0]:02x}", sha(b)) for i, b in enumerate(full_key_bodies())]
    if fullkey != wanted_full:
        raise LinksError("our own Put was fragmented differently from the host reference")

    edge = whole_edge(EDGE_BATCH, FULL_KEY_SN)
    boundary = [(int(f[2]), f[3], int(f[4])) for f in only("boundary")]
    if boundary != [(edge, "whole", 1), (edge + 1, "fragmented", 2)]:
        raise LinksError(f"the edge between whole and fragmented is at {edge} payload bytes, reported {boundary}")

    payload = [(int(f[2]), int(f[3]), f[4]) for f in only("payload")]
    wanted_payload = [(HEAP_PAYLOAD, heap_needed(pinned_configs()[2]), sha(zenoh_fragment.payload_bytes(HEAP_PAYLOAD)))]
    if payload != wanted_payload:
        raise LinksError(f"the declared payload of {HEAP_PAYLOAD} bytes must reassemble into the declared heap: {wanted_payload}, reported {payload}")

    queue = [(int(f[2]), int(f[3]), int(f[4]), f[5], f[6]) for f in only("queue")]
    wanted_queue = [(queue_held(), 1, HEAP_PAYLOAD, queue_digest(), queue_digest())]
    if queue != wanted_queue:
        raise LinksError(f"with {queue_held()} notices held and one message in progress, each of the declared {HEAP_PAYLOAD} bytes, every payload must read back as it was sent: {wanted_queue}, reported {queue}")

    aliased = [(f[2], int(f[3]), f[4]) for f in only("aliased")]
    wanted_aliased = [("5d", 5000, sha(zenoh_fragment.payload_bytes(5000)))]
    if aliased != wanted_aliased:
        raise LinksError(f"a sender-mapped aliased Put of 5000 bytes must reassemble: {wanted_aliased}, reported {aliased}")

    deadline = [(int(f[2]), int(f[3]), int(f[4]), f[5]) for f in only("deadline")]
    wanted_deadline = [(TIMEOUT_MS, 0, TIMEOUT_MS, sha(zenoh_fragment.payload_bytes(DEADLINE_PAYLOAD)))]
    if deadline != wanted_deadline:
        raise LinksError(f"a run whose last fragment arrives exactly the declared timeout after the first must reassemble: {wanted_deadline}, reported {deadline}")
    for _timeout, first, last, _digest in deadline:
        zenoh_fragment.check_deadline(first, last, TIMEOUT_MS)

    sizes = [(f[2], int(f[3]), int(f[4])) for f in only("size")]
    if [name for name, _, _ in sizes] != [c["name"] for c in pinned_configs()]:
        raise LinksError(f"stack sizes reported for {[n for n, _, _ in sizes]}, the exam pins {[c['name'] for c in pinned_configs()]}")
    for (name, measured, declared), config in zip(sizes, pinned_configs(), strict=True):
        if declared != config["stackBytes"]:
            raise LinksError(f"{name}: the stack declared is {declared}, the exam pins {config['stackBytes']}")
        if not 0 < measured <= declared:
            raise LinksError(f"{name}: the link's arrays, slots, notices and session measure {measured} bytes with size_of, past the {declared} declared")
        if measured > required_stack(config):
            raise LinksError(f"{name}: the link measures {measured} bytes with size_of, past the {required_stack(config)} the contract's formula certifies for it")

    reuse = [(int(f[2]), int(f[3]), f[4]) for f in only("reuse")]
    wanted_reuse = [(reuse_capacity(), REUSE_SIZE, sha(zenoh_fragment.payload_bytes(REUSE_SIZE)))]
    if reuse != wanted_reuse:
        raise LinksError(f"reassembly into storage of one message after a timeout is {wanted_reuse}, reported {reuse}")

    count = len(full_key_bodies())
    advance = [(f[2], int(f[3]), int(f[4]), int(f[5])) for f in only("advance")]
    wanted_advance = [(label, before, count, zenoh_fragment.next_sn(before, count)) for label, before in ADVANCE]
    if advance != wanted_advance:
        raise LinksError(f"the session sequence after a whole fragment run is {wanted_advance}, reported {advance}")

    heapfrag = [(int(f[2]), int(f[3]), f[4], f[5]) for f in only("heapfrag")]
    wanted_heapfrag = [(i, len(b), f"{b[0]:02x}", sha(b)) for i, b in enumerate(heap_max_bodies())]
    if heapfrag != wanted_heapfrag:
        raise LinksError(f"our own Put of the declared {HEAP_PAYLOAD} bytes was fragmented differently from the host reference")

    drained = [(int(f[2]), int(f[3]), f[4]) for f in only("drained")]
    wanted_drained = [(queue_held(), HEAP_PAYLOAD, drained_digest())]
    if drained != wanted_drained:
        raise LinksError(f"after {queue_held()} held samples are consumed, another message of the declared {HEAP_PAYLOAD} bytes must reassemble into the same storage: {wanted_drained}, reported {drained}")

    reuseafter = [(f[2], int(f[3]), int(f[4]), f[5]) for f in only("reuseafter")]
    wanted_reuseafter = [(cause, reuse_capacity(), REUSE_SIZE, sha(zenoh_fragment.payload_bytes(REUSE_SIZE))) for cause in REUSE_CAUSES]
    if reuseafter != wanted_reuseafter:
        raise LinksError(f"after each early end of a reassembly, {list(REUSE_CAUSES)}, a message must reassemble into storage of one message: {wanted_reuseafter}, reported {reuseafter}")

    widthsplit = [(int(f[2]), int(f[3]), int(f[4]), f[5], f[6]) for f in only("widthsplit")]
    wanted_width = [
        (start, index, len(body), f"{body[0]:02x}", sha(body))
        for start in WIDTH_STARTS
        for index, body in enumerate(width_split(start))
    ]
    for start in WIDTH_STARTS:
        widths = {len(zenoh_wire.leb128((start + index) & 0xFFFFFFFF)) for index in range(len(width_split(start)))}
        if len(widths) < 2:
            raise LinksError(f"the run from {start} does not cross a sequence-number width change")
    if widthsplit != wanted_width:
        raise LinksError("a Put split across a sequence-number width change differs from the host reference")

    refused = [(f[2], f[3]) for f in only("refused")]
    if [name for name, _ in refused] != [name for name, _ in REFUSED_SCENARIOS]:
        raise LinksError(f"refusals reported {[n for n, _ in refused]}, the exam names {[n for n, _ in REFUSED_SCENARIOS]}")
    wrong = [f"{n}={c} (expected {dict(REFUSED_SCENARIOS)[n]})" for n, c in refused if c != dict(REFUSED_SCENARIOS)[n]]
    if wrong:
        raise LinksError(f"refusal classes differ: {', '.join(wrong)}")
    positive = len(configs) + len(done) + len(sent) + len(reassembled) + len(fullkey) + len(boundary) + len(advance) + len(reuse) + len(payload) + len(sizes) + len(queue) + len(aliased) + len(deadline) + len(reuseafter) + len(widthsplit) + len(heapfrag) + len(drained)
    return positive, len(refused)


# ----------------------------------------------------------------------------- the contract


def limits_record(configs: list[dict]) -> str:
    rows = "".join(
        "    { "
        + " ".join(f'{k} = "{v}";' if isinstance(v, str) else f"{k} = {v};" for k, v in c.items())
        + " };\n"
        for c in configs
    )
    return f"{{\n  formatVersion = 1;\n  configs = [\n{rows}  ];\n}}\n"


def pinned_configs() -> list[dict]:
    return [dict(zip(CONFIG_FIELDS, c, strict=True)) for c in CONFIGS]


def contract_accepts(text: str) -> bool:
    """Whether `contracts/zenoh-link-limits/v1` decodes `text` as a valid limits record."""
    (ROOT / "build").mkdir(exist_ok=True)
    scratch = Path(tempfile.mkdtemp(prefix="zenoh-limits-", dir=ROOT / "build"))
    try:
        shutil.copyfile(LIMITS_SCHEMA, scratch / "schema.zt")
        (scratch / "decode.zt").write_text(
            's ::= import "schema.zt"; env ::= import stdlib.env; load ::= import stdlib.load; '
            "main :: { env : Env; load : Load; } -> Validation DecodeIssue s.Limits ! { * env.GetEffects; * load.ZtiEffects; } "
            '= caps => [ path := env.get caps.env "SLIME_LIMITS_PATH" ?? ""; s.decodeLimits (load.zti caps.load path) ]; main\n'
        )
        record = scratch / "limits.zti"
        record.write_text(text)
        out = zutai_cli.evaluate("run", scratch / "decode.zt", input_path=record, env_var="SLIME_LIMITS_PATH")
        return out.startswith("#valid")
    except zutai_cli.ZutaiError as error:
        raise LinksError(f"the limits contract could not decode a record: {error}") from error
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


# The arithmetic the contract also holds; kept here so a control can sit exactly on a boundary.
FRAMING, FRAGMENT_FRAMING, STACK_FACTOR, SLOT, NOTICE, NOTICES, SESSION = 32, 8, 8, 16, 40, 2, 640
STREAM = 2


def required_stack(c: dict) -> int:
    slots = (c["maxKeys"] + c["maxSubscribers"]) * (c["keyBytes"] + SLOT)
    payload = c["payloadBytes"] if c["mode"] == "fixed" else 0
    return STACK_FACTOR * c["batchBytes"] + SESSION + slots + NOTICES * c["outstandingSamples"] * (NOTICE + payload)


def message_bytes(c: dict) -> int:
    return c["payloadBytes"] + c["keyBytes"] + c["attachmentBytes"] + FRAMING


def heap_needed(c: dict) -> int:
    """A heap link hands each sample out as a slice of the buffer it was reassembled into, so every
    queued notice holds a whole message until it is consumed (two notices per outstanding sample), and one
    more is being reassembled."""
    return (NOTICES * c["outstandingSamples"] + 1) * message_bytes(c)


def limits_cases() -> list[tuple[str, list[dict], bool]]:
    """(label, configs, whether the contract must accept), each sitting on a boundary."""
    demo, large, heap = pinned_configs()

    def exact(base: dict, **change: object) -> dict:
        c = dict(base, **change)
        c["stackBytes"] = required_stack(c)
        return c

    def trio(**replace: dict) -> list[dict]:
        return [replace.get("demo", demo), replace.get("large", large), replace.get("heap", heap)]

    return [
        ("the pinned configurations", trio(), True),
        ("no configuration", [], False),
        ("a repeated name", [demo, dict(large, name="demo")], False),
        ("an unknown mode", trio(large=dict(large, mode="dynamic")), False),
        ("an empty name", trio(large=dict(large, name="")), False),
        ("a batch of 4096, one service operation exactly", trio(large=exact(large, batchBytes=4096, payloadBytes=2048)), True),
        ("a batch of 4097, one byte over an operation", trio(large=exact(large, batchBytes=4097, payloadBytes=2048)), False),
        ("a zero batch", trio(large=dict(large, batchBytes=0)), False),
        ("65535 keys", trio(large=exact(large, maxKeys=65535, maxSubscribers=1)), True),
        ("65536 keys", trio(large=exact(large, maxKeys=65536, maxSubscribers=1)), False),
        ("more subscribers than keys", trio(large=dict(large, maxKeys=2, maxSubscribers=3)), False),
        ("zero outstanding samples", trio(large=dict(large, outstandingSamples=0)), False),
        ("stack exactly the requirement", trio(large=exact(large)), True),
        ("stack one byte under the requirement", trio(large=dict(exact(large), stackBytes=required_stack(large) - 1)), False),
        ("more keys than the exact stack covers", trio(large=dict(exact(large), maxKeys=8, maxSubscribers=4)), False),
        ("a longer key than the exact stack covers", trio(large=dict(exact(large), keyBytes=200)), False),
        ("demo with one byte too little stack", trio(demo=dict(demo, stackBytes=required_stack(demo) - 1)), False),
        ("a fixed payload that fits its batch less the stream length exactly", trio(large=exact(large, payloadBytes=2048 - STREAM - 129 - 33 - FRAMING)), True),
        ("a fixed payload one byte over its batch less the stream length", trio(large=exact(large, payloadBytes=2048 - STREAM - 129 - 33 - FRAMING + 1)), False),
        ("a fixed payload that fills the whole batch, the stream length not reserved", trio(large=exact(large, payloadBytes=2048 - 129 - 33 - FRAMING)), False),
        ("a fixed link with a reassembly timeout", trio(large=dict(large, reassemblyTimeoutMs=1000)), False),
        ("a fixed link with a heap", trio(large=dict(large, heapBytes=1)), False),
        ("a heap link with a timeout of 0", trio(heap=dict(heap, reassemblyTimeoutMs=0)), False),
        ("a heap link with a negative timeout", trio(heap=dict(heap, reassemblyTimeoutMs=-1)), False),
        ("a heap exactly every outstanding sample and the message being reassembled", trio(heap=dict(heap, heapBytes=heap_needed(heap))), True),
        ("a heap one byte under that", trio(heap=dict(heap, heapBytes=heap_needed(heap) - 1)), False),
        ("a heap of exactly one message with four outstanding samples", trio(heap=dict(heap, heapBytes=message_bytes(heap))), False),
        ("one outstanding sample needs three messages of heap (two queued notices and one in progress)", trio(heap=dict(heap, outstandingSamples=1, heapBytes=3 * message_bytes(heap))), True),
        ("one outstanding sample with two messages of heap", trio(heap=dict(heap, outstandingSamples=1, heapBytes=2 * message_bytes(heap))), False),
        ("one outstanding sample with one message of heap", trio(heap=dict(heap, outstandingSamples=1, heapBytes=message_bytes(heap))), False),
        ("no heap declared", trio(heap=dict(heap, heapBytes=0)), False),
        ("a heap link with one byte too little stack", trio(heap=dict(heap, stackBytes=required_stack(heap) - 1)), False),
        ("a 1 MiB heap payload with its heap", trio(heap=exact(heap, payloadBytes=1 << 20, heapBytes=heap_needed(dict(heap, payloadBytes=1 << 20)))), True),
        ("a heap payload larger than its heap", trio(heap=exact(heap, payloadBytes=1 << 20)), False),
        ("a heap batch that cannot hold a fragment's framing", trio(heap=exact(heap, batchBytes=8)), False),
        ("a heap batch of 9", trio(heap=exact(heap, batchBytes=9)), True),
        ("an attachment of 33 bytes, the wire size", trio(large=exact(large, attachmentBytes=33)), True),
        ("an attachment of 32 bytes, one byte short", trio(large=exact(large, attachmentBytes=32)), False),
        ("an attachment of 34 bytes, one byte over", trio(large=exact(large, attachmentBytes=34)), False),
        ("an attachment of 1 byte", trio(large=exact(large, attachmentBytes=1)), False),
        ("a heap link certifying storage for a 1-byte attachment", trio(heap=exact(heap, attachmentBytes=1, heapBytes=heap_needed(dict(heap, attachmentBytes=1)))), False),
        ("a 65535-byte key with the stack and heap it needs", trio(heap=exact(heap, keyBytes=65535, heapBytes=heap_needed(dict(heap, keyBytes=65535)))), True),
        ("a 65536-byte key with the stack and heap it needs", trio(heap=exact(heap, keyBytes=65536, heapBytes=heap_needed(dict(heap, keyBytes=65536)))), False),
        ("a payload of 4294967295 bytes with the stack and heap it needs", trio(heap=exact(heap, payloadBytes=4294967295, heapBytes=heap_needed(dict(heap, payloadBytes=4294967295)))), True),
        ("a payload of 4294967296 bytes with the stack and heap it needs", trio(heap=exact(heap, payloadBytes=4294967296, heapBytes=heap_needed(dict(heap, payloadBytes=4294967296)))), False),
        ("no keys", trio(large=dict(large, maxKeys=0)), False),
        ("one key and one subscriber", trio(large=exact(large, maxKeys=1, maxSubscribers=1)), True),
        ("no subscribers", trio(large=dict(large, maxSubscribers=0)), False),
        ("a negative key count", trio(large=dict(large, maxKeys=-1)), False),
        ("a zero-byte payload", trio(large=dict(large, payloadBytes=0)), False),
        ("a one-byte payload", trio(large=exact(large, payloadBytes=1)), True),
        ("a zero-byte key", trio(large=dict(large, keyBytes=0)), False),
        ("a one-byte key", trio(large=exact(large, keyBytes=1)), True),
        ("one outstanding sample", trio(large=exact(large, outstandingSamples=1)), True),
        ("a name of 16 bytes", trio(large=dict(large, name="n" * 16)), True),
        ("a name of 17 bytes", trio(large=dict(large, name="n" * 17)), False),
        ("a field the contract does not name", [dict(demo, note="x")] + [large, heap], False),
        ("a removed field", [{k: v for k, v in demo.items() if k != "heapBytes"}, large, heap], False),
    ]


def limits_controls() -> int:
    """The contract accepts what it must and refuses what it must. Returns the refused count."""
    refused = 0
    for label, configs, accept in limits_cases():
        if contract_accepts(limits_record(configs)) != accept:
            raise LinksError(f"control: the limits contract {'refused' if accept else 'accepted'}: {label}")
        refused += 0 if accept else 1
    return refused


# --------------------------------------------------------------------------------- controls


def corrupt_last(line: str) -> str:
    head, last = line.rsplit(" ", 1)
    return f"{head} {last[:-1]}{'0' if last[-1] != '0' else '1'}"


def transcript_mutations(good: list[str]) -> list[tuple[str, list[str]]]:
    def drop(prefix: str) -> list[str]:
        return [line for line in good if not line.startswith(prefix)]

    def first(prefix: str) -> int:
        return next(i for i, line in enumerate(good) if line.startswith(prefix))

    def swap(prefix: str, new: str) -> list[str]:
        at = first(prefix)
        return good[:at] + [new] + good[at + 1 :]

    def swapped(a: int, b: int) -> list[str]:
        out = list(good)
        out[a], out[b] = out[b], out[a]
        return out

    fragment_at = first("links fragment ")
    reassembled_at = first("links reassembled ")
    fullkey_at = first("links fullkey ")
    boundary_at = first("links boundary ")
    config_at = first("links config large")
    ok_lines = [i for i, line in enumerate(good) if line.startswith("links ok ")]
    return [
        ("an empty transcript", []),
        ("a dropped first line", good[1:]),
        ("a dropped last line", good[:-1]),
        ("a duplicated line", good + [good[0]]),
        ("a record that is not a links line", good + ["stock ok invented"]),
        ("an unknown links line", good + ["links invented x"]),
        ("a config line with a missing field", swap("links config demo", "links config demo fixed 512")),
        ("a dropped scenario", [line for line in good if line != "links ok put-roundtrip-5000-2048"]),
        ("a duplicated scenario", good + ["links ok put-roundtrip-5000-2048"]),
        ("an invented scenario", good + ["links ok invented-scenario"]),
        ("scenarios in another order", swapped(ok_lines[0], ok_lines[1])),
        ("a dropped fragment", good[:fragment_at] + good[fragment_at + 1 :]),
        ("a fragment with a corrupted digest", swap("links fragment 5000-2048 0 ", corrupt_last(good[fragment_at]))),
        ("a fragment of the wrong length", swap("links fragment 5000-2048 1 ", good[fragment_at + 1].replace(" 2046 ", " 2045 ", 1))),
        ("a fragment with the wrong header", swap("links fragment 2200-2048 1 ", good[first("links fragment 2200-2048 1 ")].replace(" 26 ", " 66 ", 1))),
        ("fragments of one capture reordered", swapped(fragment_at, fragment_at + 1)),
        ("a fragment of a capture that was never taken", good + ["links fragment 9999-2048 0 10 66 " + "0" * 64]),
        ("a capture that reassembled to another payload", swap("links reassembled 5000-2048 ", corrupt_last(good[reassembled_at]))),
        ("a capture that reassembled to a shorter payload", swap("links reassembled 5000-512 ", good[first("links reassembled 5000-512 ")].replace(" 5000 ", " 4999 ", 1))),
        ("a dropped reassembly", good[:reassembled_at] + good[reassembled_at + 1 :]),
        ("our own Put fragmented differently from the host reference", swap("links fullkey 1 ", corrupt_last(good[fullkey_at + 1]))),
        ("our own Put missing a fragment", good[:fullkey_at] + good[fullkey_at + 1 :]),
        ("the edge reported one byte early", swap("links boundary", good[boundary_at].replace(f" {good[boundary_at].split()[2]} ", f" {int(good[boundary_at].split()[2]) - 1} ", 1))),
        ("a message past the edge reported as whole", swap("links boundary " + good[boundary_at + 1].split()[2] + " ", good[boundary_at + 1].replace(" fragmented 2", " whole 1", 1))),
        ("the edge scenario without its lines", drop("links boundary ")),
        ("a changed demo configuration", swap("links config demo", good[0].replace(" 512 12 ", " 512 13 ", 1))),
        ("a changed demo stack", swap("links config demo", good[0].replace(" 16384 ", " 16385 ", 1))),
        ("a changed large configuration", swap("links config large", good[config_at].replace(" 2048 1024 ", " 2048 1025 ", 1))),
        ("a missing heap configuration", drop("links config heap")),
        ("the configurations in another order", swapped(0, 1)),
        ("a dropped notice-queue line", drop("links queue ")),
        ("a notice queue of two", swap("links queue ", queue_line(2, 1, HEAP_PAYLOAD, queue_digest(), queue_digest()))),
        ("a notice queue one short of capacity", swap("links queue ", queue_line(queue_held() - 1, 1, HEAP_PAYLOAD, queue_digest(), queue_digest()))),
        ("a notice queue one past capacity", swap("links queue ", queue_line(queue_held() + 1, 1, HEAP_PAYLOAD, queue_digest(), queue_digest()))),
        ("a queue with no message in progress", swap("links queue ", queue_line(queue_held(), 0, HEAP_PAYLOAD, queue_digest(), queue_digest()))),
        ("a queue of small messages", swap("links queue ", queue_line(queue_held(), 1, 3000, queue_digest(), queue_digest()))),
        ("a queue of messages one byte under the declared size", swap("links queue ", queue_line(queue_held(), 1, HEAP_PAYLOAD - 1, queue_digest(), queue_digest()))),
        ("an earlier payload overwritten by a later reassembly", swap("links queue ", queue_line(queue_held(), 1, HEAP_PAYLOAD, queue_digest(), sha(b"overwritten")))),
        ("a queue whose payloads were not the ones sent", swap("links queue ", queue_line(queue_held(), 1, HEAP_PAYLOAD, sha(b"other"), sha(b"other")))),
        ("fragmented-put-scope-256 admitted instead of refused", drop("links refused fragmented-put-scope-256 ")),
        ("fragmented-put-scope-256 refused with the wrong class", swap("links refused fragmented-put-scope-256 ", "links refused fragmented-put-scope-256 over-bound")),
        ("fragmented-put-key-length-256 admitted instead of refused", drop("links refused fragmented-put-key-length-256 ")),
        ("fragmented-put-key-length-256 refused with the wrong class", swap("links refused fragmented-put-key-length-256 ", "links refused fragmented-put-key-length-256 unsupported-message")),
        ("fragmented-put-attachment-length-65536 admitted instead of refused", drop("links refused fragmented-put-attachment-length-65536 ")),
        ("fragmented-put-attachment-length-65536 refused with the wrong class", swap("links refused fragmented-put-attachment-length-65536 ", "links refused fragmented-put-attachment-length-65536 unsupported-message")),
        ("fragmented-put-payload-length-65536 admitted instead of refused", drop("links refused fragmented-put-payload-length-65536 ")),
        ("fragmented-put-payload-length-65536 refused with the wrong class", swap("links refused fragmented-put-payload-length-65536 ", "links refused fragmented-put-payload-length-65536 unsupported-message")),
        ("fragmented-put-payload-truncated admitted instead of refused", drop("links refused fragmented-put-payload-truncated ")),
        ("fragmented-put-payload-truncated refused with the wrong class", swap("links refused fragmented-put-payload-truncated ", "links refused fragmented-put-payload-truncated over-bound")),
        ("reassembly-one-ms-after-the-timeout admitted instead of refused", drop("links refused reassembly-one-ms-after-the-timeout ")),
        ("reassembly-one-ms-after-the-timeout refused with the wrong class", swap("links refused reassembly-one-ms-after-the-timeout ", "links refused reassembly-one-ms-after-the-timeout over-bound")),
        ("a dropped deadline line", drop("links deadline ")),
        ("a deadline run with a timeout of one millisecond", swap("links deadline ", deadline_line(1, 0, 1, sha(zenoh_fragment.payload_bytes(DEADLINE_PAYLOAD))))),
        ("a deadline run that completed one millisecond past the timeout", swap("links deadline ", deadline_line(TIMEOUT_MS, 0, TIMEOUT_MS + 1, sha(zenoh_fragment.payload_bytes(DEADLINE_PAYLOAD))))),
        ("a deadline run that completed a millisecond before the edge", swap("links deadline ", deadline_line(TIMEOUT_MS, 0, TIMEOUT_MS - 1, sha(zenoh_fragment.payload_bytes(DEADLINE_PAYLOAD))))),
        ("a deadline run that reassembled to another payload", swap("links deadline ", corrupt_last(good[first("links deadline ")]))),
        ("a deadline run of another timeout", swap("links deadline ", deadline_line(TIMEOUT_MS + 1, 0, TIMEOUT_MS + 1, sha(zenoh_fragment.payload_bytes(DEADLINE_PAYLOAD))))),
        ("the timeout-edge scenario dropped", [line for line in good if line != "links ok reassembly-completes-at-the-timeout-edge"]),
        ("fragmented-put-scope-32768 admitted instead of refused", drop("links refused fragmented-put-scope-32768 ")),
        ("fragmented-put-scope-32768 refused with the wrong class", swap("links refused fragmented-put-scope-32768 ", "links refused fragmented-put-scope-32768 over-bound")),
        ("fragmented-put-key-length-32768 admitted instead of refused", drop("links refused fragmented-put-key-length-32768 ")),
        ("fragmented-put-key-length-32768 refused with the wrong class", swap("links refused fragmented-put-key-length-32768 ", "links refused fragmented-put-key-length-32768 unsupported-message")),
        ("fragmented-put-attachment-length-2147483648 admitted instead of refused", drop("links refused fragmented-put-attachment-length-2147483648 ")),
        ("fragmented-put-attachment-length-2147483648 refused with the wrong class", swap("links refused fragmented-put-attachment-length-2147483648 ", "links refused fragmented-put-attachment-length-2147483648 unsupported-message")),
        ("fragmented-put-payload-length-2147483648 admitted instead of refused", drop("links refused fragmented-put-payload-length-2147483648 ")),
        ("fragmented-put-payload-length-2147483648 refused with the wrong class", swap("links refused fragmented-put-payload-length-2147483648 ", "links refused fragmented-put-payload-length-2147483648 unsupported-message")),
        ("fragmented-put-attachment-truncated admitted instead of refused", drop("links refused fragmented-put-attachment-truncated ")),
        ("fragmented-put-attachment-truncated refused with the wrong class", swap("links refused fragmented-put-attachment-truncated ", "links refused fragmented-put-attachment-truncated over-bound")),
        ("the early-end reuse lines dropped", drop("links reuseafter ")),
        ("the reuse after a malformed completed Put dropped", drop("links reuseafter completed-malformed-put ")),
        ("the reuse after a completed Put for an undeclared key dropped", drop("links reuseafter completed-undeclared-key ")),
        ("the reuse after a completed Put over the declared payload dropped", drop("links reuseafter completed-payload-over-declared ")),
        ("a reuse after a malformed completed Put into storage one byte short", swap("links reuseafter completed-malformed-put ", good[first("links reuseafter completed-malformed-put ")].replace(f" {reuse_capacity()} ", f" {reuse_capacity() - 1} ", 1))),
        ("the drained-queue line dropped", drop("links drained ")),
        ("a drained queue of seven", swap("links drained ", drained_line(queue_held() - 1, HEAP_PAYLOAD, drained_digest()))),
        ("a drained queue that reassembled a small message", swap("links drained ", drained_line(queue_held(), 3000, drained_digest()))),
        ("a drained queue that reassembled another payload", swap("links drained ", drained_line(queue_held(), HEAP_PAYLOAD, sha(b"other")))),
        ("the drained-queue scenario dropped", [line for line in good if line != "links ok heap-reassembles-after-queue-drained"]),
        ("the declared-payload send lines dropped", drop("links heapfrag ")),
        ("one declared-payload send fragment dropped", drop("links heapfrag 4 ")),
        ("a declared-payload send fragment with another body", swap("links heapfrag 3 ", corrupt_last(good[first("links heapfrag 3 ")]))),
        ("a declared-payload send fragment one byte over the batch", swap("links heapfrag 0 ", good[first("links heapfrag 0 ")].replace(f" {len(heap_max_bodies()[0])} ", f" {len(heap_max_bodies()[0]) + 1} ", 1))),
        ("a send capped at 5000 bytes", [line for line in good if not line.startswith("links heapfrag ")] + [f"links heapfrag {i} {len(b)} {b[0]:02x} {sha(b)}" for i, b in enumerate(zenoh_fragment.reference_split(full_key_message(5000), FULL_KEY_BATCH, FULL_KEY_SN))]),
        ("the wrapping run dropped", drop("links widthsplit 4294967294 ")),
        ("the wrapping run's last body sized from the five-byte header", swap("links widthsplit 4294967294 2 ", corrupt_last(good[first("links widthsplit 4294967294 2 ")]))),
        ("the wrapping run reported as an unwrapped split", swap("links widthsplit 4294967294 2 ", good[first("links widthsplit 4294967294 2 ")].replace(f" {len(width_split(0xFFFFFFFE)[2])} ", f" {len(width_split(0xFFFFFFFE)[2]) - 4} ", 1))),
        ("the reuse after a sequence gap dropped", drop("links reuseafter sequence-gap ")),
        ("the reuse after a whole frame mid-run dropped", drop("links reuseafter whole-frame-during-reassembly ")),
        ("the reuse after a best-effort fragment mid-run dropped", drop("links reuseafter best-effort-fragment-during-reassembly ")),
        ("a reuse after a sequence gap into storage one byte short", swap("links reuseafter sequence-gap ", good[first("links reuseafter sequence-gap ")].replace(f" {reuse_capacity()} ", f" {reuse_capacity() - 1} ", 1))),
        ("a reuse after a sequence gap that reassembled to another payload", swap("links reuseafter sequence-gap ", corrupt_last(good[first("links reuseafter sequence-gap ")]))),
        ("a reuse after an early end that is not one of the pinned causes", swap("links reuseafter sequence-gap ", good[first("links reuseafter sequence-gap ")].replace("sequence-gap", "timeout", 1))),
        ("the split across width steps dropped", drop("links widthsplit ")),
        ("the split from 127 dropped", drop("links widthsplit 127 ")),
        ("the split from 268435455 dropped", drop("links widthsplit 268435455 ")),
        ("a second fragment sized from the first header at the one-byte step", swap("links widthsplit 127 1 ", good[first("links widthsplit 127 1 ")].replace(f" {len(width_split(127)[1])} ", f" {len(width_split(127)[1]) + 1} ", 1))),
        ("a second fragment sized from the first header at the two-byte step", swap("links widthsplit 16383 1 ", good[first("links widthsplit 16383 1 ")].replace(f" {len(width_split(16383)[1])} ", f" {len(width_split(16383)[1]) + 1} ", 1))),
        ("a split across a width step with another body", swap("links widthsplit 2097151 2 ", corrupt_last(good[first("links widthsplit 2097151 2 ")]))),
        ("fragment-interior-marked-final admitted instead of refused", drop("links refused fragment-interior-marked-final ")),
        ("fragment-interior-marked-final refused with the wrong class", swap("links refused fragment-interior-marked-final ", "links refused fragment-interior-marked-final over-bound")),
        ("fragment-final-marked-more admitted instead of refused", drop("links refused fragment-final-marked-more ")),
        ("fragment-final-marked-more refused with the wrong class", swap("links refused fragment-final-marked-more ", "links refused fragment-final-marked-more over-bound")),
        ("an interior fragment marked final taken as a complete message", drop("links refused fragment-interior-marked-final ")),
        ("a last fragment still saying more taken as complete", drop("links refused fragment-final-marked-more ")),
        ("an interior fragment marked final refused as reassembly-timeout", swap("links refused fragment-interior-marked-final ", "links refused fragment-interior-marked-final reassembly-timeout")),
        ("a last fragment still saying more refused as truncated", swap("links refused fragment-final-marked-more ", "links refused fragment-final-marked-more truncated")),
        ("a dropped aliased-Put line", drop("links aliased ")),
        ("an aliased Put reported receiver-mapped", swap("links aliased ", good[first("links aliased ")].replace(" 5d ", " 1d ", 1))),
        ("an aliased Put of another size", swap("links aliased ", good[first("links aliased ")].replace(" 5000 ", " 4999 ", 1))),
        ("an aliased Put that reassembled to another payload", swap("links aliased ", corrupt_last(good[first("links aliased ")]))),
        ("the sender-mapped alias scenario dropped", [line for line in good if line != "links ok reassemble-sender-mapped-alias"]),
        ("a demo payload of 13 bytes admitted", drop("links refused demo-refuses-payload-13-bytes ")),
        ("a large payload of 1025 bytes admitted", drop("links refused large-refuses-payload-1025-bytes ")),
        ("a large payload of 1025 bytes refused as out-of-memory", swap("links refused large-refuses-payload-1025-bytes ", "links refused large-refuses-payload-1025-bytes out-of-memory")),
        *[("fragment-sequence-noncanonical admitted instead of refused", drop("links refused fragment-sequence-noncanonical ")),
          ("fragment-sequence-noncanonical refused with the wrong class", swap("links refused fragment-sequence-noncanonical ", "links refused fragment-sequence-noncanonical unsupported-message"))],
        *[("fragment-sequence-over-32-bits admitted instead of refused", drop("links refused fragment-sequence-over-32-bits ")),
          ("fragment-sequence-over-32-bits refused with the wrong class", swap("links refused fragment-sequence-over-32-bits ", "links refused fragment-sequence-over-32-bits unsupported-message"))],
        *[("fragmented-put-scope-noncanonical admitted instead of refused", drop("links refused fragmented-put-scope-noncanonical ")),
          ("fragmented-put-scope-noncanonical refused with the wrong class", swap("links refused fragmented-put-scope-noncanonical ", "links refused fragmented-put-scope-noncanonical unsupported-message"))],
        *[("fragmented-put-key-length-noncanonical admitted instead of refused", drop("links refused fragmented-put-key-length-noncanonical ")),
          ("fragmented-put-key-length-noncanonical refused with the wrong class", swap("links refused fragmented-put-key-length-noncanonical ", "links refused fragmented-put-key-length-noncanonical unsupported-message"))],
        *[("fragmented-put-attachment-length-noncanonical admitted instead of refused", drop("links refused fragmented-put-attachment-length-noncanonical ")),
          ("fragmented-put-attachment-length-noncanonical refused with the wrong class", swap("links refused fragmented-put-attachment-length-noncanonical ", "links refused fragmented-put-attachment-length-noncanonical unsupported-message"))],
        *[("fragmented-put-payload-length-noncanonical admitted instead of refused", drop("links refused fragmented-put-payload-length-noncanonical ")),
          ("fragmented-put-payload-length-noncanonical refused with the wrong class", swap("links refused fragmented-put-payload-length-noncanonical ", "links refused fragmented-put-payload-length-noncanonical unsupported-message"))],
        *[("fragmented-put-payload-length-over-32-bits admitted instead of refused", drop("links refused fragmented-put-payload-length-over-32-bits ")),
          ("fragmented-put-payload-length-over-32-bits refused with the wrong class", swap("links refused fragmented-put-payload-length-over-32-bits ", "links refused fragmented-put-payload-length-over-32-bits unsupported-message"))],
        ("fixed-refuses-fragment-sequence-noncanonical admitted instead of refused", drop("links refused fixed-refuses-fragment-sequence-noncanonical ")),
        ("fixed-refuses-fragment-sequence-noncanonical refused with the wrong class", swap("links refused fixed-refuses-fragment-sequence-noncanonical ", "links refused fixed-refuses-fragment-sequence-noncanonical over-bound")),
        ("fixed-refuses-fragment-sequence-over-32-bits admitted instead of refused", drop("links refused fixed-refuses-fragment-sequence-over-32-bits ")),
        ("fixed-refuses-fragment-sequence-over-32-bits refused with the wrong class", swap("links refused fixed-refuses-fragment-sequence-over-32-bits ", "links refused fixed-refuses-fragment-sequence-over-32-bits over-bound")),
        ("fragmented-put-alias-for-another-key admitted instead of refused", drop("links refused fragmented-put-alias-for-another-key ")),
        ("fragmented-put-alias-for-another-key refused with the wrong class", swap("links refused fragmented-put-alias-for-another-key ", "links refused fragmented-put-alias-for-another-key over-bound")),
        ("fragmented-put-receiver-mapped-alias admitted instead of refused", drop("links refused fragmented-put-receiver-mapped-alias ")),
        ("fragmented-put-receiver-mapped-alias refused with the wrong class", swap("links refused fragmented-put-receiver-mapped-alias ", "links refused fragmented-put-receiver-mapped-alias over-bound")),
        ("fragmented-put-key-length-over-16-bits admitted instead of refused", drop("links refused fragmented-put-key-length-over-16-bits ")),
        ("fragmented-put-key-length-over-16-bits refused with the wrong class", swap("links refused fragmented-put-key-length-over-16-bits ", "links refused fragmented-put-key-length-over-16-bits over-bound")),
        ("fragmented-put-attachment-length-over-32-bits admitted instead of refused", drop("links refused fragmented-put-attachment-length-over-32-bits ")),
        ("fragmented-put-attachment-length-over-32-bits refused with the wrong class", swap("links refused fragmented-put-attachment-length-over-32-bits ", "links refused fragmented-put-attachment-length-over-32-bits over-bound")),
        ("a fragmented wildcard key admitted", drop("links refused fragmented-put-key-wildcard ")),
        ("a fragmented wildcard key refused as invalid-keyexpr", swap("links refused fragmented-put-key-wildcard ", "links refused fragmented-put-key-wildcard invalid-keyexpr")),
        ("a fragmented leading-slash key admitted", drop("links refused fragmented-put-key-leading-slash ")),
        ("a fragmented leading-slash key refused as wildcard-keyexpr", swap("links refused fragmented-put-key-leading-slash ", "links refused fragmented-put-key-leading-slash wildcard-keyexpr")),
        ("a fragmented double-slash key admitted", drop("links refused fragmented-put-key-double-slash ")),
        ("a fragmented key that is not UTF-8 admitted", drop("links refused fragmented-put-key-invalid-utf8 ")),
        ("a fragmented key that is not UTF-8 refused as malformed-message", swap("links refused fragmented-put-key-invalid-utf8 ", "links refused fragmented-put-key-invalid-utf8 malformed-message")),
        ("a fragmented Put for a key that was not subscribed admitted", drop("links refused fragmented-put-key-not-subscribed ")),
        ("a fragmented Put for a key that was not subscribed refused as invalid-keyexpr", swap("links refused fragmented-put-key-not-subscribed ", "links refused fragmented-put-key-not-subscribed invalid-keyexpr")),
        ("a fragmented 130-byte key admitted", drop("links refused fragmented-put-key-130-bytes ")),
        ("a fragmented 130-byte key refused as invalid-keyexpr", swap("links refused fragmented-put-key-130-bytes ", "links refused fragmented-put-key-130-bytes invalid-keyexpr")),
        ("a fragmented Put for an undeclared alias admitted", drop("links refused fragmented-put-undeclared-alias ")),
        ("a fragmented Put for an undeclared alias refused as over-bound", swap("links refused fragmented-put-undeclared-alias ", "links refused fragmented-put-undeclared-alias over-bound")),
        ("a dropped declared-payload line", drop("links payload ")),
        ("the declared payload reported as 5000 bytes", swap("links payload ", good[first("links payload ")].replace(f" {HEAP_PAYLOAD} ", " 5000 ", 1))),
        ("the declared payload reassembled to another payload", swap("links payload ", corrupt_last(good[first("links payload ")]))),
        ("the declared payload into storage of one message", swap("links payload ", good[first("links payload ")].replace(f" {heap_needed(pinned_configs()[2])} ", f" {message_bytes(pinned_configs()[2])} ", 1))),
        ("the stack sizes dropped", drop("links size ")),
        ("one stack size dropped", drop("links size large ")),
        ("a measured size one byte over the declared stack", swap("links size large ", f"links size large {pinned_configs()[1]['stackBytes'] + 1} {pinned_configs()[1]['stackBytes']}")),
        ("a measured size one byte over the contract's formula but under the declared stack", swap("links size large ", f"links size large {required_stack(pinned_configs()[1]) + 1} {pinned_configs()[1]['stackBytes']}")),
        ("a measured size well over the formula and under the declared stack", swap("links size heap ", f"links size heap {pinned_configs()[2]['stackBytes'] - 1} {pinned_configs()[2]['stackBytes']}")),
        ("a measured size of zero", swap("links size demo ", f"links size demo 0 {pinned_configs()[0]['stackBytes']}")),
        ("a declared stack that differs from the pinned one", swap("links size heap ", f"links size heap 17736 {pinned_configs()[2]['stackBytes'] - 1}")),
        ("a size reported for a configuration that is not pinned", good + ["links size invented 1 1"]),
        ("a payload one byte over the declared one admitted", drop("links refused payload-over-declared-with-enough-storage ")),
        ("a payload over the declared one refused as out-of-memory", swap("links refused payload-over-declared-with-enough-storage ", "links refused payload-over-declared-with-enough-storage out-of-memory")),
        ("a fragment over the announced batch admitted", drop("links refused fragment-over-the-announced-batch ")),
        ("a fragment over the announced batch refused as over-bound", swap("links refused fragment-over-the-announced-batch ", "links refused fragment-over-the-announced-batch over-bound")),
        ("a dropped storage-reuse line", drop("links reuse ")),
        ("reuse reported into storage larger than one message", swap("links reuse ", good[first("links reuse ")].replace(f" {reuse_capacity()} ", f" {reuse_capacity() * 2} ", 1))),
        ("reuse reported into storage one byte short of the message", swap("links reuse ", good[first("links reuse ")].replace(f" {reuse_capacity()} ", f" {reuse_capacity() - 1} ", 1))),
        ("reuse of a payload that is not the one sent", swap("links reuse ", corrupt_last(good[first("links reuse ")]))),
        ("reuse of a shorter message", swap("links reuse ", good[first("links reuse ")].replace(f" {REUSE_SIZE} ", f" {REUSE_SIZE - 1} ", 1))),
        ("the reuse scenario reported without its line", drop("links reuse ")),
        ("two messages in one run admitted instead of refused", drop("links refused fragments-of-two-messages-in-one-run ")),
        ("two messages in one run refused with the wrong class", swap("links refused fragments-of-two-messages-in-one-run ", "links refused fragments-of-two-messages-in-one-run malformed-message")),
        ("a best-effort fragment mid-run admitted instead of refused", drop("links refused best-effort-fragment-during-reassembly ")),
        ("a fragmented Put without the attachment admitted", drop("links refused fragmented-put-without-attachment ")),
        ("a fragmented Put without the attachment refused with the wrong class", swap("links refused fragmented-put-without-attachment ", "links refused fragmented-put-without-attachment unsupported-extension")),
        ("a fragmented Put with a short attachment admitted", drop("links refused fragmented-put-with-short-attachment ")),
        ("a fragmented Put with a wrong GID length admitted", drop("links refused fragmented-put-gid-length-15 ")),
        ("a fragmented Put with a GID length of 17 admitted", drop("links refused fragmented-put-gid-length-17 ")),
        ("a fragmented Put with a GID length of 17 refused with the wrong class", swap("links refused fragmented-put-gid-length-17 ", "links refused fragmented-put-gid-length-17 over-bound")),
        ("a fragmented Put with a 34-byte attachment admitted", drop("links refused fragmented-put-attachment-34-bytes ")),
        ("a fragmented Put with a 34-byte attachment refused as malformed-message", swap("links refused fragmented-put-attachment-34-bytes ", "links refused fragmented-put-attachment-34-bytes malformed-message")),
        ("a fragmented Put with a wrong GID length refused with the wrong class", swap("links refused fragmented-put-gid-length-15 ", "links refused fragmented-put-gid-length-15 unsupported-extension")),
        ("a fragmented alias scope of 0 admitted", drop("links refused fragmented-put-alias-scope-zero ")),
        ("a fragmented alias scope of 65536 admitted", drop("links refused fragmented-put-alias-scope-65536 ")),
        ("a fragmented alias scope of 65536 refused with the wrong class", swap("links refused fragmented-put-alias-scope-65536 ", "links refused fragmented-put-alias-scope-65536 invalid-keyexpr")),
        ("a dropped session-sequence line", drop("links advance fullkey-5000-2048 ")),
        ("a session sequence that did not advance", swap("links advance fullkey-5000-2048 ", "links advance fullkey-5000-2048 1000 3 1000")),
        ("a session sequence that advanced by one for three fragments", swap("links advance fullkey-5000-2048 ", "links advance fullkey-5000-2048 1000 3 1001")),
        ("a session sequence that did not wrap at 32 bits", swap("links advance fullkey-wraps-32-bits ", "links advance fullkey-wraps-32-bits 4294967294 3 4294967297")),
        ("a session sequence that wrapped to the wrong number", swap("links advance fullkey-wraps-32-bits ", "links advance fullkey-wraps-32-bits 4294967294 3 0")),
        ("the wrap scenario dropped", drop("links advance fullkey-wraps-32-bits ")),
        ("a stale first sequence admitted instead of refused", drop("links refused fragment-stale-first-sequence ")),
        ("a skipped first sequence admitted instead of refused", drop("links refused fragment-skipped-first-sequence ")),
        ("a stale first sequence refused with the wrong class", swap("links refused fragment-stale-first-sequence ", "links refused fragment-stale-first-sequence fragment-sequence")),
        ("a fixed link that took a best-effort fragment", drop("links refused fixed-refuses-best-effort-fragment ")),
        ("a fixed link that took a fragment with an extension", drop("links refused fixed-refuses-fragment-with-extension ")),
        ("a fixed link that took an empty fragment", drop("links refused fixed-refuses-empty-fragment ")),
        ("a fixed link that refused an extension fragment as unsupported-extension", swap("links refused fixed-refuses-fragment-with-extension ", "links refused fixed-refuses-fragment-with-extension unsupported-extension")),
        ("a fixed link that refused an empty fragment as malformed-message", swap("links refused fixed-refuses-empty-fragment ", "links refused fixed-refuses-empty-fragment malformed-message")),
        ("a refused run that changed the session", drop("links ok refused-fragment-run-leaves-session-unchanged")),
        ("every refusal removed", drop("links refused ")),
        ("a dropped refusal", drop("links refused fixed-refuses-fragment ")),
        ("a duplicated refusal", good + ["links refused fixed-refuses-fragment unsupported-message"]),
        ("a refusal with the wrong class", swap("links refused fragment-sequence-gap ", "links refused fragment-sequence-gap unexpected-message")),
        ("a refusal with an invented class", swap("links refused reassembly-timeout ", "links refused reassembly-timeout invented")),
        ("an unnamed refusal", good + ["links refused invented unsupported-message"]),
        ("a fixed link that took a fragment", drop("links refused fixed-refuses-fragment ")),
        ("a fifth subscriber admitted instead of refused", drop("links refused fifth-subscriber ")),
        ("a fifth alias refused with the wrong class", swap("links refused fifth-alias ", "links refused fifth-alias subscriber-limit")),
        ("a refused allocation reported as ok", [line for line in good if "storage-refused" not in line] + ["links ok storage-refused"]),
        ("a refusal reported as an ok scenario", good + ["links ok reassembly-timeout"]),
    ]


def transcript_controls() -> int:
    good = good_lines()
    judge(good)
    refused = 0
    for label, mutated in transcript_mutations(good):
        try:
            judge(mutated)
        except LinksError:
            refused += 1
            continue
        raise LinksError(f"control: the links judge accepted {label}")
    return refused


def reference_controls() -> int:
    """The host reference refuses a damaged capture, a gap, a reordering and a truncation."""
    stock_references()
    size, batch, sn, *_ = zenoh_fragment.CAPTURES[0]
    bodies = zenoh_fragment.reference_split(zenoh_fragment.message(size), batch, sn)
    def clear_more(body: bytes) -> bytes:
        return bytes([body[0] & ~zenoh_fragment.FLAG_M]) + body[1:]

    def set_more(body: bytes) -> bytes:
        return bytes([body[0] | zenoh_fragment.FLAG_M]) + body[1:]

    def attachment_length_65536() -> bytes:
        whole = bytearray(zenoh_fragment.full_key_for_exam(5))
        at = whole.index(bytes([zenoh_wire.ID_PUT | zenoh_wire.FLAG_Z, zenoh_wire.EXT_ATTACHMENT])) + 2
        return bytes(whole[:at]) + bytes([0x80, 0x80, 0x04]) + bytes(whole[at + 1 :])

    def with_leb(offset: int, old_len: int, value: bytes, message: bytes) -> bytes:
        return message[:offset] + value + message[offset + old_len :]

    def attachment_length_at(message: bytes) -> int:
        return message.index(bytes([zenoh_wire.ID_PUT | zenoh_wire.FLAG_Z, zenoh_wire.EXT_ATTACHMENT])) + 2

    def attachment_length_2_31() -> bytes:
        whole = zenoh_fragment.full_key_for_exam(5)
        return with_leb(attachment_length_at(whole), 1, bytes([0x80, 0x80, 0x80, 0x80, 0x08]), whole)

    def payload_length_2_31() -> bytes:
        whole = zenoh_fragment.full_key_for_exam(5)
        return with_leb(attachment_length_at(whole) + 1 + 33, 1, bytes([0x80, 0x80, 0x80, 0x80, 0x08]), whole)

    def key_length_32768() -> bytes:
        whole = full_key(10, 20)
        return with_leb(2, 1, bytes([0x80, 0x80, 0x02]), whole)

    def attachment_cut() -> bytes:
        whole = zenoh_fragment.full_key_for_exam(5)
        return whole[: attachment_length_at(whole) + 1 + 10]

    def cached_width_split(message: bytes, batch_bytes: int, start: int) -> list[bytes]:
        """A splitter that sizes every slice from the first fragment's header: wrong across a width step."""
        chunk = batch_bytes - zenoh_fragment.STREAM_LENGTH_BYTES - 1 - len(zenoh_wire.leb128(start))
        out, at, number = [], 0, start
        while at < len(message):
            piece = message[at : at + chunk]
            at += len(piece)
            flags = zenoh_fragment.ID_FRAGMENT | zenoh_fragment.FLAG_R | (zenoh_fragment.FLAG_M if at < len(message) else 0)
            out.append(bytes([flags]) + zenoh_wire.leb128(number) + piece)
            number += 1
        return out

    def key_length_over() -> bytes:
        whole = bytearray(full_key(10, 20))
        return bytes(whole[:2]) + bytes([0x80, 0x80, 0x04]) + bytes(whole[3:])

    def attachment_length_over() -> bytes:
        whole = bytearray(zenoh_fragment.full_key_for_exam(5))
        at = whole.index(bytes([zenoh_wire.ID_PUT | zenoh_wire.FLAG_Z, zenoh_wire.EXT_ATTACHMENT])) + 2
        return bytes(whole[:at]) + bytes([0x80, 0x80, 0x80, 0x80, 0x10]) + bytes(whole[at + 1 :])

    def noncanonical_key_length() -> bytes:
        whole = bytearray(full_key(10, 20))
        return bytes(whole[:2]) + bytes([0x8A, 0x00]) + bytes(whole[3:])

    def noncanonical_attachment_length() -> bytes:
        whole = bytearray(zenoh_fragment.full_key_for_exam(5))
        at = whole.index(bytes([zenoh_wire.ID_PUT | zenoh_wire.FLAG_Z, zenoh_wire.EXT_ATTACHMENT])) + 2
        return bytes(whole[:at]) + bytes([0xA1, 0x00]) + bytes(whole[at + 1 :])

    def invalid_utf8_key() -> bytes:
        whole = bytearray(zenoh_wire.push_put("abcd", zenoh_fragment.attachment(), zenoh_fragment.payload_bytes(1), True))
        whole[whole.index(b"abcd") + 1] = 0xFF
        return bytes(whole)

    def full_key(length: int, payload: int, scope: int = 0) -> bytes:
        key = "k" * length
        message = bytearray(zenoh_wire.push_put(key, zenoh_fragment.attachment(), zenoh_fragment.payload_bytes(payload), True))
        if length == 0:
            message[1:3] = bytes([0x00, 0x00])
        if scope:
            message[1] = scope
        return bytes(message)

    def with_scope(scope: bytes, header: int = 0x5D) -> bytes:
        whole = zenoh_fragment.message(1)
        return bytes([header]) + scope + whole[2:]

    def no_attachment(payload: int) -> bytes:
        return bytes([0x1D, 0x01, zenoh_wire.ID_PUT]) + zenoh_wire.leb128(payload) + zenoh_fragment.payload_bytes(payload)

    def bad_gid(payload: int, length: int) -> bytes:
        whole = bytearray(zenoh_fragment.message(payload))
        whole[whole.index(bytes([zenoh_wire.ID_PUT | zenoh_wire.FLAG_Z, zenoh_wire.EXT_ATTACHMENT])) + 2 + 1 + 16] = length
        return bytes(whole)

    def long_attachment(payload: int) -> bytes:
        att = zenoh_fragment.attachment() + b"\x00"
        return (
            bytes([0x1D, 0x01, zenoh_wire.ID_PUT | zenoh_wire.FLAG_Z, zenoh_wire.EXT_ATTACHMENT])
            + zenoh_wire.leb128(len(att)) + att + zenoh_wire.leb128(payload) + zenoh_fragment.payload_bytes(payload)
        )

    def short_attachment(payload: int) -> bytes:
        att = zenoh_fragment.attachment()[:32]
        return (
            bytes([0x1D, 0x01, zenoh_wire.ID_PUT | zenoh_wire.FLAG_Z, zenoh_wire.EXT_ATTACHMENT])
            + zenoh_wire.leb128(len(att)) + att + zenoh_wire.leb128(payload) + zenoh_fragment.payload_bytes(payload)
        )

    cases: list[tuple[str, Callable[[], object]]] = [
        ("a gap in the sequence numbers", lambda: zenoh_fragment.reassemble([bodies[0], bodies[2]])),
        ("fragments out of order", lambda: zenoh_fragment.reassemble([bodies[1], bodies[0], bodies[2]])),
        ("a run that never ends", lambda: zenoh_fragment.reassemble(bodies[:2])),
        ("a final fragment followed by another", lambda: zenoh_fragment.reassemble([clear_more(bodies[0]), *bodies[1:]])),
        ("a last fragment that still says more follows", lambda: zenoh_fragment.reassemble([*bodies[:2], set_more(bodies[2])])),
        ("a repeated final fragment", lambda: zenoh_fragment.reassemble([bodies[2], bodies[2]])),
        ("no fragments", lambda: zenoh_fragment.reassemble([])),
        ("an unreliable fragment", lambda: zenoh_fragment.decode_fragment(bytes([bodies[0][0] & ~zenoh_fragment.FLAG_R]) + bodies[0][1:])),
        ("a fragment with extensions", lambda: zenoh_fragment.decode_fragment(bytes([bodies[0][0] | zenoh_fragment.FLAG_Z]) + bodies[0][1:])),
        ("a fragment that is another message", lambda: zenoh_fragment.decode_fragment(bytes([0x25]) + bodies[0][1:])),
        ("a fragment with no data", lambda: zenoh_fragment.decode_fragment(bodies[0][:5])),
        ("a message whose payload length is wrong", lambda: zenoh_fragment.decode_message(zenoh_fragment.message(size)[:-1])),
        ("a batch too small for a fragment", lambda: zenoh_fragment.reference_split(b"x" * 10, 8, 0)),
        ("a fragment one byte over the announced batch", lambda: zenoh_fragment.decode_fragment(bodies[0] + b"x", batch)),
        ("a run holding a fragment over the announced batch", lambda: zenoh_fragment.reassemble([bodies[0] + b"x", *bodies[1:]], sn, batch)),
        ("a fragmented full-key Put with a 130-byte key", lambda: zenoh_fragment.decode_message(zenoh_fragment.reassemble(zenoh_fragment.reference_split(full_key(130, 3000), 512, sn)), max_key=129)),
        ("a fragmented full key with a wildcard", lambda: zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(1, "a/*/b"), max_key=129)),
        ("a fragmented full key that is a double wildcard", lambda: zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(1, "a/**"), max_key=129)),
        ("a fragmented full key with a $ wildcard", lambda: zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(1, "a/b$*"), max_key=129)),
        ("a fragmented full key with a leading slash", lambda: zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(1, "/bad"), max_key=129)),
        ("a fragmented full key with a trailing slash", lambda: zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(1, "bad/"), max_key=129)),
        ("a fragmented full key with a double slash", lambda: zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(1, "a//b"), max_key=129)),
        ("a fragmented full key with a question mark", lambda: zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(1, "a?b"), max_key=129)),
        ("a fragmented full key with a hash", lambda: zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(1, "a#b"), max_key=129)),
        ("a fragmented full key that is not UTF-8", lambda: zenoh_fragment.decode_message(invalid_utf8_key(), max_key=129)),
        ("a fragmented full key other than the subscriber's", lambda: zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(1), max_key=129, subscribed="slime_demo/other")),
        ("a full-key Put in a nonzero scope", lambda: zenoh_fragment.decode_message(full_key(10, 20, scope=1), max_key=129)),
        ("a full-key Put with an empty key", lambda: zenoh_fragment.decode_message(full_key(0, 20), max_key=129)),
        ("a full-key Put with a truncated key", lambda: zenoh_fragment.decode_message(full_key(10, 20)[:8], max_key=129)),
        ("an alias the session never declared", lambda: zenoh_fragment.decode_message(with_scope(bytes([0x02])), alias=1)),
        ("an alias the session never declared, a larger one", lambda: zenoh_fragment.decode_message(with_scope(bytes([0xFF, 0xFF, 0x03])), alias=1)),
        ("one payload byte over the large configuration", lambda: zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(1025), max_key=129, max_payload=1024)),
        ("13 payload bytes over the demo configuration", lambda: zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(13), max_key=129, max_payload=12)),
        ("a receiver-mapped alias where the peer declared it", lambda: zenoh_fragment.decode_message(with_scope(bytes([0x01]), 0x1D), alias=1)),
        ("an alias that stands for another key than the subscriber's", lambda: zenoh_fragment.decode_message(with_scope(bytes([0x01])), alias=1, alias_key="slime_demo/other", subscribed=key())),
        ("a key length above 16 bits", lambda: zenoh_fragment.decode_message(key_length_over(), max_key=129)),
        ("an attachment length above 32 bits", lambda: zenoh_fragment.decode_message(attachment_length_over(), max_key=129)),
        ("a canonical alias scope of 256", lambda: zenoh_fragment.decode_message(with_scope(bytes([0x80, 0x02])), alias=1)),
        ("a canonical key length of 256", lambda: zenoh_fragment.decode_message(zenoh_wire.push_put("k" * 256, zenoh_fragment.attachment(), zenoh_fragment.payload_bytes(5), True), max_key=129)),
        ("a canonical attachment length of 65536", lambda: zenoh_fragment.decode_message(attachment_length_65536(), max_key=129)),
        ("a canonical payload length of 65536 against 16384", lambda: zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(65536), max_key=129, max_payload=16384)),
        ("a canonical alias scope of 32768", lambda: zenoh_fragment.decode_message(with_scope(bytes([0x80, 0x80, 0x02])), alias=1)),
        ("a canonical key length of 32768", lambda: zenoh_fragment.decode_message(key_length_32768(), max_key=129)),
        ("a canonical attachment length of 2147483648", lambda: zenoh_fragment.decode_message(attachment_length_2_31(), max_key=129)),
        ("a canonical payload length of 2147483648 against 16384", lambda: zenoh_fragment.decode_message(payload_length_2_31(), max_key=129, max_payload=16384)),
        ("a message cut off inside the attachment", lambda: zenoh_fragment.decode_message(attachment_cut(), max_key=129)),
        ("a run sized from its first fragment's header across a width step", lambda: zenoh_fragment.reassemble(cached_width_split(full_key_message(FULL_KEY_PAYLOAD), FULL_KEY_BATCH, 127), 127, FULL_KEY_BATCH)),
        ("a run that emits 4294967296 instead of wrapping", lambda: zenoh_fragment.reassemble(cached_width_split(full_key_message(FULL_KEY_PAYLOAD), FULL_KEY_BATCH, 0xFFFFFFFE), 0xFFFFFFFE, FULL_KEY_BATCH)),
        ("a payload shorter than its length says", lambda: zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(100)[:-10], max_key=129)),
        ("a payload one byte short", lambda: zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(100)[:-1], max_key=129)),
        ("a last fragment one millisecond past the timeout", lambda: zenoh_fragment.check_deadline(0, TIMEOUT_MS + 1, TIMEOUT_MS)),
        ("a last fragment before the first", lambda: zenoh_fragment.check_deadline(10, 9, TIMEOUT_MS)),
        ("a timeout of one millisecond with a last fragment at two", lambda: zenoh_fragment.check_deadline(0, 2, 1)),
        ("an alias scope of 0", lambda: zenoh_fragment.decode_message(with_scope(bytes([0x00])))),
        ("an alias scope of 65536", lambda: zenoh_fragment.decode_message(with_scope(bytes([0x80, 0x80, 0x04])))),
        ("an alias scope of four bytes", lambda: zenoh_fragment.decode_message(with_scope(bytes([0xFF, 0xFF, 0xFF, 0x7F])))),
        ("a noncanonical key length", lambda: zenoh_fragment.decode_message(noncanonical_key_length(), max_key=129)),
        ("a noncanonical attachment length", lambda: zenoh_fragment.decode_message(noncanonical_attachment_length(), max_key=129)),
        ("a noncanonical alias scope of 1", lambda: zenoh_fragment.decode_message(with_scope(bytes([0x81, 0x00])))),
        ("two messages joined in one run", lambda: zenoh_fragment.decode_message(zenoh_fragment.reassemble(zenoh_fragment.reference_split(zenoh_fragment.message(1000) + zenoh_fragment.message(1000), 512, sn)))),
        ("a fragmented Put without the attachment", lambda: zenoh_fragment.decode_message(zenoh_fragment.reassemble(zenoh_fragment.reference_split(no_attachment(3000), 512, sn)))),
        ("a fragmented Put whose GID length byte is 15", lambda: zenoh_fragment.decode_message(zenoh_fragment.reassemble(zenoh_fragment.reference_split(bad_gid(3000, 15), 512, sn)))),
        ("a fragmented Put whose GID length byte is 17", lambda: zenoh_fragment.decode_message(zenoh_fragment.reassemble(zenoh_fragment.reference_split(bad_gid(3000, 17), 512, sn)))),
        ("a fragmented Put with a 34-byte attachment", lambda: zenoh_fragment.decode_message(zenoh_fragment.reassemble(zenoh_fragment.reference_split(long_attachment(3000), 512, sn)))),
        ("a fragmented Put with a 32-byte attachment", lambda: zenoh_fragment.decode_message(zenoh_fragment.reassemble(zenoh_fragment.reference_split(short_attachment(3000), 512, sn)))),
        ("a run replayed from before the session's next sequence number", lambda: zenoh_fragment.reassemble(bodies, sn + 1000)),
        ("a run that skips ahead of the session's next sequence number", lambda: zenoh_fragment.reassemble(bodies, sn - 1)),
        ("a run one past the session's next sequence number", lambda: zenoh_fragment.reassemble(bodies, sn + 1)),
        ("a noncanonical sequence number (26 80 00 aa)", lambda: zenoh_fragment.decode_fragment(bytes.fromhex("268000aa"))),
        ("a sequence number over 32 bits (26 ff ff ff ff 10 aa)", lambda: zenoh_fragment.decode_fragment(bytes.fromhex("26ffffffff10aa"))),
        ("a sequence number of six bytes", lambda: zenoh_fragment.decode_fragment(bytes.fromhex("26ffffffffff0faa"))),
        ("a truncated sequence number", lambda: zenoh_fragment.decode_fragment(bytes.fromhex("2680"))),
        ("a noncanonical scope in the message", lambda: zenoh_fragment.decode_message(bytes.fromhex("1d8000") + zenoh_fragment.message(size)[2:])),
        ("a noncanonical payload length in the message", lambda: zenoh_fragment.decode_message(zenoh_fragment.message(size)[: -size - 2] + bytes.fromhex("a8a700") + zenoh_fragment.payload_bytes(size))),
    ]
    # A control that is refused for another reason proves nothing: these name what the refusal says.
    reasons = {
        "a noncanonical key length": "noncanonical",
        "a noncanonical attachment length": "noncanonical",
        "a key length above 16 bits": "over 16 bits",
        "an attachment length above 32 bits": "over 32 bits",
        "a receiver-mapped alias where the peer declared it": "receiver-mapped",
        "an alias that stands for another key than the subscriber's": "stands for a key",
        "a canonical alias scope of 256": "never declared",
        "a canonical key length of 256": "longer than the configuration declares",
        "a canonical attachment length of 65536": "not 33 bytes",
        "a canonical payload length of 65536 against 16384": "longer than the configuration declares",
        "a payload shorter than its length says": "truncated",
        "a payload one byte short": "truncated",
        "a canonical alias scope of 32768": "never declared",
        "a canonical key length of 32768": "longer than the configuration declares",
        "a canonical attachment length of 2147483648": "not 33 bytes",
        "a canonical payload length of 2147483648 against 16384": "longer than the configuration declares",
        "a message cut off inside the attachment": "truncated",
        "a run that emits 4294967296 instead of wrapping": "over 32 bits",
        "a run that never ends": "still says more",
        "a final fragment followed by another": "marked final is followed",
        "a last fragment that still says more follows": "still says more",
        "a repeated final fragment": "not consecutive",
        "a run sized from its first fragment's header across a width step": "announced batch",
        "two messages joined in one run": "trailing-bytes",
        "a last fragment one millisecond past the timeout": "reassembly-timeout",
        "a timeout of one millisecond with a last fragment at two": "reassembly-timeout",
    }
    refused = 0
    for label, action in cases:
        try:
            action()
        except zenoh_fragment.FragmentError as error:
            if label in reasons and reasons[label] not in str(error):
                raise LinksError(f"control: {label} was refused for another reason: {error}") from None
            refused += 1
            continue
        raise LinksError(f"control: the fragment reference accepted {label}")
    # A fragment exactly at the announced batch less the stream length is taken.
    if len(bodies[0]) != batch - 2 or zenoh_fragment.decode_fragment(bodies[0], batch).sn != sn:
        raise LinksError("control: a fragment at the announced batch was not taken")
    if zenoh_fragment.reassemble(bodies, sn, batch) != zenoh_fragment.message(size):
        raise LinksError("control: a run within the announced batch was not taken")
    # A peer that declared the alias addresses it sender-mapped, and that is read, also when it comes
    # from real fragments; the receiver mapping is read as a bare wire form and refused only where the
    # peer declared the alias.
    if zenoh_fragment.decode_message(with_scope(bytes([0x01]), 0x5D), alias=1)["scope"] != 1:
        raise LinksError("control: a sender-mapped declared alias was not read")
    if zenoh_fragment.decode_message(with_scope(bytes([0x01]), 0x1D))["scope"] != 1:
        raise LinksError("control: the receiver-mapped wire form was not read as a bare wire form")
    aliased_run = zenoh_fragment.reference_split(zenoh_fragment.message(5000, 0x5D), 2048, sn)
    if zenoh_fragment.decode_message(zenoh_fragment.reassemble(aliased_run, sn, 2048), alias=1)["payload"] != zenoh_fragment.payload_bytes(5000):
        raise LinksError("control: a sender-mapped aliased Put did not reassemble")
    # The values inside each width are read, so none of the refusals above is a blanket one: a two-byte
    # canonical scope or key length and a three-byte canonical payload length are taken when they are
    # what is declared, and a last fragment exactly at the timeout is taken.
    if zenoh_fragment.decode_message(with_scope(bytes([0x80, 0x02])), alias=256)["scope"] != 256:
        raise LinksError("control: a canonical scope of 256 was not read")
    if zenoh_fragment.decode_message(zenoh_wire.push_put("k" * 256, zenoh_fragment.attachment(), zenoh_fragment.payload_bytes(5), True), max_key=256)["key"] != b"k" * 256:
        raise LinksError("control: a canonical key length of 256 was not read")
    if len(zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(65536), max_key=129, max_payload=65536)["payload"]) != 65536:
        raise LinksError("control: a canonical payload length of 65536 was not read")
    if zenoh_fragment.decode_message(with_scope(bytes([0x80, 0x80, 0x02])), alias=32768)["scope"] != 32768:
        raise LinksError("control: a canonical scope of 32768 was not read")
    if zenoh_fragment.decode_message(full_key(32768, 5), max_key=32768)["key"] != b"k" * 32768:
        raise LinksError("control: a canonical key length of 32768 was not read")
    try:
        zenoh_fragment.decode_message(payload_length_2_31(), max_key=129, max_payload=1 << 31)
    except zenoh_fragment.FragmentError as error:
        if "truncated" not in str(error):
            raise LinksError(f"control: a payload length of 2147483648 was not read in full: {error}") from None
    else:
        raise LinksError("control: a payload of 2147483648 bytes was found in a short message")
    for start in WIDTH_STARTS:
        if zenoh_fragment.reassemble(width_split(start), start, FULL_KEY_BATCH) != full_key_message(FULL_KEY_PAYLOAD):
            raise LinksError(f"control: a run across the width step at {start} was not taken")
    for last in (TIMEOUT_MS - 1, TIMEOUT_MS):
        zenoh_fragment.check_deadline(0, last, TIMEOUT_MS)
    # The demo key is a valid key expression and the subscriber's own, so these refusals are not blanket.
    if zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(1), max_key=129, subscribed=key())["key"] != key().encode():
        raise LinksError("control: the subscriber's own key was not read")
    # The edges are taken: the demo key at exactly 129 bytes, the declared alias, and a payload exactly at
    # the declared size, so the refusals above are not blanket ones.
    if zenoh_fragment.decode_message(full_key(129, 3000), max_key=129)["key"] != b"k" * 129:
        raise LinksError("control: a 129-byte full key was not read")
    if zenoh_fragment.decode_message(with_scope(bytes([0x01])), alias=1)["scope"] != 1:
        raise LinksError("control: the declared alias was not read")
    for declared, payload_size in ((1024, 1024), (12, 12)):
        if len(zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(payload_size), max_key=129, max_payload=declared)["payload"]) != payload_size:
            raise LinksError("control: a payload exactly at the declared size was not read")
    # The largest and the smallest valid alias scope are read, so the scope refusals are not a blanket one.
    for good_scope in (bytes([0x01]), bytes([0xFF, 0xFF, 0x03])):
        if zenoh_fragment.decode_message(with_scope(good_scope))["payload"] != zenoh_fragment.payload_bytes(1):
            raise LinksError("control: a valid alias scope was not read")
    # A run starting exactly at the session's next sequence number is taken, also across the 32-bit wrap.
    if zenoh_fragment.reassemble(bodies, sn) != zenoh_fragment.message(size):
        raise LinksError("control: a run at the session's next sequence number was not taken")
    wrapped = zenoh_fragment.reference_split(zenoh_fragment.message(size), batch, 0xFFFFFFFE)
    if zenoh_fragment.reassemble(wrapped, 0xFFFFFFFE) != zenoh_fragment.message(size):
        raise LinksError("control: a run across the 32-bit wrap was not taken")
    # The largest canonical sequence number is accepted, so the refusals above are not a blanket one.
    if zenoh_fragment.decode_fragment(bytes.fromhex("26ffffffff0faa")).sn != 0xFFFFFFFF:
        raise LinksError("control: the largest canonical sequence number was not read")
    damaged = bytearray(bodies[1])
    damaged[10] ^= 1
    if hashlib.sha256(bytes(damaged)).hexdigest() == zenoh_fragment.CAPTURES[0][5][1]:
        raise LinksError("control: a damaged body still matches its pinned digest")
    return refused + 1


def all_controls() -> int:
    return transcript_controls() + reference_controls() + limits_controls()
