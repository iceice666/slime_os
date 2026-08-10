#!/usr/bin/env python3

"""B16 gate: a graph outlives `MAX_RECORDS` and still answers every live handle.

Boots `build/slime-sel4-supervision.elf` -- the image whose root task embeds the
supervision-plane generation,
`contracts/generation/v1/fixtures/sel4-supervision.zti` -- and asserts ordered
markers for backlog B16's exit condition: *a graph that creates more than
`MAX_RECORDS` tasks over its lifetime still answers `supervision_status`
correctly for every live handle.*

Before the fix, `supervision::Terminations` never reclaimed a record, so
`MAX_RECORDS` bounded the tasks a boot could ever create rather than the outcomes
owed at once. This checker derives the current record bound from root source and
requires the scenario to exceed it while retaining two handles across the
crossing.

The fix reclaims records no live holder can name. This gate asserts:

1. the configured loop is strictly greater than the current `MAX_RECORDS`;
2. a handle held by init *across* the crossing still answers afterwards;
3. a handle parked in `Transit` across the crossing is still collectable.

(3) is the one a predicate over live capability tables alone would break: a
capability mid-transfer is held by no table by construction, so a sweep reading
only `GraphTables` frees its record and the eventual receiver waits forever --
B16 reintroduced by its own fix. Removing `Transit::holds_supervision` must fail
this gate; that fault injection is recorded in the devlog entry.

# Why the loop child is a new binary

`supervision-child` exists because every other component needs a channel, and
`ChannelTable` never reclaims either (backlog B22, opened alongside this fix).
A loop child holding a channel would exhaust `MAX_CHANNELS` -- also 32 -- one
iteration before reaching the record bound, so the gate would fail for a reason
unrelated to what it tests. `check_loop_child_is_channel_free` enforces that
against the source.

An eighth image beside the seven before it, on the same rule: each gate boots
the artifact it asserts about, so none invalidates another's evidence by being
built last.
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
IMAGE = ROOT / "build" / "slime-sel4-supervision.elf"
MANIFEST = ROOT / "build" / "slime-sel4-supervision.identity.json"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-supervision.zti"
IMAGE_VARIANT = "supervision"

BOOT_TIMEOUT_SECONDS = 180
MAX_TASKS_SOURCE = ROOT / "slime-root" / "src" / "task.rs"
INIT_SOURCE = ROOT / "components" / "bins" / "src" / "bin" / "init.rs"

REQUIRED_MARKERS: tuple[tuple[str, str], ...] = (
    (
        # The allocator headroom this fixture spends. A spawn-reap loop crossing
        # 32 lifetime tasks funds 35 task constructions with *zero* reuse: root
        # CSlots are deliberately not returned on reclaim (`CleanupRecord::revoke`
        # keeps accounting monotonic) and the untyped watermarks never rewind.
        # Asserting the line makes that headroom observed rather than assumed --
        # if it ever stops being enough, this marker moves rather than the gate
        # failing somewhere confusing.
        "the root reported its allocator headroom",
        r"SLIME_ROOT allocator slots=\d+ untypeds=\d+ bytes=\d+",
    ),
    (
        # B25: `supervision_derive` gives a parent a second handle naming a task
        # it already supervises. Before it, each spawn returned exactly one and
        # neither a spawn grant nor a `cap_transfer` could place it twice — a
        # grant because it must precede the child, a transfer because it moves.
        # Init derives here while it still holds the source, queries the *derived*
        # handle for the child's outcome, and then transfers the source, so the
        # marker proves the copy carries real authority and leaves the original
        # intact.
        "the root recorded the derive",
        r"SLIME_GRAPH supervision derived task=\d+ child=\d+ slot=\d+",
    ),
    (
        "a second supervision handle was derived and carried real authority",
        r"\[init\] second supervision handle derived",
    ),
    (
        # Parked before the loop and collected only after it. From the send
        # until the matching recv the capability is held by no table at all.
        "a supervision handle was parked in transit before the crossing",
        r"\[init\] supervision handle parked in transit",
    ),
    (
        "a supervision handle was retained across the crossing",
        r"\[init\] supervision handle retained",
    ),
    (
        # The source-derived check below proves the loop exceeds the current
        # MAX_RECORDS; this marker proves that configured loop actually ran.
        "the graph created more tasks over its lifetime than MAX_RECORDS holds",
        r"\[init\] supervision lifetime bound crossed",
    ),
    (
        "a handle held across the crossing still answered",
        r"\[init\] retained handle answered after crossing",
    ),
    (
        # Fault injection #2 targets exactly this: drop the `Transit` half of
        # the sweep predicate and this marker disappears while every marker
        # above it still passes.
        "a handle parked in transit across the crossing was still collectable",
        r"\[init\] transit handle survived crossing",
    ),
    (
        "the supervision plane ran to completion",
        r"\[init\] supervision plane complete",
    ),
    (
        # B24: every one of the 38 holders this plane constructs releases its
        # generation-declared ceiling when its task dies. `quotas` is the
        # shared-buffer table's own live count, so a `MAX_CHARGE_HOLDERS`
        # (96) that measured holders a boot *ever* built rather than those live
        # at once reads 38 here instead of 0 — fault-injected exactly that way.
        #
        # Asserted on this plane rather than a tenth image because it is
        # already the deepest spawn/reap loop in the corpus, which is the shape
        # the defect needs. Reaching the 96 bound itself is out of reach: root
        # CSlots are deliberately never returned, so a boot exhausts them near
        # 52 tasks. Zero-at-teardown is the observable the graph can carry.
        "every constructed holder released its declared quota",
        r"SLIME_GRAPH loans served=\d+ loans=0 mappings=0 regions=0 transit=0 "
        r"orphans=0 aliases=0 quotas=0",
    ),
    (
        # The root's *own* accounting, and the numerically strongest evidence in
        # the gate: every marker above is a string the driver chose to print,
        # whereas `terminated` is counted by `Terminations::recorded` inside the
        # root. B16's exit condition is "more than MAX_RECORDS tasks over its
        # lifetime", and MAX_RECORDS is 32, so the pattern admits 33..=99 only --
        # a loop that silently stopped crossing the bound cannot match it.
        #
        # `drops=0` because this plane never calls `cap_drop`: the loop's handles
        # are consumed by collection and the transferred one by `cap_transfer`.
        # `endpoints=1` is the single pair init mints for the transit arm, and it
        # is B22's evidence -- a channel-per-child loop would read 34 here and
        # would have hit `MAX_CHANNELS` instead.
        #
        # Terminal, and asserted last for a second reason: `MAX_GRAPH_ITERATIONS`
        # bounds the root's dispatch loop, and a graph that reached it would
        # drain incompletely and never print this line, so iteration exhaustion
        # cannot pass as success.
        "the root's own accounting recorded more terminations than MAX_RECORDS",
        r"SLIME_GRAPH spawns served=\d+ drops=0 endpoints=1 "
        r"terminated=\d+ waits=0",
    ),
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL .*",
    # Includes `termination lost task=N reason=records-full`, which is the
    # residual silent-drop this fix converts into a reported one. If the sweep
    # ever cannot free a slot, the gate says so rather than hanging.
    r"SLIME_GRAPH FAIL .*",
    r"\[init\] supervision plane fail: .*",
    r"\[init\] spawn plane fail: .*",
    r"SLIME_GRAPH spawn unwound .*",
    r"SLIME_GRAPH spawn failed .*",
    r"SLIME_GRAPH spawn unwind incomplete .*",
    r"SLIME_GRAPH channel (?:recall|rollback) failed .*",
    r"\[slime-rt\] transfer window bind failed",
    r"SLIME_GRAPH window bind refused",
    r"SLIME_GRAPH park refused .*",
    r"SLIME_GRAPH channel unplaced .*",
    r"SLIME_GRAPH service budget exhausted",
    r"Attempted to invoke a read-only endpoint",
    r"seL4 called fail",
    r"Caught cap fault",
    r"Caught vm fault",
    r"Caught user exception",
    r"panicked at ",
    r"aborted at ",
    r"\(aborted\)",
)

def check_loop_crosses_current_bound() -> tuple[int, int]:
    try:
        task_source = MAX_TASKS_SOURCE.read_text(encoding="utf-8")
        init_source = INIT_SOURCE.read_text(encoding="utf-8")
    except OSError as error:
        fail(f"cannot read supervision bound source: {error}")
    task_match = re.search(r"pub const MAX_TASKS: usize = (\d+);", task_source)
    loop_match = re.search(r"const SUPERVISION_LOOP_CHILDREN: u32 = (\d+);", init_source)
    if task_match is None or loop_match is None:
        fail("cannot derive MAX_RECORDS or SUPERVISION_LOOP_CHILDREN")
    bound = int(task_match.group(1))
    children = int(loop_match.group(1))
    if children <= bound:
        fail(f"supervision loop creates {children} children, not more than MAX_RECORDS={bound}")
    return bound, children


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 supervision plane check: {message}")


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
    command = [sys.executable, str(BUILD_SCRIPT), "--supervision-plane"]
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
            "run `just sel4_supervision_check`"
        )
    try:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"cannot parse {MANIFEST.relative_to(ROOT)}: {error}")
    if not isinstance(manifest, dict) or manifest.get("kind") != "slime-sel4-image-identity":
        fail(f"{MANIFEST.relative_to(ROOT)} is not a Slime seL4 identity manifest")
    # Every seL4 image is built from the same sources and differs only in which
    # generation the root task embeds, so booting the wrong one would fail on
    # markers rather than on identity. Checking the variant reports the actual
    # cause instead.
    if manifest.get("variant") != IMAGE_VARIANT:
        fail(
            f"{MANIFEST.relative_to(ROOT)} records variant "
            f"{manifest.get('variant')!r}, not {IMAGE_VARIANT!r}; "
            "rebuild with `--supervision-plane`"
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
    terminal = re.compile(REQUIRED_MARKERS[-1][1])
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
    position = 0
    for description, pattern in REQUIRED_MARKERS:
        match = re.compile(pattern).search(transcript, position)
        if match is None:
            report_transcript(transcript)
            if re.search(pattern, transcript) is not None:
                fail(f"marker out of order: {description} ({pattern})")
            fail(f"missing marker: {description} ({pattern})")
        position = match.end()
    bound, children = check_loop_crosses_current_bound()
    accounting = re.search(
        r"SLIME_GRAPH spawns served=(\d+) drops=0 endpoints=1 terminated=(\d+) waits=0",
        transcript,
    )
    if accounting is None:
        fail("missing supervision accounting values")
    spawns, terminated = (int(value) for value in accounting.groups())
    expected_spawns = children + 2
    # The root's termination counter covers the spawned scenario tasks plus
    # every *root*-launched instance. This fixture declares three instances but
    # only `init` is root-owned; the other two are init-spawned and so are
    # already inside `expected_spawns`. Counting them again assumed the
    # pre-B34 model where the root launched every declared instance.
    expected_terminated = expected_spawns + 1
    if spawns != expected_spawns or terminated != expected_terminated:
        fail(
            f"supervision accounting was spawns={spawns} terminated={terminated}, "
            f"expected spawns={expected_spawns} and terminated={expected_terminated} "
            f"from {children} loop children, two retained tasks, and one "
            "root-launched instance"
        )
    if terminated <= bound:
        fail(f"root recorded {terminated} terminations, not more than MAX_RECORDS={bound}")
    check_loop_child_is_channel_free()


# The loop child must hold no channel. `ChannelTable` never reclaims (B22), so a
# child that took a launch context would exhaust `MAX_CHANNELS` before the loop
# reached the `MAX_RECORDS` bound it exists to cross -- the gate would fail, but
# for the wrong reason, and a later reader would draw the wrong conclusion from
# it. Checked against the source because the transcript cannot show it.
LOOP_CHILD = "supervision-child"


def check_loop_child_is_channel_free() -> None:
    source = ROOT / "components" / "bins" / "src" / "bin" / f"{LOOP_CHILD}.rs"
    try:
        text = source.read_text(encoding="utf-8")
    except OSError as error:
        fail(f"cannot read {source.relative_to(ROOT)}: {error}")
    # Comments are stripped first: the child's own doc-comment explains *why* it
    # takes no channel, and naming the thing it avoids must not read as using it.
    code = "\n".join(
        line for line in text.splitlines() if not line.lstrip().startswith(("//", "//!"))
    )
    for forbidden in ("launch_context", "slime_rt::recv", "slime_rt::send", "endpoint_create"):
        if forbidden in code:
            fail(
                f"{source.relative_to(ROOT)} uses {forbidden!r}; the loop child must "
                "hold no channel, or B22's lifetime bound binds before B16's"
            )
    print(
        f"component: {LOOP_CHILD} takes no channel, so the loop crosses the record "
        "bound rather than the channel bound",
        flush=True,
    )


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boot the seL4 supervision-plane image and assert ordered markers"
    )
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="boot the already-built image instead of rebuilding it first",
    )
    arguments = parser.parse_args()

    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    pins = load_pins()
    if not arguments.no_build:
        build_image()
    check_manifest()
    profile = pins["qemu_arm_virt"]
    assert isinstance(profile, dict)
    check_transcript(boot(profile))
    print(
        "seL4 supervision plane check: a graph created more tasks over its lifetime "
        "than MAX_RECORDS holds at once, and still answered supervision_status for "
        "every live handle -- including one parked in transit across the crossing"
    )


if __name__ == "__main__":
    main()
