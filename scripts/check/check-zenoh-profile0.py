#!/usr/bin/env python3

"""Zenoh Profile 0 host gate for R0: encoder, session, CDR, transport and composition.

Each Rust slice prints one `[zenoh-exam] ...` line per case it judges:

  encode  ok <name> <hex>               a batch the encoder produced
  encode  refused <control>             an input the encoder refused
  session ok <scenario>                 a scenario the session machine completed
  session refused <control>             an input the session machine refused
  cdr     ok <sequence> <value> <hex>   a sample the CDR codec produced
  cdr     refused <control>             an input the CDR decoder refused
  link    ok <scenario>                 a transport-runtime scenario over a scripted byte link
  link    refused <control>             a link input the runtime refused

This checker owns the expectation, never the tests: it counts the lines it reads
and compares each against bytes from an independent source. The decoder corpus's
accepted batches were produced by the upstream eclipse-zenoh 1.0.0 encoder, and
`scripts/lib/zenoh_wire.py` re-derives every one from its summary before it is
trusted; the CDR samples are recomputed from the demo fixture's field values.
`--controls-only` proves, without Rust, that each judge refuses missing, extra,
duplicated, reordered and corrupted evidence.

`--slice xcheck` runs `verification/zenoh-xcheck`, which drives this repository's codec
and session against the real eclipse-zenoh 1.0.0 crates (`zenoh-codec`,
`zenoh-protocol`, `zenoh-buffers`), and judges the `[zenoh-xcheck] ...` lines it
prints. The expectation is pinned here, never read from the harness: the 17 corpus
batches as upstream's own encoder now produces them, a 6000-case sweep in which
our encoder equals upstream's byte for byte (5993 compared, 7 refused as a
non-canonical Zenoh ID), our decoder accepts what upstream encodes, upstream reads
and re-encodes ours, an 11-batch session read back identically, and eight Zenoh ID
probes whose outcome this file derives from the byte pattern. It needs the crates
from a registry, so it is explicit and not in CI.

The harness prints one `[zenoh-xcheck] <word> ...` line per result:

  corpus <name> <hex>                              upstream's encoding of a corpus batch
  encoder cases=N compared=N differ=N refused=N    sweep, our encoder against upstream's
  refusal <reason> <count>                         why the sweep's refused cases were refused
  decoder cases=N differ=N                         our decoder against upstream's bytes
  reader compared=N differ=N                       upstream reading and re-encoding ours
  session batches=N checked=N differ=N             one session, every batch read back
  zid <hex> upstream=<v> encode=<v> wire-upstream=<v> wire-ours=<v>
                                                   one Zenoh ID probe, in both directions

`--slice stock` judges this repository's session against batches a stock eclipse-zenoh 1.0.0
peer put on a TCP link (`scripts/lib/zenoh_stock_peer.py`, captured once from the Python
wheel, with its hash and the way each batch was obtained). A Rust test module,
`zenoh_stock::`, replays every captured batch into the session, prints one
`[zenoh-exam] stock ...` line per scenario, and the checker owns the expectation:

  stock ok <scenario>                     a stock-peer interaction the session completed
  stock replay <fixture> ok               a captured batch the session took
  stock refused <name> <class>            a captured or constructed batch refused, with its class
  stock delivered <sequence> <hex> <ts>   a sample the subscriber side delivered: its payload from the demo
                                          contract, its timestamp from the captured attachment
  stock attachment <seq>:<ts> <gid-hex>   the attachment the decoder read from a captured push; the session
                                          event carries no GID, so the exam reads it from the decoded push

It also pins the captured pushes to the demo contract's four samples and attachments, so a
fixture cannot drift from the topic the demo exchanges. The recipe fails until the session
accepts a stock peer; nothing here claims `rmw_zenoh` interoperability or liveliness.

`--slice composition` judges the derived `sel4-zenoh` composition against the
demo contract's authority and refuses 15 mutations of an honest one. It is the
static half of the QEMU arm; the boot is `check-sel4-io-network-plane.py --arm zenoh`.
"""

from __future__ import annotations

import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import hashlib
import os as _os
import re
import shutil
import struct
import subprocess
from collections.abc import Callable
from typing import NoReturn

import devloop_observations
import zenoh_exchange
import zenoh_stock_peer
import zenoh_wire
import zutai_cli
from harness import GENERATION_COMPOSITIONS, ROOT

CORPUS = ROOT / "components" / "lib" / "src" / "zenoh_profile0" / "vectors.txt"
COMPOSITION = GENERATION_COMPOSITIONS / "sel4-zenoh.zti"
EXAM_LINE = re.compile(r"\[zenoh-exam\] (.*)$")

STOCK_FIXTURES = ROOT / "build" / "zenoh-stock" / "fixtures.zti"
ZENOH_DECODER = ROOT / "build" / "zenoh-stock" / "decode-fixtures.zt"
STOCK_FILTER = "zenoh_stock::"
# The decoder is held to the pinned corpus by running it: the digests above prove the corpus text is
# unchanged, and this test proves the decoder still gives every pinned row its pinned verdict.
STOCK_CORPUS_TEST = "zenoh_profile0::tests::corpus_agrees"
# The refused vectors in components/lib/src/zenoh_profile0/vectors.txt that the stock-peer change
# may turn from refused to accepted: only those whose bytes a captured stock batch repeats exactly.
# The KEEP_ALIVE vector is the single byte 0x04, and the stock peer sent that byte four times.
# Every other refused vector stays refused, including the ones whose names sound related (an
# extension flag, an alias, a scope, a suffix, an OAM): each was compared byte for byte against the
# capture and none matches, because the stock peer attaches a QoS extension and a body those
# vectors do not have. An implementation accepts the shapes the capture shows, and the 19 replays
# below prove it does.
STOCK_MAY_CHANGE = frozenset({"transport_keep_alive"})
# The refused vectors that must stay exactly as they were when the exam landed, by name, with a
# digest of `<name> <class> <bytes>` for each in name order. The checker reads the working tree's
# corpus and compares; a vector may be added, and transport_keep_alive may change or go, but none
# of these may move.
STOCK_CORPUS = ROOT / "components" / "lib" / "src" / "zenoh_profile0" / "vectors.txt"
STOCK_FIXED_NAMES = tuple((
    "batch_513_bytes close_extension_present close_reason_unknown close_reserved_flag "
    "close_trailing_byte close_truncated declare_extension_flag declare_interest_flag "
    "declare_keyexpr_alias declare_queryable declare_reserved_flag declare_token "
    "declare_truncated_kind dsub_double_slash dsub_empty_suffix dsub_extension_flag "
    "dsub_id_overflow dsub_invalid_utf8 dsub_key_130_bytes dsub_leading_slash dsub_no_suffix "
    "dsub_nonzero_scope dsub_truncated_suffix dsub_wildcard_double_star dsub_wildcard_star "
    "dsub_wildcard_sub empty_batch frame_best_effort frame_empty frame_extension_present "
    "frame_network_interest frame_network_oam frame_network_request frame_network_response "
    "frame_partial_network_message frame_reserved_flag frame_sn_missing frame_sn_overflow "
    "init_ack_cookie_65_bytes init_ack_cookie_empty init_ack_cookie_len_noncanonical "
    "init_ack_cookie_len_over_u16 init_ack_cookie_len_u16_max init_ack_cookie_truncated "
    "init_ack_zid_trailing_zero init_batch_size_zero init_extension_present "
    "init_reserved_packed_bits init_resolution_64bit init_trailing_byte "
    "init_truncated_after_version init_truncated_zid init_version_8 init_whatami_client "
    "init_whatami_router init_zid_all_zero init_zid_one_byte_zero init_zid_trailing_zero "
    "open_ack_extension_present open_ack_truncated open_syn_cookie_65_bytes "
    "open_syn_cookie_empty open_syn_cookie_truncated open_syn_lease_over_32_bits "
    "open_syn_lease_zero open_syn_sn_overflow push_del_body push_extension_flag "
    "push_key_130_bytes push_no_suffix push_nonzero_scope "
    "push_put_no_attachment_upstream_valid push_truncated_body push_wildcard_key "
    "put_attachment_32_bytes put_attachment_34_bytes put_attachment_gid_len_15 "
    "put_attachment_truncated put_encoding_flag put_extension_chained put_payload_13_bytes "
    "put_payload_len_noncanonical put_payload_len_overflow put_payload_truncated "
    "put_timestamp_flag put_unknown_extension transport_fragment transport_join transport_oam "
    "transport_unknown_id usub_ext_chained usub_ext_not_mandatory usub_flags_reserved_bits "
    "usub_missing_wire_expr_ext usub_no_suffix usub_nonzero_scope usub_reserved_header_bit "
    "usub_wildcard usub_wrong_ext_id usub_zbuf_length_132 usub_zbuf_truncated"
).split())
STOCK_FIXED_DIGEST = "7784f50485c9f195fe271dc2f1a92ab28791fcf8e6e7fb08e1e9d0f949ccda12"
# The accepted batches are the upstream encoder's output and are pinned by name, bytes and
# summary: replacing one, dropping one or renaming a copy all change the digest.
STOCK_ACCEPTED_NAMES = tuple((
    "close_link_invalid close_session_generic close_session_unsupported "
    "frame_declare_and_push_coalesced_sn2 frame_declare_recv_mapping_id300_sn4294967295 "
    "frame_declare_subscriber_sn0 frame_push_put_sn1 frame_undeclare_subscriber_sn7 "
    "init_ack_batch512_cookie3 init_ack_default_zid1_cookie1 init_syn_batch512 "
    "init_syn_default init_syn_default_zid16 open_ack_lease2s_sn9 "
    "open_syn_lease1500ms_sn300_cookie1 open_syn_lease2s_sn5_cookie3 "
    "open_syn_lease60s_snmax_cookie16"
).split())
STOCK_ACCEPTED_DIGEST = "1260d9d725d5fb2055d1ffb2be8f4e33f73e029f950901389c6ca586b350f7ef"
# The stream-framing rows (ten accepted, four refused) carry the 512-byte stream bound and the
# reassembly rules. None of them is something a stock peer sent, so none may change: each is pinned
# by name and by a digest of everything after its kind, which covers its verdict, class, chunks
# and summary.
STOCK_STREAM_NAMES = tuple((
    "stream_batch_split_mid_body stream_byte_at_a_time stream_complete_then_partial "
    "stream_empty_chunks_interleaved stream_incomplete_body_pending "
    "stream_incomplete_prefix_pending stream_length_513 stream_length_65535 "
    "stream_length_zero stream_max_batch_512 stream_prefix_split_across_chunks "
    "stream_refusal_after_valid_batch stream_single_batch stream_two_batches_one_chunk"
).split())
STOCK_STREAM_DIGEST = "e25ae97153255e290cab357f9a5b97bbb862a40a7caf8ffec63ac784663ca070"
STOCK_PUSHES = tuple(f"connects/connects-{n:02d}-push-{n - 4}" for n in (5, 6, 7, 8))

