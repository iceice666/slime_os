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
CLOSURE_PLATFORM = "qemu-arm-virt"
BUILD_SCRIPT = ROOT / "scripts" / "build" / "build-sel4.py"
RV64_IMAGE = ROOT / "build" / "slime-sel4-private-memory-qemu-riscv-virt.elf"
PLATFORMS = {
    "qemu-arm-virt": ("qemu_arm_virt", "qemu-system-aarch64"),
    "qemu-riscv-virt": ("qemu_riscv_virt", "qemu-system-riscv64"),
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
CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
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
        "the granted holder reached its declared ceiling and was then refused",
        (
            r"SLIME_MEM grown task=\d+ delta=512 previous=0 pages=512 "
            r"base=0x[0-9a-f]+ quota=512 total=\d+ large_frames=1 base_frames=0 leaf_tables=0",
            r"SLIME_MEM refused task=\d+ delta=1 cause=reservation "
            r"detail=ReservationExceeded \{ pages: (\d+), delta: 1, reservation: (\d+) \}",
            r"\[private-memory-probe\] granted pages=(\d+) base=0x[0-9a-f]+ "
            r"zeroed=1 survived=1 refused=1",
        ),
    ),
    (
        # The omitted probe's own sequence, independent of the granted one's.
        "the omitted holder was refused its first page by the reservation",
        (
            r"SLIME_MEM refused task=\d+ delta=512 cause=reservation "
            r"detail=ReservationExceeded \{ pages: 0, delta: 512, reservation: 0 \}",
            r"\[private-memory-probe\] denied pages=0 base=0x0 refused=1",
        ),
    ),
    (
        # C10.3's granted holder: the self-check enters its reuse phase, reports,
        # then the deliberate over-allocation is refused and the component
        # survives to report again. Causal within the chain — the refusal cannot
        # precede the check that established the heap works.
        #
        # The reuse boundary is a required marker as well as the window
        # `check_growth_was_batched_and_reused` measures growth in, so a probe
        # that stopped emitting it fails here rather than silently making that
        # window empty and its assertion vacuous.
        "the granted holder allocated through ordinary collections, then hit its ceiling",
        (
            r"\[private-heap-probe:granted\] private-heap reuse phase begins",
            r"\[private-heap-probe:granted\] private-heap quota live pages=(\d+) "
            r"growths=(\d+) reuse_growths=0 leaked=0",
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

# A size query allocates nothing and both probes issue one, so this marker is
# required but its position is not causally ordered against either probe's chain
# — asserting one would again pin which probe the scheduler ran first (B63's
# mechanism for exactly this).
CAPACITY_MARKER = (
    r"SLIME_MEM capacity holders=4 pages=65536 reserved=\d+ payload=\d+ tables=\d+ "
    r"alignment=\d+ mapped=0 reusable=0 allocation_descriptors=\d+ "
    r"allocation_descriptors_available=\d+ extent_descriptors=\d+ "
    r"extent_descriptors_available=\d+ holder_cslots=\d+ graph_cslots=\d+ "
    r"cslots_available=\d+ root_image=\d+ root_metadata=\d+ root_stack=1048576 "
    r"root_heap=524288 ordinary_available=\d+ fit=1"
)
EXPECTED_UNORDERED: tuple[str, ...] = (
    r"\[private-memory-probe\] query pages=0 base=0x[0-9a-f]+",
    CAPACITY_MARKER,
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
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"seL4 private-memory plane check: {message}")


def declared_quotas() -> dict[str, int]:
    """The ceilings `sel4-private-memory.zti` declares, read from the fixture.

    Read rather than restated: the gate's whole assertion is that the
    generation's declaration is the live ceiling, and a copy of the number in
    this file would make that a comparison against itself. Decoded through the
    contract's own schema so a malformed fixture fails here rather than
    producing a plausible dict.
    """
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    process = subprocess.run(
        [str(binary()), "json", str(FIXTURE)],
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


def build_image(platform: str) -> None:
    """Build this plane through its closure or declared cross-target legacy arm."""
    global IMAGE
    if platform == CLOSURE_PLATFORM:
        try:
            built = build_closure_image(CLOSURE)
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
    terminal = re.compile(
        r"SLIME_GRAPH HEALTHY|SLIME_ROOT FATAL|private memory plane fail"
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
        fail("QEMU timed out")
    return "\n".join(lines)


def check_markers(transcript: str) -> None:
    # The shared helper rather than a local loop, on B63's rule: every other
    # plane gate's chain matching, failure vetoing, and out-of-order reporting
    # lives here, and a private reimplementation is a second copy that can drift
    # from the one `sel4_gate_control_check` drives.
    match_marker_contract(
        transcript,
        chains_from_gate(sys.modules[__name__]),
        FAILURE_MARKERS,
        fail,
    )


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
    refusal = re.search(
        r"cause=reservation detail=ReservationExceeded \{ pages: (\d+), delta: 1, "
        r"reservation: (\d+) \}",
        transcript,
    )
    if refusal is None:
        fail("no full-window reservation refusal was recorded")
    if int(refusal.group(1)) != expected or int(refusal.group(2)) != expected:
        fail(
            f"the refusal names pages={refusal.group(1)} reservation={refusal.group(2)}, "
            f"expected both to be {expected}"
        )
    # The base the root reported installing and the base the probe dereferenced
    # must be the same address. Without this the two halves could each be
    # self-consistent about a different window.
    installed = re.search(
        r"SLIME_MEM quota task=\d+ instance=private-memory-granted "
        r"declared=\d+ installed=\d+ base=(0x[0-9a-f]+)",
        transcript,
    )
    if installed is None or installed.group(1) != measured.group(2):
        reported = installed.group(1) if installed else "<none>"
        fail(f"the root installed a window at {reported} but the probe used {measured.group(2)}")


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
    if len(served) != int(report.group(2)):
        fail(
            f"{holder}: the root served {len(served)} growth(s) but the component "
            f"counted {report.group(2)}; the two accounts must agree"
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
    # Past the report is the deliberate over-ceiling request, which must be
    # refused rather than served.
    late = re.findall(
        rf"SLIME_MEM grown task={task} delta=([1-9]\d*) ",
        transcript[report.end() :],
    )
    if late:
        fail(
            f"{holder}: the root served {late} more page(s) after the self-check, "
            "so the over-ceiling request was satisfied rather than refused"
        )
    if sum(served) != int(report.group(1)):
        fail(
            f"{holder}: the root served {sum(served)} page(s) but the component "
            f"reports {report.group(1)} backed"
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
    report = re.search(
        r"SLIME_MEM capacity holders=(?P<holders>\d+) pages=(?P<pages>\d+) "
        r"reserved=(?P<reserved>\d+) payload=(?P<payload>\d+) tables=(?P<tables>\d+) "
        r"alignment=(?P<alignment>\d+) mapped=(?P<mapped>\d+) reusable=(?P<reusable>\d+) "
        r"allocation_descriptors=(?P<allocations>\d+) "
        r"allocation_descriptors_available=(?P<allocations_available>\d+) "
        r"extent_descriptors=(?P<extents>\d+) "
        r"extent_descriptors_available=(?P<extents_available>\d+) "
        r"holder_cslots=(?P<holder_cslots>\d+) graph_cslots=(?P<graph_cslots>\d+) "
        r"cslots_available=(?P<cslots_available>\d+) root_image=(?P<image>\d+) "
        r"root_metadata=(?P<metadata>\d+) root_stack=(?P<stack>\d+) "
        r"root_heap=(?P<heap>\d+) ordinary_available=(?P<ordinary_available>\d+) fit=1",
        transcript,
    )
    if report is None:
        fail("segmented 256 MiB capacity report is missing")
    values = {name: int(value) for name, value in report.groupdict().items()}
    if values["holders"] != 4 or values["pages"] != 65536:
        fail(f"unexpected four-holder capacity report: {report.group(0)}")
    if values["payload"] != 1024 * 1024 * 1024:
        fail("capacity report does not carry four 256 MiB payloads")
    if values["reserved"] != values["payload"] + values["tables"] + values["alignment"]:
        fail("reserved RAM does not equal payload plus page tables and alignment waste")
    if values["mapped"] != 0 or values["reusable"] != 0:
        fail("host capacity plan reports runtime mapped or reusable RAM")
    if values["allocations"] > values["allocations_available"]:
        fail("capacity plan exceeds the compiled allocation descriptor table")
    if values["extents"] > values["extents_available"]:
        fail("capacity plan exceeds the compiled extent descriptor table")
    if values["holder_cslots"] + values["graph_cslots"] > values["cslots_available"]:
        fail("capacity plan exceeds the actual free root CSlots")
    if values["reserved"] > values["ordinary_available"]:
        fail("capacity plan exceeds the root's actual ordinary-memory inventory")
    platform_bytes = profile_integer(profile, "memory_mib", fail, section) * 1024 * 1024
    reported_root = values["image"] + values["stack"] + values["heap"]
    if values["reserved"] + reported_root >= platform_bytes:
        fail("four 256 MiB holders plus reported root image exceed the platform envelope")
    if values["metadata"] < values["allocations"] * 8:
        fail("root metadata report hides the capacity-scaled descriptor storage")


def main() -> None:
    parser = argparse.ArgumentParser(description="Check mixed-size private memory on seL4")
    parser.add_argument(
        "--platform",
        choices=sorted(PLATFORMS),
        default="qemu-arm-virt",
        help="the pinned QEMU profile and image to build and boot",
    )
    arguments = parser.parse_args()
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
    check_the_two_planes_are_independent(transcript, declared)
    check_segmented_capacity_report(transcript, profile, section)
    print(
        "seL4 private-memory plane check: "
        f"{marker_count(chains_from_gate(sys.modules[__name__]))} markers across "
        f"{len(CHAINS)} causal chains on {arguments.platform}; the declared quota "
        f"({declared['private-memory-granted']} page(s)) is the measured ceiling "
        "and exactly one 2 MiB frame backs the aligned window"
    )


if __name__ == "__main__":
    main()
