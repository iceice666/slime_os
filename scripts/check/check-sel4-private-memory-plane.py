#!/usr/bin/env python3

"""C10.2 gate: the generation's declared private-memory budget is the live ceiling.

C10.1 proved the mechanism against a quota compiled into `slime-root`, on the
root's own embedded fixture child. That leaves the question C10.2 exists to
answer untested: does a quota *declared in a generation* reach the component the
generation names, and does omission actually deny? This gate boots one image
whose fixture declares one executable twice — as a granted holder and as an
omitted one — and checks four things no single marker states:

* **the declared quota is the installed ceiling.** Every declared instance's
  ceiling is read out of `sel4-private-memory.zti` here and compared against
  what the root reports installing on the task. The root prints `declared=` from
  the budget it admitted and `installed=` read back off the task record, so a
  root that resolved the budget and then constructed the task from something
  else disagrees with itself in one line;
* **the ceiling binds at exactly the declared number.** The granted probe maps
  the full declared span, then asks for one more page and is refused by the
  fixed reservation at exactly that extent. The probe never reads the manifest,
  so the gate's comparison remains a measurement rather than a restatement;
* **omission denies.** The instance absent from the budget must be refused its
  full-window request with `cause=reservation` — the deny-by-default state
  carries no window at all, so it is refused before quota arithmetic is
  reached, which is a stronger statement than "was given zero";
* **a refusal has no effect.** The granted probe re-queries after its refusal
  and must find the region unchanged, and the root's growth grants must total
  exactly the declared quota: a query or a refused growth that charged a page
  would show up as a grant count above the ceiling.

The two directions are deliberately independent. The probe reports what it
observed and the root reports what it enforced; neither reads the other, and the
gate is what makes them agree. A probe that asserted its own copy of the
manifest could pass against a root that had stopped honouring declarations
entirely.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import threading
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
from boot_contracts import PRIVATE_MEMORY_CAPACITY_PROFILES  # noqa: E402
from closure_image import ClosureImageError, build as build_closure_image  # noqa: E402

from harness import (
    GENERATION_COMPOSITIONS,
    load_qemu_profile,
    profile_integer,
    profile_text,
    qemu_kernel_arguments,
    sha256_file,
)  # noqa: E402
from sel4_gate_markers import (  # noqa: E402
    chains_from_gate,
    marker_count,
    match_marker_contract,
)
from zutai_cli import STDLIB, binary  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
# The closure identity names the build's inputs and is re-resolved from repository
# state before building, so stale input is refused instead of silently changing the image.
CLOSURE = "sel4-private-memory"
IMAGE: Path | None = None
PINS = ROOT / "sel4" / "pins.toml"
FIXTURE = GENERATION_COMPOSITIONS / "sel4-private-memory.zti"
TIMEOUT = 240
ROLLBACK_CLOSURE = "sel4-private-memory-fail-second-allocation"
LARGE_MAP_CLOSURE = "sel4-private-memory-fail-large-map"
CLOSURE_PLATFORM = "qemu-arm-virt"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
RV64_IMAGE = ROOT / "build" / "slime-sel4-private-memory-qemu-riscv-virt.elf"
# MEM-64M's reuse arm. Closure-backed and AArch64 only: the clause it qualifies
# names no architecture, and the milestone's two-architecture requirement is the
# grow-to-ceiling condition the default arm already observes on both.
CYCLES_CLOSURE = "sel4-private-memory-cycles"
CYCLES_FIXTURE = GENERATION_COMPOSITIONS / "sel4-private-memory-cycles.zti"
# Holder lives the arm requires. Its declared ceiling is read from the fixture
# rather than restated, so a composition that lowered the quota fails instead of
# qualifying a smaller working set.
CYCLE_COUNT = 20
PLATFORMS = {
    "qemu-arm-virt": ("qemu_arm_virt", "qemu-system-aarch64"),
    "qemu-riscv-virt": ("qemu_riscv_virt", "qemu-system-riscv64"),
}
TARGET_PROFILES = {
    "qemu-arm-virt": "aarch64-sel4-qemu-virt",
    "qemu-riscv-virt": "riscv64-sel4-qemu-virt",
}

# Causal chains rather than one flat sequence, on B55/B68's rule: a required
# order must be one the mechanism promises, not one a scheduler happened to
# produce. Within each chain the order is causal. *Between* the two probes it is
# not: the root constructs `private-memory-denied` first and
# `private-memory-granted` second, but seL4's `tcbSchedEnqueue` is LIFO at equal
# priority, so the granted probe actually runs first. Asserting either order
# across the two would pin a scheduling artifact, which is exactly what B68
# found a determinism gate doing.
#
# `(description, pattern)` in every tuple, which is the order
# `scripts/lib/sel4_gate_markers.py` reads: `chains_from_gate` takes element 1
# as the regex, so an inverted table would make `just sel4_gate_control_check`
# mutate the prose instead of the markers and its pinned count would guard
# nothing.
CEILING_CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        # The root resolves the budget once and installs every declared
        # instance's ceiling before any of them run, so these are genuinely
        # sequential and in construction order — a single loop in the root, not
        # a scheduling order.
        #
        # Every instance, including both zero-quota ones. Without a marker per
        # instance, a root that installed no ceiling at all for a denied holder
        # would emit no record, and `check_declared_is_installed` — which
        # iterates whatever records exist — would pass on the deny-by-default
        # half by finding nothing to check.
        "the budget was admitted and every declared ceiling installed",
        (
            r"SLIME_MEM budget holders=(\d+) declared=1",
            r"SLIME_MEM quota task=\d+ instance=init declared=0 installed=0 base=0x0",
            r"SLIME_MEM quota task=\d+ instance=private-heap-both "
            r"declared=(\d+) installed=(\d+) base=0x[0-9a-f]+",
            r"SLIME_MEM quota task=\d+ instance=private-heap-denied "
            r"declared=0 installed=0 base=0x0",
            r"SLIME_MEM quota task=\d+ instance=private-heap-granted "
            r"declared=(\d+) installed=(\d+) base=0x[0-9a-f]+",
            r"SLIME_MEM quota task=\d+ instance=private-memory-denied "
            r"declared=0 installed=0 base=0x0",
            r"SLIME_MEM quota task=\d+ instance=private-memory-granted "
            r"declared=(\d+) installed=(\d+) base=0x[0-9a-f]+",
        ),
    ),
    (
        # The granted probe's own sequence: it is refused only after reaching
        # its ceiling, and it reports only after the refusal.
        #
        # `cause=reservation` because this holder's declared quota *is* the
        # target's whole region reservation, and `Region::admit` tests the
        # reservation first — an affordable-but-impossible request is malformed
        # rather than merely unaffordable. The quota cause is exercised by
        # `private-heap-granted`, whose declared ceiling sits below the
        # reservation; between the two holders both refusal arms are reached.
        "the granted holder reached its declared ceiling and was then refused",
        (
            r"SLIME_MEM grown task=\d+ delta=16384 previous=0 pages=16384 "
            r"base=0x[0-9a-f]+ quota=16384 total=\d+ large_frames=32 base_frames=0 leaf_tables=0",
            r"SLIME_MEM refused task=\d+ delta=1 cause=reservation "
            r"detail=ReservationExceeded \{ pages: 16384, delta: 1, reservation: 16384 \}",
            r"\[private-memory-probe\] granted pages=(\d+) base=0x[0-9a-f]+ "
            r"zeroed=1 survived=1 refused=1 worker_rpc_once=1 worker_grow_refused=1 retries=0",
        ),
    ),
    (
        # The omitted probe's own sequence, independent of the granted one's.
        "the omitted holder was refused its first page by the reservation",
        (
            r"SLIME_MEM refused task=\d+ delta=16384 cause=reservation "
            r"detail=ReservationExceeded \{ pages: 0, delta: 16384, reservation: 0 \}",
            r"\[private-memory-probe\] denied pages=0 base=0x0 refused=1",
        ),
    ),
    (
        # then a raw growth request covering exactly the unbacked reservation is
        # refused by the holder quota, and the component survives to report.
        # Causal within the chain — the refusal cannot precede the check that
        # established the heap works.
        #
        # The reuse boundary is a required marker as well as the window
        # `check_growth_was_batched_and_reused` measures growth in, so a probe
        # that stopped emitting it fails here rather than silently making that
        # window empty and its assertion vacuous.
        #
        # MEM-64M's runtime-heap case sits between them: 60 MiB of payload held
        # through ordinary collections, with the allocator's own overhead and
        # the pages the root backed reported separately rather than as one
        # total. `check_heap_capacity_is_within_the_declared_quota` joins those
        # to the declared ceiling, which the component cannot read itself.
        "the granted holder allocated through ordinary collections, then hit its ceiling",
        (
            r"\[private-heap-probe:granted\] private-heap reuse phase begins",
            r"\[private-heap-probe:granted\] private-heap quota live pages=(\d+) "
            r"growths=(\d+) reuse_growths=0 leaked=0",
            r"\[private-heap-probe:granted\] capacity payload=62914560 overhead=\d+ "
            r"backed=\d+ pages=\d+ touched=1",
            r"SLIME_MEM refused task=\d+ delta=[1-9]\d* cause=quota "
            r"detail=QuotaExceeded \{ pages: (\d+), delta: \d+, quota: (\d+) \}",
            r"\[private-heap-probe:granted\] granted pages=(\d+) growths=(\d+) refused=1 reused=1",
        ),
    ),
    (
        # The omitted holder's own sequence. Its allocator finds no region, so
        # it never reaches the root at all — which is why the assertion is the
        # component's own two lines rather than a root record.
        "the omitted holder could not allocate at all",
        (
            r"\[private-heap-probe:denied\] private-heap denied pages=0 growths=0 "
            r"reuse_growths=0 leaked=0",
            r"\[private-heap-probe:denied\] denied pages=0 growths=0 refused=1",
        ),
    ),
    (
        # C10.4: the one holder the generation gave *both* a private region and a
        # shared-buffer factory. The two planes are separately accounted, so
        # exhausting either must leave the other's declared ceiling intact —
        # and the buffer must not be mappable into the private window, which is
        # the only address space the two share.
        #
        # Causal within the chain and load-bearing in this order: the refusal is
        # the root's own record naming the window it defended, and the report
        # cannot precede it because the component makes the request before it
        # reports. The base in the refusal is the window's own start, which is
        # the page the allocator's first growth already took — so a root that
        # admitted this would be handing out storage in use as heap.
        "the holder of both planes was refused a buffer in its private window",
        (
            r"SLIME_MEM mapping refused task=\d+ base=0x([0-9a-f]+) "
            r"end=0x([0-9a-f]+) window=0x([0-9a-f]+)\.\.0x([0-9a-f]+)",
            r"\[private-heap-probe:both\] both pages=(\d+) growths=(\d+) buffers=1 "
            r"window_map_refused=1 outside_map=1 released=1 reused=1",
        ),
    ),
    (
        "the plane ran to completion with no declared instance failing",
        (
            r"\[init\] private memory plane complete",
            r"SLIME_GRAPH HEALTHY generation=\d+ required=\d+ live=\d+ completed=\d+ failed=0",
        ),
    ),
)

# MEM-64M's reuse clause. Four chains rather than twenty copies of one: the
# causal contract is what a *cycle* promises, and the count is an aggregate
# property `check_reuse_cycles` measures over the whole transcript. Twenty
# inlined copies would also be twenty times the synthetic transcript
# `sel4_gate_control_check` mutates, for no coverage the count check does not
# already give.
#
# Between chains the order is not asserted: init serializes the cycles, so the
# first exit precedes the first fault by construction, but pinning that here
# would restate init's loop rather than the root's promise.
CYCLE_CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        # One incarnation end to end. Causal and load-bearing in this order:
        # the ceiling is installed before the growth can be admitted, the
        # growth precedes the touch the report summarizes, and the report
        # cannot follow the exit that ends the task.
        #
        # `previous=0` is the reuse assertion inside the growth marker: a root
        # that re-served a dead holder's committed pages would start this
        # incarnation above zero. `large_frames=32` is the second: reclaimed
        # backing must come back as 2 MiB frames, not degrade into base pages
        # once the first holder has returned them.
        "a cycle installed its declared ceiling, grew into it, and reported a zeroed region",
        (
            r"SLIME_MEM quota task=\d+ instance=private-cycle-probe "
            r"declared=16384 installed=16384 base=0x[0-9a-f]+",
            r"SLIME_MEM grown task=\d+ delta=16384 previous=0 pages=16384 "
            r"base=0x[0-9a-f]+ quota=16384 total=16384 large_frames=32 base_frames=0 "
            r"leaf_tables=0",
            r"\[private-cycle-probe\] cycle=\d+ pages=16384 base=0x[0-9a-f]+ "
            r"stamp=0x[0-9a-f]+ zeroed=1 verified=1 end=exit",
            r"SLIME_GRAPH component exit task=\d+ status=0",
            r"SLIME_ROOT reclaim census task=\d+ slots=\d+ bytes=\d+ live_objects=\d+ "
            r"extent_reuses=\d+",
        ),
    ),
    (
        # The fault path's own sequence. Separate from the exit chain because
        # the two are different reclamation routes in the root
        # (`services.rs` reaches `reclaim_dead_task` from the fault arm and the
        # exit arm independently), and the milestone requires immediate reuse
        # after either.
        #
        # `kind=VirtualMemory` is pinned but the access is deliberately not.
        # Pinning `VirtualMemory` is what distinguishes a holder that died on a
        # memory access from one that died of a syscall or user exception, which
        # a pattern ending at `kind=` accepts. The access is left open because
        # this configuration does not characterize it: the store demonstrably
        # traps -- the probe's own "did not trap" line never appears -- yet the
        # root reports `access: Execute status: 0`, and the pre-existing
        # `reclamation-fault` probe reports exactly the same shape for its own
        # deliberate write. That decode question is `fault.rs`'s, not this
        # arm's, and asserting `Write` here would pin behaviour the platform
        # does not produce.
        "a cycle that ended by deliberate fault was reclaimed and censused",
        (
            r"\[private-cycle-probe\] cycle=\d+ pages=16384 base=0x[0-9a-f]+ "
            r"stamp=0x[0-9a-f]+ zeroed=1 verified=1 end=fault",
            r"SLIME_GRAPH component fault task=\d+ kind=VirtualMemory \{ access: \w+, "
            r"status: \d+ \} address=Some\(\d+\)",
            r"SLIME_ROOT reclaim census task=\d+ slots=\d+ bytes=\d+ live_objects=\d+ "
            r"extent_reuses=\d+",
        ),
    ),
    (
        # Init's own tally, so a plane that silently ran fewer cycles fails
        # here rather than passing a count check that measured what it found.
        "init drove both termination paths to the declared cycle count",
        (r"\[init\] private memory cycles exits=10 faults=10",),
    ),
    (
        "the cycles plane ran to completion with no declared instance failing",
        (
            r"\[init\] private memory cycles plane complete",
            r"SLIME_GRAPH HEALTHY generation=\d+ required=\d+ live=\d+ completed=\d+ failed=0",
        ),
    ),
)

# The union, for `sel4_gate_control_check`'s coverage count only. Each arm
# matches its own chains; a transcript from one arm does not carry the other's
# markers, so matching the union would fail every run.
CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = CEILING_CHAINS + CYCLE_CHAINS

EXPECTED_UNORDERED: tuple[str, ...] = (
    r"\[private-memory-probe\] query pages=0 base=0x[0-9a-f]+",
)
FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_MEM FAIL",
    r"\[private-memory-probe\] FAIL",
    # Every role, named explicitly. C10.4 gave this image three instances and
    # labelled all of their output per role, so a pattern naming one role would
    # go quiet for the other two — and one matching any suffix would be a
    # pattern the gate-control harness cannot instantiate. Listing the three is
    # what keeps both true, and adding a fourth instance is then a deliberate
    # edit here rather than a silent widening.
    r"\[private-heap-probe:(?:granted|denied|both)\] FAIL",
    r"\[init\] private memory plane fail",
    # A growth the root served for a holder the budget does not name would be
    # the whole milestone failing silently, so it is a failure marker rather
    # than an absent-marker check.
    r"SLIME_MEM grown task=\d+ delta=[1-9]\d* previous=\d+ pages=\d+ base=0x0 ",
    # C10.3: exhaustion must be a structural error the component observes. A
    # component that faulted or was terminated instead would leave the chain's
    # report missing, but naming the outcomes explicitly says *which* failure
    # happened rather than only that a marker is absent.
    r"\[private-heap-probe:(?:granted|denied|both)\] private-heap exhausted",
    r"\[private-heap-probe:(?:granted|denied|both)\] private-heap failed",
    # The cycles arm's own failures. The probe's `FAIL` line names which
    # invariant broke and in which incarnation, so it is vetoed rather than
    # left to surface as a missing report; a reclaim that came back incomplete
    # is the root admitting it could not return what a dead holder held, which
    # is exactly the drift this arm exists to detect.
    r"\[private-cycle-probe\] FAIL",
    r"\[init\] private memory cycles plane fail",
    r"SLIME_GRAPH task reclaim incomplete task=\d+",
    r"SLIME_GRAPH holder reclaim incomplete task=\d+",
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 private-memory plane check: {message}")


def declared_quotas(fixture: Path = FIXTURE) -> dict[str, int]:
    """The ceilings the arm's own composition declares, read from that fixture.

    Read rather than restated: the gate's whole assertion is that the
    generation's declaration is the live ceiling, and a copy of the number in
    this file would make that a comparison against itself. Decoded through the
    contract's own schema so a malformed fixture fails here rather than
    producing a plausible dict.
    """
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    process = subprocess.run(
        [str(binary()), "json", str(fixture)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if process.returncode != 0:
        fail(f"could not decode the fixture: {process.stdout.strip()}")
    manifest = json.loads(process.stdout)
    declared = {
        entry["holder"]: int(entry["pageQuota"])
        for entry in manifest.get("privateMemoryBudget") or []
    }
    instances = {entry["name"] for entry in manifest["instances"]}
    unknown = sorted(set(declared) - instances)
    if unknown:
        fail(f"the fixture's budget names undeclared instance(s): {unknown}")
    if not declared:
        fail("the fixture declares no private-memory quota, so the plane asserts nothing")
    return declared


def image_path(platform: str) -> Path | None:
    if platform == CLOSURE_PLATFORM:
        return IMAGE
    return RV64_IMAGE


def build_image(platform: str, *, reuse: bool = True) -> None:
    """Build this plane through its closure or declared cross-target legacy arm."""
    global IMAGE
    if platform == CLOSURE_PLATFORM:
        try:
            built = build_closure_image(CLOSURE, reuse=reuse)
        except ClosureImageError as error:
            fail(str(error))
        IMAGE = built.image
        actual = sha256_file(IMAGE, fail)
        if actual != built.digest():
            fail(
                f"{IMAGE} SHA-256 is {actual}, but the build result records "
                f"{built.digest()}; the image changed after it was built"
            )
        return

    command = [
        sys.executable,
        str(BUILD_SCRIPT),
        "--skip-pin-check",
        "--private-memory-plane",
        "--platform",
        platform,
    ]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot build the RV64 private-memory image: {error}")
    if process.returncode != 0:
        fail(f"RV64 private-memory image build failed with exit status {process.returncode}")


def boot(
    profile: dict[str, object],
    *,
    section: str,
    qemu_binary: str,
    image: Path,
) -> str:
    qemu = shutil.which(qemu_binary)
    if qemu is None:
        fail(f"{qemu_binary} is not on PATH")
    command = [
        qemu,
        "-machine",
        profile_text(profile, "machine", fail, section),
        "-cpu",
        profile_text(profile, "cpu", fail, section),
        "-smp",
        str(profile_integer(profile, "cpus", fail, section)),
        "-m",
        f"size={profile_integer(profile, 'memory_mib', fail, section)}M",
        "-nographic",
        "-serial",
        "mon:stdio",
        *qemu_kernel_arguments(qemu_binary, image, fail),
    ]
    print(f"[boot] {' '.join(command)}", flush=True)
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
    watchdog = threading.Timer(TIMEOUT, process.kill)
    watchdog.start()
    lines: list[str] = []
    # `SLIME_ROOT READY` as well as the graph's terminal: the rollback case
    # boots the fixture root, whose embedded child runs the injected-failure arm
    # of the private-memory phase and which never launches a component graph.
    terminal = re.compile(
        r"SLIME_GRAPH HEALTHY|SLIME_ROOT READY|SLIME_ROOT FATAL|private memory plane fail"
        r"|\[private-memory-probe\] FAIL|\[private-heap-probe:(?:granted|denied|both)\] FAIL"
    )
    try:
        assert process.stdout is not None
        for line in process.stdout:
            lines.append(line.rstrip("\n"))
            if terminal.search(line):
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
    if timed_out:
        markers = [line for line in lines if "SLIME_" in line]
        fail(
            f"QEMU timed out; markers: {' | '.join(markers[-80:])}; "
            f"tail: {' | '.join(lines[-20:])}"
        )
    return "\n".join(lines)

def build_named_image(name: str) -> Path:
    try:
        built = build_closure_image(name)
    except ClosureImageError as error:
        fail(str(error))
    actual = sha256_file(built.image, fail)
    if actual != built.digest():
        fail(f"{name}: image changed after its build result was written")
    return built.image


def check_large_map_retry(transcript: str) -> None:
    """A failed large mapping converts its reserved backing to base pages.

    The injection is scoped to the first large frame of an aligned full-window
    growth, so the refusal reports `allocated: 0` — nothing of this attempt was
    committed — and the retry reaches the same quota through base pages over the
    same window base. Quotas come from the fixture rather than being literals,
    because the declared ceiling is target-qualified and moves with the
    contract's capacity row.
    """
    refused = re.findall(
        r"SLIME_MEM refused task=(\d+) delta=(\d+) cause=frames "
        r"detail=Frames \{ allocated: 0,",
        transcript,
    )
    if len(refused) != 1:
        fail(f"large-map case recorded {len(refused)} injected refusal(s), expected one")
    task, quota = refused[0][0], int(refused[0][1])
    # The retry converts only the *failed span*: one page installs the leaf
    # table the block mapping had made impossible, the rest of that 2 MiB span
    # fills with base pages, and every later span still takes a large frame.
    # Segmentation is what makes that possible — before it, one failed mapping
    # demoted the whole window.
    conversion = re.search(
        rf"SLIME_MEM grown task={task} delta=1 previous=0 pages=1 "
        rf"base=(?P<base>0x[0-9a-f]+) quota={quota} total=\d+ "
        r"large_frames=0 base_frames=1 leaf_tables=1",
        transcript,
    )
    if conversion is None:
        fail("large-map failure did not convert its reserved backing to base pages")
    completion = re.search(
        rf"SLIME_MEM grown task={task} delta={quota - 1} previous=1 pages={quota} "
        rf"base={conversion.group('base')} quota={quota} total=\d+ "
        r"large_frames=(?P<large>\d+) base_frames=(?P<base_pages>\d+) leaf_tables=\d+",
        transcript[conversion.end() :],
    )
    if completion is None:
        fail("large-map retry did not reach the declared quota after converting")
    spans = quota // 512
    if int(completion.group("large")) != spans - 1:
        fail(
            f"large-map retry used {completion.group('large')} large frame(s), "
            f"expected {spans - 1}: only the failed span demotes"
        )
    if int(completion.group("base_pages")) != 512:
        fail(
            f"large-map retry used {completion.group('base_pages')} base page(s), "
            "expected exactly one 2 MiB span's worth"
        )
    report = re.search(
        rf"\[private-memory-probe\] granted pages={quota} base={conversion.group('base')} "
        r"zeroed=1 survived=1 refused=1 worker_rpc_once=1 worker_grow_refused=1 retries=1",
        transcript,
    )
    if report is None:
        fail("large-map failure did not retry successfully with one coherent backing object")


def check_incremental_rollback(transcript: str) -> None:
    prefix = "private rollback: "
    failures = (
        "SLIME_CHILD mem rollback failed",
        "SLIME_CHILD mem zero retry failed",
        "SLIME_MEM FAIL",
        "SLIME_ROOT FATAL",
    )
    for marker in failures:
        if marker in transcript:
            fail(prefix + f"explicit failure marker present: {marker}")

    patterns = (
        r"SLIME_MEM refused task=0 delta=2 cause=frames detail=Frames \{ allocated: 1,",
        r"SLIME_CHILD mem rollback preserved pages=1 base=0x(?P<task0>[0-9a-f]+) survived=0x4d454d5f42415345",
        r"SLIME_MEM refused task=1 delta=2 cause=frames detail=Frames \{ allocated: 1,",
        r"SLIME_CHILD mem zero rollback result=-\d+ pages=0 base=0x(?P<zero>[0-9a-f]+)",
        r"SLIME_MEM grown task=1 delta=512 previous=0 pages=512 base=0x(?P<root>[0-9a-f]+) quota=512 total=513 large_frames=0 base_frames=512 leaf_tables=1",
        r"SLIME_CHILD mem whole retry pages=512 base=0x(?P<whole>[0-9a-f]+) zeroed=1 preserved=1 survived=0x4d454d5f42415345",
        r"SLIME_CHILD fault requested addr=0x0",
        r"SLIME_ROOT child fault observed task=1 role=deliberate-fault kind=VirtualMemory \{ access: Write",
        r"SLIME_MEM enforced clean_quota=4 retry_quota=512 pages=513 grants=2 grown=513 reclaimed=0 flags=0x7f",
        r"SLIME_MEM teardown grown=513 reclaimed=513 pages=0",
        r"SLIME_ROOT READY tasks=2 grants=\d+ declared_grants=\d+ reclaimed_slots=\d+",
    )
    position = 0
    matches: list[re.Match[str]] = []
    for pattern in patterns:
        match = re.search(pattern, transcript[position:])
        if match is None:
            fail(prefix + f"missing or out-of-order evidence: {pattern}")
        position += match.end()
        matches.append(match)
    refusals = re.findall(
        r"SLIME_MEM refused task=[01] delta=2 cause=frames detail=Frames \{ allocated: 1,",
        transcript,
    )
    if len(refusals) != 2:
        fail(prefix + f"recorded {len(refusals)} injected refusals, expected two")
    bases = {
        int(matches[3].group("zero"), 16),
        int(matches[4].group("root"), 16),
        int(matches[5].group("whole"), 16),
    }
    if len(bases) != 1 or 0 in bases:
        fail(prefix + "task1 retry bases differ or are zero")

def check_markers(
    transcript: str, chains: tuple[tuple[str, tuple[str, ...]], ...] = CEILING_CHAINS
) -> None:
    # The shared helper rather than a local loop, on B63's rule: every other
    # plane gate's chain matching, failure vetoing, and out-of-order reporting
    # lives here, and a private reimplementation is a second copy that can drift
    # from the one `sel4_gate_control_check` drives.
    #
    # The arm's own chains, not the module's union: each arm boots its own
    # image, so a transcript carries one arm's markers and matching the union
    # would fail every real run. The union exists for the meta-gate's coverage
    # count, which synthesizes a transcript containing every marker.
    declared = list(chains)
    if chains is CEILING_CHAINS:
        declared.extend(("order-independent marker", (pattern,)) for pattern in EXPECTED_UNORDERED)
    match_marker_contract(
        transcript,
        declared,
        FAILURE_MARKERS,
        fail,
    )
    if re.search(r"SLIME_MEM (?:capacity|qualification)", transcript):
        fail("normal private-memory transcript contains workload qualification")


def check_reuse_cycles(transcript: str, declared_pages: int) -> None:
    """MEM-64M's reuse clause: twenty 64 MiB lives, no drift, no stale bytes.

    The chains above establish what one cycle promises. This establishes the
    aggregate the milestone actually requires, which no per-cycle marker can:
    that the count is twenty, that both termination paths occur, and that the
    allocator's own watermarks come back to the same values after every one of
    them.

    The drift check reads the root's `reclaim census`, whose three resource
    figures are taken from the allocator's watermarks rather than from the
    counters the reclamation path maintains. That distinction is B9's lesson
    and the reason this arm can fail: B9's leak had every root-maintained tally
    agreeing with every other one while thirteen frames per spawn were never
    returned. A census read from the same bookkeeping would have agreed too.
    """
    prefix = "reuse cycles: "
    reports = re.findall(
        r"\[private-cycle-probe\] cycle=(\d+) pages=(\d+) base=0x([0-9a-f]+) "
        r"stamp=0x([0-9a-f]+) zeroed=1 verified=1 end=(exit|fault)",
        transcript,
    )
    if len(reports) != CYCLE_COUNT:
        fail(prefix + f"{len(reports)} incarnation(s) reported, expected {CYCLE_COUNT}")
    cycles = [int(cycle) for cycle, *_ in reports]
    if cycles != list(range(CYCLE_COUNT)):
        fail(prefix + f"incarnations ran out of order or repeated: {cycles}")
    for cycle, pages, _base, _stamp, end in reports:
        if int(pages) != declared_pages:
            fail(prefix + f"cycle {cycle} grew {pages} page(s), expected {declared_pages}")
        # The path is a property of the cycle number, so a probe that ended
        # every life the same way fails here rather than satisfying the count.
        expected_end = "exit" if int(cycle) % 2 == 0 else "fault"
        if end != expected_end:
            fail(prefix + f"cycle {cycle} ended by {end}, expected {expected_end}")
    # Distinct stamps: a page carried from one life into the next would satisfy
    # a non-zero test, so the stamp identifies *which* life wrote it. Equal
    # stamps would make the zero test above unable to tell reuse from leak.
    stamps = {stamp for _c, _p, _b, stamp, _e in reports}
    if len(stamps) != CYCLE_COUNT:
        fail(prefix + f"{len(stamps)} distinct stamp(s) across {CYCLE_COUNT} incarnations")
    # One region, reused. The base is the holder's declared window, so every
    # incarnation must be served at the same address: a moving base would mean
    # the root was handing out fresh address space rather than reclaiming.
    bases = {base for _c, _p, base, _s, _e in reports}
    if len(bases) != 1:
        fail(prefix + f"incarnations were served {len(bases)} distinct bases: {sorted(bases)}")

    exits = len(re.findall(r"SLIME_GRAPH component exit task=\d+ status=0", transcript))
    # Every fault is a memory fault. Counting the `VirtualMemory` form against
    # the *total* refuses a run where nine holders died on a memory access and
    # one died of a syscall or user exception -- which is the distinction this
    # arm can make. The access field is not asserted: see `CYCLE_CHAINS`, where
    # the observed decode and the reason it is left open are recorded.
    faults = len(re.findall(r"SLIME_GRAPH component fault task=\d+ kind=", transcript))
    memory_faults = re.findall(
        r"SLIME_GRAPH component fault task=\d+ kind=VirtualMemory "
        r"\{ access: \w+, status: \d+ \} address=Some\((\d+)\)",
        transcript,
    )
    # Init exits too, so its own clean exit is the one beyond the ten cycles.
    if exits != CYCLE_COUNT // 2 + 1:
        fail(prefix + f"{exits} clean exit(s), expected {CYCLE_COUNT // 2} cycles plus init")
    if faults != CYCLE_COUNT // 2:
        fail(prefix + f"{faults} fault(s), expected {CYCLE_COUNT // 2}")
    if len(memory_faults) != faults:
        fail(
            prefix + f"{len(memory_faults)} of {faults} fault(s) were memory faults; "
            "the rest ended some other way"
        )
    # Each faulting holder reached its own report first, so the fault followed a
    # fully committed quota rather than killing the task during construction.
    # That is the property the milestone's clause needs -- reuse *after* a
    # deliberate fault with the whole working set live -- and it is established
    # by the per-cycle reports above being twenty, ten of them `end=fault`.
    faulting_reports = sum(1 for *_rest, end in reports if end == "fault")
    if faulting_reports != faults:
        fail(
            prefix + f"{faulting_reports} holder(s) reported a committed quota before "
            f"faulting, against {faults} fault(s)"
        )

    census = re.findall(
        r"SLIME_ROOT reclaim census task=(\d+) slots=(\d+) bytes=(\d+) "
        r"live_objects=(\d+) extent_reuses=(\d+)",
        transcript,
    )
    # Init's own exit is reclaimed too, so its census record is the extra one
    # and it is *not* a cycle: it returns a task that never held a private
    # quota, so its figures legitimately differ. Dropping the last record rather
    # than filtering by task id keeps that explicit — init exits after the loop,
    # which the `exits=10 faults=10` marker preceding it establishes.
    if len(census) != CYCLE_COUNT + 1:
        fail(
            prefix + f"{len(census)} reclamation census record(s), expected "
            f"{CYCLE_COUNT} cycles plus init"
        )
    census = census[:CYCLE_COUNT]
    # The first cycle is the baseline: it ran against an allocator no holder had
    # yet returned anything to, so its remaining figures are what every later
    # reclamation must restore. Exact equality rather than a tolerance — a
    # bounded root that returns all but one slot per cycle is still a root whose
    # capacity falls with uptime.
    _, base_slots, base_bytes, base_objects, _ = census[0]
    for index, (task, slots, byte_count, objects, _reuses) in enumerate(census[1:], start=1):
        if slots != base_slots:
            fail(
                prefix + f"cycle {index} (task {task}) left {slots} slot(s) remaining "
                f"against {base_slots} after the first: reclaimed CSlots are not returned"
            )
        if byte_count != base_bytes:
            fail(
                prefix + f"cycle {index} (task {task}) left {byte_count} untyped byte(s) "
                f"remaining against {base_bytes} after the first: backing is not returned"
            )
        if objects != base_objects:
            fail(
                prefix + f"cycle {index} (task {task}) left {objects} live object(s) "
                f"against {base_objects} after the first: kernel objects are not returned"
            )
    # Retention is proved by reuse dominating fresh records, the same reading
    # `check-sel4-reclamation-plane.py` established: `release_task_arena`
    # deactivates an extent record so `provision_extent` can re-retype into it,
    # so the free count deliberately does not return and a collapse in reuse is
    # what a retention regression looks like.
    reuses = [int(reuse) for *_rest, reuse in census]
    if reuses != sorted(reuses):
        fail(prefix + f"extent reuse count decreased across cycles: {reuses}")
    if reuses[-1] <= reuses[0]:
        fail(
            prefix + f"extent reuses did not grow across {CYCLE_COUNT} cycles "
            f"({reuses[0]} -> {reuses[-1]}): released backing is not being reused"
        )
    # Every incarnation's growth started from an empty region and took the large
    # frames the first one did. Counted rather than only chained, so a run where
    # nineteen cycles degraded to base pages fails here.
    grown = re.findall(
        rf"SLIME_MEM grown task=\d+ delta={declared_pages} previous=0 pages={declared_pages} "
        r"base=0x[0-9a-f]+ quota=\d+ total=\d+ large_frames=(\d+) base_frames=(\d+) "
        r"leaf_tables=(\d+)",
        transcript,
    )
    if len(grown) != CYCLE_COUNT:
        fail(prefix + f"{len(grown)} full growth(s) from an empty region, expected {CYCLE_COUNT}")
    shapes = set(grown)
    if len(shapes) != 1:
        fail(prefix + f"growths took {len(shapes)} distinct backing shapes: {sorted(shapes)}")
    large, base_frames, leaf_tables = grown[0]
    if (int(large), int(base_frames), int(leaf_tables)) != (declared_pages // 512, 0, 0):
        fail(
            prefix + f"a reused growth was backed by large_frames={large} "
            f"base_frames={base_frames} leaf_tables={leaf_tables}"
        )
    # No private page survives the last holder, and released backing is still
    # held for reuse: the terminal accounting's own two fields, on the
    # reclamation gate's reading of them.
    terminal = re.search(
        r"SLIME_ROOT allocator live_slots=\d+ free_slots=\d+ live_objects=\d+ live_bytes=\d+ "
        r"mapped_ram=(\d+) reusable_ram=(\d+) reusable_private_ram=(\d+) ",
        transcript,
    )
    if terminal is None:
        fail(prefix + "the root reported no terminal allocator accounting")
    if terminal.group(1) != "0":
        fail(prefix + f"private mapped RAM survived every holder: {terminal.group(1)}")
    if int(terminal.group(3)) == 0:
        fail(prefix + "no private-backing extent was retained for reuse")


def check_declared_is_installed(transcript: str, declared: dict[str, int]) -> None:
    """Every declared instance's installed ceiling is the one the budget names.

    Over every `SLIME_MEM quota` record rather than only the granted one, so an
    instance the fixture omits is checked to have received zero rather than
    simply not checked.
    """
    records = re.findall(
        r"SLIME_MEM quota task=\d+ instance=(\S+) "
        r"declared=(\d+) installed=(\d+) base=(0x[0-9a-f]+)",
        transcript,
    )
    if not records:
        fail("the root reported no installed private-memory ceilings")
    for instance, reported, installed, base in records:
        expected = declared.get(instance, 0)
        if int(reported) != expected:
            fail(
                f"{instance}: the root read a declared quota of {reported}, but "
                f"the fixture declares {expected}"
            )
        if int(installed) != expected:
            fail(
                f"{instance}: the generation declares {expected} page(s) but the "
                f"root installed {installed}"
            )
        # Deny-by-default is structural: a holder with no quota carries no
        # window either, so a nonzero base for one would mean the reservation
        # outlived the authority to use it.
        if expected == 0 and int(base, 16) != 0:
            fail(f"{instance}: no declared quota but a reserved window at {base}")
        if expected > 0 and int(base, 16) == 0:
            fail(f"{instance}: declared {expected} page(s) but no reserved window")


def check_measured_ceiling(transcript: str, declared: dict[str, int]) -> None:
    """The probe's *measured* ceiling equals the declared one.

    The granted instance maps the complete declared span and reports it without
    reading the manifest. Its next page is refused by the structural
    reservation, whose precedence over quota is itself part of the public grow
    contract. Agreement here therefore proves the declared ceiling is both
    reachable and exactly the fixed window extent.
    """
    expected = declared["private-memory-granted"]
    measured = re.search(
        r"\[private-memory-probe\] granted pages=(\d+) base=(0x[0-9a-f]+) ",
        transcript,
    )
    if measured is None:
        fail("the granted probe reported no measured ceiling")
    if int(measured.group(1)) != expected:
        fail(
            f"the granted probe grew to {measured.group(1)} page(s) against a "
            f"declared quota of {expected}"
        )
    # Both probes run in unconstrained order and the denied one emits the same
    # `ReservationExceeded` shape with pages=0, so the refusal must be scoped
    # to the granted task's own id rather than taken as the transcript's first
    # match. Captured here in the same search as the installed base, which the
    # refusal must also agree with below.
    granted = re.search(
        r"SLIME_MEM quota task=(\d+) instance=private-memory-granted "
        r"declared=\d+ installed=\d+ base=(0x[0-9a-f]+)",
        transcript,
    )
    if granted is None:
        fail("the root reported no installed ceiling for private-memory-granted")
    refusal = re.search(
        rf"SLIME_MEM refused task={granted.group(1)} delta=1 "
        r"cause=reservation detail=ReservationExceeded \{ pages: (\d+), delta: 1, "
        r"reservation: (\d+) \}",
        transcript,
    )
    if refusal is None:
        fail(
            f"private-memory-granted (task {granted.group(1)}) recorded no "
            "full-window reservation refusal"
        )
    if int(refusal.group(1)) != expected or int(refusal.group(2)) != expected:
        fail(
            f"the refusal names pages={refusal.group(1)} reservation={refusal.group(2)}, "
            f"expected both to be {expected}"
        )
    # The base the root reported installing and the base the probe dereferenced
    # must be the same address. Without this the two halves could each be
    # self-consistent about a different window.
    if granted.group(2) != measured.group(2):
        fail(
            f"the root installed a window at {granted.group(2)} but the probe "
            f"used {measured.group(2)}"
        )


def check_only_declared_pages_were_charged(transcript: str, declared: dict[str, int]) -> None:
    """Every page charged went to the holder that declared it, within its ceiling.

    Per *instance*, not per task: the `SLIME_MEM grown` record carries only a
    task id, so summing by that id alone would prove the right total was charged
    without proving it went to the right holder — the omitted holder growing a
    page the granted holder then did not would reach the same sum. The task id is
    resolved to an instance name through the root's own `SLIME_MEM quota`
    records, which name both, so the attribution is checked rather than assumed.

    Queries (`delta=0`) and refusals must charge nothing, so a holder's summed
    served deltas are exactly what its declared quota authorized. A mechanism
    that charged a page for a query would reach the right final page count by a
    different and wrong route.

    Bounded rather than equal, on C10.3: the C10.2 probes grow one page at a time
    until refused, so they necessarily land on their exact ceiling, but a
    component allocating through the C10.3 allocator takes only the pages its
    collections need — 22 of 24 on this plane. Requiring equality there would
    make the gate fail whenever the allocator's batching policy changed, which is
    the one thing C10.3 deliberately left in userspace. So: never above the
    ceiling, and never zero for a holder that declared one, because a holder
    charged nothing at all is the milestone silently not working.
    """
    names = dict(
        re.findall(
            r"SLIME_MEM quota task=(\d+) instance=(\S+) declared=\d+ installed=\d+ ",
            transcript,
        )
    )
    if not names:
        fail("no task-to-instance mapping was reported, so no charge can be attributed")
    charged: dict[str, int] = {name: 0 for name in names.values()}
    for task, delta, previous, pages in re.findall(
        r"SLIME_MEM grown task=(\d+) delta=(\d+) previous=(\d+) pages=(\d+) ",
        transcript,
    ):
        instance = names.get(task)
        if instance is None:
            fail(f"task {task} was charged {delta} page(s) but names no declared instance")
        charged[instance] += int(delta)
        # Each record must be internally consistent, which is what makes the sum
        # above meaningful rather than an accumulation of unrelated numbers.
        if int(previous) + int(delta) != int(pages):
            fail(f"{instance}: a growth of {delta} took {previous} page(s) to {pages}")
    for instance, pages in sorted(charged.items()):
        expected = declared.get(instance, 0)
        if pages > expected:
            fail(
                f"{instance}: the root charged {pages} page(s) against a declared "
                f"quota of {expected}"
            )
        if expected > 0 and pages == 0:
            fail(
                f"{instance}: declares {expected} page(s) but was charged none, so "
                "nothing proves the quota is reachable"
            )


def check_growth_was_batched_and_reused(transcript: str, declared: dict[str, int]) -> None:
    """C10.3: the allocator asked in batches and reused what it freed.

    Both halves are read from the *root's* `SLIME_MEM grown` records rather than
    from the probe's report, because the probe's own numbers come from the
    allocator it is testing: one that lost its freed spans and grew again while
    under-counting itself would report a self-consistent lie. The component's
    part is only to bracket the phases, which it cannot fake — the root's
    records fall inside or outside a window, whatever the component claims about
    them.

    Batching: no growth as small as a single page. An allocator asking per
    allocation would make a syscall of every `Vec` push that outgrew its
    capacity — the shape the milestone's second deliverable exists to avoid —
    and its first request is one page for the first small allocation.

    Reuse: no growth between the probe's reuse-phase boundary line and its
    report. That phase frees everything and asks for a comparable amount again,
    so a growth served inside the window is the free list failing to hand the
    memory back, and a component bound by a small declared ceiling that cannot
    reuse memory cannot run past its first burst of allocations.
    """
    holder = "private-heap-granted"
    if holder not in declared:
        fail(f"the fixture declares no quota for {holder}, so C10.3 asserts nothing")
    task = None
    for candidate, instance in re.findall(
        r"SLIME_MEM quota task=(\d+) instance=(\S+) declared=\d+ installed=\d+ ",
        transcript,
    ):
        if instance == holder:
            task = candidate
    if task is None:
        fail(f"the root reported no installed ceiling for {holder}")
    served = [
        int(delta)
        for delta in re.findall(
            rf"SLIME_MEM grown task={task} delta=([1-9]\d*) ",
            transcript,
        )
    ]
    if not served:
        fail(f"{holder}: the allocator never grew its region")
    # The *minimum*, not the maximum. A purely demand-driven allocator asking
    # one page per allocation would still show a large growth for the probe's
    # biggest single reallocation, so `max(served)` passes without asserting
    # anything. Its *first* growth, though, is one page for the first tiny
    # `Vec` element, and a batching policy has no growth smaller than its batch
    # floor. Testing the minimum discriminates, and does so independently of
    # whatever `GROWTH_PAGES` happens to be.
    if min(served) < 2:
        fail(
            f"{holder}: a growth of one page ({served}), so the allocator is "
            "asking per allocation rather than in batches"
        )
    boundary = re.search(
        r"\[private-heap-probe:granted\] private-heap reuse phase begins",
        transcript,
    )
    if boundary is None:
        fail(f"{holder}: the self-check never entered its reuse phase")
    report = re.search(
        r"\[private-heap-probe:granted\] private-heap quota live pages=(\d+) growths=(\d+) ",
        transcript,
    )
    if report is None:
        fail(f"{holder}: the startup self-check did not report")
    # Scoped to the self-check's own window. MEM-64M's capacity case runs
    # *after* the report and legitimately grows the region to hold 60 MiB, so
    # comparing the component's self-check counters against every growth in the
    # transcript would charge it for a later phase it never claimed to cover.
    # The capacity phase has its own evidence in
    # `check_heap_capacity_is_within_the_declared_quota`.
    self_check = [
        int(delta)
        for delta in re.findall(
            rf"SLIME_MEM grown task={task} delta=([1-9]\d*) ",
            transcript[: report.end()],
        )
    ]
    if len(self_check) != int(report.group(2)):
        fail(
            f"{holder}: the root served {len(self_check)} growth(s) before the "
            f"self-check reported but the component counted {report.group(2)}; "
            "the two accounts must agree"
        )
    if sum(self_check) != int(report.group(1)):
        fail(
            f"{holder}: the root served {sum(self_check)} page(s) but the component "
            f"reports {report.group(1)} backed"
        )
    # Reuse, asserted against the root's own records rather than the component's
    # `reuse_growths` field: that field is produced by the allocator under test,
    # so one that lost the freed spans and grew again could under-count itself
    # into agreement. The probe frees everything and reallocates a comparable
    # amount between these two lines, so a growth served in that window is the
    # free list failing to give the memory back.
    reuse_window = transcript[boundary.end() : report.start()]
    during = re.findall(rf"SLIME_MEM grown task={task} delta=([1-9]\d*) ", reuse_window)
    if during:
        fail(
            f"{holder}: the root served {during} more page(s) during the reuse "
            "phase, so freed memory was not handed out again"
        )
    # Past the capacity report is the deliberate over-ceiling request, which
    # must be refused rather than served.
    capacity = re.search(
        r"\[private-heap-probe:granted\] capacity payload=\d+ overhead=\d+ "
        r"backed=\d+ pages=\d+ touched=1",
        transcript,
    )
    if capacity is None:
        fail(f"{holder}: the runtime-heap capacity phase did not report")
    late = re.findall(
        rf"SLIME_MEM grown task={task} delta=([1-9]\d*) ",
        transcript[capacity.end() :],
    )
    if late:
        fail(
            f"{holder}: the root served {late} more page(s) after the capacity "
            "phase, so the over-ceiling request was satisfied rather than refused"
        )


def check_heap_capacity_is_within_the_declared_quota(
    transcript: str, declared: dict[str, int]
) -> None:
    """MEM-64M: the 60 MiB runtime-heap payload, its overhead, and its capacity.

    The milestone requires a separate ordinary runtime-heap case holding 60 MiB
    of payload within the declared quota, with payload, allocator overhead, and
    remaining capacity reported separately. The component can only state what it
    observes — it has no quota query — so the quota half comes from the root's
    own `SLIME_MEM quota declared=` record and is joined here. That join is the
    point: a holder reporting 60 MiB of payload proves nothing about a *bound*
    unless the bound it fits inside is the one the generation declared.
    """
    holder = "private-heap-granted"
    quota_pages = declared[holder]
    report = re.search(
        r"\[private-heap-probe:granted\] capacity payload=(\d+) overhead=(\d+) "
        r"backed=(\d+) pages=(\d+) touched=1",
        transcript,
    )
    if report is None:
        fail(f"{holder}: no runtime-heap capacity report")
    payload, overhead, backed, pages = (int(report.group(index)) for index in range(1, 5))
    if payload != 60 * 1024 * 1024:
        fail(f"{holder}: payload is {payload} bytes, expected exactly 60 MiB")
    if backed != pages * 4096:
        fail(f"{holder}: backed {backed} bytes disagrees with {pages} page(s)")
    if pages > quota_pages:
        fail(
            f"{holder}: holds {pages} page(s) against a declared quota of "
            f"{quota_pages}, so the ceiling did not bind"
        )
    if payload + overhead > pages * 4096:
        fail(
            f"{holder}: payload plus overhead ({payload + overhead}) exceeds the "
            f"{pages * 4096} bytes the root actually backed"
        )
    # The payload must genuinely need most of the quota, or "60 MiB inside the
    # ceiling" would hold for a ceiling of any size and qualify nothing.
    if pages * 4096 * 2 < quota_pages * 4096:
        fail(
            f"{holder}: 60 MiB of payload occupies {pages} of {quota_pages} "
            "declared page(s), so the case does not exercise the raised ceiling"
        )


def check_heap_refusal_is_the_declared_quota(
    transcript: str, declared: dict[str, int], platform: str
) -> None:
    """The heap holder crosses its declared quota without crossing its reservation."""
    holder = "private-heap-granted"
    quota = declared[holder]
    target = TARGET_PROFILES[platform]
    reservation = PRIVATE_MEMORY_CAPACITY_PROFILES[target][0]
    if quota >= reservation:
        fail(
            f"{holder}: declared quota {quota} must be below its {reservation}-page "
            "reservation for the quota refusal to be distinguishable"
        )
    installed = re.search(
        rf"SLIME_MEM quota task=(\d+) instance={holder} "
        rf"declared={quota} installed={quota} base=0x[0-9a-f]+",
        transcript,
    )
    if installed is None:
        fail(f"{holder}: no exact declared/installed quota record for {quota} pages")
    refusal = re.search(
        rf"SLIME_MEM refused task={installed.group(1)} delta=(\d+) cause=quota "
        r"detail=QuotaExceeded \{ pages: (\d+), delta: (\d+), quota: (\d+) \}",
        transcript,
    )
    if refusal is None:
        fail(f"{holder}: no cause=quota/QuotaExceeded refusal from its own task")
    delta, pages, detail_delta, detail_quota = (
        int(refusal.group(index)) for index in range(1, 5)
    )
    if delta != detail_delta:
        fail(f"{holder}: refusal delta={delta} but detail delta={detail_delta}")
    if detail_quota != quota:
        fail(
            f"{holder}: refusal names quota {detail_quota}, but the fixture declares {quota}"
        )
    capacity = re.search(
        r"\[private-heap-probe:granted\] capacity payload=\d+ overhead=\d+ "
        r"backed=\d+ pages=(\d+) touched=1",
        transcript,
    )
    if capacity is None:
        fail(f"{holder}: no backed-page measurement before its quota refusal")
    if pages != int(capacity.group(1)):
        fail(
            f"{holder}: refusal began at {pages} pages, but the allocator reported "
            f"{capacity.group(1)} backed before the request"
        )
    if pages > quota:
        fail(f"{holder}: refusal began at {pages} pages above declared quota {quota}")
    if pages + delta <= quota:
        fail(
            f"{holder}: request to {pages + delta} pages does not cross declared quota {quota}"
        )
    if pages + delta != reservation:
        fail(
            f"{holder}: refusal request covers {pages}+{delta} pages, expected exactly "
            f"the {reservation}-page reservation"
        )


def check_the_two_planes_are_independent(transcript: str, declared: dict[str, int]) -> None:
    """C10.4: exhausting one memory plane leaves the other's ceiling intact.

    The holder the fixture names in *both* budgets is the only instance that can
    state this. It runs each plane to its declared limit and then uses the other,
    so a shared account would be drained by whichever went first and the second
    use would be refused. The probe's own FAIL lines name that outcome
    specifically, and `FAILURE_MARKERS` catches them; this asserts the positive
    half from the root's records.

    Two properties, both read from the root rather than from the component:

    * the mapping refusal names a destination *inside* the window the same
      record reports, so the root refused it for the stated reason rather than
      for an unrelated one that happened to produce a refusal;
    * the pages charged to this holder stayed within its private quota while its
      buffer pages were charged separately — the two numbers come from two
      different root subsystems, and neither may have paid for the other.
    """
    holder = "private-heap-both"
    if holder not in declared:
        fail(f"the fixture declares no private quota for {holder}, so C10.4 asserts nothing")
    refusal = re.search(
        r"SLIME_MEM mapping refused task=(?P<task>\d+) base=0x(?P<base>[0-9a-f]+) "
        r"end=0x(?P<end>[0-9a-f]+) window=0x(?P<start>[0-9a-f]+)\.\.0x(?P<stop>[0-9a-f]+)",
        transcript,
    )
    if refusal is None:
        fail(f"{holder}: the root never refused a mapping into a private window")
    base = int(refusal.group("base"), 16)
    end = int(refusal.group("end"), 16)
    start = int(refusal.group("start"), 16)
    stop = int(refusal.group("stop"), 16)
    # The refusal must actually be about the window. A root that refused every
    # mapping, or refused this one for an unrelated reason while printing the
    # marker anyway, would satisfy the chain's presence check but not this.
    if not (base < stop and start < end):
        fail(
            f"{holder}: the root refused a mapping at {base:#x}..{end:#x} as "
            f"overlapping {start:#x}..{stop:#x}, which it does not"
        )
    if start != base:
        fail(
            f"{holder}: the refused mapping was at {base:#x} rather than the "
            f"window's own base {start:#x}, so it does not prove the page the "
            "allocator already holds is defended"
        )
    # And the same buffer mapped somewhere else, which is what makes the refusal
    # about the destination rather than about the buffer or this holder's rights.
    report = re.search(
        r"\[private-heap-probe:both\] both pages=(?P<pages>\d+) growths=(?P<growths>\d+) "
        r"buffers=1 window_map_refused=1 outside_map=1 released=1 reused=1",
        transcript,
    )
    if report is None:
        fail(f"{holder}: the holder of both planes did not report")
    pages = int(report.group("pages"))
    if pages > declared[holder]:
        fail(
            f"{holder}: holds {pages} private page(s) against a declared quota "
            f"of {declared[holder]}"
        )
    if pages == 0:
        fail(
            f"{holder}: declares {declared[holder]} private page(s) but holds "
            "none, so nothing proves the two planes were both in use at once"
        )
    # The buffer plane's own charge, from the root's separate record. Both
    # numbers nonzero at once is the whole claim: one component, two accounts,
    # neither paying for the other.
    buffer_charge = re.search(
        rf"SLIME_GRAPH quota task=\d+ instance={re.escape(holder)} executable=\S+ "
        r"pages=(?P<pages>\d+) buffers=(?P<buffers>\d+) mappings=\d+ loans=\d+",
        transcript,
    )
    if buffer_charge is None:
        fail(f"{holder}: the root reported no shared-buffer ceiling for this holder")
    if int(buffer_charge.group("buffers")) == 0:
        fail(
            f"{holder}: the root installed no buffer allowance, so the plane "
            "cannot show the two accounts are separate"
        )


def check_segmented_capacity_report(
    transcript: str, profile: dict[str, object], section: str
) -> None:
    prefix = "capacity qualification: "
    pattern = re.compile(
        r"^SLIME_MEM qualification scope=(?P<scope>\S+) holders=(?P<holders>\d+) "
        r"pages=(?P<pages>\d+) private_allocations=(?P<private_allocations>\d+) "
        r"private_extents=(?P<private_extents>\d+) private_cslots=(?P<private_cslots>\d+) "
        r"private_reserved=(?P<private_reserved>\d+) payload=(?P<payload>\d+) "
        r"tables=(?P<tables>\d+) alignment=(?P<alignment>\d+) "
        r"static_allocations=(?P<static_allocations>\d+) static_reserved=(?P<static_reserved>\d+) "
        r"required_allocations=(?P<required_allocations>\d+) required_extents=(?P<required_extents>\d+) "
        r"required_cslots=(?P<required_cslots>\d+) required_reserved=(?P<required_reserved>\d+) "
        r"allocation_capacity=(?P<allocation_capacity>\d+) allocations_available=(?P<allocations_available>\d+) "
        r"extent_capacity=(?P<extent_capacity>\d+) extents_available=(?P<extents_available>\d+) "
        r"cslots_available=(?P<cslots_available>\d+) ordinary_available=(?P<ordinary_available>\d+) "
        r"ordinary_layout=(?P<ordinary_layout>\d+) root_image=(?P<image>\d+) "
        r"root_metadata=(?P<metadata>\d+) root_stack=(?P<stack>\d+) "
        r"root_heap=(?P<heap>\d+) fit=(?P<fit>\d+)$",
        re.MULTILINE,
    )
    reports = list(pattern.finditer(transcript))
    qualification_lines = re.findall(r"^SLIME_MEM qualification\b.*$", transcript, re.MULTILINE)
    if len(reports) != 1 or len(qualification_lines) != 1:
        fail(prefix + f"expected exactly one complete report, found {len(qualification_lines)}")
    report = reports[0]
    values: dict[str, int] = {}
    for name, token in report.groupdict().items():
        if name == "scope":
            continue
        value = int(token)
        if value > 2**64 - 1:
            fail(prefix + f"{name} exceeds u64")
        values[name] = value
    if report.group("scope") != "staged-graph-plus-admitted-holder-clones":
        fail(prefix + "scope mismatch")
    # The envelope the contract actually admits for this target: 16384 pages
    # (64 MiB) per holder, and the two holders the 32768-page (128 MiB)
    # aggregate admits at that size. These are the numbers MEM-64M qualifies;
    # a larger claim belongs to the milestone that runs its workload.
    expected_private = {
        "holders": 2,
        "pages": 16_384,
        "private_allocations": 32_896,
        "private_extents": 130,
        "private_cslots": 33_026,
        "private_reserved": 134_479_872,
        "payload": 134_217_728,
        "tables": 262_144,
        "alignment": 0,
    }
    for name, expected in expected_private.items():
        if values[name] != expected:
            fail(prefix + f"{name}={values[name]}, expected {expected}")
    if values["static_allocations"] < 8:
        fail(prefix + "static_allocations is below the exemplar minimum")
    if values["static_reserved"] == 0:
        fail(prefix + "static_reserved is zero")
    expected_required = {
        "required_allocations": values["private_allocations"]
        + values["static_allocations"] * values["holders"],
        "required_extents": values["private_extents"],
        "required_cslots": values["private_cslots"]
        + values["static_allocations"] * values["holders"],
        "required_reserved": values["private_reserved"]
        + values["static_reserved"] * values["holders"],
    }
    for name, expected in expected_required.items():
        if values[name] != expected:
            fail(prefix + f"{name}={values[name]}, expected {expected}")
    if values["allocation_capacity"] < values["required_allocations"] or values["extent_capacity"] < values["required_extents"]:
        fail(prefix + "platform descriptor tables cannot represent the admitted holders")
    if not 0 <= values["allocations_available"] < values["allocation_capacity"]:
        fail(prefix + "allocations_available does not reflect staged graph use")
    if not 0 <= values["extents_available"] < values["extent_capacity"]:
        fail(prefix + "extents_available does not reflect staged graph use")
    comparisons = (
        values["required_allocations"] <= values["allocations_available"],
        values["required_extents"] <= values["extents_available"],
        values["required_cslots"] <= values["cslots_available"],
        values["required_reserved"] <= values["ordinary_available"],
        values["ordinary_layout"] == 1,
    )
    if values["ordinary_layout"] not in (0, 1):
        fail(prefix + "ordinary_layout is not 0 or 1")
    if values["fit"] not in (0, 1):
        fail(prefix + "fit is not 0 or 1")
    if values["fit"] != int(all(comparisons)):
        fail(prefix + "fit disagrees with required-versus-available resources")
    if values["fit"] != 1:
        fail(prefix + "admitted holders do not fit the live platform resources")
    platform_bytes = profile_integer(profile, "memory_mib", fail, section) * 1024 * 1024
    # This milestone qualifies the fixed 2 GiB platform, not a larger profile.
    # Live ordinary availability already excludes the root image (including its
    # metadata, stack and heap), kernel objects and staged graph reservations.
    if platform_bytes != 2 * 1024**3:
        fail(prefix + "qualification must boot the 2 GiB platform")
    if values["image"] == 0 or values["image"] > platform_bytes:
        fail(prefix + "root_image is outside the platform envelope")
    if values["metadata"] == 0:
        fail(prefix + "root_metadata is zero")
    if values["stack"] != 1_048_576 or values["heap"] != 524_288:
        fail(prefix + "root stack or heap diagnostic changed")

def run_cycles_arm() -> None:
    """MEM-64M's reuse clause on its own composition.

    A separate composition rather than a fourth instance on the ceiling plane,
    and that is forced rather than chosen: the private-memory budget's
    aggregate rule sums *declared* quotas, and `sel4-private-memory` already
    declares 32280 of the target's 32768-page ceiling. A 16384-page holder
    beside them is refused at admission, before any component runs.
    """
    declared = declared_quotas(CYCLES_FIXTURE)
    quota = declared.get("private-cycle-probe")
    if quota is None:
        fail("the cycles fixture declares no quota for private-cycle-probe")
    section, qemu_binary = PLATFORMS[CLOSURE_PLATFORM]
    profile = load_qemu_profile(fail, PINS, section)
    transcript = boot(
        profile,
        section=section,
        qemu_binary=qemu_binary,
        image=build_named_image(CYCLES_CLOSURE),
    )
    check_markers(transcript, CYCLE_CHAINS)
    check_declared_is_installed(transcript, declared)
    check_reuse_cycles(transcript, quota)
    print(
        "seL4 private-memory plane check: "
        f"{marker_count(CYCLE_CHAINS)} markers across {len(CYCLE_CHAINS)} causal "
        f"chains and 1 image case on {CLOSURE_PLATFORM}; the declared quota "
        f"({quota} page(s)) was reclaimed and re-served {CYCLE_COUNT} times over "
        f"{CYCLE_COUNT // 2} clean exits and {CYCLE_COUNT // 2} deliberate faults, "
        "every served page zero, with no drift in reusable slots, untyped bytes, "
        "or live objects"
    )


def main() -> None:
    parser = argparse.ArgumentParser(description="Check mixed-size private memory on seL4")
    parser.add_argument(
        "--arm",
        choices=("ceiling", "cycles"),
        default="ceiling",
        help="which qualification to run: the declared ceiling or MEM-64M's reuse cycles",
    )
    parser.add_argument(
        "--platform",
        choices=sorted(PLATFORMS),
        default="qemu-arm-virt",
        help="the pinned QEMU profile and image to build and boot",
    )
    arguments = parser.parse_args()
    if arguments.arm == "cycles":
        if arguments.platform != CLOSURE_PLATFORM:
            fail(f"the cycles arm is closure-backed and runs only on {CLOSURE_PLATFORM}")
        run_cycles_arm()
        return
    declared = declared_quotas()
    section, qemu_binary = PLATFORMS[arguments.platform]
    build_image(arguments.platform)
    image = image_path(arguments.platform)
    if image is None or not image.is_file():
        fail(f"missing packaged image for {arguments.platform}")
    profile = load_qemu_profile(fail, PINS, section)
    transcript = boot(
        profile,
        section=section,
        qemu_binary=qemu_binary,
        image=image,
    )
    check_markers(transcript)
    check_declared_is_installed(transcript, declared)
    check_measured_ceiling(transcript, declared)
    check_only_declared_pages_were_charged(transcript, declared)
    check_growth_was_batched_and_reused(transcript, declared)
    check_heap_capacity_is_within_the_declared_quota(transcript, declared)
    check_heap_refusal_is_the_declared_quota(transcript, declared, arguments.platform)
    check_the_two_planes_are_independent(transcript, declared)
    if arguments.platform == CLOSURE_PLATFORM:
        large_map = boot(
            profile,
            section=section,
            qemu_binary=qemu_binary,
            image=build_named_image(LARGE_MAP_CLOSURE),
        )
        check_large_map_retry(large_map)
        check_declared_is_installed(large_map, declared)
        check_measured_ceiling(large_map, declared)
        check_only_declared_pages_were_charged(large_map, declared)
        check_segmented_capacity_report(large_map, profile, section)
        if any(re.search(pattern, large_map) for pattern in FAILURE_MARKERS):
            fail("large-map qualification contains an explicit failure marker")
        if re.search(r"SLIME_GRAPH HEALTHY generation=\d+ required=\d+ live=\d+ completed=\d+ failed=0", large_map) is None:
            fail("large-map qualification did not reach a healthy graph terminal")
        rollback = boot(
            profile,
            section=section,
            qemu_binary=qemu_binary,
            image=build_named_image(ROLLBACK_CLOSURE),
        )
        check_incremental_rollback(rollback)
    cases = 3 if arguments.platform == CLOSURE_PLATFORM else 1
    # This arm's own markers, not the module's union: the union exists for
    # `sel4_gate_control_check`'s coverage pin, and reporting it here would
    # claim the cycles arm's evidence for a run that never booted its image.
    ceiling_chains = chains_from_gate(sys.modules[__name__])[: len(CEILING_CHAINS)]
    ceiling_chains += tuple(
        ("order-independent marker", (pattern,)) for pattern in EXPECTED_UNORDERED
    )
    print(
        "seL4 private-memory plane check: "
        f"{marker_count(ceiling_chains)} markers across "
        f"{len(ceiling_chains)} causal chains and {cases} image case(s) on "
        f"{arguments.platform}; "
        f"the declared quota ({declared['private-memory-granted']} page(s)) is the "
        "measured ceiling, worker RPC remained exactly-once, a worker's own growth "
        "was adjudicated against its task's region, failed large-map backing "
        "retried, and incremental rollback preserved committed bytes"
    )


if __name__ == "__main__":
    main()