XCHECK_CRATE = ROOT / "verification" / "zenoh-xcheck"
XCHECK_LINE = re.compile(r"\[zenoh-xcheck\] (.*)$")
XCHECK_SWEEP = 6000
XCHECK_REFUSED = 7
XCHECK_SESSION_BATCHES = 11
XCHECK_SESSION_CHECKED = 27
# Zenoh IDs: upstream holds one as a little-endian integer, so it refuses zero and
# drops trailing zero bytes; Profile 0 refuses a trailing zero byte so that every
# ID it accepts round-trips.
XCHECK_ZIDS = (
    "0000000000000000",
    "00",
    "0102030405060700",
    "0102030405000000",
    "0002030405060708",
    "0102000405060708",
    "a1a1a1a1a1a1a1a1",
    "5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a",
)

ENCODE_REFUSALS = (
    "key-over-129-bytes",
    "attachment-not-33-bytes",
    "payload-over-12-bytes",
    "cookie-over-64-bytes",
    "zid-empty-or-over-16-bytes",
    "wildcard-key",
    "batch-over-512-bytes",
)
SESSION_SCENARIOS = (
    "connect-handshake",
    "listen-handshake",
    "declare-then-push-delivered",
    "teardown-undeclare-then-close",
    "lease-expiry-closes",
)
SESSION_REFUSALS = (
    "router-peer",
    "version-mismatch",
    "wildcard-key-declaration",
    "push-to-undeclared-key",
    "replayed-sequence",
    "frame-before-open",
    "cookie-mismatch",
)
CDR_REFUSALS = (
    "truncated-header",
    "truncated-body",
    "trailing-bytes",
    "big-endian-encapsulation",
    "unknown-encapsulation",
    "nonzero-options",
    "over-max-serialized",
)
LINK_SCENARIOS = (
    "connector-opens-and-publishes",
    "listener-accepts-and-delivers",
    "split-and-coalesced-batches",
    "backpressure-resumes",
    "peer-close-ends-session",
    "restart-uses-fresh-session",
)
LINK_REFUSALS = (
    "oversized-length-prefix",
    "zero-length-prefix",
    "garbage-after-open",
    "receive-queue-full",
    "retry-limit-reached",
    "send-after-close",
    "stale-session-handle",
)

# slice -> (the Rust test filter, scenario names, refusal names, word on the exam line)
RUST = {
    "encoder": ("zenoh_profile0::", None, ENCODE_REFUSALS, "encode"),
    "session": ("zenoh_profile0::", SESSION_SCENARIOS, SESSION_REFUSALS, "session"),
    "cdr": ("ros_cdr::", None, CDR_REFUSALS, "cdr"),
    "transport": ("zenoh_link::", LINK_SCENARIOS, LINK_REFUSALS, "link"),
}


def fail(message: str) -> NoReturn:
    raise SystemExit(f"zenoh profile 0 check failed: {message}")


def reference() -> list[tuple[str, str, str]]:
    """The corpus's accepted batches, after proving the reference reproduces each."""
    if not CORPUS.is_file():
        fail(f"{CORPUS.relative_to(ROOT)} is missing; the decoder corpus must land first")
    try:
        zenoh_wire.self_test(CORPUS)
    except zenoh_wire.WireError as error:
        fail(f"the host wire reference disagrees with the upstream corpus: {error}")
    return zenoh_wire.corpus_accepted(CORPUS)


def demo_samples() -> dict[int, tuple[int, str]]:
    contract = zenoh_exchange.demo()
    samples = {seq: (value, hexed) for seq, value, hexed in contract["samples"]}
    for seq, (value, hexed) in samples.items():
        # Classic CDR_LE: representation id and options are big-endian (00 01, 00 00)
        # and name the byte order of the body that follows.
        independent = (struct.pack(">HH", 1, 0) + struct.pack("<Ii", seq, value)).hex()
        if hexed != independent:
            fail(f"demo sample {seq}: contract hex {hexed} differs from {independent}")
    return samples


def exam_lines(output: str) -> list[str]:
    return [match.group(1).strip() for line in output.splitlines() if (match := EXAM_LINE.search(line))]


def exactly(label: str, wanted: tuple[str, ...], seen: list[str]) -> None:
    duplicated = sorted({name for name in seen if seen.count(name) > 1})
    if duplicated:
        fail(f"{label}: reported twice: {', '.join(duplicated)}")
    missing = [name for name in wanted if name not in seen]
    extra = [name for name in seen if name not in wanted]
    if missing:
        fail(f"{label}: not reported: {', '.join(missing)}")
    if extra:
        fail(f"{label}: not in the exam: {', '.join(extra)}")


def names(lines: list[str], word: str, verdict: str) -> list[str]:
    split = [line.split() for line in lines]
    return [fields[2] for fields in split if len(fields) == 3 and fields[0] == word and fields[1] == verdict]


