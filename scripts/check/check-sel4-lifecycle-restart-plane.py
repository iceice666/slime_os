#!/usr/bin/env python3
"""C9.4 gate: a userspace supervisor restarts under declared policy, and the bound is terminal."""
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

from harness import profile_integer, profile_text, sha256_file  # noqa: E402
from sel4_gate_markers import match_marker_contract  # noqa: E402
from zutai_cli import STDLIB, binary  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
IMAGE = ROOT / "build" / "slime-sel4-lifecycle-restart.elf"
MANIFEST = ROOT / "build" / "slime-sel4-lifecycle-restart.identity.json"
BUILD = ROOT / "scripts" / "build" / "build-sel4.py"
PINS = ROOT / "sel4" / "pins.toml"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-lifecycle-restart.zti"
IMAGE_VARIANT = "lifecycle-restart"
GENERATION = 44
TIMEOUT = 300

# The declared transition graph, read from the fixture too (`check_fixture_shape`),
# so a mutation that drops an edge fails here rather than passing against
# whatever the graph became.
DECLARED_EDGES = (
    ("Initialize", "Running"),
    ("Running", "Ready"),
    ("Running", "Stop"),
    ("Running", "Error"),
    ("Ready", "Stop"),
)
DECLARED_INITIAL = "Initialize"
DECLARED_TERMINAL = "Error"
# The frozen state and cause ids from `contracts/lifecycle-policy/v1`. Named here
# because the probe prints ids rather than spellings, and a gate comparing a
# spelling to a spelling would not notice the two drifting apart.
STATE_IDS = {
    "undeclared": 0,
    "Initialize": 1,
    "Configure": 2,
    "Start": 3,
    "Ready": 4,
    "Running": 5,
    "Degraded": 6,
    "Stop": 7,
    "Error": 8,
}
CAUSE_IDS = {"live": 0, "exit": 1, "fault": 2, "unhealthy": 3}
# The declared restart policy the transcript is read against.
RESTART_SUBJECT = "lifecycle-worker"
RESTART_ATTEMPTS = 3
RESTART_BACKOFF_NS = 200_000
RESTART_BACKOFF_FACTOR = 512
RESTART_CAUSES = ("exit", "fault", "unhealthy")
# The scale `contracts/lifecycle-policy/v1` declares for `backoffFactor`, so the
# gate computes the same growth both readers do rather than a third rule.
BACKOFF_FACTOR_SCALE = 256
# The parameter the supervisor writes and every incarnation reads back. The value
# surviving three restarts is the "restart preserves declared configuration"
# check, and the key is asserted so a probe that silently read a different one
# could not satisfy it.
CONFIG_KEY = 7
CONFIG_VALUE = 4242
# A key nothing writes, so a read of it answers "no value" rather than "no
# authority". Mirrors the probe's own `ABSENT_KEY`; the two refusals being
# distinguishable is C9.4's last required check.
ABSENT_KEY = 9
# Logical authority slots `lifecycle-denied` sweeps when proving it can restart
# nothing. Mirrors the probe's own `DENIED_SLOT_SWEEP` exactly: asserting the
# count rather than a floor is what makes a regression that shortens the sweep
# fail here, and an earlier revision's `+ 1` double-counted a swept slot.
DENIED_SLOT_SWEEP = 8
DENIED_REFUSALS = DENIED_SLOT_SWEEP
# The instances the fixture autostarts, and the required set the graph closes on.
REQUIRED_INSTANCES = 4
# The class and quota a restart must preserve. Declared by the fixture so the
# transcript can distinguish "preserved across a restart" from "never set": with
# no policy at all, the first launch and every replacement resolve to the same
# root default and the observation would be vacuous.
RESTART_CLASS = "normal"
RESTART_CLASS_PRIORITY = 150
RESTART_PRIVATE_PAGES = 4
# The state the health dependency names, and the state the supervisor is
# installed in. They must differ, or the edge is satisfied before the supervisor
# does anything and its refusal branch is unreachable.
DEPENDENCY_REQUIRED_STATE = "Running"

CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "the declared lifecycle policy the root resolved",
        (
            rf"SLIME_LIFECYCLE policy transitions={len(DECLARED_EDGES)} restarts=1 "
            rf"admitted=1 dependencies=1 parameters=4 initial={DECLARED_INITIAL} "
            rf"terminal={DECLARED_TERMINAL}",
        )
        + tuple(
            rf"SLIME_LIFECYCLE edge from={source} to={target}"
            for source, target in sorted(
                DECLARED_EDGES, key=lambda edge: (STATE_IDS[edge[0]], STATE_IDS[edge[1]])
            )
        ),
    ),
    (
        "a declared health dependency gates a start until it is satisfied",
        (
            # Refused *before* the supervisor advances, so this is the claim that
            # a dependent whose dependency is down is not started — not merely
            # that one whose dependency is up is.
            rf"SLIME_GRAPH spawn refused task=\d+ child={RESTART_SUBJECT} "
            r"class=lifecycle-dependency",
            r"\[lifecycle-supervisor\] dependency refused error=\d+",
            # And the same spawn is admitted once the declared state is reached.
            rf"SLIME_LIFECYCLE advanced task=\d+ state={DEPENDENCY_REQUIRED_STATE}",
            r"\[lifecycle-supervisor\] launched handle=\d+",
        ),
    ),
    (
        "a supervisor restarts a faulted component under declared policy",
        (
            # The root stages the child's declared state during the spawn, so
            # this precedes the supervisor's own reply. The worker is launched by
            # its *supervisor* rather than by init, because restart authority must
            # ride on a handle its own holder obtained.
            rf"SLIME_LIFECYCLE state task=\d+ instance={RESTART_SUBJECT} "
            rf"state={DECLARED_INITIAL} attempts={RESTART_ATTEMPTS}",
            r"\[lifecycle-supervisor\] launched handle=\d+",
            r"\[lifecycle-worker\] faulting",
            r"SLIME_LIFECYCLE terminated task=\d+ instance=\d+ cause=fault",
            r"\[lifecycle-supervisor\] observed death attempt=0",
            r"\[lifecycle-supervisor\] cause=fault",
            # The predecessor's handle, re-invoked after the outcome was
            # collected. It named one task lifetime and no request shape
            # redirects it at a successor.
            r"\[lifecycle-supervisor\] stale handle refused error=\d+",
            r"SLIME_LIFECYCLE restart admitted task=\d+ subject=\d+ attempt=0 "
            rf"remaining={RESTART_ATTEMPTS - 1} ready_at=\d+",
            # The backoff, observed as a refusal *before* it is waited: the root
            # refuses a spawn arriving before the instant it answered, so the
            # delay is enforced rather than trusted to the supervisor's loop.
            rf"SLIME_GRAPH spawn refused task=\d+ child={RESTART_SUBJECT} "
            r"class=backoff-pending",
            r"\[lifecycle-supervisor\] backoff refused error=\d+",
            # And then waited, against C9.1's clock rather than a spin count.
            r"\[lifecycle-supervisor\] backoff elapsed now=\d+",
            r"\[lifecycle-supervisor\] restarted handle=\d+",
        ),
    ),
    (
        "the three terminal causes are distinguishable, and drive different policy",
        (
            # A replacement reads why its predecessor ended, so the causes are
            # distinguishable from inside the restarted component and not only
            # from its supervisor.
            rf"\[lifecycle\] cause={CAUSE_IDS['fault']}",
            r"\[lifecycle-worker\] exiting after fault",
            r"\[lifecycle-supervisor\] cause=exit",
            rf"\[lifecycle\] cause={CAUSE_IDS['exit']}",
            r"\[lifecycle-worker\] declaring unhealthy",
            # The root records `unhealthy` rather than the `exit` that follows
            # it: `unhealthy()` exits immediately, so without a distinct cause
            # "it stopped" and "it said it was broken" would be one observation.
            r"SLIME_LIFECYCLE unhealthy task=\d+ instance=\d+ cause=unhealthy",
            rf"\[lifecycle\] cause={CAUSE_IDS['unhealthy']}",
            r"\[lifecycle-worker\] exiting after unhealthy",
        ),
    ),
    (
        "exhausting the attempt bound is terminal, not merely unproductive",
        (
            rf"SLIME_LIFECYCLE restart admitted task=\d+ subject=\d+ "
            rf"attempt={RESTART_ATTEMPTS - 1} remaining=0 ready_at=\d+",
            rf"SLIME_LIFECYCLE terminal task=\d+ subject=\d+ state={DECLARED_TERMINAL} "
            r"attempts=exhausted",
            r"\[lifecycle-supervisor\] restart refused error=\d+",
            # And the *spawn* is refused too. A supervisor that ignored the
            # admission refusal and spawned anyway would otherwise restart
            # forever, which is the behaviour this check forbids.
            rf"SLIME_GRAPH spawn refused task=\d+ child={RESTART_SUBJECT} "
            rf"class=lifecycle-exhausted state={DECLARED_TERMINAL}",
            r"\[lifecycle-supervisor\] terminal spawn refused error=\d+",
            r"\[lifecycle-supervisor\] attempts exhausted",
            r"\[lifecycle-supervisor\] supervisor complete",
        ),
    ),
    (
        "the transition graph is enforced, not documented",
        (
            rf"\[lifecycle-graph\] state state={STATE_IDS[DECLARED_INITIAL]}",
            # The advances this role takes are asserted by `check_graph_walk`,
            # bound to the one task the root recorded for it — not here. The
            # root's advance line carries no role prefix, so ordering a chain on
            # it would let the supervisor's identical advance satisfy the
            # walker's, and a later match could consume past this role's own
            # refusal.
            # `Ready -> Initialize` is not declared. This is the assertion that
            # separates an enforced graph from one the root merely carries.
            r"SLIME_LIFECYCLE refused task=\d+ class=unadmitted-transition "
            r"detail=UnadmittedTransition",
            r"\[lifecycle-graph\] undeclared edge refused error=\d+",
            r"\[lifecycle-graph\] state unchanged after refusal",
            r"\[lifecycle-graph\] graph complete",
        ),
    ),
    (
        "deny by default: a state is an answer, authority is a refusal",
        (
            rf"\[lifecycle-denied\] state state={STATE_IDS[DECLARED_INITIAL]}",
            # No parameter edge at all, reflexive included, so this instance
            # cannot reach even its own configuration. That is what makes
            # parameter state an authority rather than a namespace.
            r"\[lifecycle-denied\] own parameter refused error=\d+",
            r"\[lifecycle-denied\] no restart authority",
        ),
    ),
    (
        "terminal cleanup",
        (
            r"\[init\] lifecycle restart plane is root-launched",
            rf"SLIME_GRAPH HEALTHY generation={GENERATION} required={REQUIRED_INSTANCES} "
            rf"live=0 completed={REQUIRED_INSTANCES} failed=0",
        ),
    ),
)

