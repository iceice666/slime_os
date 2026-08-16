#!/usr/bin/env python3

"""C8.13 gate: concurrent cross-plane traffic and resource ceilings on seL4.

C8.10 proved the stream, call, and operation planes fit one collision-free
partition while every participant parks. This gate boots the identical
partition (`sel4-traffic.zti` is `sel4-boot.zti` with `bootAction` and
`generation` changed, plus the additional grants real traffic needs) and
requires every worker to carry its real C8.4-C8.9 scenario *concurrently*
under one fixed schedule instead:

* The stream, call, and operation workers each complete their own bounded
  scenario, observably interleaved rather than run as three sequential
  phases -- checked by requiring at least one marker from a different plane
  to land between two markers of another.
* Every declared resource ceiling emits bounded high-water evidence: a peak
  record while traffic runs, and -- for every counter the worker actually
  releases back to zero -- a second baseline record after release, both
  through the C8.11 trace sink with nothing dropped or rejected.
* Every spawned task reaches exactly one clean exit, except the declared
  interposition proxy and the filtered-view observer, which the milestone
  requires to stay parked (C8.8's real behavior for both belongs to the
  `visibility` and `matrix` actions, not this one) and so are asserted
  healthy-idle instead.

The QoS-timed stream arm now runs here too: `fabric-publisher-b`'s simulated
clock edge (`fabric-publisher-b-clock`, wired into this partition alongside
the C8.10 grants) drives real RELIABLE retry accounting and exhaustion on the
telemetry route concurrently with the call and operation planes' own
unconditional clocks -- the retry/deadline evidence this milestone asks for
comes from all three now, not just the latter two.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
import threading
import tomllib
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from fabric_trace_contract import (  # noqa: E402
    FABRIC_TRACE_RESOURCE_BUFFERS,
    FABRIC_TRACE_RESOURCE_CALLS,
    FABRIC_TRACE_RESOURCE_COMPLETE,
    FABRIC_TRACE_RESOURCE_FRAMES,
    FABRIC_TRACE_RESOURCE_HISTORY,
    FABRIC_TRACE_RESOURCE_LOAN,
    FABRIC_TRACE_RESOURCE_MAPPING,
    FABRIC_TRACE_RESOURCE_OPERATIONS,
    FABRIC_TRACE_RESOURCE_QUEUE,
    FABRIC_TRACE_RESOURCE_RETAINED,
    FABRIC_TRACE_RESOURCE_RETRIES,
)

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4-traffic.elf"
MANIFEST = ROOT / "build" / "slime-sel4-traffic.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-traffic.zti"
IMAGE_VARIANT = "traffic"
BOOT_TIMEOUT_SECONDS = 240

INIT_COMPLETE = r"\[init\] traffic plane reclaimed"
TERMINAL_MARKER = r"SLIME_GRAPH component exit task=(\d+) status=(-?\d+)"

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    r"\[init\] fabric boot fail: .*",
    r"SLIME_GRAPH spawn (?:failed|unwound|unwind incomplete) .*",
    r"SLIME_GRAPH capability (?:export|import|cancel) (?:failed|refused) .*",
    r"SLIME_GRAPH buffer create refused .*",
    r"SLIME_GRAPH loan refused .*",
    r"<<seL4\(CPU 0\) \[decode(?!CNodeInvocation/107\b)",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
    r"unhandled",
)

# Init spawns every task itself, single-threaded, so this order is a fact
# about `drive_traffic_plane` rather than a scheduling accident -- unlike
# everything each worker does with them afterward, which three concurrent
# brokers race over and this gate deliberately does not order.
CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "the generation was admitted with its declared partition",
        (
            r"SLIME_ROOT generation admitted number=36 executables=20 instances=20 "
            r"grants=45 health=20 bootstrap=1",
            r"SLIME_ROOT fabric graph=admitted schemas=4 routes=5 participants=15 "
            r"interpositions=1",
        ),
    ),
    (
        "init spawns every declared task in one fixed, single-threaded order",
        (
            r"\[init\] traffic control channels minted",
            r"\[init\] traffic stream participants spawned",
            r"\[init\] traffic stream broker spawned",
            r"\[init\] traffic call plane spawned",
            r"\[init\] traffic operation plane spawned",
            r"\[init\] traffic graph spawned with static endpoints",
        ),
    ),
    (
        "the plane closes only after init observes every worker settle",
        (
            r"\[init\] traffic plane reclaimed",
            r"SLIME_GRAPH component exit task=0 status=0",
        ),
    ),
)

SPAWN_PATTERN = re.compile(
    r"SLIME_GRAPH spawned task=(\d+) child=(\d+) component=([^ ]+) "
    r"grants=(\d+) endpoints=(\d+) notifications=(\d+) handle=(\d+)"
)
EXIT_PATTERN = re.compile(r"SLIME_GRAPH component exit task=(\d+) status=(-?\d+)")
COMPONENT_FAILURE = re.compile(r"\[fabric[^\]]*\] fail: .*")

TRACE_PATTERN = re.compile(
    r"\[trace\] (?P<family>stream|call|operation) kind=(?P<kind>\w+) "
    r"order=(?P<order>[\w-]+) now=(?P<now>\d+) route=(?P<route>[0-9a-f]{16}) "
    r"correlation=(?P<correlation>\d+) sequence=(?P<sequence>\d+) "
    r"status=(?P<status>-?\d+) event=(?P<event>\d+) high_water=(?P<high_water>\d+)"
    r"(?P<terminal> terminal)?"
)
SUMMARY_PATTERN = re.compile(
    r"\[trace\] (?P<family>stream|call|operation) complete "
    r"capacity=(?P<capacity>\d+) records=(?P<records>\d+) "
    r"dropped=(?P<dropped>\d+) rejected=(?P<rejected>\d+)"
)

# Every task `drive_traffic_plane` spawns, in the fixed order it spawns them.
EXPECTED_SPAWNED = (
    "fabric-publisher",
    "fabric-subscriber",
    "fabric-publisher-b",
    "fabric-subscriber-b",
    "fabric-observer",
    "fabric-proxy",
    "fabric-probe",
    "fabric-service",
    "fabric-call-client",
    "fabric-call-client-b",
    "fabric-call-server",
    "fabric-call-time",
    "fabric-call-worker",
    "fabric-op-client",
    "fabric-op-client-b",
    "fabric-op-server",
    "fabric-op-time",
    "fabric-op-client-b-restart",
    "fabric-op-worker",
)
# The two structural C8.10 roles this plane keeps parked rather than driving
# real traffic through: C8.8's filtered view and declared interposition are
# not this milestone's traffic classes, and the plain stream broker's
# delivery loop has no notion of a subscriber that will never consume again
# (`fabric-service::traffic_graph`, `fabric-observer::main`).
EXPECTED_PARKED = frozenset({"fabric-observer", "fabric-proxy"})

# Resource evidence C8.13 adds, by trace family and RESOURCE_* event code
# (`contracts/fabric-trace/v1/schema.zt`): `(event, name, baselines)`, where
# `baselines` is how many times the counter is recorded after its peak. A
# held-and-released counter (frames, buffers, calls, operations, retained
# results, queue, history) reports peak then baseline; a cumulative one
# (retries) reports only its peak, since a retry count never returns to a
# meaningful "zero" a reader should expect.
#
# `resourceEvent` and a live capability-slot ceiling remain outside this dict:
#
# - `resourceEvent` has a real table behind it (the operation worker's
#   `pending_deliveries`) but no reachable signal: `queue_delivery` queues only
#   on `ERR_WOULDBLOCK` from `slime_rt::send`, which resolves to seL4's
#   blocking `Cap::send` and cannot return it, so the peak is a structural zero
#   under every schedule rather than under this one.
# - The capability-slot ceiling has no signal at all: nothing in the root
#   tracks a live child's own CNode occupancy, so no query could return it.
#
# C8.13.1's `mapping`/`loan` occupancy is reported by the stream worker alone,
# and that is a sink-capacity bound rather than a scoping choice: the call
# worker holds the same loan and mapping state, but its own scenario already
# fills 63 of its sink's 64 declared slots -- 62 ordinary records plus the
# terminal, against `maxTraceDepth = 64` and `terminalReserve = 2`, so zero
# ordinary slots remain. That ceiling is the schema's absolute page-sized bound
# rather than a fixture value that can be raised, and reporting both counters
# there costs four records (a peak and a baseline each), not one.
EXPECTED_RESOURCES: dict[str, tuple[tuple[int, str, int], ...]] = {
    "stream": (
        (FABRIC_TRACE_RESOURCE_FRAMES, "frames", 2),
        (FABRIC_TRACE_RESOURCE_BUFFERS, "buffers", 2),
        (FABRIC_TRACE_RESOURCE_QUEUE, "queue", 2),
        (FABRIC_TRACE_RESOURCE_HISTORY, "history", 2),
        # Cumulative rather than held-and-released -- one peak record, no
        # baseline -- now that the QoS-timed clock edge drives
        # `Subscriber::retry_count` under `"traffic"` too.
        (FABRIC_TRACE_RESOURCE_RETRIES, "retries", 1),
        # C8.13.1: the root's own per-holder charges for this broker, read back
        # through the self-scoped occupancy query rather than counted from the
        # worker's own frames -- so the two disagreeing is the accounting
        # regression these records exist to catch. `mapping` is asserted
        # constant and `loan` drained; see `check_resources` for why each shape
        # is the assertion rather than a weakening of one.
        (FABRIC_TRACE_RESOURCE_MAPPING, "mapping", 2),
        (FABRIC_TRACE_RESOURCE_LOAN, "loan", 2),
    ),
    "call": (
        (FABRIC_TRACE_RESOURCE_CALLS, "calls", 2),
        (FABRIC_TRACE_RESOURCE_BUFFERS, "buffers", 2),
        (FABRIC_TRACE_RESOURCE_RETRIES, "retries", 1),
    ),
    "operation": (
        (FABRIC_TRACE_RESOURCE_OPERATIONS, "operations", 2),
        (FABRIC_TRACE_RESOURCE_RETAINED, "retained", 2),
    ),
}


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 traffic plane check: {message}")


def load_pins() -> dict[str, object]:
    if not PINS_PATH.is_file():
        fail(f"missing pin manifest: {PINS_PATH.relative_to(ROOT)}")
    try:
        pins = tomllib.loads(PINS_PATH.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse {PINS_PATH.relative_to(ROOT)}: {error}")
    if "qemu_arm_virt" not in pins:
        fail(f"{PINS_PATH.relative_to(ROOT)} declares no [qemu_arm_virt] profile")
    return pins


def profile_text(profile: dict[str, object], key: str) -> str:
    value = profile.get(key)
    if not isinstance(value, str) or not value:
        fail(f"qemu_arm_virt profile is missing a text field {key!r}")
    return value


def profile_integer(profile: dict[str, object], key: str) -> int:
    value = profile.get(key)
    if not isinstance(value, int) or isinstance(value, bool):
        fail(f"qemu_arm_virt profile is missing an integer field {key!r}")
    return value


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def build_image() -> None:
    command = [sys.executable, str(BUILD_SCRIPT), "--traffic-plane"]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot run the seL4 image build: {error}")
    if process.returncode != 0:
        fail(f"seL4 image build failed with exit status {process.returncode}")


def check_manifest() -> None:
    if not MANIFEST.is_file():
        fail(
            f"missing identity manifest {MANIFEST.relative_to(ROOT)}; "
            "run `just sel4_traffic_check`"
        )
    try:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse {MANIFEST.relative_to(ROOT)}: {error}")
    if not isinstance(manifest, dict) or manifest.get("kind") != "slime-sel4-image-identity":
        fail(f"{MANIFEST.relative_to(ROOT)} is not a Slime seL4 identity manifest")
    if manifest.get("variant") != IMAGE_VARIANT:
        fail(
            f"{MANIFEST.relative_to(ROOT)} records variant "
            f"{manifest.get('variant')!r}, not {IMAGE_VARIANT!r}; "
            "rebuild with `--traffic-plane`"
        )
    image = manifest.get("image")
    if not isinstance(image, dict) or not isinstance(image.get("sha256"), str):
        fail("identity manifest does not record the packaged image digest")
    if not IMAGE.is_file():
        fail(f"missing packaged image {IMAGE.relative_to(ROOT)}")
    actual = sha256_file(IMAGE)
    if actual != image["sha256"]:
        fail(
            f"{IMAGE.relative_to(ROOT)} SHA-256 is {actual}, but the identity manifest "
            f"records {image['sha256']}; rebuild before booting"
        )


def boot(profile: dict[str, object]) -> str:
    """Boot until init's clean exit, or stop immediately on a failure marker."""
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    command = [
        qemu,
        "-machine",
        profile_text(profile, "machine"),
        "-cpu",
        profile_text(profile, "cpu"),
        "-smp",
        str(profile_integer(profile, "cpus")),
        "-m",
        f"size={profile_integer(profile, 'memory_mib')}M",
        "-nographic",
        "-serial",
        "mon:stdio",
        "-kernel",
        str(IMAGE),
    ]
    print(f"[boot] {' '.join(command)}", flush=True)
    failures = re.compile("|".join(FAILURE_MARKERS))
    init_complete = re.compile(INIT_COMPLETE)
    component_exit = re.compile(TERMINAL_MARKER)
    lines: list[str] = []
    saw_init_complete = False
    init_task: str | None = None
    saw_init_exit = False
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
            spawn = SPAWN_PATTERN.search(line)
            if spawn is not None:
                parent = spawn.group(1)
                if init_task is None:
                    init_task = parent
                elif init_task != parent:
                    fail(
                        f"traffic spawn records named multiple init tasks: "
                        f"{init_task}, {parent}"
                    )
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
    if timed_out and not saw_init_exit:
        report_transcript(transcript)
        fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without init's clean exit")
    if saw_init_complete and not saw_init_exit:
        report_transcript(transcript)
        fail("init reported traffic-plane completion but no clean exit record followed")
    return transcript