def judge_encoder(lines: list[str]) -> tuple[int, int]:
    golden = {name: hexed for name, hexed, _ in reference()}
    produced: dict[str, str] = {}
    for line in lines:
        fields = line.split()
        if len(fields) == 4 and fields[:2] == ["encode", "ok"]:
            if fields[2] in produced:
                fail(f"encoder reported {fields[2]} twice")
            produced[fields[2]] = fields[3]
    exactly("encoder batches", tuple(golden), list(produced))
    for name, hexed in produced.items():
        if hexed != golden[name]:
            fail(f"encoder batch {name}: produced {hexed}, upstream bytes are {golden[name]}")
    refused = names(lines, "encode", "refused")
    exactly("encoder refusals", ENCODE_REFUSALS, refused)
    return len(produced), len(refused)


def judge_named(slice_name: str) -> Callable[[list[str]], tuple[int, int]]:
    _, scenarios, refusals, word = RUST[slice_name]

    def judge(lines: list[str]) -> tuple[int, int]:
        done = names(lines, word, "ok")
        refused = names(lines, word, "refused")
        exactly(f"{slice_name} scenarios", scenarios, done)
        exactly(f"{slice_name} refusals", refusals, refused)
        return len(done), len(refused)

    return judge


def judge_cdr(lines: list[str]) -> tuple[int, int]:
    wanted = demo_samples()
    produced: dict[int, tuple[int, str]] = {}
    for line in lines:
        fields = line.split()
        if len(fields) == 5 and fields[:2] == ["cdr", "ok"] and fields[2].isdigit() and fields[3].isdigit():
            seq = int(fields[2])
            if seq in produced:
                fail(f"CDR reported sample {seq} twice")
            produced[seq] = (int(fields[3]), fields[4])
    if sorted(produced) != sorted(wanted):
        fail(f"CDR samples reported {sorted(produced)}, the demo contract has {sorted(wanted)}")
    for seq, (value, hexed) in produced.items():
        if (value, hexed) != wanted[seq]:
            fail(f"CDR sample {seq}: produced {value} {hexed}, contract has {wanted[seq]}")
    refused = names(lines, "cdr", "refused")
    exactly("CDR refusals", CDR_REFUSALS, refused)
    return len(produced), len(refused)


JUDGES: dict[str, Callable[[list[str]], tuple[int, int]]] = {
    "encoder": judge_encoder,
    "session": judge_named("session"),
    "cdr": judge_cdr,
    "transport": judge_named("transport"),
}


def good_lines(slice_name: str) -> list[str]:
    _, scenarios, refusals, word = RUST[slice_name]
    refused = [f"{word} refused {c}" for c in refusals]
    if slice_name == "encoder":
        return [f"encode ok {n} {h}" for n, h, _ in reference()] + refused
    if slice_name == "cdr":
        return [f"cdr ok {seq} {value} {hexed}" for seq, (value, hexed) in demo_samples().items()] + refused
    return [f"{word} ok {s}" for s in scenarios] + refused


def controls(slice_name: str) -> int:
    """Refuse corrupted evidence before trusting the judge; returns the count."""
    judge = JUDGES[slice_name]
    good = good_lines(slice_name)
    judge(good)
    word = RUST[slice_name][3]
    mutations: list[tuple[str, list[str]]] = [
        ("an empty transcript", []),
        ("a dropped first line", good[1:]),
        ("a dropped last line", good[:-1]),
        ("a duplicated line", good + [good[0]]),
        ("an unknown refusal", good + [f"{word} refused invented"]),
        ("every refusal removed", [line for line in good if " refused " not in line]),
    ]
    if slice_name in ("encoder", "cdr"):
        at = next(i for i, line in enumerate(good) if " ok " in line)
        fields = good[at].split()
        flipped = fields[-1][:-1] + ("0" if fields[-1][-1] != "0" else "1")
        mutations.append(("a corrupted byte", good[:at] + [" ".join(fields[:-1] + [flipped])] + good[at + 1 :]))
        mutations.append(("a truncated hex string", good[:at] + [" ".join(fields[:-1] + [fields[-1][:-2]])] + good[at + 1 :]))
    for label, mutated in mutations:
        try:
            judge(mutated)
        except SystemExit:
            continue
        fail(f"control: {label} was accepted by the {slice_name} judge")
    return len(mutations)


def run_tests(slice_name: str) -> list[str]:
    host = subprocess.run(["rustc", "-vV"], capture_output=True, text=True, check=True)
    target = next(line.split(": ", 1)[1] for line in host.stdout.splitlines() if line.startswith("host: "))
    filter_ = RUST[slice_name][0]
    command = [
        "cargo", "test", "--target", target, "-p", "slime-components", "--no-default-features",
        filter_, "--", "--nocapture", "--test-threads=1",
    ]
    result = subprocess.run(command, cwd=ROOT / "components", capture_output=True, text=True)
    output = result.stdout + result.stderr
    if result.returncode != 0:
        _sys.stderr.write(output[-4000:])
        fail(f"{' '.join(command)} exited {result.returncode}")
    results = re.findall(r"test result: (\w+)\. (\d+) passed; (\d+) failed", output)
    if not any(status == "ok" and int(passed) > 0 for status, passed, _ in results) or any(
        status != "ok" or int(failed) for status, _, failed in results
    ):
        fail(f"no passing test result for filter {filter_!r}")
    return exam_lines(result.stdout)


def keyed(lines: list[str], word: str) -> dict[str, str]:
    """The `key=value` fields of the one `[zenoh-xcheck] <word> ...` line."""
    found = [line.split()[1:] for line in lines if line.split()[:1] == [word]]
    if len(found) != 1:
        fail(f"xcheck: {len(found)} {word!r} lines, expected exactly one")
    pairs = [field.partition("=") for field in found[0]]
    if any(not sep for _, sep, _ in pairs):
        fail(f"xcheck: malformed {word!r} line: {' '.join(found[0])}")
    return {key: value for key, _, value in pairs}


def expect(word: str, actual: dict[str, str], wanted: dict[str, int]) -> None:
    if set(actual) != set(wanted):
        fail(f"xcheck {word}: fields {sorted(actual)}, expected {sorted(wanted)}")
    for key, value in wanted.items():
        if actual[key] != str(value):
            fail(f"xcheck {word}: {key}={actual[key]}, expected {value}")


def judge_xcheck(lines: list[str]) -> tuple[int, int]:
    golden = {name: hexed for name, hexed, _ in reference()}
    corpus: dict[str, str] = {}
    for line in lines:
        fields = line.split()
        if len(fields) == 3 and fields[0] == "corpus":
            if fields[1] in corpus:
                fail(f"xcheck: corpus batch {fields[1]} reported twice")
            corpus[fields[1]] = fields[2]
    exactly("xcheck corpus batches", tuple(golden), list(corpus))
    for name, hexed in corpus.items():
        if hexed != golden[name]:
            fail(f"xcheck corpus {name}: upstream encodes {hexed}, the corpus has {golden[name]}")

    compared = XCHECK_SWEEP - XCHECK_REFUSED
    expect("encoder", keyed(lines, "encoder"), {"cases": XCHECK_SWEEP, "compared": compared, "differ": 0, "refused": XCHECK_REFUSED})
    expect("decoder", keyed(lines, "decoder"), {"cases": XCHECK_SWEEP, "differ": 0})
    expect("reader", keyed(lines, "reader"), {"compared": compared, "differ": 0})
    expect("session", keyed(lines, "session"), {"batches": XCHECK_SESSION_BATCHES, "checked": XCHECK_SESSION_CHECKED, "differ": 0})

    reasons = [line.split()[1:] for line in lines if line.split()[:1] == ["refusal"]]
    if reasons != [["ZidNotCanonical", str(XCHECK_REFUSED)]]:
        fail(f"xcheck: sweep refusals {reasons}, expected only ZidNotCanonical x {XCHECK_REFUSED}")

    probes: dict[str, dict[str, str]] = {}
    for line in lines:
        fields = line.split()
        if fields[:1] == ["zid"] and len(fields) == 6:
            if fields[1] in probes:
                fail(f"xcheck: Zenoh ID probe {fields[1]} reported twice")
            probes[fields[1]] = dict(field.partition("=")[::2] for field in fields[2:])
    exactly("xcheck Zenoh ID probes", XCHECK_ZIDS, list(probes))
    for zid, seen in probes.items():
        raw = bytes.fromhex(zid)
        wanted = {
            "upstream": "refuses" if not any(raw) else "accepts",
            "encode": "refuses" if raw[-1] == 0 else "encodes",
            "wire-upstream": "refuses" if not any(raw) else "accepts",
            "wire-ours": "refuses" if raw[-1] == 0 else "accepts",
        }
        if seen != wanted:
            fail(f"xcheck Zenoh ID {zid}: {seen}, expected {wanted}")
    return len(corpus) + XCHECK_SWEEP + XCHECK_SESSION_BATCHES + len(probes), XCHECK_REFUSED


