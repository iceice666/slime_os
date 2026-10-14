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
import zenoh_stock_peer
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
    ("heap", "heap", 2048, 16384, 129, 33, 4, 4, 4, 32768, 5000, 149004),
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
    "subscriber-ids-use-all-32-bits",
    "reassemble-short-key",
    "reassemble-high-aliases",
    "edge-batch-89",
    "edge-batch-4096",
    "edge-key-2028",
    "edge-payload-1mib",
    "edge-payload-2mib",
    "edge-payload-256mib",
    "edge-fixed-batch-89",
    "edge-fixed-batch-4096",
    "edge-minimum-fixed",
    "edge-minimum-heap",
    "edge-alias-long-key-2028",
    "edge-alias-long-key-4076",
    "reassemble-single-final-fragment",
    "reassemble-underfilled-interior",
    "nine-keys-subscribers-and-samples",
    "nine-keys-subscribers-and-samples-fixed",
    "wide-counts-256-heap",
    "wide-counts-256-fixed",
    "wide-counts-65535-heap",
    "wide-counts-65535-fixed",
    "reassemble-multibyte-key",
    "boundary-at-sequence-width-changes",
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
    # A sequence number that ends before it is complete is truncated on a heap link and, as every
    # malformed sequence number is, unsupported-message on a fixed one.
    ("fixed-refuses-fragment-sequence-truncated", "unsupported-message"),
    ("fragment-best-effort", "unsupported-message"),
    ("fragment-extension", "unsupported-extension"),
    ("empty-fragment", "malformed-message"),
    ("fragment-before-open", "unexpected-message"),
    ("fragment-sequence-gap", "fragment-sequence"),
    # A continuation must carry the next sequence number: one that repeats the number before it or goes
    # back is refused as the gap is, so chunks are never joined out of order.
    ("fragment-sequence-truncated", "truncated"),
    ("fragment-sequence-duplicate", "fragment-sequence"),
    ("fragment-sequence-backward", "fragment-sequence"),
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
    # A key named in full is global: one carrying a nonzero scope is relative to an alias, and is refused
    # as the whole-frame decoder refuses it, not delivered as if it were global.
    ("fragmented-put-full-key-nonzero-scope", "invalid-keyexpr"),
    ("fragmented-put-empty-key", "invalid-keyexpr"),
    # Every invalid key form the whole-frame `key_expression` refuses is refused after reassembly, a
    # trailing slash, `?` and `#` as invalid-keyexpr and `$` as a wildcard, and a message that ends
    # inside the bytes the key length declares is truncated.
    ("fragmented-put-key-trailing-slash", "invalid-keyexpr"),
    ("fragmented-put-key-question-mark", "invalid-keyexpr"),
    ("fragmented-put-key-hash", "invalid-keyexpr"),
    ("fragmented-put-key-dollar", "wildcard-keyexpr"),
    ("fragmented-put-key-truncated", "truncated"),
    # A PUSH whose body is not a Put is unsupported, one that ends before its body is truncated, and a
    # message that ends in the middle of any variable-length field of the reconstructed Put is
    # truncated, not defaulted, not malformed and not an overflow.
    ("fragmented-push-delete-body", "unsupported-message"),
    ("fragmented-push-ends-before-body", "truncated"),
    ("fragmented-put-scope-unterminated", "truncated"),
    ("fragmented-put-key-length-unterminated", "truncated"),
    ("fragmented-put-attachment-length-unterminated", "truncated"),
    ("fragmented-put-payload-length-unterminated", "truncated"),
    # The PUSH and the Put inside a fragmented message carry what a whole-frame one carries and nothing
    # more: each of these is refused with the class the corpus gives it.
    ("fragmented-push-extension-flag", "unsupported-extension"),
    ("fragmented-put-timestamp-flag", "unsupported-message"),
    ("fragmented-put-encoding-flag", "unsupported-message"),
    ("fragmented-put-unknown-extension", "unsupported-extension"),
    ("fragmented-put-extension-chained", "unsupported-extension"),
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
    # In a configuration of nine keys and nine subscribers the limit is nine, not four: the ninth is
    # taken, so a table sized for four cannot pass, and the tenth is refused as the fifth is.
    ("tenth-subscriber-in-nine", "subscriber-limit"),
    ("tenth-alias-in-nine", "over-bound"),
    # The same limit in fixed mode, where the tables are arrays and not storage the component supplies.
    ("tenth-subscriber-in-nine-fixed", "subscriber-limit"),
    ("tenth-alias-in-nine-fixed", "over-bound"),
    # With 256 slots the limit is 256 and not 255: the 257th subscriber and alias are refused as the
    # tenth is, in each mode.
    ("257th-subscriber-in-256-heap", "subscriber-limit"),
    ("257th-alias-in-256-heap", "over-bound"),
    ("257th-subscriber-in-256-fixed", "subscriber-limit"),
    ("257th-alias-in-256-fixed", "over-bound"),
    # A key is counted in encoded bytes, not characters: five characters of 10 bytes against a declared 9.
    ("fragmented-put-multibyte-key-over-declared", "over-bound"),
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


# A session that has been up a while: the first fragment arrives long after the clock passed the
# timeout, so a deadline compared with the absolute clock refuses it; a middle fragment arrives one
# millisecond before the edge, so a deadline refreshed by each continuation accepts the run that is
# one millisecond late, which it must not.
DEADLINE_FIRST = 7_000_000


# A subscriber id is an `EntityId`, 32 bits: the largest that fits 31 bits, the top bit alone and the
# largest of all, each declared and undeclared with the encodings pinned, so a reader or a slot of 16
# or 31 bits fails on a valid declaration.
SUBSCRIBER_IDS = (0, 0x7FFFFFFF, 0x80000000, 0xFFFFFFFF)


# Configurations at the edges the contract admits, driven through the link: a handshake that fills the
# smallest batch exactly and a declaration of the longest key it holds, a Put fragmented at the
# largest batch, and a key far past the 129 bytes the demo configuration uses, declared, used and
# undeclared. (name, batch, key, payload). The key is a valid key expression and is the subscriber's.
EDGE_SN = 1000
EDGES = (
    ("edge-batch-89", 89, "k" * 70, 300),
    ("edge-batch-4096", 4096, "k" * 4076, 16384),
    ("edge-key-2028", 2048, "k" * 2028, 5000),
    # A payload far above the 16384 of the heap configuration, sent and reassembled whole.
    ("edge-payload-1mib", 4096, "k" * 129, 1 << 20),
    # The payload length is a variable-length field: 1 MiB needs three bytes, 2 MiB four and 256 MiB
    # five, the first of the widest width, so a writer that stops at three or four bytes fails here.
    ("edge-payload-2mib", 4096, "k" * 129, 1 << 21),
    ("edge-payload-256mib", 4096, "k" * 129, 1 << 28),
)


# The same admitted bounds in fixed mode, where the whole Put travels in one FRAME and is never
# fragmented: the smallest batch, with the largest message the contract lets it hold, and the largest
# batch, with a key of 1000 bytes, far past the demo's 129. (name, batch, key, payload).
FIXED_EDGES = (
    ("edge-fixed-batch-89", 89, "k" * 10, 31),
    ("edge-fixed-batch-4096", 4096, "k" * 1000, 3046),
)
CHUNK = 1 << 20

# The smallest configuration the contract admits: a one-byte key and a one-byte payload in the smallest
# batch, one key, one subscriber and one sample, in each mode. The Put travels whole in one frame, as
# nothing this small is fragmented. (name, mode).
MINIMUM_EDGES = (("edge-minimum-fixed", "fixed"), ("edge-minimum-heap", "heap"))
MINIMUM_BATCH = 89
MINIMUM_KEY = "k"
MINIMUM_PAYLOAD = 1


def edge_config(batch: int, key: str, payload: int, mode: str = "heap") -> dict:
    c = dict(
        name="edge", mode=mode, batchBytes=batch, payloadBytes=payload, keyBytes=len(key),
        attachmentBytes=33, maxKeys=1, maxSubscribers=1, outstandingSamples=1,
        stackBytes=0, reassemblyTimeoutMs=5000 if mode == "heap" else 0, heapBytes=0,
    )
    c["stackBytes"] = required_stack(c)
    c["heapBytes"] = heap_needed(c) if mode == "heap" else 0
    return c


def pattern_digest(size: int) -> str:
    """The sha256 of the payload pattern of `size` bytes, taken a slice at a time."""
    hasher = hashlib.sha256()
    for offset in range(0, size, CHUNK):
        hasher.update(zenoh_fragment.payload_slice(offset, min(size, offset + CHUNK)))
    return hasher.hexdigest()


def edge_message(key: str, payload: int) -> bytearray:
    """Our own Put of `payload` bytes, built in place so a payload of 256 MiB is held once."""
    head = zenoh_wire.push_put(key, zenoh_fragment.attachment(), b"", True)
    if head[-1] != 0:
        raise LinksError("the empty-payload Put does not end in its zero length")
    prefix = head[:-1] + zenoh_wire.leb128(payload)
    message = bytearray(len(prefix) + payload)
    message[: len(prefix)] = prefix
    for offset in range(0, payload, CHUNK):
        stop = min(payload, offset + CHUNK)
        message[len(prefix) + offset : len(prefix) + stop] = zenoh_fragment.payload_slice(offset, stop)
    if payload <= 1 << 16 and bytes(message) != zenoh_wire.push_put(key, zenoh_fragment.attachment(), zenoh_fragment.payload_bytes(payload), True):
        raise LinksError("the Put built in place differs from the wire encoder's")
    return message


def tapped_bodies(message, batch: int, digests: list[str], first_sn: int = EDGE_SN):
    """The fragment bodies of `message`, noting the sha256 of each and keeping none."""
    for body in zenoh_fragment.split_iter(message, batch, first_sn):
        digests.append(sha(body))
        yield body


def init_ack_bytes(batch: int) -> bytes:
    """The longest INIT_ACK the profile permits, announcing `batch`."""
    return zenoh_wire.init(bytes(range(1, 17)), batch=batch, cookie=bytes(range(1, 65)))


def init_syn_bytes(batch: int) -> bytes:
    """The INIT_SYN a connector sends, announcing `batch`: the same ZID, no cookie."""
    return zenoh_wire.init(bytes(range(1, 17)), batch=batch)


def handshake_announce(name: str, batch: int) -> tuple[int, str, str]:
    """The batch the INIT_ACK and the INIT_SYN announce, each decoded from its bytes, and the sha256 of
    both. The INIT_ACK is the same 87 bytes for every non-default batch, so its length cannot tell them
    apart; a link that is the connector sends the INIT_SYN, which a link that is the acceptor never does,
    so a session that announced another batch there would pass an exam that pinned only the INIT_ACK."""
    ack, syn = init_ack_bytes(batch), init_syn_bytes(batch)
    announced = int.from_bytes(ack[20:22], "little")
    connector = int.from_bytes(syn[20:22], "little")
    if announced != batch or connector != batch or not syn[0] & zenoh_wire.FLAG_S:
        raise LinksError(f"{name}: the handshake announces {announced} and {connector} and not the batch {batch}")
    return announced, sha(ack), sha(syn)


def edge_sizes(name: str, batch: int, key: str) -> tuple[int, int, int]:
    """The longest handshake message and the declaration and undeclaration of `key`, stream length
    included, each of which must fit one batch because none can be fragmented."""
    handshake = len(zenoh_wire.init(bytes(range(1, 17)), batch=batch, cookie=bytes(range(1, 65)))) + 2
    declare = len(zenoh_wire.frame(0xFFFFFFFF, zenoh_wire.declare_subscriber(0xFFFFFFFF, key, True))) + 2
    undeclare = len(zenoh_wire.frame(0xFFFFFFFF, zenoh_wire.undeclare_subscriber(0xFFFFFFFF, key, True))) + 2
    if max(handshake, declare, undeclare) > batch:
        raise LinksError(f"{name}: a handshake or a declaration does not fit its batch")
    return handshake, declare, undeclare


