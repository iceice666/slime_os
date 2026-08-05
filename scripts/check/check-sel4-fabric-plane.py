#!/usr/bin/env python3

"""P5.5.1 gate: one declared typed route, carried over seL4.

Boots `build/slime-sel4-fabric.elf` -- the image whose root task embeds the
typed-fabric generation, `contracts/generation/v1/fixtures/sel4-fabric.zti` --
and asserts P5.5's exit condition:

    One declared typed route carries a sample from a publisher to a subscriber
    over seL4, with the route endpoints provisioned by the fabric from the
    generation's declared edges, a re-delegation refused, and an undeclared
    participant denied.

# What this gate is asserting, and what it is not

The four clauses above are C8.3-shaped: they are about *where route authority
comes from*, not about how much data a route can move. So the graph is the
smallest one that can carry them -- one route, one publisher, one subscriber,
and one component the graph declares no edge for.

The mechanism under test is `Operation::CapTransfer`, which `slime-root` did
not mediate before this slice. Everything the fabric provisions crosses through
it: a publisher's send-only half, a subscriber's receive-only half, and the ack
and credit channels that keep each role one direction. `SLIME_GRAPH transfers
served=` is the root's own count of those moves, and it is what distinguishes a
graph whose authority was *placed by the generation* from one whose authority
was *handed on by a broker* -- every earlier seL4 gate reports zero.

Deliberately **not** asserted here, because P5.5.1's graph cannot produce them:

  * the `>MAX_INLINE_BYTES` shared-sample path, which needs a second publisher
    (`fabric-publisher-b`) and the C7.6 descriptor/loan hop;
  * KEEP_LAST eviction and `SAMPLE_LOST`, which need a stalled subscriber;
  * the call and operation planes, and the two-route many-to-many fan-in.

Those are P5.5.2's, which runs the full stream plane with the components
unmodified. This slice does not claim them, and the roadmap records the split.

# One rule this gate does *not* cover, stated rather than implied

`serve_cap_transfer` enforces four rules. Three are observed here: transfer
authority at the source (`re-delegation denied`), the per-kind mask
(`widening denied`), and the descriptor/kind agreement. The fourth -- the
**subset test**, `rights & !source.rights` -- is not, and deleting it leaves
every marker below intact. Fault injection found that, and the arm was removed
rather than left as coverage it does not provide.

It is unreachable from any graph this cutover can currently declare, because
the four rules are one disjunction and every candidate subject fails an earlier
one first:

  * a **provisioned role** carries no `RIGHT_TRANSFER`, so rule 1 refuses it --
    which is exactly why `fabric-publisher`'s own widening attempt proves the
    per-kind rule rather than this one;
  * a **factory** is granted its single operation right and no transfer bit,
    so rule 1 again;
  * an **endpoint** minted by `endpoint_create` holds `send|recv|transfer`,
    which is precisely what `valid_rights` admits for its kind, so no mask can
    widen it without the per-kind rule refusing the same mask.

Reaching it needs a capability that holds transfer authority *and* is strictly
narrower than its kind admits. `cap_transfer` itself is the only thing that
produces one -- a role moved with `FLAG_RETAIN_TRANSFER` -- and a component
cannot move a capability to itself, because the two ends of a channel it holds
alone are a loopback the root refuses to split.

So the coverage gap is a property of the *graph*, not of the root: a two-broker
composition where one fabric provisions another would produce the subject
naturally. That is P5.5.2's shape, and the backlog records it.

# On "unmodified"

P5.3.4's exit condition said "unmodified" and this one does not, which is the
reason this gate checks something weaker than that one did.
`check_components_are_minimally_branched` below asserts the *exact* extent of
the seL4 branching rather than its absence, so the difference between this
slice and P5.5.2 stays a fact in the transcript instead of a claim in a
comment.
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

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4-fabric.elf"
MANIFEST = ROOT / "build" / "slime-sel4-fabric.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-fabric.zti"
IMAGE_VARIANT = "fabric"

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
#
# This is the shape `check-fabric-authority.py` and `check-data-fabric-boot.py`
# already use, and for the same reason.
CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "the graph was admitted and launched",
        (
            r"SLIME_ROOT generation admitted number=\d+ components=5 grants=9 ",
            r"SLIME_ROOT graph admitted; legacy SLIMECM images not activated "
            r"components=5 slimecm=0 elf=5 unrecognized=0",
            # Three pairs, one per participant. Init holds both halves of each
            # and gives one away, so the binding between a control endpoint and
            # a component identity is established here and nowhere else -- init
            # itself holds no route capability at all.
            r"\[init\] fabric control channels minted",
            # The fabric starts before any participant, so no request can
            # arrive before the service can answer it. Its grants are its two
            # factories and the three control halves -- five, which is over the
            # four records this root admitted before B15 was closed.
            r"SLIME_GRAPH spawned task=\d+ child=\d+ component=fabric-service grants=5 ",
            r"\[init\] fabric service spawned",
            r"\[init\] fabric participants spawned",
        ),
    ),
    (
        "the publish role was provisioned, narrowed, and is terminal",
        (
            # The publisher starts with one control endpoint and no route at
            # all. Asking is the only way it can obtain one.
            r"\[fabric-publisher\] role requested",
            # The fabric answers from the generation graph keyed by the control
            # endpoint the request arrived on -- never from the route name,
            # direction, or type identity the request carries.
            r"\[fabric\] provisioned fabric-publisher telemetry publish",
            r"\[fabric-publisher\] publish role received",
            # A publisher has no receive authority on its own route: the fabric
            # holds the other half, and this half was narrowed to send.
            r"\[fabric-publisher\] route receive denied",
            # The role is terminal. Re-transferring it -- even back over the
            # control endpoint, even narrowing further -- must fail, because
            # the move omitted `RIGHT_TRANSFER`.
            r"\[fabric-publisher\] re-delegation denied",
            # Nor can a participant widen its own role by asking for more than
            # it holds: the transfer path is narrow-only.
            r"\[fabric-publisher\] widening denied",
            # Only after every denial does the role do what it is for.
            r"\[fabric-publisher\] inline samples published",
            r"\[fabric-publisher\] done",
        ),
    ),
    (
        "the subscribe role was provisioned, narrowed, and is terminal",
        (
            r"\[fabric-subscriber\] role requested",
            r"\[fabric\] provisioned fabric-subscriber telemetry subscribe",
            r"\[fabric-subscriber\] subscribe role received",
            r"\[fabric-subscriber\] route publish denied",
            # The ack channel is a separate object in the opposite direction,
            # so releasing a delivery slot never requires publish authority.
            r"\[fabric-subscriber\] ack channel is send-only",
            r"\[fabric-subscriber\] re-delegation denied",
            # The sample reached the far end of the route. Not the fabric's own
            # bookkeeping: the subscriber decodes a payload that is a function
            # of the sequence, so it verifies the exact sample the publisher
            # sent rather than a well-formed one.
            r"\[fabric-subscriber\] inline received",
            r"\[fabric-subscriber\] done",
        ),
    ),
    (
        "an undeclared participant is denied",
        (
            # Everything a naive registry would accept is present: a real
            # generation-provisioned control endpoint and the exact route
            # strings the publisher supplies.
            r"\[fabric-intruder\] exact route strings supplied",
            # It is refused anyway, because the graph declares no edge for it.
            # Authority is the graph's, and a name grants nothing.
            r"\[fabric\] ungranted component denied: fabric-intruder",
            # The denial is total: a refusal status, an empty rights mask, and
            # no capability in the message at all.
            r"\[fabric-intruder\] undeclared edge denied",
            r"\[fabric-intruder\] done",
        ),
    ),
    (
        "both route halves crossed as one-direction endpoints",
        (
            # C8.3's narrow-on-transfer move, which `slime-root` mediates for
            # the first time in this slice. `rights=0x1` is `RIGHT_SEND` alone
            # and `0x2` is `RIGHT_RECV` alone: the transfer bit is dropped at
            # the destination because the descriptor did not set
            # `FLAG_RETAIN_TRANSFER`, which is what makes each role
            # non-delegable by construction rather than by convention.
            #
            # Together these are the property that a role is one direction: the
            # two halves of a route are separate objects, so neither
            # participant can perform the other's operation even by misusing
            # what it holds. Unordered against each other -- which participant
            # is provisioned first is the scheduling detail above -- so each is
            # its own one-element chain.
            r"SLIME_GRAPH capability transferred task=\d+ channel=\d+ to=\d+ "
            r"kind=endpoint rights=0x1\b",
        ),
    ),
    (
        "the subscribe half crossed receive-only",
        (
            r"SLIME_GRAPH capability transferred task=\d+ channel=\d+ to=\d+ "
            r"kind=endpoint rights=0x2\b",
        ),
    ),
    (
        "the route carried its sample and the graph drained",
        (
            r"\[fabric\] every declared stream edge provisioned",
            r"\[fabric\] stream plane complete",
            r"\[init\] fabric plane complete",
            # Every in-flight capability settled. A transfer parks its
            # capability in the transit table between the send and the receive
            # that collects it, so a nonzero `transit` would mean a role was
            # moved and never landed -- authority belonging to nobody.
            r"SLIME_GRAPH loans served=\d+ loans=0 mappings=0 regions=0 transit=0 "
            r"orphans=0 aliases=0",
            # The count this slice adds, and an exact number rather than a
            # nonzero one: **four** narrow-on-transfer moves, which is every
            # capability the fabric provisioned -- the publisher's data and
            # credit halves, and the subscriber's data and ack halves. The
            # intruder's denial carries none, which is what makes four rather
            # than six the right number and is why the denial is checked here
            # too rather than only in its own chain.
            #
            # Zero would mean every role was placed by the generation rather
            # than provisioned by the fabric, which is exactly the property the
            # milestone exists to establish; every earlier seL4 gate reports
            # zero because no component in those graphs invokes the operation.
            r"SLIME_GRAPH transfers served=4\b",
        ),
    ),
)

# The last marker any chain requires, which is what `boot` watches for to know
# the run is over.
TERMINAL_MARKER = CHAINS[-1][1][-1]

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL .*",
    r"SLIME_GRAPH FAIL .*",
    r"\[init\] fabric plane fail: .*",
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
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 fabric plane check: {message}")


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


def profile_text(profile: dict[str, object], key: str) -> str:
    value = profile.get(key)
    if not isinstance(value, str) or not value:
        fail(f"sel4/pins.toml [qemu_arm_virt].{key} must be non-empty text")
    return value


def profile_integer(profile: dict[str, object], key: str) -> int:
    value = profile.get(key)
    if not isinstance(value, int) or isinstance(value, bool):
        fail(f"sel4/pins.toml [qemu_arm_virt].{key} must be an integer")
    return value


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    try:
        with path.open("rb") as handle:
            while chunk := handle.read(1024 * 1024):
                digest.update(chunk)
    except OSError as error:
        fail(f"cannot hash {path.relative_to(ROOT)}: {error}")
    return digest.hexdigest()


def build_image() -> None:
    command = [sys.executable, str(BUILD_SCRIPT), "--fabric-plane"]
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
            "run `just sel4_fabric_check`"
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
            "rebuild with `--fabric-plane`"
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
    check_components_are_minimally_branched()


def check_no_participant_failed(transcript: str) -> None:
    """No component *of this composition* reported a failure.

    Scoped, for the reason P5.3.4's gate records: the root launches every
    component the generation declares (P5.2), so this boot also starts one
    unconfigured `fabric-service`, `fabric-publisher`, `fabric-subscriber`, and
    `fabric-intruder` holding no control endpoint at all. Each fails its own
    first operation and exits non-zero. Those failures are expected, and reading
    them as this composition's would be reading a different graph.

    Scoped by **identity rather than by time**, which P5.3.4's window could not
    be. There the unconfigured pair failed before the composition began, so a
    transcript slice separated them; here the four unconfigured instances are
    activated alongside init's four children and interleave freely with them --
    the unconfigured service fails its first `endpoint_create` *while* the
    composition is still brokering. A window would either admit that failure or
    exclude a real one depending on scheduling.

    So the composition's members are identified exactly: the root names each
    child it constructs, and every `[component] fail:` line is attributed by
    counting which instance produced it. The unconfigured instances are the ones
    the root *launched*; the composition's are the ones init *spawned*.
    """
    spawned = {
        match.group("component")
        for match in re.finditer(
            r"SLIME_GRAPH spawned task=\d+ child=(?P<child>\d+) "
            r"component=(?P<component>[a-z-]+) ",
            transcript,
        )
    }
    expected = {"fabric-service", "fabric-publisher", "fabric-subscriber", "fabric-intruder"}
    if spawned != expected:
        report_transcript(transcript)
        fail(
            f"init spawned {sorted(spawned)}, not the four participants this "
            f"composition declares ({sorted(expected)})"
        )

    # Each component name appears twice per boot -- once unconfigured, once
    # spawned -- so a failure line alone cannot say which produced it. What can
    # is the count: the unconfigured instance of each contributes exactly one
    # failure, so a second from the same component is necessarily the spawned
    # one. `fabric-service` logs as `[fabric]`.
    #
    # `!= 1`, not `> 1`. Requiring the unconfigured failure to be *present*
    # rather than merely tolerated is what keeps the premise structural instead
    # of scheduling-dependent: if an unconfigured instance ever stopped failing
    # -- it faults before reaching its first operation, or its first `send`
    # succeeds because the control channels the generation declares happen to be
    # materialised by then -- then a real participant's failure would land in
    # its budget and pass unnoticed. Under `> 1` that regression is silent;
    # under `!= 1` the disappearance itself fails the gate and says so.
    for component, prefix in (
        ("fabric-service", r"\[fabric\]"),
        ("fabric-publisher", r"\[fabric-publisher\]"),
        ("fabric-subscriber", r"\[fabric-subscriber\]"),
        ("fabric-intruder", r"\[fabric-intruder\]"),
    ):
        failures = re.findall(rf"{prefix} fail: .*", transcript)
        if len(failures) != 1:
            report_transcript(transcript)
            fail(
                f"{component} reported {len(failures)} failures; exactly one is "
                f"expected (the unconfigured instance the root launches): {failures}"
            )
    print(
        "transcript: each fabric component failed exactly once -- the "
        "unconfigured instance the root launches -- so no participant init "
        "spawned reported a failure",
        flush=True,
    )


# Every compile-time scenario flag any seL4 gate sets. A fabric component
# branching on one of the *other* five would be tailoring itself to a scenario
# it has no part in.
OTHER_SEL4_CHECK_FLAGS = (
    "SLIME_SEL4_CHANNEL_CHECK",
    "SLIME_SEL4_LOAN_CHECK",
    "SLIME_SEL4_SPAWN_CHECK",
    "SLIME_SEL4_SAMPLE_CHECK",
)

# How many `SLIME_SEL4_FABRIC_CHECK` branches each participant is allowed, and
# what each one is for. The exact count, not a ceiling: a component that grew a
# second branch would be tailoring more of itself to this root than the
# milestone admits, and would pass a `<=` check silently.
ALLOWED_FABRIC_BRANCHES: dict[str, tuple[int, str]] = {
    "fabric-service.rs": (
        0,
        "runs unmodified: ROUTE_NAMES is its own constant rather than the "
        "profile's, so `main` provisions and brokers whatever subset of those "
        "routes the generation declares. A route the graph does not name has "
        "no participants and therefore no edges to provision.",
    ),
    "fabric-publisher.rs": (
        0,
        "runs unmodified: its inline path is within MAX_INLINE_BYTES and needs "
        "nothing this graph does not declare.",
    ),
    "fabric-intruder.rs": (
        0,
        "runs unmodified: a denial is a denial on either kernel.",
    ),
    "fabric-subscriber.rs": (
        1,
        "requires *both* sample forms before it will finish, and the "
        "`>MAX_MSG` one comes from `fabric-publisher-b`, which this graph does "
        "not declare. The branch relaxes that one condition and renames the "
        "marker so the transcript cannot claim a shared sample arrived. "
        "P5.5.2 restores it by declaring the second publisher, not by editing "
        "this component.",
    ),
}


def check_components_are_minimally_branched() -> None:
    """Each participant carries exactly the seL4 branching this slice admits.

    P5.3.4's components ran *unmodified* and its gate asserted the absence of
    any branch. P5.5's exit condition does not say "unmodified", and two of
    these four components cannot run on a one-route graph without one -- see
    `ALLOWED_FABRIC_BRANCHES` for which and why.

    So this asserts the exact extent instead of the absence. That keeps the
    difference between this slice and P5.5.2 a checked fact rather than a
    claim: a component that grew a second branch fails here, and so does one
    whose branch was removed without the graph that made it unnecessary.
    """
    for name, (allowed, reason) in ALLOWED_FABRIC_BRANCHES.items():
        source = ROOT / "components" / "bins" / "src" / "bin" / name
        try:
            text = source.read_text(encoding="utf-8")
        except OSError as error:
            fail(f"cannot read {source.relative_to(ROOT)}: {error}")
        for flag in OTHER_SEL4_CHECK_FLAGS:
            if flag in text:
                fail(
                    f"{source.relative_to(ROOT)} branches on {flag}, which "
                    "belongs to another gate's scenario"
                )
        found = len(re.findall(r'option_env!\("SLIME_SEL4_FABRIC_CHECK"\)', text))
        if found != allowed:
            fail(
                f"{source.relative_to(ROOT)} has {found} "
                f"SLIME_SEL4_FABRIC_CHECK branches, but this slice admits "
                f"exactly {allowed}: {reason}"
            )
    unmodified = [
        name for name, (allowed, _) in ALLOWED_FABRIC_BRANCHES.items() if allowed == 0
    ]
    branched = [
        name for name, (allowed, _) in ALLOWED_FABRIC_BRANCHES.items() if allowed != 0
    ]
    print(
        "components: "
        + ", ".join(unmodified)
        + " run as the x86 oracle builds them; "
        + ", ".join(branched)
        + " each carry exactly one documented route-arity branch",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 typed-fabric image and assert ordered markers"
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
        "seL4 fabric plane check: one declared typed route carried a sample from a "
        "publisher to a subscriber over seL4, with both route endpoints provisioned "
        "by the fabric from the generation's declared edges, a re-delegation refused, "
        "and an undeclared participant denied"
    )


if __name__ == "__main__":
    main()
