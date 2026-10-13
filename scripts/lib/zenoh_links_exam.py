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
    ("heap", "heap", 2048, 16384, 129, 33, 4, 4, 4, 32768, 5000, 82890),
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
    "heap-storage-from-component",
    "put-roundtrip-5000-2048",
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
    ("fragment-best-effort", "unsupported-message"),
    ("fragment-extension", "unsupported-extension"),
    ("empty-fragment", "malformed-message"),
    ("fragment-before-open", "unexpected-message"),
    ("fragment-sequence-gap", "fragment-sequence"),
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
    ("fragmented-put-alias-scope-zero", "invalid-keyexpr"),
    ("fragmented-put-alias-scope-65536", "leb128-overflow"),
    ("message-over-declared-heap", "over-bound"),
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


def reuse_capacity() -> int:
    return REUSE_SIZE + 129 + 33 + FRAMING


PINNED_CASES = 68
PINNED_CONTROLS = 130


class LinksError(ValueError):
    """Evidence the judge refuses."""


def capture_name(size: int, batch: int) -> str:
    return f"{size}-{batch}"


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def key() -> str:
    return zenoh_exchange.demo()["key"]


def full_key_message(size: int) -> bytes:
    """Our own Put: a PUSH naming the demo key in full, sender mapping, with the 33-byte attachment."""
    return zenoh_wire.push_put(key(), zenoh_fragment.attachment(), zenoh_fragment.payload_bytes(size), True)


def full_key_bodies() -> list[bytes]:
    return zenoh_fragment.reference_split(full_key_message(FULL_KEY_PAYLOAD), FULL_KEY_BATCH, FULL_KEY_SN)


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
        if name == "reassembly-after-timeout-reuses-storage":
            lines.append(f"links reuse {reuse_capacity()} {REUSE_SIZE} {sha(zenoh_fragment.payload_bytes(REUSE_SIZE))}")
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

    reuse = [(int(f[2]), int(f[3]), f[4]) for f in only("reuse")]
    wanted_reuse = [(reuse_capacity(), REUSE_SIZE, sha(zenoh_fragment.payload_bytes(REUSE_SIZE)))]
    if reuse != wanted_reuse:
        raise LinksError(f"reassembly into storage of one message after a timeout is {wanted_reuse}, reported {reuse}")

    count = len(full_key_bodies())
    advance = [(f[2], int(f[3]), int(f[4]), int(f[5])) for f in only("advance")]
    wanted_advance = [(label, before, count, zenoh_fragment.next_sn(before, count)) for label, before in ADVANCE]
    if advance != wanted_advance:
        raise LinksError(f"the session sequence after a whole fragment run is {wanted_advance}, reported {advance}")

    refused = [(f[2], f[3]) for f in only("refused")]
    if [name for name, _ in refused] != [name for name, _ in REFUSED_SCENARIOS]:
        raise LinksError(f"refusals reported {[n for n, _ in refused]}, the exam names {[n for n, _ in REFUSED_SCENARIOS]}")
    wrong = [f"{n}={c} (expected {dict(REFUSED_SCENARIOS)[n]})" for n, c in refused if c != dict(REFUSED_SCENARIOS)[n]]
    if wrong:
        raise LinksError(f"refusal classes differ: {', '.join(wrong)}")
    positive = len(configs) + len(done) + len(sent) + len(reassembled) + len(fullkey) + len(boundary) + len(advance) + len(reuse)
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
FRAMING, FRAGMENT_FRAMING, STACK_FACTOR, SLOT, NOTICE, NOTICES = 32, 8, 8, 16, 24, 2


def required_stack(c: dict) -> int:
    slots = (c["maxKeys"] + c["maxSubscribers"]) * (c["keyBytes"] + SLOT)
    payload = c["payloadBytes"] if c["mode"] == "fixed" else 0
    return STACK_FACTOR * c["batchBytes"] + slots + NOTICES * c["outstandingSamples"] * (NOTICE + payload)


def message_bytes(c: dict) -> int:
    return c["payloadBytes"] + c["keyBytes"] + c["attachmentBytes"] + FRAMING