@lru_cache(maxsize=None)
def edge_lines() -> tuple[str, ...]:
    """What the link must report at each heap edge, computed from the host reference after the limits
    contract has accepted the configuration the edge instantiates. The message is held once and the
    fragments are hashed as they are made, so a payload of 256 MiB costs 256 MiB and no more."""
    demo, large = pinned_configs()[:2]
    out = []
    for name, batch, key, payload in EDGES:
        if not contract_accepts(limits_record([demo, large, edge_config(batch, key, payload)])):
            raise LinksError(f"the limits contract refuses the configuration of {name}")
        handshake, declare, undeclare = edge_sizes(name, batch, key)
        announced, ack_digest, syn_digest = handshake_announce(name, batch)
        message = edge_message(key, payload)
        digests: list[str] = []
        joined = hashlib.sha256()
        for chunk in zenoh_fragment.reassemble_iter(tapped_bodies(message, batch, digests), EDGE_SN, batch):
            joined.update(chunk)
        if joined.hexdigest() != sha(message):
            raise LinksError(f"{name}: the host reference does not reassemble its own Put")
        decoded = zenoh_fragment.decode_message(message, max_key=len(key), subscribed=key)
        if bytes(decoded["key"]) != key.encode() or sha(decoded["payload"]) != pattern_digest(payload):
            raise LinksError(f"{name}: the host reference does not read its own Put back")
        del decoded, message
        out.append(
            f"links edge {name} {batch} {len(key)} {payload} {handshake} {declare} {undeclare} "
            f"{len(digests)} {sha(chr(10).join(digests).encode())} {pattern_digest(payload)} {announced} {ack_digest} {syn_digest}"
        )
    return tuple(out)


@lru_cache(maxsize=None)
def minimum_lines() -> tuple[str, ...]:
    """What a link must report for the smallest configuration the contract admits, in each mode."""
    demo, large = pinned_configs()[:2]
    out = []
    for name, mode in MINIMUM_EDGES:
        c = edge_config(MINIMUM_BATCH, MINIMUM_KEY, MINIMUM_PAYLOAD, mode)
        if not contract_accepts(limits_record([demo, large, c])):
            raise LinksError(f"the limits contract refuses the configuration of {name}")
        handshake, declare, undeclare = edge_sizes(name, MINIMUM_BATCH, MINIMUM_KEY)
        announced, ack_digest, syn_digest = handshake_announce(name, MINIMUM_BATCH)
        message = bytes(edge_message(MINIMUM_KEY, MINIMUM_PAYLOAD))
        frame = zenoh_wire.frame(FIXED_SN, message)
        if len(frame) + zenoh_fragment.STREAM_LENGTH_BYTES > MINIMUM_BATCH:
            raise LinksError(f"{name}: the Put does not travel whole in one frame")
        decoded = zenoh_fragment.decode_message(message, max_key=len(MINIMUM_KEY), max_payload=MINIMUM_PAYLOAD, subscribed=MINIMUM_KEY)
        if bytes(decoded["key"]) != MINIMUM_KEY.encode() or bytes(decoded["payload"]) != zenoh_fragment.payload_bytes(MINIMUM_PAYLOAD):
            raise LinksError(f"{name}: the host reference does not read its own Put back")
        out.append(
            f"links minimum {name} {mode} {MINIMUM_BATCH} {len(MINIMUM_KEY)} {MINIMUM_PAYLOAD} "
            f"{handshake} {declare} {undeclare} {len(frame)} {sha(frame)} {sha(zenoh_fragment.payload_bytes(MINIMUM_PAYLOAD))} {announced} {ack_digest} {syn_digest}"
        )
    return tuple(out)


@lru_cache(maxsize=None)
def fixed_edge_lines() -> tuple[str, ...]:
    """What a fixed link must report at each admitted edge: the Put travels whole, in one FRAME that
    fits the batch less its stream length, and is never fragmented."""
    demo, large = pinned_configs()[:2]
    out = []
    for name, batch, key, payload in FIXED_EDGES:
        if not contract_accepts(limits_record([demo, large, edge_config(batch, key, payload, "fixed")])):
            raise LinksError(f"the limits contract refuses the configuration of {name}")
        handshake, declare, undeclare = edge_sizes(name, batch, key)
        announced, ack_digest, syn_digest = handshake_announce(name, batch)
        if payload != fixed_max_payload(batch, len(key)):
            raise LinksError(f"{name}: {payload} is not the largest payload a batch of {batch} holds with a {len(key)}-byte key")
        message = bytes(edge_message(key, payload))
        frame = zenoh_wire.frame(FIXED_SN, message)
        if len(frame) + zenoh_fragment.STREAM_LENGTH_BYTES != batch:
            raise LinksError(f"{name}: the frame at the widest sequence number does not fill the batch exactly")
        decoded = zenoh_fragment.decode_message(message, max_key=len(key), subscribed=key)
        if bytes(decoded["key"]) != key.encode() or bytes(decoded["payload"]) != zenoh_fragment.payload_bytes(payload):
            raise LinksError(f"{name}: the host reference does not read its own Put back")
        out.append(
            f"links fixededge {name} {batch} {len(key)} {payload} {handshake} {declare} {undeclare} "
            f"{len(frame)} {sha(frame)} {sha(zenoh_fragment.payload_bytes(payload))} {announced} {ack_digest} {syn_digest}"
        )
    return tuple(out)


# The widest sequence number, five bytes: a frame that fills a batch only at that width is built, so a
# sender or a buffer that fails only there is caught. The frame is pinned by its sha256.
FIXED_SN = 0xFFFFFFFF


def fixed_edge_frame(batch: int, key: str, payload: int, sn: int) -> bytes:
    return zenoh_wire.frame(sn, bytes(edge_message(key, payload)))


def minimum_frame(sn: int) -> bytes:
    return zenoh_wire.frame(sn, bytes(edge_message(MINIMUM_KEY, MINIMUM_PAYLOAD)))


# A peer may flush a complete Put as one reliable FRAGMENT with the more flag clear, so a run can be a
# single body. Every other run has two or more, and a receiver that insists the first fragment says
# "more" follows would fail only here. The next expected sequence number advances by one, also at the
# wrap.
SINGLE_SNS = (1000, 0xFFFFFFFF)
SINGLE_PAYLOAD = 100


@lru_cache(maxsize=None)
def single_lines() -> tuple[str, ...]:
    message = full_key_message(SINGLE_PAYLOAD)
    out = []
    for sn in SINGLE_SNS:
        bodies = zenoh_fragment.reference_split(message, 2048, sn)
        if len(bodies) != 1 or bodies[0][0] & zenoh_fragment.FLAG_M:
            raise LinksError("a complete Put must be one fragment with the more flag clear")
        if zenoh_fragment.reassemble(bodies, sn, 2048) != message:
            raise LinksError("the host reference does not reassemble a single final fragment")
        after = zenoh_fragment.next_sn(sn, 1)
        out.append(f"links single {sn} {len(bodies[0])} {sha(bodies[0])} {after} {sha(message)}")
    return tuple(out)


# An admitted long key is also declared as a key-expression alias and addressed through it, because the
# contract budgets every key slot at `keyBytes`, not only the subscriber's. (name, batch, key length,
# alias id): the alias id has bit 15 set at the larger batch.
ALIAS_EDGES = (
    ("edge-alias-long-key-2028", 2048, 2028, 1),
    ("edge-alias-long-key-4076", 4096, 4076, 0x8000),
)
ALIAS_PAYLOAD = 5000
# The declaration and the Put that uses the alias share one reliable channel, and a session that accepts
# a FRAME expects the next sequence number, so the declaration is at 4294967294 and the fragments start
# at the next, 4294967295, and run across the wrap to 0 and 1.
ALIAS_SN = 0xFFFFFFFE


def declare_keyexpr(alias: int, key: str) -> bytes:
    """DECLARE of a key-expression alias in the form that names its key in full: the DECLARE id, the
    DeclareKeyExpr id with the named flag, the alias id, scope 0 and the key behind its length."""
    return (
        bytes([zenoh_wire.ID_DECLARE, zenoh_wire.FLAG_N])
        + zenoh_wire.leb128(alias)
        + zenoh_wire.leb128(0)
        + zenoh_wire.prefixed(key.encode())
    )


@lru_cache(maxsize=None)
def alias_long_lines() -> tuple[str, ...]:
    """What a link must report for a long key declared as an alias and addressed by a sender-mapped
    fragmented Put. The encoder is first shown to write the bytes the stock peer wrote."""
    demo, large = pinned_configs()[:2]
    capture = bytes.fromhex(dict(zenoh_stock_peer.CONNECTS)["connects-04-declare-keyexpr-declare-subscriber"])
    at = capture.index(bytes([0x9E, 0x21, 0x08]))
    mine = declare_keyexpr(1, key())
    if capture[at + 3 : at + 3 + len(mine) - 1] != mine[1:]:
        raise LinksError("the DeclareKeyExpr encoder does not write the bytes the stock peer wrote")
    out = []
    for name, batch, length, alias in ALIAS_EDGES:
        key_text = "k" * length
        if not contract_accepts(limits_record([demo, large, edge_config(batch, key_text, ALIAS_PAYLOAD)])):
            raise LinksError(f"the limits contract refuses the configuration of {name}")
        declaration = zenoh_wire.frame(ALIAS_SN, declare_keyexpr(alias, key_text))
        first_fragment = zenoh_fragment.next_sn(ALIAS_SN, 1)
        if len(declaration) + zenoh_fragment.STREAM_LENGTH_BYTES > batch:
            raise LinksError(f"{name}: the declaration does not fit its batch")
        message = zenoh_fragment.message(ALIAS_PAYLOAD, 0x5D, alias)
        digests: list[str] = []
        joined = zenoh_fragment.reassemble(list(tapped_bodies(message, batch, digests, first_fragment)), first_fragment, batch)
        after = zenoh_fragment.next_sn(first_fragment, len(digests))
        decoded = zenoh_fragment.decode_message(joined, alias=alias, alias_key=key_text, subscribed=key_text)
        if joined != message or decoded["scope"] != alias or bytes(decoded["payload"]) != zenoh_fragment.payload_bytes(ALIAS_PAYLOAD):
            raise LinksError(f"{name}: the host reference does not resolve its own Put through the alias")
        out.append(
            f"links aliaslong {name} {batch} {length} {alias} {len(declaration)} {sha(declaration)} "
            f"{len(digests)} {sha(chr(10).join(digests).encode())} {sha(message)} {ALIAS_SN} {first_fragment} {after}"
        )
    return tuple(out)


# A sender may flush early: a fragment carries at most `batch - 2` bytes, not exactly that. This run's
# interior fragments are 1000, 1 and 1500 bytes of slice, so a receiver that insists every fragment
# marked "more" fills the batch fails. (Every stock capture and every reference-split run fills them.)
UNDERFILLED_BATCH = 2048
UNDERFILLED_LEAD = (1000, 1, 1500)


@lru_cache(maxsize=None)
def underfilled_lines() -> tuple[str, ...]:
    message = full_key_message(FULL_KEY_PAYLOAD)
    bodies = zenoh_fragment.reference_split_uneven(message, UNDERFILLED_BATCH, FULL_KEY_SN, UNDERFILLED_LEAD)
    if zenoh_fragment.reassemble(bodies, FULL_KEY_SN, UNDERFILLED_BATCH) != message:
        raise LinksError("the host reference does not reassemble an underfilled run")
    full = len(bodies[0]) == UNDERFILLED_BATCH - zenoh_fragment.STREAM_LENGTH_BYTES
    if full or any(len(b) >= UNDERFILLED_BATCH - zenoh_fragment.STREAM_LENGTH_BYTES for b in bodies[:-1][:len(UNDERFILLED_LEAD)]):
        raise LinksError("an underfilled run must have interior fragments shorter than the maximum")
    lengths = ",".join(str(len(b)) for b in bodies)
    digest = sha("\n".join(sha(b) for b in bodies).encode())
    return (f"links underfilled {UNDERFILLED_BATCH} {len(bodies)} {lengths} {digest} {sha(message)}",)


# A configuration with more than four of each resource, filled to its limits: nine keys and nine
# subscribers declared, and the notice queue of nine outstanding samples (two notices each) held, each
# a distinct payload, with one more message in progress. A link whose tables or queue are sized for
# four cannot build it.
COUNTS = (9, 9, 9, 1024)


def counts_config(mode: str = "heap") -> dict:
    keys, subscribers, samples, payload = COUNTS
    c = dict(
        name="counts", mode=mode, batchBytes=2048, payloadBytes=payload, keyBytes=129,
        attachmentBytes=33, maxKeys=keys, maxSubscribers=subscribers, outstandingSamples=samples,
        stackBytes=0, reassemblyTimeoutMs=5000 if mode == "heap" else 0, heapBytes=0,
    )
    c["stackBytes"] = required_stack(c)
    c["heapBytes"] = heap_needed(c) if mode == "heap" else 0
    return c