def xcheck_good_lines() -> list[str]:
    lines = [f"corpus {name} {hexed}" for name, hexed, _ in reference()]
    compared = XCHECK_SWEEP - XCHECK_REFUSED
    lines += [
        f"encoder cases={XCHECK_SWEEP} compared={compared} differ=0 refused={XCHECK_REFUSED}",
        f"refusal ZidNotCanonical {XCHECK_REFUSED}",
        f"decoder cases={XCHECK_SWEEP} differ=0",
        f"reader compared={compared} differ=0",
        f"session batches={XCHECK_SESSION_BATCHES} checked={XCHECK_SESSION_CHECKED} differ=0",
    ]
    for zid in XCHECK_ZIDS:
        raw = bytes.fromhex(zid)
        up = "refuses" if not any(raw) else "accepts"
        ours = "refuses" if raw[-1] == 0 else None
        lines.append(f"zid {zid} upstream={up} encode={ours or 'encodes'} wire-upstream={up} wire-ours={'refuses' if ours else 'accepts'}")
    return lines


def xcheck_controls() -> int:
    """Refuse corrupted or hollowed-out harness output before trusting a real one."""
    good = xcheck_good_lines()
    judge_xcheck(good)

    def swap(prefix: str, new: str) -> list[str]:
        at = next(i for i, line in enumerate(good) if line.startswith(prefix))
        return good[:at] + [new] + good[at + 1 :]

    first = next(i for i, line in enumerate(good) if line.startswith("corpus "))
    fields = good[first].split()
    flipped = fields[2][:-1] + ("0" if fields[2][-1] != "0" else "1")
    compared = XCHECK_SWEEP - XCHECK_REFUSED
    mutations: list[tuple[str, list[str]]] = [
        ("an empty transcript", []),
        ("a dropped corpus batch", good[:first] + good[first + 1 :]),
        ("a corrupted corpus byte", swap("corpus ", f"corpus {fields[1]} {flipped}")),
        ("a duplicated corpus batch", good + [good[first]]),
        ("an encoder difference", swap("encoder ", f"encoder cases={XCHECK_SWEEP} compared={compared} differ=1 refused={XCHECK_REFUSED}")),
        ("a decoder difference", swap("decoder ", f"decoder cases={XCHECK_SWEEP} differ=1")),
        ("a reader difference", swap("reader ", f"reader compared={compared} differ=1")),
        ("a session difference", swap("session ", f"session batches={XCHECK_SESSION_BATCHES} checked={XCHECK_SESSION_CHECKED} differ=1")),
        ("a short session", swap("session ", f"session batches=10 checked={XCHECK_SESSION_CHECKED} differ=0")),
        ("a short sweep", swap("encoder ", f"encoder cases=5999 compared={compared - 1} differ=0 refused={XCHECK_REFUSED}")),
        ("the Zenoh ID rule silently dropped", swap("encoder ", f"encoder cases={XCHECK_SWEEP} compared={XCHECK_SWEEP} differ=0 refused=0")),
        ("an unexplained refusal", good + ["refusal KeyTooLong 1"]),
        ("a wrong refusal count", swap("refusal ", "refusal ZidNotCanonical 6")),
        ("an all-zero Zenoh ID that upstream accepts", [line.replace("upstream=refuses", "upstream=accepts", 1) if line.startswith("zid 0000") else line for line in good]),
        ("a trailing-zero Zenoh ID that the encoder emits", [line.replace("encode=refuses", "encode=encodes", 1) if line.startswith("zid 0102030405060700") else line for line in good]),
        ("a dropped Zenoh ID probe", [line for line in good if not line.startswith("zid 5a")]),
        ("a duplicated encoder line", good + [good[next(i for i, line in enumerate(good) if line.startswith("encoder "))]]),
    ]
    for label, mutated in mutations:
        try:
            judge_xcheck(mutated)
        except SystemExit:
            continue
        fail(f"control: {label} was accepted by the xcheck judge")
    return len(mutations)


def run_xcheck() -> list[str]:
    manifest = XCHECK_CRATE / "Cargo.toml"
    if not manifest.is_file():
        fail(f"{manifest.relative_to(ROOT)} does not exist; the cross-check harness has not landed")
    command = [
        "cargo", "run", "--release", "--locked", "--manifest-path", str(manifest),
        "--target-dir", str(ROOT / "build" / "zenoh-xcheck-target"), "--", str(XCHECK_SWEEP),
    ]
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if result.returncode != 0:
        _sys.stderr.write((result.stdout + result.stderr)[-4000:])
        fail(f"{' '.join(command)} exited {result.returncode}")
    return [match.group(1).strip() for line in result.stdout.splitlines() if (match := XCHECK_LINE.search(line))]


def xcheck_main(controls_only: bool) -> int:
    refused_controls = xcheck_controls()
    if controls_only:
        print(f"zenoh profile 0 xcheck judge: refused {refused_controls} corrupted transcripts")
        return 0
    cases, refusals = judge_xcheck(run_xcheck())
    print(f"[zenoh-profile0] slice=xcheck cases={cases} refusals={refusals} controls={refused_controls}")
    devloop_observations.record(casesObserved=cases, negativeControlsRefused=refused_controls)
    print("zenoh profile 0 xcheck: the codec and session agree with the upstream eclipse-zenoh 1.0.0 crates")
    return 0


def stock_fixtures() -> list[tuple[str, str]]:
    """The captured batches as `group/name`, after tying the pushes to the demo contract."""
    rows = [(f"{group}/{name}", hexed) for group in ("connects", "listens", "refuse") for name, hexed in zenoh_stock_peer.batches(group)]
    names = [name for name, _ in rows]
    if len(set(names)) != len(names):
        fail("stock fixtures: a fixture name is used twice")
    by_name = dict(rows)
    samples = zenoh_exchange.demo()["samples"]
    if len(STOCK_PUSHES) != len(samples):
        fail("stock fixtures: the captured pushes do not match the demo's sample count")
    for push, (sequence, _value, payload) in zip(STOCK_PUSHES, samples, strict=True):
        raw = bytes.fromhex(by_name.get(push, ""))
        if len(raw) < 12 + 1 + 33 or raw[-12:].hex() != payload:
            fail(f"stock fixtures: {push} does not end in the demo payload for sample {sequence}")
        attachment = raw[-12 - 1 - 33 : -12 - 1]
        if int.from_bytes(attachment[:8], "little") != sequence or attachment[16] != 16:
            fail(f"stock fixtures: {push} has attachment sequence/GID length differing from sample {sequence}")
        if raw[-13] != 12 or raw[-47] != 33:
            fail(f"stock fixtures: {push} does not carry the 12-byte payload and 33-byte attachment lengths")
    for name, hexed in rows:
        try:
            bytes.fromhex(hexed)
        except ValueError:
            fail(f"stock fixtures: {name} is not hexadecimal")
    return rows


def stock_item_pins() -> tuple[int, int]:
    """The case count and the control floor the admitted stock-peer item fixes, read from its stored
    body, so the number the checker reports and the number the acceptance requires cannot drift."""
    items = sorted((ROOT / ".tasks" / "items").glob("*.md"))
    for path in items:
        text = path.read_text()
        if "zenoh_stock_check" not in text or "slime-zenoh-profile0-stock-peer-" not in text:
            continue
        cases = re.search(r'id = "all-cases-observed";.*?intValue = (\d+);', text, re.S)
        floor = re.search(r'observation = "negativeControlsRefused";.*?lower = (\d+);', text, re.S)
        if cases and floor:
            return int(cases.group(1)), int(floor.group(1))
    return 0, 0


