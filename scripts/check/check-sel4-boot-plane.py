#!/usr/bin/env python3

"""P5.4.9/C8.10 gate: the full C8 graph in one seL4 generation.

The x86 gate (``just data_fabric_boot_check``) proves that every C8 role can
coexist in one boot. This gate boots the ``sel4-boot`` image and proves the same
of ``slime-root``: one generation, all three planes at once, in disjoint slots
with no profile-dependent rewrite, every participant reaching a checked role or
a declared role-less idle, and the supervisor certifying the complete graph.

Fabric idle is required evidence, but it is not terminal: independently
scheduled tasks may still exit after the fabric parks. Capture continues until
the root supervisor emits its unique healthy record, and any preceding nonzero
component exit poisons the transcript.
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
IMAGE = ROOT / "build" / "slime-sel4-boot.elf"
MANIFEST = ROOT / "build" / "slime-sel4-boot.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-boot.zti"
IMAGE_VARIANT = "boot"
BOOT_TIMEOUT_SECONDS = 300

CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        # C8.10's declarative half: one generation carrying every C8 role, with
        # all five routes and the declared interposition admitted together.
        "one generation admitted every C8 role and route",
        (
            # Twenty instances: `init`, the fabric, its two route workers, and
            # the sixteen participants. Each is declared so the generation can
            # state the capabilities its owner hands it at spawn; only `init`
            # is root-autostart, which the staging line below asserts.
            r"SLIME_ROOT generation admitted number=22 executables=20 instances=20 grants=39 ",
            r"SLIME_ROOT fabric graph=admitted schemas=4 routes=5 participants=15 "
            r"interpositions=1",
            r"SLIME_GRAPH staged task=\d+ instance=init executable=init grants=\d+ bindings=\d+ ",
            r"SLIME_GRAPH staged instances=1 root_autostart=1 ",
            r"SLIME_GRAPH activated instances=1",
            r"\[init\] fabric boot control channels minted",
        ),
    ),
    (
        # The composition, in the order the graph forces: subscribers first
        # because the fabric is granted their supervision handles, then the
        # fabric with the two worker executables it spawns itself.
        "init launched the graph in dependency order",
        (
            r"\[init\] fabric boot subscribers spawned",
            r"SLIME_GRAPH spawned task=\d+ child=\d+ component=fabric-service ",
            r"\[init\] fabric boot service spawned",
            r"\[init\] fabric boot participants spawned",
            r"\[init\] fabric boot supervision transferred",
            r"\[init\] fabric boot graph launched",
        ),
    ),
    (
        # C8.10's bounded route workers: the fabric splits itself into three
        # tasks because no single wait set could hold every live source.
        "the fabric split into three bounded route workers",
        (
            r"SLIME_GRAPH spawned task=\d+ child=\d+ component=fabric-call-worker ",
            r"\[fabric\] route worker provisioned: call",
            r"SLIME_GRAPH spawned task=\d+ child=\d+ component=fabric-op-worker ",
            r"\[fabric\] route worker provisioned: operation",
            r"\[fabric\] bounded route workers spawned",
        ),
    ),
    (
        "every stream role was provisioned and checked by its holder",
        (
            r"\[fabric\] provisioned fabric-publisher telemetry publish",
            r"\[fabric-publisher\] boot role provisioned",
        ),
    ),
    (
        "the second publisher holds both stream routes",
        (
            r"\[fabric\] provisioned fabric-publisher-b telemetry publish",
            r"\[fabric\] provisioned fabric-publisher-b diagnostics publish",
            r"\[fabric-publisher-b\] boot role provisioned",
        ),
    ),
    (
        # Each subscriber's own provisioning is causal; the order *between*
        # subscribers is not — the broker answers whichever control is ready,
        # and subscriber-b takes both its routes in one round.
        "the telemetry subscriber took its role",
        (
            r"\[fabric\] provisioned fabric-subscriber telemetry subscribe",
            r"\[fabric-subscriber\] boot role provisioned",
        ),
    ),
    (
        "the second subscriber took both of its routes",
        (
            r"\[fabric\] provisioned fabric-subscriber-b telemetry subscribe",
            r"\[fabric\] provisioned fabric-subscriber-b diagnostics subscribe",
            r"\[fabric-subscriber-b\] boot role provisioned",
        ),
    ),
    (
        "the filtered-introspection client took its role",
        (
            r"\[fabric\] provisioned fabric-observer telemetry subscribe",
            r"\[fabric-observer\] boot role provisioned",
        ),
    ),
    (
        # C8.10 required check 3: the probe is its own task, and holding a real
        # control endpoint buys it nothing.
        "the unauthorized probe is a distinct task and is refused",
        (
            r"\[fabric-probe\] exact route strings supplied",
            r"\[fabric\] ungranted component denied: fabric-probe",
            r"\[fabric-probe\] undeclared edge denied",
            r"\[fabric-probe\] done",
        ),
    ),
    # Each participant's own chain: the worker provisions every role in one
    # round and the three then run concurrently, so which of them prints first
    # is scheduling. What is causal is that each took its role *after* its
    # worker announced the round.
    (
        "the call client took its role",
        (
            r"\[fabric\] call roles provisioned",
            r"\[fabric-call-client\] boot role provisioned",
        ),
    ),
    (
        "the second call client took its role",
        (
            r"\[fabric\] call roles provisioned",
            r"\[fabric-call-client-b\] boot role provisioned",
        ),
    ),
    (
        "the call server took its role",
        (
            r"\[fabric\] call roles provisioned",
            r"\[fabric-call-server\] boot role provisioned",
        ),
    ),
    (
        "the operation client took its role",
        (
            r"\[fabric\] operation roles provisioned",
            r"\[fabric-op-client\] boot role provisioned",
        ),
    ),
    (
        "the second operation client took its role",
        (
            r"\[fabric\] operation roles provisioned",
            r"\[fabric-op-client-b\] boot role provisioned",
        ),
    ),
    (
        "the operation server took its role",
        (
            r"\[fabric\] operation roles provisioned",
            r"\[fabric-op-server\] boot role provisioned",
        ),
    ),
    (
        # Fabric idle is required causal evidence that every declared edge was
        # minted and the worker has nothing left to answer. It is deliberately
        # not terminal: the supervisor must still observe the whole graph.
        "the stream worker came to rest",
        (
            r"\[fabric\] idle: parked on control endpoints",
        ),
    ),
)

# The supervisor record is the sole terminal. The instance digest is deliberately
# shape-checked rather than pinned: it changes when the generation changes.
TERMINAL_MARKER = (
    r"SLIME_GRAPH healthy generation=\d+ instances=[0-9a-f]+ "
    r"required=1 live=1 idle=1 failed=0"
)
CHAINS = CHAINS + (("the supervisor certified the complete graph", (TERMINAL_MARKER,)),)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH component exit .*status=-?[1-9]\d*",
    r"SLIME_GRAPH wedged waiter",
    r"\[init\] fabric boot fail: .*",
    r"SLIME_GRAPH spawn (?:failed|refused|unwound|unwind incomplete) .*",
    r"SLIME_GRAPH channel (?:recall|rollback) failed .*",
    r"SLIME_GRAPH capability transfer rolled back .*",
    r"SLIME_GRAPH debug write refused .*",
    r"SLIME_GRAPH channel unplaced .*",
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
    r"grants=(\d+) channels=(\d+) handle=(\d+)"
)
EXIT_PATTERN = re.compile(r"SLIME_GRAPH component exit task=(\d+) status=(-?\d+)")
STAGE_PATTERN = re.compile(
    r"SLIME_GRAPH staged task=(\d+) instance=([^ ]+) executable=([^ ]+) "
    r"grants=(\d+) bindings=(\d+) "
)
LAYOUT_HEADER = re.compile(r"\[layout\] path=init slots=(\d+) max=(\d+)")

# The sixteen participants init spawns, in boot-layout order, plus the fabric.
EXPECTED_INIT_CHILDREN = (
    "fabric-subscriber",
    "fabric-subscriber-b",
    "fabric-observer",
    "fabric-service",
    "fabric-publisher",
    "fabric-publisher-b",
    "fabric-probe",
    "fabric-proxy",
    "fabric-call-client",
    "fabric-call-client-b",
    "fabric-call-server",
    "fabric-call-time",
    "fabric-op-client",
    "fabric-op-client-b",
    "fabric-op-server",
    "fabric-op-time",
    "fabric-op-client-b-restart",
)
# The two the fabric spawns itself. C8.10's "bounded route workers" half: the
# component that binds a worker's control endpoints is the component that
# created it.
EXPECTED_WORKERS = ("fabric-call-worker", "fabric-op-worker")
# Roles that take a checked route capability, and roles the graph declares but
# gives no work: the two clocks, the interposition proxy, and the operation
# replacement. Each parks holding only its control endpoint.
EXPECTED_ROLES = 11
EXPECTED_IDLE_WITHOUT_ROLE = (
    "fabric-call-time",
    "fabric-op-time",
    "fabric-proxy",
    "fabric-op-client-b-restart",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 boot plane check: {message}")


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
    command = [sys.executable, str(BUILD_SCRIPT), "--boot-plane"]
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
            "run `just sel4_boot_check`"
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
            "rebuild with `--boot-plane`"
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
    """Boot until the supervisor certifies the graph, or a failure appears."""
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
    terminal = re.compile(TERMINAL_MARKER)
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
    outcome = "eof"
    failure: str | None = None
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\r\n"))
            matched_failure = failures.search(line)
            if matched_failure is not None:
                outcome = "failure"
                failure = matched_failure.group(0)
                break
            if terminal.search(line):
                outcome = "terminal"
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
    if outcome == "failure":
        report_transcript(transcript)
        fail(f"boot stopped at failure marker: {failure!r}")
    if outcome != "terminal":
        report_transcript(transcript)
        if timed_out:
            fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without supervisor certification")
        fail("QEMU exited before the supervisor certified the graph healthy")
    return transcript


def report_transcript(transcript: str) -> None:
    tail = transcript.splitlines()[-60:]
    if tail:
        sys.stdout.write("--- serial transcript (tail) ---\n")
        sys.stdout.write("\n".join(tail) + "\n")
        sys.stdout.write("--- end transcript ---\n")
        sys.stdout.flush()


def check_layout(transcript: str) -> int:
    """Init's table is one collision-free layout, strictly under the ceiling."""
    headers = LAYOUT_HEADER.findall(transcript)
    if len(headers) != 1:
        fail(f"expected exactly one init layout report, saw {len(headers)}")
    used, ceiling = (int(value) for value in headers[0])
    if used >= ceiling:
        fail(f"init's layout uses {used} of {ceiling} slots; it must stay under the ceiling")
    slots = [int(slot) for slot in re.findall(r"\[layout\] (\d+) ", transcript)]
    if len(slots) != used:
        fail(f"the layout reports {used} slots but lists {len(slots)}")
    if len(set(slots)) != len(slots):
        fail("the layout claims a slot twice; the planes are not disjoint")
    return used


