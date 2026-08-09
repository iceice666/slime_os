#!/usr/bin/env python3

"""P5.4.7/C8.7 gate: bounded native operations on the seL4 operation plane.

The x86 gate (`just fabric_operation_check`) proves the native-operation
semantics against the frozen oracle. This gate boots the `sel4-operation` image
and proves the same broker and the same five participants run **unmodified** on
`slime-root` after `init` mints private authenticated control pairs, spawns the
graph, and vouches for each participant's identity by transferring its
supervision handle to the broker.

What this adds over the x86 gate is the seL4 composition: four parent-vouched
post-spawn introductions plus a fifth for the restart replacement, and a task
lifecycle in which every spawned participant reaches exactly one clean exit. What
it does not do is re-derive C8.7's semantics — the causal chains below are the
oracle's own markers, emitted by the oracle's own code.
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
IMAGE = ROOT / "build" / "slime-sel4-operation.elf"
MANIFEST = ROOT / "build" / "slime-sel4-operation.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-operation.zti"
IMAGE_VARIANT = "operation"
BOOT_TIMEOUT_SECONDS = 240

# Participants run concurrently, so only causal order *within* each chain is part
# of the contract. The first chain is the seL4 composition; the rest are C8.7's
# four required checks, one chain per property the milestone names.
CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "the generation was admitted and the parent introduced every participant",
        (
            r"SLIME_ROOT generation admitted number=20 components=7 grants=13 ",
            r"SLIME_ROOT fabric graph=admitted schemas=1 routes=2 participants=5 ",
            r"SLIME_GRAPH activated components=7",
            r"\[init\] operation control channels minted",
            r"SLIME_GRAPH spawned task=\d+ child=\d+ component=fabric-service ",
            r"\[init\] operation fabric spawned",
            r"SLIME_GRAPH spawned task=\d+ child=\d+ component=fabric-op-client ",
            r"SLIME_GRAPH spawned task=\d+ child=\d+ component=fabric-op-client-b ",
            r"SLIME_GRAPH spawned task=\d+ child=\d+ component=fabric-op-server ",
            r"\[init\] operation participants spawned",
            r"\[init\] operation supervision delegated",
            r"SLIME_GRAPH spawned task=\d+ child=\d+ component=fabric-op-client-b-restart ",
            r"\[init\] operation replacement introduced",
            r"SLIME_GRAPH spawned task=\d+ child=\d+ component=fabric-op-time ",
            r"\[init\] operation replacement released",
            r"\[fabric\] operation roles provisioned",
        ),
    ),
    (
        # C8.7 required check 1, the positive half: one client's goal, feedback,
        # and result correlate end to end and produce exactly one terminal.
        "correlation and ordered feedback",
        (
            r"\[fabric\] operation roles provisioned",
            r"\[fabric-op-client\] success correlated",
            r"\[fabric-op-server\] feedback streamed",
            r"\[fabric-op-client\] feedback ordered",
            r"\[fabric-op-server\] goal rejected",
            r"\[fabric-op-client\] rejection distinct",
        ),
    ),
    (
        # C8.7 required check 1, the negative half: two operations live at once
        # under different authorities never cross-correlate.
        "concurrent operations do not cross-correlate",
        (
            r"\[fabric-op-client-b\] concurrent operation isolated",
        ),
    ),
    (
        # C8.7 required check 3: bounded determinism at the transport edges.
        "terminal state, duplicates, and single-terminal enforcement",
        (
            r"\[fabric-op-server\] post-terminal feedback emitted",
            r"\[fabric-op-client\] terminal state closed",
            r"\[fabric-op-client\] duplicate goal rejected",
            r"\[fabric-op-server\] duplicate result emitted",
            r"\[fabric-op-client\] single terminal enforced",
        ),
    ),
    (
        "retained results are claimable exactly once",
        (
            r"\[fabric-op-client\] result retrieved",
            r"\[fabric-op-client\] retained result claimed once",
        ),
    ),
    (
        # C8.7 required check 2: knowing an identity is not authority over it.
        # All three denials are asserted after client A has produced the exact
        # operation identities client B names.
        "unauthorized observation, retrieval, and cancellation are refused",
        (
            r"\[fabric-op-client\] retained result claimed once",
            r"\[fabric-op-client-b\] unauthorized retrieval denied",
            r"\[fabric-op-client-b\] unauthorized cancel denied",
            r"\[fabric-op-client-b\] forged transport record denied",
        ),
    ),
    (
        # C8.7 required check 3: participant restart is deterministic. The
        # replacement finds the retained result under the authenticated client
        # index and its replayed goal is suppressed.
        "participant restart is deterministic",
        (
            r"\[fabric-op-client-b\] restart state retained",
            r"\[fabric\] operation participant restarted",
            r"\[fabric-op-client-b\] participant restart deterministic",
        ),
    ),
    (
        "cancellation races settle exactly once",
        (
            r"\[fabric-op-server\] awaiting cancellation",
            r"\[fabric-op-server\] cancellation honoured",
            r"\[fabric-op-client-b\] cancellation settled once",
        ),
    ),
    (
        # C8.7 required check 3: expiry is driven by the capability-routed clock
        # rather than by a poll, so timeout and result expiry are orderable.
        "explicit time produces distinct timeout and expiry outcomes",
        (
            r"\[fabric-op-server\] goal left unanswered",
            r"\[fabric-op-time\] bounded time advanced",
            r"\[fabric-op-client\] timeout distinct",
            r"\[fabric-op-client\] result expiry observed",
        ),
    ),
    (
        # C8.7 required check 4: peer death settles the dead server's client's
        # active operation, and client A's own second route still carries.
        "peer death settles the operation and leaves an unrelated route live",
        (
            r"\[fabric-op-server\] injected peer death",
            r"\[fabric-op-client\] peer death distinct",
            r"\[fabric-op-client\] unrelated operation route live",
        ),
    ),
    (
        # The other half of check 4, as its own chain: the two clients observe
        # the same death concurrently, so their relative order is scheduling,
        # not contract. What is causal is that B's *own* active operation on the
        # same route settles too, rather than being left in flight.
        "peer death settles the unrelated client's operation as well",
        (
            r"\[fabric-op-server\] injected peer death",
            r"\[fabric-op-client-b\] concurrent peer fault isolated",
        ),
    ),
    (
        "the broker reclaimed its state and every task exited",
        (
            r"\[fabric\] operation state reclaimed",
            r"\[fabric\] operation plane complete",
            r"\[init\] operation plane complete",
            r"SLIME_GRAPH component exit task=\d+ status=0",
        ),
    ),
)

TERMINAL_MARKER = r"SLIME_GRAPH component exit task=\d+ status=0"

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    r"\[init\] operation plane fail: .*",
    r"\[fabric\] fail: .*",
    r"\[fabric-op\] fail: .*",
    r"goal executed twice",
    r"cross-correlated feedback",
    r"operation role missing one direction",
    r"SLIME_GRAPH spawn (?:failed|unwound|unwind incomplete) .*",
    r"SLIME_GRAPH channel (?:recall|rollback) failed .*",
    r"SLIME_GRAPH capability transfer rolled back .*",
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
EXPECTED_SPAWNED = (
    "fabric-service",
    "fabric-op-client",
    "fabric-op-client-b",
    "fabric-op-server",
    "fabric-op-client-b-restart",
    "fabric-op-time",
)
# Every participant whose identity the parent vouches for: both clients, the
# server, and the restart replacement. The clock needs none — it drives the
# broker's time input and holds no route.
EXPECTED_INTRODUCTIONS = 4


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 operation plane check: {message}")


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
    command = [sys.executable, str(BUILD_SCRIPT), "--operation-plane"]
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
            "run `just sel4_operation_check`"
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
            "rebuild with `--operation-plane`"
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
    init_complete = re.compile(r"\[init\] operation plane complete")
    component_exit = re.compile(TERMINAL_MARKER)
    lines: list[str] = []
    saw_init_complete = False
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
            lines.append(line.rstrip("\n"))
            if failures.search(line):
                break
            if init_complete.search(line):
                saw_init_complete = True
            elif saw_init_complete and component_exit.search(line):
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
    if timed_out and not saw_init_complete:
        report_transcript(transcript)
        fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without reaching the final marker")
    return transcript


def report_transcript(transcript: str) -> None:
    tail = transcript.splitlines()[-60:]
    if tail:
        sys.stdout.write("--- serial transcript (tail) ---\n")
        sys.stdout.write("\n".join(tail) + "\n")
        sys.stdout.write("--- end transcript ---\n")
        sys.stdout.flush()


def check_task_lifecycle(transcript: str) -> None:
    """Every spawned participant reaches exactly one clean exit, and so does init.

    Derived from the root's own spawn records rather than from a component's
    claim: a participant that never ran, ran twice, or exited non-zero is caught
    here even if every scenario marker it was supposed to emit appeared.
    """
    spawns = SPAWN_PATTERN.findall(transcript)
    if tuple(match[2] for match in spawns) != EXPECTED_SPAWNED:
        fail(
            "spawned component sequence was "
            f"{tuple(match[2] for match in spawns)!r}, expected {EXPECTED_SPAWNED!r}"
        )
    parent_ids = {match[0] for match in spawns}
    if len(parent_ids) != 1:
        fail(f"spawn records name multiple parents: {sorted(parent_ids)}")
    children = {match[2]: match[1] for match in spawns}
    exits: dict[str, list[int]] = {}
    for task, status in EXIT_PATTERN.findall(transcript):
        exits.setdefault(task, []).append(int(status))
    for component, task in children.items():
        if exits.get(task) != [0]:
            fail(f"{component} task {task} exit statuses were {exits.get(task, [])}, expected [0]")
    parent = next(iter(parent_ids))
    if exits.get(parent) != [0]:
        fail(f"init task {parent} exit statuses were {exits.get(parent, [])}, expected [0]")

    # The composition's own claim: the parent, not the participant, established
    # every admitted identity. A plane that let a participant vouch for itself
    # would reach every C8.7 marker above and fail here.
    introductions = re.findall(
        rf"SLIME_GRAPH capability transferred task={parent} channel=\d+ "
        r"side=producer kind=supervision rights=0x40000",
        transcript,
    )
    if len(introductions) != EXPECTED_INTRODUCTIONS:
        fail(
            f"parent delivered {len(introductions)} supervision introductions, "
            f"expected {EXPECTED_INTRODUCTIONS}"
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
    # The restart replacement provisions exactly once. A broker that re-admitted
    # it would satisfy the chain above while handing out a second role.
    restarts = transcript.count("[fabric] operation participant restarted")
    if restarts != 1:
        fail(f"the replacement was provisioned {restarts} times, expected 1")
    check_task_lifecycle(transcript)
    print(
        f"transcript: {sum(len(chain) for _, chain in CHAINS)} markers observed "
        f"across {len(CHAINS)} causal chains; six spawned tasks and init exited cleanly",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 operation-plane image and assert C8.7"
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
        "seL4 operation plane check: init minted authenticated control pairs, vouched "
        "for four participant identities, every C8.7 bounded-operation arm ran with the "
        "unmodified broker and participants, and all six spawned tasks exited cleanly"
    )


if __name__ == "__main__":
    main()