def report_transcript(transcript: str) -> None:
    tail = transcript.splitlines()[-80:]
    if tail:
        sys.stdout.write("--- serial transcript (tail) ---\n")
        sys.stdout.write("\n".join(tail) + "\n")
        sys.stdout.write("--- end transcript ---\n")
        sys.stdout.flush()


def composition(transcript: str) -> str:
    """The composition through init's clean exit."""
    complete = re.search(INIT_COMPLETE, transcript)
    if complete is None:
        return transcript
    spawns = SPAWN_PATTERN.findall(transcript[: complete.end()])
    parent_ids = {spawn[0] for spawn in spawns}
    if len(parent_ids) != 1:
        return transcript
    init_task = next(iter(parent_ids))
    exit_match = re.search(
        rf"SLIME_GRAPH component exit task={re.escape(init_task)} status=0",
        transcript[complete.end() :],
    )
    if exit_match is None:
        return transcript
    return transcript[: complete.end() + exit_match.end()]


def check_task_lifecycle(transcript: str) -> None:
    """Every spawned task exits cleanly, except the two structural roles this
    plane keeps parked -- checked healthy-idle instead of exited."""
    head = composition(transcript)
    spawns = SPAWN_PATTERN.findall(head)
    if tuple(match[2] for match in spawns) != EXPECTED_SPAWNED:
        fail(
            "spawned component sequence was "
            f"{tuple(match[2] for match in spawns)!r}, expected {EXPECTED_SPAWNED!r}"
        )
    parent_ids = {match[0] for match in spawns}
    if len(parent_ids) != 1:
        fail(f"spawn records name multiple parents: {sorted(parent_ids)}")
    init_task = next(iter(parent_ids))
    children = {match[2]: match[1] for match in spawns}
    exits: dict[str, list[int]] = {}
    for task, status in EXIT_PATTERN.findall(transcript):
        exits.setdefault(task, []).append(int(status))
    for component, task in children.items():
        if component in EXPECTED_PARKED:
            if task in exits:
                fail(
                    f"{component} task {task} exited with status(es) {exits[task]}, but "
                    "the milestone requires it to stay parked"
                )
            continue
        if exits.get(task) != [0]:
            fail(f"{component} task {task} exit statuses were {exits.get(task, [])}, expected [0]")
    if exits.get(init_task) != [0]:
        fail(f"init task {init_task} exit statuses were {exits.get(init_task, [])}, expected [0]")
    reported = COMPONENT_FAILURE.findall(head)
    if reported:
        report_transcript(transcript)
        fail(f"a component failed inside the composition: {reported}")