def stock_attachments() -> list[tuple[int, int, str]]:
    """Sequence, source timestamp and GID of each captured push, read from the captured bytes."""
    by_name = {f"{group}/{name}": hexed for group in ("connects", "listens") for name, hexed in zenoh_stock_peer.batches(group)}
    rows = []
    for push in STOCK_PUSHES:
        raw = bytes.fromhex(by_name[push])
        attachment = raw[-12 - 1 - 33 : -12 - 1]
        rows.append((int.from_bytes(attachment[:8], "little"), int.from_bytes(attachment[8:16], "little", signed=True), attachment[17:].hex()))
    return rows


def judge_stock_corpus(text: str) -> int:
    """The corpus the stock-peer change may not widen: the refused vectors outside
    STOCK_MAY_CHANGE keep their name, class and bytes, and the accepted batches keep their
    name, bytes and summary. A vector may be added; none may be duplicated."""
    refused: dict[str, tuple[str, str]] = {}
    accepted: dict[str, tuple[str, str]] = {}
    streams: dict[str, list[str]] = {}
    for number, line in enumerate(text.splitlines(), 1):
        fields = line.split()
        if fields[:1] == ["stream"] and len(fields) >= 3:
            if fields[1] in streams or fields[1] in refused or fields[1] in accepted:
                fail(f"stock corpus: line {number}: vector {fields[1]} appears twice")
            streams[fields[1]] = fields[2:]
            continue
        if len(fields) != 5 or fields[0] != "batch":
            continue
        name, kind = fields[1], fields[2]
        if kind == "refused":
            table = refused
        elif kind == "ok":
            table = accepted
        else:
            continue
        if name in refused or name in accepted or name in streams:
            fail(f"stock corpus: line {number}: vector {name} appears twice")
        table[name] = (fields[3], fields[4])
    missing = sorted(set(STOCK_FIXED_NAMES) - set(refused))
    if missing:
        fail(f"stock corpus: refused vectors removed or renamed: {', '.join(missing[:6])}")
    digest = hashlib.sha256(
        "\n".join(f"{name} {refused[name][0]} {refused[name][1]}" for name in sorted(STOCK_FIXED_NAMES)).encode()
    ).hexdigest()
    if digest != STOCK_FIXED_DIGEST:
        fail(f"stock corpus: a refused vector outside the stock-peer change has a different class or bytes (digest {digest[:16]}, pinned {STOCK_FIXED_DIGEST[:16]})")
    absent = sorted(set(STOCK_ACCEPTED_NAMES) - set(accepted))
    if absent:
        fail(f"stock corpus: accepted upstream batches removed or renamed: {', '.join(absent[:6])}")
    # A vector the stock-peer change turns from refused to accepted keeps its name and may stay as
    # a positive vector, and a positive vector for a captured stock batch may be added under a
    # stock_ name when its bytes are exactly that batch's. Any other accepted name is an
    # invention, not the upstream corpus.
    captured = {hexed for group in ("connects", "listens") for _, hexed in zenoh_stock_peer.batches(group)}
    extra = sorted(
        name
        for name in set(accepted) - set(STOCK_ACCEPTED_NAMES) - STOCK_MAY_CHANGE
        if not (name.startswith("stock_") and accepted[name][0] in captured)
    )
    if extra:
        fail(f"stock corpus: accepted batches outside the upstream corpus, the stock-peer change and the captured batches: {', '.join(extra[:6])}")
    accepted_digest = hashlib.sha256(
        "\n".join(f"{name} {accepted[name][0]} {accepted[name][1]}" for name in sorted(STOCK_ACCEPTED_NAMES)).encode()
    ).hexdigest()
    if accepted_digest != STOCK_ACCEPTED_DIGEST:
        fail(f"stock corpus: an accepted upstream batch has different bytes or summary (digest {accepted_digest[:16]}, pinned {STOCK_ACCEPTED_DIGEST[:16]})")
    missing_streams = sorted(set(STOCK_STREAM_NAMES) - set(streams))
    if missing_streams:
        fail(f"stock corpus: stream vectors removed or renamed: {', '.join(missing_streams[:6])}")
    added_streams = sorted(set(streams) - set(STOCK_STREAM_NAMES))
    if added_streams:
        fail(f"stock corpus: stream vectors outside the pinned set: {', '.join(added_streams[:6])}")
    stream_digest = hashlib.sha256(
        "\n".join(f"{name} {' '.join(streams[name])}" for name in sorted(STOCK_STREAM_NAMES)).encode()
    ).hexdigest()
    if stream_digest != STOCK_STREAM_DIGEST:
        fail(f"stock corpus: a stream-framing vector has a different verdict, class or chunks (digest {stream_digest[:16]}, pinned {STOCK_STREAM_DIGEST[:16]})")
    return len(STOCK_FIXED_NAMES) + len(STOCK_ACCEPTED_NAMES) + len(STOCK_STREAM_NAMES)


def stock_expected() -> tuple[tuple[str, ...], tuple[tuple[str, str], ...], tuple[str, ...]]:
    return zenoh_stock_peer.OK_SCENARIOS, zenoh_stock_peer.REFUSED_SCENARIOS, zenoh_stock_peer.replay_names()


def judge_stock(lines: list[str]) -> tuple[int, int]:
    ok_wanted, refused_wanted, replay_wanted = stock_expected()
    split = [line.split() for line in lines]
    done = [f[2] for f in split if len(f) == 3 and f[0] == "stock" and f[1] == "ok"]
    replays = [f[2] for f in split if len(f) == 4 and f[:2] == ["stock", "replay"] and f[3] == "ok"]
    refused = [(f[2], f[3]) for f in split if len(f) == 4 and f[:2] == ["stock", "refused"]]
    pushes = [(f[2], f[3]) for f in split if len(f) == 4 and f[:2] == ["stock", "attachment"]]
    attachments = stock_attachments()
    expected_pushes = [(f"{sequence}:{timestamp}", gid) for sequence, timestamp, gid in attachments]
    if pushes != expected_pushes:
        fail(f"stock attachments decoded {pushes}, the captured pushes carry {expected_pushes}")
    delivered = [(f[2], f[3], f[4]) for f in split if len(f) == 5 and f[:2] == ["stock", "delivered"]]
    wanted = [(str(sequence), payload, str(attachments[index][1])) for index, (sequence, _value, payload) in enumerate(zenoh_exchange.demo()["samples"])]
    if [seq for seq, _, _ in delivered] != [seq for seq, _, _ in wanted]:
        fail(f"stock delivered samples {[seq for seq, _, _ in delivered]}, the demo contract has {[seq for seq, _, _ in wanted]}")
    for (seq, got, stamp), (_, payload, source) in zip(delivered, wanted, strict=True):
        if got != payload:
            fail(f"stock delivered sample {seq}: {got}, the demo contract has {payload}")
        if stamp != source:
            fail(f"stock delivered sample {seq}: timestamp {stamp}, the captured attachment has {source}")
    exactly("stock scenarios", ok_wanted, done)
    if done != list(ok_wanted):
        fail(f"stock scenarios are out of order: {done}, the exam runs them as {list(ok_wanted)}")
    exactly("stock replays", replay_wanted, [f"replay:{name}" for name in replays])
    for group in ("connects", "listens"):
        seen = [name for name in replays if name.startswith(f"{group}-")]
        order = [name[len("replay:") :] for name in replay_wanted if name.startswith(f"replay:{group}-")]
        if seen != order:
            fail(f"stock replays of {group} are out of capture order: {seen}, captured as {order}")
    exactly("stock refusals", tuple(name for name, _ in refused_wanted), [name for name, _ in refused])
    wrong = [f"{name}={cls} (expected {dict(refused_wanted)[name]})" for name, cls in refused if cls != dict(refused_wanted)[name]]
    if wrong:
        fail(f"stock refusal classes differ: {', '.join(wrong)}")
    shapes = {
        ("stock", "ok"): 3,
        ("stock", "replay"): 4,
        ("stock", "refused"): 4,
        ("stock", "delivered"): 5,
        ("stock", "attachment"): 4,
    }
    for line, fields in zip(lines, split, strict=True):
        if fields[:1] != ["stock"]:
            fail(f"stock: a record that is not a stock exam line: {line!r}")
        kind = tuple(fields[:2])
        if kind not in shapes:
            fail(f"stock: unrecognised line: {line}")
        if len(fields) != shapes[kind]:
            fail(f"stock: a {kind[1]} line has {len(fields)} fields, expected {shapes[kind]}: {line}")
        if kind == ("stock", "replay") and fields[3] != "ok":
            fail(f"stock: a replay line must end in ok: {line}")
    # The attachment lines are validated above but are not cases: the item fixes the count at the
    # scenarios, replays and delivered samples plus the refusals.
    return len(done) + len(replays) + len(delivered), len(refused)


