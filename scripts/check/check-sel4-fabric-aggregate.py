#!/usr/bin/env python3

"""C8.15 gate: full-graph determinism and the C8 parent close.

C8.9-C8.14 each assert one property of one plane. This gate closes the parent
milestone by asserting the two things no single-plane gate can:

1. **Determinism.** The same graph, inputs, and simulated-time sequence run
   twice must produce identical *semantic* traces. This is checked by booting
   each aggregate plane twice and comparing its `[trace]` records field by
   field, not by comparing a summary or a count -- a trace that agreed on how
   many records it emitted while disagreeing on their content would satisfy a
   count and still be unusable as the comparison baseline C8.11 promises.

   "Semantic" is a declared split rather than a synonym for "every byte", and
   `SEMANTIC_FIELDS` below states which fields carry it and why. Three of the
   rendered fields are *observations* of the run rather than properties of the
   composition -- a peak sample, a per-instant arrival ordinal, and the instant
   a deferred conclusion happened to land in. Asserting those as constants
   asserted that the host scheduled two boots identically, which is neither
   what C8.15 claims nor true of QEMU under load (B75).

2. **One aggregate path.** Both required schedules -- the normal concurrent one
   and the fault one -- are exercised over the *same* declared composition, so
   the parent exit condition is observed on one graph rather than assembled from
   separate profile boots. The fault variant shares `sel4-traffic.zti` with
   `generation` changed and nothing else; the fault variant differs only in that
   its interposition hop and its telemetry publisher are compiled to die. That
   is what makes the pair an aggregate rather than two unrelated planes.

Why this is a separate gate rather than an extension of either plane's own. Each
plane gate boots once, because booting twice doubles the slowest step in the
suite for a property only this milestone needs. And determinism is a claim about
the relationship *between* runs, which a gate holding one transcript cannot
state at all.

What this gate deliberately does not re-assert: every property
`check-sel4-traffic-plane.py` and `check-sel4-fault-plane.py` already check.
Those gates are invoked here, in-process, against each boot they take -- so a
regression in any of them fails this gate too, and the aggregate does not become
a second, drifting copy of their expectations. `--no-build` is passed through so
this gate never rebuilds an image a plane gate just built.

The audit half of C8.15's deliverables -- reconciling the final authority,
resource, and fault corpus against every C8 deliverable -- is recorded in the
roadmap and its devlog entry rather than automated here: it is a reading of
prose against evidence, and a script asserting it would only be asserting that
someone wrote the prose.
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
import threading
import tomllib
from collections import Counter
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from fabric_trace_contract import (  # noqa: E402
    FABRIC_TRACE_MAX_RESOURCE_COUNTER,
    FABRIC_TRACE_RESOURCE_BUFFERS,
    FABRIC_TRACE_RESOURCE_CALLS,
    FABRIC_TRACE_RESOURCE_CAPABILITY_SLOTS,
    FABRIC_TRACE_RESOURCE_FRAMES,
    FABRIC_TRACE_RESOURCE_HISTORY,
    FABRIC_TRACE_RESOURCE_LOAN,
    FABRIC_TRACE_RESOURCE_OPERATIONS,
    FABRIC_TRACE_RESOURCE_QUEUE,
    FABRIC_TRACE_RESOURCE_RETAINED,
    FABRIC_TRACE_RESOURCE_RETRIES,
)
from harness import load_script  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
BOOT_TIMEOUT_SECONDS = 240

# The planes this aggregate composes, in the order it exercises them:
# `(label, gate module name, gate path, build flag, image)`.
#
# Both run the identical `drive_traffic_plane` composition over the identical
# declared graph. The fault plane's image differs only in that its declared
# interposition hop is compiled to exit rather than park and its telemetry
# publisher to exit without ending its stream, which is why the two are an
# aggregate over one graph rather than two planes to be compared.
PLANES: tuple[tuple[str, str, str, str, str], ...] = (
    (
        "normal concurrent schedule",
        "sel4_traffic_plane",
        "check/check-sel4-traffic-plane.py",
        "--traffic-plane",
        "slime-sel4-traffic.elf",
    ),
    (
        "fault schedule over the same graph",
        "sel4_fault_plane",
        "check/check-sel4-fault-plane.py",
        "--fault-plane",
        "slime-sel4-fault.elf",
    ),
)

# One trace record line, and the summary that closes a worker's trace. Parsed
# into fields rather than matched whole, because the comparison below is
# per field: an earlier revision matched `\[trace\] .*` and compared the line,
# which is what made every rendered field byte-significant.
#
# Deliberately only the C8.11 trace records: they are the milestone's declared
# evidence stream, they carry simulated time rather than wall time, and the
# schema forbids task ids and addresses in them -- so equality here is a real
# claim about the schedule rather than about how the transcript was captured.
#
# Serial markers are excluded because several legitimately vary: a broker's
# per-edge print races a participant's own summary print, which is exactly why
# the plane gates check those as membership rather than as order.
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
TRACE_LINE = re.compile(r"\[trace\] .*")

# The fields whose value is a property of the declared composition. These are
# compared between boots, and a difference in any of them is a real divergence:
# which worker emitted, which evidence family, which declared tie class, which
# route, which request, which outcome, which event code, and whether the record
# is the mandatory terminal or the saturation report.
SEMANTIC_FIELDS: tuple[str, ...] = (
    "worker",
    "kind",
    "order",
    "now",
    "route",
    "correlation",
    "status",
    "event",
    "flags",
)

# The fields whose value is an observation of the run that produced it, rather
# than of the composition that declared it. B75 measured each of these diverging
# between two boots of one composition under CPU oversubscription, and each was
# root-caused to a faithful reading of a genuinely varying quantity rather than
# to an ordering bug -- so no broker change can make them constant, and the
# earlier verbatim comparison was asserting a property the system never had.
#
# `sequence` is a per-instant *arrival* ordinal, not a declared position. It is
# assigned onto a record from the live counter in `Trace::blank`
# (`components/bins/src/fabric_trace_log.rs:233`) and advanced by `Trace::push`
# on every attempt including a refused one (`:255`), so where two independent
# peers are live in one instant its value states only which of them arrived
# first. That is the cross-activity arrival order B68's grouping was introduced
# to quarantine, one level down: the call worker's two scripted clients settle
# requests 4 and 22 independently
# (`components/bins/src/bin/fabric-call-client.rs:54-58`,
# `components/bins/src/fabric_call_scenario.rs:58-66`), both acks occur on every
# boot, and only their arrival order moves.
#
# `high_water` is exempt *per counter* rather than outright -- see
# `POLL_SAMPLED_COUNTERS`. It is zero on every non-resource record, since only
# `Trace::resource` ever sets it.
OBSERVED_FIELDS: tuple[str, ...] = ("sequence", "high_water")

# The resource counters whose `high_water` is a poll sample: a running maximum
# over a value read once per sweep of a broker's own loop. `peak_frames` maxes
# over a `live_frames` count taken at `fabric-service.rs:1193-1195`, after both
# pumps have run, so two publishers admitted within one iteration sample 2 while
# the same two split across iterations sample 1. Both are truthful readings of a
# real instant and the run's true concurrent peak genuinely differs.
#
# Named from the contract rather than restated as literals, so the gate and the
# format cannot disagree about which counter is which.
POLL_SAMPLED_COUNTERS: frozenset[int] = frozenset(
    {
        FABRIC_TRACE_RESOURCE_FRAMES,  # fabric-service.rs:1193-1195
        FABRIC_TRACE_RESOURCE_OPERATIONS,  # operation_broker.rs:330
        FABRIC_TRACE_RESOURCE_CALLS,  # call_broker.rs:283
        FABRIC_TRACE_RESOURCE_BUFFERS,  # fabric-service.rs:1201, call_broker.rs:291
        FABRIC_TRACE_RESOURCE_RETRIES,  # fabric-service.rs:1226, call_broker.rs:301
        FABRIC_TRACE_RESOURCE_RETAINED,  # operation_broker.rs:334
        FABRIC_TRACE_RESOURCE_QUEUE,  # fabric-service.rs:1209
        FABRIC_TRACE_RESOURCE_HISTORY,  # fabric-service.rs:1217
        FABRIC_TRACE_RESOURCE_LOAN,  # fabric-service.rs:1256
        FABRIC_TRACE_RESOURCE_CAPABILITY_SLOTS,  # fabric-service.rs:1321-1324
    }
)

# Every other counter's `high_water` stays compared, and the two that matter are
# compared for opposite reasons. `resourceMapping` is the counter whose *not
# moving* is the invariant -- the contract declares a region mapped when a role
# is provisioned and mapped while its holder lives -- and no plane gate pins its
# value for the `stream` worker (`check-sel4-traffic-plane.py`'s mapping arm
# asserts only `baseline == peak` and `peak != 0`), so exempting it would have
# been the one place this relaxation silently stopped catching a regression.
# `resourceSinkDropped` is the sink's own loss count, which must not move at all.
COMPARED_COUNTERS: frozenset[int] = frozenset(
    set(range(1, FABRIC_TRACE_MAX_RESOURCE_COUNTER + 1)) - POLL_SAMPLED_COUNTERS
)

# The records whose `now` is an observation rather than a property, keyed by
# `(worker, order)`. Narrow on purpose, in both axes: `now` stays semantic
# everywhere else, so a clock that started drifting on ordinary data records
# still fails this gate.
#
# Only the stream worker's peer death is deferred. It is concluded from a drain
# that runs *after* the termination latch
# (`components/bins/src/bin/fabric-service.rs:1136-1180`), and B75 made the
# record's presence independent of that race without fixing which instant the
# drain completes in -- a record's stamp is whatever `self.now_ns` had reached
# when `Trace::blank` built it (`components/bins/src/fabric_trace_log.rs:234`).
# Measured directly: the same scripted death stamped `now=50` on one boot and
# `now=100` on the next.
#
# The call and operation workers' peer-death records are *not* deferred and stay
# compared. Each is emitted by a `retire_server` whose only call site is an
# `observe_server_death` that acts straight off the supervision read
# (`components/bins/src/call_broker.rs:773` from `:1545-1559`,
# `components/bins/src/operation_broker.rs:297` from `:1321-1335`), with no drain
# between. Their stamps are fixed in every captured transcript.
DEFERRED_INSTANT_RECORDS: frozenset[tuple[str, str]] = frozenset({("stream", "peer-death")})

# The split must cover the grammar exactly, and cover it once. Without this an
# added field would belong to neither set and go silently uncompared, which is
# the failure mode a relaxation invites: the gate would keep passing while
# quietly checking less than it says it does. The counter split is guarded the
# same way, over the contract's own declared range.
_GRAMMAR_FIELDS = frozenset(RECORD.groupindex)
_CLASSIFIED_FIELDS = frozenset(SEMANTIC_FIELDS) | frozenset(OBSERVED_FIELDS)
if frozenset(SEMANTIC_FIELDS) & frozenset(OBSERVED_FIELDS):
    raise SystemExit(
        "seL4 fabric aggregate check: a field is both semantic and observed: "
        f"{sorted(frozenset(SEMANTIC_FIELDS) & frozenset(OBSERVED_FIELDS))}"
    )
if _CLASSIFIED_FIELDS != _GRAMMAR_FIELDS:
    raise SystemExit(
        "seL4 fabric aggregate check: the semantic/observed split does not cover "
        f"the record grammar: unclassified {sorted(_GRAMMAR_FIELDS - _CLASSIFIED_FIELDS)}, "
        f"unknown {sorted(_CLASSIFIED_FIELDS - _GRAMMAR_FIELDS)}"
    )
if POLL_SAMPLED_COUNTERS & COMPARED_COUNTERS:
    raise SystemExit(
        "seL4 fabric aggregate check: a resource counter is both poll-sampled and "
        f"compared: {sorted(POLL_SAMPLED_COUNTERS & COMPARED_COUNTERS)}"
    )
if POLL_SAMPLED_COUNTERS | COMPARED_COUNTERS != set(
    range(1, FABRIC_TRACE_MAX_RESOURCE_COUNTER + 1)
):
    raise SystemExit(
        "seL4 fabric aggregate check: the resource-counter split does not cover the "
        f"contract's declared range 1..{FABRIC_TRACE_MAX_RESOURCE_COUNTER}"
    )

# Every boot of a plane must emit exactly this many trace records. Pinned rather
# than merely compared between the two boots of one plane: without it, a
# regression that silently stopped every worker from emitting would produce two
# identical empty transcripts and pass the determinism comparison.
#
# Per plane rather than one shared number, because the two planes genuinely
# differ by one record: the fault plane scripts its telemetry publisher to exit
# without ending its stream, which the broker reports as a stream-family peer
# death. The traffic plane's publisher ends its route normally and so emits no
# such record. That difference used to be a race rather than a plane property --
# the traffic plane's publisher also ended normally there, but whether the
# broker observed the terminal sample or the exit first decided whether a
# peer-death record appeared, so both planes intermittently emitted 140 and
# intermittently 139. Splitting the constant states the difference these two
# images are supposed to have; it does not paper over a variable one.
EXPECTED_TRACE_RECORDS: dict[str, int] = {
    "normal concurrent schedule": 139,
    "fault schedule over the same graph": 140,
}

# The keys above are `PLANES` labels. Checked here rather than left to the
# lookup, so renaming a plane fails at import with the reason rather than at the
# assertion with a missing-count message that reads like a broker regression.
if {plane[0] for plane in PLANES} != set(EXPECTED_TRACE_RECORDS):
    raise SystemExit(
        "seL4 fabric aggregate check: EXPECTED_TRACE_RECORDS keys do not match "
        f"PLANES labels: {sorted(EXPECTED_TRACE_RECORDS)} vs "
        f"{sorted(plane[0] for plane in PLANES)}"
    )

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"\[init\] fabric boot fail: .*",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)

INIT_COMPLETE = r"\[init\] traffic plane reclaimed"
# B74: the root ran its dispatcher bound out with tasks still live, after having
# certified the graph healthy. Reaching this before init's completion marker means
# the guest has stopped serving and that marker is never coming, so the read below
# stops here instead of waiting out the watchdog and reporting a bare timeout.
GRAPH_EXHAUSTED = r"SLIME_GRAPH exhausted live=(\d+) iterations=(\d+) certified=1"
TERMINAL_MARKER = r"SLIME_GRAPH component exit task=(\d+) status=(-?\d+)"
SPAWN_PATTERN = re.compile(r"SLIME_GRAPH spawned task=(\d+) child=(\d+) component=([^ ]+) ")


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 fabric aggregate check: {message}")


def load_pins() -> dict[str, object]:
    if not PINS_PATH.is_file():
        fail(f"missing pin manifest: {PINS_PATH.relative_to(ROOT)}")
    try:
        pins = tomllib.loads(PINS_PATH.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse {PINS_PATH.relative_to(ROOT)}: {error}")
    profile = pins.get("qemu_arm_virt")
    if not isinstance(profile, dict):
        fail(f"{PINS_PATH.relative_to(ROOT)} declares no [qemu_arm_virt] table")
    return profile


def build_image(flag: str) -> None:
    command = [sys.executable, str(BUILD_SCRIPT), flag]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot run the seL4 image build: {error}")
    if process.returncode != 0:
        fail(f"seL4 image build failed with exit status {process.returncode}")


def boot(profile: dict[str, object], image: str, attempt: int) -> str:
    """Boot one image until init's clean exit, returning the transcript."""
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    path = ROOT / "build" / image
    if not path.is_file():
        fail(f"missing packaged image {path.relative_to(ROOT)}")
    command = [
        qemu,
        "-machine",
        str(profile["machine"]),
        "-cpu",
        str(profile["cpu"]),
        "-smp",
        str(profile["cpus"]),
        "-m",
        f"size={profile['memory_mib']}M",
        "-nographic",
        "-serial",
        "mon:stdio",
        "-kernel",
        str(path),
    ]
    print(f"[boot {attempt}] {image}", flush=True)
    failures = re.compile("|".join(FAILURE_MARKERS))
    init_complete = re.compile(INIT_COMPLETE)
    component_exit = re.compile(TERMINAL_MARKER)
    graph_exhausted = re.compile(GRAPH_EXHAUSTED)
    lines: list[str] = []
    saw_init_complete = False
    init_task: str | None = None
    saw_init_exit = False
    exhausted: re.Match[str] | None = None
    try:
        process = subprocess.Popen(
            command,
            cwd=ROOT,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
    except OSError as error:
        fail(f"cannot run QEMU: {error}")
    watchdog = threading.Timer(BOOT_TIMEOUT_SECONDS, process.kill)
    watchdog.start()
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\r\n"))
            if failures.search(line):
                break
            exhausted = graph_exhausted.search(line) or exhausted
            if exhausted is not None:
                break
            spawn = SPAWN_PATTERN.search(line)
            if spawn is not None and init_task is None:
                init_task = spawn.group(1)
            if init_complete.search(line):
                saw_init_complete = True
                continue
            exit_match = component_exit.search(line)
            if saw_init_complete and exit_match is not None and exit_match.group(1) == init_task:
                saw_init_exit = int(exit_match.group(2)) == 0
                break
    finally:
        timed_out = not watchdog.is_alive()
        watchdog.cancel()
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    transcript = "\n".join(lines)
    if exhausted is not None and not saw_init_exit:
        fail(
            f"{image} boot {attempt} wedged: the root exhausted its dispatcher bound after "
            f"{exhausted.group(2)} iterations with {exhausted.group(1)} tasks still live, "
            "having already certified the graph healthy, and stopped serving without "
            "reclaiming the plane. The workload did not drain; this is not a slow boot."
        )
    if timed_out and not saw_init_exit:
        fail(f"{image} boot {attempt} exceeded {BOOT_TIMEOUT_SECONDS}s without init's clean exit")
    if not saw_init_exit:
        fail(f"{image} boot {attempt} did not reach init's clean exit")
    return transcript


def trace_records(transcript: str) -> list[str]:
    return [line.strip() for line in TRACE_LINE.findall(transcript)]


def render(fields: tuple[tuple[str, str], ...], count: int) -> str:
    """One projected record, in record field order, with its multiplicity.

    Field order is the record's own rather than alphabetical, so a line in a
    failure message is greppable against the transcript it came from.
    """
    body = " ".join(f"{name}={value}" for name, value in fields)
    return f"{body}  x{count}" if count != 1 else body


def is_deferred(fields: dict[str, str]) -> bool:
    return (fields["worker"], fields["order"]) in DEFERRED_INSTANT_RECORDS


def declared_key(fields: dict[str, str]) -> tuple[str, ...]:
    """The position a record holds by declaration rather than by arrival.

    `TraceSink` arranges a worker's records by `(now_ns, order_class, sequence)`,
    so within one `(worker, kind)` group the declared position is `(order, now)`
    and `sequence` is the arrival tie-break the format explicitly leaves to the
    emitter.

    Comparing this key positionally does work the content comparison below
    cannot: a boot that emitted one group's records in a different declared
    order -- `[data@0, time@0]` against `[time@0, data@0]` -- has an identical
    content multiset and a different position list. Nothing else covers that on
    these two images, since `check_order` lives in `check-sel4-trace-plane.py`
    and runs against the separate qos/call/operation images, while the traffic
    and fault gates assert only that the terminal is last.

    A deferred-instant record drops `now` for the reason
    `DEFERRED_INSTANT_RECORDS` states: its instant is when a conclusion landed,
    not when a fact became true.
    """
    if "order" not in fields:
        return ("complete",)
    if is_deferred(fields):
        return (fields["order"],)
    return (fields["order"], fields["now"])


def semantic_projection(fields: dict[str, str]) -> tuple[tuple[str, str], ...]:
    """The part of a record that the declared composition determines.

    Ordered by `SEMANTIC_FIELDS` rather than sorted, so `render` above emits a
    line in the record's own field order.
    """
    if "order" not in fields:
        # A worker's closing summary: capacity, record count, and the two loss
        # counters. Every one of them is control-flow determined, so it is
        # compared whole.
        return tuple(fields.items())
    projected = [
        (name, fields[name].strip() if name == "flags" else fields[name])
        for name in SEMANTIC_FIELDS
    ]
    if is_deferred(fields):
        projected = [
            (name, "<deferred>" if name == "now" else value) for name, value in projected
        ]
    # `high_water` is compared for the counters the contract does not leave to a
    # poll, and `event` names which counter a resource record carries.
    if fields["kind"] == "resource" and int(fields["event"]) in COMPARED_COUNTERS:
        projected.append(("high_water", fields["high_water"]))
    return tuple(projected)


def records_by_participant(records: list[str]) -> dict[tuple[str, str], list[dict[str, str]]]:
    """Group a boot's parsed trace records by emitting worker *and* record kind.

    B68: comparing the flat record list positionally asserts one *interleaving*
    of concurrent activity, not the trace's determinism. Observed failing about
    one run in four with two boots disagreeing at record 12 about which worker
    emitted next — a `subscriber-b` resource record against an `operation` route
    record, both legitimate.

    Grouping by worker alone is not enough, which a second failure showed: a
    worker's `[trace]` prefix names its *sink*, and one sink aggregates several
    kinds. `stream`'s record 2 came out `kind=fault order=peer-death` on one boot
    and `kind=qos order=time` on the other, both at `sequence=2` — two independent
    activities racing into one sink. What C8.15 claims, and what this compares, is
    that each (worker, kind) sequence is identical; which kind reaches the sink
    first is scheduling.

    A worker's closing summary carries no `kind=`, and there is exactly one per
    worker, so it groups under its own pseudo-kind rather than being rejected.
    """
    grouped: dict[tuple[str, str], list[dict[str, str]]] = {}
    for line in records:
        record = RECORD.fullmatch(line)
        if record is not None:
            fields = record.groupdict()
            key = (fields["worker"], fields["kind"])
        else:
            closing = SUMMARY.fullmatch(line)
            if closing is None:
                fail(f"trace line matches neither the record nor the summary grammar: {line}")
            fields = closing.groupdict()
            key = (fields["worker"], "complete")
        grouped.setdefault(key, []).append(fields)
    return grouped


def check_determinism(label: str, first: str, second: str) -> int:
    """Every participant's semantic trace is identical across two boots.

    Per participant rather than across the merged list (B68), for the reason
    `records_by_participant` documents. The total count is still pinned, so a
    plane that stopped emitting cannot compare equal to itself, and the
    participant *set* must match too — otherwise a boot that dropped a whole
    participant would pass by comparing the ones that remain.

    Within a participant's group the comparison is two independent claims rather
    than one (B75). The ordered list of *declared* positions must match exactly,
    which catches a group emitted in a different declared order even when its
    content is unchanged. The *semantic* content is then compared as a multiset,
    because records sharing one declared position are ordered by arrival: the
    call worker's two scripted clients both settle on every boot and only their
    arrival order moves, so comparing content positionally asserted an
    interleaving the format never promised. Neither claim subsumes the other --
    the first is blind to content, the second to order.
    """
    left = trace_records(first)
    right = trace_records(second)
    expected = EXPECTED_TRACE_RECORDS.get(label)
    if expected is None:
        fail(f"{label}: no pinned trace-record count declared for this plane")
    if not left:
        fail(f"{label}: the first boot emitted no trace records at all")
    if len(left) != expected:
        fail(
            f"{label}: the first boot emitted {len(left)} trace records, expected "
            f"{expected}; a plane that stopped emitting would otherwise "
            "compare equal to itself and pass"
        )
    if len(right) != expected:
        fail(
            f"{label}: the second boot emitted {len(right)} trace records, expected "
            f"{expected}"
        )
    left_by = records_by_participant(left)
    right_by = records_by_participant(right)
    if set(left_by) != set(right_by):
        only_first = sorted(set(left_by) - set(right_by))
        only_second = sorted(set(right_by) - set(left_by))
        fail(
            f"{label}: the two boots emitted traces for different participants -- "
            f"only in the first: {only_first or 'none'}; only in the second: "
            f"{only_second or 'none'}"
        )
    for participant in sorted(left_by):
        mine, theirs = left_by[participant], right_by[participant]
        worker, kind = participant
        if len(mine) != len(theirs):
            fail(
                f"{label}: {worker}/{kind} emitted {len(mine)} records in the "
                f"first boot "
                f"and {len(theirs)} in the second; its own sequence is not scheduling "
                "dependent, so a differing count is a real divergence"
            )
        my_positions = [declared_key(record) for record in mine]
        their_positions = [declared_key(record) for record in theirs]
        if my_positions != their_positions:
            for index, (a, b) in enumerate(zip(my_positions, their_positions, strict=True)):
                if a != b:
                    fail(
                        f"{label}: {worker}/{kind} record {index} holds a different declared "
                        f"position between boots -- a record's instant and tie class are "
                        f"properties of the composition.\n  first:  {a}\n  second: {b}"
                    )
        my_content = Counter(semantic_projection(record) for record in mine)
        their_content = Counter(semantic_projection(record) for record in theirs)
        if my_content != their_content:
            # `Counter.__sub__` keeps only positive counts, so each side lists
            # what the other lacks *and by how many* -- a record whose
            # multiplicity moved from two copies to three is a real divergence
            # and would otherwise print as one unannotated line. A side can
            # legitimately be empty when the other is a strict submultiset, so
            # it says so rather than trailing off.
            only_first = my_content - their_content
            only_second = their_content - my_content

            def listing(difference: Counter[tuple[tuple[str, str], ...]]) -> str:
                if not difference:
                    return "none"
                return "\n                      ".join(
                    render(record, count) for record, count in sorted(difference.items())
                )

            fail(
                f"{label}: {worker}/{kind} emitted different semantic records between "
                f"boots.\n  only in the first:  {listing(only_first)}"
                f"\n  only in the second: {listing(only_second)}"
            )
    return len(left)


def main() -> None:
    parser = argparse.ArgumentParser(
        description=(
            "Boot every C8 aggregate plane twice and assert C8.15's determinism and "
            "parent-close conditions"
        )
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built images instead of rebuilding them first",
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    profile = load_pins()

    total = 0
    for label, module_name, module_path, flag, image in PLANES:
        if not arguments.no_build:
            build_image(flag)
        gate = load_script(module_name, module_path)
        first = boot(profile, image, 1)
        # Every property the plane's own gate asserts, on this exact boot. Run
        # in-process against the transcript rather than by re-invoking the gate,
        # so the aggregate cannot drift from what the narrow gate requires and
        # neither boot is spent twice.
        gate.check_transcript(first)
        second = boot(profile, image, 2)
        gate.check_transcript(second)
        records = check_determinism(label, first, second)
        total += records
        print(
            f"[{label}] both boots satisfied {module_path} and emitted "
            f"{records} semantically identical trace records",
            flush=True,
        )

    print(
        f"seL4 fabric aggregate check: {len(PLANES)} schedules over one declared "
        f"composition each passed their own plane gate on two independent boots and "
        f"produced {total} semantically identical trace records in total, each holding "
        "its declared instant and tie class; every declared authority, resource, and "
        "fault property those gates assert holds on both runs",
        flush=True,
    )


if __name__ == "__main__":
    main()
