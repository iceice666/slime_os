#!/usr/bin/env python3
"""C9.3 gate: a declared class orders the CPU, and no component widens itself."""
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

from harness import GENERATION_COMPOSITIONS, profile_integer, profile_text, sha256_file  # noqa: E402
from sel4_gate_markers import match_marker_contract  # noqa: E402
from zutai_cli import STDLIB, binary  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
IMAGE = ROOT / "build" / "slime-sel4-scheduling-class.elf"
MANIFEST = ROOT / "build" / "slime-sel4-scheduling-class.identity.json"
BUILD = ROOT / "scripts" / "build" / "build-sel4.py"
PINS = ROOT / "sel4" / "pins.toml"
FIXTURE = GENERATION_COMPOSITIONS / "sel4-scheduling-class.zti"
IMAGE_VARIANT = "scheduling-class"
GENERATION = 43
TIMEOUT = 300

# The declared band mapping, read from the fixture too (`check_fixture_shape`),
# so a mutation that renumbers a band fails here rather than passing against
# whatever it became.
DECLARED_BANDS = (("foreground", 200), ("normal", 150), ("bestEffort", 100))
# The frozen class ids from `contracts/scheduling-class/v1`. Named here because
# the probe prints ids rather than spellings, and a gate comparing a spelling to
# a spelling would not notice the two drifting apart.
CLASS_IDS = {"undeclared": 0, "foreground": 1, "normal": 2, "bestEffort": 3}
# The root's own child priority, one below the root, which an instance in no
# declared band runs at. Asserted rather than derived so a change to the root's
# default has to be made deliberately here too.
CHILD_DEFAULT_PRIORITY = 254
# The promotion edge the fixture declares, and the ceiling it is bounded by.
DECLARED_EDGE = ("sched-controller", "sched-promotable", "normal")
# The class the promotion asks for, and the one above the ceiling that must be
# refused. Derived from the edge's ceiling rather than written as literals: a
# fixture that raised the ceiling to `foreground` would move both, instead of
# leaving the refusal assertion satisfied by a request that is no longer too
# high.
PROMOTED_CLASS = DECLARED_EDGE[2]
REFUSED_CLASS = "foreground"
# Slots `sched-denied` sweeps when proving it can promote nothing. Mirrors the
# probe's own `DENIED_SLOT_SWEEP`; asserting the exact count rather than a floor
# is what makes a regression that shortens the sweep fail here.
DENIED_SLOT_SWEEP = 8

BAND_PRIORITY = dict(DECLARED_BANDS)

CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "the declared band mapping the root resolved",
        (
            rf"SLIME_SCHED policy bands={len(DECLARED_BANDS)} instances=4 promotions=1 unnamed=undeclared",
        )
        + tuple(
            rf"SLIME_SCHED band class={name} priority={priority}"
            for name, priority in DECLARED_BANDS
        ),
    ),
    (
        "a declared promotion applies, and its ceiling is enforced",
        (
            # The subject is named by a capability, so the handle it resolved
            # through is printed first: promotion authority is not a name lookup.
            r"\[sched-controller\] spawned subject handle=(\d+)",
            rf"SLIME_SCHED promoted task=\d+ subject=\d+ class={PROMOTED_CLASS} "
            rf"priority={BAND_PRIORITY[PROMOTED_CLASS]}",
            # One band above the edge's declared ceiling. This is the assertion
            # that separates an enforced ceiling from a written-down one.
            r"SLIME_SCHED refused task=\d+ subject=\d+ class=above-ceiling detail=AboveCeiling",
            r"\[sched-controller\] above ceiling refused error=\d+",
            # `undeclared` is the read side's answer, never an assignment, so it
            # is refused even to a caller holding a real capability over the
            # subject -- which is what makes this a statement about the class
            # rather than about authority.
            r"\[sched-controller\] undeclared target refused error=\d+",
        ),
    ),
    (
        "no component widens its own class",
        (
            # The controller holds real promotion authority and still cannot
            # reach itself: the subject comes from a supervision capability, and
            # the root mints one only for a task's *spawner*, never for the task
            # itself. So the closest a holder can come is naming a slot that is
            # not a supervision capability, which is refused.
            r"\[sched-controller\] self promotion refused error=\d+",
            # And its own class is unchanged by everything it just did.
            rf"SLIME_SCHED read task=\d+ class={PROMOTED_CLASS} "
            rf"priority={BAND_PRIORITY[PROMOTED_CLASS]}",
            r"\[sched-controller\] controller complete",
        ),
    ),
    (
        "deny by default is a class, not a refusal",
        (
            rf"\[sched-denied\] undeclared priority={CHILD_DEFAULT_PRIORITY}",
            rf"\[sched-denied\] undeclared class id={CLASS_IDS['undeclared']}",
            r"\[sched-denied\] no promotion authority",
        ),
    ),
    (
        "terminal cleanup",
        (
            r"\[init\] scheduling class plane is root-launched",
            rf"SLIME_GRAPH HEALTHY generation={GENERATION} required=5 live=0 completed=5 failed=0",
        ),
    ),
)

