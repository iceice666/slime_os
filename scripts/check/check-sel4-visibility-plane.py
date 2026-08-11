#!/usr/bin/env python3

"""P5.4.8/C8.8 gate: filtered introspection and declared interposition on seL4.

The x86 gate (`just fabric_visibility_check`) proves the C8.8 semantics against
the frozen oracle. This gate boots the `sel4-visibility` image and proves the
same broker and the same five participants run **unmodified** on `slime-root`
after `init` mints authenticated control pairs and hands each participant exactly
one capability: its own control endpoint.

That last fact is what makes the authority claims meaningful here. Every route
half a component ends up with was minted by the broker and narrowed at transfer,
so "the proxy relays only its declared route" is a statement about provisioning
rather than about what the parent happened to hand out.

Beyond the causal chains, this gate re-derives the oracle's two structural
assertions from the transcript: the composition emits exactly twelve serialized
view records, and exactly two interposition traces that differ from each other.
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
IMAGE = ROOT / "build" / "slime-sel4-visibility.elf"
MANIFEST = ROOT / "build" / "slime-sel4-visibility.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-visibility.zti"
IMAGE_VARIANT = "visibility"
BOOT_TIMEOUT_SECONDS = 240

# Participants run concurrently, so only causal order within each chain is part
# of the contract.
CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "the generation was admitted with its declared interposition chain",
        (
            r"SLIME_ROOT generation admitted number=21 executables=7 instances=7 grants=13 ",
            # `interpositions=1` is the declared chain surviving admission. A
            # profile whose chain was dropped would admit a graph with a direct
            # edge where the generation declared a proxy hop.
            r"SLIME_ROOT fabric graph=admitted schemas=2 routes=2 participants=6 "
            r"interpositions=1",
            r"SLIME_GRAPH activated instances=\d+",
            r"\[init\] visibility control channels minted",
            r"SLIME_GRAPH spawned task=\d+ child=\d+ component=fabric-service ",
            r"\[init\] visibility fabric spawned",
            r"\[init\] visibility participants spawned",
        ),
    ),
    (
        # C8.8 required check 1: two callers with different grants receive
        # different bounded views. The publisher holds graph visibility on both
        # routes; the subscriber holds private visibility on one.
        "different visibility grants yield different bounded views",
        (
            r"\[fabric-publisher\] graph view routes=2",
            r"\[fabric-subscriber\] private view routes=1",
        ),
    ),
    (
        # The other half of check 1: the proxy holds no visibility grant, and
        # cannot infer the protected route through counts, names, types, match
        # events, or error detail. Its very first record is the terminal one.
        "an ungranted caller infers nothing",
        (
            r"\[fabric-intruder\] ungranted view is byte-empty",
            r"\[fabric\] filtered graph views complete",
        ),
    ),
    (
        # C8.8 required check 3, first half: the broker holds the upstream half
        # of the proxy's downstream edge and proves it cannot deliver directly.
        # Asserted before any relay, so a bypass could not be masked by a
        # successful chain.
        "the declared proxy cannot be bypassed",
        (
            r"\[fabric\] direct interposition bypass absent",
            r"\[fabric-intruder\] proxy authority narrowed to chain",
        ),
    ),
    (
        # C8.8 required check 2: the relay traverses the declared chain, and the
        # sample reaches the subscriber only through the proxy.
        "telemetry reaches the subscriber through the declared chain",
        (
            r"\[fabric-publisher\] interposed sample published",
            r"\[fabric-subscriber\] sample arrived through proxy",
            r"\[fabric-intruder\] declared relay complete; exiting",
            r"\[fabric\] declared proxy relayed telemetry",
        ),
    ),
    (
        # C8.8 required check 3, second half: proxy death is a route-scoped
        # event the subscriber both receives and subsequently sees in its
        # filtered view — the projection, not merely the notification.
        "proxy death is a route event, not a fabric failure",
        (
            r"\[fabric\] proxy death isolated to telemetry",
            r"\[fabric-subscriber\] proxy loss route event observed",
            r"\[fabric-subscriber\] proxy loss visible in graph view",
        ),
    ),
    (
        # The isolation claim, from both ends: the unrelated diagnostics route
        # carries a sample after the telemetry proxy is gone.
        "an unrelated route stays live through proxy death",
        (
            r"\[fabric-publisher-b\] unrelated diagnostics published",
            r"\[fabric-subscriber-b\] unrelated diagnostics live after proxy death",
            r"\[fabric\] unrelated diagnostics route live after proxy death",
            r"\[fabric\] visibility plane complete",
            r"\[init\] visibility plane complete",
        ),
    ),
)

INIT_COMPLETE = r"\[init\] visibility plane complete"
# Grouped: `boot` matches init's own exit against the task id the spawn
# records named, so a *participant* exiting cannot end the capture early.
TERMINAL_MARKER = r"SLIME_GRAPH component exit task=(\d+) status=(-?\d+)"

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    r"\[init\] visibility plane fail: .*",
    r"SLIME_GRAPH spawn (?:failed|unwound|unwind incomplete) .*",
    r"SLIME_GRAPH channel (?:recall|rollback) failed .*",
    r"SLIME_GRAPH capability transfer rolled back .*",
    r"SLIME_GRAPH debug write refused .*",
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
VIEW_PATTERN = re.compile(r"\[fabric-view\] ([0-9a-f]+)")
TRACE_PATTERN = re.compile(r"\[fabric-trace\] ([0-9a-f]+)")
EXPECTED_SPAWNED = (
    "fabric-service",
    "fabric-publisher",
    "fabric-subscriber",
    "fabric-intruder",
    "fabric-publisher-b",
    "fabric-subscriber-b",
)
# The oracle's own figures. Twelve serialized view records is what this graph's
# three paging callers produce against their exact grants; two traces is one
# relay plus one loss.
EXPECTED_VIEW_RECORDS = 12
EXPECTED_TRACE_RECORDS = 2
# Every component name appears twice per boot: once as the unconfigured instance
# the root launches from the generation (P5.2), and once as the instance init
# spawns. The unconfigured one holds no control endpoint and fails its first
# operation. `fabric-service` logs as `[fabric]`.
#
# Unlike the stream plane, this gate does not budget one failure per component.
# It asserts a stronger and scheduling-independent property instead: **zero**
# component failures inside the composition window, which ends at init's own
# clean exit. The unconfigured instances here are slower than the composition —
# they fail after init has already collected every child — so a per-component
# budget would depend on how far QEMU ran past the last marker. Requiring the
# window itself to be failure-free cannot be satisfied by a real participant
# failing, whenever the unconfigured ones happen to get scheduled.
COMPONENT_FAILURE = re.compile(r"\[fabric[^\]]*\] fail: .*")


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 visibility plane check: {message}")


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
    command = [sys.executable, str(BUILD_SCRIPT), "--visibility-plane"]
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
            "run `just sel4_visibility_check`"
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
            "rebuild with `--visibility-plane`"
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
                    fail(f"visibility spawn records named multiple init tasks: {init_task}, {parent}")
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
        fail("init reported visibility completion but no clean exit record followed")
    return transcript


def report_transcript(transcript: str) -> None:
    tail = transcript.splitlines()[-60:]
    if tail:
        sys.stdout.write("--- serial transcript (tail) ---\n")
        sys.stdout.write("\n".join(tail) + "\n")
        sys.stdout.write("--- end transcript ---\n")
        sys.stdout.flush()


def composition(transcript: str) -> str:
    """The composition through init's clean exit.

    The terminal reader stops on init's own exit record, so the slice retains
    lifecycle evidence while excluding later failures from unconfigured copies.
    """
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


def check_records(transcript: str) -> None:
    """The oracle's two structural assertions, re-derived here.

    Exactly twelve view records is what this graph's three paging callers produce
    against their exact grants; a broker that leaked one route into one caller's
    view, or dropped one, moves this number. Exactly two *distinct* traces is one
    relay plus one loss: identical traces would mean the loss trace carried the
    relay's event, and a third would mean a route emitted one it should not.
    """
    head = composition(transcript)
    views = VIEW_PATTERN.findall(head)
    if len(views) != EXPECTED_VIEW_RECORDS:
        report_transcript(transcript)
        fail(
            f"the composition emitted {len(views)} view records, "
            f"expected {EXPECTED_VIEW_RECORDS}"
        )
    traces = TRACE_PATTERN.findall(head)
    if len(traces) != EXPECTED_TRACE_RECORDS:
        report_transcript(transcript)
        fail(
            f"the composition emitted {len(traces)} interposition traces, "
            f"expected {EXPECTED_TRACE_RECORDS}"
        )
    if len(set(traces)) != EXPECTED_TRACE_RECORDS:
        report_transcript(transcript)
        fail("the relay and loss traces are byte-identical; they must differ")


def check_task_lifecycle(transcript: str) -> None:
    """Every task init spawned reaches exactly one clean exit, and so does init.

    Derived from the root's own spawn records. The unconfigured instances the
    root launches are excluded by construction: they have no spawn record, so
    their exits are never consulted here. The composition-window failure check
    below is what keeps that exclusion honest.
    """
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

    # No component reported a failure while the composition ran. This is where
    # the unconfigured instances are separated from the real ones: theirs land
    # after init's clean exit, and a real participant's could not.
    reported = COMPONENT_FAILURE.findall(head)
    if reported:
        report_transcript(transcript)
        fail(f"a component failed inside the composition: {reported}")


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
    check_records(transcript)
    check_task_lifecycle(transcript)
    print(
        f"transcript: {sum(len(chain) for _, chain in CHAINS)} markers observed "
        f"across {len(CHAINS)} causal chains; {EXPECTED_VIEW_RECORDS} view records and "
        f"{EXPECTED_TRACE_RECORDS} distinct traces; six spawned tasks exited cleanly",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 visibility-plane image and assert C8.8"
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
        "seL4 visibility plane check: three callers received different bounded views "
        "from their exact grants, an ungranted caller inferred nothing, telemetry "
        "reached its subscriber only through the declared proxy, and proxy death "
        "produced a route event while the unrelated route stayed live"
    )


if __name__ == "__main__":
    main()