def stock_good_lines() -> list[str]:
    ok_wanted, refused_wanted, replay_wanted = stock_expected()
    return (
        [f"stock ok {name}" for name in ok_wanted]
        + [f"stock replay {name[len('replay:'):]} ok" for name in replay_wanted]
        + [f"stock delivered {sequence} {payload} {stock_attachments()[index][1]}" for index, (sequence, _value, payload) in enumerate(zenoh_exchange.demo()["samples"])]
        + [f"stock attachment {sequence}:{timestamp} {gid}" for sequence, timestamp, gid in stock_attachments()]
        + [f"stock refused {name} {cls}" for name, cls in refused_wanted]
    )


def first_replay_index(lines: list[str], prefix: str) -> int:
    return next(i for i, line in enumerate(lines) if line.startswith(f"stock replay {prefix}"))


def swap_lines(lines: list[str], first: int, second: int) -> list[str]:
    swapped = list(lines)
    swapped[first], swapped[second] = swapped[second], swapped[first]
    return swapped


def stock_handoff_controls() -> int:
    """The contract's decoder accepts the honest hand-off and refuses each corrupted one."""
    import tempfile

    honest = zenoh_stock_peer.fixture_zti()
    mutations = [
        ("a missing group", honest.replace("  refuse = [", "  refuze = [")),
        ("a wrong format version", honest.replace("formatVersion = 1;", "formatVersion = 2;")),
        ("a group of the wrong size", honest.replace('    { name = "connects-11-close"; hex = "0300"; };\n', "", 1)),
        ("a hex string with an odd length", honest.replace('hex = "0300"', 'hex = "030"', 1)),
        ("a hex string in capitals", honest.replace('hex = "0300"', 'hex = "03AB"', 1)),
        ("a cookie of the wrong length", re.sub(r'cookieHex = "[0-9a-f]{2}', 'cookieHex = "', honest, count=1)),
        ("a row with no hex", honest.replace('hex = "', 'hxx = "', 1)),
        ("a hex that is not text", honest.replace('hex = "', "hex = 7; zz = \"", 1)),
        ("an unknown extra field", honest.replace("formatVersion = 1;", "formatVersion = 1;\n  surprise = 1;")),
    ]
    (ROOT / "build").mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(dir=ROOT / "build") as scratch:
        path = _Path(scratch) / "handoff.zti"
        path.write_text(honest)
        stock_validate_handoff(path)
        for label, text in mutations:
            path.write_text(text)
            try:
                stock_validate_handoff(path)
            except SystemExit:
                continue
            fail(f"control: {label} was accepted by the fixture contract")
    return len(mutations)