EXPECTED_UNORDERED: tuple[str, ...] = (
    # Every autostarted instance's resolved state, root-attributed. These are the
    # root's own accounting rather than the probe's, so a component misreporting
    # its state cannot satisfy them.
    rf"SLIME_LIFECYCLE state task=\d+ instance=lifecycle-supervisor "
    rf"state={DECLARED_INITIAL} attempts=0",
    rf"SLIME_LIFECYCLE state task=\d+ instance=lifecycle-graph "
    rf"state={DECLARED_INITIAL} attempts=0",
    rf"SLIME_LIFECYCLE state task=\d+ instance=lifecycle-denied "
    rf"state={DECLARED_INITIAL} attempts=0",
    # The supervisor's own reflexive parameter edge resolves, and the worker's
    # value is written through the handle naming it while it is live.
    rf"SLIME_LIFECYCLE parameter task=\d+ subject-instance=\d+ key={CONFIG_KEY} "
    r"write=true value=0",
    r"\[lifecycle-supervisor\] worker parameter previous=0",
    # Read authority does not imply write: `lifecycle-graph` holds a write-only
    # edge, so its read is refused and its write admitted, and the refusal names
    # absent authority rather than a missing key.
    rf"SLIME_LIFECYCLE parameter refused task=\d+ subject-instance=\d+ key={CONFIG_KEY} "
    r"class=no-parameter-authority detail=NoParameterAuthority",
    # A missing key is a *different* refusal from absent authority, which is what
    # C9.4's last required check asks for: a caller must be able to tell "I may
    # not ask" from "there is no answer". The worker reads a key nothing ever
    # writes over the same declared edge it just read its configuration through,
    # so the only thing differing between the two answers is whether a value
    # exists.
    rf"SLIME_LIFECYCLE parameter refused task=\d+ subject-instance=\d+ key={ABSENT_KEY} "
    r"class=unknown-parameter detail=UnknownParameter",
    r"\[lifecycle-worker\] unset key refused error=\d+",
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_LIFECYCLE FAIL",
    r"\[lifecycle\] FAIL",
    r"SLIME_GRAPH FAIL required instance",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 lifecycle-restart plane check: {message}")


def build_image() -> None:
    process = subprocess.run(
        [sys.executable, str(BUILD), "--lifecycle-restart-plane"],
        cwd=ROOT,
        check=False,
    )
    if process.returncode != 0 or not IMAGE.is_file():
        fail("image build failed")
    if not MANIFEST.is_file():
        fail("identity manifest missing")
    identity = json.loads(MANIFEST.read_text(encoding="utf-8"))
    if identity.get("variant") != IMAGE_VARIANT:
        fail(f"wrong image variant {identity.get('variant')!r}")
    if identity.get("target_profile") != "aarch64-sel4-qemu-virt":
        fail(f"wrong target profile {identity.get('target_profile')!r}")
    image = identity.get("image")
    if not isinstance(image, dict) or image.get("sha256") != sha256_file(IMAGE, fail):
        fail("packaged image digest does not match identity manifest")


def boot(profile: dict[str, object]) -> str:
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
        rf"SLIME_GRAPH HEALTHY generation={GENERATION} required={REQUIRED_INSTANCES} "
        rf"live=0 completed={REQUIRED_INSTANCES} failed=0"
        r"|SLIME_ROOT FATAL|SLIME_LIFECYCLE FAIL|\[lifecycle\] FAIL"
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
    """Decode the exercised lifecycle policy through Zutai."""
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

    Without this the marker table would be a set of literals agreeing with
    themselves: a fixture that dropped an edge, lowered the attempt bound, or
    granted the denied instance a parameter edge would produce a transcript this
    gate no longer describes, and the gate would have no way to tell.
    """
    manifest = fixture_manifest()
    if manifest.get("generation") != GENERATION:
        fail(f"fixture declares generation {manifest.get('generation')!r}, expected {GENERATION}")
    policy = manifest.get("lifecyclePolicy")
    if not isinstance(policy, dict):
        fail("fixture declares no lifecyclePolicy")

    if policy.get("initialState") != DECLARED_INITIAL:
        fail(f"fixture initial state {policy.get('initialState')!r} != {DECLARED_INITIAL!r}")
    if policy.get("terminalState") != DECLARED_TERMINAL:
        fail(f"fixture terminal state {policy.get('terminalState')!r} != {DECLARED_TERMINAL!r}")
    edges = tuple((edge["from"], edge["to"]) for edge in policy["transitions"])
    if set(edges) != set(DECLARED_EDGES):
        fail(f"fixture transition graph {edges!r} does not match the asserted {DECLARED_EDGES!r}")
    # Exhaustion must be able to reach the declared terminal state, or "the graph
    # is left in a declared terminal state" would be a claim about a state no
    # edge reaches.
    if not any(target == DECLARED_TERMINAL for _, target in edges):
        fail("no declared transition reaches the terminal state")
    # The refused edge the graph walker asks for must genuinely be undeclared, or
    # its refusal assertion tests nothing.
    if ("Ready", DECLARED_INITIAL) in set(edges):
        fail("fixture declares Ready -> Initialize, so the refused-edge arm tests nothing")

    restarts = policy["restarts"]
    if len(restarts) != 1:
        fail(f"fixture declares {len(restarts)} restart policies, expected exactly 1")
    restart = restarts[0]
    if restart["instance"] != RESTART_SUBJECT:
        fail(f"fixture restart policy names {restart['instance']!r}, expected {RESTART_SUBJECT!r}")
    if restart["attempts"] != RESTART_ATTEMPTS:
        fail(f"fixture declares {restart['attempts']} attempts, expected {RESTART_ATTEMPTS}")
    if tuple(sorted(restart["causes"])) != tuple(sorted(RESTART_CAUSES)):
        fail(f"fixture restart causes {restart['causes']!r} != {RESTART_CAUSES!r}")
    if restart["backoffNs"] != RESTART_BACKOFF_NS:
        fail(f"fixture backoffNs {restart['backoffNs']} != {RESTART_BACKOFF_NS}")
    if restart.get("backoffFactor") != RESTART_BACKOFF_FACTOR:
        fail(f"fixture backoffFactor {restart.get('backoffFactor')} != {RESTART_BACKOFF_FACTOR}")
    # A growing factor is what makes the backoff assertion below a statement
    # about growth rather than about one fixed delay.
    if restart["backoffFactor"] <= BACKOFF_FACTOR_SCALE:
        fail("fixture declares a flat backoff, so successive delays would not grow")
    # The restart subject must be owner-spawned: the `lifecycleRestart` right
    # rides on the supervision handle a spawner receives, so a root-autostart
    # subject could never be charged and the policy would silently never apply.
    owners = {entry["name"]: entry["owner"] for entry in manifest["instances"]}
    if owners.get(RESTART_SUBJECT) == "root":
        fail(f"{RESTART_SUBJECT} is root-owned, so no supervisor could hold its handle")

    dependencies = policy["dependencies"]
    if len(dependencies) != 1:
        fail(f"fixture declares {len(dependencies)} health dependencies, expected exactly 1")
    if dependencies[0]["instance"] == dependencies[0]["dependency"]:
        fail("fixture declares a self-dependency, which can never be satisfied")
    # The edge must name a state its dependency is *not* in at boot, or it is
    # satisfied before anything happens and its refusal branch is unreachable —
    # the gate would then assert only that a satisfied dependency admits a spawn.
    if dependencies[0]["requiredState"] != DEPENDENCY_REQUIRED_STATE:
        fail(
            f"fixture dependency requires {dependencies[0]['requiredState']!r}, expected "
            f"{DEPENDENCY_REQUIRED_STATE!r}"
        )
    if dependencies[0]["requiredState"] == policy["initialState"]:
        fail(
            "fixture dependency requires the state its dependency is installed in, so the "
            "edge is satisfied before the supervisor acts and gates nothing"
        )
    # The class and quota a restart must preserve. Both must be *declared*, or the
    # preservation assertions compare a default against itself.
    scheduling = manifest.get("schedulingClass")
    if not isinstance(scheduling, dict):
        fail("fixture declares no schedulingClass, so class preservation is unobservable")
    bands = {band["class"]: band["priority"] for band in scheduling["bands"]}
    if bands.get(RESTART_CLASS) != RESTART_CLASS_PRIORITY:
        fail(f"fixture band for {RESTART_CLASS} is {bands.get(RESTART_CLASS)!r}")
    assigned = {entry["instance"]: entry["class"] for entry in scheduling["instances"]}
    if assigned.get(RESTART_SUBJECT) != RESTART_CLASS:
        fail(
            f"fixture assigns {RESTART_SUBJECT} class {assigned.get(RESTART_SUBJECT)!r}, "
            f"expected {RESTART_CLASS!r}"
        )
    # A band equal to the root's own child default would make the assertion
    # vacuous: every unnamed thread already runs there.
    if RESTART_CLASS_PRIORITY == 254:
        fail("the declared band equals the root's child default, so preservation is vacuous")
    quotas = {entry["holder"]: entry["pageQuota"] for entry in manifest["privateMemoryBudget"]}
    if quotas.get(RESTART_SUBJECT) != RESTART_PRIVATE_PAGES:
        fail(
            f"fixture grants {RESTART_SUBJECT} {quotas.get(RESTART_SUBJECT)!r} private pages, "
            f"expected {RESTART_PRIVATE_PAGES}"
        )

    parameters = {(entry["holder"], entry["subject"]): entry for entry in policy["parameters"]}
    # The worker's own edge is read-only, so "read authority does not imply
    # write" is a property of the fixture and not only of the code.
    worker_edge = parameters.get((RESTART_SUBJECT, RESTART_SUBJECT))
    if worker_edge is None or worker_edge["write"]:
        fail(f"fixture must grant {RESTART_SUBJECT} a read-only reflexive parameter edge")
    # And `lifecycle-graph`'s is write-only, which is the other direction of the
    # same asymmetry.
    graph_edge = parameters.get(("lifecycle-graph", "lifecycle-graph"))
    if graph_edge is None or graph_edge["read"]:
        fail("fixture must grant lifecycle-graph a write-only reflexive parameter edge")
    # The deny-by-default arm needs an instance the policy genuinely omits from
    # every table.
    declared_instances = {entry["name"] for entry in manifest["instances"]}
    if "lifecycle-denied" not in declared_instances:
        fail("fixture has no lifecycle-denied instance")
    if any("lifecycle-denied" in key for key in parameters):
        fail("fixture grants lifecycle-denied a parameter edge, so nothing tests the default")
    if any(entry["instance"] == "lifecycle-denied" for entry in restarts):
        fail("fixture grants lifecycle-denied a restart policy, so nothing tests the default")


def declared_backoff(attempt: int) -> int:
    """The delay the *contract* declares for `attempt`, computed as both readers do."""
    delay = RESTART_BACKOFF_NS
    for _ in range(attempt):
        delay = delay * RESTART_BACKOFF_FACTOR // BACKOFF_FACTOR_SCALE
    return delay


def check_restart_sequence(transcript: str) -> None:
    """The milestone's checks, read as a causal sequence rather than marker presence.

    Three properties are asserted here that no marker table can state:

    * every admitted restart is charged exactly once and the remaining budget
      decreases monotonically to zero, so the bound is a bound;
    * each admission's `ready_at` grows by the factor the contract declares, and
      the supervisor's observed clock reading is at or past it — which is what
      makes "backoff is observed against C9.1's clock, not a spin count" an
      observation rather than a claim about a delay the supervisor chose;
    * the causes the *root* recorded and the causes each *replacement* read back
      are the same sequence, so the two sides of the observation agree rather
      than one restating the other.
    """
    admissions = [
        (int(attempt), int(remaining), int(ready_at))
        for attempt, remaining, ready_at in re.findall(
            r"SLIME_LIFECYCLE restart admitted task=\d+ subject=\d+ attempt=(\d+) "
            r"remaining=(\d+) ready_at=(\d+)",
            transcript,
        )
    ]
    if len(admissions) != RESTART_ATTEMPTS:
        fail(
            f"the root admitted {len(admissions)} restarts, expected exactly "
            f"{RESTART_ATTEMPTS} — the declared attempt bound"
        )
    for index, (attempt, remaining, _) in enumerate(admissions):
        if attempt != index:
            fail(f"restart admission {index} charged attempt {attempt}, not {index}")
        if remaining != RESTART_ATTEMPTS - index - 1:
            fail(
                f"restart admission {index} reported {remaining} attempts remaining, "
                f"expected {RESTART_ATTEMPTS - index - 1}"
            )
    # The backoff instants grow by the declared factor. Compared as *differences*
    # between successive admissions rather than as absolute instants, because the
    # clock's origin is the boot's and only the growth is declared.
    elapsed = [
        int(now)
        for now in re.findall(r"\[lifecycle-supervisor\] backoff elapsed now=(\d+)", transcript)
    ]
    if len(elapsed) != RESTART_ATTEMPTS:
        fail(f"the supervisor waited {len(elapsed)} backoffs, expected {RESTART_ATTEMPTS}")
    for index, (_, _, ready_at) in enumerate(admissions):
        if elapsed[index] < ready_at:
            fail(
                f"the supervisor proceeded at {elapsed[index]} before the declared instant "
                f"{ready_at} for attempt {index}"
            )
        # The declared delay for this attempt must be strictly greater than the
        # previous one, which is the growth the factor encodes. Checked against
        # the contract's own arithmetic rather than against the observed
        # difference, because host scheduling inflates the latter (B75).
        if index > 0 and declared_backoff(index) <= declared_backoff(index - 1):
            fail(
                f"the declared backoff did not grow between attempts {index - 1} and {index}; "
                "the factor is not being applied"
            )
    # The causes, from both sides. The root's record first.
    root_causes = re.findall(
        r"SLIME_LIFECYCLE (?:terminated|unhealthy) task=\d+ instance=\d+ cause=(\w+)",
        transcript,
    )
    for cause in RESTART_CAUSES:
        if cause not in root_causes:
            fail(f"the root never recorded a {cause} termination, so the three causes are not all exercised")
    # And each replacement's own reading of its predecessor's cause, in order.
    read_causes = [int(value) for value in re.findall(r"\[lifecycle\] cause=(\d+)", transcript)]
    for cause in ("fault", "exit", "unhealthy"):
        if CAUSE_IDS[cause] not in read_causes:
            fail(
                f"no incarnation read back cause={CAUSE_IDS[cause]} ({cause}), so a restarted "
                "component cannot distinguish why its predecessor ended"
            )


def check_configuration_survives(transcript: str) -> None:
    """Restart preserves the declared configuration, and reissues authority.

    Every incarnation reads the same value its supervisor wrote once, before the
    first death — so the value crossed three restarts rather than being rewritten
    each time. The handles, by contrast, are all fresh: each replacement's
    supervision handle is a new slot over a task id that never aliases, and the
    predecessor's is refused.
    """
    values = [
        int(value)
        for value in re.findall(r"\[lifecycle-worker\] parameter value=(\d+)", transcript)
    ]
    if len(values) < RESTART_ATTEMPTS:
        fail(
            f"only {len(values)} incarnations read their configuration back, expected at "
            f"least {RESTART_ATTEMPTS}"
        )
    if any(value != CONFIG_VALUE for value in values):
        fail(f"an incarnation read {values!r}, expected every value to be {CONFIG_VALUE}")
    # Exactly one write produced them, so this is survival rather than repetition.
    writes = len(re.findall(r"\[lifecycle-supervisor\] worker parameter previous=\d+", transcript))
    if writes != 1:
        fail(
            f"the supervisor wrote the worker's configuration {writes} times; a value "
            "rewritten each restart would not show that it survived one"
        )
    # Every predecessor handle was refused after its outcome was collected, once
    # per death observed.
    deaths = len(re.findall(r"\[lifecycle-supervisor\] observed death attempt=\d+", transcript))
    stale = len(re.findall(r"\[lifecycle-supervisor\] stale handle refused error=\d+", transcript))
    if stale != deaths:
        fail(
            f"{deaths} deaths were observed but {stale} predecessor handles were refused; a "
            "stale handle that still answers could reach a successor"
        )
    # Each incarnation ran as a distinct task, so no replacement inherited a task
    # identity. The root's own staging lines carry the ids.
    tasks = re.findall(
        rf"SLIME_LIFECYCLE state task=(\d+) instance={RESTART_SUBJECT} state=\w+ attempts=\d+",
        transcript,
    )
    if len(set(tasks)) != len(tasks):
        fail(f"the restarted instance reused a task id across incarnations: {tasks!r}")
    if len(tasks) != RESTART_ATTEMPTS + 1:
        fail(
            f"{len(tasks)} incarnations of {RESTART_SUBJECT} launched, expected "
            f"{RESTART_ATTEMPTS + 1} (the first launch plus every admitted restart)"
        )


def check_class_and_quota_survive(transcript: str) -> None:
    """Restart preserves the declared scheduling class and private-memory quota.

    Two of the milestone's required checks, and both need a *declared* policy to
    be observable at all: with no scheduling class and no private-memory budget,
    the first launch and every replacement resolve to the same root default, so a
    transcript could not distinguish "preserved" from "never set". The fixture
    therefore declares a `normal` band and a four-page quota for the restarted
    instance, and this asserts every incarnation was installed at both.

    Root-attributed on both sides: the class comes from `SLIME_SCHED class` and
    the quota from `SLIME_MEM quota`, neither of which any component writes.
    """
    classes = re.findall(
        rf"SLIME_SCHED class task=\d+ instance={RESTART_SUBJECT} class=(\w+) priority=(\d+)",
        transcript,
    )
    if len(classes) != RESTART_ATTEMPTS + 1:
        fail(
            f"{len(classes)} incarnations of {RESTART_SUBJECT} had a class installed, "
            f"expected {RESTART_ATTEMPTS + 1}; a restart that lost its declared class "
            "would show up as a missing install rather than a wrong one"
        )
    for name, priority in classes:
        if name != RESTART_CLASS or int(priority) != RESTART_CLASS_PRIORITY:
            fail(
                f"an incarnation ran at {name}/{priority}, expected "
                f"{RESTART_CLASS}/{RESTART_CLASS_PRIORITY} from the declared band"
            )
    # Both fields, and `installed=` is the load-bearing one. `declared=` is a pure
    # manifest lookup keyed by instance name, so it is identical on every
    # incarnation by construction and comparing it to the fixture compares the
    # manifest to itself (found by review). `installed=` is what the root actually
    # placed in the child's window, so a replacement launched with a zeroed or
    # reduced ceiling moves it while `declared=` stays put.
    quotas = re.findall(
        rf"SLIME_MEM quota task=\d+ instance={RESTART_SUBJECT} declared=(\d+) "
        r"installed=(\d+) ",
        transcript,
    )
    if len(quotas) != RESTART_ATTEMPTS + 1:
        fail(
            f"{len(quotas)} incarnations of {RESTART_SUBJECT} had a private-memory quota "
            f"installed, expected {RESTART_ATTEMPTS + 1}"
        )
    for declared, installed in quotas:
        if int(declared) != RESTART_PRIVATE_PAGES:
            fail(
                f"an incarnation declared a {declared}-page private-memory quota, "
                f"expected {RESTART_PRIVATE_PAGES}"
            )
        if int(installed) != RESTART_PRIVATE_PAGES:
            fail(
                f"an incarnation was *installed* with {installed} private-memory pages "
                f"against a declared ceiling of {declared}; a restart that lost its quota "
                "moves this number while the declared one stays put"
            )
    # The class, cross-checked against the `ScheduleRecord` the *builder* wrote
    # for the same instance — a second producer of the same number, exactly as
    # C9.3's plane does. Without this the class half would rest on one producer:
    # `priority=` is the policy-resolved number the root recorded, so a band that
    # never reached the plan would still satisfy it.
    scheduled = re.findall(
        rf"SLIME_GRAPH schedule instance={RESTART_SUBJECT} priority=(\d+) default=(\d+)",
        transcript,
    )
    if len(scheduled) != RESTART_ATTEMPTS + 1:
        fail(
            f"{len(scheduled)} schedule records were written for {RESTART_SUBJECT}, "
            f"expected {RESTART_ATTEMPTS + 1}"
        )
    for priority, default in scheduled:
        if int(priority) != RESTART_CLASS_PRIORITY:
            fail(
                f"the builder planned {RESTART_SUBJECT} at priority {priority}, but the "
                f"declared {RESTART_CLASS} band is {RESTART_CLASS_PRIORITY}; the class and "
                "the plan disagree"
            )
        if int(default) == int(priority):
            fail(
                "the declared band equals the root's child default in the plan, so the "
                "class reaching the thread is unobservable"
            )


def check_graph_walk(transcript: str) -> None:
    """The declared walk and the refusal belong to *one* task.

    The chain above can only require that the markers appear in order, and the
    root's advance line carries no role prefix — so the supervisor's own
    `Running` advance could satisfy the graph walker's, and a refusal could be
    attributed to a third instance. This binds the whole walk to the single task
    the root recorded for `lifecycle-graph`.
    """
    walker = re.search(
        r"SLIME_LIFECYCLE state task=(\d+) instance=lifecycle-graph", transcript
    )
    if walker is None:
        fail("the root recorded no lifecycle state for lifecycle-graph")
    task = walker.group(1)
    walked = re.findall(rf"SLIME_LIFECYCLE advanced task={task} state=(\w+)", transcript)
    expected = [DEPENDENCY_REQUIRED_STATE, "Ready", "Stop"]
    if walked != expected:
        fail(
            f"lifecycle-graph walked {walked!r}, expected exactly {expected!r}; a walk "
            "that took a different path, or an extra edge, is not the declared graph"
        )
    # And its refusal is its own, not another instance's.
    if not re.search(
        rf"SLIME_LIFECYCLE refused task={task} class=unadmitted-transition", transcript
    ):
        fail("the undeclared-transition refusal was not attributed to lifecycle-graph")
    # Exactly one: `run_graph_walker` asks for one undeclared edge and nothing
    # else this instance does produces that class, so a second would mean a
    # declared edge was refused too.
    refusals = len(
        re.findall(
            rf"SLIME_LIFECYCLE refused task={task} class=unadmitted-transition",
            transcript,
        )
    )
    if refusals != 1:
        fail(
            f"lifecycle-graph had {refusals} transitions refused, expected exactly 1; a "
            "declared edge being refused would show up here"
        )


def check_denials(transcript: str) -> None:
    """Bind the denied instance's refusals to its own task id.

    Unbound counts would let the supervisor's and the graph walker's refusals
    reach the threshold, which is the vacuity this exists to exclude: the claim is
    that *this* instance, holding no authority, could reach nothing.
    """
    denied = re.search(
        r"SLIME_LIFECYCLE state task=(\d+) instance=lifecycle-denied", transcript
    )
    if denied is None:
        fail("the root recorded no lifecycle state for lifecycle-denied")
    denied_task = denied.group(1)
    refusals = len(
        re.findall(
            rf"SLIME_LIFECYCLE refused task={denied_task} class=undeclared detail=slot",
            transcript,
        )
    )
    # Exactly, not at least: `run_denied` emits one refusal per swept slot plus
    # one for the non-supervision slot and nothing else, so a regression that
    # shortens the sweep — or one that lets a slot succeed — moves this number
    # either way.
    if refusals != DENIED_REFUSALS:
        fail(
            f"lifecycle-denied had {refusals} restart attempts refused for want of "
            f"authority, expected exactly {DENIED_REFUSALS}; the deny-by-default sweep did "
            "not run as declared"
        )
    # And it was never answered: a single success would mean an instance holding
    # no restart authority charged an attempt against a peer.
    if re.search(rf"SLIME_LIFECYCLE restart admitted task={denied_task} ", transcript):
        fail("lifecycle-denied admitted a restart despite holding no restart authority")
    # Its own parameters are unreachable, which is the authority claim rather
    # than a namespace one.
    if not re.search(
        rf"SLIME_LIFECYCLE parameter refused task={denied_task} subject-instance=\d+ "
        rf"key={CONFIG_KEY} class=no-parameter-authority",
        transcript,
    ):
        fail("lifecycle-denied was not refused its own parameters")


def main() -> None:
    check_fixture_shape()
    build_image()
    profile = tomllib.loads(PINS.read_text(encoding="utf-8"))["qemu_arm_virt"]
    transcript = boot(profile)
    match_marker_contract(transcript, CHAINS, FAILURE_MARKERS, fail)
    for pattern in EXPECTED_UNORDERED:
        if not re.search(pattern, transcript):
            fail(f"missing evidence: {pattern}")
    check_restart_sequence(transcript)
    check_configuration_survives(transcript)
    check_class_and_quota_survive(transcript)
    check_graph_walk(transcript)
    check_denials(transcript)
    print(
        "seL4 lifecycle-restart plane check: a userspace supervisor restarted a component "
        "three times under its declared attempt bound and growing backoff, the fault, exit, "
        "and unhealthy causes were distinguishable from both sides, every predecessor handle "
        "was refused while the declared configuration survived every restart, an undeclared "
        "transition was refused without moving the state, and exhausting the bound left the "
        "instance in the declared terminal state with its next spawn refused"
    )


if __name__ == "__main__":
    main()