def check_root_instances(transcript: str) -> None:
    """Exactly the declared root-owned autostart instance is launched once."""
    stages = STAGE_PATTERN.findall(transcript)
    if len(stages) != 1:
        fail(f"expected exactly one staged root instance, saw {len(stages)}")
    _task, instance, executable, _grants, _bindings = stages[0]
    if instance != "init" or executable != "init":
        fail(
            f"root staged instance={instance!r} executable={executable!r}, expected init/init"
        )
    if len(re.findall(TERMINAL_MARKER, transcript)) != 1:
        fail("expected exactly one healthy supervisor terminal")


def check_composition(transcript: str) -> None:
    """Every declared role is a distinct live task, and none of them exited."""
    spawns = SPAWN_PATTERN.findall(transcript)
    parents = {match[0] for match in spawns}
    if len(parents) != 2:
        fail(f"expected two spawning parents (init and the fabric), saw {sorted(parents)}")
    by_parent: dict[str, list[str]] = {}
    children: dict[str, str] = {}
    spawn_tasks: set[str] = set()
    child_tasks: set[str] = set()
    for parent, child, component, *_ in spawns:
        by_parent.setdefault(parent, []).append(component)
        if child in child_tasks:
            fail(f"child identity {child} was reused by multiple spawns")
        if component in children:
            fail(f"{component} was spawned twice; every executable identity is one task")
        spawn_tasks.add(parent)
        child_tasks.add(child)
        children[component] = child
    if spawn_tasks & child_tasks != {next(p for p in parents if p != max(parents, key=lambda p: len(by_parent[p])))}:
        fail("spawn task identities do not form exactly the init/fabric composition tree")
    init = max(parents, key=lambda p: len(by_parent[p]))
    fabric = next(p for p in parents if p != init)
    if tuple(by_parent[init]) != EXPECTED_INIT_CHILDREN:
        fail(f"init spawned {tuple(by_parent[init])!r}, expected {EXPECTED_INIT_CHILDREN!r}")
    if tuple(by_parent[fabric]) != EXPECTED_WORKERS:
        fail(f"the fabric spawned {tuple(by_parent[fabric])!r}, expected {EXPECTED_WORKERS!r}")
    exited = {
        component: task
        for component, task in children.items()
        for exit_task, _ in EXIT_PATTERN.findall(transcript)
        if exit_task == task
    }
    if exited:
        report_transcript(transcript)
        fail(f"composition tasks exited before the graph came to rest: {sorted(exited)}")
    roles = len(re.findall(r"\[fabric[^\]]*\] boot role provisioned", transcript))
    if roles != EXPECTED_ROLES:
        fail(f"{roles} participants took a checked role, expected {EXPECTED_ROLES}")
    for component in EXPECTED_IDLE_WITHOUT_ROLE:
        if f"[{component}] boot idle without a role" not in transcript:
            fail(f"{component} did not report its declared role-less idle")


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
    check_root_instances(transcript)
    used = check_layout(transcript)
    check_composition(transcript)
    print(
        f"transcript: {sum(len(chain) for _, chain in CHAINS)} markers observed across "
        f"{len(CHAINS)} causal chains; init's layout used {used} slots; one root instance "
        f"and 19 composition tasks reached {EXPECTED_ROLES} checked roles plus "
        f"{len(EXPECTED_IDLE_WITHOUT_ROLE)} declared idles, and none exited",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 full-graph image and assert C8.10"
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
        "seL4 boot plane check: one generation launched every C8 role at once through "
        "a collision-free layout, the fabric split into three bounded route workers, "
        "the unauthorized probe was refused as a distinct task, and the whole graph "
        "came to rest without any participant exiting"
    )


if __name__ == "__main__":
    main()