def stock_controls() -> int:
    """Refuse corrupted or hollowed-out stock evidence before trusting a real transcript."""
    stock_fixtures()
    good = stock_good_lines()
    judge_stock(good)

    def swap(prefix: str, new: str) -> list[str]:
        at = next(i for i, line in enumerate(good) if line.startswith(prefix))
        return good[:at] + [new] + good[at + 1 :]

    first_ok = good[0]
    first_replay = next(line for line in good if line.startswith("stock replay "))
    first_refused = next(line for line in good if line.startswith("stock refused "))
    mutations: list[tuple[str, list[str]]] = [
        ("an empty transcript", []),
        ("a dropped scenario", [line for line in good if line != first_ok]),
        ("a duplicated scenario", good + [first_ok]),
        ("an unknown scenario", good + ["stock ok invented-scenario"]),
        ("a dropped replay", [line for line in good if line != first_replay]),
        ("a duplicated replay", good + [first_replay]),
        ("a replay of a batch that was never captured", good + ["stock replay stock-connects-99-invented ok"]),
        ("a replay reported refused", [("stock refused " + line.split()[2] + " unsupported-extension") if line == first_replay else line for line in good]),
        ("every refusal removed", [line for line in good if not line.startswith("stock refused ")]),
        ("a dropped refusal", [line for line in good if line != first_refused]),
        ("a refusal with the wrong class", swap("stock refused refuse-wildcard", "stock refused refuse-wildcard unsupported-message")),
        ("a refusal with no class", swap("stock refused refuse-client", "stock refused refuse-client")),
        ("a stock peer that was accepted as a client", [line for line in good if not line.startswith("stock refused refuse-client")] + ["stock ok refuse-client"]),
        ("an unrecognised line kind", good + ["stock maybe silence-past-lease-closes"]),
        ("a refusal line with no class", good + ["stock refused refuse-client"]),
        ("a replay line with no verdict", good + ["stock replay stock-connects-01-init-syn"]),
        ("a replay line with an extra field", good + [first_replay + " extra"]),
        ("a replay line whose verdict is not ok", good + [first_replay.rsplit(" ", 1)[0] + " refused"]),
        ("an ok line with an extra field", good + [first_ok + " and-more"]),
        ("a bare stock line", good + ["stock"]),
        ("a record that is not a stock line", good + ["session ok invented"]),
        ("a record from another slice", good + ["encode ok somebatch aabb"]),
        ("free text among the stock lines", good + ["random noise"]),
        ("two scenarios reported in the other order", swap_lines(good, good.index(f"stock ok {zenoh_stock_peer.OK_SCENARIOS[0]}"), good.index(f"stock ok {zenoh_stock_peer.OK_SCENARIOS[1]}"))),
        ("the last scenario reported first", [f"stock ok {zenoh_stock_peer.OK_SCENARIOS[-1]}"] + [line for line in good if line != f"stock ok {zenoh_stock_peer.OK_SCENARIOS[-1]}"]),
        ("a delivered sample dropped", [line for line in good if not line.startswith("stock delivered 3 ")]),
        ("a delivered sample duplicated", good + [next(line for line in good if line.startswith("stock delivered 0 "))]),
        ("a delivered sample out of order", [line for line in good if not line.startswith("stock delivered")] + [next(line for line in good if line.startswith(f"stock delivered {n} ")) for n in (1, 0, 2, 3)]),
        ("a delivered payload that differs from the demo's", swap("stock delivered 2 ", f"stock delivered 2 00010000020000001e000001 {stock_attachments()[2][1]}")),
        ("a delivered timestamp that differs from the captured attachment's", swap("stock delivered 1 ", f"stock delivered 1 000100000100000014000000 {stock_attachments()[1][1] + 1}")),
        ("a decoded attachment GID with one byte changed", swap("stock attachment 2:", f"stock attachment 2:{stock_attachments()[2][1]} " + stock_attachments()[2][2][:-2] + "00")),
        ("a decoded attachment with the wrong sequence", swap("stock attachment 1:", f"stock attachment 9:{stock_attachments()[1][1]} {stock_attachments()[1][2]}")),
        ("a decoded attachment dropped", [line for line in good if not line.startswith("stock attachment 3:")]),
        ("a decoded attachment with no GID", swap("stock attachment 0:", f"stock attachment 0:{stock_attachments()[0][1]}")),
        ("a delivered line with no timestamp", swap("stock delivered 0 ", "stock delivered 0 00010000000000000a000000")),
        ("the first two replays of the stock-connects capture swapped", swap_lines(good, first_replay_index(good, "connects-01-"), first_replay_index(good, "connects-02-"))),
        ("the first two replays of the stock-listens capture swapped", swap_lines(good, first_replay_index(good, "listens-01-"), first_replay_index(good, "listens-02-"))),
        ("the close replayed before the handshake it ends", swap_lines(good, first_replay_index(good, "connects-01-"), first_replay_index(good, "connects-11-"))),
        ("the lease-silence scenario missing", [line for line in good if line != "stock ok silence-past-lease-closes"]),
        ("the 2000 ms keep-alive scenario missing", [line for line in good if line != "stock ok keep-alive-sent-lease-2000"]),
        ("the 10000 ms keep-alive scenario missing", [line for line in good if line != "stock ok keep-alive-sent-lease-10000"]),
    ]
    for label, mutated in mutations:
        try:
            judge_stock(mutated)
        except SystemExit:
            continue
        fail(f"control: {label} was accepted by the stock judge")
    handoff_refused = stock_handoff_controls()
    corpus_text = STOCK_CORPUS.read_text() if STOCK_CORPUS.is_file() else ""
    if corpus_text:
        judge_stock_corpus(corpus_text)
        def drop(name: str) -> str:
            return "\n".join(line for line in corpus_text.splitlines() if not line.startswith((f"batch {name} ", f"stream {name} ")))

        def restream(name: str, position: int, value: str) -> str:
            return "\n".join(
                " ".join(line.split()[:position] + [value] + line.split()[position + 1 :])
                if line.startswith(f"stream {name} ")
                else line
                for line in corpus_text.splitlines()
            )
        def retarget(name: str, cls: str) -> str:
            return "\n".join(
                " ".join(line.split()[:3] + [cls] + line.split()[4:]) if line.startswith(f"batch {name} ") else line
                for line in corpus_text.splitlines()
            )
        def rebytes(name: str, accepted: bool = False) -> str:
            column = 3 if accepted else 4
            return "\n".join(
                " ".join(
                    line.split()[:column]
                    + [line.split()[column][:-2] + ("00" if line.split()[column][-2:] != "00" else "01")]
                    + line.split()[column + 1 :]
                )
                if line.startswith(f"batch {name} ")
                else line
                for line in corpus_text.splitlines()
            )
        first_accepted = next(line.split()[1] for line in corpus_text.splitlines() if line.startswith("batch ") and line.split()[2:3] == ["ok"])
        corpus_mutations = [
            ("a fixed refused vector removed", drop(STOCK_FIXED_NAMES[0])),
            ("a fixed refused vector reclassified", retarget(STOCK_FIXED_NAMES[1], "invented-class")),
            ("a fixed refused vector with different bytes", rebytes(STOCK_FIXED_NAMES[2])),
            ("a router peer vector reclassified", retarget("init_whatami_router", "unsupported-message")),
            ("an accepted upstream batch removed", drop(first_accepted)),
            ("an accepted upstream batch with different bytes", rebytes(first_accepted, accepted=True)),
            ("an accepted batch dropped and a renamed copy of another added", drop(first_accepted) + "\n" + next(line for line in corpus_text.splitlines() if line.startswith("batch ") and line.split()[2:3] == ["ok"] and line.split()[1] != first_accepted).replace(next(line for line in corpus_text.splitlines() if line.startswith("batch ") and line.split()[2:3] == ["ok"] and line.split()[1] != first_accepted).split()[1], first_accepted, 1)),
            ("an accepted batch repeated", corpus_text + "\n" + next(line for line in corpus_text.splitlines() if line.startswith(f"batch {first_accepted} "))),
            ("an accepted batch the upstream encoder did not produce", corpus_text + "\nbatch invented_by_the_implementation ok 00 invented"),
            ("an unobserved extension position now accepted (frame_extension_present)", retarget("frame_extension_present", "ok")),
            ("an unobserved extension position removed (push_extension_flag)", drop("push_extension_flag")),
            ("a fixed refused vector turned into an accepted one (init_whatami_router)", retarget("init_whatami_router", "ok")),
            ("an invented accepted vector outside the upstream corpus and the stock-peer change", corpus_text + "\nbatch invented_by_the_implementation ok 00 x"),
            ("every stream-framing row deleted", "\n".join(line for line in corpus_text.splitlines() if not line.startswith("stream "))),
            ("the 512-byte stream refusal turned into an accepted one (stream_length_513)", restream("stream_length_513", 2, "ok")),
            ("a stream refusal reclassified (stream_length_zero)", restream("stream_length_zero", 3, "invented-class")),
            ("a stream refusal removed (stream_refusal_after_valid_batch)", drop("stream_refusal_after_valid_batch")),
            ("an accepted stream row with other chunks (stream_single_batch)", restream("stream_single_batch", 3, "00")),
            ("an invented stream row", corpus_text + "\nstream invented_stream_row ok 00 x 0"),
            ("a vector with a stock-sounding name turned accepted (declare_extension_flag)", retarget("declare_extension_flag", "ok")),
            ("a vector with a stock-sounding name turned accepted (declare_keyexpr_alias)", retarget("declare_keyexpr_alias", "ok")),
            ("a vector with a stock-sounding name turned accepted (frame_network_oam)", retarget("frame_network_oam", "ok")),
            ("a vector with a stock-sounding name turned accepted (dsub_nonzero_scope)", retarget("dsub_nonzero_scope", "ok")),
            ("a vector with a stock-sounding name turned accepted (push_no_suffix)", retarget("push_no_suffix", "ok")),
            ("a stock_ row whose bytes are not a captured batch", corpus_text + "\nbatch stock_invented ok 00ff invented"),
            ("a captured batch's bytes under a name that is not stock_", corpus_text + f"\nbatch invented_name ok {zenoh_stock_peer.batches('connects')[0][1]} invented"),
            ("a batch row reusing a stream row's name", corpus_text + "\nbatch stream_single_batch refused empty-batch -"),
            ("a stream row reusing a batch row's name", corpus_text + "\nstream init_syn_default ok 00 x 0"),
            ("a batch row reusing an accepted batch's name", corpus_text + f"\nbatch {first_accepted} refused empty-batch -"),
        ]
        for label, mutated in corpus_mutations:
            try:
                judge_stock_corpus(mutated)
            except SystemExit:
                continue
            fail(f"control: {label} was accepted by the stock corpus judge")
        # A vector the stock peer sends may change, go, or be kept as an accepted vector: the exam
        # must not stand in the way of the item, whichever of those it chooses.
        judge_stock_corpus(drop(sorted(STOCK_MAY_CHANGE)[0]))
        for name in sorted(STOCK_MAY_CHANGE):
            judge_stock_corpus("\n".join(" ".join(line.split()[:2] + ["ok"] + line.split()[3:]) if line.startswith(f"batch {name} ") else line for line in corpus_text.splitlines()))
        first_captured = zenoh_stock_peer.batches("connects")[0][1]
        judge_stock_corpus(corpus_text + f"\nbatch stock_init_syn ok {first_captured} INIT_SYN;captured")
        extra = len(corpus_mutations)
    else:
        extra = 0
    # The fixtures themselves are held to the demo contract: a push whose payload is changed is refused.
    original = zenoh_stock_peer.CONNECTS
    try:
        broken = list(original)
        index = next(i for i, (name, _h) in enumerate(broken) if name == STOCK_PUSHES[0].split("/", 1)[1])
        name, hexed = broken[index]
        broken[index] = (name, hexed[:-2] + ("00" if hexed[-2:] != "00" else "01"))
        zenoh_stock_peer.CONNECTS = tuple(broken)
        try:
            stock_fixtures()
        except SystemExit:
            return len(mutations) + 1 + extra + handoff_refused
        fail("control: a capture whose push payload differs from the demo sample was accepted")
    finally:
        zenoh_stock_peer.CONNECTS = original


