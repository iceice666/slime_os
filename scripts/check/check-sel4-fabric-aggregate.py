#!/usr/bin/env python3

"""C8.15 gate: full-graph determinism and the C8 parent close.

C8.9-C8.14 each assert one property of one plane. This gate closes the parent
milestone by asserting the two things no single-plane gate can:

1. **Determinism.** The same graph, inputs, and simulated-time sequence run
   twice must produce byte-identical semantic traces. This is checked by booting
   each aggregate plane twice and comparing its `[trace]` records verbatim, not
   by comparing a summary or a count -- a trace that agreed on how many records
   it emitted while disagreeing on their order would satisfy a count and still
   be unusable as the comparison baseline C8.11 promises.

2. **One aggregate path.** Both required schedules -- the normal concurrent one
   and the fault one -- are exercised over the *same* declared composition, so
   the parent exit condition is observed on one graph rather than assembled from
   separate profile boots. The fault variant shares `sel4-traffic.zti` with
   `generation` changed and nothing else; the fault variant differs only in that
   its interposition hop is compiled to die. That is what makes the pair an
   aggregate rather than two unrelated planes.

Why this is a separate gate rather than an extension of either plane's own. Each
plane gate boots once, because booting twice doubles the slowest step in the
suite for a property only this milestone needs. And determinism is a claim about
the relationship *between* runs, which a gate holding one transcript cannot
state at all.

What this gate deliberately does not re-assert: every property
`check-sel4-traffic-plane.py` and `check-sel4-fault-plane.py` already check.
Those gates are invoked here, in-process, against each boot they take -- so a
regression in any of them fails this gate too, and the aggregate does not become
a second, drifting copy of their expectations. `--no-build` is passed through so
this gate never rebuilds an image a plane gate just built.

The audit half of C8.15's deliverables -- reconciling the final authority,
resource, and fault corpus against every C8 deliverable -- is recorded in the
roadmap and its devlog entry rather than automated here: it is a reading of
prose against evidence, and a script asserting it would only be asserting that
someone wrote the prose.
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

from harness import load_script  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PINS_PATH = ROOT / "sel4" / "pins.toml"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
BOOT_TIMEOUT_SECONDS = 240

# The planes this aggregate composes, in the order it exercises them:
# `(label, gate module name, gate path, build flag, image)`.
#
# Both run the identical `drive_traffic_plane` composition over the identical
# declared graph. The fault plane's image differs only in that its declared
# interposition hop is compiled to exit rather than park, which is why the two
# are an aggregate over one graph rather than two planes to be compared.
PLANES: tuple[tuple[str, str, str, str, str], ...] = (
    (
        "normal concurrent schedule",
        "sel4_traffic_plane",
        "check/check-sel4-traffic-plane.py",
        "--traffic-plane",
        "slime-sel4-traffic.elf",
    ),
    (
        "fault schedule over the same graph",
        "sel4_fault_plane",
        "check/check-sel4-fault-plane.py",
        "--fault-plane",
        "slime-sel4-fault.elf",
    ),
)

# Records whose content is compared verbatim between boots. Deliberately only
# the C8.11 trace records: they are the milestone's declared evidence stream,
# they carry simulated time rather than wall time, and the schema forbids task
# ids and addresses in them -- so byte equality is a real claim about the
# schedule rather than about how the transcript was captured.
#
# Serial markers are excluded because several legitimately vary: a broker's
# per-edge print races a participant's own summary print, which is exactly why
# the plane gates check those as membership rather than as order.
TRACE_LINE = re.compile(r"\[trace\] .*")

# Every boot of either plane must emit exactly this many trace records. Pinned
# rather than merely compared between the two boots of one plane: without it, a
# regression that silently stopped every worker from emitting would produce two
# identical empty transcripts and pass the determinism comparison.
EXPECTED_TRACE_RECORDS = 140

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_ROOT FAIL",
    r"SLIME_GRAPH FAIL",
    r"\[init\] fabric boot fail: .*",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)

INIT_COMPLETE = r"\[init\] traffic plane reclaimed"
TERMINAL_MARKER = r"SLIME_GRAPH component exit task=(\d+) status=(-?\d+)"
SPAWN_PATTERN = re.compile(r"SLIME_GRAPH spawned task=(\d+) child=(\d+) component=([^ ]+) ")


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 fabric aggregate check: {message}")


def load_pins() -> dict[str, object]:
    if not PINS_PATH.is_file():
        fail(f"missing pin manifest: {PINS_PATH.relative_to(ROOT)}")
    try:
        pins = tomllib.loads(PINS_PATH.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as error:
        fail(f"cannot parse {PINS_PATH.relative_to(ROOT)}: {error}")
    profile = pins.get("qemu_arm_virt")
    if not isinstance(profile, dict):
        fail(f"{PINS_PATH.relative_to(ROOT)} declares no [qemu_arm_virt] table")
    return profile


def build_image(flag: str) -> None:
    command = [sys.executable, str(BUILD_SCRIPT), flag]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot run the seL4 image build: {error}")
    if process.returncode != 0:
        fail(f"seL4 image build failed with exit status {process.returncode}")


def boot(profile: dict[str, object], image: str, attempt: int) -> str:
    """Boot one image until init's clean exit, returning the transcript."""
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
    path = ROOT / "build" / image
    if not path.is_file():
        fail(f"missing packaged image {path.relative_to(ROOT)}")
    command = [
        qemu,
        "-machine",
        str(profile["machine"]),
        "-cpu",
        str(profile["cpu"]),
        "-smp",
        str(profile["cpus"]),
        "-m",
        f"size={profile['memory_mib']}M",
        "-nographic",
        "-serial",
        "mon:stdio",
        "-kernel",
        str(path),
    ]
    print(f"[boot {attempt}] {image}", flush=True)
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
            if spawn is not None and init_task is None:
                init_task = spawn.group(1)
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
        fail(f"{image} boot {attempt} exceeded {BOOT_TIMEOUT_SECONDS}s without init's clean exit")
    if not saw_init_exit:
        fail(f"{image} boot {attempt} did not reach init's clean exit")
    return transcript