EXPECTED_UNORDERED: tuple[str, ...] = (
    # Every instance's resolved class, root-attributed. These are the root's own
    # accounting rather than the probe's, so a component misreporting its band
    # cannot satisfy them.
    r"SLIME_SCHED class task=\d+ instance=sched-foreground class=foreground priority=200"
    r" worker=foreground worker_priority=200",
    r"SLIME_SCHED class task=\d+ instance=sched-burner class=bestEffort priority=100"
    r" worker=bestEffort worker_priority=100",
    r"SLIME_SCHED class task=\d+ instance=sched-controller class=normal priority=150"
    r" worker=normal worker_priority=150",
    # The instance the policy names no class for reads back as `undeclared` at
    # the root's own child priority -- the same number the builder left in that
    # instance's `ScheduleRecord`. Not `normal`: naming a band would report a
    # priority that band does not have, and would make promoting this subject to
    # `normal` look like a no-op while silently moving it (found by review).
    rf"SLIME_SCHED class task=\d+ instance=sched-denied class=undeclared"
    rf" priority={CHILD_DEFAULT_PRIORITY} worker=undeclared"
    rf" worker_priority={CHILD_DEFAULT_PRIORITY}",
    # The plan's own `ScheduleRecord`s carry the same priorities, so a class
    # reached the TCB rather than only the root's table. Same numbers, different
    # producer: these lines are written before any component runs, which is what
    # makes them a cross-check rather than a restatement.
    #
    # `sched-denied` is included deliberately: it is the one instance where the
    # two producers could disagree, because it is the one the policy does not
    # name. Omitting it here is what let an earlier revision report 150 for a
    # thread running at 254 (found by review).
    r"SLIME_GRAPH schedule instance=sched-foreground priority=200 default=254",
    r"SLIME_GRAPH schedule instance=sched-burner priority=100 default=254",
    r"SLIME_GRAPH schedule instance=sched-controller priority=150 default=254",
    rf"SLIME_GRAPH schedule instance=sched-denied priority={CHILD_DEFAULT_PRIORITY}"
    rf" default={CHILD_DEFAULT_PRIORITY}",
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_SCHED FAIL",
    r"\[sched\] FAIL",
    r"SLIME_GRAPH FAIL required instance",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 scheduling-class plane check: {message}")


def build_image() -> None:
    process = subprocess.run(
        [sys.executable, str(BUILD), "--scheduling-class-plane"],
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
        rf"SLIME_GRAPH HEALTHY generation={GENERATION} required=5 live=0 completed=5 failed=0"
        r"|SLIME_ROOT FATAL|SLIME_SCHED FAIL|\[sched\] FAIL"
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
    """Decode the exercised class policy through Zutai."""
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
    themselves: a fixture that renumbered a band or dropped the promotion edge
    would produce a transcript this gate no longer describes, and the gate would
    have no way to tell.
    """
    manifest = fixture_manifest()
    if manifest.get("generation") != GENERATION:
        fail(f"fixture declares generation {manifest.get('generation')!r}, expected {GENERATION}")
    policy = manifest.get("schedulingClass")
    if not isinstance(policy, dict):
        fail("fixture declares no schedulingClass policy")

    bands = {band["class"]: band["priority"] for band in policy["bands"]}
    if bands != BAND_PRIORITY:
        fail(f"fixture band mapping {bands!r} does not match the asserted {BAND_PRIORITY!r}")
    # Distinct priorities are what make the ordering observable at all; the
    # decoder refuses a collision, and this is the fixture-side statement of the
    # same property so the plane cannot be weakened into vacuity.
    if len(set(bands.values())) != len(bands):
        fail("fixture declares two bands at one priority, so no ordering is observable")
    # The saturating workload must be strictly below the foreground one, or the
    # interleaving assertion below would be asserting round-robin rather than
    # priority.
    if not bands["bestEffort"] < bands["foreground"]:
        fail("fixture does not place bestEffort below foreground")

    classes = {entry["instance"]: entry["class"] for entry in policy["instances"]}
    for instance, expected in (
        ("sched-foreground", "foreground"),
        ("sched-burner", "bestEffort"),
        ("sched-controller", "normal"),
    ):
        if classes.get(instance) != expected:
            fail(f"fixture declares {instance} as {classes.get(instance)!r}, expected {expected!r}")
    # The deny-by-default arm needs an instance the policy genuinely omits.
    declared_instances = {entry["name"] for entry in manifest["instances"]}
    if "sched-denied" not in declared_instances:
        fail("fixture has no sched-denied instance")
    if "sched-denied" in classes:
        fail("fixture names sched-denied in its class policy, so nothing tests the default")

    promotions = policy["promotions"]
    if len(promotions) != 1:
        fail(f"fixture declares {len(promotions)} promotion edges, expected exactly 1")
    edge = promotions[0]
    if (edge["holder"], edge["subject"], edge["ceiling"]) != DECLARED_EDGE:
        fail(f"fixture promotion edge {edge!r} does not match the asserted {DECLARED_EDGE!r}")
    # Self-promotion must be unrepresentable, not merely absent by luck.
    if edge["holder"] == edge["subject"]:
        fail("fixture declares a self-promotion edge, which the decoder must refuse")
    # The refused request must be genuinely above the ceiling, or the refusal
    # assertion tests nothing.
    if not bands[REFUSED_CLASS] > bands[edge["ceiling"]]:
        fail(
            f"{REFUSED_CLASS} is not above the declared ceiling {edge['ceiling']}, "
            "so the refused promotion would not be testing the ceiling"
        )


def check_preemption(transcript: str) -> None:
    """The milestone's first required check, read as ordered interleaving.

    A saturating `bestEffort` workload cannot prevent a `foreground` component
    from being scheduled. Marker *presence* cannot show this and neither can
    marker order alone: what distinguishes a scheduler honouring the declared
    bands from one ignoring them is that foreground progress lands *between* two
    chunks of a burn loop that is still running.

    So the assertion is on serial positions, in the shape
    `check-sel4-traffic-plane.py::check_concurrency` uses: at least one
    foreground step must sit strictly between two consecutive burner chunk
    markers. A priority-ignoring scheduler on one vCPU runs the burner's whole
    200M-iteration loop first, producing every chunk marker before any
    foreground step and failing here.

    Elapsed time is deliberately not used: the harness pins no `-icount`, so a
    duration would be a host-load measurement rather than a scheduling property
    (B75).
    """
    steps = [match.start() for match in re.finditer(r"\[sched-foreground\] progress step=\d+", transcript)]
    chunks = [match.start() for match in re.finditer(r"\[sched-burner\] chunk=\d+", transcript)]
    if len(steps) < 2:
        fail(f"the foreground component emitted {len(steps)} progress steps; expected several")
    if len(chunks) < 2:
        fail(f"the burner emitted {len(chunks)} chunk markers; expected several")
    spinning = transcript.find("[sched-burner] bestEffort spinning")
    if spinning < 0:
        fail("the burner never announced that it had started spinning")
    # Foreground progress after the burn began at all. Necessary but not
    # sufficient, so it is only the first half.
    if not any(step > spinning for step in steps):
        fail("no foreground progress occurred after the burner started spinning")
    interleaved = any(
        start < step < end
        for start, end in zip(chunks, chunks[1:], strict=False)
        for step in steps
    )
    if not interleaved:
        fail(
            "no foreground progress landed between two burner chunks: the transcript is "
            "consistent with the burn loop running to completion first, which is what a "
            "scheduler ignoring the declared bands would produce"
        )
    # And the burner did finish, so the foreground's preemption did not starve
    # the lower band into never completing. A class orders CPU access; it does
    # not reserve or deny an amount of it.
    if "[sched-burner] bestEffort complete" not in transcript:
        fail("the bestEffort workload never completed, so foreground preemption starved it")


def check_semantics(transcript: str) -> None:
    """Bind the probe's observations to the root's own accounting."""
    promoted = re.search(
        r"SLIME_SCHED promoted task=(\d+) subject=(\d+) class=(\w+) priority=(\d+)",
        transcript,
    )
    if promoted is None:
        fail("the root recorded no promotion")
    caller, subject, class_name, priority = promoted.groups()
    # The two must differ. The decoder refuses a *declared* self-edge and the
    # service refuses a *caller* naming itself, and this reads the second of
    # those off a promotion that actually happened.
    if caller == subject:
        fail("the root recorded a promotion whose caller and subject are the same task")
    if class_name != PROMOTED_CLASS or int(priority) != BAND_PRIORITY[PROMOTED_CLASS]:
        fail(
            f"promotion resolved {class_name}/{priority}, not "
            f"{PROMOTED_CLASS}/{BAND_PRIORITY[PROMOTED_CLASS]} from the declared band"
        )
    # The controller's own class after promoting a peer, from the root rather
    # than from the component: holding authority over another component is not
    # authority over yourself.
    unchanged = re.findall(
        rf"SLIME_SCHED read task={caller} class=(\w+) priority=(\d+)", transcript
    )
    if not unchanged:
        fail("the controller never read its own class back")
    for observed_class, observed_priority in unchanged:
        if (
            observed_class != PROMOTED_CLASS
            or int(observed_priority) != BAND_PRIORITY[PROMOTED_CLASS]
        ):
            fail(
                f"the controller's own class became {observed_class}/{observed_priority} "
                "while it was promoting a peer"
            )
    # The denied instance must have been refused every slot it tried, and never
    # answered. Bound to *its own* task id: the controller also emits this exact
    # marker for its self-promotion attempt, so an unbound count could reach the
    # threshold on the controller's line plus a single denied refusal — which is
    # the "lucky empty slot" vacuity this assertion exists to exclude (found by
    # review).
    denied = re.search(r"SLIME_SCHED class task=(\d+) instance=sched-denied", transcript)
    if denied is None:
        fail("the root recorded no class for sched-denied")
    denied_task = denied.group(1)
    denied_refusals = len(
        re.findall(
            rf"SLIME_SCHED refused task={denied_task} class=undeclared detail=slot",
            transcript,
        )
    )
    # Exactly, not at least: `run_denied` emits one refusal per swept slot and
    # nothing else, so a regression that shortens the sweep -- or one that lets a
    # slot succeed -- moves this number either way (found by review, which caught
    # an earlier revision emitting nine against a floor of eight).
    if denied_refusals != DENIED_SLOT_SWEEP:
        fail(
            f"sched-denied had {denied_refusals} promotion attempts refused for want of "
            f"authority, expected exactly {DENIED_SLOT_SWEEP}; the deny-by-default sweep "
            "did not run as declared"
        )
    # And it was never *answered*: a single success would mean an instance
    # holding no promotion authority reprioritized a peer.
    if re.search(rf"SLIME_SCHED promoted task={denied_task} ", transcript):
        fail("sched-denied promoted a peer despite holding no promotion authority")


def main() -> None:
    check_fixture_shape()
    build_image()
    profile = tomllib.loads(PINS.read_text(encoding="utf-8"))["qemu_arm_virt"]
    transcript = boot(profile)
    match_marker_contract(transcript, CHAINS, FAILURE_MARKERS, fail)
    for pattern in EXPECTED_UNORDERED:
        if not re.search(pattern, transcript):
            fail(f"missing evidence: {pattern}")
    check_preemption(transcript)
    check_semantics(transcript)
    print(
        "seL4 scheduling-class plane check: the declared bands reached every TCB, a "
        "foreground component preempted a saturating bestEffort loop still in flight, a "
        "declared promotion applied within its ceiling, no component widened itself, and "
        "an instance the policy does not name read back as undeclared at the root's own "
        "child priority rather than at any band"
    )


if __name__ == "__main__":
    main()
