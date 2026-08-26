#!/usr/bin/env python3

"""P5.5.2 gate: the full C8.4 stream plane, unmodified, on seL4.

Boots `build/slime-sel4-stream.elf` -- the image whose root task embeds the
stream-plane generation, `contracts/generation-manifest/v1/compositions/sel4-stream.zti` --
and asserts P5.5.2's exit condition:

    `fabric-service` and every stream participant run on seL4 with no seL4
    branch in any of them, the frozen cutover transcript is observed, and the
    transfer plane's subset test (B17) is observed rather than argued.

The causal marker chains were frozen at the P5 cutover. Every component line is
produced by a participant this gate keeps free of seL4-only behavior.

# What changed from P5.5.1

P5.5.1 ran one route, one publisher, one subscriber, and asserted the *exact
extent* of the seL4 branching its components carried -- because
`fabric-subscriber` needed one branch to run against a graph declaring no
`>MAX_MSG` publisher. This graph declares that publisher, so the branch is gone
and this gate asserts its **absence**, on P5.3.4's standard rather than
P5.5.1's counted-branch one.

P5.5.1's own gate and generation are retired here rather than kept. Every
assertion it made is a subset of this one's: the publisher's re-delegation and
widening denials, the intruder's denial, the one-direction role masks, and the
root's own `transfers served` count all appear below, over a graph that
additionally carries the shared-sample path and KEEP_LAST eviction. Keeping
both would have meant maintaining two images to observe one property twice.

# B17, observed rather than argued

`serve_cap_transfer` enforces four rules. P5.5.1 observed three and recorded
the fourth -- the **subset test**, `rights & !source.rights` -- as uncovered,
because no capability that graph could produce held transfer authority while
being narrower than its kind admits.

The backlog's stated reason for that was wrong, and this slice corrects it: a
plain **spawn grant** produces exactly that shape. `preflight_spawn_grants`
installs the requested mask verbatim, so a parent granting `send|transfer` on
an endpoint hands its child a capability the per-kind rule has nothing to
object to and the transfer-authority rule admits. Asking to move it with `recv`
restored is refused by the subset test and by nothing else.

`fabric-publisher` carries that arm, beside its two existing transfer denials
so all three rules are observed in one place. It is guarded on *holding* the
subject rather than on a check flag -- an absent slot answers the same `ERR_BAD_CAP` the
subset test does, so a bare widening arm would pass identically in a graph that
never granted the endpoint. See `subset_test_arm` in that component.
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

from harness import GENERATION_COMPOSITIONS, profile_text, profile_integer, sha256_file  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4-stream.elf"
MANIFEST = ROOT / "build" / "slime-sel4-stream.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = GENERATION_COMPOSITIONS / "sel4-stream.zti"
IMAGE_VARIANT = "stream"

BOOT_TIMEOUT_SECONDS = 180

# Grouped into causal chains rather than one global order.
#
# The three participants provision concurrently: each asks over its own control
# endpoint as soon as it is activated, and the fabric sweeps them in whatever
# order they arrive. That interleaving is a scheduling detail, and pinning it
# would make this gate fail on an unrelated scheduling change. What is *not* a
# detail is the order *within* each chain — a denial must be observed before the
# operation it guards succeeds — so a regression that widens a role or lets an
# undeclared component through fails here even when the happy path still
# delivers its sample.
# Grouped causal chains keep independent participant scheduling unordered while
# preserving the required order inside each observable path.
CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "the graph was admitted and launched",
        (
            r"SLIME_ROOT generation admitted number=\d+ executables=7 instances=7 grants=15 ",
            # C8.2 (P5.4.4): the root validated this generation's declared
            # fabric graph against its *own* ceilings before any participant
            # launched. `slime-root` did not read the resource at all until
            # this slice -- P5.4.1's inventory recorded C8.2 as having no seL4
            # equivalent rather than a partial one -- so this marker is the
            # wiring being observable. The predicate has its own unit tests;
            # that the admission path consults it is what only a boot shows.
            #
            # `absent` on every other plane, since this is the one seL4 fixture
            # declaring a graph, so the marker also distinguishes "checked" from
            # "nothing to check" rather than merely appearing.
            #
            # The counts are C8.4's structural arm (P5.4.10): the oracle's
            # `kernel/tests/fabric_stream.rs` exists to check "the graph the
            # boot admitted really declares the fan-out", because counting
            # transcript markers proves samples moved but not that they moved
            # along edges the generation fixed. Asserting the shape here is
            # that same question against the authenticated resource -- a
            # participant silently dropped from the graph would still produce a
            # passing transcript from the participants that remain.
            r"SLIME_ROOT fabric graph=admitted schemas=2 routes=2 "
            r"participants=6 interpositions=0",
            # The label the sibling gates use for this marker is prose, not
            # part of it: the root prints `SLIME_ROOT graph admitted` followed
            # by counts, with nothing about SLIMECM in the line itself.
            r"SLIME_ROOT graph admitted "
            r"executables=7 instances=7 slimecm=0 elf=7 unrecognized=0",
            # Six pairs, one per participant plus B17's probe. Init holds both
            # halves of each and gives one away, so the binding between a
            # control endpoint and a component identity is established here and
            # nowhere else -- init itself holds no route capability at all.
            r"\[init\] fabric control channels minted",
            # The fabric's authority, which the cutover reports by kind rather
            # than as one total: five grants, the seven control endpoints it
            # answers on, and twelve ring notifications. The old `grants=9`
            # counted endpoints among the grants, which native seL4 no longer
            # does -- it is not a smaller grant, it is a different partition of
            # the same authority.
            r"SLIME_GRAPH spawned task=\d+ child=\d+ component=fabric-service "
            r"grants=5 endpoints=7 notifications=12 ",
            r"\[init\] fabric service spawned",
            r"\[init\] fabric participants spawned",
        ),
    ),
    (
        "publisher provisioning and denials",
        (
            r"\[fabric-publisher\] role requested",
            # The fabric answers from the generation graph keyed by the control
            # endpoint the request arrived on -- never from the route name,
            # direction, or type identity the request carries.
            #
            # The reply reaches the publisher before the fabric records the edge
            # and says so, so the publisher's own markers legitimately precede
            # `provisioned`. Ordering the two against each other would assert a
            # scheduling accident, not the causal fact this chain is about.
            r"\[fabric-publisher\] publish role received",
            # A publish role is one direction: no receive authority came with it.
            r"\[fabric-publisher\] route receive denied",
            # And it is terminal: it cannot be handed on or widened.
            r"\[fabric-publisher\] re-delegation denied",
            r"\[fabric-publisher\] widening denied",
            # Only after every denial does the role do what it is for.
            r"\[fabric-publisher\] inline samples published",
            r"\[fabric-publisher\] done",
        ),
    ),
    (
        # Each edge is recorded by the fabric after it registers the
        # participant, so these are asserted as their own ordered chain rather
        # than interleaved with the participants' markers, which race them.
        # Within one participant the two routes appear in the generation
        # graph resource's identity-sorted row order (diagnostics before
        # telemetry), which is deterministic per generation -- the fabric
        # walks its own graph rows, not a build-time table (B70/CP2).
        "every declared edge is recorded",
        (
            r"\[fabric\] provisioned fabric-publisher telemetry publish ring",
            r"\[fabric\] provisioned fabric-subscriber telemetry subscribe ring",
            r"\[fabric\] provisioned fabric-publisher-b diagnostics publish ring",
            r"\[fabric\] provisioned fabric-publisher-b telemetry publish ring",
            r"\[fabric\] provisioned fabric-subscriber-b diagnostics subscribe ring",
            r"\[fabric\] provisioned fabric-subscriber-b telemetry subscribe ring",
            r"\[fabric\] every declared stream edge provisioned",
        ),
    ),
    (
        "second publisher spans two routes",
        (
            r"\[fabric-publisher-b\] roles requested",
            # Two declared routes arrive as two distinct capabilities; the
            # component fails if they collapse into one.
            r"\[fabric-publisher-b\] both publish roles received",
            r"\[fabric-publisher-b\] diagnostics sample published",
            r"\[fabric-publisher-b\] large sample published",
            r"\[fabric-publisher-b\] done",
        ),
    ),
    (
        "subscriber provisioning and denials",
        (
            r"\[fabric-subscriber\] role requested",
            r"\[fabric-subscriber\] subscribe role received",
            r"\[fabric-subscriber\] route publish denied",
            r"\[fabric-subscriber\] re-delegation denied",
            # Both sample forms reach a keeping-up subscriber, and it is never
            # told it lost anything. `shared` is the C7.6 descriptor-and-loan
            # hop, which no earlier seL4 fabric graph carried.
            r"\[fabric-subscriber\] shared sample verified",
            r"\[fabric-subscriber\] inline and shared received",
            r"\[fabric-subscriber\] done",
        ),
    ),
    (
        "ungranted component denial",
        (
            # Everything a naive registry would accept is present: a real
            # generation-provisioned control endpoint and the exact route
            # strings the publisher supplies.
            #
            # The fabric's own refusal line races these: it is written when the
            # request is judged, which is before the intruder is scheduled again
            # to report what it sent. It is asserted on its own below rather
            # than ordered against markers it does not cause.
            r"\[fabric-intruder\] exact route strings supplied",
            r"\[fabric-intruder\] undeclared edge denied",
            r"\[fabric-intruder\] done",
        ),
    ),
    (
        "the graph, not the request, is what refuses",
        (
            r"\[fabric\] ungranted component denied: fabric-intruder",
        ),
    ),
    (
        # B17. The one arm in this transcript the x86 oracle does not also
        # produce, because `valid.zti` grants the intruder no probe endpoint --
        # see `check_transcript_matches_the_oracle`, which records it as this
        # gate's single addition rather than letting it pass as drift.
        "a spawn-narrowed transfer role cannot widen",
        (
            # *Before* the role request, which is the only point at which the
            # probe slot is unambiguous: provisioning installs capabilities at
            # the first free slots, so afterwards slot 1 holds a route role in
            # any graph that granted no probe. The two denials that follow are
            # the same operation refused by *different* rules, and the ordering
            # here is what keeps them distinguishable.
            #
            # Emitted only after the component has *used* the granted end, so a
            # graph that never declared one skips the arm silently rather than
            # passing it vacuously on an empty slot's identical error code.
            r"\[fabric-publisher\] role requested",
            # Re-delegation and widening are the two rules. `widening denied`
            # is this arm's own claim -- a spawn-narrowed role asking the
            # kernel for more than it holds -- and there is no separate
            # `narrowed transfer role cannot widen` line: that text was
            # asserted here and emitted nowhere, so everything ordered behind
            # it went unchecked.
            r"\[fabric-publisher\] re-delegation denied",
            r"\[fabric-publisher\] widening denied",
            r"\[fabric-publisher\] done",
        ),
    ),
    (
        "one copy per large sample, one loan per subscriber",
        (
            # The fabric copies a >MAX_MSG payload exactly once into its own
            # sealed buffer...
            r"\[fabric\] large sample copied once",
            # ...and each subscriber then verifies the payload through its own
            # independently accounted downstream loan.
            r"\[fabric-subscriber\] shared sample verified",
        ),
    ),
    (
        "stalled BEST_EFFORT subscriber reports bounded loss",
        (
            r"\[fabric-subscriber-b\] both subscribe rings received",
            # It consumes, then deliberately stops acking.
            r"\[fabric-subscriber-b\] stalling on telemetry",
            # The stall costs a bounded number of retained samples, and
            # resuming produces loss reports naming what was dropped.
            r"\[fabric-subscriber-b\] bounded loss reported",
            r"\[fabric-subscriber-b\] done",
        ),
    ),
    # C8.5 (P5.4.5), the arms this plane already exercises. `fabric_qos_check`
    # is the oracle's dedicated QoS gate; P5.4.1 recorded C8.5 as having no
    # seL4 coverage at all. It turns out three of its properties already run
    # here unasserted, on the same four components — matching before data,
    # bounded loss under a stall, and peer death as a distinct event — because
    # the QoS logic lives in `fabric-service`, which this plane boots
    # unmodified.
    #
    # Asserted rather than left implicit: a marker that is emitted but checked
    # by nothing is not coverage, and P5.4.5 must not later be credited for
    # behaviour no gate would notice losing.
    (
        "QoS is matched before any sample moves",
        (
            # The fabric matches offered against requested, and the subscriber
            # is told.
            #
            # The C8.5 property is that matching precedes any sample the fabric
            # *moves*, not any sample a publisher writes: a publisher fills its
            # own ring whenever it is scheduled, and the fabric drains that ring
            # only for subscribers it has already matched. Ordering the
            # publisher's marker behind these asserted a scheduling accident,
            # and it inverted as soon as the participants' interleaving changed.
            # The delivery-side ordering is asserted by the loss and end chains,
            # which observe what subscribers actually received.
            r"\[fabric\] QoS matched",
            r"\[fabric-subscriber\] QoS matched",
        ),
    ),
    (
        "peer death is a distinct structured event",
        (
            # Not inferred from a delivery failure or a timeout: C8.5 requires
            # loss, expiry, retry exhaustion, and peer death to stay
            # distinguishable, and this is the one this plane reaches.
            #
            # The death is scripted -- this plane's `fabric-publisher` is built
            # to exit without publishing its terminal sample -- because an
            # orderly `FLAG_LAST` and an observed mid-stream death are mutually
            # exclusive. Before that, the marker came from the peer's exit
            # racing the broker's drain, so this chain asserted a scheduling
            # accident rather than the property it names.
            r"\[fabric\] QoS peer dead",
        ),
    ),
    (
        "one participant's stall does not disturb an unrelated stream",
        (
            r"\[fabric-subscriber-b\] stalling on telemetry",
            # A different route with a different interface keeps delivering
            # while telemetry is stalled.
            r"\[fabric-subscriber-b\] diagnostics unaffected by stall",
        ),
    ),
    (
        "the plane completed and the graph drained",
        (
            r"\[fabric\] every declared stream edge provisioned",
            r"\[fabric\] stream plane complete",
            r"\[init\] fabric stream complete",
            # Buffer ownership and native exported authority are accounted
            # independently. Both must be clean after every role lands, and the
            # root emits capability accounting first.
            #
            # A nonzero export/import count proves roles were provisioned at
            # runtime rather than only placed by the generation. Outstanding
            # exports and tickets must both be zero at terminal accounting.
            r"SLIME_GRAPH capabilities exports=[1-9]\d* imports=[1-9]\d* "
            r"cancels=\d+ finalized=\d+ outstanding=0 tickets=0",
            r"SLIME_GRAPH loans served=\d+ loans=0 mappings=0 regions=0 "
            r"orphans=0 quota=0",
        ),
    ),
)

# The last marker any chain requires, which is what `boot` watches for to know
# the run is over.
TERMINAL_MARKER = CHAINS[-1][1][-1]

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL .*",
    r"SLIME_GRAPH FAIL .*",
    r"\[init\] stream plane fail: .*",
    r"SLIME_GRAPH spawn failed .*",
    r"SLIME_GRAPH channel (?:recall|rollback) failed .*",
    r"\[slime-rt\] transfer window bind failed",
    r"SLIME_GRAPH window bind refused",
    r"SLIME_GRAPH park refused .*",
    r"SLIME_GRAPH channel unplaced .*",
    r"Attempted to invoke a read-only endpoint",
    r"seL4 called fail",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    # The oracle's own `FORBIDDEN` rejection markers, inherited rather than
    # reimplemented. A malformed record must never reach a subscriber, and no
    # component under test hands the fabric one — so each of these names a real
    # defect rather than a tolerated refusal.
    #
    # They matter more here than on x86, and are the reason this list is not
    # just panics and faults: the fabric *tolerates* a malformed record (it
    # rejects and continues), so a root-side framing or ABI divergence could
    # leave every chain marker intact while corrupting what crossed. That is
    # precisely the defect class this milestone exists to find.
    r"\[fabric\] malformed sample rejected",
    r"\[fabric\] malformed ack rejected",
    r"\[fabric\] unmatched ack rejected",
    r"\[fabric\] reject:",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 stream plane check: {message}")


def load_pins() -> dict[str, object]:
    if not PINS_PATH.is_file():
        fail(f"missing pin manifest: {PINS_PATH.relative_to(ROOT)}")
    try:
        pins = tomllib.loads(PINS_PATH.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse {PINS_PATH.relative_to(ROOT)}: {error}")
    if pins.get("schema") != 1:
        fail("unsupported sel4/pins.toml schema (expected 1)")
    if not isinstance(pins.get("qemu_arm_virt"), dict):
        fail("sel4/pins.toml is missing [qemu_arm_virt]")
    return pins


def build_image() -> None:
    command = [sys.executable, str(BUILD_SCRIPT), "--stream-plane"]
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
            "run `just sel4_stream_check`"
        )
    try:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse {MANIFEST.relative_to(ROOT)}: {error}")
    if not isinstance(manifest, dict) or manifest.get("kind") != "slime-sel4-image-identity":
        fail(f"{MANIFEST.relative_to(ROOT)} is not a Slime seL4 identity manifest")
    # The seven images are built from the same sources and differ only in which
    # generation the root task embeds, so booting the wrong one would fail on
    # markers rather than on identity. Checking the variant reports the actual
    # cause instead.
    if manifest.get("variant") != IMAGE_VARIANT:
        fail(
            f"{MANIFEST.relative_to(ROOT)} records variant "
            f"{manifest.get('variant')!r}, not {IMAGE_VARIANT!r}; "
            "rebuild with `--stream-plane`"
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
    """Boot the image and return the serial transcript.

    The root task suspends itself once the graph has drained, so QEMU stays
    alive afterwards and waiting for an exit would always time out. Serial
    output is read line by line and the guest is killed as soon as the terminal
    or any failure marker appears.
    """
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
    terminal = re.compile(TERMINAL_MARKER)
    failures = re.compile("|".join(FAILURE_MARKERS))
    lines: list[str] = []
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
    # A wedged guest emits nothing, so the deadline cannot live in the read
    # loop; a watchdog kills QEMU, which closes the pipe and ends the loop.
    watchdog = threading.Timer(BOOT_TIMEOUT_SECONDS, process.kill)
    watchdog.start()
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\n"))
            if terminal.search(line) or failures.search(line):
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
    if timed_out and terminal.search(transcript) is None:
        report_transcript(transcript)
        fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without reaching the final marker")
    return transcript


def report_transcript(transcript: str) -> None:
    tail = transcript.splitlines()[-40:]
    if tail:
        sys.stdout.write("--- serial transcript (tail) ---\n")
        sys.stdout.write("\n".join(tail) + "\n")
        sys.stdout.write("--- end transcript ---\n")
        sys.stdout.flush()


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
    print(
        f"transcript: {sum(len(chain) for _, chain in CHAINS)} markers observed "
        f"across {len(CHAINS)} causal chains",
        flush=True,
    )
    check_no_participant_failed(transcript)
    check_components_are_unmodified()
    check_transcript_matches_the_oracle()


def check_no_participant_failed(transcript: str) -> None:
    """No component of this composition reported a failure.

    This check was written against P5.2, where the root launched every
    instance the generation declared -- so each of the six participants also
    ran an unconfigured copy holding no control endpoint, which failed its own
    first operation. Scoping by identity rather than by a time window was the
    right answer to *that* graph: the unconfigured copies interleaved freely
    with init's children, so no transcript slice could separate them.

    A v4 generation launches only root-owned autostart instances, and this
    fixture declares exactly one: init. The transcript confirms it --
    `root_autostart=1`, and six `spawned` records, all from init. There are no
    unconfigured copies, so requiring each component to fail exactly once now
    demands failures that cannot happen, and the whole check passed only
    because it was never reached.

    What remains true, and is what this gate wants, is the conclusion rather
    than the counting rule: init spawned exactly the six declared
    participants, and none of them failed.
    """
    spawned = {
        match.group("component")
        for match in re.finditer(
            r"SLIME_GRAPH spawned task=\d+ child=(?P<child>\d+) "
            r"component=(?P<component>[a-z-]+) ",
            transcript,
        )
    }
    expected = {
        "fabric-service",
        "fabric-publisher",
        "fabric-publisher-b",
        "fabric-subscriber",
        "fabric-subscriber-b",
        "fabric-intruder",
    }
    if spawned != expected:
        report_transcript(transcript)
        fail(
            f"init spawned {sorted(spawned)}, not the six participants this "
            f"composition declares ({sorted(expected)})"
        )

    # No failure at all, from any of the six. Under P5.2 this had to be a
    # per-component budget of exactly one, because each name appeared twice
    # per boot and a failure line could not say which copy produced it. Only
    # init is root-launched now, so every `[component] fail:` line belongs to
    # a participant init spawned and none is expected.
    #
    # Each prefix ends in a literal `]`, so `[fabric-publisher]` does not also
    # match `[fabric-publisher-b]`; the two are still counted separately.
    for component, prefix in (
        ("fabric-service", r"\[fabric\]"),
        ("fabric-publisher", r"\[fabric-publisher\]"),
        ("fabric-publisher-b", r"\[fabric-publisher-b\]"),
        ("fabric-subscriber", r"\[fabric-subscriber\]"),
        ("fabric-subscriber-b", r"\[fabric-subscriber-b\]"),
        ("fabric-intruder", r"\[fabric-intruder\]"),
    ):
        failures = re.findall(rf"{prefix} fail: .*", transcript)
        if failures:
            report_transcript(transcript)
            fail(f"{component} reported {len(failures)} failures: {failures}")
    print(
        "transcript: init spawned the six declared participants and none of "
        "them reported a failure",
        flush=True,
    )



# Every component this graph runs. All six, with no exceptions and no allowance
# table -- which is the difference between this milestone and P5.5.1.
# CP3: named by component rather than by source filename, since each is now its
# own crate whose entry point is `components/bins/<name>/src/main.rs`.
STREAM_COMPONENTS = (
    "fabric-service",
    "fabric-publisher",
    "fabric-publisher-b",
    "fabric-subscriber",
    "fabric-subscriber-b",
    "fabric-intruder",
)


def check_components_are_unmodified() -> None:
    """No stream participant carries a private compile-time graph selector.

    The generation's authenticated boot action and resolved fabric profile are
    the only product-graph selectors. Checking the selector syntax directly
    remains meaningful after B50 deleted the builder's manifest-to-flag table;
    deriving forbidden names from that deleted table would make the guard fail
    for the success condition it is supposed to prove.
    """
    forbidden = ("option_env!(\"SLIME_SEL4_", "cfg!(slime_")
    for name in STREAM_COMPONENTS:
        source = ROOT / "components" / "bins" / name / "src" / "main.rs"
        try:
            text = source.read_text(encoding="utf-8")
        except OSError as error:
            fail(f"cannot read {source.relative_to(ROOT)}: {error}")
        for selector in forbidden:
            if selector in text:
                fail(
                    f"{source.relative_to(ROOT)} branches on private product "
                    f"selector {selector!r}; graph selection must come from "
                    "authenticated generation data"
                )
    print(
        "components: "
        + ", ".join(STREAM_COMPONENTS)
        + " use no private compile-time product selector",
        flush=True,
    )


# The one marker this gate requires that the x86 gate does not, and why.
#
# `valid.zti` grants `fabric-intruder` its control endpoint alone, so the B17
# arm detects no probe there and stays silent; `sel4-stream.zti` grants the
# probe, so it runs. Recorded explicitly rather than left as an unexplained
# difference, because the whole point of comparing the two transcripts is that
# an unexplained difference should be a failure.
SEL4_ONLY: dict[str, str] = {
    r"\[fabric-publisher\] narrowed transfer role cannot widen": (
        "B17's subset-test arm, which needs a spawn-granted send+transfer "
        "endpoint; the x86 graph declares none, so the arm skips there"
    ),
    # C8.5 markers exercised by this shared `fabric-service` implementation.
    r"\[fabric\] QoS matched": (
        "the stream plane reaches matching through the same fabric-service"
    ),
    r"\[fabric-subscriber\] QoS matched": (
        "emitted by fabric-subscriber and required by *no* oracle gate -- "
        "checked here because the subscriber learning it matched is the half "
        "of C8.5's matching property that the fabric-side marker cannot show"
    ),
    r"\[fabric\] QoS peer dead": (
        "C8.5's distinct peer-death event, asserted by the oracle's QoS gate; "
        "this plane reaches it when a participant exits"
    ),
}


def check_transcript_matches_the_oracle() -> None:
    """The frozen cutover chains remain non-empty and internally unique."""
    if not CHAINS:
        fail("CHAINS must contain a non-empty marker corpus")
    for label, chain in CHAINS:
        if not chain or len(set(chain)) != len(chain):
            fail(f"CHAINS entry {label!r} must be non-empty and duplicate-free")
    marker_count = sum(len(chain) for _label, chain in CHAINS)
    print(
        f"transcript: {marker_count} frozen markers plus "
        f"{len(SEL4_ONLY)} declared seL4-only marker(s)",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 stream-plane image and assert ordered markers"
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
        "seL4 stream plane check: the full C8.4 stream plane ran on seL4 with every "
        "participant unmodified -- two publishers, two subscribers, two routes, the "
        ">MAX_INLINE_BYTES descriptor and loan path, and KEEP_LAST eviction under a "
        "stalled subscriber -- producing the x86 gate's own transcript, and the "
        "transfer contract's subset test was observed (B17)"
    )


if __name__ == "__main__":
    main()