def check_concurrency(transcript: str) -> None:
    """Stream, call, and operation traffic interleave under one schedule.

    Three sequential phases and one concurrent schedule both produce three
    non-empty per-family marker sets; only interleaving distinguishes them.
    Asserted by finding, for each family, at least one marker from a
    *different* family strictly between two of its own -- which a run that
    finished one plane before starting the next could never produce.
    """
    head = composition(transcript)
    positions: dict[str, list[int]] = {
        # Anchored on markers `broker()`'s relay loop alone emits, not
        # `provision()`'s role-request phase: `provisioned`/`QoS matched` both
        # land before `broker()` is ever entered, so they cannot distinguish
        # real interleaved traffic from three sequential provisioning phases.
        "stream": [
            m.start()
            for m in re.finditer(
                r"\[fabric\] (?:downstream loan created|large sample copied once|QoS peer dead)",
                head,
            )
        ],
        "call": [m.start() for m in re.finditer(r"\[fabric\] call (?:forwarded|reply correlated|timed out|retry exhausted|peer death propagated)", head)],
        "operation": [m.start() for m in re.finditer(r"\[fabric\] operation (?:accepted|goal forwarded|result routed|feedback routed)", head)],
    }
    for family, marks in positions.items():
        if len(marks) < 2:
            fail(f"the {family} plane emitted too few markers ({len(marks)}) to show real traffic")
    interleaved = {family: False for family in positions}
    for family, marks in positions.items():
        others = [p for other, pts in positions.items() if other != family for p in pts]
        for start, end in zip(marks, marks[1:], strict=False):
            if any(start < other < end for other in others):
                interleaved[family] = True
                break
    stalled = [family for family, seen in interleaved.items() if not seen]
    if stalled:
        report_transcript(transcript)
        fail(
            f"{stalled} showed no marker from another plane between two of its own; "
            "the schedule looks sequential rather than concurrent"
        )