@lru_cache(maxsize=None)
def counts_lines() -> tuple[str, ...]:
    demo, large = pinned_configs()[:2]
    c = counts_config()
    if not contract_accepts(limits_record([demo, large, c])):
        raise LinksError("the limits contract refuses the nine-slot configuration")
    held = NOTICES * c["outstandingSamples"]
    digest = sha("\n".join(sha(distinct_payload(c["payloadBytes"], i)) for i in range(held + 1)).encode())
    return (
        f"links counts {c['maxKeys']} {c['maxSubscribers']} {c['outstandingSamples']} {held} {c['payloadBytes']} {c['heapBytes']} {digest}",
    )


# The same counts in fixed mode, where nothing is reassembled and each queued notice carries a copy of its
# payload inline: nine keys and nine subscribers declared and the eighteen notices of nine samples held
# undrained, each a distinct payload, so a link that keeps four-entry arrays for a fixed configuration
# cannot build it. There is no message in progress and no heap.
@lru_cache(maxsize=None)
def counts_fixed_lines() -> tuple[str, ...]:
    demo, large = pinned_configs()[:2]
    c = counts_config("fixed")
    if not contract_accepts(limits_record([demo, large, c])):
        raise LinksError("the limits contract refuses the nine-slot fixed configuration")
    held = NOTICES * c["outstandingSamples"]
    digest = sha("\n".join(sha(distinct_payload(c["payloadBytes"], i)) for i in range(held)).encode())
    return (
        f"links countsfixed {c['maxKeys']} {c['maxSubscribers']} {c['outstandingSamples']} {held} {c['payloadBytes']} {c['stackBytes']} {digest}",
    )


# Counts that cross the widths a count could be stored in: 256 keys, subscribers and samples, one past
# what a byte holds, and 65535 keys and subscribers, the most the contract admits and all a 16-bit field
# holds, each in both modes. A link that keeps a count in a byte or rejects its 256th slot fails the
# first, one that narrows a slot index below 16 bits the second. (scenario, mode, keys, subscribers,
# samples, payload). Each held notice carries a distinct payload that includes its index, which the
# 251-byte period of `distinct_payload` would repeat across 512 of them.
WIDE = (
    ("wide-counts-256-heap", "heap", 256, 256, 256, 12),
    ("wide-counts-256-fixed", "fixed", 256, 256, 256, 12),
    ("wide-counts-65535-heap", "heap", 65535, 65535, 1, 12),
    ("wide-counts-65535-fixed", "fixed", 65535, 65535, 1, 12),
)


def wide_config(mode: str, keys: int, subscribers: int, samples: int, payload: int) -> dict:
    c = dict(
        name="wide", mode=mode, batchBytes=2048, payloadBytes=payload, keyBytes=129,
        attachmentBytes=33, maxKeys=keys, maxSubscribers=subscribers, outstandingSamples=samples,
        stackBytes=0, reassemblyTimeoutMs=5000 if mode == "heap" else 0, heapBytes=0,
    )
    c["stackBytes"] = required_stack(c)
    c["heapBytes"] = heap_needed(c) if mode == "heap" else 0
    return c


def wide_payload(size: int, index: int) -> bytes:
    return (index.to_bytes(4, "big") + distinct_payload(size, index))[:size]


@lru_cache(maxsize=None)
def wide_lines() -> tuple[str, ...]:
    demo, large = pinned_configs()[:2]
    out = []
    for _name, mode, keys, subscribers, samples, payload in WIDE:
        c = wide_config(mode, keys, subscribers, samples, payload)
        if not contract_accepts(limits_record([demo, large, c])):
            raise LinksError(f"the limits contract refuses the {mode} configuration of {keys} keys, {subscribers} subscribers and {samples} samples")
        held = NOTICES * samples
        count = held + (1 if mode == "heap" else 0)
        digest = sha("\n".join(sha(wide_payload(payload, i)) for i in range(count)).encode())
        out.append(f"links wide {mode} {keys} {subscribers} {samples} {held} {payload} {c['stackBytes']} {c['heapBytes']} {digest}")
    return tuple(out)


# Sequence numbers either side of each step in the width of the encoded number. The largest payload
# that travels whole is one byte smaller at the larger width, so a link that sizes the frame from a
# cached width puts a whole batch one byte over the announced one.
BOUNDARY_SNS = (127, 128, 16383, 16384, 2097151, 2097152, 268435455, 268435456)


@lru_cache(maxsize=None)
def boundarysn_lines() -> tuple[str, ...]:
    out = []
    for sn in BOUNDARY_SNS:
        edge = whole_edge(EDGE_BATCH, sn)
        out.append(f"links boundarysn {sn} {edge} whole 1")
        out.append(f"links boundarysn {sn} {edge + 1} fragmented 2")
    for narrow, wide in ((127, 128), (16383, 16384), (2097151, 2097152), (268435455, 268435456)):
        if whole_edge(EDGE_BATCH, narrow) != whole_edge(EDGE_BATCH, wide) + 1:
            raise LinksError(f"the whole-message edge does not move by a byte between sequence numbers {narrow} and {wide}")
    return tuple(out)


# `keyBytes` is a maximum, so a valid key shorter than it reassembles, and an alias id is 16 bits, so
# an id with bit 15 set is declared and a sender-mapped Put resolves through it.
SHORT_KEY = "a/b"
HIGH_ALIASES = (0x8000, 0xFFFF)


# A key is a length in bytes: "\u00e9/\u65e5\u672c" is four characters and nine bytes, and its twin with an
# `x` is five characters and ten. A link that counts characters, or assumes ASCII, takes the twin under a
# declared 9 or mis-sizes the first.
MULTIBYTE_KEY = "\u00e9/\u65e5\u672c"
MULTIBYTE_OVER = MULTIBYTE_KEY + "x"
MULTIBYTE_BATCH = 512
MULTIBYTE_PAYLOAD = 1500


@lru_cache(maxsize=None)
def multibyte_lines() -> tuple[str, ...]:
    demo, large = pinned_configs()[:2]
    key = MULTIBYTE_KEY
    key_bytes = len(key.encode())
    if len(key) == key_bytes or len(MULTIBYTE_OVER) > key_bytes or len(MULTIBYTE_OVER.encode()) <= key_bytes:
        raise LinksError("the multibyte key must differ in characters and bytes, and its twin must pass by characters only")
    if not contract_accepts(limits_record([demo, large, edge_config(MULTIBYTE_BATCH, "k" * key_bytes, MULTIBYTE_PAYLOAD)])):
        raise LinksError("the limits contract refuses the multibyte-key configuration")
    _handshake, declare, undeclare = edge_sizes("multibyte", MULTIBYTE_BATCH, key)
    message = edge_message(key, MULTIBYTE_PAYLOAD)
    digests: list[str] = []
    joined = zenoh_fragment.reassemble(list(tapped_bodies(message, MULTIBYTE_BATCH, digests)), EDGE_SN, MULTIBYTE_BATCH)
    decoded = zenoh_fragment.decode_message(joined, max_key=key_bytes, subscribed=key)
    if bytes(decoded["key"]) != key.encode() or bytes(decoded["payload"]) != zenoh_fragment.payload_bytes(MULTIBYTE_PAYLOAD):
        raise LinksError("the host reference does not read its own multibyte Put back")
    return (
        f"links multibyte {len(key)} {key_bytes} {MULTIBYTE_BATCH} {MULTIBYTE_PAYLOAD} "
        f"{declare} {sha(zenoh_wire.declare_subscriber(1, key, True))} {undeclare} {sha(zenoh_wire.undeclare_subscriber(1, key, True))} "
        f"{len(digests)} {sha(chr(10).join(digests).encode())} {sha(message)}",
    )


def short_key_line() -> str:
    return f"links shortkey {len(SHORT_KEY)} 5000 {sha(zenoh_fragment.payload_bytes(5000))}"


def high_alias_line(alias: int) -> str:
    return f"links highalias {alias} 5000 {sha(zenoh_fragment.payload_bytes(5000))}"


def subscriber_id_line(sub_id: int) -> str:
    declare = zenoh_wire.declare_subscriber(sub_id, key(), True)
    undeclare = zenoh_wire.undeclare_subscriber(sub_id, key(), True)
    return f"links subid {sub_id} {len(declare)} {sha(declare)} {len(undeclare)} {sha(undeclare)}"


def deadline_line(timeout: int, first: int, middle: int, last: int, outcome: str, digest: str) -> str:
    return f"links deadline {timeout} {first} {middle} {last} {outcome} {digest}"


# The contract accepts any positive timeout, so the link is probed at three small ones, the pinned
# 5000 ms, a shorter 250 ms and the shortest, 1 ms, and at one on each side of the widths a timeout
# could be stored in: 65536 (past 16 bits), 4294967296 (past 32 bits) and 9223372036854775807, the
# largest the contract can state. The session's clock is a 64-bit count of milliseconds, so every one
# of them is a valid timeout, and a link that stores it narrower, saturates it or rejects it expires
# the exact run early. A receiver that hard-codes one value expires too early or too late at the others.
TIMEOUTS = (TIMEOUT_MS, 250, 1, 65536, 1 << 32, (1 << 63) - 1)


@lru_cache(maxsize=None)
def check_timeout_configs() -> None:
    demo, large = pinned_configs()[:2]
    for timeout in TIMEOUTS:
        c = edge_config(4096, "k" * 129, 5000)
        c["reassemblyTimeoutMs"] = timeout
        if not contract_accepts(limits_record([demo, large, c])):
            raise LinksError(f"the limits contract refuses a heap configuration with a {timeout} ms timeout")


def deadline_lines() -> list[str]:
    check_timeout_configs()
    digest = sha(zenoh_fragment.payload_bytes(DEADLINE_PAYLOAD))
    out = []
    for timeout in TIMEOUTS:
        edge = DEADLINE_FIRST + timeout
        out.append(deadline_line(timeout, DEADLINE_FIRST, edge - 1, edge, "reassembled", digest))
        out.append(deadline_line(timeout, DEADLINE_FIRST, edge - 1, edge + 1, "reassembly-timeout", "-"))
    return out


def reuse_capacity() -> int:
    return message_size(REUSE_SIZE, 129)


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


