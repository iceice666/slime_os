#!/usr/bin/env python3

"""C8.14 gate: degradation and fault isolation on seL4.

This plane is `sel4-traffic.zti` with `generation` changed and nothing else,
built with the proxy early-death injection enabled. That asymmetry is the
milestone's central finding rather than a shortcut: C8.13's concurrent graph
*already drives* every degradation and terminal condition C8.14 names, through
its own scripted participants, and nothing asserted any of it.

Measured on a traffic boot before this gate existed, the graph emitted 6
`kind=denial` and 3 `kind=fault` records, plus QoS records for timeout and
expiry, and component markers for rejection, malformed reply, retry exhaustion,
cancellation, duplicate, stale session, unauthorized cancel, unauthorized
retrieval, forged transport record, and two scripted peer deaths. All of it was
unchecked. So this gate's job is to *require* that vocabulary rather than to
invent a new scenario for it.

One condition genuinely could not be scripted. A declared interposition hop
dying is not something any participant can ask for -- a proxy that relays
correctly cannot also be absent -- and under the traffic action the hop parks
forever (`fabric_boot::park_only` is `-> !`). So this variant compiles the hop
to exit instead, which is why it needs its own image rather than only its own
assertions.

What this gate requires, beyond everything `check-sel4-traffic-plane.py`
already requires of the same graph:

* **Every declared degradation is a distinct record.** Each condition in
  `EXPECTED_DENIALS`, `EXPECTED_QOS_DEGRADATION`, and `EXPECTED_FAULTS` must
  appear with its own status or event code. A single code standing in for two
  conditions fails, which is what "distinguishable" has to mean for a reader
  who only has the transcript.
* **Every fault path settles.** Each plane's broker must report its state
  reclaimed, and the resource evidence `check_resources` inherits must still
  return every counter to its declared baseline -- a fault that leaked a loan,
  mapping, buffer, or correlation would show up there as a baseline above zero.
* **No fault crosses a route class.** The injected hop death and both scripted
  peer deaths must leave every unrelated stream, call, and operation route
  completing anyway: `EXPECTED_ISOLATION` pins the markers each plane emits to
  say so in its own words, and `check_concurrency` still requires the three
  planes to interleave, so a fault that serialized the schedule fails too.
* **The injection took.** `fabric-proxy` must record its injected death and
  exit, rather than parking as it does on every other plane. Without this the
  whole variant could pass as a second traffic boot.

Nothing here asserts a *new* status code, and that is deliberate: C8.14 is an
exercise-and-observe milestone over machinery C8.4-C8.9 built. The one code
change it needs is the injection above.
"""

from __future__ import annotations

import argparse
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

