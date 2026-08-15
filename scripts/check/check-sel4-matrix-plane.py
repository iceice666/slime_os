#!/usr/bin/env python3

"""C8.12 gate: the integrated matching, visibility, and denial matrix on seL4.

C8.3-C8.8 each prove one authority property against a graph shaped for it. This
gate boots one generation that exercises all of them at once, with the cases
that only exist when they run together:

* **Alternate names.** `telemetry` and `telemetry-alt` carry the *same*
  `TelemetryStream` interface under different names. Route authority is the fold
  of (name, full interface identity, contract kind), so the two are distinct
  routes and a participant on one holds nothing on the other.
* **Conflicting types.** `telemetry-alt` asked for under the `DiagnosticsStream`
  type tag is a different identity again, so it is a different route rather than
  a badly typed request against a known one.
* **A total denial.** Every refusal — ungranted component, wrong route name,
  wrong type — carries a nonzero status and nothing else: no rights, no
  capability, no route identity. The gate asserts that from both ends, since the
  components check their own replies and fail loudly if one leaks.
* **Visibility is not authority.** The observer's filtered view spans exactly
  its one granted route, and a role request on a route it cannot see is refused.
* **Interposition is the only path.** The declared proxy is the sole telemetry
  path to the subscriber, and holds no participant edge of its own.

It also observes the four C8.11 trace families that had validator arms and
generated codes but no emitter: `schema`, `visibility`, `interposition`, and
`denial`. C8.11 recorded that their natural producers belong here.

The QoS half of C8.12's matching matrix is proven at *admission* rather than at
runtime, and deliberately: `slime-root`'s `fabric_graph_is_satisfiable` refuses
any generation whose graph declares an incompatible offered/requested pair
(`all_pairs_qos_compatible`), so such a generation cannot boot at all. This gate
builds one and asserts it fails closed, which is the stronger property and the
one C8.2's exit condition already claims.
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
IMAGE = ROOT / "build" / "slime-sel4-matrix.elf"
MANIFEST = ROOT / "build" / "slime-sel4-matrix.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-matrix.zti"
IMAGE_VARIANT = "matrix"
BOOT_TIMEOUT_SECONDS = 240
# C8.12's negative arm: the same graph with one incompatible QoS pair, which
# `slime-root` must refuse before any component launches.
UNSATISFIABLE_IMAGE = ROOT / "build" / "slime-sel4-matrix-unsatisfiable.elf"
UNSATISFIABLE_FIXTURE = (
    ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-matrix-unsatisfiable.zti"
)
# The root's own refusal marker for a graph it cannot satisfy.
UNSATISFIABLE_REFUSAL = r"SLIME_ROOT FATAL generation admission rejected: UnsatisfiableFabricGraph"

# Participants run concurrently, so only causal order *within* each chain is
# part of the contract. Anything asserted across two tasks belongs in the
# membership checks below instead, where scheduling cannot reorder it.
CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "the generation was admitted with its declared chain and both telemetry routes",
        (
            r"SLIME_ROOT generation admitted number=34 executables=9 instances=9 grants=29 ",
            # Three routes over two schemas: `telemetry` and `telemetry-alt`
            # share `TelemetryStream`, which is the alternate-name case as the
            # graph states it. `interpositions=1` is the declared chain
            # surviving admission.
            r"SLIME_ROOT fabric graph=admitted schemas=2 routes=3 participants=7 "
            r"interpositions=1",
            r"\[init\] matrix control channels minted",
            r"\[init\] matrix participants spawned",
            r"\[init\] matrix fabric spawned",
        ),
    ),
    (
        # The broker's own startup assertions, in the order it makes them. Each
        # is a property of the resolved graph rather than of any traffic, so all
        # three precede every request.
        "the resolved graph keeps its alternate names and its chain distinct",
        (
            r"\[fabric\] matrix probe holds only its control endpoint",
            r"\[fabric\] matrix admitted 2 interface schemas",
            r"\[fabric\] alternate names hold distinct route identities",
            r"\[fabric\] direct interposition bypass absent by binding",
        ),
    ),
    (
        # C8.12 required check 2, from the publishing side: this component holds
        # an edge on `telemetry-alt` alone. Asking under the other route's name
        # is refused, asking under a conflicting type is refused, and only then
        # does its own tuple match — so a broker that granted before checking
        # could not pass this chain.
        "alternate names and conflicting types never alias in provisioning",
        (
            r"\[fabric-publisher-b\] alternate name denied",
            r"\[fabric-publisher-b\] conflicting type denied",
            r"\[fabric-publisher-b\] matrix alternate route matched",
        ),
    ),
    (
        # C8.12 required check 3: an ungranted caller holding a real control
        # endpoint. Every declared route refused, then the planes it holds no
        # endpoint to — absent by capability, not by policy.
        "an ungranted probe acquires nothing on any plane",
        (
            r"\[fabric-probe\] matrix refused every declared route",
            r"\[fabric-probe\] matrix reached every plane it holds an endpoint to",
            r"\[fabric-probe\] done",
        ),
    ),
    (
        # C8.12 required check 4, second half: read-only visibility never
        # becomes route authority. The view is bounded first, then the role
        # request on a route outside it is refused.
        "a filtered view is not a path to route authority",
        (
            r"\[fabric-observer\] matrix filtered view routes=1",
            r"\[fabric-observer\] matrix view granted no route authority",
        ),
    ),
    (
        # C8.12 required check 4, first half: the sample reaches the
        # subscriber only through the declared proxy. Only the publish-then-
        # relay edge is causal — the publisher prints after its blocking
        # ingress send is drained, and the relay only reads that same sample —
        # so only these two are asserted in order. The proxy's own refusal is
        # an independent task's marker with no protocol relationship to
        # either; it is asserted as unordered membership below
        # (`EXPECTED_DENIED_UNGRANTED`), where scheduling cannot reorder it.
        "the declared proxy is the only telemetry path",
        (
            r"\[fabric-publisher\] matrix sample published",
            r"\[fabric\] matrix relayed telemetry through declared proxy",
        ),
    ),
    (
        # No cross-task ordering to violate: a chain of one marker is
        # membership, not sequence. This is the proxy's own account of its
        # empty participant authority — its broker-side counterpart
        # (`DENIED_UNGRANTED_PATTERN`) is checked as unordered membership for
        # the same reason, and a singleton chain here keeps this marker
        # exercised by the same mutation testing every other chain gets
        # (`check-sel4-gate-controls.py`'s `chains_from_gate`) without
        # claiming an order it does not have.
        "the proxy states its own empty participant authority",
        (r"\[fabric-proxy\] matrix chain hop holds no participant edge",),
    ),
    (
        "the plane closes with every trace family accounted for",
        (
            r"\[fabric\] matrix matching complete",
            r"\[trace\] matrix complete capacity=\d+ records=\d+ dropped=0 rejected=0",
            r"\[fabric\] matrix plane complete",
            r"\[init\] matrix plane complete",
        ),
    ),
)


INIT_COMPLETE = r"\[init\] matrix plane complete"
TERMINAL_MARKER = r"SLIME_GRAPH component exit task=(\d+) status=(-?\d+)"

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    r"\[init\] matrix plane fail: .*",
    r"SLIME_GRAPH spawn (?:failed|unwound|unwind incomplete) .*",
    r"SLIME_GRAPH capability (?:export|import|cancel) (?:failed|refused) .*",
    r"<<seL4\(CPU 0\) \[decodeInvocation",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
    r"unhandled",
)

SPAWN_PATTERN = re.compile(
    r"SLIME_GRAPH spawned task=(\d+) child=(\d+) component=([^ ]+) "
    r"grants=(\d+) endpoints=(\d+) notifications=(\d+) handle=(\d+)"
)
EXIT_PATTERN = re.compile(r"SLIME_GRAPH component exit task=(\d+) status=(-?\d+)")
TRACE_PATTERN = re.compile(
    r"\[trace\] matrix kind=(?P<kind>\w+) order=(?P<order>\w+) now=(?P<now>\d+) "
    r"route=(?P<route>[0-9a-f]{16}) correlation=(?P<correlation>\d+) "
    r"sequence=(?P<sequence>\d+) status=(?P<status>-?\d+) event=(?P<event>\d+) "
    r"high_water=(?P<high_water>\d+)(?P<terminal> terminal)?"
)
SUMMARY_PATTERN = re.compile(
    r"\[trace\] matrix complete capacity=(?P<capacity>\d+) records=(?P<records>\d+) "
    r"dropped=(?P<dropped>\d+) rejected=(?P<rejected>\d+)"
)
COMPONENT_FAILURE = re.compile(r"\[fabric[^\]]*\] fail: .*")

EXPECTED_SPAWNED = (
    "fabric-publisher",
    "fabric-subscriber",
    "fabric-publisher-b",
    "fabric-subscriber-b",
    "fabric-observer",
    "fabric-proxy",
    "fabric-probe",
    "fabric-service",
)

# Every match the broker grants, as an unordered set. Membership rather than
# order because seven tasks ask concurrently: which of two independent callers
# is served first is a scheduling fact, not a property of the matrix.
EXPECTED_MATCHED = {
    "fabric-publisher",
    "fabric-subscriber",
    "fabric-publisher-b",
    "fabric-subscriber-b",
}
# Every refusal, by class and by caller. `fabric-observer` and `fabric-proxy`
# appear as refused callers deliberately: neither is ungranted in general — one
# holds a visibility grant, the other a declared chain — and both must still be
# refused a *participant role*, which is exactly the confusion the matrix exists
# to rule out.
EXPECTED_DENIED_UNGRANTED = {"fabric-probe", "fabric-proxy"}
EXPECTED_DENIED_NAME = {"fabric-publisher-b", "fabric-subscriber-b", "fabric-observer"}
EXPECTED_DENIED_TYPE = {"fabric-publisher-b"}

MATCHED_PATTERN = re.compile(r"\[fabric\] matrix matched exact tuple: ([\w-]+)")
DENIED_UNGRANTED_PATTERN = re.compile(r"\[fabric\] matrix denied ungranted: ([\w-]+)")
DENIED_NAME_PATTERN = re.compile(r"\[fabric\] matrix denied name mismatch: ([\w-]+)")
DENIED_TYPE_PATTERN = re.compile(r"\[fabric\] matrix denied type mismatch: ([\w-]+)")

# The four families C8.11 left without an emitter, plus the resource accounting
# every worker closes with. Required, not merely admitted: while a family is
# only admitted, an emitter that stopped firing produces no line and no error,
# which is exactly how C8.11's six silent defects hid.
# `resource` is required alongside them because the terminal record is one: the
# contract reserves two sink slots for it so a reader can tell a complete trace
# from a truncated one, which makes its absence exactly the silent failure this
# set exists to catch.
REQUIRED_KINDS = {"schema", "visibility", "interposition", "denial", "resource"}
ADMITTED_KINDS = REQUIRED_KINDS | {"route", "qos", "fault"}


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 matrix plane check: {message}")


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
    command = [sys.executable, str(BUILD_SCRIPT), "--matrix-plane"]
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
            "run `just sel4_matrix_check`"
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
            "rebuild with `--matrix-plane`"
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


def boot_image(profile: dict[str, object], image: Path, stop: str) -> str:
    """Boot `image` until `stop` matches, and return the transcript.

    The negative arm's counterpart to [`boot`]: that one reads to a clean exit
    the failing image will never reach, so it would only ever report a timeout —
    which is indistinguishable from a hang and says nothing about *why* the
    generation was refused.
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
        str(image),
    ]
    print(f"[boot] {' '.join(command)}", flush=True)
    terminal = re.compile(stop)
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
    watchdog = threading.Timer(BOOT_TIMEOUT_SECONDS, process.kill)
    watchdog.start()
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\r\n"))
            if terminal.search(line):
                break
    finally:
        watchdog.cancel()
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    return "\n".join(lines)


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
                    fail(f"matrix spawn records named multiple init tasks: {init_task}, {parent}")
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
        fail("init reported matrix completion but no clean exit record followed")
    return transcript