PINNED_CASES = 267
PINNED_CONTROLS = 632


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
        if name.startswith("edge-alias-"):
            lines.append(next(line for line in alias_long_lines() if line.split()[2] == name))
        elif name.startswith("edge-minimum-"):
            lines.append(next(line for line in minimum_lines() if line.split()[2] == name))
        elif name.startswith("edge-fixed-"):
            lines.append(next(line for line in fixed_edge_lines() if line.split()[2] == name))
        elif name.startswith("edge-"):
            lines.append(next(line for line in edge_lines() if line.split()[2] == name))
        if name == "reassemble-single-final-fragment":
            lines += list(single_lines())
        if name == "reassemble-underfilled-interior":
            lines += list(underfilled_lines())
        if name.startswith("wide-counts-"):
            lines.append(next(line for line, spec in zip(wide_lines(), WIDE, strict=True) if spec[0] == name))
        if name == "nine-keys-subscribers-and-samples-fixed":
            lines += list(counts_fixed_lines())
        if name == "reassemble-multibyte-key":
            lines += list(multibyte_lines())
        if name == "nine-keys-subscribers-and-samples":
            lines += list(counts_lines())
        if name == "boundary-at-sequence-width-changes":
            lines += list(boundarysn_lines())
        if name == "reassemble-short-key":
            lines.append(short_key_line())
        if name == "reassemble-high-aliases":
            lines += [high_alias_line(alias) for alias in HIGH_ALIASES]
        if name == "subscriber-ids-use-all-32-bits":
            lines += [subscriber_id_line(sub_id) for sub_id in SUBSCRIBER_IDS]
        if name == "reassembly-completes-at-the-timeout-edge":
            lines += deadline_lines()
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
    ("links", "deadline"): 8,
    ("links", "subid"): 7,
    ("links", "shortkey"): 5,
    ("links", "edge"): 15,
    ("links", "fixededge"): 15,
    ("links", "minimum"): 16,
    ("links", "aliaslong"): 14,
    ("links", "countsfixed"): 9,
    ("links", "wide"): 11,
    ("links", "multibyte"): 13,
    ("links", "single"): 7,
    ("links", "counts"): 9,
    ("links", "underfilled"): 7,
    ("links", "boundarysn"): 6,
    ("links", "highalias"): 5,
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

    wide = [" ".join(f) for f in only("wide")]
    if wide != list(wide_lines()):
        raise LinksError(f"configurations of 256 and of 65535 slots must be built and filled in each mode: {list(wide_lines())}, reported {wide}")

    countsfixed = [" ".join(f) for f in only("countsfixed")]
    if countsfixed != list(counts_fixed_lines()):
        raise LinksError(f"a fixed configuration of nine keys, subscribers and samples must be built and filled: {list(counts_fixed_lines())}, reported {countsfixed}")

    multibyte = [" ".join(f) for f in only("multibyte")]
    if multibyte != list(multibyte_lines()):
        raise LinksError(f"a valid multibyte key must be declared and carried, its length counted in bytes: {list(multibyte_lines())}, reported {multibyte}")

    aliaslong = [" ".join(f) for f in only("aliaslong")]
    if aliaslong != list(alias_long_lines()):
        raise LinksError(f"a long key must be declared as an alias and resolve a sender-mapped fragmented Put: {list(alias_long_lines())}, reported {aliaslong}")

    single = [" ".join(f) for f in only("single")]
    if single != list(single_lines()):
        raise LinksError(f"a complete Put in a single final fragment must reassemble and advance the sequence by one: {list(single_lines())}, reported {single}")

    minimum = [" ".join(f) for f in only("minimum")]
    if minimum != list(minimum_lines()):
        raise LinksError(f"the smallest configuration the contract admits must open, declare and send in each mode: {list(minimum_lines())}, reported {minimum}")

    fixededges = [" ".join(f) for f in only("fixededge")]
    if fixededges != list(fixed_edge_lines()):
        raise LinksError(f"the admitted edges of a fixed link must open, declare and send a whole Put through the link: {list(fixed_edge_lines())}, reported {fixededges}")

    edges = [" ".join(f) for f in only("edge")]
    if edges != list(edge_lines()):
        raise LinksError(f"the admitted edges must open, declare, send and reassemble through the link: {list(edge_lines())}, reported {edges}")

    underfilled = [" ".join(f) for f in only("underfilled")]
    if underfilled != list(underfilled_lines()):
        raise LinksError(f"a run whose interior fragments are shorter than the maximum must reassemble: {list(underfilled_lines())}, reported {underfilled}")

    counts = [" ".join(f) for f in only("counts")]
    if counts != list(counts_lines()):
        raise LinksError(f"a configuration of nine keys, subscribers and samples must be built and filled: {list(counts_lines())}, reported {counts}")

    boundarysn = [" ".join(f) for f in only("boundarysn")]
    if boundarysn != list(boundarysn_lines()):
        raise LinksError(f"the edge between a whole and a fragmented message must hold at each sequence-number width: {list(boundarysn_lines())}, reported {boundarysn}")

    shortkey = [" ".join(f) for f in only("shortkey")]
    if shortkey != [short_key_line()]:
        raise LinksError(f"a fragmented Put for the valid {len(SHORT_KEY)}-byte key {SHORT_KEY!r} must reassemble: {short_key_line()}, reported {shortkey}")

    highalias = [" ".join(f) for f in only("highalias")]
    if highalias != [high_alias_line(alias) for alias in HIGH_ALIASES]:
        raise LinksError(f"aliases {[hex(a) for a in HIGH_ALIASES]} must be declared and resolve a sender-mapped fragmented Put: {[high_alias_line(a) for a in HIGH_ALIASES]}, reported {highalias}")

    subid = [" ".join(f) for f in only("subid")]
    if subid != [subscriber_id_line(sub_id) for sub_id in SUBSCRIBER_IDS]:
        raise LinksError(f"subscriber ids {[hex(i) for i in SUBSCRIBER_IDS]} must be declared and undeclared with the pinned encodings: {[subscriber_id_line(i) for i in SUBSCRIBER_IDS]}, reported {subid}")

    deadline = [" ".join(f) for f in only("deadline")]
    if deadline != deadline_lines():
        raise LinksError(f"a run whose last fragment arrives exactly the declared timeout after the first, which started after the clock had passed it, must reassemble and the same run a millisecond later must not: {deadline_lines()}, reported {deadline}")
    for line in deadline:
        _, _, timeout, first, middle, last, outcome, _digest = line.split()
        if not int(first) <= int(middle) <= int(last):
            raise LinksError("the fragments of a deadline run arrive out of order")
        try:
            zenoh_fragment.check_deadline(int(first), int(last), int(timeout))
        except zenoh_fragment.FragmentError:
            if outcome == "reassembled":
                raise LinksError("a run reported as reassembled outlived its declared timeout") from None
        else:
            if outcome != "reassembled":
                raise LinksError("a run within its declared timeout was reported as refused")

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
    positive = len(configs) + len(done) + len(sent) + len(reassembled) + len(fullkey) + len(boundary) + len(advance) + len(reuse) + len(payload) + len(sizes) + len(queue) + len(aliased) + len(deadline) + len(reuseafter) + len(widthsplit) + len(heapfrag) + len(drained) + len(subid) + len(shortkey) + len(highalias) + len(edges) + len(counts) + len(boundarysn) + len(underfilled) + len(fixededges) + len(minimum) + len(aliaslong) + len(single) + len(countsfixed) + len(multibyte) + len(wide)
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
FRAGMENT_FRAMING, STACK_FACTOR, SLOT, NOTICE, NOTICES, SESSION = 8, 8, 16, 40, 2, 640
MESSAGE_FIXED, FRAME_FIXED = 5, 6
STREAM = 2


def required_stack(c: dict) -> int:
    slots = (c["maxKeys"] + c["maxSubscribers"]) * (c["keyBytes"] + SLOT)
    payload = c["payloadBytes"] if c["mode"] == "fixed" else 0
    return STACK_FACTOR * c["batchBytes"] + SESSION + slots + NOTICES * c["outstandingSamples"] * (NOTICE + payload)


def leb_size(value: int) -> int:
    return len(zenoh_wire.leb128(value))


def message_size(payload: int, key: int, attachment: int = 33) -> int:
    """The serialized full-key Put: its parts, five fixed bytes and the widths of the two lengths."""
    return payload + key + attachment + MESSAGE_FIXED + leb_size(key) + leb_size(payload)


def fixed_need(payload: int, key: int) -> int:
    """What a fixed link needs of its batch: the message, a frame header and a five-byte sequence
    number, and the two-byte stream length."""
    return message_size(payload, key) + FRAME_FIXED + STREAM


def fixed_max_payload(batch: int, key: int) -> int:
    payload = batch
    while fixed_need(payload, key) > batch:
        payload -= 1
    return payload


def message_fills(batch: int, key: int) -> int:
    """The largest payload whose message alone fits `batch`."""
    payload = batch
    while message_size(payload, key) > batch:
        payload -= 1
    return payload