def check_resources(transcript: str) -> None:
    """Every declared resource ceiling emits bounded peak(+baseline) evidence,
    on all three planes, through a sink that dropped and rejected nothing."""
    head = composition(transcript)
    records_by_family: dict[str, list[dict[str, str]]] = {"stream": [], "call": [], "operation": []}
    for match in TRACE_PATTERN.finditer(head):
        record = match.groupdict()
        records_by_family[record["family"]].append(record)
    for family, records in records_by_family.items():
        if not records:
            report_transcript(transcript)
            fail(f"the {family} worker emitted no trace records")
        terminals = [index for index, record in enumerate(records) if record["terminal"]]
        if len(terminals) != 1:
            report_transcript(transcript)
            fail(f"the {family} trace carries {len(terminals)} terminal records, expected 1")
        if terminals[0] != len(records) - 1:
            report_transcript(transcript)
            fail(f"the {family} terminal record is not the last record in its trace")
        resource_records = [record for record in records if record["kind"] == "resource"]
        by_event: dict[int, list[int]] = {}
        for record in resource_records:
            by_event.setdefault(int(record["event"]), []).append(int(record["high_water"]))
        completes = by_event.pop(FABRIC_TRACE_RESOURCE_COMPLETE, [])
        if completes != [0]:
            report_transcript(transcript)
            fail(f"the {family} worker's terminal resource record was {completes}, expected [0]")
        for event, name, expected_count in EXPECTED_RESOURCES[family]:
            observed = by_event.pop(event, [])
            if len(observed) != expected_count:
                report_transcript(transcript)
                fail(
                    f"the {family} worker emitted {len(observed)} {name!r} resource "
                    f"record(s) (event={event}), expected {expected_count}"
                )
            if expected_count == 2 and name == "retained":
                # Not asserted zero: `operation_broker.rs`'s own close comment
                # says why -- an unclaimed result stays live until it expires
                # even after every client is gone, since expiry (not client
                # presence) ends its retrievability window, so a legitimate run
                # can still hold one when the plane closes. Bounded by the peak
                # is still a real assertion: a baseline that exceeded the run's
                # own historical high-water mark would be incoherent evidence.
                if observed[1] > observed[0]:
                    report_transcript(transcript)
                    fail(
                        f"the {family} worker's {name!r} baseline {observed[1]} exceeded "
                        f"its own peak {observed[0]}"
                    )
            elif expected_count == 2 and name == "mapping":
                # C8.13.1: asserted *constant*, not drained, and that is the
                # assertion rather than a weakening of one. A stream
                # participant's ring is mapped once when its role is
                # provisioned and stays mapped as long as the broker lives, so
                # this counter's baseline equalling its peak is what "no ring
                # was mapped or unmapped outside provisioning" looks like --
                # measured as a flat 6 across repeated boots. A baseline that
                # had drained to zero here would mean the broker lost a mapping
                # it still needs. The traffic-varying half of this holder's
                # occupancy is `loan` below, which does drain.
                if observed[1] != observed[0]:
                    report_transcript(transcript)
                    fail(
                        f"the {family} worker's {name!r} baseline {observed[1]} differs from "
                        f"its peak {observed[0]}; a provisioned mapping is not released "
                        "while the broker lives"
                    )
                if observed[0] == 0:
                    report_transcript(transcript)
                    fail(
                        f"the {family} worker reported no {name!r} occupancy at all; the "
                        "self-scoped query answered zero where the graph provisions rings"
                    )
            elif expected_count == 2 and name == "loan":
                # C8.13.1: the traffic-varying half, so the peak is asserted
                # nonzero -- a run that stopped minting downstream loans would
                # otherwise report a flat zero pair and pass every other check
                # here, which is exactly the degenerate evidence this counter
                # exists to rule out. Measured 5 at peak across repeated boots.
                if observed[0] == 0:
                    report_transcript(transcript)
                    fail(
                        f"the {family} worker's {name!r} peak was 0; this holder lends a "
                        "ring to every provisioned participant, so a zero peak means the "
                        "occupancy query or the loan path regressed"
                    )
                # Bounded by the peak rather than asserted zero. A ring loan is
                # settled when the root reclaims its *receiver*, and this loop's
                # exit condition proves the death of the two subscribers and the
                # clock peer but never inspects `fabric-publisher`'s liveness --
                # `publisher.finished` is set from `FLAG_LAST` on its last
                # sample, strictly before its exit. Observed 0 on five
                # consecutive boots, but that is scheduling margin (the broker
                # still owes the clock peer several round trips while the
                # publisher returns), not an invariant the code states. Asserting
                # 0 would make this gate fail with an occupancy message about a
                # task-teardown race; `baseline <= peak` is the claim the code
                # actually guarantees, the same treatment `retained` gets above
                # and for the same reason.
                if observed[1] > observed[0]:
                    report_transcript(transcript)
                    fail(
                        f"the {family} worker's {name!r} baseline {observed[1]} exceeded "
                        f"its own peak {observed[0]}"
                    )
            elif expected_count == 2 and observed[1] != 0:
                report_transcript(transcript)
                fail(
                    f"the {family} worker's {name!r} baseline was {observed[1]}, "
                    "expected 0 once every holder released"
                )
        if by_event:
            report_transcript(transcript)
            fail(f"the {family} worker emitted undeclared resource events: {sorted(by_event)}")

        summaries = [m for m in SUMMARY_PATTERN.finditer(head) if m.group("family") == family]
        if len(summaries) != 1:
            fail(f"the {family} worker did not close its trace exactly once")
        summary = summaries[0]
        if int(summary.group("rejected")) != 0:
            report_transcript(transcript)
            fail(
                f"the {family} worker emitted {summary.group('rejected')} record(s) its "
                "own validator refused"
            )
        if int(summary.group("dropped")) != 0:
            report_transcript(transcript)
            fail(
                f"the {family} trace sink dropped {summary.group('dropped')} record(s); "
                "raise the fixture's declared traceDepth"
            )
        if int(summary.group("records")) != len(records):
            report_transcript(transcript)
            fail(
                f"the {family} sink reports {summary.group('records')} records but the "
                f"transcript carries {len(records)}"
            )
        if int(summary.group("records")) > int(summary.group("capacity")):
            report_transcript(transcript)
            fail(f"the {family} sink reports more records than its declared capacity")


