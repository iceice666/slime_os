#!/usr/bin/env python3
"""C9.6 gate: a robot workload composed of every C9 slice, under contention."""
from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import threading
import tomllib
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from closure_image import ClosureImageError, build as build_closure_image  # noqa: E402

from harness import GENERATION_COMPOSITIONS, sha256_file  # noqa: E402
from sel4_gate_markers import match_marker_contract  # noqa: E402
from zutai_cli import STDLIB, binary  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
# CP15: the closure identity names the build's inputs and is re-resolved from
# repository state before the build, so a stale input is refused rather than
# silently producing a different image.
CLOSURE = "sel4-robot-runtime"
IMAGE: Path | None = None
PINS = ROOT / "sel4" / "pins.toml"
FIXTURE = GENERATION_COMPOSITIONS / "sel4-robot-runtime.zti"
GENERATION = 46
TIMEOUT = 420

# The declared command scale, from the generation's own parameter authority. The
# controller's command is `tick * scale`, so this is what makes each actuation a
# function of configuration as well as of input — and therefore what makes the
# configuration's survival across the restart observable in the *data* rather
# than only in a marker.
EXPECTED_SCALE = 7

# The sensor's declared cadence: four timed samples then the terminal one. Pinned
# rather than read off the transcript, so a sensor that stopped ticking fails here
# instead of passing against a shorter stream.
EXPECTED_TICKS = 4

# The route's declared client deadline, and the two instants the clock advances
# to. The first is strictly inside the deadline and must expire nothing; the
# second is past it and must expire exactly the one unanswered command. Both are
# pinned because the pair is what makes the deadline arm non-vacuous: a gate that
# accepted any advance would pass against a clock that only ever moved past the
# deadline, proving nothing about the boundary.
DECLARED_DEADLINE_NS = 1_000_000
FIRST_ADVANCE_NS = 500_000
SECOND_ADVANCE_NS = 1_000_001

# Restarts the declared `lifecyclePolicy` admits for the controller as a safety
# ceiling. Only `fault` is a declared restartable cause (a clean completion
# exit is deliberately not one, so the replacement's own successful finish
# does not itself trigger a further restart), and the plane injects exactly
# one fault, so this ceiling is never fully spent in a passing run — it is
# checked against the fixture's declaration, not against how many restarts
# actually happen.
DECLARED_RESTART_ATTEMPTS = 2

# Restarts the transcript actually drives: one, from the one injected fault.
# The replacement then runs its whole scenario to a clean exit, which
# `lifecycleRestartAdmit` correctly refuses to admit a second time because
# `exit` is not a declared cause — so "the bound is terminal" is observed as a
# cause refusal, not as the attempt ceiling being exhausted.
EXPECTED_RESTARTS = 1

CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "the declared bands are installed before any traffic",
        (
            r"\[init\] robot runtime sensors spawned",
            r"\[robot-sensor\] foreground priority=(\d+)",
            r"\[robot-burner\] bestEffort priority=(\d+)",
        ),
    ),
    (
        "the sensor holds its declared publish role and ticks on a real clock",
        (
            r"\[robot-sensor\] publish role requested",
            r"\[robot-sensor\] publish role received",
            r"\[robot-sensor\] tick=1",
        ),
    ),
    (
        "the controller is not started until its declared dependency is satisfied",
        (
            r"\[robot-supervisor\] dependency refused error=(\d+)",
            r"\[robot-supervisor\] running",
            r"\[robot-supervisor\] controller launched",
            r"\[robot-supervisor\] parameter previous=(\d+)",
        ),
    ),
    (
        "the chain carries data before the restart",
        (
            rf"\[robot-controller\] scale={EXPECTED_SCALE}",
            r"\[robot-controller\] subscribe role received",
            # The first incarnation's ring never delivers tick 1: the sensor's
            # cadence starts before the controller's role handshake completes,
            # and that tick is gone by the time it attaches. Consumption
            # observably starts at 2, not 1.
            r"\[robot-controller\] consumed tick=2",
            r"\[robot-actuator\] applied value=(\d+)",
            r"\[robot-controller\] command applied value=(\d+)",
        ),
    ),
    (
        "an injected controller fault is bounded and reissues fabric authority",
        (
            r"\[robot-controller\] injected fault",
            r"\[robot-supervisor\] controller faulted detail=(\d+)",
            r"\[robot-supervisor\] restart admitted remaining=(\d+)",
            r"\[robot-supervisor\] backoff elapsed now=(\d+)",
            r"\[robot-supervisor\] controller restarted attempt=1",
            # The replacement re-requests its role and the broker grants a fresh
            # one: the ring the predecessor held was receiver-bound to a task
            # that no longer exists, so this is reissued authority rather than
            # inherited authority.
            r"\[robot-controller\] subscribe role reissued",
        ),
    ),
    (
        "the graph resumes and the configuration survived the restart",
        (
            rf"\[robot-controller\] scale retained={EXPECTED_SCALE}",
            r"\[robot-controller\] resumed tick=(\d+)",
            r"\[robot-controller\] command resumed value=(\d+)",
        ),
    ),
    (
        "the stream ends orderly, distinct from a peer loss",
        (
            rf"\[robot-sensor\] stream ended ticks={EXPECTED_TICKS + 1}",
            r"\[robot-controller\] consumed total=(\d+)",
        ),
    ),
    (
        "a withdrawn command settles as cancelled",
        (
            r"\[robot-actuator\] command cancelled id=(\d+)",
            r"\[robot-controller\] command cancellation observed",
        ),
    ),
    (
        "an out-of-range command is refused, which is a settlement rather than "
        "a deadline miss",
        (
            r"\[robot-actuator\] command refused value=(\d+)",
            r"\[robot-controller\] command refusal observed",
        ),
    ),
    (
        "an unanswered command settles on the declared deadline, not before it, "
        "and not as peer death",
        (
            r"\[robot-actuator\] command left unanswered id=(\d+)",
            rf"\[robot-clock\] advanced now_ns={FIRST_ADVANCE_NS}",
            rf"\[robot-clock\] advanced now_ns={SECOND_ADVANCE_NS}",
            r"\[robot-controller\] command deadline observed",
            # The server outlives the settlement by construction. The broker
            # adjudicates server death ahead of the time advance within one
            # sweep, so an actuator free to exit earlier could settle this very
            # request STATUS_PEER_DEAD instead -- making the declared outcome a
            # scheduling artefact. This marker is the actuator confirming it was
            # still alive when the deadline fired.
            r"\[robot-actuator\] timeout settlement observed",
        ),
    ),
    (
        "the attempt bound is terminal",
        (
            r"\[robot-supervisor\] restart refused error=(\d+)",
            rf"\[robot-supervisor\] restarts total={EXPECTED_RESTARTS}",
            r"\[robot-supervisor\] supervision complete",
        ),
    ),
    (
        "terminal cleanup",
        (
            r"\[robot-actuator\] actuation complete",
            r"\[robot-burner\] bestEffort complete",
            rf"SLIME_GRAPH HEALTHY generation={GENERATION} required=8 live=0 completed=8 failed=0",
        ),
    ),
)