def message_bytes(c: dict) -> int:
    return message_size(c["payloadBytes"], c["keyBytes"], c["attachmentBytes"])


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
        ("a fixed payload whose frame fills the batch exactly", trio(large=exact(large, payloadBytes=fixed_max_payload(2048, 129))), True),
        ("a fixed payload one byte over what the frame holds", trio(large=exact(large, payloadBytes=fixed_max_payload(2048, 129) + 1)), False),
        ("a fixed payload whose message fills the batch, the frame and stream length not reserved", trio(large=exact(large, payloadBytes=message_fills(2048, 129))), False),
        ("a fixed payload whose message and stream length fill the batch, the frame not reserved", trio(large=exact(large, payloadBytes=message_fills(2048 - STREAM, 129))), False),
        ("a fixed payload of 127 bytes, whose length is one byte, in the batch that holds it exactly", trio(large=exact(large, batchBytes=fixed_need(127, 129), payloadBytes=127)), True),
        ("a fixed payload of 128 bytes in a batch one byte short of it", trio(large=exact(large, batchBytes=fixed_need(128, 129) - 1, payloadBytes=128)), False),
        ("a fixed payload of 128 bytes, whose length takes two bytes, in the batch that holds it exactly", trio(large=exact(large, batchBytes=fixed_need(128, 129), payloadBytes=128)), True),
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
        ("a heap batch one byte under the largest handshake message", trio(heap=exact(heap, batchBytes=88, keyBytes=1, heapBytes=heap_needed(dict(heap, keyBytes=1)))), False),
        ("a heap batch that holds the largest handshake message exactly", trio(heap=exact(heap, batchBytes=89, keyBytes=1, heapBytes=heap_needed(dict(heap, keyBytes=1)))), True),
        ("a fixed batch one byte under the largest handshake message", trio(large=exact(large, batchBytes=88, payloadBytes=1, keyBytes=1)), False),
        ("a fixed batch that holds the largest handshake message exactly", trio(large=exact(large, batchBytes=89, payloadBytes=1, keyBytes=1)), True),
        ("an attachment of 33 bytes, the wire size", trio(large=exact(large, attachmentBytes=33)), True),
        ("an attachment of 32 bytes, one byte short", trio(large=exact(large, attachmentBytes=32)), False),
        ("an attachment of 34 bytes, one byte over", trio(large=exact(large, attachmentBytes=34)), False),
        ("an attachment of 1 byte", trio(large=exact(large, attachmentBytes=1)), False),
        ("a heap link certifying storage for a 1-byte attachment", trio(heap=exact(heap, attachmentBytes=1, heapBytes=heap_needed(dict(heap, attachmentBytes=1)))), False),
        ("a 65535-byte key, more than any batch carries a declaration for", trio(heap=exact(heap, keyBytes=65535, heapBytes=heap_needed(dict(heap, keyBytes=65535)))), False),
        ("a key that fits a declaration exactly in the largest batch", trio(heap=exact(heap, batchBytes=4096, keyBytes=4076, heapBytes=heap_needed(dict(heap, keyBytes=4076)))), True),
        ("a key one byte over what a declaration fits in the largest batch", trio(heap=exact(heap, batchBytes=4096, keyBytes=4077, heapBytes=heap_needed(dict(heap, keyBytes=4077)))), False),
        ("a key that fits a declaration exactly in a batch of 2048", trio(heap=exact(heap, keyBytes=2028, heapBytes=heap_needed(dict(heap, keyBytes=2028)))), True),
        ("a key one byte over what a declaration fits in a batch of 2048", trio(heap=exact(heap, keyBytes=2029, heapBytes=heap_needed(dict(heap, keyBytes=2029)))), False),
        ("a key of 125 bytes, the last whose declaration length is one byte, in a batch of 144", trio(heap=exact(heap, batchBytes=144, keyBytes=125, heapBytes=heap_needed(dict(heap, keyBytes=125)))), True),
        ("a key of 125 bytes in a batch of 143", trio(heap=exact(heap, batchBytes=143, keyBytes=125, heapBytes=heap_needed(dict(heap, keyBytes=125)))), False),
        ("a key of 126 bytes, whose declaration length takes two bytes, in a batch of 145", trio(heap=exact(heap, batchBytes=145, keyBytes=126, heapBytes=heap_needed(dict(heap, keyBytes=126)))), False),
        ("a key of 126 bytes in a batch of 146", trio(heap=exact(heap, batchBytes=146, keyBytes=126, heapBytes=heap_needed(dict(heap, keyBytes=126)))), True),
        ("the smallest batch with the longest key it holds a declaration for", trio(heap=exact(heap, batchBytes=89, keyBytes=70, heapBytes=heap_needed(dict(heap, keyBytes=70)))), True),
        ("the smallest batch with a key a byte too long", trio(heap=exact(heap, batchBytes=89, keyBytes=71, heapBytes=heap_needed(dict(heap, keyBytes=71)))), False),
        ("a 65536-byte key with the stack and heap it needs", trio(heap=exact(heap, keyBytes=65536, heapBytes=heap_needed(dict(heap, keyBytes=65536)))), False),
        ("a payload of 4294967295 bytes with the stack and heap it needs", trio(heap=exact(heap, payloadBytes=4294967295, heapBytes=heap_needed(dict(heap, payloadBytes=4294967295)))), True),
        ("a payload of 4294967296 bytes with the stack and heap it needs", trio(heap=exact(heap, payloadBytes=4294967296, heapBytes=heap_needed(dict(heap, payloadBytes=4294967296)))), False),
        ("a heap link at the largest sample count the contract's arithmetic holds", trio(heap=exact(heap, outstandingSamples=10**9, heapBytes=heap_needed(dict(heap, outstandingSamples=10**9)))), True),
        ("a heap link one sample past it", trio(heap=exact(heap, outstandingSamples=10**9 + 1, heapBytes=heap_needed(dict(heap, outstandingSamples=10**9 + 1)))), False),
        ("a fixed link at the largest sample count", trio(large=exact(large, outstandingSamples=10**9)), True),
        ("a fixed link one sample past it", trio(large=exact(large, outstandingSamples=10**9 + 1)), False),
        ("a sample count of 2^62 with the largest storage the contract can hold", trio(heap=dict(heap, outstandingSamples=2**62, stackBytes=2**63 - 1, heapBytes=2**63 - 1)), False),
        ("the largest integer as a sample count", trio(heap=dict(heap, outstandingSamples=2**63 - 1, stackBytes=2**63 - 1, heapBytes=2**63 - 1)), False),
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

    def minimum_swap(name: str, index: int, value: str) -> list[str]:
        at = first(f"links minimum {name} ")
        fields = good[at].split()
        fields[index] = value
        return good[:at] + [" ".join(fields)] + good[at + 1 :]

    def fixed_swap(name: str, index: int, value: str) -> list[str]:
        at = first(f"links fixededge {name} ")
        fields = good[at].split()
        fields[index] = value
        return good[:at] + [" ".join(fields)] + good[at + 1 :]

    def under_swap(index: int, value: str) -> list[str]:
        at = first("links underfilled ")
        fields = good[at].split()
        fields[index] = value
        return good[:at] + [" ".join(fields)] + good[at + 1 :]

    def wide_swap(mode: str, keys: int, index: int, value: str) -> list[str]:
        at = first(f"links wide {mode} {keys} ")
        fields = good[at].split()
        fields[index] = value
        return good[:at] + [" ".join(fields)] + good[at + 1 :]

    def fixed_counts_swap(index: int, value: str) -> list[str]:
        at = first("links countsfixed ")
        fields = good[at].split()
        fields[index] = value
        return good[:at] + [" ".join(fields)] + good[at + 1 :]

    def multibyte_swap(index: int, value: str) -> list[str]:
        at = first("links multibyte ")
        fields = good[at].split()
        fields[index] = value
        return good[:at] + [" ".join(fields)] + good[at + 1 :]

    def counts_swap(index: int, value: str) -> list[str]:
        at = first("links counts ")
        fields = good[at].split()
        fields[index] = value
        return good[:at] + [" ".join(fields)] + good[at + 1 :]

    def line_set(kind: str, name: str, updates: dict) -> list[str]:
        at = first(f"links {kind} {name} ")
        fields = good[at].split()
        for index, value in updates.items():
            fields[index] = value
        return good[:at] + [" ".join(fields)] + good[at + 1 :]

    def edge_swap(name: str, index: int, value: str) -> list[str]:
        at = first(f"links edge {name} ")
        fields = good[at].split()
        fields[index] = value
        return good[:at] + [" ".join(fields)] + good[at + 1 :]

    def with_deadline(a: str, b: str) -> list[str]:
        out, seen = [], 0
        for line in good:
            if line.startswith("links deadline "):
                out.append((a, b)[seen] if seen < 2 else line)
                seen += 1
            else:
                out.append(line)
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
        ("the 65536 ms deadline runs dropped", [line for line in good if not line.startswith("links deadline 65536 ")]),
        ("the 4294967296 ms deadline runs dropped", [line for line in good if not line.startswith("links deadline 4294967296 ")]),
        ("the 9223372036854775807 ms deadline runs dropped", [line for line in good if not line.startswith("links deadline 9223372036854775807 ")]),
        ("a link that stores 65536 ms in 16 bits expiring the exact run", [deadline_line(65536, DEADLINE_FIRST, DEADLINE_FIRST + 65535, DEADLINE_FIRST + 65536, "reassembly-timeout", "-") if line == deadline_lines()[6] else line for line in good]),
        ("a link that stores 4294967296 ms in 32 bits expiring the exact run", [deadline_line(1 << 32, DEADLINE_FIRST, DEADLINE_FIRST + (1 << 32) - 1, DEADLINE_FIRST + (1 << 32), "reassembly-timeout", "-") if line == deadline_lines()[8] else line for line in good]),
        ("a link that saturates the largest timeout accepting the late run", [deadline_line((1 << 63) - 1, DEADLINE_FIRST, DEADLINE_FIRST + (1 << 63) - 2, DEADLINE_FIRST + (1 << 63), "reassembled", sha(zenoh_fragment.payload_bytes(DEADLINE_PAYLOAD))) if line == deadline_lines()[11] else line for line in good]),
        ("a link that stores 65536 ms in 16 bits accepting the run at 1 ms past zero", [deadline_line(65536, DEADLINE_FIRST, DEADLINE_FIRST, DEADLINE_FIRST + 1, "reassembled", sha(zenoh_fragment.payload_bytes(DEADLINE_PAYLOAD))) if line == deadline_lines()[7] else line for line in good]),
        ("the 250 ms deadline runs dropped", [line for line in good if not line.startswith("links deadline 250 ")]),
        ("the 1 ms deadline runs dropped", [line for line in good if not line.startswith("links deadline 1 ")]),
        ("a link that hard-codes 5000 ms accepting the late 250 ms run", [deadline_line(250, DEADLINE_FIRST, DEADLINE_FIRST + 249, DEADLINE_FIRST + 251, "reassembled", sha(zenoh_fragment.payload_bytes(DEADLINE_PAYLOAD))) if line == deadline_lines()[3] else line for line in good]),
        ("a link that expires the exact 250 ms run early", [deadline_line(250, DEADLINE_FIRST, DEADLINE_FIRST + 249, DEADLINE_FIRST + 250, "reassembly-timeout", "-") if line == deadline_lines()[2] else line for line in good]),
        ("a link that hard-codes 5000 ms accepting the late 1 ms run", [deadline_line(1, DEADLINE_FIRST, DEADLINE_FIRST, DEADLINE_FIRST + 2, "reassembled", sha(zenoh_fragment.payload_bytes(DEADLINE_PAYLOAD))) if line == deadline_lines()[5] else line for line in good]),
        ("a link that expires the exact 1 ms run early", [deadline_line(1, DEADLINE_FIRST, DEADLINE_FIRST, DEADLINE_FIRST + 1, "reassembly-timeout", "-") if line == deadline_lines()[4] else line for line in good]),
        ("the late deadline run dropped", [line for line in good if line != deadline_lines()[1]]),
        ("the deadline run at the edge dropped", [line for line in good if line != deadline_lines()[0]]),
        ("a deadline measured from time zero", with_deadline(deadline_line(TIMEOUT_MS, 0, TIMEOUT_MS - 1, TIMEOUT_MS, "reassembled", sha(zenoh_fragment.payload_bytes(DEADLINE_PAYLOAD))), deadline_lines()[1])),
        ("a late run accepted by a deadline that each continuation refreshes", with_deadline(deadline_lines()[0], deadline_line(TIMEOUT_MS, DEADLINE_FIRST, DEADLINE_FIRST + TIMEOUT_MS - 1, DEADLINE_FIRST + TIMEOUT_MS + 1, "reassembled", sha(zenoh_fragment.payload_bytes(DEADLINE_PAYLOAD))))),
        ("a run refused at the edge", with_deadline(deadline_line(TIMEOUT_MS, DEADLINE_FIRST, DEADLINE_FIRST + TIMEOUT_MS - 1, DEADLINE_FIRST + TIMEOUT_MS, "reassembly-timeout", "-"), deadline_lines()[1])),
        ("a deadline run with a timeout of one millisecond", with_deadline(deadline_line(1, DEADLINE_FIRST, DEADLINE_FIRST, DEADLINE_FIRST + 1, "reassembled", sha(zenoh_fragment.payload_bytes(DEADLINE_PAYLOAD))), deadline_lines()[1])),
        ("a deadline run that reassembled to another payload", with_deadline(deadline_lines()[0].rsplit(" ", 1)[0] + " " + sha(b"other"), deadline_lines()[1])),
        ("a deadline run of another timeout", with_deadline(deadline_line(TIMEOUT_MS + 1, DEADLINE_FIRST, DEADLINE_FIRST + TIMEOUT_MS, DEADLINE_FIRST + TIMEOUT_MS + 1, "reassembled", sha(zenoh_fragment.payload_bytes(DEADLINE_PAYLOAD))), deadline_lines()[1])),
        ("a deadline run whose fragments arrive out of order", with_deadline(deadline_line(TIMEOUT_MS, DEADLINE_FIRST, DEADLINE_FIRST + TIMEOUT_MS + 1, DEADLINE_FIRST + TIMEOUT_MS, "reassembled", sha(zenoh_fragment.payload_bytes(DEADLINE_PAYLOAD))), deadline_lines()[1])),
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
        ("fragment-sequence-duplicate admitted instead of refused", drop("links refused fragment-sequence-duplicate ")),
        ("fragment-sequence-duplicate refused with the wrong class", swap("links refused fragment-sequence-duplicate ", "links refused fragment-sequence-duplicate over-bound")),
        ("fragment-sequence-backward admitted instead of refused", drop("links refused fragment-sequence-backward ")),
        ("fragment-sequence-backward refused with the wrong class", swap("links refused fragment-sequence-backward ", "links refused fragment-sequence-backward over-bound")),
        ("fragmented-put-full-key-nonzero-scope admitted instead of refused", drop("links refused fragmented-put-full-key-nonzero-scope ")),
        ("fragmented-put-full-key-nonzero-scope refused with the wrong class", swap("links refused fragmented-put-full-key-nonzero-scope ", "links refused fragmented-put-full-key-nonzero-scope over-bound")),
        ("fragmented-push-extension-flag admitted instead of refused", drop("links refused fragmented-push-extension-flag ")),
        ("fragmented-push-extension-flag refused with the wrong class", swap("links refused fragmented-push-extension-flag ", "links refused fragmented-push-extension-flag over-bound")),
        ("fragmented-put-timestamp-flag admitted instead of refused", drop("links refused fragmented-put-timestamp-flag ")),
        ("fragmented-put-timestamp-flag refused with the wrong class", swap("links refused fragmented-put-timestamp-flag ", "links refused fragmented-put-timestamp-flag over-bound")),
        ("fragmented-put-encoding-flag admitted instead of refused", drop("links refused fragmented-put-encoding-flag ")),
        ("fragmented-put-encoding-flag refused with the wrong class", swap("links refused fragmented-put-encoding-flag ", "links refused fragmented-put-encoding-flag over-bound")),
        ("fragmented-put-unknown-extension admitted instead of refused", drop("links refused fragmented-put-unknown-extension ")),
        ("fragmented-put-unknown-extension refused with the wrong class", swap("links refused fragmented-put-unknown-extension ", "links refused fragmented-put-unknown-extension over-bound")),
        ("fragmented-put-extension-chained admitted instead of refused", drop("links refused fragmented-put-extension-chained ")),
        ("fragmented-put-extension-chained refused with the wrong class", swap("links refused fragmented-put-extension-chained ", "links refused fragmented-put-extension-chained over-bound")),
        ("fragmented-put-empty-key admitted instead of refused", drop("links refused fragmented-put-empty-key ")),
        ("fragmented-put-empty-key refused with the wrong class", swap("links refused fragmented-put-empty-key ", "links refused fragmented-put-empty-key over-bound")),
        ("fragment-sequence-truncated admitted instead of refused", drop("links refused fragment-sequence-truncated ")),
        ("fragment-sequence-truncated refused with the wrong class", swap("links refused fragment-sequence-truncated ", "links refused fragment-sequence-truncated over-bound")),
        ("fixed-refuses-fragment-sequence-truncated admitted instead of refused", drop("links refused fixed-refuses-fragment-sequence-truncated ")),
        ("fixed-refuses-fragment-sequence-truncated refused with the wrong class", swap("links refused fixed-refuses-fragment-sequence-truncated ", "links refused fixed-refuses-fragment-sequence-truncated over-bound")),
        ("a truncated sequence number refused as malformed-message on a heap link", swap("links refused fragment-sequence-truncated ", "links refused fragment-sequence-truncated malformed-message")),
        ("a truncated sequence number refused as truncated on a fixed link", swap("links refused fixed-refuses-fragment-sequence-truncated ", "links refused fixed-refuses-fragment-sequence-truncated truncated")),
        ("fragmented-push-delete-body admitted instead of refused", drop("links refused fragmented-push-delete-body ")),
        ("fragmented-push-delete-body refused with the wrong class", swap("links refused fragmented-push-delete-body ", "links refused fragmented-push-delete-body over-bound")),
        ("fragmented-push-ends-before-body admitted instead of refused", drop("links refused fragmented-push-ends-before-body ")),
        ("fragmented-push-ends-before-body refused with the wrong class", swap("links refused fragmented-push-ends-before-body ", "links refused fragmented-push-ends-before-body over-bound")),
        ("fragmented-put-scope-unterminated admitted instead of refused", drop("links refused fragmented-put-scope-unterminated ")),
        ("fragmented-put-scope-unterminated refused with the wrong class", swap("links refused fragmented-put-scope-unterminated ", "links refused fragmented-put-scope-unterminated over-bound")),
        ("fragmented-put-key-length-unterminated admitted instead of refused", drop("links refused fragmented-put-key-length-unterminated ")),
        ("fragmented-put-key-length-unterminated refused with the wrong class", swap("links refused fragmented-put-key-length-unterminated ", "links refused fragmented-put-key-length-unterminated over-bound")),
        ("fragmented-put-attachment-length-unterminated admitted instead of refused", drop("links refused fragmented-put-attachment-length-unterminated ")),
        ("fragmented-put-attachment-length-unterminated refused with the wrong class", swap("links refused fragmented-put-attachment-length-unterminated ", "links refused fragmented-put-attachment-length-unterminated over-bound")),
        ("fragmented-put-payload-length-unterminated admitted instead of refused", drop("links refused fragmented-put-payload-length-unterminated ")),
        ("fragmented-put-payload-length-unterminated refused with the wrong class", swap("links refused fragmented-put-payload-length-unterminated ", "links refused fragmented-put-payload-length-unterminated over-bound")),
        ("fragmented-put-key-trailing-slash admitted instead of refused", drop("links refused fragmented-put-key-trailing-slash ")),
        ("fragmented-put-key-trailing-slash refused with the wrong class", swap("links refused fragmented-put-key-trailing-slash ", "links refused fragmented-put-key-trailing-slash over-bound")),
        ("fragmented-put-key-question-mark admitted instead of refused", drop("links refused fragmented-put-key-question-mark ")),
        ("fragmented-put-key-question-mark refused with the wrong class", swap("links refused fragmented-put-key-question-mark ", "links refused fragmented-put-key-question-mark over-bound")),
        ("fragmented-put-key-hash admitted instead of refused", drop("links refused fragmented-put-key-hash ")),
        ("fragmented-put-key-hash refused with the wrong class", swap("links refused fragmented-put-key-hash ", "links refused fragmented-put-key-hash over-bound")),
        ("fragmented-put-key-dollar admitted instead of refused", drop("links refused fragmented-put-key-dollar ")),
        ("fragmented-put-key-dollar refused with the wrong class", swap("links refused fragmented-put-key-dollar ", "links refused fragmented-put-key-dollar over-bound")),
        ("fragmented-put-key-truncated admitted instead of refused", drop("links refused fragmented-put-key-truncated ")),
        ("fragmented-put-key-truncated refused with the wrong class", swap("links refused fragmented-put-key-truncated ", "links refused fragmented-put-key-truncated over-bound")),
        ("tenth-subscriber-in-nine admitted instead of refused", drop("links refused tenth-subscriber-in-nine ")),
        ("tenth-subscriber-in-nine refused with the wrong class", swap("links refused tenth-subscriber-in-nine ", "links refused tenth-subscriber-in-nine over-bound")),
        ("tenth-alias-in-nine admitted instead of refused", drop("links refused tenth-alias-in-nine ")),
        ("tenth-alias-in-nine refused with the wrong class", swap("links refused tenth-alias-in-nine ", "links refused tenth-alias-in-nine unsupported-message")),
        ("the nine-slot line dropped", drop("links counts ")),
        ("a link of four keys reported for nine", counts_swap(2, "4")),
        ("a link of four subscribers reported for nine", counts_swap(3, "4")),
        ("a link of four samples reported for nine", counts_swap(4, "4")),
        ("a notice queue of eight reported for eighteen", counts_swap(5, "8")),
        ("a nine-slot queue of small messages", counts_swap(6, "512")),
        ("a nine-slot queue whose payloads were overwritten", counts_swap(8, sha(b"overwritten"))),
        ("the nine-slot scenario dropped", [line for line in good if line != "links ok nine-keys-subscribers-and-samples"]),
        ("the sequence-width boundary lines dropped", drop("links boundarysn ")),
        ("the boundary at 127 dropped", drop("links boundarysn 127 ")),
        ("the boundary at 16383 dropped", drop("links boundarysn 16383 ")),
        ("the boundary at 2097151 dropped", drop("links boundarysn 2097151 ")),
        ("the boundary at 2097152 dropped", drop("links boundarysn 2097152 ")),
        ("the boundary at 268435455 dropped", drop("links boundarysn 268435455 ")),
        ("the boundary at 268435456 dropped", drop("links boundarysn 268435456 ")),
        ("a cached three-byte width at 2097152", swap("links boundarysn 2097152 " + str(whole_edge(EDGE_BATCH, 2097152)) + " whole", f"links boundarysn 2097152 {whole_edge(EDGE_BATCH, 2097151)} whole 1")),
        ("a cached four-byte width at 268435456", swap("links boundarysn 268435456 " + str(whole_edge(EDGE_BATCH, 268435456)) + " whole", f"links boundarysn 268435456 {whole_edge(EDGE_BATCH, 268435455)} whole 1")),
        ("a message one byte over the edge reported whole at 2097152", swap("links boundarysn 2097152 " + str(whole_edge(EDGE_BATCH, 2097152) + 1) + " fragmented", f"links boundarysn 2097152 {whole_edge(EDGE_BATCH, 2097152) + 1} whole 1")),
        ("a message one byte over the edge reported whole at 268435456", swap("links boundarysn 268435456 " + str(whole_edge(EDGE_BATCH, 268435456) + 1) + " fragmented", f"links boundarysn 268435456 {whole_edge(EDGE_BATCH, 268435456) + 1} whole 1")),
        ("a cached one-byte width at 128", swap("links boundarysn 128 " + str(whole_edge(EDGE_BATCH, 128)) + " whole", f"links boundarysn 128 {whole_edge(EDGE_BATCH, 127)} whole 1")),
        ("a cached two-byte width at 16384", swap("links boundarysn 16384 " + str(whole_edge(EDGE_BATCH, 16384)) + " whole", f"links boundarysn 16384 {whole_edge(EDGE_BATCH, 16383)} whole 1")),
        ("a message one byte over the edge reported whole at 128", swap("links boundarysn 128 " + str(whole_edge(EDGE_BATCH, 128) + 1) + " fragmented", f"links boundarysn 128 {whole_edge(EDGE_BATCH, 128) + 1} whole 1")),
        ("the sequence-width scenario dropped", [line for line in good if line != "links ok boundary-at-sequence-width-changes"]),
        ("the underfilled-run line dropped", drop("links underfilled ")),
        ("an underfilled run reported with full fragments", under_swap(4, ",".join(str(len(b)) for b in full_key_bodies()))),
        ("an underfilled run reported with one fragment fewer", under_swap(3, str(int(good[first("links underfilled ")].split()[3]) - 1))),
        ("an underfilled run with other bodies", under_swap(5, sha(b"other"))),
        ("an underfilled run that reassembled to another message", under_swap(6, sha(b"other"))),
        ("an underfilled run at another batch", under_swap(2, "4096")),
        ("the underfilled scenario dropped", [line for line in good if line != "links ok reassemble-underfilled-interior"]),
        ("the 2 MiB edge dropped", drop("links edge edge-payload-2mib ")),
        ("the 256 MiB edge dropped", drop("links edge edge-payload-256mib ")),
        ("the 2 MiB payload reported as 1 MiB", edge_swap("edge-payload-2mib", 5, str(1 << 20))),
        ("the 256 MiB payload reported as 2 MiB", edge_swap("edge-payload-256mib", 5, str(1 << 21))),
        ("the 256 MiB payload reported as 4294967295", edge_swap("edge-payload-256mib", 5, "4294967295")),
        ("a 2 MiB send with another number of fragments", edge_swap("edge-payload-2mib", 9, str(int(good[first("links edge edge-payload-2mib ")].split()[9]) + 1))),
        ("a 256 MiB send with another number of fragments", edge_swap("edge-payload-256mib", 9, str(int(good[first("links edge edge-payload-256mib ")].split()[9]) - 1))),
        ("a 256 MiB send with other bodies", edge_swap("edge-payload-256mib", 10, sha(b"other"))),
        ("a 256 MiB payload that reassembled differently", edge_swap("edge-payload-256mib", 11, sha(b"other"))),
        ("the 1 MiB edge dropped", drop("links edge edge-payload-1mib ")),
        ("the 1 MiB payload reported as 16384", edge_swap("edge-payload-1mib", 5, "16384")),
        ("a 1 MiB send with another number of fragments", edge_swap("edge-payload-1mib", 9, str(int(good[first("links edge edge-payload-1mib ")].split()[9]) - 1))),
        ("a 1 MiB payload that reassembled differently", edge_swap("edge-payload-1mib", 11, sha(b"other"))),
        ("an edge handshake announcing 512 at a batch of 89", edge_swap("edge-batch-89", 12, "512")),
        ("an edge handshake announcing 512 at a batch of 89, its bytes matching", line_set("edge", "edge-batch-89", {12: "512", 13: sha(init_ack_bytes(512))})),
        ("an edge handshake announcing 512 at a batch of 4096, its bytes matching", line_set("edge", "edge-batch-4096", {12: "512", 13: sha(init_ack_bytes(512))})),
        ("an edge handshake announcing the batch with other bytes", edge_swap("edge-batch-89", 13, sha(b"other"))),
        ("an edge handshake that announces the default batch", line_set("edge", "edge-payload-256mib", {12: "65535", 13: sha(init_ack_bytes(65535))})),
        ("a fixed edge handshake announcing 2048 at a batch of 4096, its bytes matching", line_set("fixededge", "edge-fixed-batch-4096", {12: "2048", 13: sha(init_ack_bytes(2048))})),
        ("a fixed edge handshake announcing 512 at a batch of 89", fixed_swap("edge-fixed-batch-89", 12, "512")),
        ("a minimum handshake announcing 512 at a batch of 89, its bytes matching", line_set("minimum", "edge-minimum-fixed", {13: "512", 14: sha(init_ack_bytes(512))})),
        ("a minimum handshake announcing the batch with other bytes", minimum_swap("edge-minimum-heap", 14, sha(b"other"))),
        ("a fixed edge frame built at a two-byte sequence number, batch 4096", line_set("fixededge", "edge-fixed-batch-4096", {9: str(len(fixed_edge_frame(*FIXED_EDGES[1][1:], EDGE_SN))), 10: sha(fixed_edge_frame(*FIXED_EDGES[1][1:], EDGE_SN))})),
        ("a fixed edge frame built at a two-byte sequence number, batch 89", line_set("fixededge", "edge-fixed-batch-89", {9: str(len(fixed_edge_frame(*FIXED_EDGES[0][1:], EDGE_SN))), 10: sha(fixed_edge_frame(*FIXED_EDGES[0][1:], EDGE_SN))})),
        ("a fixed edge frame reported at 4093 bytes", fixed_swap("edge-fixed-batch-4096", 9, "4093")),
        ("a minimum frame built at a two-byte sequence number", line_set("minimum", "edge-minimum-fixed", {10: str(len(minimum_frame(EDGE_SN))), 11: sha(minimum_frame(EDGE_SN))})),
        ("the single-final-fragment lines dropped", drop("links single ")),
        ("the single final fragment at 1000 dropped", drop("links single 1000 ")),
        ("the single final fragment at the wrap dropped", drop("links single 4294967295 ")),
        ("a single final fragment that did not advance the sequence", line_set("single", "1000", {5: "1000"})),
        ("a single final fragment whose sequence did not wrap", line_set("single", "4294967295", {5: "4294967296"})),
        ("a single final fragment that advanced by two", line_set("single", "1000", {5: "1002"})),
        ("a single final fragment taken with the more flag set", line_set("single", "1000", {4: sha(bytes([int(zenoh_fragment.reference_split(full_key_message(SINGLE_PAYLOAD), 2048, 1000)[0][0]) | zenoh_fragment.FLAG_M]) + zenoh_fragment.reference_split(full_key_message(SINGLE_PAYLOAD), 2048, 1000)[0][1:])})),
        ("a single final fragment that reassembled to another message", line_set("single", "1000", {6: sha(b"other")})),
        ("the single-final-fragment scenario dropped", [line for line in good if line != "links ok reassemble-single-final-fragment"]),
        ("the long-key alias lines dropped", drop("links aliaslong ")),
        ("the 2028-byte alias dropped", drop("links aliaslong edge-alias-long-key-2028 ")),
        ("the 4076-byte alias dropped", drop("links aliaslong edge-alias-long-key-4076 ")),
        ("a long-key alias cut to 129 bytes", line_set("aliaslong", "edge-alias-long-key-2028", {4: "129"})),
        ("a 4076-byte alias cut to 129 bytes", line_set("aliaslong", "edge-alias-long-key-4076", {4: "129"})),
        ("alias 0x8000 reported as alias 1", line_set("aliaslong", "edge-alias-long-key-4076", {5: "1"})),
        ("alias 0x8000 stored in 15 bits", line_set("aliaslong", "edge-alias-long-key-4076", {5: "32767"})),
        ("an alias declaration reported one byte short", line_set("aliaslong", "edge-alias-long-key-2028", {6: str(int(alias_long_lines()[0].split()[6]) - 1)})),
        ("an alias declaration with other bytes", line_set("aliaslong", "edge-alias-long-key-4076", {7: sha(b"other")})),
        ("an alias Put with another number of fragments", line_set("aliaslong", "edge-alias-long-key-2028", {8: "9"})),
        ("an alias Put with other fragment bodies", line_set("aliaslong", "edge-alias-long-key-4076", {9: sha(b"other")})),
        ("an alias Put that resolved to another message", line_set("aliaslong", "edge-alias-long-key-2028", {10: sha(b"other")})),
        ("an alias Put whose fragments start at 1000 after a declaration at the wrap", line_set("aliaslong", "edge-alias-long-key-2028", {12: "1000"})),
        ("an alias Put whose fragments restart at the declaration's own number", line_set("aliaslong", "edge-alias-long-key-4076", {12: str(ALIAS_SN)})),
        ("an alias declaration at sequence 999", line_set("aliaslong", "edge-alias-long-key-2028", {11: "999"})),
        ("an alias declaration at the widest sequence number, the fragments left where they were", line_set("aliaslong", "edge-alias-long-key-4076", {11: str(FIXED_SN)})),
        ("an alias run that did not wrap to 0", line_set("aliaslong", "edge-alias-long-key-2028", {13: "4294967298"})),
        ("an alias run that left the expected sequence where it began", line_set("aliaslong", "edge-alias-long-key-4076", {13: str(zenoh_fragment.next_sn(ALIAS_SN, 1))})),
        ("the long-key alias scenarios dropped", [line for line in good if not line.startswith("links ok edge-alias-")]),
        ("a connector announcing 512 at a batch of 89, its INIT_SYN bytes matching", edge_swap("edge-batch-89", 14, sha(init_syn_bytes(512)))),
        ("a connector announcing 512 at a batch of 4096, its INIT_SYN bytes matching", edge_swap("edge-batch-4096", 14, sha(init_syn_bytes(512)))),
        ("a connector announcing the default batch", edge_swap("edge-payload-256mib", 14, sha(init_syn_bytes(65535)))),
        ("a connector whose INIT_SYN is the acceptor's INIT_ACK", edge_swap("edge-batch-89", 14, sha(init_ack_bytes(89)))),
        ("an edge line without its connector INIT_SYN", [" ".join(line.split()[:14]) if line.startswith("links edge edge-batch-89 ") else line for line in good]),
        ("a fixed edge connector announcing 2048 at a batch of 4096", fixed_swap("edge-fixed-batch-4096", 14, sha(init_syn_bytes(2048)))),
        ("a fixed edge connector announcing 512 at a batch of 89", fixed_swap("edge-fixed-batch-89", 14, sha(init_syn_bytes(512)))),
        ("a minimum connector announcing 512 at a batch of 89", minimum_swap("edge-minimum-fixed", 15, sha(init_syn_bytes(512)))),
        ("a minimum connector with other bytes", minimum_swap("edge-minimum-heap", 15, sha(b"other"))),
        ("the minimum-configuration lines dropped", drop("links minimum ")),
        ("the fixed minimum configuration dropped", drop("links minimum edge-minimum-fixed ")),
        ("the heap minimum configuration dropped", drop("links minimum edge-minimum-heap ")),
        ("a fixed minimum link with a three-byte key", minimum_swap("edge-minimum-fixed", 5, "3")),
        ("a heap minimum link with a twelve-byte payload", minimum_swap("edge-minimum-heap", 6, "12")),
        ("a minimum link reported at a batch of 512", minimum_swap("edge-minimum-fixed", 4, "512")),
        ("a minimum link reported in the other mode", minimum_swap("edge-minimum-fixed", 3, "heap")),
        ("a minimum handshake reported one byte long", minimum_swap("edge-minimum-heap", 7, "88")),
        ("a minimum declaration reported one byte short", minimum_swap("edge-minimum-fixed", 8, str(int(good[first("links minimum edge-minimum-fixed ")].split()[8]) - 1))),
        ("a minimum frame with other bytes", minimum_swap("edge-minimum-heap", 11, sha(b"other"))),
        ("a minimum payload that arrived differently", minimum_swap("edge-minimum-fixed", 12, sha(b"other"))),
        ("the minimum-configuration scenarios dropped", [line for line in good if not line.startswith("links ok edge-minimum-")]),
        ("the fixed-edge lines dropped", drop("links fixededge ")),
        ("the fixed edge at the smallest batch dropped", drop("links fixededge edge-fixed-batch-89 ")),
        ("the fixed edge at the largest batch dropped", drop("links fixededge edge-fixed-batch-4096 ")),
        ("a fixed link at 89 reported at 512", fixed_swap("edge-fixed-batch-89", 3, "512")),
        ("a fixed link at 4096 reported at 2048", fixed_swap("edge-fixed-batch-4096", 3, "2048")),
        ("a fixed link at 4096 with the key cut to 129 bytes", fixed_swap("edge-fixed-batch-4096", 4, "129")),
        ("a fixed link at 4096 with the payload cut to 1024", fixed_swap("edge-fixed-batch-4096", 5, "1024")),
        ("a fixed handshake reported one byte long", fixed_swap("edge-fixed-batch-89", 6, "90")),
        ("a fixed declaration reported one byte short", fixed_swap("edge-fixed-batch-4096", 7, str(int(good[first("links fixededge edge-fixed-batch-4096 ")].split()[7]) - 1))),
        ("a fixed frame reported one byte longer than the batch holds", fixed_swap("edge-fixed-batch-4096", 9, "4095")),
        ("a fixed frame with other bytes", fixed_swap("edge-fixed-batch-89", 10, sha(b"other"))),
        ("a fixed payload that arrived differently", fixed_swap("edge-fixed-batch-4096", 11, sha(b"other"))),
        ("the fixed-edge scenarios dropped", [line for line in good if not line.startswith("links ok edge-fixed-")]),
        ("the batch-edge lines dropped", drop("links edge ")),
        ("the smallest-batch edge dropped", drop("links edge edge-batch-89 ")),
        ("the largest-batch edge dropped", drop("links edge edge-batch-4096 ")),
        ("the long-key edge dropped", drop("links edge edge-key-2028 ")),
        ("a handshake reported larger than the smallest batch", edge_swap("edge-batch-89", 6, "90")),
        ("a handshake reported one byte short", edge_swap("edge-batch-89", 6, "88")),
        ("a declaration reported one byte short at the long key", edge_swap("edge-key-2028", 7, str(int(good[first("links edge edge-key-2028 ")].split()[7]) - 1))),
        ("an undeclaration reported one byte long at the largest batch", edge_swap("edge-batch-4096", 8, str(int(good[first("links edge edge-batch-4096 ")].split()[8]) + 1))),
        ("a different fragment count at the largest batch", edge_swap("edge-batch-4096", 9, str(int(good[first("links edge edge-batch-4096 ")].split()[9]) + 1))),
        ("different fragment bodies at the smallest batch", edge_swap("edge-batch-89", 10, sha(b"other"))),
        ("a payload that reassembled differently at the long key", edge_swap("edge-key-2028", 11, sha(b"other"))),
        ("the long key cut to 129 bytes", edge_swap("edge-key-2028", 4, "129")),
        ("the largest batch reported as 2048", edge_swap("edge-batch-4096", 3, "2048")),
        ("the smallest batch reported as 512", edge_swap("edge-batch-89", 3, "512")),
        ("the batch-edge scenarios dropped", [line for line in good if not line.startswith("links ok edge-")]),
        ("the short-key line dropped", drop("links shortkey ")),
        ("a short key reported as the 129-byte demo key", swap("links shortkey ", good[first("links shortkey ")].replace(f" {len(SHORT_KEY)} ", " 129 ", 1))),
        ("a short key that reassembled to another payload", swap("links shortkey ", corrupt_last(good[first("links shortkey ")]))),
        ("the short-key scenario dropped", [line for line in good if line != "links ok reassemble-short-key"]),
        ("the high-alias lines dropped", drop("links highalias ")),
        ("alias 0x8000 dropped", drop("links highalias 32768 ")),
        ("alias 0xffff dropped", drop("links highalias 65535 ")),
        ("alias 0x8000 reported as alias 0", swap("links highalias 32768 ", high_alias_line(0))),
        ("alias 0xffff stored in 15 bits", swap("links highalias 65535 ", high_alias_line(0x7FFF))),
        ("a high alias that resolved to another payload", swap("links highalias 32768 ", corrupt_last(good[first("links highalias 32768 ")]))),
        ("the high-alias scenario dropped", [line for line in good if line != "links ok reassemble-high-aliases"]),
        ("tenth-subscriber-in-nine-fixed admitted instead of refused", drop("links refused tenth-subscriber-in-nine-fixed ")),
        ("tenth-subscriber-in-nine-fixed refused with the wrong class", swap("links refused tenth-subscriber-in-nine-fixed ", "links refused tenth-subscriber-in-nine-fixed over-bound")),
        ("tenth-alias-in-nine-fixed admitted instead of refused", drop("links refused tenth-alias-in-nine-fixed ")),
        ("tenth-alias-in-nine-fixed refused with the wrong class", swap("links refused tenth-alias-in-nine-fixed ", "links refused tenth-alias-in-nine-fixed unsupported-message")),
        ("fragmented-put-multibyte-key-over-declared admitted instead of refused", drop("links refused fragmented-put-multibyte-key-over-declared ")),
        ("fragmented-put-multibyte-key-over-declared refused with the wrong class", swap("links refused fragmented-put-multibyte-key-over-declared ", "links refused fragmented-put-multibyte-key-over-declared unsupported-message")),
        ("257th-subscriber-in-256-heap admitted instead of refused", drop("links refused 257th-subscriber-in-256-heap ")),
        ("257th-subscriber-in-256-heap refused with the wrong class", swap("links refused 257th-subscriber-in-256-heap ", "links refused 257th-subscriber-in-256-heap over-bound")),
        ("257th-alias-in-256-heap admitted instead of refused", drop("links refused 257th-alias-in-256-heap ")),
        ("257th-alias-in-256-heap refused with the wrong class", swap("links refused 257th-alias-in-256-heap ", "links refused 257th-alias-in-256-heap unsupported-message")),
        ("257th-subscriber-in-256-fixed admitted instead of refused", drop("links refused 257th-subscriber-in-256-fixed ")),
        ("257th-subscriber-in-256-fixed refused with the wrong class", swap("links refused 257th-subscriber-in-256-fixed ", "links refused 257th-subscriber-in-256-fixed over-bound")),
        ("257th-alias-in-256-fixed admitted instead of refused", drop("links refused 257th-alias-in-256-fixed ")),
        ("257th-alias-in-256-fixed refused with the wrong class", swap("links refused 257th-alias-in-256-fixed ", "links refused 257th-alias-in-256-fixed unsupported-message")),
        ("the wide-count lines dropped", drop("links wide ")),
        ("the 256-slot heap line dropped", drop("links wide heap 256 ")),
        ("the 256-slot fixed line dropped", drop("links wide fixed 256 ")),
        ("the 65535-slot heap line dropped", drop("links wide heap 65535 ")),
        ("the 65535-slot fixed line dropped", drop("links wide fixed 65535 ")),
        ("a heap link of 255 keys reported for 256", wide_swap("heap", 256, 3, "255")),
        ("a fixed link of 255 subscribers reported for 256", wide_swap("fixed", 256, 4, "255")),
        ("a heap link of 255 samples reported for 256", wide_swap("heap", 256, 5, "255")),
        ("a fixed notice queue of 510 reported for 512", wide_swap("fixed", 256, 6, "510")),
        ("a heap link of 65534 keys reported for 65535", wide_swap("heap", 65535, 3, "65534")),
        ("a fixed link of 255 subscribers reported for 65535", wide_swap("fixed", 65535, 4, "255")),
        ("a heap link of 256 keys reported for 65535", wide_swap("heap", 65535, 3, "256")),
        ("a wide queue of payloads that repeat every 251 notices", wide_swap("heap", 256, 9, sha(chr(10).join(sha(distinct_payload(12, i)) for i in range(513)).encode()))),
        ("a wide queue whose payloads were overwritten", wide_swap("fixed", 256, 9, sha(b"overwritten"))),
        ("a wide heap link with a heap one message short", wide_swap("heap", 256, 8, str(int(good[first("links wide heap 256 ")].split()[8]) - 1))),
        ("a wide stack smaller than the arithmetic needs", wide_swap("fixed", 65535, 7, str(int(good[first("links wide fixed 65535 ")].split()[7]) - 1))),
        ("the wide-count scenarios dropped", [line for line in good if not line.startswith("links ok wide-counts-")]),
        ("the fixed nine-slot line dropped", drop("links countsfixed ")),
        ("a fixed link of four keys reported for nine", fixed_counts_swap(2, "4")),
        ("a fixed link of four subscribers reported for nine", fixed_counts_swap(3, "4")),
        ("a fixed link of four samples reported for nine", fixed_counts_swap(4, "4")),
        ("a fixed notice queue of eight reported for eighteen", fixed_counts_swap(5, "8")),
        ("a fixed nine-slot queue of small payloads", fixed_counts_swap(6, "512")),
        ("a fixed nine-slot queue whose payloads were overwritten", fixed_counts_swap(8, sha(b"overwritten"))),
        ("a fixed nine-slot stack smaller than the arithmetic needs", fixed_counts_swap(7, str(int(good[first("links countsfixed ")].split()[7]) - 1))),
        ("the fixed nine-slot scenario dropped", [line for line in good if line != "links ok nine-keys-subscribers-and-samples-fixed"]),
        ("the multibyte-key line dropped", drop("links multibyte ")),
        ("a multibyte key counted in characters", multibyte_swap(3, str(len(MULTIBYTE_KEY)))),
        ("a multibyte key reported as one character longer", multibyte_swap(2, str(len(MULTIBYTE_KEY) + 1))),
        ("a multibyte declaration sized for the characters", multibyte_swap(6, str(int(good[first("links multibyte ")].split()[6]) - 5))),
        ("a multibyte undeclaration with other bytes", multibyte_swap(9, sha(b"other"))),
        ("a multibyte Put with other fragment bodies", multibyte_swap(11, sha(b"other"))),
        ("a multibyte Put with another number of fragments", multibyte_swap(10, "2")),
        ("a multibyte Put that reassembled to another message", multibyte_swap(12, sha(b"other"))),
        ("the multibyte scenario dropped", [line for line in good if line != "links ok reassemble-multibyte-key"]),
        ("the subscriber id zero dropped", drop("links subid 0 ")),
        ("a subscriber id zero reported as one", swap("links subid 0 ", subscriber_id_line(1))),
        ("a subscriber id zero declared with another encoding", swap("links subid 0 ", corrupt_last(good[first("links subid 0 ")]))),
        ("the subscriber-id lines dropped", drop("links subid ")),
        ("the top-bit subscriber id dropped", drop("links subid 2147483648 ")),
        ("the largest subscriber id dropped", drop("links subid 4294967295 ")),
        ("a top-bit subscriber id declared with another encoding", swap("links subid 2147483648 ", corrupt_last(good[first("links subid 2147483648 ")]))),
        ("a subscriber id read in 31 bits", swap("links subid 2147483648 ", subscriber_id_line(0x7FFFFFFF))),
        ("a subscriber id read in 16 bits", swap("links subid 4294967295 ", subscriber_id_line(0xFFFF))),
        ("a subscriber id with a short declaration", swap("links subid 2147483648 ", good[first("links subid 2147483648 ")].replace(" 139 ", " 138 ", 1))),
        ("the subscriber-id scenario dropped", [line for line in good if line != "links ok subscriber-ids-use-all-32-bits"]),
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
    def put_at(message: bytes) -> int:
        return message.index(bytes([zenoh_wire.ID_PUT | zenoh_wire.FLAG_Z, zenoh_wire.EXT_ATTACHMENT]))

    def push_flag(flag: int) -> bytes:
        whole = bytearray(zenoh_fragment.full_key_for_exam(5))
        whole[0] |= flag
        return bytes(whole)

    def put_flag(flag: int) -> bytes:
        whole = bytearray(zenoh_fragment.full_key_for_exam(5))
        whole[put_at(bytes(whole))] |= flag
        return bytes(whole)

    def put_extension(value: int) -> bytes:
        whole = bytearray(zenoh_fragment.full_key_for_exam(5))
        whole[put_at(bytes(whole)) + 1] = value
        return bytes(whole)

    def delete_body() -> bytes:
        whole = zenoh_fragment.full_key_for_exam(5)
        return whole[: put_at(whole)] + bytes([0x02])

    def before_body() -> bytes:
        whole = zenoh_fragment.full_key_for_exam(5)
        return whole[: put_at(whole)]

    def unterminated_attachment_length() -> bytes:
        whole = zenoh_fragment.full_key_for_exam(5)
        return whole[: put_at(whole) + 2] + bytes([0x80])

    def unterminated_payload_length() -> bytes:
        whole = zenoh_fragment.full_key_for_exam(5)
        return whole[: put_at(whole) + 2 + 1 + zenoh_wire.ATTACHMENT_BYTES] + bytes([0x80])

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
        ("a multibyte key longer in bytes than declared though not in characters", lambda: zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(5, MULTIBYTE_OVER), max_key=len(MULTIBYTE_KEY.encode()))),
        ("a gap in the sequence numbers", lambda: zenoh_fragment.reassemble([bodies[0], bodies[2]])),
        ("fragments out of order", lambda: zenoh_fragment.reassemble([bodies[1], bodies[0], bodies[2]])),
        ("a run that never ends", lambda: zenoh_fragment.reassemble(bodies[:2])),
        ("a final fragment followed by another", lambda: zenoh_fragment.reassemble([clear_more(bodies[0]), *bodies[1:]])),
        ("a last fragment that still says more follows", lambda: zenoh_fragment.reassemble([*bodies[:2], set_more(bodies[2])])),
        ("a duplicate continuation sequence number", lambda: zenoh_fragment.reassemble([bodies[0], bodies[0], bodies[2]])),
        ("a backward continuation sequence number", lambda: zenoh_fragment.reassemble([bodies[0], bodies[1], bodies[0]])),
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
        ("a last fragment one millisecond past the timeout from a nonzero first timestamp", lambda: zenoh_fragment.check_deadline(DEADLINE_FIRST, DEADLINE_FIRST + TIMEOUT_MS + 1, TIMEOUT_MS)),
        ("a PUSH with the extension flag", lambda: zenoh_fragment.decode_message(push_flag(0x80), max_key=129)),
        ("a Put with a timestamp", lambda: zenoh_fragment.decode_message(put_flag(0x20), max_key=129)),
        ("a Put with an encoding", lambda: zenoh_fragment.decode_message(put_flag(0x40), max_key=129)),
        ("a Put with an unknown extension", lambda: zenoh_fragment.decode_message(put_extension(0x41), max_key=129)),
        ("a Put with a chained attachment extension", lambda: zenoh_fragment.decode_message(put_extension(0xC3), max_key=129)),
        ("a PUSH whose body is a delete", lambda: zenoh_fragment.decode_message(delete_body(), max_key=129)),
        ("a PUSH that ends before its body", lambda: zenoh_fragment.decode_message(before_body(), max_key=129)),
        ("an unterminated scope", lambda: zenoh_fragment.decode_message(bytes([0x5D, 0x80]))),
        ("an unterminated key length", lambda: zenoh_fragment.decode_message(bytes([0x7D, 0x00, 0x80]), max_key=129)),
        ("an unterminated attachment length", lambda: zenoh_fragment.decode_message(unterminated_attachment_length(), max_key=129)),
        ("an unterminated payload length", lambda: zenoh_fragment.decode_message(unterminated_payload_length(), max_key=129)),
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
        "a duplicate continuation sequence number": "not consecutive",
        "a last fragment one millisecond past the timeout from a nonzero first timestamp": "reassembly-timeout",
        "a PUSH with the extension flag": "unsupported-extension",
        "a Put with a timestamp": "unsupported-message",
        "a Put with an encoding": "unsupported-message",
        "a Put with an unknown extension": "unsupported-extension",
        "a Put with a chained attachment extension": "unsupported-extension",
        "a full-key Put with an empty key": "empty key",
        "a full-key Put with a truncated key": "truncated",
        "a fragmented full key with a trailing slash": "invalid-keyexpr",
        "a fragmented full key with a question mark": "invalid-keyexpr",
        "a fragmented full key with a hash": "invalid-keyexpr",
        "a fragmented full key with a $ wildcard": "wildcard-keyexpr",
        "a truncated sequence number": "truncated",
        "a multibyte key longer in bytes than declared though not in characters": "longer than the configuration declares",
        "a PUSH whose body is a delete": "unsupported-message",
        "a PUSH that ends before its body": "truncated",
        "an unterminated scope": "truncated",
        "an unterminated key length": "truncated",
        "an unterminated attachment length": "truncated",
        "an unterminated payload length": "truncated",
        "a backward continuation sequence number": "not consecutive",
        "a full-key Put in a nonzero scope": "nonzero scope",
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
        zenoh_fragment.check_deadline(DEADLINE_FIRST, DEADLINE_FIRST + last, TIMEOUT_MS)
    # A run whose interior fragments are shorter than the maximum is taken, so the reassembler does not
    # insist that every fragment fills the batch.
    uneven = zenoh_fragment.reference_split_uneven(zenoh_fragment.message(size), batch, sn, (1000, 1))
    if zenoh_fragment.reassemble(uneven, sn, batch) != zenoh_fragment.message(size):
        raise LinksError("control: a run with short interior fragments was not taken")
    # A Put must follow its declaration: after a FRAME at 4294967294 the next number is 4294967295, and a
    # run from 1000 is refused, as the session refuses it.
    after_declaration = zenoh_fragment.next_sn(0xFFFFFFFE, 1)
    consecutive = zenoh_fragment.reference_split(zenoh_fragment.message(5000), 2048, after_declaration)
    if zenoh_fragment.reassemble(consecutive, after_declaration, 2048) != zenoh_fragment.message(5000):
        raise LinksError("control: a run that follows its declaration was not taken")
    try:
        zenoh_fragment.reassemble(zenoh_fragment.reference_split(zenoh_fragment.message(5000), 2048, 1000), after_declaration, 2048)
    except zenoh_fragment.FragmentError:
        pass
    else:
        raise LinksError("control: a run that does not follow its declaration was taken")
    # The multibyte key itself is read where its byte length is declared, so the refusal above is not a
    # blanket one.
    if zenoh_fragment.decode_message(zenoh_fragment.full_key_for_exam(5, MULTIBYTE_KEY), max_key=len(MULTIBYTE_KEY.encode()), subscribed=MULTIBYTE_KEY)["key"] != MULTIBYTE_KEY.encode():
        raise LinksError("control: a valid multibyte key was not read")
    # A complete Put in one fragment with the more flag clear is taken.
    lone = zenoh_fragment.reference_split(zenoh_fragment.message(100), batch, sn)
    if len(lone) != 1 or zenoh_fragment.reassemble(lone, sn, batch) != zenoh_fragment.message(100):
        raise LinksError("control: a complete Put in a single final fragment was not taken")
    # A valid key shorter than the maximum and an alias with bit 15 set are read, so none of the key and
    # alias refusals is a blanket one.
    short_run = zenoh_fragment.reference_split(zenoh_fragment.full_key_for_exam(5000, SHORT_KEY), 2048, sn)
    if zenoh_fragment.decode_message(zenoh_fragment.reassemble(short_run, sn, 2048), max_key=129, subscribed=SHORT_KEY)["key"] != SHORT_KEY.encode():
        raise LinksError("control: a fragmented Put for a short valid key was not read")
    for high in (0x8000, 0xFFFF):
        if zenoh_fragment.decode_message(with_scope(zenoh_wire.leb128(high)), alias=high)["scope"] != high:
            raise LinksError(f"control: alias {high:#x} was not read")
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
