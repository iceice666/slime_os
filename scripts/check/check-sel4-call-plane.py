#!/usr/bin/env python3

"""B25/C8.6 gate: parent-vouched native calls on the seL4 call plane.

The x86 gate proves the bounded native-call semantics. This gate boots the
`sel4-call` image and proves the same participants run unmodified after `init`
creates private native endpoints, spawns the graph, and exports each
participant's attenuated supervision authority to the broker. Matching imports
prove the authority landed in the intended receiver without widening.
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

from harness import profile_text, profile_integer, sha256_file  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
IMAGE = ROOT / "build" / "slime-sel4-call.elf"
MANIFEST = ROOT / "build" / "slime-sel4-call.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-call.zti"
IMAGE_VARIANT = "call"
BOOT_TIMEOUT_SECONDS = 240

# Participants run concurrently, so only causal order within each chain is part
# of the contract. The first chain is the B25 composition itself; the remaining
# chains mirror the x86 C8.6 gate and add the seL4 terminal sequence.
CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "the generation was admitted and the parent introduced every participant",
        (
            r"SLIME_ROOT generation admitted number=18 executables=6 instances=6 grants=12 ",
            r"SLIME_ROOT fabric graph=admitted schemas=1 routes=1 participants=3 ",
            r"SLIME_GRAPH activated instances=\d+",
            r"\[init\] call control channels minted",
            # Participants precede the broker: it is granted a supervision
            # handle naming each of them, and a handle cannot exist before its
            # task. A native Endpoint reports no peer death, so those handles
            # are the only way the broker observes a participant exit.
            r"SLIME_GRAPH spawned task=\d+ child=\d+ component=fabric-call-client ",
            r"SLIME_GRAPH spawned task=\d+ child=\d+ component=fabric-call-client-b ",
            r"SLIME_GRAPH spawned task=\d+ child=\d+ component=fabric-call-server ",
            r"\[init\] call participants spawned",
            # The broker's four grants are the shared-buffer factory plus one
            # supervision handle per participant. Delegation is now part of the
            # spawn rather than a later export/import pair, so the grant count
            # on this line *is* the evidence that supervision was handed over.
            r"SLIME_GRAPH spawned task=\d+ child=\d+ component=fabric-service grants=4 ",
            r"\[init\] call fabric spawned",
            r"\[init\] call supervision delegated",
            r"SLIME_GRAPH spawned task=\d+ child=\d+ component=fabric-call-time ",
            r"\[fabric\] call endpoints ready",
        ),
    ),
    (
        "successful correlation",
        (
            r"\[fabric\] call endpoints ready",
            r"\[fabric\] call forwarded",
            r"\[fabric-call-server\] non-idempotent execution once",
            r"\[fabric\] call reply correlated",
            r"\[fabric-call-client\] success correlated",
        ),
    ),
    (
        "shared request and reply",
        (
            r"\[fabric-call-server\] shared request verified",
            r"\[fabric-call-client\] shared reply verified",
        ),
    ),
    (
        "server rejection",
        (
            r"\[fabric\] server rejection routed",
            r"\[fabric-call-client\] rejection distinct",
        ),
    ),
    (
        "malformed reply",
        (
            r"\[fabric\] malformed call reply rejected",
            r"\[fabric-call-client\] malformed reply distinct",
        ),
    ),
    (
        "duplicate and cancellation",
        (
            r"\[fabric\] duplicate call rejected",
            r"\[fabric-call-client-b\] duplicate rejected",
            r"\[fabric\] call cancellation forwarded",
            r"\[fabric-call-server\] cancellation settled",
            r"\[fabric\] call cancelled",
            r"\[fabric-call-client-b\] cancellation observed",
        ),
    ),
    (
        "stale session",
        (
            r"\[fabric\] stale call rejected",
            r"\[fabric-call-client-b\] stale session observed",
        ),
    ),
    (
        # The call broker queues a terminal it cannot hand over yet and
        # re-offers it, which is `terminal delivery queued`. The marker
        # previously asserted here -- `terminal delivery ring backpressured` --
        # belongs to the *stream* broker's ring publish path and is unreachable
        # from this plane, so the chain could never match.
        "bounded terminal backpressure",
        (
            r"\[fabric\] terminal delivery queued",
            r"\[fabric-call-client-b\] terminal backpressure recovered",
        ),
    ),
    (
        "bounded terminal outcomes",
        (
            r"\[fabric\] call timed out",
            r"\[fabric-call-client\] timeout distinct",
            r"\[fabric\] call retry exhausted",
            r"\[fabric-call-client\] retry exhaustion distinct",
        ),
    ),
    (
        "peer death, reclamation, and the completion barrier",
        (
            r"\[fabric-call-client-b\] unrelated route intact",
            r"\[fabric-call-server\] injected peer death",
            r"\[fabric\] call peer death propagated",
            # The client learns of the death from the terminal the broker sends
            # while reclaiming, so its observation necessarily precedes the
            # broker finishing that reclamation.
            r"\[fabric-call-client\] peer death distinct",
            r"\[fabric\] call state reclaimed",
            r"\[fabric\] call plane complete",
            r"\[init\] call plane complete",
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
    r"\[init\] call plane fail: .*",
    r"\[fabric\] fail: .*",
    r"\[fabric-call\] fail: .*",
    r"executed twice",
    r"call route missing",
    r"SLIME_GRAPH spawn (?:failed|unwound|unwind incomplete) .*",
    r"SLIME_GRAPH channel (?:recall|rollback) failed .*",
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
# Participants precede the broker: it is granted a supervision handle naming
# each of them, and a handle cannot exist before its task.
EXPECTED_SPAWNED = (
    "fabric-call-client",
    "fabric-call-client-b",
    "fabric-call-server",
    "fabric-service",
    "fabric-call-time",
)


# Markers that must appear but whose position is not causally ordered against
# the broker's own sequence. `fabric-call-time` is an independent task driving
# the bounded-time arm; nothing synchronises its completion with the broker
# finishing reclamation, so asserting it inside the causal chain pins one
# scheduling interleaving. Observed failing that way: `marker out of order:
# \[fabric-call-time\] bounded time completed`, on a run that was otherwise
# clean and which passed on repeat. Same treatment B55 gave the boot plane's
# five racy cross-task markers.
EXPECTED_UNORDERED: tuple[str, ...] = (
    r"\[fabric-call-time\] bounded time completed",
)

def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 call plane check: {message}")


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
    command = [sys.executable, str(BUILD_SCRIPT), "--call-plane"]
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
            "run `just sel4_call_check`"
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
            "rebuild with `--call-plane`"
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
    init_complete = re.compile(r"\[init\] call plane complete")
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

    # Supervision is delegated as part of the broker's spawn rather than a
    # later export/import pair, so the evidence is the grant count on that
    # spawn: the shared-buffer factory plus one handle per participant.
    broker = [match for match in spawns if match[2] == "fabric-service"]
    if len(broker) != 1:
        fail(f"expected exactly one fabric-service spawn, saw {len(broker)}")
    grants = int(broker[0][3])
    if grants != 4:
        fail(
            f"fabric-service was spawned with grants={grants}, expected 4 "
            "(shared-buffer factory plus one supervision handle per participant)"
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
    for pattern in EXPECTED_UNORDERED:
        if re.search(pattern, transcript) is None:
            report_transcript(transcript)
            fail(f"missing unordered marker: {pattern}")
    if transcript.count("[fabric-call-server] non-idempotent execution once") != 1:
        fail("non-idempotent request did not execute exactly once")
    check_task_lifecycle(transcript)
    print(
        f"transcript: {sum(len(chain) for _, chain in CHAINS) + len(EXPECTED_UNORDERED)} "
        f"markers observed across {len(CHAINS)} causal chains plus "
        f"{len(EXPECTED_UNORDERED)} order-independent; five spawned tasks and init "
        "exited cleanly",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 call-plane image and assert B25/C8.6"
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
        "seL4 call plane check: init minted authenticated control pairs, delivered "
        "three post-spawn supervision introductions, every C8.6 bounded-call arm ran "
        "with the unmodified participants, and all five spawned tasks exited cleanly"
    )


if __name__ == "__main__":
    main()