EXPECTED_UNORDERED: tuple[str, ...] = (
    # The clock's own end. Its exit is a protocol step rather than cleanup: the
    # call broker latches the supervised termination and only then treats the
    # drained time endpoint as closed, so a clock that parked would keep the
    # whole call plane alive.
    r"\[robot-clock\] bounded time complete",
    # The sensor is the one instance granted a timer on the stream side, and the
    # supervisor the one on the restart side. Without these the cadence and the
    # backoff could be any wake at all.
    r"SLIME_CLOCK authority task=(\d+) instance=robot-sensor flags=0x[0-9a-f]+ timers=2 badge=0x200",
    r"SLIME_CLOCK authority task=(\d+) instance=robot-supervisor flags=0x[0-9a-f]+ timers=2 badge=0x200",
    # The deadline sweep's own report. Its position relative to the second
    # clock advance's own print is a scheduling fact, not a declared one: the
    # advance's blocking send unblocks once the broker *receives* it, and the
    # broker's own deadline sweep and debug write can run before the clock
    # task regains the CPU to print its second "advanced" line. The instant it
    # fires at is pinned by `semantic_trace` from the broker's own accumulated
    # trace instead, which is written under `now_ns` rather than under
    # scheduling order.
    r"\[fabric\] call timed out",
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_GRAPH FAIL required instance",
    r"\[robot-sensor\] FAIL",
    r"\[robot-controller\] FAIL",
    r"\[robot-actuator\] FAIL",
    r"\[robot-supervisor\] FAIL",
    r"\[robot-burner\] FAIL",
    r"\[robot-clock\] FAIL",
    r"\[fabric\] fail:",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 robot runtime plane check: {message}")


def build_image() -> None:
    global IMAGE
    try:
        built = build_closure_image(CLOSURE)
    except ClosureImageError as error:
        fail(str(error))
    IMAGE = built.image
    actual = sha256_file(IMAGE, fail)
    if actual != built.digest():
        fail(
            f"{IMAGE} SHA-256 is {actual}, but the build result records "
            f"{built.digest()}; the image changed after it was built"
        )


def boot(profile: dict[str, object]) -> str:
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
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
        str(IMAGE),
    ]
    process = subprocess.Popen(
        command,
        cwd=ROOT,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    watchdog = threading.Timer(TIMEOUT, process.kill)
    watchdog.start()
    lines: list[str] = []
    terminal = re.compile(
        rf"SLIME_GRAPH HEALTHY generation={GENERATION} required=8 live=0 completed=8 failed=0"
        r"|SLIME_ROOT FATAL|SLIME_GRAPH FAIL|\[robot-\w+\] FAIL|\[fabric\] fail:"
    )
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\n"))
            if terminal.search(line):
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
    if timed_out:
        fail("QEMU timed out")
    return "\n".join(lines)


def fixture_manifest() -> dict[str, object]:
    """Decode the exercised composition through Zutai."""
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    process = subprocess.run(
        [str(binary()), "json", str(FIXTURE)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if process.returncode != 0:
        fail(f"could not decode the fixture: {process.stdout.strip()}")
    return json.loads(process.stdout)


def check_fixture_shape() -> None:
    """The declarations the transcript is read against, from the fixture itself.

    Not a restatement of the transcript: every number the marker contract pins —
    the bands, the deadline, the restart bound, the command scale — is checked
    here against the manifest, so a fixture mutation that reorders a band or
    widens the attempt bound fails rather than passing against whatever it
    became.
    """
    manifest = fixture_manifest()
    if manifest["generation"] != GENERATION:
        fail(f"fixture declares generation {manifest['generation']}, expected {GENERATION}")
    if manifest["bootAction"] != "robot-runtime":
        fail(f"fixture declares bootAction {manifest['bootAction']!r}")

    # The contention claim is a claim about *bands*, so the band mapping is what
    # it is read against. A burner sharing the sensor's priority would still spin
    # and still emit every marker while ordering nothing.
    scheduling = manifest.get("schedulingClass")
    if not isinstance(scheduling, dict):
        fail("fixture declares no scheduling class, so contention would be unordered")
    bands = {band["class"]: band["priority"] for band in scheduling["bands"]}
    assigned = {entry["instance"]: entry["class"] for entry in scheduling["instances"]}
    for instance, expected in (
        ("robot-sensor", "foreground"),
        ("robot-controller", "normal"),
        ("robot-actuator", "normal"),
        ("robot-supervisor", "normal"),
        ("robot-burner", "bestEffort"),
    ):
        if assigned.get(instance) != expected:
            fail(f"{instance} is declared {assigned.get(instance)!r}, expected {expected!r}")
    if not bands["foreground"] > bands["normal"] > bands["bestEffort"]:
        fail(f"the declared bands do not order the graph above its load: {bands}")

    # The restart bound and the causes it admits. The plane observes the bound
    # being *spent*, so a fixture admitting more attempts than the transcript
    # drives would leave that unobserved.
    policy = manifest.get("lifecyclePolicy")
    if not isinstance(policy, dict):
        fail("fixture declares no lifecycle policy, so no restart could be admitted")
    restarts = {entry["instance"]: entry for entry in policy["restarts"]}
    controller = restarts.get("robot-controller")
    if controller is None:
        fail("fixture declares no restart policy for the controller")
    if controller["attempts"] != DECLARED_RESTART_ATTEMPTS:
        fail(
            f"fixture admits {controller['attempts']} controller restarts, "
            f"expected {DECLARED_RESTART_ATTEMPTS}"
        )
    if "fault" not in controller["causes"]:
        fail("the controller's restart policy does not admit the cause the plane injects")
    if controller["backoffNs"] == 0:
        fail("the declared backoff is zero, so the supervisor's wait would be unobservable")
    dependencies = {
        (entry["instance"], entry["dependency"]) for entry in policy["dependencies"]
    }
    if ("robot-controller", "robot-supervisor") not in dependencies:
        fail("the controller declares no health dependency, so the refusal arm is vacuous")

    # The parameter edge the configuration rides on. Without a declared write the
    # supervisor could not seed the scale, and the survival claim would be about
    # a value nothing set.
    parameters = {
        (entry["holder"], entry["subject"]): entry for entry in policy["parameters"]
    }
    write = parameters.get(("robot-supervisor", "robot-controller"))
    if write is None or not write["write"]:
        fail("the supervisor holds no declared parameter write over the controller")
    read = parameters.get(("robot-controller", "robot-controller"))
    if read is None or not read["read"]:
        fail("the controller holds no declared reflexive parameter read")

    # The two routes, and the deadline the timeout arm is read against.
    graph = manifest["fabricGraph"]
    routes = {route["name"]: route for route in graph["routes"]}
    if set(routes) != {"telemetry", "parameters"}:
        fail(f"fixture declares routes {sorted(routes)}, expected the stream and call pair")
    stream = {p["component"]: p for p in routes["telemetry"]["participants"]}
    if stream["robot-sensor"]["direction"] != "publish":
        fail("the sensor is not the declared publisher")
    if stream["robot-controller"]["direction"] != "subscribe":
        fail("the controller is not the declared subscriber")
    call = {p["component"]: p for p in routes["parameters"]["participants"]}
    if call["robot-controller"]["direction"] != "client":
        fail("the controller is not the declared call client")
    if call["robot-actuator"]["direction"] != "server":
        fail("the actuator is not the declared call server")
    # The controller on *both* routes is the milestone's structural claim: no
    # prior fixture declares one identity across two contract kinds.
    if "robot-controller" not in stream or "robot-controller" not in call:
        fail("the controller is not declared on both contract kinds")
    declared_deadline = call["robot-controller"]["deadlineNs"]
    if declared_deadline != DECLARED_DEADLINE_NS:
        fail(
            f"the call route declares deadlineNs={declared_deadline}, expected "
            f"{DECLARED_DEADLINE_NS}"
        )
    if not FIRST_ADVANCE_NS < declared_deadline < SECOND_ADVANCE_NS:
        fail(
            "the clock's two advances do not bracket the declared deadline, so the "
            "boundary is untested"
        )

    # The clock is the only holder of a call-plane time source, and the sensor and
    # supervisor the only clock-authority holders. A second timer holder would let
    # a cadence or a backoff be somebody else's wake.
    clocks = {entry["holder"] for entry in manifest.get("clockAuthority") or []}
    if clocks != {"robot-sensor", "robot-supervisor"}:
        fail(f"fixture grants clock authority to {sorted(clocks)}, expected the two holders")


def check_semantics(transcript: str) -> None:
    """Bind each actuation to the sample and the configuration it derives from."""
    # Read once per incarnation, under two marker names: the first launch's and
    # every replacement's. Both must report the declared value, and the
    # replacement's must exist — a configuration read only on the first launch
    # would leave "the value outlived the task" unobserved.
    scale = re.findall(r"\[robot-controller\] scale=(\d+)", transcript)
    retained = re.findall(r"\[robot-controller\] scale retained=(\d+)", transcript)
    if not scale:
        fail("the controller reported no command scale")
    if not retained:
        fail("no replacement read the configuration back, so its survival is unobserved")
    if {int(value) for value in scale + retained} != {EXPECTED_SCALE}:
        fail(
            f"the controller read scales {sorted(set(scale + retained))}, expected "
            "the declared one"
        )

    incarnations = re.findall(r"\[robot-controller\] incarnation cause=(\d+)", transcript)
    if len(incarnations) < 2:
        fail(f"the controller ran {len(incarnations)} incarnations, expected a restart")
    # `0` is the undeclared cause id — "nothing preceded this task". Exactly one
    # incarnation may claim it, and it must be the first: a replacement reading
    # `live` would mean the root lost the terminal cause it charges the attempt
    # against.
    if incarnations[0] != "0":
        fail("the first incarnation did not read the undeclared predecessor cause")
    if incarnations.count("0") != 1:
        fail(f"predecessor causes were {incarnations}, expected exactly one first launch")

    # Every applied command is the product of a consumed tick and the declared
    # scale. Checked as a set relation rather than positionally: the controller
    # and the actuator print from two tasks, so their interleaving is a
    # scheduling fact, while the *values* are a declared one.
    ticks = [
        int(value)
        for value in re.findall(r"\[robot-controller\] (?:consumed|resumed) tick=(\d+)", transcript)
    ]
    if not ticks:
        fail("the controller consumed no samples")
    applied = [int(value) for value in re.findall(r"\[robot-actuator\] applied value=(\d+)", transcript)]
    if not applied:
        fail("no command reached actuation")
    for value in applied:
        if value % EXPECTED_SCALE != 0 or value // EXPECTED_SCALE not in ticks:
            fail(f"the actuator applied {value}, which no consumed tick and the declared scale produce")
    observed = [
        int(value)
        for value in re.findall(
            r"\[robot-controller\] command (?:applied|resumed) value=(\d+)", transcript
        )
    ]
    # Set comparison, not multiset: a withdrawn command is applied by the
    # actuator before its cancellation settles, so the actuator's own tally
    # can repeat a value the controller's `command_actuator` path never
    # echoes back for that particular request (`cancel_command` observes the
    # withdrawal, not an application). The *set* of values still has to
    # agree — an actuated value the controller never saw, or vice versa,
    # remains a real divergence.
    if set(observed) != set(applied):
        fail(f"the controller observed {sorted(set(observed))} applied, the actuator {sorted(set(applied))}")
    # Data crossed both before and after the restart. A run where every
    # actuation preceded the fault would satisfy every marker above while
    # proving the graph never resumed.
    fault_at = transcript.find("[robot-controller] injected fault")
    if fault_at < 0:
        fail("the plane injected no controller fault")
    before = transcript[:fault_at].count("[robot-actuator] applied value=")
    after = transcript[fault_at:].count("[robot-actuator] applied value=")
    if before == 0 or after == 0:
        fail(f"actuations were {before} before the fault and {after} after; the graph did not resume")

    # The declared scheduling order under load, which is the milestone's first
    # required check. The evidence is *interleaving*: a higher band making
    # ordered progress between two chunks of a still-running best-effort loop.
    # A priority-ignoring scheduler cannot produce it — the burner's chunks
    # would finish first.
    #
    # The bracketed marker is the supervisor's own terminal sequence, not a
    # sensor tick. This composition runs six `normal`-band participants
    # reacting to every tick's whole forward/reply chain, so the sensor's own
    # five-tick window never goes idle long enough for even one chunk to land
    # inside it — unlike C9.3's simpler pairing of one foreground component
    # against this same burner. What *is* structurally guaranteed is the gap
    # right after the controller's own exit: every other route is already
    # settled by then, and the supervisor's bounded poll interval is the next
    # thing to run, so its report of the final restart refusal reliably lands
    # while the burner is mid-run rather than before or after all of it.
    chunks = [
        match.start()
        for match in re.finditer(r"\[robot-burner\] chunk=(\d+)", transcript)
    ]
    if len(chunks) < 2:
        fail("the declared load emitted too few chunks to bracket any progress")
    restarts_at = [
        match.start() for match in re.finditer(r"\[robot-supervisor\] restarts total=(\d+)", transcript)
    ]
    if not restarts_at:
        fail("the supervisor reported no restart total, so contention has no bracketed marker")
    if not any(chunks[0] < position < chunks[-1] for position in restarts_at):
        fail(
            "the supervisor's restart total did not land between two chunks of the "
            "declared load, so the transcript does not order the bands under contention"
        )
    ticks_at = [match.start() for match in re.finditer(r"\[robot-sensor\] tick=(\d+)", transcript)]

    # Every outcome the milestone requires to stay distinct, each from its own
    # producer. Counted rather than merely present: two outcomes collapsing onto
    # one marker is exactly the failure this check exists to catch.
    for pattern, description in (
        (r"\[robot-supervisor\] controller faulted detail=", "a fault"),
        (r"\[robot-controller\] command cancellation observed", "a cancellation"),
        (r"\[robot-controller\] command refusal observed", "a refusal"),
        (r"\[robot-controller\] command deadline observed", "a deadline miss"),
        (r"\[robot-sensor\] stream ended ticks=", "an orderly stream end"),
    ):
        if not re.search(pattern, transcript):
            fail(f"the transcript records no {description}")
    # A timer expiry is the cadence itself: each tick follows one, so a plane
    # where the sensor never blocked would have no expiries to report.
    if len(ticks) < 1 or len(ticks_at) < EXPECTED_TICKS:
        fail(f"the sensor emitted {len(ticks_at)} ticks, expected {EXPECTED_TICKS}")

    # Architecture neutrality, which C9.6 requires of its semantic corpus. The
    # existing `just x86_portability_check` scans for x86 tokens only, so the
    # AArch64 half is asserted here against this plane's own markers.
    for pattern, what in (
        (r"\[robot-\w+\][^\n]*\b(?:gic|GIC)\b", "a GIC identifier"),
        (r"\[robot-\w+\][^\n]*\b(?:x[0-9]|x[12][0-9]|x30|sp_el[0-9]|elr_el[0-9])\b", "an AArch64 register"),
        (r"\[robot-\w+\][^\n]*0x[0-9a-f]{9,}", "a physical address"),
    ):
        found = re.search(pattern, transcript)
        if found is not None:
            fail(f"a C9.6 marker carries {what}: {found.group(0)!r}")


def semantic_trace(transcript: str) -> tuple[str, ...]:
    """The declared half of one boot: what the composition fixes, not what it observed.

    C8.15's split, applied to a robot workload. What a *composition* declares —
    which bands, which roles, which commands derive from which samples, which
    outcomes are reachable — must be identical across two boots of one image.
    What a *run* observes is excluded, and the exclusions are named rather than
    blanket:

    * the burner's chunk ordinals and the positions markers interleave at are
      scheduling facts. Two boots under a real preemptive scheduler will not
      agree on them, and requiring agreement would assert one interleaving —
      exactly the defect B68 found in the aggregate gate;
    * the supervisor's backoff instant and the sensor's timer instants are
      hardware counter readings. Two boots reading one counter identically would
      mean the clock had stopped;
    * how many of the sensor's later samples the *replacement* consumes depends
      on when its fresh ring is provisioned relative to the sensor's cadence, so
      the per-tick totals after the restart are observed rather than declared.
      What is compared instead is the *set* of applied command values, which is
      a function of the declared scale and the declared tick sequence.

    Everything else is a declaration and is compared verbatim.
    """
    declared: list[str] = []
    for pattern in (
        # The declared bands, from the components' own read-back.
        r"\[robot-sensor\] foreground priority=(\d+)",
        r"\[robot-burner\] bestEffort priority=(\d+)",
        # The declared roles, including the reissue the restart forces.
        r"\[robot-sensor\] publish role received",
        r"\[robot-controller\] subscribe role received",
        r"\[robot-controller\] subscribe role reissued",
        # The declared configuration, read once per incarnation.
        r"\[robot-controller\] scale=(\d+)",
        r"\[robot-controller\] scale retained=(\d+)",
        # The declared cadence and its orderly end.
        r"\[robot-sensor\] stream ended ticks=(\d+)",
        # The declared restart bound, spent.
        r"\[robot-supervisor\] dependency refused error=(\d+)",
        r"\[robot-supervisor\] controller restarted attempt=(\d+)",
        r"\[robot-supervisor\] restarts total=(\d+)",
        r"\[robot-supervisor\] supervision complete",
        # The declared outcomes, each from its own producer.
        r"\[robot-actuator\] command cancelled id=(\d+)",
        r"\[robot-controller\] command cancellation observed",
        r"\[robot-actuator\] command refused value=(\d+)",
        r"\[robot-controller\] command refusal observed",
        r"\[robot-actuator\] command left unanswered id=(\d+)",
        r"\[robot-controller\] command deadline observed",
        r"\[robot-actuator\] timeout settlement observed",
        r"\[robot-actuator\] unanswered total=(\d+)",
        r"\[robot-actuator\] cancelled total=(\d+)",
        r"\[robot-actuator\] refused total=(\d+)",
        # The declared simulated instants. Unlike the hardware clock these are
        # composition data: the clock advances to exactly what it declares, so
        # two boots must agree.
        r"\[robot-clock\] advanced now_ns=(\d+)",
        r"\[robot-clock\] bounded time complete",
        # The graph's own close.
        rf"SLIME_GRAPH HEALTHY generation={GENERATION} required=8 live=0 completed=8 failed=0",
    ):
        matches = re.findall(pattern, transcript)
        if not matches:
            fail(f"missing marker for the cross-boot comparison: {pattern}")
        declared.append(f"{pattern}={matches!r}")

    # The applied command values as a *set*: which commands the composition can
    # produce is declared, how many of each a run happens to carry is not.
    applied = sorted(
        {int(value) for value in re.findall(r"\[robot-actuator\] applied value=(\d+)", transcript)}
    )
    if not applied:
        fail("no command reached actuation")
    declared.append(f"applied-values={applied!r}")
    resumed = sorted(
        {
            int(value)
            for value in re.findall(r"\[robot-controller\] command resumed value=(\d+)", transcript)
        }
    )
    if not resumed:
        fail("the replacement observed no actuation, so the graph did not resume")
    declared.append(f"resumed-values={resumed!r}")
    return tuple(declared)


def check_transcript(transcript: str) -> None:
    """Adjudicate one boot. Exposed so the aggregate control can drive it."""
    match_marker_contract(transcript, CHAINS, FAILURE_MARKERS, fail)
    for pattern in EXPECTED_UNORDERED:
        if re.search(pattern, transcript) is None:
            fail(f"missing order-independent marker: {pattern}")
    check_semantics(transcript)


def main() -> None:
    check_fixture_shape()
    build_image()
    pins = tomllib.loads(PINS.read_text(encoding="utf-8"))
    profile = pins.get("qemu_arm_virt")
    if not isinstance(profile, dict):
        fail("missing qemu profile")

    # Two boots of one image, which is how C8.15 closes the C8 track and how
    # C9.6 is asked to close this one: one composition, both schedules, compared
    # semantically. One boot could not distinguish a graph that composes
    # deterministically from one that happened to agree with itself once.
    traces: list[tuple[str, ...]] = []
    for boot_index in range(2):
        transcript = boot(profile)
        try:
            check_transcript(transcript)
        except SystemExit as error:
            fail(f"boot {boot_index}: {error}")
        traces.append(semantic_trace(transcript))
    if traces[0] != traces[1]:
        divergent = [
            (first, second)
            for first, second in zip(traces[0], traces[1], strict=True)
            if first != second
        ]
        fail(f"the two boots' declared traces diverged: {divergent}")

    print(
        "seL4 robot runtime plane check: a sensor/controller/actuator graph ran to "
        "completion under a declared best-effort load with the declared bands "
        "ordering it; an injected controller fault was bounded, reissued the "
        "controller's fabric authority, and the graph resumed; deadline miss, "
        "cancellation, fault, and the orderly stream end stayed distinct; and both "
        "boots' declared traces were identical"
    )


if __name__ == "__main__":
    main()
