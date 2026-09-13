#!/usr/bin/env python3
"""C8.11: the bounded, deterministic semantic trace, on every timed fabric worker.

Why three planes. C8.11's tie order is defined over data, acknowledgement, peer
death, and time, so it is only testable where all four actually occur — which
means a plane whose generation grants a clock. Three do, one per timed worker:
`sel4-qos` drives the stream worker, `sel4-call` the call worker, and
`sel4-operation` the operation worker. Each is booted and each worker's own trace
is validated separately, because each worker holds its own sink.

Covering all three is not thoroughness for its own sake. An earlier revision of
this gate read only the stream worker, and that scoping hid two real defects: the
call and operation brokers emitted their peer-death record carrying their *own*
plane's `STATUS_PEER_DEAD`, which is a positive protocol enumerator, while a fault
record must carry a failure status — so every peer-death record on those two
workers was silently refused by its own validator and the transcript merely lacked
a line. The stream worker happened to pass a negative code and looked fine. A gate
that reads one worker cannot see that class of bug at all.

What this adds over the per-plane gates. `sel4_qos_check`, `sel4_call_check`, and
`sel4_operation_check` assert the *policy* is enforced. This asserts the *evidence
stream* is trustworthy: every record structurally well formed, records in the
declared `(now_ns, order_class, sequence)` order, the sink inside the depth its
generation declared, nothing rejected by its own validator, and the mandatory
terminal present. Those are separable failures — a plane can enforce every policy
correctly and still emit an unusable trace.

Determinism is checked per plane by booting twice and comparing the record lines
verbatim. Two boots of one fixed generation must produce byte-identical evidence;
if they do not, the trace depends on scheduling and cannot serve as the comparison
baseline C8.11 promises.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from fabric_trace_contract import (  # noqa: E402
    FABRIC_TRACE_MAX_DEPTH,
    FABRIC_TRACE_MAX_ORDER_CLASS,
    FABRIC_TRACE_ORDER_ACK,
    FABRIC_TRACE_ORDER_DATA,
    FABRIC_TRACE_ORDER_PEER_DEATH,
    FABRIC_TRACE_ORDER_TIME,
    FABRIC_TRACE_RESOURCE_COMPLETE,
)
from harness import GENERATION_COMPOSITIONS, ROOT, load_script  # noqa: E402
from sel4_gate_markers import match_marker_contract  # noqa: E402


# One entry per timed worker: the plane gate that owns booting it, the worker name
# its trace lines carry, and the fixture whose declared `traceDepth` the running
# sink must report.
PLANES = (
    ("stream", "sel4_qos_plane_for_trace", "check/check-sel4-qos-plane.py", "sel4-qos"),
    ("call", "sel4_call_plane_for_trace", "check/check-sel4-call-plane.py", "sel4-call"),
    (
        "operation",
        "sel4_operation_plane_for_trace",
        "check/check-sel4-operation-plane.py",
        "sel4-operation",
    ),
)

# One record line, and the summary that closes a worker's trace.
RECORD = re.compile(
    r"\[trace\] (?P<worker>[a-z-]+) kind=(?P<kind>[a-z-]+) order=(?P<order>[a-z-]+) "
    r"now=(?P<now>\d+) route=(?P<route>[0-9a-f]{16}) correlation=(?P<correlation>\d+) "
    r"sequence=(?P<sequence>\d+) status=(?P<status>-?\d+) event=(?P<event>\d+) "
    r"high_water=(?P<high_water>\d+)(?P<flags>(?: terminal| dropped)*)"
)
SUMMARY = re.compile(
    r"\[trace\] (?P<worker>[a-z-]+) complete capacity=(?P<capacity>\d+) "
    r"records=(?P<records>\d+) dropped=(?P<dropped>\d+) rejected=(?P<rejected>\d+)"
)

# The declared tie order, as ranks. Each name maps to the schema-owned code it
# renders, rather than to a hand-written number: the ranks are the contract's
# vocabulary, and restating them here would let the gate and the format disagree
# about the very order the gate exists to check.
ORDER_RANKS = {
    "data": FABRIC_TRACE_ORDER_DATA,
    "ack": FABRIC_TRACE_ORDER_ACK,
    "peer-death": FABRIC_TRACE_ORDER_PEER_DEATH,
    "time": FABRIC_TRACE_ORDER_TIME,
}

# Every worker's trace must carry its route provisioning, its clock advances, its
# resource plus terminal accounting, and its peer-death observation -- all three
# of these scenarios kill a peer, so a worker that emits no fault record has lost
# evidence rather than had none to give.
#
# `fault` is required *per worker* for a specific reason: while it was merely
# admitted, the peer-death chain was satisfied by any one plane emitting it, and
# two of the three workers were in fact emitting none. Both had attached the
# record to a single observation path out of four racing ones.
REQUIRED_KINDS = {"route", "qos", "resource", "fault"}
# Families any worker may legitimately emit on these planes.
ADMITTED_KINDS = REQUIRED_KINDS | {"call", "operation", "denial"}

CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "a worker recorded its routes before any traffic",
        (
            r"\[trace\] [a-z-]+ kind=route order=data now=0 route=[0-9a-f]{16} ",
            r"\[trace\] [a-z-]+ kind=qos order=time now=0 ",
        ),
    ),
    (
        # A fault must carry a *failure* status. This is the assertion the two
        # silently-refused peer-death records would have failed: each broker was
        # passing its own plane's `STATUS_PEER_DEAD`, a positive protocol
        # enumerator, so the record never reached the artifact at all.
        #
        # Only the status and shape are asserted here, not what follows. Whether
        # a clock advance closes the instant a peer died is a property of the
        # scenario -- on the call plane the death is at the final instant and
        # nothing follows it -- while the *arrangement* of records within an
        # instant is checked structurally by `check_order` for every plane.
        "peer death was recorded with a failure status",
        (
            r"\[trace\] [a-z-]+ kind=fault order=peer-death now=\d+ route=[0-9a-f]{16} "
            r"correlation=0 sequence=\d+ status=-\d+ ",
        ),
    ),
    (
        "a worker recorded a resource high-water count and then its terminal",
        (
            r"\[trace\] [a-z-]+ kind=resource order=data now=\d+ .* event=[1-3] ",
            rf"\[trace\] [a-z-]+ kind=resource order=time now=\d+ .* "
            rf"event={FABRIC_TRACE_RESOURCE_COMPLETE} .* terminal",
        ),
    ),
    (
        # A saturated or defective trace is reported, not inferred from absence.
        # `rejected=0` is the assertion the development defects would have failed.
        "the trace closed inside its bounds with nothing rejected",
        (
            r"\[trace\] [a-z-]+ complete capacity=\d+ records=\d+ dropped=0 rejected=0",
        ),
    ),
)

TERMINAL_MARKER = CHAINS[-1][1][-1]

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    # A nonzero rejected count means an emitter produced a record its own
    # validator refuses. The record is missing from the artifact, so without
    # this the trace looks merely shorter rather than wrong.
    r"\[trace\] [a-z-]+ complete capacity=\d+ records=\d+ dropped=\d+ rejected=[1-9]\d*",
    # Saturation is a bounded, reported condition, but on these planes the
    # declared depth comfortably exceeds the record count -- so a drop here
    # means the depth and the plane disagree.
    r"\[trace\] [a-z-]+ complete capacity=\d+ records=\d+ dropped=[1-9]\d*",
    r"<<seL4\(CPU 0\) \[decodeInvocation",
    r"unhandled",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 trace plane check: {message}")


def declared_depth(fixture: str) -> int:
    """The depth this plane's generation declares.

    Read from the fixture rather than restated here: restating it would make the
    gate agree with itself instead of with the generation, which is the
    disagreement worth catching.
    """
    text = (GENERATION_COMPOSITIONS / f"{fixture}.zti").read_text(encoding="utf-8")
    match = re.search(r"^\s*traceDepth = (\d+);", text, re.MULTILINE)
    if match is None:
        fail(f"{fixture}.zti declares no traceDepth")
    return int(match.group(1))


def records(transcript: str, worker: str) -> list[dict[str, str]]:
    return [
        match.groupdict()
        for match in RECORD.finditer(transcript)
        if match.group("worker") == worker
    ]


def summary(transcript: str, worker: str) -> dict[str, str]:
    matches = [
        match.groupdict()
        for match in SUMMARY.finditer(transcript)
        if match.group("worker") == worker
    ]
    if len(matches) != 1:
        fail(f"expected exactly one {worker} trace summary, found {len(matches)}")
    return matches[0]


def check_structure(parsed: list[dict[str, str]], worker: str) -> None:
    """Every record is well formed, and every family is an admitted one."""
    if not parsed:
        fail(f"the {worker} worker emitted no trace records at all")
    for record in parsed:
        if record["kind"] == "unknown" or record["order"] == "unknown":
            fail(f"record carries an unnamed kind or order class: {record}")
        if record["order"] not in ORDER_RANKS:
            fail(f"record names an undeclared order class: {record['order']!r}")
        if ORDER_RANKS[record["order"]] > FABRIC_TRACE_MAX_ORDER_CLASS:
            fail(f"order class rank exceeds the contract: {record['order']!r}")
        # The trace must name no task, address, or component identity. Enforced
        # positively: the line grammar admits only the contract's own fields, so
        # a record that parsed cannot carry one. What is checkable here is that a
        # record closing an instant names neither an edge nor a correlation.
        if record["order"] == "time" and record["route"] != "0" * 16:
            fail(f"a record closing an instant names an edge: {record}")
        if record["order"] == "time" and record["correlation"] != "0":
            fail(f"a record closing an instant names a correlation: {record}")
        # A fault must report a failure, not a positive protocol enumerator.
        if record["kind"] == "fault" and int(record["status"]) >= 0:
            fail(f"a fault record carries a non-failure status: {record}")
    kinds = {record["kind"] for record in parsed}
    missing = REQUIRED_KINDS - kinds
    if missing:
        fail(f"the {worker} trace is missing required families {sorted(missing)}")
    undeclared = kinds - ADMITTED_KINDS
    if undeclared:
        fail(f"the {worker} trace carries undeclared families {sorted(undeclared)}")


def check_order(parsed: list[dict[str, str]], worker: str) -> None:
    """The records are in the declared `(now_ns, order_class, sequence)` order."""
    previous = None
    for record in parsed:
        key = (
            int(record["now"]),
            ORDER_RANKS[record["order"]],
            int(record["sequence"]),
        )
        if previous is not None and key < previous:
            fail(
                f"{worker} record {record} violates the declared order: "
                f"{key} sorts before {previous}"
            )
        previous = key


def check_bounds(
    parsed: list[dict[str, str]], closing: dict[str, str], depth: int, worker: str
) -> None:
    """The sink held the declared depth, stayed inside it, and reported no loss."""
    # The generation's declared depth must be the depth the running sink holds.
    # Without this a plane whose records fit comfortably would pass under any
    # declared depth, leaving the bound unenforced exactly where it governs.
    if int(closing["capacity"]) != depth:
        fail(
            f"the {worker} sink reports capacity {closing['capacity']}, but its "
            f"generation declares traceDepth {depth}"
        )
    if int(closing["records"]) != len(parsed):
        fail(
            f"the {worker} summary counts {closing['records']} records but "
            f"{len(parsed)} were emitted"
        )
    if len(parsed) > FABRIC_TRACE_MAX_DEPTH:
        fail(
            f"{len(parsed)} {worker} records exceed the contract ceiling "
            f"{FABRIC_TRACE_MAX_DEPTH}"
        )
    if int(closing["dropped"]) != 0 or int(closing["rejected"]) != 0:
        fail(f"the {worker} trace reported loss: {closing}")
    terminals = [record for record in parsed if "terminal" in record["flags"]]
    if len(terminals) != 1:
        fail(f"expected exactly one {worker} terminal record, found {len(terminals)}")
    if terminals[0] is not parsed[-1]:
        fail(f"the {worker} terminal record is not the last record in its trace")


def check_transcript(transcript: str) -> None:
    """The marker contract. Worker-agnostic, so it runs on every plane."""
    match_marker_contract(transcript, CHAINS, FAILURE_MARKERS, fail)


def check_worker(transcript: str, worker: str, depth: int) -> int:
    parsed = records(transcript, worker)
    check_structure(parsed, worker)
    check_order(parsed, worker)
    check_bounds(parsed, summary(transcript, worker), depth, worker)
    return len(parsed)


def artifact(transcript: str, worker: str) -> str:
    """The comparable trace artifact: this worker's record lines, in order.

    The summary is excluded because it is a rendering of the records rather than
    evidence of its own, and the surrounding serial output is excluded because
    C8.11 requires the trace to be identical *independent of serial-log
    interleaving* -- so comparing whole transcripts would assert the opposite of
    the property.
    """
    return "\n".join(
        line.strip()
        for line in transcript.splitlines()
        if (match := RECORD.fullmatch(line.strip())) is not None
        and match.group("worker") == worker
    )


def check_determinism(first: str, second: str, worker: str) -> None:
    left = artifact(first, worker)
    right = artifact(second, worker)
    if left == right:
        return
    for index, (a, b) in enumerate(
        zip(left.splitlines(), right.splitlines(), strict=False)
    ):
        if a != b:
            fail(f"{worker} trace artifacts diverge at record {index}:\n  {a}\n  {b}")
    fail(
        f"{worker} trace artifacts differ in length: {len(left.splitlines())} vs "
        f"{len(right.splitlines())} records"
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot each timed fabric plane and assert its C8.11 semantic trace"
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built images instead of rebuilding them first",
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")

    total = 0
    for worker, module, relative_path, fixture in PLANES:
        gate = load_script(module, relative_path)
        pins = gate.load_pins()
        if not arguments.no_build:
            gate.build_image()
        gate.check_manifest()
        profile = pins["qemu_arm_virt"]
        assert isinstance(profile, dict)
        depth = declared_depth(fixture)
        # Each plane gate's `boot` stops at its own terminal, which prints after
        # the trace flush, so one boot yields the whole artifact.
        first = gate.boot(profile)
        check_transcript(first)
        count = check_worker(first, worker, depth)
        second = gate.boot(profile)
        check_transcript(second)
        check_worker(second, worker, depth)
        check_determinism(first, second, worker)
        total += count
        print(
            f"{worker}: {count} trace records in declared order, capacity "
            f"{depth} as declared, one terminal, nothing rejected, and "
            f"byte-identical across two boots",
            flush=True,
        )
    print(
        f"seL4 trace plane check: {total} C8.11 semantic-trace records across "
        f"{len(PLANES)} timed workers are bounded, structurally valid, in their "
        "declared total tie order across data, acknowledgement, peer death, and "
        "time, free of task ids and addresses, and byte-identical across two "
        "boots of each fixed generation"
    )


if __name__ == "__main__":
    main()