def report_transcript(transcript: str) -> None:
    tail = transcript.splitlines()[-60:]
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


def check_matrix(transcript: str) -> None:
    """Exactly the declared matches and exactly the declared refusals.

    Sets rather than counts, and separated by refusal class. A broker that
    refused everything would satisfy any assertion about denials alone; one that
    granted everything would satisfy any assertion about matches alone. Naming
    both sides, per caller, is what makes the matrix a matrix.
    """
    head = composition(transcript)
    matched = set(MATCHED_PATTERN.findall(head))
    if matched != EXPECTED_MATCHED:
        report_transcript(transcript)
        fail(f"matched callers were {sorted(matched)}, expected {sorted(EXPECTED_MATCHED)}")
    for label, pattern, expected in (
        ("ungranted", DENIED_UNGRANTED_PATTERN, EXPECTED_DENIED_UNGRANTED),
        ("name-mismatch", DENIED_NAME_PATTERN, EXPECTED_DENIED_NAME),
        ("type-mismatch", DENIED_TYPE_PATTERN, EXPECTED_DENIED_TYPE),
    ):
        observed = set(pattern.findall(head))
        if observed != expected:
            report_transcript(transcript)
            fail(
                f"{label} refusals named {sorted(observed)}, expected {sorted(expected)}"
            )
    # No caller may appear as both matched and refused *on the same grounds*.
    # `fabric-publisher-b` legitimately appears in both sets, because its two
    # mismatched asks and its one exact ask are different requests — that
    # coexistence is the alternate-name property, not a contradiction. What
    # must not happen is a component the graph declares nothing for being
    # matched — already excluded by construction: `matched == EXPECTED_MATCHED`
    # was just asserted above, and `EXPECTED_MATCHED` and
    # `EXPECTED_DENIED_UNGRANTED` are disjoint literals, so no further check
    # here could ever fail without the equality above having already caught it.


