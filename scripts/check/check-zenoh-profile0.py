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

`--slice composition` judges the derived `sel4-zenoh` composition against the
demo contract's authority and refuses 15 mutations of an honest one. It is the
static half of the QEMU arm; the boot is `check-sel4-io-network-plane.py --arm zenoh`.
"""

from __future__ import annotations

import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import re
import struct
import subprocess
from collections.abc import Callable
from typing import NoReturn

import devloop_observations
import zenoh_exchange
import zenoh_wire
from harness import GENERATION_COMPOSITIONS, ROOT

CORPUS = ROOT / "components" / "lib" / "src" / "zenoh_profile0" / "vectors.txt"
COMPOSITION = GENERATION_COMPOSITIONS / "sel4-zenoh.zti"
EXAM_LINE = re.compile(r"\[zenoh-exam\] (.*)$")

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
    parser.add_argument("--slice", choices=(*JUDGES, "composition", "xcheck"), required=True)
    parser.add_argument("--controls-only", action="store_true")
    arguments = parser.parse_args()
    if arguments.slice == "composition":
        return composition_main(arguments.controls_only)
    if arguments.slice == "xcheck":
        return xcheck_main(arguments.controls_only)
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