from fabric_graph_limits import declared_limits  # noqa: E402
from harness import profile_text, profile_integer, sha256_file  # noqa: E402
from fabric_trace_contract import (  # noqa: E402
    FABRIC_TRACE_RESOURCE_BUFFERS,
    FABRIC_TRACE_RESOURCE_CALLS,
    FABRIC_TRACE_RESOURCE_CAPABILITY_SLOTS,
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
IMAGE = ROOT / "build" / "slime-sel4-fault.elf"
MANIFEST = ROOT / "build" / "slime-sel4-fault.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-traffic.zti"
IMAGE_VARIANT = "fault"
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
    # C8.13.3: the root reports rather than refuses on an occupancy breach, so
    # these keep the report guarded rather than left as prose.
    r"SLIME_GRAPH cspace occupancy over-ceiling .*",
    r"SLIME_GRAPH cspace occupancy over-capacity .*",
    r"SLIME_GRAPH cspace occupancy refused .*",
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
# brokers race over and this gate deliberately does not order. Identical to
# `check-sel4-traffic-plane.py`'s chain except the admitted generation number.
#
# B62: this plane shares `sel4-traffic.zti`. It was a full 1882-line copy
# differing only in `generation`, which `build-sel4.py` now supplies as a
# declared per-variant delta, so every other admitted count matches by
# construction rather than by two fixtures happening to agree. What differs is
# the *build*: this variant is compiled with the proxy early-death injection
# enabled.
CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "the generation was admitted with its declared partition",
        (
            r"SLIME_ROOT generation admitted number=40 executables=20 instances=20 "
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

# Same family set as the traffic gate: this plane runs the identical `"traffic"`
# action, so every emitter there reports here too.
TRACE_FAMILIES = (
    "stream",
    "call",
    "operation",
    "publisher",
    "publisher-b",
    "subscriber",
    "subscriber-b",
)
# Longest first, so `publisher` cannot match inside `publisher-b`.
_FAMILY_ALTERNATION = "|".join(sorted(TRACE_FAMILIES, key=len, reverse=True))

TRACE_PATTERN = re.compile(
    rf"\[trace\] (?P<family>{_FAMILY_ALTERNATION}) kind=(?P<kind>\w+) "
    r"order=(?P<order>[\w-]+) now=(?P<now>\d+) route=(?P<route>[0-9a-f]{16}) "
    r"correlation=(?P<correlation>\d+) sequence=(?P<sequence>\d+) "
    r"status=(?P<status>-?\d+) event=(?P<event>\d+) high_water=(?P<high_water>\d+)"
    r"(?P<terminal> terminal)?"
)
SUMMARY_PATTERN = re.compile(
    rf"\[trace\] (?P<family>{_FAMILY_ALTERNATION}) complete "
    r"capacity=(?P<capacity>\d+) records=(?P<records>\d+) "
    r"dropped=(?P<dropped>\d+) rejected=(?P<rejected>\d+)"
)

# Every task `drive_traffic_plane` spawns, in the fixed order it spawns them --
# identical to the traffic plane's, since this fixture changes declared
# ceilings, not the participant set.
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
# C8.14: only the filtered-view observer still parks. The declared
# interposition hop is compiled to die instead -- see this gate's docstring --
# so it is expected to *exit*, and `EXPECTED_PROXY_DEATH` below is what proves
# the injection actually took rather than the component quietly parking anyway.
EXPECTED_PARKED = frozenset({"fabric-observer"})
EXPECTED_PROXY_DEATH = "fabric-proxy"

# Same convention as `check-sel4-traffic-plane.py`'s `EXPECTED_RESOURCES`:
# every counter the traffic plane already emits must still emit here,
# unregressed by the tightened ceilings.
EXPECTED_RESOURCES: dict[str, tuple[tuple[int, str, int], ...]] = {
    "stream": (
        (FABRIC_TRACE_RESOURCE_FRAMES, "frames", 2),
        (FABRIC_TRACE_RESOURCE_BUFFERS, "buffers", 2),
        (FABRIC_TRACE_RESOURCE_QUEUE, "queue", 2),
        (FABRIC_TRACE_RESOURCE_HISTORY, "history", 2),
        (FABRIC_TRACE_RESOURCE_RETRIES, "retries", 1),
        # C8.13.1: emitted by the stream worker under the traffic action, which
        # this plane also runs, so neither may regress here either.
        (FABRIC_TRACE_RESOURCE_MAPPING, "mapping", 2),
        (FABRIC_TRACE_RESOURCE_LOAN, "loan", 2),
        # C8.13.3: the broker's own live child-CSpace occupancy, likewise
        # emitted under the traffic action this plane runs. Checked against
        # this fixture's own declared `capabilitySlots`, which the tightened
        # ceilings leave unchanged at 48.
        (FABRIC_TRACE_RESOURCE_CAPABILITY_SLOTS, "capability-slots", 2),
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
    # C8.13.2: the four stream participants report their own mapping occupancy
    # under the traffic action, which this plane also runs, so none may regress
    # here either.
    "publisher": ((FABRIC_TRACE_RESOURCE_MAPPING, "mapping", 2),),
    "publisher-b": ((FABRIC_TRACE_RESOURCE_MAPPING, "mapping", 2),),
    "subscriber": ((FABRIC_TRACE_RESOURCE_MAPPING, "mapping", 2),),
    "subscriber-b": ((FABRIC_TRACE_RESOURCE_MAPPING, "mapping", 2),),
}

# C8.13.2: the exact count each participant must report; see the traffic gate
# for why the value is pinned rather than merely required nonzero.
PARTICIPANT_MAPPINGS: dict[str, int] = {
    "publisher": 1,
    "publisher-b": 2,
    "subscriber": 1,
    "subscriber-b": 2,
}



def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 fault plane check: {message}")


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


def build_image() -> None:
    command = [sys.executable, str(BUILD_SCRIPT), "--fault-plane"]
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
            "run `just sel4_fault_check`"
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
            "rebuild with `--fault-plane`"
        )
    image = manifest.get("image")
    if not isinstance(image, dict) or not isinstance(image.get("sha256"), str):
        fail("identity manifest does not record the packaged image digest")
    if not IMAGE.is_file():
        fail(f"missing packaged image {IMAGE.relative_to(ROOT)}")
    actual = sha256_file(IMAGE, fail)
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
        profile_text(profile, "machine", fail),
        "-cpu",
        profile_text(profile, "cpu", fail),
        "-smp",
        str(profile_integer(profile, "cpus", fail)),
        "-m",
        f"size={profile_integer(profile, 'memory_mib', fail)}M",
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
                        f"fault spawn records named multiple init tasks: "
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

# C8.14: every refusal the concurrent graph drives, by trace family and the
# status code the broker records it under. `trace_denial` negates the protocol
# status (`denial_status` in both brokers), so a `STATUS_STALE` of 8 arrives as
# -8; the codes below are those negated values, taken from
# `components/proto/src/fabric_{call,operation}.rs`.
#
# A denial record carries no route identity and no correlation, by design: the
# refusal exists to withhold exactly that metadata, so this table is keyed on
# the status alone. What it proves is *distinctness* -- each condition arrives
# under its own code, so a reader with only the transcript can tell a duplicate
# from a stale session from a retry exhaustion.
EXPECTED_DENIALS: dict[str, tuple[tuple[int, str], ...]] = {
    # -4 is STATUS_RETRY_EXHAUSTED, -7 is STATUS_DUPLICATE.
    "call": ((-4, "retry exhausted"), (-7, "duplicate request")),
    # -6 is STATUS_DUPLICATE, -7 is STATUS_STALE.
    "operation": ((-6, "duplicate goal"), (-7, "stale or unauthorized session")),
}

# The QoS degradations the graph drives, by family and (status, event). These
# travel as `kind=qos` records rather than denials because a degradation is a
# property of a live edge, not a refusal of a request.
EXPECTED_QOS_DEGRADATION: dict[str, tuple[tuple[int, int, str], ...]] = {
    # STATUS_TIMEOUT (2) reported as EVENT_DEADLINE_MISSED (6).
    "call": ((2, 6, "call timeout"),),
    # STATUS_EXPIRED (3) as EVENT_LIFESPAN_EXPIRED (4); STATUS_TIMEOUT (4) as
    # EVENT_DEADLINE_MISSED (6). The two share neither code, which is the point.
    "operation": (
        (3, 4, "retained result expiry"),
        (4, 6, "operation timeout"),
    ),
}

# Peer death is the one terminal condition every plane must report, and it is a
# `kind=fault` record on the `peer-death` order class rather than a denial or a
# QoS event -- a distinct third shape, which is what makes it distinguishable
# from both. `event=8` is EVENT_PEER_DEAD; the status is the negated protocol
# peer-dead code for that plane.
#
# All three planes appear, and all three records are the *broker's* observation
# of a peer that exited: the call and operation servers are scripted to exit
# mid-exchange, and this plane's telemetry publisher is scripted to exit without
# ending its stream. The stream record was previously produced by a race rather
# than a scripted death -- the publisher ended its route normally, and whether
# the broker saw the terminal sample or the exit first decided whether the
# record appeared at all -- so it was present on some boots and absent on
# others. A plane missing from this transcript would mean a peer died and the
# broker did not notice, which is the failure mode that wedges a route worker
# forever.
EXPECTED_FAULTS: tuple[str, ...] = ("call", "operation", "stream")
PEER_DEATH_EVENT = 8

# Each plane's own words for "a fault landed and an unrelated route kept going".
# Pinned as markers rather than inferred from record counts because isolation is
# a claim about what *did not* happen to a neighbour, which no counter states.
EXPECTED_ISOLATION: tuple[tuple[str, str], ...] = (
    (
        "the call plane settles every in-flight correlation on peer loss",
        r"\[fabric\] call peer death propagated",
    ),
    (
        "the call plane reclaims its correlation table afterward",
        r"\[fabric\] call state reclaimed",
    ),
    (
        "an unrelated call route survives the fault",
        r"\[fabric-call-client-b\] unrelated route intact",
    ),
    (
        "the operation plane settles every in-flight goal on peer loss",
        r"\[fabric\] operation peer death propagated",
    ),
    (
        "the operation plane reclaims its operation table afterward",
        r"\[fabric\] operation state reclaimed",
    ),
    (
        "an unrelated operation route survives the fault",
        r"\[fabric\] unrelated operation route live",
    ),
    (
        "a concurrent participant observes the peer fault as isolated",
        r"\[fabric-op-client-b\] concurrent peer fault isolated",
    ),
    (
        "the stream plane completes despite its clock peer leaving",
        r"\[fabric\] traffic stream plane complete",
    ),
)

# Degradations the participants themselves report as distinct. These are
# component-side markers rather than broker records, and they are required
# because several of these conditions are only ever *observed* by the caller --
# a malformed reply is rejected by the broker but distinguished by the client.
EXPECTED_DISTINCT_DEGRADATIONS: tuple[tuple[str, str], ...] = (
    ("a server rejection is distinguishable", r"\[fabric-call-client\] rejection distinct"),
    ("a malformed reply is distinguishable", r"\[fabric-call-client\] malformed reply distinct"),
    ("a call timeout is distinguishable", r"\[fabric-call-client\] timeout distinct"),
    (
        "retry exhaustion is distinguishable",
        r"\[fabric-call-client\] retry exhaustion distinct",
    ),
    ("call peer death is distinguishable", r"\[fabric-call-client\] peer death distinct"),
    ("a duplicate request is refused", r"\[fabric-call-client-b\] duplicate rejected"),
    ("a stale session is refused", r"\[fabric-call-client-b\] stale session observed"),
    ("a cancellation settles once", r"\[fabric-call-client-b\] cancellation observed"),
    (
        "terminal backpressure recovers rather than wedging",
        r"\[fabric-call-client-b\] terminal backpressure recovered",
    ),
    ("an operation rejection is distinguishable", r"\[fabric-op-client\] rejection distinct"),
    ("an operation timeout is distinguishable", r"\[fabric-op-client\] timeout distinct"),
    ("operation peer death is distinguishable", r"\[fabric-op-client\] peer death distinct"),
    ("a duplicate goal is refused", r"\[fabric-op-client\] duplicate goal rejected"),
    ("a retained result expires observably", r"\[fabric-op-client\] result expiry observed"),
    (
        "an unauthorized cancel is refused",
        r"\[fabric-op-client-b\] unauthorized cancel denied",
    ),
    (
        "an unauthorized retrieval is refused",
        r"\[fabric-op-client-b\] unauthorized retrieval denied",
    ),
    (
        "a forged transport record is refused",
        r"\[fabric-op-client-b\] forged transport record denied",
    ),
    ("an ungranted component is denied a role", r"\[fabric\] ungranted component denied: "),
)

# The injected fault itself. Without this the variant could pass as a second
# traffic boot: the hop parks on every other plane, so its death is the only
# thing distinguishing this image.
EXPECTED_INJECTION = r"\[fabric-proxy\] injected early proxy death"

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
    """Stream, call, and operation traffic interleave under one schedule, the
    same property `check-sel4-traffic-plane.py` requires -- a tightened
    ceiling must not collapse the schedule into three sequential phases."""
    head = composition(transcript)
    positions: dict[str, list[int]] = {
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
    on all three broker planes and the four instrumented participants, through a
    sink that dropped and rejected nothing -- identical to
    `check-sel4-traffic-plane.py`'s assertion, unregressed by the tightened
    ceilings."""
    head = composition(transcript)
    # Read once; the ceiling is constant for the whole run.
    limits = declared_limits(FIXTURE)
    records_by_family: dict[str, list[dict[str, str]]] = {
        family: [] for family in TRACE_FAMILIES
    }
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
                if observed[1] > observed[0]:
                    report_transcript(transcript)
                    fail(
                        f"the {family} worker's {name!r} baseline {observed[1]} exceeded "
                        f"its own peak {observed[0]}"
                    )
            elif expected_count == 2 and name == "mapping":
                # C8.13.1/C8.13.2: constant by design and asserted nonzero,
                # exactly as the traffic gate does. Both halves are needed:
                # without the nonzero check a query that regressed to answering
                # all zeros would satisfy `0 == 0` and pass vacuously, and this
                # plane runs the same emitters under the same `"traffic"` boot
                # action, so it has the same standing to falsify that. For the
                # four participants the equality is structural -- one read
                # recorded twice -- so the pin below is what constrains them.
                #
                # `capability-slots` is deliberately not in this branch: it is
                # genuinely held and released, so it takes `loan`'s
                # bounded-by-peak shape below.
                if observed[1] != observed[0]:
                    report_transcript(transcript)
                    fail(
                        f"the {family} holder's {name!r} baseline {observed[1]} differs from "
                        f"its peak {observed[0]}; a provisioned mapping is not released "
                        "while its holder lives"
                    )
                if observed[0] == 0:
                    report_transcript(transcript)
                    fail(
                        f"the {family} holder reported no {name!r} occupancy at all; the "
                        "self-scoped query answered zero where the graph provisions regions"
                    )
            elif expected_count == 2 and name == "loan":
                # C8.13.1: nonzero peak, baseline bounded by it rather than
                # asserted zero -- see the traffic gate for why a ring loan's
                # settlement depends on receiver teardown this loop does not
                # order.
                if observed[0] == 0:
                    report_transcript(transcript)
                    fail(
                        f"the {family} worker's {name!r} peak was 0; this holder lends a "
                        "ring to every provisioned participant, so a zero peak means the "
                        "occupancy query or the loan path regressed"
                    )
                if observed[1] > observed[0]:
                    report_transcript(transcript)
                    fail(
                        f"the {family} worker's {name!r} baseline {observed[1]} exceeded "
                        f"its own peak {observed[0]}"
                    )
            elif expected_count == 2 and name == "capability-slots":
                # C8.13.3: held and released, as in the traffic gate -- the
                # broker drops the supervision handles it no longer waits on, so
                # the count rises and partially drains rather than staying flat.
                if observed[0] == 0:
                    report_transcript(transcript)
                    fail(
                        f"the {family} holder's {name!r} peak was 0; this broker holds a "
                        "control endpoint per participant, so a zero peak means the query "
                        "or the credit path regressed"
                    )
                if observed[1] > observed[0]:
                    report_transcript(transcript)
                    fail(
                        f"the {family} holder's {name!r} baseline {observed[1]} exceeded "
                        f"its own peak {observed[0]}"
                    )
            elif expected_count == 2 and observed[1] != 0:
                report_transcript(transcript)
                fail(
                    f"the {family} worker's {name!r} baseline was {observed[1]}, "
                    "expected 0 once every holder released"
                )
            # C8.13.2: pinned per participant, as in the traffic gate.
            if name == "mapping" and family in PARTICIPANT_MAPPINGS:
                expected_mapping = PARTICIPANT_MAPPINGS[family]
                if observed[0] != expected_mapping:
                    report_transcript(transcript)
                    fail(
                        f"the {family} participant reported {observed[0]} mapping(s), "
                        f"expected exactly {expected_mapping}"
                    )
            # C8.13.3: declared-space occupancy against this fixture's own
            # declared ceiling, as in the traffic gate -- not the physical CNode
            # count, which the same reply also carries but which this ceiling
            # does not bound. `capabilitySlots` is one of the limits this fixture
            # leaves at the traffic plane's value, so the check is that
            # tightening elsewhere did not push real occupancy over a bound that
            # did not move.
            if name == "capability-slots":
                ceiling = limits.get("capabilitySlots")
                if ceiling is None:
                    fail("the fixture declares no 'capabilitySlots' limit")
                if observed[0] == 0:
                    report_transcript(transcript)
                    fail(
                        f"the {family} holder reported 0 occupied declared slots; this broker "
                        "holds a control endpoint per participant plus its own factories"
                    )
                if observed[0] > ceiling:
                    report_transcript(transcript)
                    fail(
                        f"the {family} holder occupies {observed[0]} declared capability "
                        f"slots, exceeding the {ceiling} its generation declares as "
                        "'capabilitySlots'"
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
                "the declared ceiling was tightened past what fits the fixed traceDepth"
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


def check_injection(transcript: str) -> None:
    """The injected hop death actually happened, and the hop exited."""
    head = composition(transcript)
    if re.search(EXPECTED_INJECTION, head) is None:
        report_transcript(transcript)
        fail(
            "the declared interposition hop never recorded its injected death; this "
            "image is indistinguishable from a plain traffic boot, so rebuild with "
            "`--fault-plane`"
        )
    spawns = SPAWN_PATTERN.findall(head)
    proxy = next((match[1] for match in spawns if match[2] == EXPECTED_PROXY_DEATH), None)
    if proxy is None:
        fail(f"{EXPECTED_PROXY_DEATH} was never spawned")
    exits = [int(status) for task, status in EXIT_PATTERN.findall(transcript) if task == proxy]
    if exits != [0]:
        report_transcript(transcript)
        fail(
            f"{EXPECTED_PROXY_DEATH} task {proxy} exit statuses were {exits}, expected [0] -- "
            "an injected departure is declared, not a failure, so it exits cleanly"
        )


def check_distinct_degradations(transcript: str) -> None:
    """Every declared degradation and terminal condition arrives under its own
    code, and the participants that can only observe one report it.

    Distinctness is asserted as *disjointness of codes within a family*, not
    merely presence: a broker that collapsed two conditions onto one status
    would still emit both records, and a reader with only the transcript could
    no longer tell them apart. That is the property this milestone names."""
    head = composition(transcript)
    records: dict[tuple[str, str], list[tuple[int, int]]] = {}
    for match in TRACE_PATTERN.finditer(head):
        record = match.groupdict()
        key = (record["family"], record["kind"])
        records.setdefault(key, []).append((int(record["status"]), int(record["event"])))

    for family, expected in EXPECTED_DENIALS.items():
        observed = records.get((family, "denial"), [])
        if not observed:
            report_transcript(transcript)
            fail(f"the {family} plane recorded no denial at all")
        codes = {status for status, _ in observed}
        for status, name in expected:
            if status not in codes:
                report_transcript(transcript)
                fail(
                    f"the {family} plane recorded no denial for {name} (status={status}); "
                    f"observed {sorted(codes)}"
                )
        if len(codes) < len({status for status, _ in expected}):
            report_transcript(transcript)
            fail(
                f"the {family} plane's denials collapsed onto {sorted(codes)}, fewer codes "
                "than the conditions it drives -- a reader cannot distinguish them"
            )

    for family, expected in EXPECTED_QOS_DEGRADATION.items():
        observed = records.get((family, "qos"), [])
        if not observed:
            report_transcript(transcript)
            fail(f"the {family} plane recorded no QoS degradation at all")
        for status, event, name in expected:
            if (status, event) not in observed:
                report_transcript(transcript)
                fail(
                    f"the {family} plane recorded no {name} "
                    f"(status={status} event={event}); observed {sorted(set(observed))}"
                )

    for family in EXPECTED_FAULTS:
        observed = records.get((family, "fault"), [])
        if not observed:
            report_transcript(transcript)
            fail(
                f"the {family} plane recorded no peer-death fault; a peer left and the "
                "broker did not notice, which is what wedges a route worker forever"
            )
        if not any(event == PEER_DEATH_EVENT for _, event in observed):
            report_transcript(transcript)
            fail(
                f"the {family} plane's fault records carry events {sorted({e for _, e in observed})}, "
                f"none of them EVENT_PEER_DEAD ({PEER_DEATH_EVENT})"
            )

    for label, pattern in EXPECTED_DISTINCT_DEGRADATIONS:
        if re.search(pattern, head) is None:
            report_transcript(transcript)
            fail(f"{label}: missing marker: {pattern}")


def check_isolation(transcript: str) -> None:
    """No fault crossed a route class: every plane settled its own in-flight
    state and every unrelated route completed anyway."""
    head = composition(transcript)
    for label, pattern in EXPECTED_ISOLATION:
        if re.search(pattern, head) is None:
            report_transcript(transcript)
            fail(f"{label}: missing marker: {pattern}")

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
    check_injection(transcript)
    check_distinct_degradations(transcript)
    check_isolation(transcript)
    denials = sum(len(codes) for codes in EXPECTED_DENIALS.values())
    degradations = sum(len(codes) for codes in EXPECTED_QOS_DEGRADATION.values())
    print(
        f"transcript: {sum(len(chain) for _, chain in CHAINS)} markers observed across "
        f"{len(CHAINS)} causal chains; {len(EXPECTED_SPAWNED)} spawned participants ran "
        f"the stream, call, and operation planes concurrently while the declared "
        f"interposition hop died; {denials} distinct denial codes, {degradations} distinct "
        f"QoS degradations, and {len(EXPECTED_FAULTS)} peer-death faults observed, with "
        f"{len(EXPECTED_ISOLATION)} isolation markers intact",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description=(
            "Boot the seL4 fault-plane image and assert C8.14's degradation and "
            "fault-isolation envelope"
        )
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
    if not isinstance(profile, dict):
        fail("sel4/pins.toml [qemu_arm_virt] is not a table")
    check_transcript(boot(profile))
    print(
        "seL4 fault plane check: every declared degradation and terminal condition "
        "stayed bounded and distinguishable, every fault path settled its own "
        "in-flight state and returned each declared resource to baseline, and the "
        "injected interposition-hop death disrupted no unrelated stream, call, or "
        "operation route",
        flush=True,
    )


if __name__ == "__main__":
    main()