def check_transcript(transcript: str) -> None:
    for pattern in FAILURE_MARKERS:
        match = re.search(pattern, transcript)
        if match is not None:
            report_transcript(transcript)
            fail(f"failure marker in serial transcript: {match.group(0)!r}")
    for label, chain in CHAINS:
        position = 0
        for pattern in chain:
            match = re.compile(pattern).search(transcript, position)
            if match is None:
                report_transcript(transcript)
                if re.search(pattern, transcript) is not None:
                    fail(f"{label}: marker out of order: {pattern}")
                fail(f"{label}: missing marker: {pattern}")
            position = match.end()
    check_task_lifecycle(transcript)
    check_concurrency(transcript)
    check_resources(transcript)
    print(
        f"transcript: {sum(len(chain) for _, chain in CHAINS)} markers observed across "
        f"{len(CHAINS)} causal chains; {len(EXPECTED_SPAWNED)} spawned participants ran "
        f"the stream, call, and operation planes concurrently, {len(EXPECTED_SPAWNED) - len(EXPECTED_PARKED)} "
        f"exited cleanly and {sorted(EXPECTED_PARKED)} stayed healthy-parked by design",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 traffic-plane image and assert C8.13"
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built image instead of rebuilding it first",
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    if not FIXTURE.is_file():
        fail(f"missing generation fixture {FIXTURE.relative_to(ROOT)}")
    pins = load_pins()
    if not arguments.no_build:
        build_image()
    check_manifest()
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    check_transcript(boot(profile))
    print(
        "seL4 traffic plane check: the stream, call, and operation planes ran real "
        "traffic concurrently under one fixed schedule, every declared resource "
        "ceiling reported bounded peak-and-baseline evidence with nothing dropped "
        "or rejected, and every spawned task settled -- either a clean exit or, for "
        "the two structural roles this plane does not drive, healthy-idle"
    )


if __name__ == "__main__":
    main()
