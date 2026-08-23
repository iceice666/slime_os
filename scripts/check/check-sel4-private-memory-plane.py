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
* **the ceiling binds at exactly the declared number.** The granted probe
  discovers its own ceiling by growing one page at a time until refused, and the
  gate requires that measurement to equal the fixture's `pageQuota`. The probe
  never reads the manifest, so this is a measurement rather than a restatement;
* **omission denies.** The instance absent from the budget must be refused its
  first page with `cause=reservation` — the deny-by-default state carries no
  window at all, so it is refused by the reservation before quota arithmetic is
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

import json
import os
import re
import shutil
import subprocess
import sys
import threading
import tomllib
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import sha256_file  # noqa: E402
from sel4_gate_markers import (  # noqa: E402
    chains_from_gate,
    marker_count,
    match_marker_contract,
)
from zutai_cli import STDLIB, binary  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
IMAGE = ROOT / "build" / "slime-sel4-private-memory.elf"
MANIFEST = ROOT / "build" / "slime-sel4-private-memory.identity.json"
BUILD = ROOT / "scripts" / "build" / "build-sel4.py"
PINS = ROOT / "sel4" / "pins.toml"
FIXTURE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "sel4-private-memory.zti"
IMAGE_VARIANT = "private-memory"
TIMEOUT = 240

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
            r"SLIME_MEM refused task=\d+ delta=1 cause=quota "
            r"detail=QuotaExceeded \{ pages: (\d+), delta: 1, quota: (\d+) \}",
            r"\[private-memory-probe\] granted pages=(\d+) base=0x[0-9a-f]+ "
            r"zeroed=1 survived=1 refused=1",
        ),
    ),
    (
        # The omitted probe's own sequence, independent of the granted one's.
        "the omitted holder was refused its first page by the reservation",
        (
            r"SLIME_MEM refused task=\d+ delta=1 cause=reservation "
            r"detail=ReservationExceeded \{ pages: 0, delta: 1, reservation: 0 \}",
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
            r"\[private-heap-probe\] private-heap reuse phase begins",
            r"\[private-heap-probe\] private-heap quota live pages=(\d+) "
            r"growths=(\d+) reuse_growths=0 leaked=0",
            r"SLIME_MEM refused task=\d+ delta=[1-9]\d* cause=quota "
            r"detail=QuotaExceeded \{ pages: (\d+), delta: \d+, quota: (\d+) \}",
            r"\[private-heap-probe\] granted pages=(\d+) growths=(\d+) refused=1 reused=1",
        ),
    ),
    (
        # The omitted holder's own sequence. Its allocator finds no region, so
        # it never reaches the root at all — which is why the assertion is the
        # component's own two lines rather than a root record.
        "the omitted holder could not allocate at all",
        (
            r"\[private-heap-probe\] private-heap denied pages=0 growths=0 "
            r"reuse_growths=0 leaked=0",
            r"\[private-heap-probe\] denied pages=0 growths=0 refused=1",
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
EXPECTED_UNORDERED: tuple[str, ...] = (
    r"\[private-memory-probe\] query pages=0 base=0x[0-9a-f]+",
)

FAILURE_MARKERS: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_MEM FAIL",
    r"\[private-memory-probe\] FAIL",
    r"\[private-heap-probe\] FAIL",
    r"\[init\] private memory plane fail",
    # A growth the root served for a holder the budget does not name would be
    # the whole milestone failing silently, so it is a failure marker rather
    # than an absent-marker check.
    r"SLIME_MEM grown task=\d+ delta=[1-9]\d* previous=\d+ pages=\d+ base=0x0 ",
    # C10.3: exhaustion must be a structural error the component observes. A
    # component that faulted or was terminated instead would leave the chain's
    # report missing, but naming the outcomes explicitly says *which* failure
    # happened rather than only that a marker is absent.
    r"\[private-heap-probe\] private-heap exhausted",
    r"\[private-heap-probe\] private-heap failed",
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


def build_image() -> None:
    process = subprocess.run(
        [sys.executable, str(BUILD), "--private-memory-plane"],
        cwd=ROOT,
        check=False,
    )
    if process.returncode != 0 or not IMAGE.is_file():
        fail(f"seL4 image build failed with exit status {process.returncode}")


def check_manifest() -> None:
    if not MANIFEST.is_file():
        fail("identity manifest missing")
    identity = json.loads(MANIFEST.read_text(encoding="utf-8"))
    if identity.get("variant") != IMAGE_VARIANT:
        fail(f"wrong image variant {identity.get('variant')!r}")
    image = identity.get("image")
    if not isinstance(image, dict) or image.get("sha256") != sha256_file(IMAGE, fail):
        fail("packaged image digest does not match identity manifest")


def boot(profile: dict[str, object]) -> str:
    qemu = shutil.which("qemu-system-aarch64")
    if qemu is None:
        fail("qemu-system-aarch64 is not on PATH")
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
        str(IMAGE),
    ]
    process = subprocess.Popen(
        command,
        cwd=ROOT,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    watchdog = threading.Timer(TIMEOUT, process.kill)
    watchdog.start()
    lines: list[str] = []
    terminal = re.compile(
        r"SLIME_GRAPH HEALTHY|SLIME_ROOT FATAL|private memory plane fail"
        r"|\[private-memory-probe\] FAIL|\[private-heap-probe\] FAIL"
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

    This is the assertion the milestone turns on. The probe grows one page at a
    time until it is refused and reports the total it reached; it never reads the
    manifest, so agreement here means the declared number is what actually bound
    it — not that two copies of a constant match.
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
    # The refusal must name the same number, so the cause the root recorded is
    # the ceiling the probe hit rather than a coincidence at a different bound.
    refusal = re.search(
        r"cause=quota detail=QuotaExceeded \{ pages: (\d+), delta: 1, quota: (\d+) \}",
        transcript,
    )
    if refusal is None:
        fail("no quota refusal was recorded")
    if int(refusal.group(1)) != expected or int(refusal.group(2)) != expected:
        fail(
            f"the refusal names pages={refusal.group(1)} quota={refusal.group(2)}, "
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
        fail(
            f"the root installed a window at {reported} but the probe used "
            f"{measured.group(2)}"
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
            fail(
                f"{instance}: a growth of {delta} took {previous} page(s) to "
                f"{pages}"
            )
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
        r"\[private-heap-probe\] private-heap reuse phase begins",
        transcript,
    )
    if boundary is None:
        fail(f"{holder}: the self-check never entered its reuse phase")
    report = re.search(
        r"\[private-heap-probe\] private-heap quota live pages=(\d+) growths=(\d+) ",
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


def main() -> None:
    declared = declared_quotas()
    build_image()
    check_manifest()
    pins = tomllib.loads(PINS.read_text(encoding="utf-8"))
    profile = pins.get("qemu_arm_virt")
    if not isinstance(profile, dict):
        fail("missing qemu profile")
    transcript = boot(profile)
    check_markers(transcript)
    check_declared_is_installed(transcript, declared)
    check_measured_ceiling(transcript, declared)
    check_only_declared_pages_were_charged(transcript, declared)
    check_growth_was_batched_and_reused(transcript, declared)
    print(
        "seL4 private-memory plane check: "
        f"{marker_count(chains_from_gate(sys.modules[__name__]))} markers across "
        f"{len(CHAINS)} causal chains; the declared quota "
        f"({declared['private-memory-granted']} page(s)) is the measured ceiling, "
        f"{declared['private-heap-granted']} page(s) is the allocator's ceiling, "
        "and an omitted holder grows nothing"
    )


if __name__ == "__main__":
    main()