def trace_records(transcript: str) -> list[str]:
    return TRACE_LINE.findall(transcript)


def check_determinism(label: str, first: str, second: str) -> int:
    """The plane's semantic trace is byte-identical across two boots."""
    left = trace_records(first)
    right = trace_records(second)
    if not left:
        fail(f"{label}: the first boot emitted no trace records at all")
    if len(left) != EXPECTED_TRACE_RECORDS:
        fail(
            f"{label}: the first boot emitted {len(left)} trace records, expected "
            f"{EXPECTED_TRACE_RECORDS}; a plane that stopped emitting would otherwise "
            "compare equal to itself and pass"
        )
    if len(right) != EXPECTED_TRACE_RECORDS:
        fail(
            f"{label}: the second boot emitted {len(right)} trace records, expected "
            f"{EXPECTED_TRACE_RECORDS}"
        )
    for index, (a, b) in enumerate(zip(left, right, strict=True)):
        if a != b:
            fail(
                f"{label}: trace record {index} differs between boots -- the semantic "
                f"trace depends on scheduling, so it cannot serve as a comparison "
                f"baseline.\n  first:  {a}\n  second: {b}"
            )
    return len(left)


def main() -> None:
    parser = argparse.ArgumentParser(
        description=(
            "Boot every C8 aggregate plane twice and assert C8.15's determinism and "
            "parent-close conditions"
        )
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built images instead of rebuilding them first",
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    profile = load_pins()

    total = 0
    for label, module_name, module_path, flag, image in PLANES:
        if not arguments.no_build:
            build_image(flag)
        gate = load_script(module_name, module_path)
        first = boot(profile, image, 1)
        # Every property the plane's own gate asserts, on this exact boot. Run
        # in-process against the transcript rather than by re-invoking the gate,
        # so the aggregate cannot drift from what the narrow gate requires and
        # neither boot is spent twice.
        gate.check_transcript(first)
        second = boot(profile, image, 2)
        gate.check_transcript(second)
        records = check_determinism(label, first, second)
        total += records
        print(
            f"[{label}] both boots satisfied {module_path} and emitted "
            f"{records} byte-identical trace records",
            flush=True,
        )

    print(
        f"seL4 fabric aggregate check: {len(PLANES)} schedules over one declared "
        f"composition each passed their own plane gate on two independent boots and "
        f"produced {total} byte-identical semantic-trace records in total; every "
        "declared authority, resource, and fault property those gates assert holds on "
        "both runs",
        flush=True,
    )


if __name__ == "__main__":
    main()
