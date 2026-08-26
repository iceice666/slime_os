#!/usr/bin/env python3

"""P5.4.3 gate: M6.4's Dango console session (M6.4).

A scripted console session launches commands through the spawn service, with
every launch traced to a profile resolution and a spawn request. The component
marker sequence was frozen at the P5 cutover; `dango.rs`, `spawn-service.rs`,
`sysinfo.rs`, `echo-agent.rs`, and `console.rs` remain unmodified.

Four lines of script, and each proves something different:

* `$(sysinfo)` — a plain launch resolves through the profile, is accepted, and
  reports a structured exit;
* the `with-env`/`with-cwd`/`with-stdin` composition — a launch carrying a
  derived working-directory capability and a stdin endpoint, so the child gets
  explicit context rather than an ambient one;
* `$(inject)` — a command the profile does not name is denied at resolution,
  before any spawn;
* `$(echo a b c)` — a malformed line is a parse error, not a launch.

The session ends on the scripted escape byte, and Dango's exit status is
*nonzero by design*: it reports the last failure, which the two refusals above
guarantee. What the gate requires is that it terminated rather than faulted.
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
import threading
import tomllib
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import GENERATION_COMPOSITIONS, profile_text, profile_integer  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
IMAGE = ROOT / "build" / "slime-sel4-dango.elf"
FIXTURE = GENERATION_COMPOSITIONS / "sel4-dango.zti"
BOOT_TIMEOUT_SECONDS = 300

# Markers whose presence is required but whose position is not, because they
# come from a component running concurrently with the shell.
UNORDERED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        # The child saw the context Dango gave it, and only that.
        "the launched command ran with the profile's authority",
        r"\[sysinfo\] spawned through profile",
    ),
)

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        "init spawned the spawn service",
        r"\[init\] spawn service spawned",
    ),
    (
        "init spawned dango",
        r"\[init\] dango spawned",
    ),
    (
        "the shell reached its prompt",
        r"\[dango\] native runtime ready",
    ),
    (
        # A plain launch. Resolution first — the command must be one the
        # generation's profile names — then the request, then the exit.
        "the first command resolved through the profile",
        r"resolved:profile",
    ),
    (
        "the spawn service accepted the request",
        r"spawn-request:accepted",
    ),
    (
        "the launched command reported a structured exit",
        r"result:exit:0",
    ),
    (
        # The composition: a derived cwd capability and a stdin endpoint both
        # cross to the spawn service with the request.
        "the second command resolved through the profile",
        r"resolved:profile",
    ),
    (
        "the second request was accepted",
        r"spawn-request:accepted",
    ),
    (
        "the second command reported a structured exit",
        r"result:exit:0",
    ),
    (
        # A command the profile does not name. Denied at resolution, so no
        # spawn request is ever made — there is no executable to name.
        "an undeclared command was denied at resolution",
        r"resolve-denied",
    ),
    (
        "a malformed line was a parse error rather than a launch",
        r"parse-error",
    ),
    (
        # C10.4: the repeat of the *first* command, scripted before the escape.
        # Same executable, so the released arena's retained parent is reused
        # rather than a new one provisioned, which is what makes the census
        # assertion a conservation claim rather than an observation of which
        # binaries ran.
        "the repeated command resolved through the profile",
        r"resolved:profile",
    ),
    (
        "the repeated request was accepted",
        r"spawn-request:accepted",
    ),
    (
        "the repeated command reported a structured exit",
        r"result:exit:0",
    ),
    (
        "the session closed on the scripted escape",
        r"\[dango\] interactive session closed",
    ),
    (
        "init observed the composition finish",
        r"\[init\] dango plane complete",
    ),
)

TERMINAL_MARKER = r"\[init\] dango plane complete"

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"SLIME_GRAPH wedged waiter",
    r"\[init\] dango plane fail: .*",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)

def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 dango plane check: {message}")


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
    command = [sys.executable, str(BUILD_SCRIPT), "--dango-plane"]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot run the seL4 image build: {error}")
    if process.returncode != 0:
        fail(f"seL4 image build failed with exit status {process.returncode}")




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
    print(f"[boot] {' '.join(command)}", flush=True)
    failures = re.compile("|".join(FAILURE_MARKERS))
    terminal = re.compile(TERMINAL_MARKER)
    lines: list[str] = []
    reached = False
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
            if terminal.search(line):
                reached = True
                break
    finally:
        watchdog.cancel()
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
    transcript = "\n".join(lines)
    if not reached:
        report_transcript(transcript)
        fail(f"boot exceeded {BOOT_TIMEOUT_SECONDS}s without completing the plane")
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
    position = 0
    for label, pattern in REQUIRED_MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            report_transcript(transcript)
            if re.search(pattern, transcript) is not None:
                fail(f"marker out of order: {label} ({pattern})")
            fail(f"missing marker: {label} ({pattern})")
        position = match.end()
    # Asserted for presence but not position. This is a different component's
    # output, and the spawned child runs before its requester hears back:
    # `spawn` starts the thread, then the spawn service sends the launch
    # context and only afterwards replies. Ordering it against the shell's own
    # lines would assert a race rather than the authority this plane is about.
    for label, pattern in UNORDERED_MARKERS:
        if re.search(pattern, transcript) is None:
            report_transcript(transcript)
            fail(f"missing marker: {label} ({pattern})")
    # Every launch is traced to a resolution and a request, and there are
    # exactly three of each. A fourth would mean a denied or malformed line
    # reached the spawn service, which is the property `resolve-denied` and
    # `parse-error` assert from the shell's side and this asserts from the
    # service's.
    resolutions = transcript.count("resolved:profile")
    accepted = transcript.count("spawn-request:accepted")
    if resolutions != 3 or accepted != 3:
        report_transcript(transcript)
        fail(
            f"{resolutions} profile resolutions and {accepted} accepted requests, "
            "expected exactly 3 of each"
        )
    # The spawn service launched both executables its profile names, and each
    # child saw the context the shell gave it rather than an ambient one.
    if "[sysinfo] command=sysinfo" not in transcript:
        fail("sysinfo did not report the command it was launched with")
    if "echo-agent{tool=echo" not in transcript:
        fail("echo-agent did not report its arguments")
    reuses = check_spawn_exit_returned_every_frame(transcript)
    print(
        f"transcript: {len(REQUIRED_MARKERS)} markers observed; {resolutions} "
        f"commands resolved through the profile and {accepted} launched through "
        "the spawn service, an undeclared command was denied at resolution, "
        "a malformed line was a parse error, and a repeated spawn/exit cycle "
        f"returned every frame with {reuses} arena reuse(s)",
        flush=True,
    )


def check_spawn_exit_returned_every_frame(transcript: str) -> int:
    """C10.4: a repeated spawn/exit cycle returns the free-frame count.

    Read from `SLIME_ROOT reclaim census`, which the root prints from its
    allocator's own watermarks at the point a task has just returned everything
    it held. Deliberately not from the counts the root *tracks* — reclaimed
    CSlots, live tasks, released buffers — because those are produced by the same
    bookkeeping that would be wrong if reclamation were broken. B9 is the
    standing evidence: terminated tasks were marked terminated and their buffers
    reclaimed while thirteen frames per spawn were never returned, and every
    counter the root printed agreed with every other one throughout.

    The claim is about the *repeat*. `begin_task_arena` retains a released
    arena's parent untyped for reuse by the next task of the same size, so a
    cycle launching a new executable legitimately provisions a new parent and
    the free count legitimately falls. The script therefore repeats its first
    command, and the assertion is that the census after the repeat equals the
    census after that first command exactly — with the reuse counter having
    advanced, which is what proves the repeat took the retained arena rather
    than getting a fresh one that happened to cost the same.
    """
    # Bound to the spawned commands rather than taken positionally. The shell's
    # own graph tears down at the end of the plane and emits census records too,
    # and init and the spawn service die in an order the scheduler picks — so
    # "the first three records" is only the three spawned commands if nothing
    # else ever dies first.
    #
    # Each command's census is the last one *before* its `result:exit`, not the
    # first one after. The order is a property of the mechanism rather than of
    # this boot: a dead task is reclaimed by `reclaim_task_objects` inside the
    # root's own service-loop sweep, and the shell only learns the outcome when
    # the spawn service's reply reaches it afterwards. Both reclamation call
    # sites are in that one loop — the console dispatcher (B41's second root
    # thread) never reclaims — so the two lines cannot interleave the other way.
    # Slicing forward instead collected the *next* command's record, and for the
    # third command it collected the shell's own teardown, which is what made
    # the first version of this fail against a correct boot.
    exits = [m.start() for m in re.finditer(r"result:exit:0", transcript)]
    if len(exits) != 3:
        report_transcript(transcript)
        fail(f"{len(exits)} structured command exits, expected exactly 3")
    pattern = re.compile(
        r"SLIME_ROOT reclaim census task=\d+ slots=(?P<slots>\d+) "
        r"bytes=(?P<bytes>\d+) live_objects=(?P<objects>\d+) "
        r"arena_reuses=(?P<reuses>\d+)"
    )
    census = []
    start = 0
    for index, exit_at in enumerate(exits):
        records = list(pattern.finditer(transcript, start, exit_at))
        if not records:
            report_transcript(transcript)
            fail(
                f"command {index + 1} reported a structured exit but the root printed "
                "no reclaim census for it, so its frames were never accounted"
            )
        match = records[-1]
        census.append(
            (
                int(match["slots"]),
                int(match["bytes"]),
                int(match["objects"]),
                int(match["reuses"]),
            )
        )
        start = exit_at
    first, second, repeat = census
    if repeat[3] == 0:
        report_transcript(transcript)
        fail(
            "the repeated spawn/exit cycle provisioned a new arena rather than "
            f"reusing the released one (arena_reuses={repeat[3]}), so the frame "
            "count below is not a conservation claim"
        )
    if repeat[:3] != second[:3]:
        report_transcript(transcript)
        fail(
            "a repeated spawn/exit cycle did not return every frame: after the "
            f"second command the allocator had slots={second[0]} "
            f"bytes={second[1]} live_objects={second[2]}, and after repeating "
            f"the first it had slots={repeat[0]} bytes={repeat[1]} "
            f"live_objects={repeat[2]}"
        )
    # And the drift across the two *distinct* executables is bounded rather than
    # unexamined: each new executable takes one arena parent, so the count may
    # fall once per new size class and no further.
    if second[0] > first[0]:
        report_transcript(transcript)
        fail(
            f"the allocator gained slots across a spawn/exit cycle "
            f"({first[0]} then {second[0]}), which no reclamation path can do"
        )
    return repeat[3]


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 dango-plane image and assert M6.4"
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
    if not IMAGE.is_file():
        fail(f"missing packaged image {IMAGE.relative_to(ROOT)}")
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    check_transcript(boot(profile))
    print(
        "seL4 dango plane check: a scripted console session resolved three "
        "commands through the generation's profile and launched all three through "
        "the spawn service with explicit environment, working directory, and "
        "stdin; an undeclared command was denied at resolution, a malformed "
        "line was a parse error, and repeating the first command returned every "
        "frame the cycle had taken"
    )


if __name__ == "__main__":
    main()
