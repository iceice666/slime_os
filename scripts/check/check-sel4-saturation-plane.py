#!/usr/bin/env python3

"""C8.13 gate: declared resource ceilings driven to their exact bound on seL4.

`just sel4_traffic_check` proves the stream, call, and operation planes carry
real concurrent traffic with generous headroom against every declared
ceiling. This gate boots the identical `"traffic"` action and component
behavior against a second fixture (`sel4-saturation.zti` is `sel4-traffic.zti`
with `inFlightOperations` tightened from 4 to 2 and the generation number
changed; nothing else differs) and additionally requires three of the
declared resource classes to land *exactly* at their fixture's own bound
rather than comfortably under it:

* in-flight calls (`inFlightCalls`): already exactly matched by the unmodified
  traffic scenario (4 declared, 4 observed), so this fixture leaves it
  unchanged and asserts the coincidence is real rather than assumed.
* in-flight operations (`inFlightOperations`): tightened from the traffic
  fixture's 4 to 2, the scenario's own observed peak.
* retained operation results (`retainedSamples`): already exactly matched (4
  declared, 4 observed), left unchanged and asserted for the same reason as
  in-flight calls.

`check_saturation` reads each declared ceiling back out of the fixture itself
rather than restating the numbers, so a future edit that loosens
`sel4-saturation.zti` back toward the traffic fixture's headroom fails this
gate instead of silently passing it.

Everything `check-sel4-traffic-plane.py` already asserts -- interleaving,
clean task lifecycle, bounded trace evidence with nothing dropped or
rejected -- is asserted here too: a ceiling driven to its exact bound must
still complete without deadlocking a route worker, which is the milestone's
required check. Two independent things catch an undersized ceiling actually
breaking something: `check_task_lifecycle`'s `COMPONENT_FAILURE` pattern
catches a component's own `fail()` writing `[fabric*] fail: ...`, and
`FAILURE_MARKERS` catches the coarser admission-time and fault-level failures
(a spawn/capability refusal, a kernel fault) that a tightened *graph-level*
quota rather than one worker's own runtime table would produce.

Not driven to an exact declared bound: `mappings`, `loans`,
`bufferPages`/`buffers` (graph-wide and per-holder `sharedBufferBudget`
quotas), `retries` (real evidence now, inherited from the traffic plane's own
QoS-timed clock, but not asserted equal to the fixture's declared `retries`
ceiling the way the three `SATURATED_CEILINGS` entries are), `eventDepth`,
and `capabilitySlots`. The last of those is no longer *unmeasured* -- C8.13.3
added the root-side census and this gate checks the broker's live occupancy
against the declared ceiling -- it is simply well under it, which is a
property of the graph rather than of the fixture. See the roadmap's C8.13
section for why each remains open.
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

from fabric_graph_limits import declared_limits  # noqa: E402
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
IMAGE = ROOT / "build" / "slime-sel4-saturation.elf"
MANIFEST = ROOT / "build" / "slime-sel4-saturation.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-saturation.zti"
IMAGE_VARIANT = "saturation"
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
# brokers race over and this gate deliberately does not order. Identical to
# `check-sel4-traffic-plane.py`'s chain except the admitted generation number:
# `sel4-saturation.zti` is `sel4-traffic.zti` with its resource ceilings
# tightened, not its structure changed, so every other admitted count matches.
CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "the generation was admitted with its declared partition",
        (
            r"SLIME_ROOT generation admitted number=39 executables=20 instances=20 "
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
EXPECTED_PARKED = frozenset({"fabric-observer", "fabric-proxy"})

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

# The three ceilings this fixture deliberately tightens or confirms already
# tight, by trace family and RESOURCE_* event code: `(event, name, limits
# field)`. The bound itself is read from the fixture's own declared
# `fabricGraph.limits` block (`declared_limits`) rather than restated here,
# so a future edit that loosens the fixture is caught instead of silently
# passing. A peak strictly under its declared ceiling here would mean the
# fixture is not actually adversarial -- generous headroom dressed up as a
# saturation test -- so `check_saturation` requires equality, not merely a
# bounded value.
SATURATED_CEILINGS: tuple[tuple[str, int, str, str], ...] = (
    ("call", FABRIC_TRACE_RESOURCE_CALLS, "in-flight calls", "inFlightCalls"),
    ("operation", FABRIC_TRACE_RESOURCE_OPERATIONS, "in-flight operations", "inFlightOperations"),
    ("operation", FABRIC_TRACE_RESOURCE_RETAINED, "retained results", "retainedSamples"),
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 saturation plane check: {message}")


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
    command = [sys.executable, str(BUILD_SCRIPT), "--saturation-plane"]
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
            "run `just sel4_saturation_check`"
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
            "rebuild with `--saturation-plane`"
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
                        f"saturation spawn records named multiple init tasks: "
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


def check_saturation(transcript: str) -> None:
    """The three deliberately tightened ceilings land exactly at their
    fixture's own declared bound, not comfortably under it -- the property
    that distinguishes a saturation fixture from a smaller traffic fixture."""
    head = composition(transcript)
    limits = declared_limits(FIXTURE)
    peaks: dict[tuple[str, int], int] = {}
    for match in TRACE_PATTERN.finditer(head):
        record = match.groupdict()
        # No explicit terminal check: `RESOURCE_COMPLETE`'s event code never
        # coincides with any code named in `SATURATED_CEILINGS`, so the
        # terminal record cannot be mistaken for one of these peaks.
        if record["kind"] != "resource":
            continue
        key = (record["family"], int(record["event"]))
        value = int(record["high_water"])
        if key not in peaks or value > peaks[key]:
            peaks[key] = value
    for family, event, name, field in SATURATED_CEILINGS:
        ceiling = limits.get(field)
        if ceiling is None:
            fail(f"the fixture declares no {field!r} limit")
        observed = peaks.get((family, event))
        if observed is None:
            report_transcript(transcript)
            fail(f"the {family} worker emitted no {name!r} resource record to check")
        if observed != ceiling:
            report_transcript(transcript)
            fail(
                f"the {family} worker's {name!r} peak was {observed}, expected exactly "
                f"{ceiling} (this fixture's declared {field!r}) -- not a saturation test "
                "if the peak sits under the bound"
            )


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
    check_saturation(transcript)
    print(
        f"transcript: {sum(len(chain) for _, chain in CHAINS)} markers observed across "
        f"{len(CHAINS)} causal chains; {len(EXPECTED_SPAWNED)} spawned participants ran "
        f"the stream, call, and operation planes concurrently against tightened "
        f"ceilings, {len(EXPECTED_SPAWNED) - len(EXPECTED_PARKED)} exited cleanly and "
        f"{sorted(EXPECTED_PARKED)} stayed healthy-parked by design",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 saturation-plane image and assert C8.13's tightened ceilings"
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
        "seL4 saturation plane check: in-flight calls, in-flight operations, and "
        "retained operation results each landed exactly at this fixture's declared "
        "ceiling with nothing dropped, rejected, or exceeded, and the stream, call, "
        "and operation planes still completed concurrently without deadlocking a "
        "route worker"
    )


if __name__ == "__main__":
    main()