def heap_needed(c: dict) -> int:
    """A heap link hands each sample out as a slice of the buffer it was reassembled into, so every
    outstanding sample holds a whole message until it is consumed, and one more is being reassembled."""
    return (c["outstandingSamples"] + 1) * message_bytes(c)


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
        ("a batch of 4094, one service operation exactly", trio(large=exact(large, batchBytes=4094, payloadBytes=2048)), True),
        ("a batch of 4095, one byte over an operation", trio(large=exact(large, batchBytes=4095, payloadBytes=2048)), False),
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
        ("a fixed payload that fits its batch exactly", trio(large=exact(large, payloadBytes=2048 - 129 - 33 - FRAMING)), True),
        ("a fixed payload one byte over its batch", trio(large=exact(large, payloadBytes=2048 - 129 - 33 - FRAMING + 1)), False),
        ("a fixed link with a reassembly timeout", trio(large=dict(large, reassemblyTimeoutMs=1000)), False),
        ("a fixed link with a heap", trio(large=dict(large, heapBytes=1)), False),
        ("a heap link with a timeout of 0", trio(heap=dict(heap, reassemblyTimeoutMs=0)), False),
        ("a heap link with a negative timeout", trio(heap=dict(heap, reassemblyTimeoutMs=-1)), False),
        ("a heap exactly every outstanding sample and the message being reassembled", trio(heap=dict(heap, heapBytes=heap_needed(heap))), True),
        ("a heap one byte under that", trio(heap=dict(heap, heapBytes=heap_needed(heap) - 1)), False),
        ("a heap of exactly one message with four outstanding samples", trio(heap=dict(heap, heapBytes=message_bytes(heap))), False),
        ("one outstanding sample needs two messages of heap", trio(heap=dict(heap, outstandingSamples=1, heapBytes=2 * message_bytes(heap))), True),
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
    def with_scope(scope: bytes) -> bytes:
        whole = zenoh_fragment.message(1)
        return bytes([0x1D]) + scope + whole[2:]

    def no_attachment(payload: int) -> bytes:
        return bytes([0x1D, 0x01, zenoh_wire.ID_PUT]) + zenoh_wire.leb128(payload) + zenoh_fragment.payload_bytes(payload)

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
        ("a final fragment followed by another", lambda: zenoh_fragment.reassemble([bodies[2], bodies[2]])),
        ("no fragments", lambda: zenoh_fragment.reassemble([])),
        ("an unreliable fragment", lambda: zenoh_fragment.decode_fragment(bytes([bodies[0][0] & ~zenoh_fragment.FLAG_R]) + bodies[0][1:])),
        ("a fragment with extensions", lambda: zenoh_fragment.decode_fragment(bytes([bodies[0][0] | zenoh_fragment.FLAG_Z]) + bodies[0][1:])),
        ("a fragment that is another message", lambda: zenoh_fragment.decode_fragment(bytes([0x25]) + bodies[0][1:])),
        ("a fragment with no data", lambda: zenoh_fragment.decode_fragment(bodies[0][:5])),
        ("a message whose payload length is wrong", lambda: zenoh_fragment.decode_message(zenoh_fragment.message(size)[:-1])),
        ("a batch too small for a fragment", lambda: zenoh_fragment.reference_split(b"x" * 10, 8, 0)),
        ("an alias scope of 0", lambda: zenoh_fragment.decode_message(with_scope(bytes([0x00])))),
        ("an alias scope of 65536", lambda: zenoh_fragment.decode_message(with_scope(bytes([0x80, 0x80, 0x04])))),
        ("an alias scope of four bytes", lambda: zenoh_fragment.decode_message(with_scope(bytes([0xFF, 0xFF, 0xFF, 0x7F])))),
        ("a noncanonical alias scope of 1", lambda: zenoh_fragment.decode_message(with_scope(bytes([0x81, 0x00])))),
        ("two messages joined in one run", lambda: zenoh_fragment.decode_message(zenoh_fragment.reassemble(zenoh_fragment.reference_split(zenoh_fragment.message(1000) + zenoh_fragment.message(1000), 512, sn)))),
        ("a fragmented Put without the attachment", lambda: zenoh_fragment.decode_message(zenoh_fragment.reassemble(zenoh_fragment.reference_split(no_attachment(3000), 512, sn)))),
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
    refused = 0
    for label, action in cases:
        try:
            action()
        except zenoh_fragment.FragmentError:
            refused += 1
            continue
        raise LinksError(f"control: the fragment reference accepted {label}")
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