def check_trace(transcript: str) -> None:
    """The four families C8.11 left without an emitter, all present and valid.

    `rejected` is asserted zero for the reason C8.11 records: every emission
    site discards its `Result`, so a record its own validator refuses produces
    no output and no error. A nonzero count is an emitter defect, and without
    this assertion it would read as an event that never happened.
    """
    head = composition(transcript)
    records = [match.groupdict() for match in TRACE_PATTERN.finditer(head)]
    if not records:
        report_transcript(transcript)
        fail("the matrix worker emitted no trace records")
    kinds = {record["kind"] for record in records}
    missing = REQUIRED_KINDS - kinds
    if missing:
        report_transcript(transcript)
        fail(f"the matrix trace is missing required families {sorted(missing)}")
    undeclared = kinds - ADMITTED_KINDS
    if undeclared:
        report_transcript(transcript)
        fail(f"the matrix trace carries undeclared families {sorted(undeclared)}")

    for record in records:
        # A denial names nothing. Enforced by `valid_trace_record`, and asserted
        # here too because the two checks answer different questions: the
        # validator refuses a malformed record silently, while this says the
        # emitted artifact really withholds what the contract says it must.
        if record["kind"] == "denial":
            if (
                int(record["route"], 16) != 0
                or int(record["correlation"]) != 0
                or int(record["event"]) != 0
                or int(record["status"]) >= 0
            ):
                report_transcript(transcript)
                fail(f"a denial record named something it must withhold: {record}")
        # Visibility and interposition are graph-shaped: an edge, no outcome,
        # and an event naming what was observed or traversed.
        if record["kind"] in ("visibility", "interposition"):
            if (
                int(record["route"], 16) == 0
                or int(record["correlation"]) != 0
                or int(record["event"]) == 0
            ):
                report_transcript(transcript)
                fail(f"a {record['kind']} record is not graph-shaped: {record}")
        # Schema admission is a per-generation fact naming no edge and no
        # outcome.
        if record["kind"] == "schema":
            if (
                int(record["route"], 16) != 0
                or int(record["correlation"]) != 0
                or int(record["status"]) != 0
                or int(record["event"]) != 0
            ):
                report_transcript(transcript)
                fail(f"a schema record named an edge or an outcome: {record}")

    # Exactly one terminal record, and it is the last. The contract reserves
    # sink slots for it precisely so a reader can distinguish a complete trace
    # from a truncated one, so a trace missing it — or carrying one in the
    # middle — is by that definition not a complete artifact.
    terminals = [index for index, record in enumerate(records) if record["terminal"]]
    if len(terminals) != 1:
        report_transcript(transcript)
        fail(f"the matrix trace carries {len(terminals)} terminal records, expected 1")
    if terminals[0] != len(records) - 1:
        report_transcript(transcript)
        fail("the matrix terminal record is not the last record in its trace")

    summary = SUMMARY_PATTERN.search(head)
    if summary is None:
        report_transcript(transcript)
        fail("the matrix worker never closed its trace")
    if int(summary.group("rejected")) != 0:
        report_transcript(transcript)
        fail(
            f"the matrix worker emitted {summary.group('rejected')} record(s) its own "
            "validator refused"
        )
    if int(summary.group("dropped")) != 0:
        report_transcript(transcript)
        fail(f"the matrix trace sink dropped {summary.group('dropped')} record(s)")
    if int(summary.group("records")) != len(records):
        report_transcript(transcript)
        fail(
            f"the sink reports {summary.group('records')} records but the transcript "
            f"carries {len(records)}"
        )
    if int(summary.group("records")) > int(summary.group("capacity")):
        report_transcript(transcript)
        fail("the sink reports more records than its declared capacity")