def stock_validate_handoff(path: _Path) -> None:
    """Decode the hand-off with the contract's own decoder; the Rust tests read the same file."""
    # Zutai imports stay inside the project, so the decoder and a copy of the schema sit in one
    # scratch directory under build/, rebuilt from the contract on every call.
    scratch = ZENOH_DECODER.parent
    scratch.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(ROOT / "contracts" / "zenoh-stock-fixtures" / "v1" / "schema.zt", scratch / "schema.zt")
    ZENOH_DECODER.write_text(
        's ::= import "schema.zt"; env ::= import stdlib.env; load ::= import stdlib.load; '
        "main :: { env : Env; load : Load; } -> Validation DecodeIssue s.Capture ! { * env.GetEffects; * load.ZtiEffects; } "
        '= caps => [ path := env.get caps.env "SLIME_ZENOH_STOCK_FIXTURES_PATH" ?? ""; s.decodeCapture (load.zti caps.load path) ]; main\n'
    )
    try:
        decoded = zutai_cli.evaluate("run", ZENOH_DECODER, input_path=path, env_var="SLIME_ZENOH_STOCK_FIXTURES_PATH")
    except zutai_cli.ZutaiError as error:
        fail(f"stock hand-off does not decode under contracts/zenoh-stock-fixtures/v1: {error}")
    if not decoded.startswith("#valid"):
        fail(f"stock hand-off is refused by contracts/zenoh-stock-fixtures/v1: {decoded[:200]}")
    # The decoder fixes shape and types; the values the schema names are held here, as the other
    # contracts' exams do: the version, the counts per group, the cookie, and the bounds.
    text = path.read_text()
    fields = re.findall(r"^  (\w+) = ", text, re.M)
    if sorted(fields) != sorted(["formatVersion", "cookieHex", "connects", "listens", "refuse"]):
        fail(f"stock hand-off: fields {sorted(fields)} differ from the contract's")
    if not re.search(r"^  formatVersion = 1;$", text, re.M):
        fail("stock hand-off: formatVersion is not 1")
    for group, count in (("connects", 11), ("listens", 8), ("refuse", 5)):
        block = re.search(rf"^  {group} = \[\n(.*?)^  \];", text, re.M | re.S)
        rows = re.findall(r'\{ name = "([^"]*)"; hex = "([^"]*)"; \};', block.group(1)) if block else []
        if len(rows) != count:
            fail(f"stock hand-off: {group} has {len(rows)} rows, the contract fixes {count}")
        for name, hexed in rows:
            if len(name) > 64 or len(hexed) % 2 or len(hexed) // 2 > 512 or not re.fullmatch(r"[0-9a-f]*", hexed):
                fail(f"stock hand-off: {group}/{name} breaks the contract's bounds or is not lowercase hexadecimal")
    cookie = re.search(r'^  cookieHex = "([0-9a-f]*)";$', text, re.M)
    if not cookie or len(cookie.group(1)) != 58:
        fail("stock hand-off: cookieHex is not the 29-byte cookie")


def run_stock() -> list[str]:
    stock_fixtures()
    if not STOCK_CORPUS.is_file():
        fail(f"{STOCK_CORPUS.relative_to(ROOT)} is missing")
    judge_stock_corpus(STOCK_CORPUS.read_text())
    STOCK_FIXTURES.parent.mkdir(parents=True, exist_ok=True)
    STOCK_FIXTURES.write_text(zenoh_stock_peer.fixture_zti())
    stock_validate_handoff(STOCK_FIXTURES)
    host = subprocess.run(["rustc", "-vV"], capture_output=True, text=True, check=True)
    target = next(line.split(": ", 1)[1] for line in host.stdout.splitlines() if line.startswith("host: "))
    command = [
        "cargo", "test", "--target", target, "-p", "slime-components", "--no-default-features",
        "--", STOCK_FILTER, STOCK_CORPUS_TEST, "--nocapture", "--test-threads=1",
    ]
    environment = {**_os.environ, "ZENOH_STOCK_FIXTURES": str(STOCK_FIXTURES)}
    result = subprocess.run(command, cwd=ROOT / "components", capture_output=True, text=True, env=environment)
    output = result.stdout + result.stderr
    if result.returncode != 0:
        _sys.stderr.write(output[-4000:])
        fail(f"{' '.join(command)} exited {result.returncode}")
    results = re.findall(r"test result: (\w+)\. (\d+) passed; (\d+) failed", output)
    if not any(status == "ok" and int(passed) > 0 for status, passed, _ in results) or any(
        status != "ok" or int(failed) for status, _, failed in results
    ):
        fail(f"no passing test result for filter {STOCK_FILTER!r}")
    # A name filter that matches nothing passes silently, so the corpus test is required by name.
    if not re.search(rf"^test {re.escape(STOCK_CORPUS_TEST)} \.\.\. ok$", output, re.M):
        fail(f"{STOCK_CORPUS_TEST} did not run and pass: the decoder was not held to the pinned corpus")
    return exam_lines(result.stdout)


def stock_main(controls_only: bool) -> int:
    refused_controls = stock_controls()
    pinned_cases, pinned_floor = stock_item_pins()
    positive, refusals_total = judge_stock(stock_good_lines())
    if pinned_cases and positive + refusals_total != pinned_cases:
        fail(f"stock: an honest transcript is {positive + refusals_total} cases, the admitted item fixes {pinned_cases}")
    if pinned_floor and refused_controls < pinned_floor:
        fail(f"stock: {refused_controls} controls, the admitted item requires at least {pinned_floor}")
    if controls_only:
        print(f"zenoh profile 0 stock judge: refused {refused_controls} corrupted transcripts")
        return 0
    cases, refusals = judge_stock(run_stock())
    print(f"[zenoh-profile0] slice=stock cases={cases} refusals={refusals} controls={refused_controls}")
    devloop_observations.record(casesObserved=cases + refusals, negativeControlsRefused=refused_controls)
    print("zenoh profile 0 stock check: the session took every batch a stock Zenoh peer sent and refused what a bounded profile must")
    return 0


def composition_main(controls_only: bool) -> int:
    contract = zenoh_exchange.demo()
    try:
        facts, refused = zenoh_exchange.composition_controls(contract)
    except zenoh_exchange.ExchangeError as error:
        fail(str(error))
    if controls_only:
        print(f"zenoh composition judge: {facts} authority facts on an honest composition, {refused} mutations refused")
        return 0
    if not COMPOSITION.is_file():
        fail(f"{COMPOSITION.relative_to(ROOT)} does not exist; the sel4-zenoh composition has not landed")
    try:
        verified = zenoh_exchange.check_composition_rows(*zenoh_exchange.composition_from_text(COMPOSITION.read_text()), contract)
    except zenoh_exchange.ExchangeError as error:
        fail(str(error))
    print(f"[zenoh-profile0] slice=composition facts={verified} controls={refused}")
    devloop_observations.record(casesObserved=verified, negativeControlsRefused=refused)
    print("zenoh profile 0 composition check: the derived composition grants the nodes exactly the demo's authority")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--slice", choices=(*JUDGES, "composition", "xcheck", "stock"), required=True)
    parser.add_argument("--controls-only", action="store_true")
    arguments = parser.parse_args()
    if arguments.slice == "composition":
        return composition_main(arguments.controls_only)
    if arguments.slice == "xcheck":
        return xcheck_main(arguments.controls_only)
    if arguments.slice == "stock":
        return stock_main(arguments.controls_only)
    refused_controls = controls(arguments.slice)
    if arguments.controls_only:
        print(f"zenoh profile 0 {arguments.slice} judge: refused {refused_controls} corrupted transcripts")
        return 0
    cases, refusals = JUDGES[arguments.slice](run_tests(arguments.slice))
    print(f"[zenoh-profile0] slice={arguments.slice} cases={cases} refusals={refusals} controls={refused_controls}")
    devloop_observations.record(casesObserved=cases, negativeControlsRefused=refusals)
    print(f"zenoh profile 0 {arguments.slice} check: every case matched independent bytes and every refusal was observed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