def check_task_lifecycle(transcript: str) -> None:
    """Every task init spawned reaches exactly one clean exit, and so does init."""
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
        if exits.get(task) != [0]:
            fail(f"{component} task {task} exit statuses were {exits.get(task, [])}, expected [0]")
    if exits.get(init_task) != [0]:
        fail(f"init task {init_task} exit statuses were {exits.get(init_task, [])}, expected [0]")
    reported = COMPONENT_FAILURE.findall(head)
    if reported:
        report_transcript(transcript)
        fail(f"a component failed inside the composition: {reported}")


def check_incompatible_qos_fails_closed(profile: dict[str, object]) -> None:
    """A graph declaring an incompatible offered/requested pair cannot boot.

    C8.12 asks for incompatible endpoint pairs alongside the compatible ones.
    They cannot coexist in one *booting* generation: `slime-root`'s
    `fabric_graph_is_satisfiable` refuses any graph whose
    `all_pairs_qos_compatible` is false, before a single component launches.
    That refusal is the stronger property and the one C8.2's exit condition
    already claims, so the incompatible half is proven where it actually lives —
    at admission.

    Proven by booting it, not by asking the builder. The builder emits this
    generation quite happily: pairwise QoS is not a shape property, and the
    resolver validates shape. The refusal belongs to the root, so only a boot
    can observe it — and observing it is what distinguishes "the root refuses
    this" from "nothing ever built it".

    `sel4-matrix-unsatisfiable.zti` is `sel4-matrix.zti` with one
    `telemetry-alt` publisher weakened from RELIABLE to BEST_EFFORT, leaving its
    RELIABLE subscriber promised delivery its writer never offers. So what is
    refused is this plane's own graph rather than a hand-written stand-in.
    """
    command = [sys.executable, str(BUILD_SCRIPT), "--matrix-unsatisfiable-plane"]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot build the incompatible-QoS image: {error}")
    if process.returncode != 0:
        fail(
            f"the seL4 image build failed with exit status {process.returncode} while "
            "building the incompatible-QoS generation; it must emit that generation for "
            "the boot below to observe the root's own refusal"
        )
    transcript = boot_image(profile, UNSATISFIABLE_IMAGE, UNSATISFIABLE_REFUSAL)
    if re.search(UNSATISFIABLE_REFUSAL, transcript) is None:
        report_transcript(transcript)
        fail(
            "a generation promising a reader more than its writer offers was not "
            f"refused; expected {UNSATISFIABLE_REFUSAL!r}"
        )
    # It must fail *before* any component runs. A root that admitted the graph
    # and only later noticed would have launched components under a promise it
    # could not keep, which is the thing failing closed exists to prevent.
    refusal = re.search(UNSATISFIABLE_REFUSAL, transcript)
    assert refusal is not None
    if re.search(r"SLIME_GRAPH spawned task=", transcript[: refusal.start()]) is not None:
        report_transcript(transcript)
        fail("the incompatible graph was refused only after a component had launched")
    print(
        "admission: a graph promising a reader more than its writer offers is "
        "refused before any component launches",
        flush=True,
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
    check_matrix(transcript)
    check_trace(transcript)
    check_task_lifecycle(transcript)
    print(
        f"transcript: {sum(len(chain) for _, chain in CHAINS)} markers observed across "
        f"{len(CHAINS)} causal chains; {len(EXPECTED_MATCHED)} exact tuples matched and "
        f"{len(EXPECTED_DENIED_UNGRANTED | EXPECTED_DENIED_NAME | EXPECTED_DENIED_TYPE)} "
        "callers refused across three classes; eight spawned tasks exited cleanly",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 matrix-plane image and assert C8.12"
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
    if not UNSATISFIABLE_FIXTURE.is_file():
        fail(f"missing negative fixture {UNSATISFIABLE_FIXTURE.relative_to(ROOT)}")
    pins = load_pins()
    if not arguments.no_build:
        build_image()
    check_manifest()
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    check_transcript(boot(profile))
    check_incompatible_qos_fails_closed(profile)
    print(
        "seL4 matrix plane check: only the exact compatible tuple matched, alternate "
        "names and conflicting types stayed distinct, every unauthorized operation "
        "returned a graph-independent denial, a filtered view yielded no route "
        "authority, the declared proxy was the only telemetry path, and an "
        "incompatible QoS pair fails closed at admission"
    )


if __name__ == "__main__":
    main()
