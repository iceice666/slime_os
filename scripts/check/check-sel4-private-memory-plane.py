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
import hashlib
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
from sel4_plane import verify_image_identity  # noqa: E402
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
# MEM-64M's reuse arm, on both QEMU architectures: the milestone requires both
# to carry this workload, and an architecture-specific reclamation path can
# regress while the other stays green. AArch64 builds through the closure; RV64
# re-targets the same composition through the legacy plane flag, exactly as the
# ceiling arm's RV64 case does, because a closure's platform follows its system
# spec's `targetRequirement` and every spec declares aarch64.
# MEM-ADAPTIVE's declared-policy compositions. The arm reads each one's policy
# out of its own derived manifest rather than restating it here: the gate's
# claim is that the *declaration* is what the root reserved, bound, and
# adjudicated against, and a copy of the numbers in this file would make that a
# comparison against itself.
ADAPTIVE_FIXTURES = {
    "qemu-arm-virt": GENERATION_COMPOSITIONS / "sel4-private-memory-adaptive.zti",
    "qemu-riscv-virt": GENERATION_COMPOSITIONS / "sel4-private-memory-adaptive-rv64.zti",
}
ADAPTIVE_OVERCOMMIT_FIXTURE = (
    GENERATION_COMPOSITIONS / "sel4-private-memory-adaptive-overcommit.zti"
)
ADAPTIVE_COORDINATOR = "private-adaptive-coordinator"
# The schedule's length, which the coordinator also prints. Pinned because a
# composition that shortened the schedule would otherwise pass with fewer
# boundaries exercised; the individual deltas are read from the transcript and
# checked against the declared maxima instead of being restated.
ADAPTIVE_STEPS = 18
# Steps whose refusal follows from the declaration alone: one page past a
# subject maximum, and one page past the guaranteed subject's maximum. Their
# cause must be `maximum`, never an inventory accident.
ADAPTIVE_DECLARED_REFUSALS = (13, 14)
# The step whose request is inside its declared maximum but beyond any
# inventory. It must be refused, and for a resource reason rather than the
# declaration, or the two boundaries are not distinguishable.
ADAPTIVE_POOL_REFUSAL = 12
ADAPTIVE_ENTITLEMENT_DOMAIN = b"slime-private-memory-entitlement-v2\0"

CYCLES_CLOSURE = "sel4-private-memory-cycles"
CYCLES_RV64_IMAGE = ROOT / "build" / "slime-sel4-private-memory-cycles-qemu-riscv-virt.elf"
CYCLES_FIXTURE = GENERATION_COMPOSITIONS / "sel4-private-memory-cycles.zti"
# Holder lives the arm requires. Its declared ceiling is read from the fixture
# rather than restated, so a composition that lowered the quota fails instead of
# qualifying a smaller working set.
CYCLE_COUNT = 20
CAPACITY_COMPLETE = "[private-memory-1g] complete faults=20 replacements=20 exits=4"
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
        # Both ceiling-arm holders declare a quota below the target's region
        # reservation, so their refusals must name `cause=quota`. The
        # reservation arm of `Region::admit` is reached by the capacity
        # composition, whose holders declare the whole reservation.
        "the granted holder reached its declared ceiling and was then refused",
        (
            r"SLIME_MEM grown task=\d+ delta=16384 previous=0 pages=16384 "
            r"base=0x[0-9a-f]+ quota=16384 total=\d+ large_frames=32 base_frames=0 leaf_tables=0",
            r"SLIME_MEM refused task=\d+ delta=1 cause=quota "
            r"detail=QuotaExceeded \{ pages: 16384, delta: 1, quota: 16384 \}",
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
        # The first odd cycle stores into the guard. Later odd cycles alternate
        # guard writes and execution from backed private memory; semantic checks
        # join each fault to its own task, region and cycle.
        "a cycle that ended by deliberate fault was reclaimed and censused",
        (
            r"\[private-cycle-probe\] cycle=\d+ pages=16384 base=0x[0-9a-f]+ "
            r"stamp=0x[0-9a-f]+ zeroed=1 verified=1 end=fault",
            r"SLIME_GRAPH component fault task=\d+ kind=VirtualMemory \{ access: Write, "
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

CAPACITY_CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "four resident holders precede twenty coordinated replacements and teardown",
        (
            r"\[private-memory-1g\] zeroed holder=0 incarnation=0 pages=65536",
            r"\[private-memory-1g\] zeroed holder=1 incarnation=0 pages=65536",
            r"\[private-memory-1g\] zeroed holder=2 incarnation=0 pages=65536",
            r"\[private-memory-1g\] zeroed holder=3 incarnation=0 pages=65536",
            r"\[private-memory-1g\] resident holders=4 pages=262144",
            r"\[private-memory-1g\] retained cycle=1 holders=4 pages=262144",
            r"\[private-memory-1g\] retained cycle=20 holders=4 pages=262144",
            r"\[private-memory-1g\] complete faults=20 replacements=20 exits=4",
        ),
    ),
)

ISOLATION_CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "owned buffers and reciprocal loans were refused inside private windows before foreign faults",
        (
            r"\[private-memory-isolation\] victim base=\d+ pages=65536",
            r"\[private-memory-isolation\] loan receiver source=1 holder=2 positive=1 "
            r"window_denied=2 unmapped=1 returned=1 return_denied=1 pages=0 buffers=0 mappings=0 loans=0",
            r"\[private-memory-isolation\] loan lender holder=1 receiver=2 transferred=1 "
            r"released=1 pages=0 buffers=0 mappings=0 loans=0",
            r"\[private-memory-isolation\] loan receiver source=2 holder=1 positive=1 "
            r"window_denied=2 unmapped=1 returned=1 return_denied=1 pages=0 buffers=0 mappings=0 loans=0",
            r"\[private-memory-isolation\] loan lender holder=2 receiver=1 transferred=1 "
            r"released=1 pages=0 buffers=0 mappings=0 loans=0",
            r"\[private-memory-isolation\] attacker holder=1 operation=6 address=\d+ "
            r"own_base=\d+ own_pages=1 own_buffer=1 buffer_window_denied=2 "
            r"unowned_denied=3 seal_denied=1",
            r"\[private-memory-isolation\] denied holder=1 operation=6 address=\d+ "
            r"victim_preserved=1",
            r"\[private-memory-isolation\] attacker holder=2 operation=7 address=\d+ "
            r"own_base=\d+ own_pages=1 own_buffer=1 buffer_window_denied=2 "
            r"unowned_denied=3 seal_denied=1",
            r"\[private-memory-isolation\] denied holder=2 operation=7 address=\d+ "
            r"victim_preserved=1",
            r"\[private-memory-isolation\] execute address=\d+ pages=65536",
            r"\[private-memory-isolation\] complete read=1 write=1 execute=1 "
            r"buffer_window_denied=4 loan_window_denied=4 unowned_denied=6 "
            r"seal_denied=2 loan_transfer=2",
        ),
    ),
)

HEALTHY_MARKER = r"SLIME_GRAPH HEALTHY generation=\d+ required=\d+ live=\d+ completed=\d+ failed=0"
METADATA_FIELDS = (
    "pages", "bytes", "allocations_live", "allocations_free", "extents_live",
    "extents_free", "preserved_live", "preserved_free", "slot_words", "slots_free",
    "nodes", "quarantined", "pending",
)
METADATA_VALUES = " ".join(field + r"=\d+" for field in METADATA_FIELDS)
CSPACE_CHAINS = (("expanded cspace pressure precedes execution and healthy graph", (
    r"SLIME_ROOT cspace expanded base=\d+ infrastructure_caps=\d+",
    r"SLIME_ROOT cspace pressure initial_limit=\d+ retired=\d+ free=\d+",
    r"SLIME_ROOT cspace leaf base=\d+ slots=\d+ beyond_initial=1",
    r"SLIME_ROOT cspace exercised created=\d+ invoked=\d+ copied=\d+ retyped=\d+ deleted=\d+ revoked=\d+ min_address=\d+ initial_limit=\d+",
    r"SLIME_ROOT cspace live root=1 second=1 leaves=\d+",
    HEALTHY_MARKER,
)),)
METADATA_CHAINS = (
    ("metadata capacity grows and reconciles after release", tuple(
        rf"SLIME_ROOT metadata census phase={phase} {METADATA_VALUES}"
        for phase in ("baseline", "grown", "released", "final")
    ) + (HEALTHY_MARKER,)),
    ("metadata growth funds later reuse", tuple(
        rf"SLIME_ROOT metadata round={index} grew=\d+ reused=\d+" for index in range(3)
    )),
    *((f"metadata {kind} quarantine precedes retry", (
        rf"SLIME_ROOT metadata injected kind={kind} retained=\d+ reassigned=0",
        rf"SLIME_ROOT metadata retry kind={kind} released=1 reassigned=0",
    )) for kind in ("construction", "revoke")),
)
BOOTSTRAP_CHAINS = (
    ("bootstrap source precedes bounded reservation", (
        r"SLIME_ROOT metadata bootstrap objects=\d+ alignment=\d+ remaining=\d+",
        r"SLIME_ROOT bootstrap reserve objects=\d+ alignment=\d+ remaining=\d+ slots=64 transaction=\d+ recursion=0 fit=1",
    )),
    *((f"independent {cause} refusal precedes publication", (
        rf"SLIME_ROOT bootstrap exhausted cause={cause} published=0 refused=1 ram_free=\d+ slots_free=\d+ metadata_free=\d+",
        r"SLIME_ROOT bootstrap boundaries complete cases=3 published=1",
        r"SLIME_GRAPH staged task=\d+ .*",
        HEALTHY_MARKER,
    )) for cause in ("ram", "cnode-slots", "metadata")),
)

ELASTIC_CENSUS = (
    r"ordinary=\d+ retained=\d+ reusable=\d+ live_bytes=\d+ extents_active=\d+ "
    r"extents_anchored=\d+ slots_free=\d+ descriptors_free=\d+ extent_records_free=\d+ metadata=\d+"
)
ELASTIC_CHAINS = (
    ("an idle maximum reserves nothing a peer then consumes", (
        rf"SLIME_MEM elastic census phase=admitted {ELASTIC_CENSUS}",
        r"SLIME_MEM elastic admitted pool_bytes=\d+ guarantee_pages=\d+ holders=3",
        r"SLIME_MEM elastic idle subject=qualification-idle maximum=\d+ reserved_bytes=0 "
        r"pool_before=\d+ pool_after=\d+",
        r"SLIME_MEM elastic grow subject=qualification-bulk committed=\d+ bytes=\d+ pool_after=\d+",
        r"SLIME_MEM elastic refused subject=qualification-bulk pages=\d+ cause=\S+ "
        r"committed=\d+ peer_committed=\d+",
        # Same chain, because the order is the claim: the guarantee is served
        # *after* the pool the elastic holders share ran out.
        r"SLIME_MEM elastic guarantee subject=qualification-guaranteed-holder committed=\d+ "
        r"promised=\d+ served=1",
        r"SLIME_MEM elastic idle_request subject=qualification-idle served=\d+ committed=\d+",
        r"SLIME_MEM elastic intact bulk=1 guaranteed=1 idle=1 pages=\d+",
    )),
    ("every holder returns its capacity", (
        rf"SLIME_MEM elastic census phase=served {ELASTIC_CENSUS}",
        rf"SLIME_MEM elastic census phase=retired {ELASTIC_CENSUS}",
        r"SLIME_MEM elastic complete case=idle-and-guarantee holders=3 granted=\d+ reclaimed=\d+",
        HEALTHY_MARKER,
    )),
)
FRAGMENTATION_CHAINS = (
    ("page-granular and span requests are priced separately", (
        r"SLIME_MEM elastic inventory pool_bytes=\d+ largest_aligned=\d+ retained=\d+ reusable=\d+",
        r"SLIME_MEM elastic request kind=single pages=1 committed=1 charged=\d+ large=0 base=1 tables=1",
        r"SLIME_MEM elastic request kind=mixed pages=\d+ committed=\d+ charged=\d+ large=\d+ base=\d+ tables=\d+",
    )),
    ("fragmented backing serves pages no aligned span could", (
        r"SLIME_MEM elastic fragmented ordinary_aligned=0 served=1 committed=\d+ "
        r"retained=\d+ reusable=\d+",
        r"SLIME_MEM elastic fragmented_span served=0 cause=\S+ committed=\d+",
    )),
    ("a terminal refusal names its resource and still serves a fitting request", (
        r"SLIME_MEM elastic request kind=bulk pages=\d+ committed=\d+ charged=\d+ large=\d+ base=0 tables=0",
        r"SLIME_MEM elastic cost small_bytes_per_page=\d+ bulk_bytes_per_page=\d+",
        r"SLIME_MEM elastic refusal kind=bulk resource=\S+ pool_bytes=\d+ ordinary=\d+ "
        r"retained=\d+ reusable=\d+",
        r"SLIME_MEM elastic next kind=fitting served=1 committed=\d+ pool_bytes=\d+",
        r"SLIME_MEM elastic reuse returned_pages=\d+ reused_extents=\d+ served=\d+ committed=\d+",
        r"SLIME_MEM elastic complete case=mixed-fragmentation granted=\d+ reclaimed=\d+",
        HEALTHY_MARKER,
    )),
)
ROLLBACK_STAGES = (
    "extent", "descriptors", "retype", "table-map", "frame-map", "near-limit-descriptors",
)
ROLLBACK_CHAINS = (
    ("every injected stage leaves committed state and sentinels untouched", (
        r"SLIME_MEM elastic baseline committed=\d+ charged=\d+ pool_bytes=\d+ peer=\d+",
    ) + tuple(
        rf"SLIME_MEM elastic injected stage={stage} cause=\S+ committed=\d+ pool_before=\d+ "
        rf"pool_after=\d+ ordinary_before=\d+ ordinary_after=\d+ retained_tables=\d+ "
        rf"sentinels=1 quarantined=0"
        for stage in ROLLBACK_STAGES
    )),
    ("a retry after every failure charges once", (
        r"SLIME_MEM elastic retry stage=all committed=\d+ charged=\d+ clean=\d+ doubled=0",
        r"SLIME_MEM elastic quarantine stage=cleanup cause=\S+ owned=1 refused=1 released=1 "
        r"remaining=0 pool_held=\d+ pool_after=\d+",
        r"SLIME_MEM elastic complete case=failure-rollback stages=7 granted=\d+ reclaimed=\d+",
        HEALTHY_MARKER,
    )),
)
CONSERVATION_CHAINS = (
    ("a revoked holder's capacity becomes another holder's", (
        r"SLIME_MEM elastic holder subject=qualification-first committed=\d+ pattern=1 pool_bytes=\d+",
        r"SLIME_MEM elastic retire subject=qualification-first revoked=1 returned_pages=\d+ "
        r"pool_before=\d+ pool_after=\d+ reusable=\d+",
        r"SLIME_MEM elastic reuse subject=qualification-second committed=\d+ reused_extents=\d+ "
        r"zeroed=1 peer_pattern=1",
    )),
    ("a failed revoke keeps ownership until its retry", (
        r"SLIME_MEM elastic revoke_failure subject=qualification-stuck first_attempt=0 "
        r"retained=1 retry=1 pool_before=\d+ pool_held=\d+ pool_after=\d+",
        r"SLIME_MEM elastic baseline phase=final owned_before=\d+ owned_after=\d+ "
        r"slots_before=\d+ slots_after=\d+ pool_before=\d+ pool_after=\d+ granted=\d+ reclaimed=\d+",
        r"SLIME_MEM elastic complete case=cross-holder-conservation holders=4 granted=\d+ reclaimed=\d+",
        HEALTHY_MARKER,
    )),
)

# MEM-ADAPTIVE. Every numeric policy field is a capture rather than a literal:
# `check_adaptive_plane` resolves it against the composition's own declaration,
# so a plane whose maxima changed fails there instead of quietly qualifying a
# different policy. The entitlement column is a 16-hex identity prefix because
# the root cannot invert the identity hash; the validator recomputes
# `sha256("slime-private-memory-entitlement-v2\0" || name)` from the declared
# name and requires the transcript's prefix to be that one.
ADAPTIVE_BINDING = (
    r"entitlement=[0-9a-f]{16} incarnation=(\d+) guarantee=(\d+) maximum=(\d+) "
    r"mode=fixed installed=(\d+) base=0x[0-9a-f]+"
)
# Deny-by-default is spelled out rather than captured: every field of an
# unbound instance's line is a constant, so a root that bound something to an
# instance the policy never names cannot satisfy it.
ADAPTIVE_UNBOUND = (
    r"entitlement=none incarnation=0 guarantee=0 maximum=0 mode=none installed=0 base=0x0"
)
ADAPTIVE_CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        # Reserve-first: the guarantees are physically reserved and reported
        # before any task exists, and each task's binding is then printed
        # during that task's own construction. `init` and the coordinator are
        # declared instances the policy does not name, so their lines are the
        # deny-by-default half; without a line per instance a denied holder is
        # indistinguishable from an absent one.
        "the declared policy is reserved before any task is constructed",
        (
            r"SLIME_MEM policy entitlements=(\d+) subjects=(\d+) guarantee_pages=(\d+) "
            r"reserved_bytes=(\d+) reserved_slots=(\d+) reserved_descriptors=(\d+) "
            r"reserved_extents=(\d+) reserved_tables=(\d+) pool_bytes=(\d+)",
            r"SLIME_MEM entitlement task=\d+ instance=init " + ADAPTIVE_UNBOUND,
            r"SLIME_MEM entitlement task=\d+ instance=private-adaptive-coordinator "
            + ADAPTIVE_UNBOUND,
            r"SLIME_MEM entitlement task=\d+ instance=private-adaptive-pool-b "
            + ADAPTIVE_BINDING,
        ),
    ),
    (
        # The coordinator reports its own refusal before it spawns anything, so
        # deny-by-default is observed by a live task rather than inferred from
        # an absent grant. Both dynamically spawned subjects then bind on the
        # spawn path, and a second live incarnation of one of them must not
        # acquire the entitlement again.
        #
        # The duplicate is refused by `class=instance-live`, before a task
        # exists and therefore before any entitlement is bound. That is the
        # stronger of the two possible refusals and it is the one the root
        # actually makes: an earlier revision expected the ledger's duplicate
        # arm instead, which can only be reached by constructing a second task
        # for a live instance and unwinding it. The claim — one live
        # incarnation per subject, never two — is asserted directly over the
        # binding and retirement record by `check_adaptive_incarnations`.
        "a task with no entitlement is refused, and a bound one is not bound twice",
        (
            r"\[private-adaptive\] coordinator subject=private-adaptive-coordinator "
            r"entitlement=none base=0x0 pages=0 refused=1",
            r"SLIME_MEM entitlement task=\d+ instance=private-adaptive-guaranteed "
            + ADAPTIVE_BINDING,
            r"SLIME_MEM entitlement task=\d+ instance=private-adaptive-pool-a "
            + ADAPTIVE_BINDING,
            r"SLIME_GRAPH spawn refused task=\d+ child=private-adaptive-guaranteed "
            r"class=instance-live",
            r"\[private-adaptive\] duplicate subject=private-adaptive-guaranteed "
            r"refused=1 live_base=0x[0-9a-f]+",
        ),
    ),
    (
        # The schedule's own order, which is the claim. Only the boundaries and
        # the terminals are pinned positionally; `check_adaptive_schedule` reads
        # every step and joins it to the root's adjudication of the same
        # request.
        "the repeated request schedule reaches each declared boundary and recovers",
        (
            r"\[private-adaptive\] step=0 subject=private-adaptive-pool-b delta=(\d+) "
            r"served=1 pages=(\d+) base=0x[0-9a-f]+",
            r"\[private-adaptive\] step=12 subject=private-adaptive-pool-a delta=(\d+) "
            r"served=0 pages=(\d+) base=0x[0-9a-f]+",
            r"\[private-adaptive\] step=13 subject=private-adaptive-pool-b delta=(\d+) "
            r"served=0 pages=(\d+) base=0x[0-9a-f]+",
            r"\[private-adaptive\] step=14 subject=private-adaptive-guaranteed delta=(\d+) "
            r"served=0 pages=(\d+) base=0x[0-9a-f]+",
            r"\[private-adaptive\] step=17 subject=private-adaptive-guaranteed delta=(\d+) "
            r"served=1 pages=(\d+) base=0x[0-9a-f]+",
            r"\[private-adaptive\] schedule steps=18 served=(\d+) refused=(\d+) "
            r"digest=0x[0-9a-f]{16}",
        ),
    ),
    (
        # A deliberate fault, the backing it returns to its own entitlement, and
        # a replacement that binds that entitlement once. `entitlement_committed=0`
        # after retirement is the half that says a dead incarnation's charge did
        # not survive it; `same_base=1` is the half that says the replacement got
        # the same declared window rather than a new one.
        "a faulted incarnation returns its backing and its replacement rebinds once",
        (
            r"\[private-adaptive\] restart subject=private-adaptive-restart incarnation=0 "
            r"base=0x[0-9a-f]+ pages=(\d+) end=fault",
            r"\[private-adaptive:restart\] end incarnation=0 pages=(\d+) refused=0 kind=fault",
            r"SLIME_MEM adaptive retired task=\d+ instance=private-adaptive-restart "
            r"entitlement=[0-9a-f]{16} returned_pages=(\d+) entitlement_committed=0 "
            r"quarantined=0",
            r"SLIME_MEM entitlement task=\d+ instance=private-adaptive-restart "
            + ADAPTIVE_BINDING,
            r"\[private-adaptive\] restart subject=private-adaptive-restart incarnation=1 "
            r"base=0x[0-9a-f]+ pages=(\d+) same_base=1 end=exit",
            r"\[private-adaptive:restart\] end incarnation=1 pages=(\d+) refused=0 kind=exit",
        ),
    ),
    (
        # Omission denies. The refusal names `cause=entitlement`, which is a
        # stronger statement than a zero maximum: the task carries no window at
        # all, so it is refused before any maximum arithmetic is reached.
        "an omitted subject holds no window, is refused, and still exits cleanly",
        (
            r"SLIME_MEM entitlement task=\d+ instance=private-adaptive-denied "
            + ADAPTIVE_UNBOUND,
            r"SLIME_MEM adaptive refused task=\d+ instance=private-adaptive-denied "
            r"entitlement=none delta=1 pages=0 cause=entitlement entitlement_committed=0 "
            r"pool_bytes=(\d+)",
            r"\[private-adaptive\] denied subject=private-adaptive-denied pages=0 base=0x0 "
            r"refused=1 end=exit",
            r"\[private-adaptive:denied\] end incarnation=0 pages=0 refused=1 kind=exit",
        ),
    ),
    (
        "every surviving subject kept its window and its bytes, then returned them",
        (
            r"\[private-adaptive\] stable subjects=3 guaranteed_pages=(\d+) "
            r"pool_a_pages=(\d+) pool_b_pages=(\d+)",
            r"\[private-adaptive:guaranteed\] end incarnation=0 pages=(\d+) refused=(\d+) "
            r"kind=exit",
            r"\[private-adaptive:pool-a\] end incarnation=0 pages=(\d+) refused=(\d+) kind=exit",
            r"\[private-adaptive:pool-b\] end incarnation=0 pages=(\d+) refused=(\d+) kind=exit",
            r"\[private-adaptive\] complete steps=18 served=(\d+) refused=(\d+) restarts=1 "
            r"duplicates=1 denied=1 exits=5 digest=0x[0-9a-f]{16}",
            HEALTHY_MARKER,
        ),
    ),
)

# The over-guaranteed negative composition, kept out of `CHAINS` on purpose: its
# terminal is a `SLIME_MEM FAIL` line, which every other arm vetoes, so a
# synthetic transcript carrying both would be self-contradictory. It has its own
# validator and its own controls.
ADAPTIVE_REFUSAL_CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "a guarantee no inventory can fund is refused before anything is published",
        (
            r"SLIME_MEM FAIL adaptive guarantee exceeds inventory required=(\d+) "
            r"available=(\d+) published=0",
        ),
    ),
)
ADAPTIVE_REFUSAL_FORBIDDEN: tuple[str, ...] = (
    r"SLIME_GRAPH staged ",
    r"SLIME_GRAPH staged task=",
    r"SLIME_GRAPH spawned ",
    r"SLIME_GRAPH activated ",
    r"SLIME_MEM policy entitlements=",
    r"SLIME_MEM entitlement task=",
    r"\[private-adaptive\] ",
    HEALTHY_MARKER,
)

# The union, for `sel4_gate_control_check`'s coverage count only. Each arm
# matches its own chains; a transcript from one arm does not carry the other's
# markers, so matching the union would fail every run.
CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    CEILING_CHAINS + CYCLE_CHAINS + CAPACITY_CHAINS + ISOLATION_CHAINS
    + CSPACE_CHAINS + METADATA_CHAINS + BOOTSTRAP_CHAINS
    + ELASTIC_CHAINS + FRAGMENTATION_CHAINS + ROLLBACK_CHAINS + CONSERVATION_CHAINS
    + ADAPTIVE_CHAINS
)

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
    r"\[private-memory-1g\] FAIL",
    r"\[init\] private memory cycles plane fail",
    r"SLIME_GRAPH task reclaim incomplete task=\d+",
    r"SLIME_GRAPH holder reclaim incomplete task=\d+",
    # MEM-ADAPTIVE. The coordinator's and each holder's own diagnostics: a
    # holder that measured a moved base, a charged refusal, or a non-zero page
    # it was just served says so on one line, which is more informative than
    # the missing report that would otherwise be the only evidence. The two
    # spellings are separate patterns rather than one optional group so the
    # meta-gate can instantiate both.
    r"\[private-adaptive\] FAIL",
    r"\[private-adaptive:[a-z-]+\] FAIL",
    # A subject the adaptive policy does not name must never receive a window;
    # a base on a denied line is the whole deny-by-default clause failing
    # silently, so it is vetoed rather than left to a missing marker.
    r"\[private-adaptive\] denied subject=\S+ pages=[1-9]",
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
    additional_arguments: tuple[str, ...] = (),
    memory_mib: int | None = None,
    timeout: int = TIMEOUT,
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
        f"size={memory_mib or profile_integer(profile, 'memory_mib', fail, section)}M",
        "-nographic",
        "-serial",
        "mon:stdio",
        *qemu_kernel_arguments(qemu_binary, image, fail),
        # An arm whose holder drives a device needs one attached: the root
        # probes the platform's stable device order, and an unattached plane
        # would admit an IO budget naming a transport that is not there.
        *additional_arguments,
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
    watchdog = threading.Timer(timeout, process.kill)
    watchdog.start()
    lines: list[str] = []
    # `SLIME_ROOT READY` as well as the graph's terminal: the rollback case
    # boots the fixture root, whose embedded child runs the injected-failure arm
    # of the private-memory phase and which never launches a component graph.
    terminal = re.compile(
        r"SLIME_GRAPH HEALTHY|SLIME_ROOT READY|SLIME_ROOT FATAL|private memory plane fail"
        r"|\[private-memory-probe\] FAIL|\[private-heap-probe:(?:granted|denied|both)\] FAIL"
        r"|\[private-memory-1g\] FAIL|\[private-heap-probe:stress\] FAIL"
        r"|\[private-matrix(?::[a-z]+)?\] FAIL"
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


def check_cycle_fault_attribution(transcript: str, declared_pages: int) -> None:
    prefix = "cycle fault attribution: "
    quotas = list(re.finditer(
        r"^SLIME_MEM quota task=(\d+) instance=private-cycle-probe "
        r"declared=(\d+) installed=(\d+) base=(0x[0-9a-f]+)$", transcript, re.MULTILINE))
    if len(quotas) != CYCLE_COUNT or len({q.group(1) for q in quotas}) != CYCLE_COUNT:
        fail(prefix + "missing, duplicate or reused holder identity")
    all_faults = re.findall(r"^SLIME_GRAPH component fault task=", transcript, re.MULTILINE)
    if len(all_faults) != CYCLE_COUNT // 2:
        fail(prefix + "unexpected fault count")
    for cycle, quota in enumerate(quotas):
        task, declared, installed, base = quota.groups()
        if (int(declared), int(installed)) != (declared_pages, declared_pages):
            fail(prefix + "installed quota differs")
        end = quotas[cycle + 1].start() if cycle + 1 < len(quotas) else len(transcript)
        interval = transcript[quota.end():end]
        report = re.search(
            rf"\[private-cycle-probe\] cycle={cycle} pages={declared_pages} base={base} "
            r"stamp=0x[0-9a-f]+ zeroed=1 verified=1 end=(exit|fault)", interval)
        growth = re.search(
            rf"SLIME_MEM grown task={task} delta={declared_pages} previous=0 pages={declared_pages} "
            rf"base={base} quota={declared_pages} total={declared_pages} large_frames={declared_pages // 512} base_frames=0 leaf_tables=0",
            interval)
        if report is None or growth is None or growth.end() > report.start():
            fail(prefix + "fault subject did not establish its backed private region")
        faults = list(re.finditer(
            r"SLIME_GRAPH component fault task=(\d+) kind=VirtualMemory "
            r"\{ access: (\w+), status: (\d+) \} address=Some\((\d+)\)", interval))
        if cycle % 2 == 0:
            if report.group(1) != "exit" or faults:
                fail(prefix + "clean incarnation unexpectedly faulted")
            continue
        access = "Write" if cycle % 4 == 1 else "Execute"
        address = int(base, 16) + (declared_pages * 4096 if access == "Write" else 0)
        if report.group(1) != "fault" or len(faults) != 1:
            fail(prefix + "missing exact fault record")
        fault = faults[0]
        if fault.start() < report.end() or fault.group(1) != task or fault.group(2) != access or int(fault.group(4)) != address or int(fault.group(3)) == 0:
            fail(prefix + f"cycle {cycle} has wrong task, access, address, status or ordering")


def check_reuse_cycles(transcript: str, declared_pages: int) -> None:
    """MEM-64M's reuse clause: twenty 64 MiB lives, no drift, no stale bytes.

    The chains above establish what one cycle promises. This establishes the
    aggregate the milestone actually requires, which no per-cycle marker can:
    that the count is twenty, that both termination paths occur, and that the
    allocator's own watermarks come back to the same values after every one of
    them.

    The drift check reads the root's `reclaim census`, whose three resource
    figures come from the allocator's own watermarks rather than from the
    counters the reclamation path maintains. That is what lets this arm fail at
    all: a leak can leave every root-maintained tally agreeing with every other
    one, so a census read from the same bookkeeping would agree too.
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
    # Access, address and task identity must describe the requested fault,
    # not stale fast-register words from an earlier service call.
    faults = len(re.findall(r"SLIME_GRAPH component fault task=\d+ kind=", transcript))
    memory_faults = re.findall(
        r"SLIME_GRAPH component fault task=(\d+) kind=VirtualMemory "
        r"\{ access: (Write|Execute), status: (\d+) \} address=Some\((\d+)\)",
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
    check_cycle_fault_attribution(transcript, declared_pages)
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
        r"cause=quota detail=QuotaExceeded \{ pages: (\d+), delta: 1, "
        r"quota: (\d+) \}",
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


# `slime-root/src/object_allocator.rs`'s planning constants. The report is the
# root's own arithmetic over the contract row, so the gate must recompute it
# from the declared envelope rather than restate one target's observed numbers.
PRIVATE_EXTENT_PAGES = 512
PRIVATE_EXTENT_BYTES = 2 * 1024 * 1024
GRANULE_BYTES = 4096


def declared_capacity_envelope(target: str, declared_pages: int) -> dict[str, int]:
    """The private headroom the contract still admits for `target`.

    The aggregate ceiling is the whole population an image may install, so the
    holders it can still admit are those the ceiling covers beyond the quotas
    the generation already declares. Counting the declared graph plus a second
    full aggregate would qualify a population admission itself refuses.

    Headroom below one whole holder is not qualified here; an image that fills
    the aggregate with full-ceiling holders is qualified by the capacity arm's
    own boot rather than by this report.
    """
    pages, total = PRIVATE_MEMORY_CAPACITY_PROFILES[target]
    holders = (total - declared_pages) // pages
    spans = -(-pages // PRIVATE_EXTENT_PAGES)
    payload = pages * GRANULE_BYTES
    tables = spans * GRANULE_BYTES
    reserved_data = spans * PRIVATE_EXTENT_BYTES
    return {
        "holders": holders,
        "pages": pages,
        "private_allocations": (pages + 2 * spans) * holders,
        "private_extents": (1 + 2 * spans) * holders,
        "private_cslots": (pages + 2 * spans + 1 + 2 * spans) * holders,
        "private_reserved": (reserved_data + tables) * holders,
        "payload": payload * holders,
        "tables": tables * holders,
        "alignment": (reserved_data - payload) * holders,
    }


def check_segmented_capacity_report(
    transcript: str, profile: dict[str, object], section: str, target: str, declared_pages: int
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
        r"root_heap=(?P<heap>\d+) fit=(?P<fit>\d+) limit=(?P<limit>[a-z-]+)$",
        re.MULTILINE,
    )
    reports = list(pattern.finditer(transcript))
    qualification_lines = re.findall(r"^SLIME_MEM qualification\b.*$", transcript, re.MULTILINE)
    if len(reports) != 1 or len(qualification_lines) != 1:
        fail(prefix + f"expected exactly one complete report, found {len(qualification_lines)}")
    report = reports[0]
    values: dict[str, int] = {}
    for name, token in report.groupdict().items():
        if name in ("scope", "limit"):
            continue
        value = int(token)
        if value > 2**64 - 1:
            fail(prefix + f"{name} exceeds u64")
        values[name] = value
    if report.group("scope") != "contract-aggregate-headroom":
        fail(prefix + "scope mismatch")
    # Zero holders would satisfy every comparison with nothing required, so a
    # report that qualifies no headroom is a refusal rather than a pass.
    if values["holders"] < 1:
        fail(prefix + "the contract admits no headroom beyond the declared graph")
    # The envelope the contract admits for this target: its declared per-holder
    # ceiling, and the holders its aggregate ceiling admits at that size.
    expected_private = declared_capacity_envelope(target, declared_pages)
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
    # A refusal must name the resource that ran out; the layout cause is not
    # derivable from this line, so any of its three causes is accepted there.
    limit = report.group("limit")
    if values["required_allocations"] > values["allocations_available"]:
        expected_limits = {"allocation-descriptors"}
    elif values["required_extents"] > values["extents_available"]:
        expected_limits = {"extent-descriptors"}
    elif values["required_cslots"] > values["cslots_available"]:
        expected_limits = {"root-cslots"}
    elif values["required_reserved"] > values["ordinary_available"]:
        expected_limits = {"ordinary-bytes"}
    elif values["ordinary_layout"] != 1:
        expected_limits = {"metadata-records", "root-cslots", "ordinary-layout"}
    else:
        expected_limits = {"none"}
    if limit not in expected_limits:
        fail(prefix + f"limit={limit} does not name the refusing resource")
    if (limit == "none") != (values["fit"] == 1):
        fail(prefix + "limit disagrees with fit")
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

def build_cycles_image(platform: str) -> Path:
    """The cycles image for `platform`: closure on AArch64, plane flag on RV64."""
    if platform == CLOSURE_PLATFORM:
        return build_named_image(CYCLES_CLOSURE)
    command = [
        sys.executable,
        str(BUILD_SCRIPT),
        "--skip-pin-check",
        "--private-memory-cycles-plane",
        "--platform",
        platform,
    ]
    print(f"[build] {' '.join(command)}", flush=True)
    try:
        process = subprocess.run(command, cwd=ROOT, check=False)
    except OSError as error:
        fail(f"cannot build the RV64 private-memory-cycles image: {error}")
    if process.returncode != 0:
        fail(
            "RV64 private-memory-cycles image build failed with exit status "
            f"{process.returncode}"
        )
    if not CYCLES_RV64_IMAGE.is_file():
        fail(f"missing packaged image {CYCLES_RV64_IMAGE}")
    return CYCLES_RV64_IMAGE


def run_cycles_arm(platform: str) -> None:
    """MEM-64M's reuse clause on its own composition, on one architecture.

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
    section, qemu_binary = PLATFORMS[platform]
    profile = load_qemu_profile(fail, PINS, section)
    transcript = boot(
        profile,
        section=section,
        qemu_binary=qemu_binary,
        image=build_cycles_image(platform),
    )
    (ROOT / "build" / f"private-memory-cycles-{platform}.serial.log").write_text(
        transcript + "\n", encoding="utf-8"
    )
    check_markers(transcript, CYCLE_CHAINS)
    check_declared_is_installed(transcript, declared)
    check_reuse_cycles(transcript, quota)
    print(
        "seL4 private-memory plane check: "
        f"{marker_count(CYCLE_CHAINS)} markers across {len(CYCLE_CHAINS)} causal "
        f"chains and 1 image case on {platform}; the declared quota "
        f"({quota} page(s)) was reclaimed and re-served {CYCLE_COUNT} times over "
        f"{CYCLE_COUNT // 2} clean exits and {CYCLE_COUNT // 2} deliberate faults, "
        "including five exact guard Write faults and five backed-private Execute faults; "
        "every served word zero, with no drift in reusable slots, untyped bytes, "
        "or live objects"
    )


def check_capacity_conservation(transcript: str, prefix: str = "") -> None:
    matches = list(re.finditer(r"^SLIME_MEM census (.*)$", transcript, re.MULTILINE))
    rows = [match.group(1) for match in matches]
    if len(rows) < 2:
        fail(prefix + "capacity conservation: missing initial or final census")
    parsed = [dict((key, int(value)) for key, value in re.findall(r"(\w+)=(\d+)", row)) for row in rows]
    initial, final = parsed[0], parsed[-1]
    required = {
        "untyped", "reusable", "shared_reusable", "preserved_bytes",
        "active_extent_bytes", "mapped_pages", "free_slots", "anchors",
        "shared_anchors", "preserved_anchors", "allocations_free", "infrastructure_owned",
    }
    if any(not required <= row.keys() for row in (initial, final)):
        fail(prefix + "capacity conservation: incomplete resource ledger")
    if initial["mapped_pages"] != 0 or final["mapped_pages"] != 0 or final["active_extent_bytes"] != 0:
        fail(prefix + "capacity conservation: workload did not return private mappings and active backing")
    available = ("untyped", "reusable", "shared_reusable", "preserved_bytes", "infrastructure_owned")
    expected = sum(initial[key] for key in available) + initial["active_extent_bytes"]
    observed = sum(final[key] for key in available)
    if observed != expected:
        fail(prefix + f"capacity conservation: available backing {observed}, expected {expected}")
    slots = ("free_slots", "anchors", "shared_anchors", "preserved_anchors")
    if sum(final[key] for key in slots) < sum(initial[key] for key in slots):
        fail(prefix + "capacity conservation: root slots lost beyond explicitly retained anchors")
    if final["allocations_free"] < initial["allocations_free"]:
        fail(prefix + "capacity conservation: allocation descriptors lost")
    interval = transcript[matches[0].end():matches[-1].start()]
    if not re.search(r"^SLIME_ALLOC preserved parent=\d+ slot=\d+ paddr=\d+ bytes=\d+$", interval, re.MULTILINE):
        fail(prefix + "capacity conservation: preserved backing was never allocated")


def check_backing_ledger(transcript: str) -> None:
    """Replay exact parent partitions; counters alone cannot establish ownership."""
    prefix = "backing ledger: "
    nodes: dict[int, dict[str, int | str]] = {}
    consumed: list[tuple[int, int, int]] = []
    inventory: list[tuple[int, int]] = []
    phase: str | None = None
    snapshots: dict[str, dict[str, list[dict[str, int | str]]]] = {}
    census: dict[str, int] | None = None
    completed: list[str] = []
    # Task extents a buddy split currently divides: parent slot -> children.
    split_extents: dict[int, list[int]] = {}

    def fields(line: str) -> dict[str, int | str]:
        result: dict[str, int | str] = {}
        for token in line.split():
            if "=" not in token:
                fail(prefix + f"malformed field {token!r}")
            key, value = token.split("=", 1)
            if key in result:
                fail(prefix + f"duplicate field {key}")
            result[key] = int(value) if value.isdecimal() else value
        return result

    def integer(row: dict[str, int | str], key: str) -> int:
        value = row.get(key)
        if not isinstance(value, int) or not 0 <= value < 2**64:
            fail(prefix + f"invalid {key}")
        return value

    def exact(row: dict[str, int | str], keys: str) -> None:
        if set(row) != set(keys.split()):
            fail(prefix + f"unexpected fields: {sorted(row)}")

    def owned(cap: int) -> dict[str, int | str]:
        if cap not in nodes:
            fail(prefix + f"unknown parent {cap}")
        return nodes[cap]

    def consume_range(parent: int, start: int, size: int) -> None:
        node = owned(parent)
        base, length, used = (integer(node, key) for key in ("paddr", "bytes", "used"))
        if size == 0 or start != base + used or start + size > base + length:
            fail(prefix + "gap, overlap, or out-of-parent allocation")
        node["used"] = used + size

    def allocation(slot: int, size: int) -> tuple[int, int]:
        candidates = [(start, length) for cap, start, length in consumed if cap == slot]
        if len(candidates) != 1 or candidates[0][1] != size:
            fail(prefix + f"extent {slot} has no unique matching allocation")
        return candidates[0]

    def verify_snapshot(name: str) -> None:
        if census is None:
            fail(prefix + "snapshot has no preceding census")
        rows = snapshots[name]
        for category, kind in (("ordinary", "ordinary"), ("retained", "preserved")):
            expected = {cap: node for cap, node in nodes.items() if node["kind"] == kind}
            seen: set[int] = set()
            for row in rows[category]:
                exact(row, "parent paddr bytes used")
                cap = integer(row, "parent")
                if cap in seen or cap not in expected:
                    fail(prefix + "duplicate or unowned snapshot parent")
                seen.add(cap)
                if any(integer(row, key) != integer(expected[cap], key) for key in ("paddr", "bytes", "used")):
                    fail(prefix + "snapshot disagrees with allocation history")
            if seen != set(expected):
                fail(prefix + "snapshot omits owned backing")
        totals = {
            "untyped": sum(integer(node, "bytes") - integer(node, "used") for node in nodes.values() if node["kind"] == "ordinary"),
            "preserved_bytes": sum(integer(node, "bytes") - integer(node, "used") for node in nodes.values() if node["kind"] == "preserved"),
            "preserved_anchors": len(rows["retained"]),
            "active_extent_bytes": 0, "reusable": 0, "anchors": 0,
            "shared_reusable": 0, "shared_retained": 0, "shared_anchors": len(rows["shared_node"]),
        }
        task_caps: set[int] = set()
        for row in rows["task_extent"]:
            exact(row, "parent bytes active")
            cap, size, active = (integer(row, key) for key in ("parent", "bytes", "active"))
            if cap in task_caps or active not in (0, 1):
                fail(prefix + "duplicate task extent or invalid state")
            task_caps.add(cap)
            allocation(cap, size)
            totals["active_extent_bytes" if active else "reusable"] += size
            totals["anchors"] += int(not active)
        shared: dict[int, dict[str, int | str]] = {}
        children: dict[int, list[tuple[int, int]]] = {}
        for row in rows["shared_node"]:
            exact(row, "cap parent paddr bytes state")
            cap, parent, start, size = (integer(row, key) for key in ("cap", "parent", "paddr", "bytes"))
            if cap in shared or cap in task_caps or size == 0 or size & (size - 1) or start % size:
                fail(prefix + "duplicate, unaligned, or invalid shared extent")
            if row["state"] not in {"Free", "Split", "Leased", "Quarantined"}:
                fail(prefix + "invalid shared state")
            shared[cap] = row
            if parent == 0:
                if allocation(cap, size) != (start, size):
                    fail(prefix + "shared root differs from its allocation")
                totals["shared_retained"] += size
            else:
                children.setdefault(parent, []).append((start, size))
            if row["state"] == "Free":
                totals["shared_reusable"] += size
        for cap, row in shared.items():
            parent = integer(row, "parent")
            if parent and parent not in shared:
                fail(prefix + "shared child has no retained parent")
            child_ranges = sorted(children.get(cap, []))
            if row["state"] == "Split":
                start, size = integer(row, "paddr"), integer(row, "bytes")
                if child_ranges != [(start, size // 2), (start + size // 2, size // 2)]:
                    fail(prefix + "shared siblings do not partition parent")
            elif child_ranges:
                fail(prefix + "unsplit shared parent has children")
        # All event leaves and remaining tails must partition the initial
        # BootInfo inventory exactly. Parent/child ancestry is never summed.
        leaves = [(start, size) for slot, start, size in consumed if slot not in split_extents]
        leaves += [(integer(node, "paddr") + integer(node, "used"), integer(node, "bytes") - integer(node, "used"))
                   for node in nodes.values() if integer(node, "used") < integer(node, "bytes")]
        leaves.sort()
        index = 0
        for start, size in sorted(inventory):
            cursor = start
            while index < len(leaves) and leaves[index][0] < start + size:
                leaf_start, leaf_size = leaves[index]
                if leaf_start != cursor or leaf_size <= 0 or leaf_start + leaf_size > start + size:
                    fail(prefix + "exclusive leaves overlap, omit, or cross inventory")
                cursor += leaf_size
                index += 1
            if cursor != start + size:
                fail(prefix + "inventory is not fully represented")
        if index != len(leaves):
            fail(prefix + "backing lies outside ordinary inventory")
        for key, value in totals.items():
            if census.get(key) != value:
                fail(prefix + f"{name} {key}={census.get(key)}, exclusive ownership={value}")
        completed.append(name)

    for line in transcript.splitlines():
        if line.startswith("SLIME_MEM census "):
            census = {key: int(value) for key, value in re.findall(r"(\w+)=(\d+)", line)}
        if not line.startswith("SLIME_BACKING "):
            continue
        body = line.removeprefix("SLIME_BACKING ")
        if body.startswith("snapshot "):
            match = re.fullmatch(r"snapshot phase=(initial|final) (begin|end)", body)
            if match is None:
                fail(prefix + "malformed snapshot boundary")
            name, edge = match.groups()
            if edge == "begin":
                if phase is not None or name in snapshots or (name == "final" and completed != ["initial"]):
                    fail(prefix + "duplicate or reordered snapshot")
                phase = name
                snapshots[name] = {key: [] for key in ("ordinary", "retained", "task_extent", "shared_node")}
            else:
                if phase != name:
                    fail(prefix + "unpaired snapshot end")
                verify_snapshot(name)
                phase = None
            continue
        kind, _, payload = body.partition(" ")
        row = fields(payload)
        if phase is not None:
            if kind not in snapshots[phase]:
                fail(prefix + "allocation interleaved with frozen snapshot")
            snapshots[phase][kind].append(row)
            continue
        if kind == "inventory":
            exact(row, "parent paddr bytes")
            cap, start, size = (integer(row, key) for key in ("parent", "paddr", "bytes"))
            if cap in nodes or size == 0 or size & (size - 1) or start % size:
                fail(prefix + "duplicate or invalid inventory")
            if any(start < base + length and base < start + size for base, length in inventory):
                fail(prefix + "overlapping inventory")
            inventory.append((start, size))
            nodes[cap] = {"kind": "ordinary", "paddr": start, "bytes": size, "used": 0}
        elif kind == "infrastructure":
            exact(row, "parent anchor paddr bytes")
            parent, anchor, start, size = (integer(row, key) for key in ("parent", "anchor", "paddr", "bytes"))
            node = owned(parent)
            if node["kind"] != "ordinary" or start != integer(node, "paddr") or size != integer(node, "bytes"):
                fail(prefix + "infrastructure must exclusively own a complete ordinary source")
            consume_range(parent, start, size)
            consumed.append((anchor, start, size))
        elif kind == "infrastructure_tail":
            exact(row, "parent paddr bytes")
            parent, start, size = (integer(row, key) for key in ("parent", "paddr", "bytes"))
            node = owned(parent)
            if node["kind"] != "ordinary" or size != integer(node, "bytes") - integer(node, "used"):
                fail(prefix + "infrastructure tail must own the complete remaining ordinary tail")
            consume_range(parent, start, size)
            consumed.append((parent, start, size))
        elif kind in {"preserve", "split"}:
            exact(row, "parent child paddr bytes")
            parent, child, start, size = (integer(row, key) for key in ("parent", "child", "paddr", "bytes"))
            node = owned(parent)
            if child in nodes or size == 0 or size & (size - 1) or start % size:
                fail(prefix + "duplicate or invalid preserved child")
            if (kind == "preserve") != (node["kind"] == "ordinary"):
                fail(prefix + "wrong preservation parent kind")
            if kind == "split" and size * 2 != integer(node, "bytes"):
                fail(prefix + "split is not a buddy half")
            consume_range(parent, start, size)
            nodes[child] = {"kind": "preserved", "paddr": start, "bytes": size, "used": 0}
        elif kind == "extent_split":
            exact(row, "parent child paddr bytes")
            parent, child, start, size = (integer(row, key) for key in ("parent", "child", "paddr", "bytes"))
            base, length = allocation(parent, size * 2)
            halves = split_extents.setdefault(parent, [])
            if len(halves) >= 2 or start != base + len(halves) * size or any(slot == child for slot, _, _ in consumed):
                fail(prefix + "extent split child is not the next buddy half of its parent")
            halves.append(child)
            consumed.append((child, start, size))
        elif kind == "infrastructure_extent":
            # A returned extent adopted whole: it stays one owned leaf, now
            # held by infrastructure, so the partition does not change.
            exact(row, "parent paddr bytes")
            parent, start, size = (integer(row, key) for key in ("parent", "paddr", "bytes"))
            if parent in split_extents or allocation(parent, size) != (start, size):
                fail(prefix + "infrastructure adopted an extent it does not own whole")
        elif kind == "extent_merge":
            exact(row, "parent paddr bytes")
            parent, start, size = (integer(row, key) for key in ("parent", "paddr", "bytes"))
            if allocation(parent, size) != (start, size) or len(split_extents.get(parent, [])) != 2:
                fail(prefix + "merge of an extent that was not split in two")
            if any(child in split_extents for child in split_extents[parent]):
                fail(prefix + "merge of an extent whose child is still split")
            children = set(split_extents.pop(parent))
            consumed[:] = [entry for entry in consumed if entry[0] not in children]
        elif kind == "consume":
            exact(row, "source parent slot paddr bytes count")
            parent, slot, start, size, count = (integer(row, key) for key in ("parent", "slot", "paddr", "bytes", "count"))
            node = owned(parent)
            if row["source"] != node["kind"] or count == 0 or size % count:
                fail(prefix + "invalid allocation source or count")
            unit = size // count
            if unit == 0 or unit & (unit - 1) or start % unit:
                fail(prefix + "unaligned allocation")
            if node["kind"] == "preserved" and (count != 1 or size != integer(node, "bytes")):
                fail(prefix + "preserved allocation is not whole-leaf")
            consume_range(parent, start, size)
            consumed.append((slot, start, size))
        else:
            fail(prefix + f"unknown record {kind}")
    if phase is not None or completed != ["initial", "final"]:
        fail(prefix + "missing complete initial/final ownership snapshots")
    reported_inventory = [(int(start, 16), int(size)) for start, size in re.findall(
        r"^SLIME_ROOT ordinary range=\d+ paddr=(0x[0-9a-f]+) bytes=(\d+)$", transcript, re.MULTILINE)]
    if sorted(reported_inventory) != sorted(inventory):
        fail(prefix + "ledger inventory differs from BootInfo report")


def check_capacity_workload(transcript: str) -> None:
    """Join component observations to each root-created task incarnation."""
    prefix = "capacity workload: "
    match_marker_contract(transcript, CAPACITY_CHAINS, FAILURE_MARKERS, fail)
    lines = transcript.splitlines()
    spawns = list(re.finditer(
        r"^SLIME_GRAPH spawned task=(\d+) child=(\d+) component=private-memory-1g-holder-([abcd]) .*?$",
        transcript, re.MULTILINE,
    ))
    if [m.group(3) for m in spawns] != list("abcd") + ["d"] * 20:
        fail(prefix + "expected exactly four initial holders and twenty D replacements")
    task_ids = [m.group(2) for m in spawns]
    if len(set(task_ids)) != 24 or len({m.group(1) for m in spawns}) != 1:
        fail(prefix + "task identities are reused or owners differ")
    coordinator = spawns[0].group(1)
    expected_growths = []
    for ordinal, spawned in enumerate(spawns):
        task = spawned.group(2)
        total = min(ordinal + 1, 4) * 65536
        expected_growths.append((task, "65536", "0", "65536", "65536", str(total), "128", "0", "0"))
    grown = re.findall(
        r"^SLIME_MEM grown task=(\d+) delta=([1-9]\d*) previous=(\d+) pages=(\d+) "
        r"base=0x[0-9a-f]+ quota=(\d+) total=(\d+) large_frames=(\d+) base_frames=(\d+) leaf_tables=(\d+)$",
        transcript, re.MULTILINE,
    )
    if grown != expected_growths:
        fail(prefix + "growth attribution, aggregate population, or bulk frame shape differs")
    expected_zeroes = [(str(holder), "0") for holder in range(4)]
    expected_zeroes += [("3", str(cycle)) for cycle in range(1, 21)]
    zeroes = re.findall(r"^\[private-memory-1g\] zeroed holder=(\d+) incarnation=(\d+) pages=65536$", transcript, re.MULTILINE)
    if zeroes != expected_zeroes:
        fail(prefix + "zero-filled incarnation coverage differs")
    # Consume causally ordered evidence, allowing unrelated root diagnostics
    # between records but never accepting a report from another incarnation.
    position = 0

    def take(pattern: str) -> re.Match[str]:
        nonlocal position
        match = re.compile(pattern, re.MULTILINE).search(transcript, position)
        if match is None:
            fail(prefix + f"missing or reordered evidence: {pattern}")
        position = match.end()
        return match

    bases: dict[int, str] = {}

    def grow(spawned: re.Match[str], holder: int, incarnation: int) -> None:
        task = spawned.group(2)
        quota = take(rf"^SLIME_MEM quota task={task} instance=private-memory-1g-holder-{spawned.group(3)} declared=65536 installed=65536 base=(0x[0-9a-f]+)$")
        bases[holder] = quota.group(1)
        take(rf"^SLIME_GRAPH spawned task={coordinator} child={task} component=private-memory-1g-holder-{spawned.group(3)} .*$")
        take(rf"^SLIME_MEM grown task={task} delta=65536 previous=0 pages=65536 base={quota.group(1)} quota=65536 total=\d+ large_frames=128 base_frames=0 leaf_tables=0$")
        take(rf"^\[private-memory-1g\] zeroed holder={holder} incarnation={incarnation} pages=65536$")

    for holder, spawned in enumerate(spawns[:4]):
        grow(spawned, holder, 0)
    take(r"^\[private-memory-1g\] resident holders=4 pages=262144$")
    verification_reports: list[tuple[str, str, str, str]] = []
    rounds = [0, 0, 0, 0]
    current = task_ids[:4]
    censuses = []

    def verify(holder: int, incarnation: int) -> None:
        task = current[holder]
        take(rf"^SLIME_MEM refused task={task} delta=1 cause=reservation detail=ReservationExceeded \{{ pages: 65536, delta: 1, reservation: 65536 \}}$")
        take(rf"^SLIME_MEM grown task={task} delta=0 previous=65536 pages=65536 base=0x[0-9a-f]+ quota=65536 total=262144 large_frames=128 base_frames=0 leaf_tables=0$")
        if holder == 0:
            take(rf"^SLIME_GRAPH buffer created task={task} slot=\d+ id=\d+ pages=1 writable=1$")
            take(rf"^SLIME_GRAPH buffer create refused task={task} pages=1 class=quota$")
            take(rf"^SLIME_MEM mapping refused task={task} base=0x[0-9a-f]+ end=0x[0-9a-f]+ window=0x[0-9a-f]+\.\.0x[0-9a-f]+$")
        shared = int(holder == 0)
        take(rf"^\[private-memory-1g\] verified holder={holder} incarnation={incarnation} round={rounds[holder]} pages=65536 refused=1 shared={shared}$")
        verification_reports.append((str(holder), str(incarnation), str(rounds[holder]), str(shared)))
        rounds[holder] += 1

    for cycle in range(20):
        for holder in range(4):
            verify(holder, cycle if holder == 3 else 0)
        task = current[3]
        # The deliberate store lands on the guard page directly above the
        # holder's backed region, so the fault must name that exact address.
        guard = int(bases[3], 16) + 65536 * 4096
        take(rf"^SLIME_GRAPH component fault task={task} kind=VirtualMemory \{{ access: Write, status: [1-9]\d* \}} address=Some\({guard}\)$")
        census = take(rf"^SLIME_MEM census retired={task} (.*)$")
        censuses.append(census.group(1))
        if "mapped_pages=196608 " not in census.group(1):
            fail(prefix + "three peers were not resident during reclamation")
        take(rf"^SLIME_LIFECYCLE restart admitted task={coordinator} subject={task} attempt={cycle} remaining={19 - cycle} ready_at=\d+$")
        current[3] = task_ids[4 + cycle]
        rounds[3] = 0
        grow(spawns[4 + cycle], 3, cycle + 1)
        for holder in range(4):
            verify(holder, cycle + 1 if holder == 3 else 0)
        take(rf"^\[private-memory-1g\] retained cycle={cycle + 1} holders=4 pages=262144$")
    if len(set(censuses)) != 1:
        fail(prefix + "resources drift across equivalent holder deaths")
    for task in current:
        take(rf"^SLIME_GRAPH component exit task={task} status=0$")
        take(rf"^SLIME_MEM census retired={task} .*$")
    take(r"^\[private-memory-1g\] complete faults=20 replacements=20 exits=4$")
    take(r"^SLIME_GRAPH loans served=\d+ loans=0 mappings=0 regions=0 orphans=0 quota=0$")
    take(r"^SLIME_GRAPH HEALTHY generation=56 required=2 live=0 completed=2 failed=0$")
    actual_verified = re.findall(r"^\[private-memory-1g\] verified holder=(\d+) incarnation=(\d+) round=(\d+) pages=65536 refused=1 shared=(\d+)$", transcript, re.MULTILINE)
    if actual_verified != verification_reports:
        fail(prefix + "unexpected or duplicated retention acknowledgement")
    if re.findall(r"^\[private-memory-1g\] retained cycle=(\d+) holders=4 pages=262144$", transcript, re.MULTILINE) != [str(i) for i in range(1, 21)]:
        fail(prefix + "retention cycle sequence differs")
    faults = re.findall(r"^SLIME_GRAPH component fault task=(\d+) ", transcript, re.MULTILINE)
    if faults != task_ids[3:-1]:
        fail(prefix + "fault sequence differs")
    if len([line for line in lines if line.startswith("SLIME_LIFECYCLE restart admitted ")]) != 20:
        fail(prefix + "restart admission count differs")
    if len([line for line in lines if line == CAPACITY_COMPLETE]) != 1:
        fail(prefix + "completion must occur exactly once")


def check_capacity_image_profile(record: dict, platform: str, profile: dict) -> None:
    if record.get("platform") != platform or record.get("target_profile") != TARGET_PROFILES[platform]:
        fail("capacity image: identity target differs from requested platform")
    qemu = record.get("qemu")
    if not isinstance(qemu, dict) or any(qemu.get(key) != profile.get(key) for key in ("cpu", "cpus", "machine", "memory_mib")):
        fail("capacity image: execution profile differs from pinned identity")
    if qemu.get("version") != profile.get("qemu_version"):
        fail("capacity image: QEMU version differs from pinned identity")


def check_private_isolation(transcript: str) -> None:
    prefix = "private isolation: "
    for pattern in FAILURE_MARKERS:
        if re.search(pattern, transcript):
            fail(prefix + f"explicit failure: {pattern}")
    victim = re.search(r"^\[private-memory-isolation\] victim base=(\d+) pages=65536$", transcript, re.MULTILINE)
    if victim is None:
        fail(prefix + "missing victim population")
    address = int(victim.group(1))
    quotas = re.findall(r"^SLIME_MEM quota task=(\d+) instance=private-memory-1g-holder-([abc]) declared=(\d+) installed=(\d+) base=(0x[0-9a-f]+)$", transcript, re.MULTILINE)
    if len(quotas) != 3 or [entry[1] for entry in quotas] != list("abc"):
        fail(prefix + "wrong holder population")
    tasks = {name: task for task, name, *_ in quotas}
    if len(set(tasks.values())) != 3:
        fail(prefix + "task identity reused")
    for _task, name, declared, installed, base in quotas:
        expected = 65536 if name == "a" else 1
        # Every task's private window starts at the same virtual address, so the
        # attackers probe an address their own VSpace also names. Only VSpace
        # separation, not a distinct base, can refuse them.
        if (int(declared), int(installed), int(base, 16)) != (expected, expected, address):
            fail(prefix + "holder quota or window origin differs from its declaration")
    probe = address + 65536 * 4096 // 2
    growths = re.findall(r"^SLIME_MEM grown task=(\d+) delta=([1-9]\d*) previous=(\d+) pages=(\d+) base=(0x[0-9a-f]+) quota=(\d+) total=(\d+) large_frames=(\d+) base_frames=(\d+) leaf_tables=\d+$", transcript, re.MULTILINE)
    # Both attackers retain a backed page throughout the reciprocal exchange;
    # their relative growth order is scheduler-dependent, not their population.
    victim_growth = (tasks["a"], "65536", "0", "65536", hex(address), "65536", "65536", "128", "0")
    if len(growths) != 3 or growths[0] != victim_growth:
        fail(prefix + "holders did not each back exactly their declared window")
    if {entry[0] for entry in growths[1:]} != {tasks["b"], tasks["c"]}:
        fail(prefix + "attacker growth identity missing or duplicated")
    for total, entry in zip((65537, 65538), growths[1:], strict=True):
        if entry[1:] != ("1", "0", "1", hex(address), "1", str(total), "0", "1"):
            fail(prefix + "both attacker pages were not resident before the exchange")
    faults = re.findall(r"^SLIME_GRAPH component fault task=(\d+) kind=VirtualMemory \{ access: (\w+), status: (\d+) \} address=Some\((\d+)\)$", transcript, re.MULTILINE)
    expected_faults = [
        (tasks["b"], "Read", str(probe)),
        (tasks["c"], "Write", str(probe)),
        (tasks["a"], "Execute", str(address)),
    ]
    if [(task, access, at) for task, access, _, at in faults] != expected_faults or any(int(status) == 0 for _, _, status, _ in faults):
        fail(prefix + "wrong task, access, address or fault status")
    position = victim.end()
    def take(pattern: str) -> re.Match[str]:
        nonlocal position
        match = re.compile(pattern, re.MULTILINE).search(transcript, position)
        if match is None:
            fail(prefix + f"missing/reordered {pattern}")
        position = match.end()
        return match

    loan_ids: set[str] = set()
    transfer_ids: set[str] = set()
    for source, receiver, lender_name, receiver_name in ((1, 2, "b", "c"), (2, 1, "c", "b")):
        lender, recipient = tasks[lender_name], tasks[receiver_name]
        take(rf"^SLIME_GRAPH buffer created task={lender} slot=\d+ id=\d+ pages=1 writable=1$")
        created = take(rf"^SLIME_GRAPH loan created task={lender} slot=\d+ id=(\d+) to={recipient} offset=0 length=4096$")
        loan = created.group(1)
        if source == 1:
            before_exchange = transcript[:created.start()]
            for task in (tasks["b"], tasks["c"]):
                if not re.search(rf"^SLIME_MEM grown task={task} delta=1 previous=0 pages=1 ", before_exchange, re.MULTILINE):
                    fail(prefix + "loan exchange preceded an attacker's backed private page")
        exported = take(rf"^SLIME_GRAPH capability exported task={lender} id=(\d+) kind=loan rights=0x200 retain=0$").group(1)
        take(rf"^SLIME_GRAPH capability imported task={recipient} id={exported} kind=loan rights=0x200 retain=0$")
        for target in (address, probe):
            take(rf"^SLIME_MEM mapping refused task={recipient} base=0x{target:x} end=0x{target + 4096:x} window=0x{address:x}\.\.0x{address + 65536 * 4096:x}$")
        mapped = take(rf"^SLIME_GRAPH loan mapped task={recipient} slot=(\d+) id={loan}$")
        take(rf"^SLIME_GRAPH loan returned task={recipient} slot={mapped.group(1)} id={loan}$")
        take(rf"^\[private-memory-isolation\] loan receiver source={source} holder={receiver} positive=1 window_denied=2 unmapped=1 returned=1 return_denied=1 pages=0 buffers=0 mappings=0 loans=0$")
        take(rf"^\[private-memory-isolation\] loan lender holder={source} receiver={receiver} transferred=1 released=1 pages=0 buffers=0 mappings=0 loans=0$")
        if loan in loan_ids or exported in transfer_ids:
            fail(prefix + "loan or transfer identity reused across reciprocal directions")
        loan_ids.add(loan)
        transfer_ids.add(exported)
    for description, pattern, count in (
        ("loan creation", r"^SLIME_GRAPH loan created ", 2),
        ("loan export", r"^SLIME_GRAPH capability exported .* kind=loan ", 2),
        ("loan import", r"^SLIME_GRAPH capability imported .* kind=loan ", 2),
        ("loan mapping", r"^SLIME_GRAPH loan mapped task=", 2),
        ("loan return", r"^SLIME_GRAPH loan returned task=", 2),
        ("loan receiver", r"^\[private-memory-isolation\] loan receiver ", 2),
        ("loan lender", r"^\[private-memory-isolation\] loan lender ", 2),
        ("attacker report", r"^\[private-memory-isolation\] attacker ", 2),
        ("isolation completion", r"^\[private-memory-isolation\] complete ", 1),
        ("attacker window refusal", rf"^SLIME_MEM mapping refused task=(?:{tasks['b']}|{tasks['c']}) ", 8),
        ("attacker buffer creation", rf"^SLIME_GRAPH buffer created task=(?:{tasks['b']}|{tasks['c']}) ", 4),
    ):
        if len(re.findall(pattern, transcript, re.MULTILINE)) != count:
            fail(prefix + f"wrong {description} population")
    for holder, name, op, access in [(1, "b", 6, "Read"), (2, "c", 7, "Write")]:
        # The root's own records for the same peer: it really held a usable
        # buffer, and every attempt to place that buffer over a private window
        # was refused with the window the root resolved.
        buffer_slot = take(rf"^SLIME_GRAPH buffer created task={tasks[name]} slot=(\d+) id=\d+ pages=1 writable=1$").group(1)
        for target in (address, probe):
            take(rf"^SLIME_MEM mapping refused task={tasks[name]} base=0x{target:x} end=0x{target + 4096:x} window=0x{address:x}\.\.0x{address + 65536 * 4096:x}$")
        take(rf"^SLIME_GRAPH buffer map refused task={tasks[name]} slot={buffer_slot} class=write$")
        take(rf"^\[private-memory-isolation\] attacker holder={holder} operation={op} address={probe} own_base={address} own_pages=1 own_buffer=1 buffer_window_denied=2 unowned_denied=3 seal_denied=1 pages=0 buffers=0 mappings=0 loans=0$")
        take(rf"^SLIME_GRAPH component fault task={tasks[name]} kind=VirtualMemory \{{ access: {access}, status: [1-9]\d* \}} address=Some\({probe}\)$")
        take(rf"^\[private-memory-1g\] verified holder=0 incarnation=0 round={holder - 1} pages=65536 refused=1 shared=1$")
        take(rf"^\[private-memory-isolation\] denied holder={holder} operation={op} address={probe} victim_preserved=1$")
    take(rf"^\[private-memory-isolation\] execute address={address} pages=65536$")
    take(rf"^SLIME_GRAPH component fault task={tasks['a']} kind=VirtualMemory \{{ access: Execute, status: [1-9]\d* \}} address=Some\({address}\)$")
    take(r"^\[private-memory-isolation\] complete read=1 write=1 execute=1 buffer_window_denied=4 loan_window_denied=4 unowned_denied=6 seal_denied=2 loan_transfer=2$")
    take(r"^SLIME_GRAPH native task_caps=0 exports=0 tickets=0$")
    take(r"^SLIME_GRAPH capabilities exports=2 imports=2 cancels=0 finalized=2 outstanding=0 tickets=0$")
    take(r"^SLIME_GRAPH loans served=\d+ loans=0 mappings=0 regions=0 orphans=0 quota=0$")
    take(r"^SLIME_GRAPH HEALTHY generation=57 required=2 live=0 completed=2 failed=0$")
    match_marker_contract(transcript, ISOLATION_CHAINS, FAILURE_MARKERS, fail)


def check_stress_workload(transcript: str) -> None:
    prefix = "private stress: "
    for pattern in FAILURE_MARKERS:
        if re.search(pattern, transcript):
            fail(prefix + f"explicit failure: {pattern}")
    spawns = list(re.finditer(r"^SLIME_GRAPH spawned task=(\d+) child=(\d+) component=private-memory-1g-holder-([abcd]) .*?$", transcript, re.MULTILINE))
    if [m.group(3) for m in spawns] != list("abcdd") or len({m.group(2) for m in spawns}) != 5 or len({m.group(1) for m in spawns}) != 1:
        fail(prefix + "wrong successful holder population")
    peers = [m.group(2) for m in spawns[:3]]
    subjects = [m.group(2) for m in spawns[3:]]
    position = 0

    def take(pattern: str) -> re.Match[str]:
        nonlocal position
        match = re.compile(pattern, re.MULTILINE).search(transcript, position)
        if match is None:
            fail(prefix + f"missing/reordered {pattern}")
        position = match.end()
        return match

    for index, task in enumerate(peers):
        take(rf"^SLIME_MEM grown task={task} delta=65536 previous=0 pages=65536 base=0x[0-9a-f]+ quota=65536 total={(index + 1) * 65536} large_frames=128 base_frames=0 leaf_tables=0$")
    census_pattern = r"slots=(\d+) descriptors=(\d+) extents=(\d+) objects=(\d+) bytes=(\d+) reusable_anchors=(\d+) reusable_bytes=(\d+) ordinary_bytes=(\d+) preserved_bytes=(\d+) preserved_anchors=(\d+) descriptor_capacity=(\d+) extent_capacity=(\d+) infrastructure_owned=(\d+)"
    for attempt, case in enumerate(("construction", "slots", "descriptors")):
        before = take(rf"^SLIME_MEM stress census attempt={attempt} phase=before {census_pattern}$")
        injection = take(rf"^SLIME_MEM stress construction case={case} attempt={attempt} actual=(\d+) effective=(\d+) required=(\d+) reserved=(\d+)$")
        actual, effective, required, reserved = map(int, injection.groups())
        if case == "construction" and reserved != 0:
            fail(prefix + "unexpected construction pressure reservation")
        if case != "construction" and not (actual >= required > effective >= 0 and reserved == actual - effective > 0):
            fail(prefix + "resource pressure was not an effective limit below the required allocation")
        after = take(rf"^SLIME_MEM stress census attempt={attempt} phase=after {census_pattern}$")
        pre, post = tuple(map(int, before.groups())), tuple(map(int, after.groups()))
        if (pre[0] + pre[5] + pre[9], pre[10] - pre[1], pre[11] - pre[2] - pre[5], pre[3] - pre[5] - pre[9], pre[4] - pre[6], sum(pre[6:9]) + pre[12]) != (post[0] + post[5] + post[9], post[10] - post[1], post[11] - post[2] - post[5], post[3] - post[5] - post[9], post[4] - post[6], sum(post[6:9]) + post[12]):
            fail(prefix + "failed construction leaked resources beyond retained backing anchors")
        for holder in range(3):
            take(rf"^\[private-memory-1g\] verified holder={holder} incarnation=0 round=\d+ pages=65536 refused=1 shared={int(holder == 0)}$")
        take(rf"^\[private-memory-stress\] spawn_refused attempt={attempt + 1} peers_preserved=3$")
    fragmented = re.findall(r"^SLIME_MEM stress fragmented guards=(\d+) bytes=(\d+) released=1$", transcript, re.MULTILINE)
    if fragmented != [("4", "16777216")]:
        fail(prefix + "missing bounded fragmentation and guard release")
    backing = re.findall(r"^SLIME_MEM stress backing attempt=(\d+) data_extents=(\d+) discontinuities=(\d+) reused=(\d+)$", transcript, re.MULTILINE)
    if [int(row[0]) for row in backing] != [0, 1, 3, 4]:
        fail(prefix + "wrong fragmented backing population")
    for attempt, data, gaps, reused in backing:
        if int(data) != 128 or int(gaps) == 0 or (int(attempt) != 0 and int(reused) < 256):
            fail(prefix + "backing was not fragmented and reused at raised quota")
    expected_stages = []
    retired_ledgers = []
    reuse_counts = []
    for incarnation, task in enumerate(subjects):
        stages = [(12, 1), (12, 511)]
        if incarnation == 0:
            stages.append((13, 1024))
        stages += [(12, 1024), (12, 63998)]
        if incarnation == 0:
            stages.append((13, 2))
        stages += [(12, 2), (14, 0)]
        previous = 0
        for stage, (operation, delta) in enumerate(stages):
            if operation == 13:
                case, backed = ("map", 512) if previous == 512 else ("allocation", 1)
                take(rf"^SLIME_MEM stress growth case={case} previous={previous} delta={delta} backed={backed}$")
                take(rf"^SLIME_MEM refused task={task} delta={delta} cause=frames detail=Frames \{{ allocated: {backed},.*$")
                pages = previous
            elif operation == 12:
                pages = previous + delta
                grown = take(rf"^SLIME_MEM grown task={task} delta={delta} previous={previous} pages={pages} base=0x[0-9a-f]+ quota=65536 total={196608 + pages} large_frames=(\d+) base_frames=(\d+) leaf_tables=(\d+)$")
                large, base, tables = map(int, grown.groups())
                if large * 512 + base != pages or base < 1 or tables < 1 or (pages == 65536 and large == 0):
                    fail(prefix + "mixed backing shape does not account for committed pages")
            else:
                pages = previous
            marker = (str(incarnation), str(operation), str(delta), str(previous), str(pages), str(int(operation == 12)))
            take(rf"^\[private-memory-stress\] stage incarnation={incarnation} operation={operation} delta={delta} previous={previous} pages={pages} zeroed={int(operation == 12)} preserved=1$")
            expected_stages.append(marker)
            if operation != 14:
                for holder in range(3):
                    take(rf"^\[private-memory-1g\] verified holder={holder} incarnation=0 round=\d+ pages=65536 refused=1 shared={int(holder == 0)}$")
                take(rf"^\[private-memory-stress\] retained incarnation={incarnation} stage={stage} peers=3 pages=196608$")
            previous = pages
        if previous != 65536:
            fail(prefix + "subject did not reach full declared quota")
        if incarnation == 0:
            take(rf"^SLIME_GRAPH component exit task={task} status=0$")
        else:
            quota = re.search(rf"^SLIME_MEM quota task={task} instance=private-memory-1g-holder-d declared=65536 installed=65536 base=(0x[0-9a-f]+)$", transcript, re.MULTILINE)
            if quota is None:
                fail(prefix + "replacement quota absent")
            guard = int(quota.group(1), 16) + 65536 * 4096
            take(rf"^SLIME_GRAPH component fault task={task} kind=VirtualMemory \{{ access: Write, status: [1-9]\d* \}} address=Some\({guard}\)$")
        retired = take(rf"^SLIME_MEM census retired={task} (.*)$")
        retired_ledgers.append(retired.group(1))
        reuse = take(rf"^SLIME_ROOT reclaim census task={task} slots=\d+ bytes=\d+ live_objects=\d+ extent_reuses=(\d+)$")
        reuse_counts.append(int(reuse.group(1)))
        if "mapped_pages=196608 " not in retired.group(1):
            fail(prefix + "retained peers were not resident at subject teardown")
        for holder in range(3):
            take(rf"^\[private-memory-1g\] verified holder={holder} incarnation=0 round=\d+ pages=65536 refused=1 shared={int(holder == 0)}$")
    if retired_ledgers[0] != retired_ledgers[1] or reuse_counts[1] <= reuse_counts[0]:
        fail(prefix + "equivalent subject reclamation drifted or backing was not reused")
    actual_stages = re.findall(r"^\[private-memory-stress\] stage incarnation=(\d+) operation=(\d+) delta=(\d+) previous=(\d+) pages=(\d+) zeroed=(\d+) preserved=1$", transcript, re.MULTILINE)
    if actual_stages != expected_stages:
        fail(prefix + "stage population differs")
    for task in peers:
        take(rf"^SLIME_GRAPH component exit task={task} status=0$")
    take(r"^\[private-memory-stress\] complete spawn_refused=3 growth_refused=2 retries=2 replacements=1 faults=1 exits=4$")
    take(r"^SLIME_GRAPH native task_caps=0 exports=0 tickets=0$")
    take(r"^SLIME_GRAPH loans served=\d+ loans=0 mappings=0 regions=0 orphans=0 quota=0$")
    take(r"^SLIME_GRAPH HEALTHY generation=\d+ required=2 live=0 completed=2 failed=0$")
    for pattern, count in ((r"^SLIME_MEM stress construction ", 3), (r"^SLIME_MEM stress growth ", 2), (r"^SLIME_GRAPH component fault ", 1)):
        if len(re.findall(pattern, transcript, re.MULTILINE)) != count:
            fail(prefix + "unexpected injection or fault population")


def check_heap_stress_workload(transcript: str) -> None:
    prefix = "heap stress: "
    for pattern in (*FAILURE_MARKERS, r"\[private-heap-probe:stress\] FAIL", r"SLIME_MEM stress "):
        if re.search(pattern, transcript):
            fail(prefix + f"explicit failure or unexpected injection: {pattern}")
    position = 0
    def take(pattern: str) -> re.Match[str]:
        nonlocal position
        match = re.compile(pattern, re.MULTILINE).search(transcript, position)
        if match is None:
            fail(prefix + f"missing/reordered {pattern}")
        position = match.end()
        return match
    peers = []
    for index, name in enumerate("abc"):
        spawned = take(rf"^SLIME_GRAPH spawned task=\d+ child=(\d+) component=private-memory-1g-holder-{name} .*?$")
        task = spawned.group(1)
        peers.append(task)
        take(rf"^SLIME_MEM grown task={task} delta=65536 previous=0 pages=65536 base=0x[0-9a-f]+ quota=65536 total={(index + 1) * 65536} large_frames=128 base_frames=0 leaf_tables=0$")
    quota = take(r"^SLIME_MEM quota task=(\d+) instance=private-heap-probe declared=65536 installed=65536 base=(0x[0-9a-f]+)$")
    heap_task, heap_base = quota.groups()
    take(rf"^SLIME_GRAPH spawned task=\d+ child={heap_task} component=private-heap-probe .*?$")
    if len(set([*peers, heap_task])) != 4:
        fail(prefix + "duplicate holder identity")

    def verify_peers() -> None:
        for holder in range(3):
            take(rf"^\[private-memory-1g\] verified holder={holder} incarnation=0 round=\d+ pages=65536 refused=1 shared={int(holder == 0)}$")

    verify_peers()
    growth_start = position
    report = take(r"^\[private-heap-probe:stress\] capacity payload=251723776 overhead=(\d+) backed=(\d+) pages=(\d+) touched=1 vecs=120 boxes=120 small=256$")
    overhead, backed, pages = map(int, report.groups())
    if backed != pages * 4096 or not (251723776 + overhead <= backed <= 65536 * 4096):
        fail(prefix + "payload, allocator overhead and backing do not fit declared quota")
    growth_end = report.start()
    growths = list(re.finditer(rf"^SLIME_MEM grown task={heap_task} delta=(\d+) previous=(\d+) pages=(\d+) base={heap_base} quota=65536 total=(\d+) large_frames=\d+ base_frames=\d+ leaf_tables=\d+$", transcript[growth_start:growth_end], re.MULTILINE))
    charged = 0
    for growth in growths:
        delta, previous, current, total = map(int, growth.groups())
        if delta == 0 or previous != charged or current != previous + delta or total != 196608 + current:
            fail(prefix + "heap growth has wrong extent or resident population")
        charged = current
    if charged != pages:
        fail(prefix + "heap report differs from attributed root backing")
    holes = take(rf"^\[private-heap-probe:stress\] holes reused=1 growths=(\d+) pages={pages}$")
    if int(holes.group(1)) != len(growths):
        fail(prefix + "heap growth count differs from root")
    take(rf"^\[private-heap-probe:stress\] exhaustion requested=16777216 refused=1 intact=1 pages={pages}$")
    verify_peers()
    take(r"^\[private-heap-probe:stress\] verified payload=251723776 intact=1$")
    verify_peers()
    released = take(rf"^\[private-heap-probe:stress\] released live=0 reused=1 growths={holes.group(1)} pages={pages}$")
    if re.search(rf"^SLIME_MEM grown task={heap_task} ", transcript[report.end():released.end()], re.MULTILINE):
        fail(prefix + "heap reuse or refusal grew backing")
    take(rf"^SLIME_GRAPH component exit task={heap_task} status=0$")
    verify_peers()
    for task in peers:
        take(rf"^SLIME_GRAPH component exit task={task} status=0$")
    take(r"^\[private-memory-stress\] heap_complete peers=3 exits=4$")
    take(r"^SLIME_GRAPH native task_caps=0 exports=0 tickets=0$")
    take(r"^SLIME_GRAPH loans served=\d+ loans=0 mappings=0 regions=0 orphans=0 quota=0$")
    take(r"^SLIME_GRAPH HEALTHY generation=\d+ required=2 live=0 completed=2 failed=0$")
    if re.search(r"^SLIME_GRAPH component fault ", transcript, re.MULTILINE):
        fail(prefix + "heap or retained peer faulted")


def adaptive_rows(transcript: str, marker: str, pattern: str, prefix: str) -> list[re.Match[str]]:
    rows = list(re.finditer(r"^" + pattern + r"$", transcript, re.MULTILINE))
    if len(rows) != len(re.findall(r"^" + re.escape(marker) + r"(?: |$)", transcript, re.MULTILINE)):
        fail(prefix + f"malformed {marker}")
    return rows


def check_adaptive_markers(transcript: str, chains, prefix: str) -> None:
    match_marker_contract(
        transcript,
        tuple((label, tuple("(?m)^" + pattern + "$" for pattern in patterns)) for label, patterns in chains),
        FAILURE_MARKERS,
        lambda message: fail(prefix + message),
    )


def check_cspace_execution(transcript: str) -> None:
    prefix = "cspace execution: "
    check_adaptive_markers(transcript, CSPACE_CHAINS, prefix)

    def rows(name: str, fields: str) -> list[re.Match[str]]:
        marker = "SLIME_ROOT cspace " + name
        return adaptive_rows(transcript, marker, marker + " " + fields, prefix)

    pressure = rows("pressure", r"initial_limit=(\d+) retired=(\d+) free=(\d+)")
    exercised = rows("exercised", r"created=(\d+) invoked=(\d+) copied=(\d+) retyped=(\d+) deleted=(\d+) revoked=(\d+) min_address=(\d+) initial_limit=(\d+)")
    live = rows("live", r"root=(\d+) second=(\d+) leaves=(\d+)")
    expanded = rows("expanded", r"base=(\d+) infrastructure_caps=(\d+)")
    leaves = rows("leaf", r"base=(\d+) slots=(\d+) beyond_initial=(\d+)")
    if any(len(group) != 1 for group in (pressure, exercised, live, expanded)) or not leaves:
        fail(prefix + "wrong marker population")
    limit, retired, free = map(int, pressure[0].groups())
    counters = tuple(map(int, exercised[0].groups()))
    if limit != 1 << 19 or counters[-1] != limit:
        fail(prefix + "initial CNode limit differs from kernel width")
    if retired == 0 or free >= 1024:
        fail(prefix + "initial namespace was not exhausted")
    bases = set()
    for leaf in leaves:
        base, slots, beyond = map(int, leaf.groups())
        if base < 1 << 60 or slots != 1024 or beyond != 1 or base in bases:
            fail(prefix + "invalid or duplicate expanded leaf")
        if leaf.start() <= pressure[0].end():
            fail(prefix + "leaf admitted before pressure")
        bases.add(base)
    if any(value == 0 for value in counters[:6]) or counters[-2] <= limit:
        fail(prefix + "kernel operations did not exercise expanded addresses")
    leaves_before_live = sum(leaf.start() < live[0].start() for leaf in leaves)
    if tuple(map(int, live[0].groups())) != (1, 1, leaves_before_live):
        fail(prefix + "root/second-thread liveness or leaf count differs")
    slots = []
    for line in transcript[pressure[0].end():].splitlines():
        if line.startswith(("SLIME_ALLOC ", "SLIME_BACKING ", "SLIME_MEM ")):
            slots.extend(int(value) for value in re.findall(r"\bslot=(\d+)(?= |$)", line))
    if not slots or any(slot <= limit for slot in slots):
        fail(prefix + "workload allocation aliases the initial namespace or lacks slot evidence")


def check_metadata_lifecycle(transcript: str) -> None:
    prefix = "metadata lifecycle: "
    check_adaptive_markers(transcript, METADATA_CHAINS, prefix)
    pattern = r"SLIME_ROOT metadata census phase=(baseline|grown|released|final) " + " ".join(field + r"=(\d+)" for field in METADATA_FIELDS)
    rows = adaptive_rows(transcript, "SLIME_ROOT metadata census", pattern, prefix)
    if [row.group(1) for row in rows] != ["baseline", "grown", "released", "final"]:
        fail(prefix + "wrong census phase population/order")
    census = [dict(zip(METADATA_FIELDS, map(int, row.groups()[1:]), strict=True)) for row in rows]
    baseline, grown, released, final = census
    if grown["pages"] <= baseline["pages"] or released["pages"] != grown["pages"] + 1:
        fail(prefix + "metadata growth or retried page publication differs")
    if grown["nodes"] != baseline["nodes"] or released["nodes"] != baseline["nodes"] + 1:
        fail(prefix + "retried construction did not publish exactly one node")
    if not baseline["allocations_free"] < grown["allocations_free"] < released["allocations_free"]:
        fail(prefix + "quarantined metadata page became reusable before retry")
    for field in ("allocations_live", "extents_live", "preserved_live", "slots_free"):
        if released[field] != baseline[field]:
            fail(prefix + f"released baseline leaked {field}")
    if any(final[field] < released[field] for field in ("pages", "bytes", "nodes")):
        fail(prefix + "final retained pool regressed")
    if any(later["bytes"] < earlier["bytes"] for earlier, later in zip(census, census[1:], strict=False)):
        fail(prefix + "infrastructure-owned bytes decreased")
    if any(row["pending"] != expected for row, expected in zip(census, (0, 1, 0, 0), strict=True)) or grown["quarantined"] == 0 or any(row["quarantined"] != 0 for row in (baseline, released, final)):
        fail(prefix + "quarantine/pending lifecycle differs")
    rounds = list(re.finditer(r"^SLIME_ROOT metadata round=(\d+) grew=(\d+) reused=(\d+)$", transcript, re.MULTILINE))
    # Round numbers belong to the marker itself, unlike the named census fields.
    if len(rounds) != len(re.findall(r"^SLIME_ROOT metadata round=", transcript, re.MULTILINE)):
        fail(prefix + "malformed round evidence")
    if [int(row.group(1)) for row in rounds] != [0, 1, 2]:
        fail(prefix + "wrong round population/order")
    if not any(int(earlier.group(2)) > 0 and int(later.group(3)) > 0 for index, earlier in enumerate(rounds) for later in rounds[index + 1:]):
        fail(prefix + "growth did not fund later reuse")
    if not all(rows[0].end() < row.start() < rows[1].start() for row in rounds):
        fail(prefix + "rounds outside baseline/grown interval")
    injected = adaptive_rows(transcript, "SLIME_ROOT metadata injected", r"SLIME_ROOT metadata injected kind=(construction|revoke) retained=(\d+) reassigned=(\d+)", prefix)
    retries = adaptive_rows(transcript, "SLIME_ROOT metadata retry", r"SLIME_ROOT metadata retry kind=(construction|revoke) released=(\d+) reassigned=(\d+)", prefix)
    if sorted(row.group(1) for row in injected) != ["construction", "revoke"] or sorted(row.group(1) for row in retries) != ["construction", "revoke"]:
        fail(prefix + "wrong injection/retry population")
    for injection in injected:
        retry = next(row for row in retries if row.group(1) == injection.group(1))
        if int(injection.group(2)) == 0 or injection.group(3) != "0" or retry.groups()[1:] != ("1", "0"):
            fail(prefix + "quarantined resource reassigned or not released")
        if not rounds[-1].end() < injection.start() < rows[1].start() < rows[1].end() < retry.start() < rows[2].start():
            fail(prefix + "missing pending census between injection and retry")


def check_bootstrap_boundaries(transcript: str) -> None:
    prefix = "bootstrap boundaries: "
    check_adaptive_markers(transcript, BOOTSTRAP_CHAINS, prefix)
    source = adaptive_rows(transcript, "SLIME_ROOT metadata bootstrap", r"SLIME_ROOT metadata bootstrap objects=(\d+) alignment=(\d+) remaining=(\d+)", prefix)
    reserve = adaptive_rows(transcript, "SLIME_ROOT bootstrap reserve", r"SLIME_ROOT bootstrap reserve objects=(\d+) alignment=(\d+) remaining=(\d+) slots=(\d+) transaction=(\d+) recursion=(\d+) fit=(\d+)", prefix)
    exhausted = adaptive_rows(transcript, "SLIME_ROOT bootstrap exhausted", r"SLIME_ROOT bootstrap exhausted cause=(ram|cnode-slots|metadata) published=(\d+) refused=(\d+) ram_free=(\d+) slots_free=(\d+) metadata_free=(\d+)", prefix)
    complete = adaptive_rows(transcript, "SLIME_ROOT bootstrap boundaries complete", r"SLIME_ROOT bootstrap boundaries complete cases=(\d+) published=(\d+)", prefix)
    if any(len(group) != 1 for group in (source, reserve, complete)):
        fail(prefix + "wrong reservation/completion population")
    values = tuple(map(int, reserve[0].groups()))
    if values[3] != 64 or values[4] >= values[3] or values[5:] != (0, 1) or sum(values[:3]) != sum(map(int, source[0].groups())):
        fail(prefix + "reserve exceeds adopted source or transaction bounds")
    causes = ("ram", "cnode-slots", "metadata")
    if sorted(row.group(1) for row in exhausted) != sorted(causes):
        fail(prefix + "wrong exhaustion cause population")
    publication = re.search(r"^SLIME_GRAPH (?:staged task=|spawned |activated )", transcript, re.MULTILINE)
    if publication is None or complete[0].groups() != ("3", "1") or complete[0].end() >= publication.start():
        fail(prefix + "completion did not precede first publication")
    for row in exhausted:
        counters = tuple(map(int, row.groups()[3:]))
        if row.groups()[1:3] != ("0", "1") or any((value == 0) != (index == causes.index(row.group(1))) for index, value in enumerate(counters)):
            fail(prefix + "exhaustion was not independent and unpublished")
        if not reserve[0].end() < row.start() < complete[0].start():
            fail(prefix + "exhaustion outside reserve/completion interval")


def elastic_rows(transcript: str, name: str, fields: str, prefix: str) -> list[re.Match[str]]:
    marker = "SLIME_MEM elastic " + name
    return adaptive_rows(transcript, marker, marker + " " + fields, prefix)


def check_idle_and_guarantee(transcript: str) -> None:
    prefix = "elastic guarantee: "
    check_adaptive_markers(transcript, ELASTIC_CHAINS, prefix)
    idle = elastic_rows(
        transcript,
        "idle",
        r"subject=(\S+) maximum=(\d+) reserved_bytes=(\d+) pool_before=(\d+) pool_after=(\d+)",
        prefix,
    )
    grow = elastic_rows(
        transcript, "grow", r"subject=(\S+) committed=(\d+) bytes=(\d+) pool_after=(\d+)", prefix
    )
    refused = elastic_rows(
        transcript,
        "refused",
        r"subject=(\S+) pages=(\d+) cause=(\S+) committed=(\d+) peer_committed=(\d+)",
        prefix,
    )
    guarantee = elastic_rows(
        transcript, "guarantee", r"subject=(\S+) committed=(\d+) promised=(\d+) served=(\d+)", prefix
    )
    if len(idle) != 1 or len(grow) != 1 or len(refused) != 1 or len(guarantee) != 1:
        fail(prefix + "wrong holder population")
    maximum, reserved, before, after = (int(value) for value in idle[0].groups()[1:])
    if reserved != 0 or before != after or maximum == 0:
        fail(prefix + "an authorized maximum reserved pool capacity")
    committed, bytes_backed = (int(value) for value in grow[0].groups()[1:3])
    if committed == 0 or bytes_backed != committed * 4096:
        fail(prefix + "the peer consumed no spare capacity")
    # The peer's idle neighbour held nothing, and the refusal did not shrink
    # what the peer already had.
    if int(refused[0].group(4)) != committed or int(refused[0].group(5)) != 0:
        fail(prefix + "a refusal changed a holder's committed pages")
    served, promised = int(guarantee[0].group(4)), int(guarantee[0].group(3))
    if served != 1 or int(guarantee[0].group(2)) != promised:
        fail(prefix + "the guarantee was not serviceable under elastic pressure")
    if refused[0].start() >= guarantee[0].start():
        fail(prefix + "the guarantee was served before the pool was exhausted")


def check_mixed_fragmentation(transcript: str) -> None:
    prefix = "elastic fragmentation: "
    check_adaptive_markers(transcript, FRAGMENTATION_CHAINS, prefix)
    requests = {
        row.group(1): tuple(int(value) for value in row.groups()[1:])
        for row in elastic_rows(
            transcript,
            "request",
            r"kind=(\S+) pages=(\d+) committed=(\d+) charged=(\d+) large=(\d+) base=(\d+) tables=(\d+)",
            prefix,
        )
    }
    if sorted(requests) != ["bulk", "mixed", "single"]:
        fail(prefix + "wrong request population")
    if requests["single"][2] != 2 * 4096:
        fail(prefix + "a one-page request charged more than a page and its table")
    if requests["mixed"][3] == 0 or requests["mixed"][4] == 0:
        fail(prefix + "the mixed request took only one frame size")
    cost = elastic_rows(
        transcript, "cost", r"small_bytes_per_page=(\d+) bulk_bytes_per_page=(\d+)", prefix
    )
    if len(cost) != 1:
        fail(prefix + "per-page costs were not reported")
    small, bulk = (int(value) for value in cost[0].groups())
    if bulk >= small:
        fail(prefix + "bulk capacity was reported as the small-page cost")
    fragmented = elastic_rows(
        transcript,
        "fragmented",
        r"ordinary_aligned=(\d+) served=(\d+) committed=(\d+) retained=(\d+) reusable=(\d+)",
        prefix,
    )
    span = elastic_rows(
        transcript, "fragmented_span", r"served=(\d+) cause=(\S+) committed=(\d+)", prefix
    )
    if len(fragmented) != 1 or len(span) != 1:
        fail(prefix + "wrong fragmented-service population")
    if int(fragmented[0].group(1)) != 0 or int(fragmented[0].group(2)) != 1:
        fail(prefix + "page-granular growth required an aligned block")
    if int(span[0].group(1)) != 0 or span[0].group(2) in ("none", "policy"):
        fail(prefix + "an unbackable span request was not refused by a named resource")
    reuse = elastic_rows(
        transcript,
        "reuse",
        r"returned_pages=(\d+) reused_extents=(\d+) served=(\d+) committed=(\d+)",
        prefix,
    )
    if len(reuse) != 1 or int(reuse[0].group(1)) == 0 or int(reuse[0].group(2)) == 0:
        fail(prefix + "returned extents were not reused")


def check_failure_rollback(transcript: str) -> None:
    prefix = "elastic rollback: "
    check_adaptive_markers(transcript, ROLLBACK_CHAINS, prefix)
    baseline = elastic_rows(
        transcript, "baseline", r"committed=(\d+) charged=(\d+) pool_bytes=(\d+) peer=(\d+)", prefix
    )
    injected = elastic_rows(
        transcript,
        "injected",
        r"stage=(\S+) cause=(\S+) committed=(\d+) pool_before=(\d+) pool_after=(\d+) "
        r"ordinary_before=(\d+) ordinary_after=(\d+) retained_tables=(\d+) sentinels=(\d+) "
        r"quarantined=(\d+)",
        prefix,
    )
    if len(baseline) != 1 or [row.group(1) for row in injected] != list(ROLLBACK_STAGES):
        fail(prefix + "wrong injected-stage population or order")
    committed = int(baseline[0].group(1))
    for row in injected:
        values = row.groups()
        if int(values[2]) != committed:
            fail(prefix + f"stage {values[0]} changed committed pages")
        if values[8] != "1" or values[9] != "0":
            fail(prefix + f"stage {values[0]} damaged a holder or left resources owned")
        # Everything a failed growth took comes back except a leaf table it
        # already bound to a span, which stays charged by exactly its bytes.
        retained = int(values[7]) * 4096
        if int(values[3]) - int(values[4]) != retained:
            fail(prefix + f"stage {values[0]} did not return its pool bytes")
    if not any(int(row.group(8)) for row in injected):
        fail(prefix + "no stage retained a span-bound leaf table")
    retry = elastic_rows(
        transcript, "retry", r"stage=all committed=(\d+) charged=(\d+) clean=(\d+) doubled=(\d+)", prefix
    )
    if len(retry) != 1 or retry[0].group(4) != "0" or int(retry[0].group(2)) > int(retry[0].group(3)):
        fail(prefix + "a retry after the injected failures charged twice")
    quarantine = elastic_rows(
        transcript,
        "quarantine",
        r"stage=cleanup cause=(\S+) owned=(\d+) refused=(\d+) released=(\d+) remaining=(\d+) "
        r"pool_held=(\d+) pool_after=(\d+)",
        prefix,
    )
    if len(quarantine) != 1:
        fail(prefix + "the cleanup failure was not reported")
    owned, refused, released, remaining = (int(value) for value in quarantine[0].groups()[1:5])
    if (owned, refused, released, remaining) != (1, 1, 1, 0):
        fail(prefix + "a failed cleanup was not owned, refusing and retryable exactly once")
    if injected[-1].start() >= quarantine[0].start():
        fail(prefix + "the quarantine case did not follow the injected stages")


def check_cross_holder_conservation(transcript: str) -> None:
    prefix = "elastic conservation: "
    check_adaptive_markers(transcript, CONSERVATION_CHAINS, prefix)
    retire = elastic_rows(
        transcript,
        "retire",
        r"subject=(\S+) revoked=(\d+) returned_pages=(\d+) pool_before=(\d+) pool_after=(\d+) "
        r"reusable=(\d+)",
        prefix,
    )
    reuse = elastic_rows(
        transcript,
        "reuse",
        r"subject=(\S+) committed=(\d+) reused_extents=(\d+) zeroed=(\d+) peer_pattern=(\d+)",
        prefix,
    )
    stuck = elastic_rows(
        transcript,
        "revoke_failure",
        r"subject=(\S+) first_attempt=(\d+) retained=(\d+) retry=(\d+) pool_before=(\d+) "
        r"pool_held=(\d+) pool_after=(\d+)",
        prefix,
    )
    final = elastic_rows(
        transcript,
        "baseline",
        r"phase=final owned_before=(\d+) owned_after=(\d+) slots_before=(\d+) slots_after=(\d+) "
        r"pool_before=(\d+) pool_after=(\d+) granted=(\d+) reclaimed=(\d+)",
        prefix,
    )
    if len(retire) != 1 or len(reuse) != 1 or len(stuck) != 1 or len(final) != 1:
        fail(prefix + "wrong conservation population")
    returned, before, after = (int(value) for value in retire[0].groups()[2:5])
    if returned == 0 or after <= before:
        fail(prefix + "a completed revoke returned no capacity")
    committed, reused, zeroed, peer = (int(value) for value in reuse[0].groups()[1:])
    if committed == 0 or reused == 0 or zeroed != 1 or peer != 1:
        fail(prefix + "recovered capacity was not reused, zeroed and peer-safe")
    first, retained, retry, held_before, held, released = (
        int(value) for value in stuck[0].groups()[1:7]
    )
    if first != 0 or retained != 1 or retry != 1 or held != held_before or released < held:
        fail(prefix + "a failed revoke advertised or lost capacity")
    owned_before, owned_after, _, slots_after, _, _, granted, reclaimed = (
        int(value) for value in final[0].groups()
    )
    if owned_after < owned_before or granted != reclaimed:
        fail(prefix + "the workload's capacity did not return to its baseline")
    if slots_after == 0:
        fail(prefix + "no root CSlots survived the workload")


def entitlement_identity(name: str) -> str:
    """The 16-hex prefix the root prints for the entitlement named `name`.

    The root holds only the hashed identity — the policy's wire form carries no
    names — so it cannot print one. Recomputing the hash here from the *spec's*
    declared name is what keeps the marker a measurement: a root that bound a
    different entitlement prints a prefix this does not produce.
    """
    return hashlib.sha256(ADAPTIVE_ENTITLEMENT_DOMAIN + name.encode()).hexdigest()[:16]


def decoded_manifest(fixture: Path) -> dict:
    """A derived composition manifest, decoded by the pinned Zutai tool."""
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
        fail(f"could not decode {fixture.name}: {process.stdout.strip()}")
    return json.loads(process.stdout)


def declared_policy(fixture: Path) -> dict:
    """The adaptive policy this arm's own composition declares.

    Read from the derived manifest rather than restated, for the same reason
    `declared_quotas` is: the gate asserts that the declaration is what the root
    reserved and enforced, and a second copy of the numbers here would compare
    this file against itself.
    """
    manifest = decoded_manifest(fixture)
    policy = manifest.get("privateMemoryPolicy")
    if policy is None:
        fail(f"{fixture.name} declares no adaptive policy, so the arm asserts nothing")
    if manifest.get("privateMemoryBudget"):
        fail(f"{fixture.name} carries a fixed budget beside its adaptive policy")
    instances = {entry["name"]: entry for entry in manifest["instances"]}
    entitlements = {entry["name"]: entry for entry in policy["entitlements"]}
    subjects = {entry["instance"]: entry for entry in policy["subjects"]}
    unknown = sorted(set(subjects) - set(instances))
    if unknown:
        fail(f"{fixture.name}: the policy names undeclared instance(s): {unknown}")
    if ADAPTIVE_COORDINATOR in subjects:
        fail(f"{fixture.name}: the coordinator is a subject, so it cannot prove denial")
    shared = [
        name
        for name, entry in entitlements.items()
        if sum(1 for subject in subjects.values() if subject["entitlement"] == name) > 1
    ]
    if not shared:
        fail(f"{fixture.name}: no entitlement is shared, so cohort accounting is untested")
    owners = {name for entry in subjects for name in (instances[entry]["owner"],)}
    if len(owners) < 2:
        fail(f"{fixture.name}: every subject has one owner, so the shared cohort is trivial")
    return {
        "entitlements": entitlements,
        "subjects": subjects,
        "instances": instances,
        "reserve": policy["reserve"],
        "shared": shared,
    }


def adaptive_bindings(transcript: str) -> dict[str, tuple[str, ...]]:
    """Every `SLIME_MEM entitlement` line, keyed by instance.

    A tuple per instance rather than one record: the restartable subject binds
    twice, and collapsing its two incarnations would hide exactly the
    multiplication this arm exists to refuse.
    """
    prefix = "SLIME_MEM entitlement"
    rows = adaptive_rows(
        transcript,
        prefix,
        prefix + r" task=(\d+) instance=(\S+) entitlement=(\S+) incarnation=(\d+) "
        r"guarantee=(\d+) maximum=(\d+) mode=(\S+) installed=(\d+) base=(0x[0-9a-f]+)",
        "adaptive plane: ",
    )
    bindings: dict[str, list[tuple[str, ...]]] = {}
    for row in rows:
        bindings.setdefault(row.group(2), []).append(row.groups())
    return {name: tuple(entries) for name, entries in bindings.items()}


def check_adaptive_bindings(transcript: str, policy: dict) -> None:
    """Every declared instance bound exactly what its composition declares.

    Both directions are checked, because only one of them is deny-by-default: a
    subject must receive its declared guarantee and maximum, and an instance the
    policy does not name must receive no window at all.
    """
    prefix = "adaptive plane: "
    bindings = adaptive_bindings(transcript)
    missing = sorted(set(policy["instances"]) - set(bindings))
    if missing:
        fail(prefix + f"no entitlement decision was reported for {missing}")
    reserved = re.search(
        r"^SLIME_MEM policy entitlements=(\d+) subjects=(\d+) guarantee_pages=(\d+) "
        r"reserved_bytes=(\d+) reserved_slots=(\d+) reserved_descriptors=(\d+) "
        r"reserved_extents=(\d+) reserved_tables=(\d+) pool_bytes=(\d+)$",
        transcript,
        re.MULTILINE,
    )
    if reserved is None:
        fail(prefix + "no policy reservation was reported")
    counts = tuple(int(value) for value in reserved.groups())
    if counts[0] != len(policy["entitlements"]) or counts[1] != len(policy["subjects"]):
        fail(prefix + "the reserved policy is not the declared one")
    promised = sum(entry["guaranteePages"] for entry in policy["entitlements"].values())
    if counts[2] != promised:
        fail(prefix + f"reserved {counts[2]} guaranteed page(s), declared {promised}")
    # The sum of elastic maxima is a permission, never a promise. If admission
    # had reserved it, this plane's 1537 MiB subjects alone would exceed either
    # reference machine's RAM.
    permitted = sum(entry["maximumPages"] for entry in policy["subjects"].values())
    if counts[2] >= permitted:
        fail(prefix + "admission reserved the sum of elastic maxima as a guarantee")
    # `reserved_*` describe the guarantee reservations the allocator
    # materialized; the policy's `reserve` record is the operational headroom
    # the ledger subtracts from the pool before them. They are different
    # quantities over different owners, so comparing one against the other
    # says nothing: a guarantee served from coarse aligned parents needs two
    # dozen extents where the operational reserve asks for a hundred of
    # headroom, and neither number bounds the other.
    #
    # What is checkable here is that the reservation covers the promise at its
    # worst placement: every guaranteed page can need its own payload granule,
    # its own leaf table, and a descriptor and CSlot for each. Extents are
    # deliberately only bounded below by one per promising entitlement,
    # because how many extents a guarantee needs is the reservation's
    # granularity rather than its page count.
    #
    # That the operational reserve is subtracted at all is *not* observable
    # from this transcript, which reports the residual pool and not the
    # inventory it was computed from. It is covered by the ledger's own tests
    # (`guarantees_and_reserves_are_simultaneously_affordable_or_admission_fails`),
    # which refuse an admission whose reserve and guarantees together exceed
    # what is available.
    page_bytes = 4096
    if counts[3] < promised * page_bytes:
        fail(prefix + "the reservation is smaller than the payload it promises")
    for index, field in ((4, "slots"), (5, "descriptors"), (7, "tables")):
        if counts[index] < promised:
            fail(prefix + f"the reservation funds fewer {field} than it promises pages")
    promising = sum(
        1 for entry in policy["entitlements"].values() if entry["guaranteePages"] > 0
    )
    if counts[6] < promising:
        fail(prefix + "an entitlement promising pages reserved no backing extent")
    if counts[8] == 0:
        fail(prefix + "reservation consumed the whole ordinary pool")
    # Reserve-first: the reservation is reported before any task is constructed.
    first_task = re.search(
        r"^SLIME_(?:MEM entitlement task=|GRAPH (?:staged|spawned|activated))",
        transcript,
        re.MULTILINE,
    )
    if first_task is None or first_task.start() < reserved.end():
        fail(prefix + "a task was constructed before the guarantees were reserved")

    for name, entries in bindings.items():
        subject = policy["subjects"].get(name)
        for task, _instance, identity, incarnation, guarantee, maximum, mode, installed, base in entries:
            if subject is None:
                if (identity, incarnation, guarantee, maximum, mode, installed, base) != (
                    "none",
                    "0",
                    "0",
                    "0",
                    "none",
                    "0",
                    "0x0",
                ):
                    fail(prefix + f"{name} is not a declared subject but was bound something")
                continue
            entitlement = policy["entitlements"][subject["entitlement"]]
            expected = entitlement_identity(subject["entitlement"])
            if identity != expected:
                fail(prefix + f"{name} bound entitlement {identity}, declared {expected}")
            if mode != subject["maximumMode"]:
                fail(prefix + f"{name} bound maximum mode {mode!r}")
            if int(maximum) != subject["maximumPages"] or int(installed) != subject["maximumPages"]:
                fail(prefix + f"{name}'s installed address maximum is not its declared one")
            if int(guarantee) != entitlement["guaranteePages"]:
                fail(prefix + f"{name} bound a guarantee its entitlement does not declare")
            if base == "0x0" or task == "0":
                fail(prefix + f"{name} is a declared subject but received no window")
    # A cohort's guarantee is one promise, however many subjects share it. Each
    # subject binding its entitlement's full guarantee is how the marker reads;
    # what must not happen is the *reservation* carrying it once per subject.
    for name in policy["shared"]:
        members = [
            instance
            for instance, subject in policy["subjects"].items()
            if subject["entitlement"] == name
        ]
        identity = entitlement_identity(name)
        bound = {
            entry[2]
            for instance in members
            for entry in bindings.get(instance, ())
        }
        if bound != {identity}:
            fail(prefix + f"the shared entitlement {name} was not one identity across {members}")


def adaptive_adjudications(transcript: str) -> list[tuple[str, dict[str, str]]]:
    """The root's grant/refusal lines, in the order it emitted them."""
    pattern = re.compile(
        r"^SLIME_MEM adaptive (grant|refused) task=\d+ instance=(?P<instance>\S+) "
        r"entitlement=(?P<entitlement>\S+) delta=(?P<delta>\d+) "
        r"(?:previous=(?P<previous>\d+) )?pages=(?P<pages>\d+) "
        r"(?:guaranteed=(?P<guaranteed>\d+) elastic=(?P<elastic>\d+) )?"
        r"(?:cause=(?P<cause>\S+) )?"
        r"entitlement_committed=(?P<committed>\d+) pool_bytes=(?P<pool>\d+)",
        re.MULTILINE,
    )
    declared = len(re.findall(r"^SLIME_MEM adaptive (?:grant|refused) ", transcript, re.MULTILINE))
    rows = [(match.group(1), match.groupdict()) for match in pattern.finditer(transcript)]
    if len(rows) != declared:
        fail("adaptive plane: malformed SLIME_MEM adaptive grant/refused line")
    return rows


def adaptive_steps(transcript: str) -> list[tuple[int, str, int, bool, int, str]]:
    """Every schedule step the coordinator reported, in its own order."""
    prefix = "adaptive plane: "
    # Counted directly rather than through `adaptive_rows`: that helper keys its
    # population check on a marker followed by a space, and this marker's first
    # field is attached to it.
    declared = len(re.findall(r"^\[private-adaptive\] step=", transcript, re.MULTILINE))
    rows = list(
        re.finditer(
            r"^\[private-adaptive\] step=(\d+) subject=(\S+) delta=(\d+) served=([01]) "
            r"pages=(\d+) base=(0x[0-9a-f]+)$",
            transcript,
            re.MULTILINE,
        )
    )
    if len(rows) != declared:
        fail(prefix + "malformed schedule step report")
    steps = [
        (
            int(row.group(1)),
            row.group(2),
            int(row.group(3)),
            row.group(4) == "1",
            int(row.group(5)),
            row.group(6),
        )
        for row in rows
    ]
    if [entry[0] for entry in steps] != list(range(ADAPTIVE_STEPS)):
        fail(prefix + "the schedule did not run every step exactly once, in order")
    return steps


def check_adaptive_schedule(transcript: str, policy: dict) -> list[tuple]:
    """Join each request the coordinator issued to the root's own adjudication.

    Neither side is authoritative alone. The coordinator reports what the holder
    measured; the root reports what it decided. A root that answered a different
    request, charged a refused one, or served a subject past its declared
    maximum disagrees with the holder here.
    """
    prefix = "adaptive plane: "
    steps = adaptive_steps(transcript)
    adjudications = adaptive_adjudications(transcript)
    subjects = {entry[1] for entry in steps}
    scheduled = [entry for entry in adjudications if entry[1]["instance"] in subjects]
    if len(scheduled) < len(steps):
        fail(prefix + "the root adjudicated fewer requests than the schedule issued")
    # Receipt order: the coordinator's calls are synchronous, so the root's
    # decisions for the scheduled subjects must appear in the schedule's order.
    for step, (kind, row) in zip(steps, scheduled[: len(steps)], strict=True):
        number, instance, delta, served, pages, base = step
        if row["instance"] != instance or int(row["delta"]) != delta:
            fail(prefix + f"step {number} was adjudicated as a different request")
        if (kind == "grant") != served:
            fail(prefix + f"step {number}: the root and the holder disagree on the outcome")
        if int(row["pages"]) != pages:
            fail(prefix + f"step {number}: the root and the holder disagree on the extent")
        subject = policy["subjects"][instance]
        if pages > subject["maximumPages"]:
            fail(prefix + f"step {number}: {instance} holds more than its declared maximum")
        if served:
            if int(row["guaranteed"] or 0) + int(row["elastic"] or 0) != delta:
                fail(prefix + f"step {number}: the served sources do not sum to the request")
        elif row["cause"] is None:
            fail(prefix + f"step {number}: a refusal named no cause")
        if int(row["pool"]) == 0:
            fail(prefix + f"step {number}: the ordinary pool was reported empty")
    # A refused request must charge nothing: the entitlement's committed total
    # is unchanged against the previous decision for that entitlement.
    committed: dict[str, int] = {}
    for kind, row in scheduled:
        identity = row["entitlement"]
        current = int(row["committed"])
        if kind == "refused" and identity in committed and committed[identity] != current:
            fail(prefix + "a refused request changed its entitlement's committed total")
        committed[identity] = current
    # The two declaration-derived refusals, and the one inventory-derived one,
    # must be distinguishable. The first pair asks one page past a declared
    # maximum, so a root that refused them for lack of memory would be right by
    # accident.
    by_step = {step[0]: (kind, row) for step, (kind, row) in zip(steps, scheduled[: len(steps)], strict=True)}
    for number in ADAPTIVE_DECLARED_REFUSALS:
        kind, row = by_step[number]
        subject = policy["subjects"][row["instance"]]
        if int(row["delta"]) + int(row["pages"]) != subject["maximumPages"] + 1:
            fail(prefix + f"step {number} is not one page past {row['instance']}'s maximum")
        if kind != "refused" or row["cause"] != "maximum":
            fail(prefix + f"step {number} was not refused on its declared maximum")
    kind, row = by_step[ADAPTIVE_POOL_REFUSAL]
    subject = policy["subjects"][row["instance"]]
    if int(row["delta"]) + int(row["pages"]) != subject["maximumPages"]:
        fail(prefix + "the pool-exhaustion step does not reach exactly its declared maximum")
    if kind != "refused" or row["cause"] not in ("pool", "resource", "reservation"):
        fail(prefix + "a request inside its permission was not refused for a resource reason")
    # An idle maximum reserves nothing: the subject that never reached its
    # maximum still let its cohort peer be served after it was refused.
    if not any(served for _, instance, _, served, _, _ in steps[ADAPTIVE_POOL_REFUSAL + 1 :]):
        fail(prefix + "no fitting request was served after the refusals")
    for name in policy["shared"]:
        members = {
            instance
            for instance, subject in policy["subjects"].items()
            if subject["entitlement"] == name
        }
        if len({entry[1] for entry in steps} & members) < 2:
            fail(prefix + f"only one member of the shared entitlement {name} issued requests")
    return steps


def check_adaptive_holder_reports(transcript: str, steps: list[tuple]) -> None:
    """Each holder's own measurement of the request the coordinator issued to it.

    A third independent report, and the one the other two cannot fake: the
    coordinator prints what came back over the endpoint and the root prints what
    it decided, while this line is what the holder read out of its own address
    space. `zeroed=1` on exactly the served requests is the no-stale-bytes half;
    `stable=1` is the claim that a refusal left the earlier stamps intact.
    """
    prefix = "adaptive plane: "
    labels = {
        "private-adaptive-guaranteed": "guaranteed",
        "private-adaptive-pool-a": "pool-a",
        "private-adaptive-pool-b": "pool-b",
    }
    pattern = re.compile(
        r"^\[private-adaptive:([a-z-]+)\] request incarnation=(\d+) delta=(\d+) "
        r"served=([01]) pages=(\d+) base=(0x[0-9a-f]+) zeroed=([01]) stable=([01])$",
        re.MULTILINE,
    )
    declared = len(
        re.findall(r"^\[private-adaptive:[a-z-]+\] request ", transcript, re.MULTILINE)
    )
    rows = list(pattern.finditer(transcript))
    if len(rows) != declared:
        fail(prefix + "malformed holder request report")
    reported: dict[str, list[tuple[int, bool, int]]] = {}
    for row in rows:
        label, _incarnation, delta, served, pages, _base, zeroed, stable = row.groups()
        if stable != "1":
            fail(prefix + f"{label} lost an earlier stamp across a request")
        if (zeroed == "1") != (served == "1"):
            fail(prefix + f"{label} reported zeroing that does not follow its outcome")
        reported.setdefault(label, []).append((int(delta), served == "1", int(pages)))
    for instance, label in labels.items():
        expected = [
            (delta, served, pages)
            for _number, subject, delta, served, pages, _base in steps
            if subject == instance
        ]
        if reported.get(label) != expected:
            fail(prefix + f"{label}'s own measurements differ from the schedule's report")


def check_adaptive_lifecycle(transcript: str, policy: dict) -> None:
    """The restartable subject, the omitted one, and the surviving three."""
    prefix = "adaptive plane: "
    restart = "private-adaptive-restart"
    bindings = adaptive_bindings(transcript)
    incarnations = [entry[3] for entry in bindings.get(restart, ())]
    if incarnations != ["0", "1"]:
        fail(prefix + "the restartable subject did not bind exactly two incarnations")
    identities = {entry[2] for entry in bindings[restart]}
    if identities != {entitlement_identity(policy["subjects"][restart]["entitlement"])}:
        fail(prefix + "a replacement bound a different entitlement than its predecessor")
    bases = {entry[8] for entry in bindings[restart]}
    if len(bases) != 1:
        fail(prefix + "a replacement received a different declared window")
    retired = adaptive_rows(
        transcript,
        "SLIME_MEM adaptive retired",
        r"SLIME_MEM adaptive retired task=\d+ instance=(\S+) entitlement=(\S+) "
        r"returned_pages=(\d+) entitlement_committed=(\d+) quarantined=(\d+)",
        prefix,
    )
    restarts = [row for row in retired if row.group(1) == restart]
    if not restarts:
        fail(prefix + "a faulted incarnation returned nothing to its entitlement")
    if any(row.group(5) != "0" for row in retired):
        fail(prefix + "reclamation quarantined backing, so its capacity is not proven returned")
    guarantee = policy["entitlements"][policy["subjects"][restart]["entitlement"]]["guaranteePages"]
    if int(restarts[0].group(3)) != guarantee or restarts[0].group(4) != "0":
        fail(prefix + "a dead incarnation's charge outlived it")
    # An incarnation is not a second entitlement. Both lives are served the same
    # promise from the same reservation, and the cohort total after the
    # replacement is one guarantee rather than two.
    served = [
        row
        for kind, row in adaptive_adjudications(transcript)
        if kind == "grant" and row["instance"] == restart
    ]
    if len(served) != 2:
        fail(prefix + "the restartable subject was not served once per incarnation")
    for row in served:
        if int(row["pages"]) != guarantee or int(row["guaranteed"] or 0) != guarantee:
            fail(prefix + "an incarnation's guarantee was not served from its reservation")
    if [int(row["committed"]) for row in served] != [guarantee, guarantee]:
        fail(prefix + "a replacement multiplied its entitlement's committed total")
    denied = sorted(set(policy["instances"]) - set(policy["subjects"]))
    for name in denied:
        for entry in bindings.get(name, ()):
            if entry[2] != "none":
                fail(prefix + f"{name} is omitted from the policy but was bound {entry[2]}")
    refusals = [row for kind, row in adaptive_adjudications(transcript) if kind == "refused"]
    if not any(
        row["instance"] in denied and row["cause"] == "entitlement" for row in refusals
    ):
        fail(prefix + "no omitted instance was refused on its absent entitlement")


def check_adaptive_incarnations(transcript: str, policy: dict) -> None:
    """No subject ever holds two live incarnations of its entitlement.

    Asserted over the whole record rather than over one refusal, so it holds
    however the root refuses a duplicate: for each subject, admitted bindings
    and retirements must alternate, starting with a binding. Two admissions
    without a retirement between them would be one subject holding its
    entitlement twice, which is exactly what a cohort guarantee cannot survive.
    """
    prefix = "adaptive plane: "
    events: dict[str, list[tuple[int, str]]] = {}
    for match in re.finditer(
        r"^SLIME_MEM adaptive incarnation instance=(\S+) entitlement=(\S+) "
        r"live=(\d+) admitted=([01]) cause=(\S+)$",
        transcript,
        re.MULTILINE,
    ):
        if match.group(4) == "1":
            events.setdefault(match.group(1), []).append((match.start(), "bind"))
    for match in re.finditer(
        r"^SLIME_MEM adaptive retired task=\d+ instance=(\S+) entitlement=\S+ "
        r"returned_pages=\d+ entitlement_committed=\d+ quarantined=0$",
        transcript,
        re.MULTILINE,
    ):
        events.setdefault(match.group(1), []).append((match.start(), "retire"))
    for name, ordered in events.items():
        live = 0
        for _, kind in sorted(ordered):
            live += 1 if kind == "bind" else -1
            if live > 1:
                fail(prefix + f"{name} held two live incarnations of its entitlement")
            if live < 0:
                fail(prefix + f"{name} retired an incarnation it had not bound")
    for name in policy["subjects"]:
        if name not in events:
            fail(prefix + f"{name} is a declared subject but never bound an incarnation")
    # Every task that reports an entitlement must have had an admitted
    # incarnation to report: a task holding a binding the record never admits
    # would be an entitlement acquired outside the ledger.
    bound = adaptive_bindings(transcript)
    for name in policy["subjects"]:
        admitted = sum(1 for _, kind in events.get(name, ()) if kind == "bind")
        if admitted != len(bound.get(name, ())):
            fail(
                prefix
                + f"{name} reported {len(bound.get(name, ()))} bound task(s) "
                + f"against {admitted} admitted incarnation(s)"
            )


def check_adaptive_io_window(transcript: str) -> None:
    """Device mappings meet the same private window every other path does.

    Checked as its own ordered group rather than as a marker chain, because the
    IO holder is an independent instance: its work is concurrent with the
    coordinator's schedule, and pinning the two together in one global order
    would assert a scheduling accident rather than the claim.

    The claim is that the *destination* is what refuses. The positive controls
    run first with the same capabilities, rights and device epoch, so a
    refusal below cannot be missing authority; the root's own marker names the
    window each refused range fell inside.
    """
    prefix = "adaptive plane: "
    ordered = (
        r"SLIME_MEM entitlement task=(\d+) instance=private-adaptive-io-holder "
        r"entitlement=[0-9a-f]{16} incarnation=0 guarantee=\d+ maximum=(\d+) mode=fixed "
        r"installed=(\d+) base=0x([0-9a-f]+)",
        r"\[private-adaptive:io-supervisor\] holder spawned",
        r"\[private-adaptive:io\] outside_window mmio=1 queue=1",
        r"\[private-adaptive:io\] mmio_backed_refused=1",
        r"\[private-adaptive:io\] mmio_unbacked_refused=1",
        r"\[private-adaptive:io\] queue_backed_refused=1",
        r"\[private-adaptive:io\] queue_unbacked_refused=1",
        r"\[private-adaptive:io\] pages=1",
        r"\[private-adaptive:io\] device mappings excluded from the private window",
    )
    position = 0
    binding = None
    for pattern in ordered:
        found = re.compile(pattern, re.MULTILINE).search(transcript, position)
        if found is None:
            fail(prefix + f"missing or out-of-order device-window marker: {pattern}")
        if binding is None:
            binding = found
        position = found.end()
    task, maximum, installed, base = binding.groups()
    if installed != maximum:
        fail(prefix + "the IO holder's installed window is not its declared maximum")
    window_start = int(base, 16)
    window_end = window_start + int(installed) * 4096
    refusals = [
        match
        for match in re.finditer(
            r"^SLIME_MEM mapping refused task=(\d+) base=0x([0-9a-f]+) end=0x([0-9a-f]+) "
            r"window=0x([0-9a-f]+)\.\.0x([0-9a-f]+)$",
            transcript,
            re.MULTILINE,
        )
        if match.group(1) == task
    ]
    if len(refusals) != 4:
        fail(
            prefix
            + f"the IO holder produced {len(refusals)} window refusals rather than four"
        )
    backed = 0
    for refusal in refusals:
        start, end = int(refusal.group(2), 16), int(refusal.group(3), 16)
        if refusal.group(4) != base or int(refusal.group(5), 16) != window_end:
            fail(prefix + "a refusal named a window other than the holder's own")
        if start < window_start or end > window_end:
            fail(prefix + "a refused range fell outside the window it was refused for")
        backed += int(start == window_start)
    # Two of the four are the holder's own backed page and two are reserved but
    # unbacked: a window defended only where it is backed would leave the rest
    # of the reservation open to a device mapping.
    if backed != 2:
        fail(prefix + "backed and unbacked destinations were not both refused")


def check_adaptive_quarantine(transcript: str) -> None:
    """A failed revoke retains an incarnation's charge until a retry lands.

    The injected failure hits the first adaptive holder that dies, so the two
    retirement records below are the same incarnation's: the first recovers
    nothing and refunds nothing, and exactly one retry returns the pages the
    holder actually held. A refund on the failed attempt would be capacity the
    machine never recovered.
    """
    prefix = "adaptive lifecycle: "
    injected = re.search(
        r"^SLIME_MEM adaptive injected kind=revoke task=(\d+) instance=(\S+)$",
        transcript,
        re.MULTILINE,
    )
    if injected is None:
        fail(prefix + "no revoke failure was injected, so nothing was proven")
    task, instance = injected.groups()
    retirements = [
        match
        for match in re.finditer(
            r"^SLIME_MEM adaptive retired task=(\d+) instance=(\S+) entitlement=(\S+) "
            r"returned_pages=(\d+) entitlement_committed=(\d+) quarantined=([01])$",
            transcript,
            re.MULTILINE,
        )
        if match.group(1) == task
    ]
    if len(retirements) < 2:
        fail(prefix + "the quarantined incarnation was never retried")
    first, second = retirements[0], retirements[1]
    if first.group(6) != "1" or first.group(4) != "0":
        fail(prefix + "a failed revoke returned capacity it had not recovered")
    if second.group(6) != "0" or int(second.group(4)) == 0:
        fail(prefix + "the retry did not return the incarnation's pages")
    if second.group(2) != instance or second.group(3) != first.group(3):
        fail(prefix + "the retry settled a different incarnation than it quarantined")
    if any(row.group(6) == "1" for row in retirements[2:]):
        fail(prefix + "the incarnation was quarantined more than once")
    # The peers are untouched by one holder's failed cleanup: the plane still
    # reaches its own terminal rather than stalling on the quarantine.
    if not re.search(r"^\[private-adaptive\] complete steps=\d+ ", transcript, re.MULTILINE):
        fail(prefix + "the plane did not complete after the quarantine was settled")


def check_adaptive_construction_failure(transcript: str) -> None:
    """A construction that fails after binding releases its incarnation once.

    The injected failure lands after the incarnation is bound and before the
    task is published. The entitlement must be held until the unwind's revoke
    succeeds and then returned, so the retried spawn binds the *next*
    incarnation of the same subject and completes its work, rather than a
    second live incarnation beside a leaked one.
    """
    prefix = "adaptive lifecycle: "
    injected = re.search(
        r"^SLIME_MEM adaptive injected kind=construction task=(\d+) instance=(\S+)$",
        transcript,
        re.MULTILINE,
    )
    if injected is None:
        fail(prefix + "no construction failure was injected, so nothing was proven")
    task, instance = injected.groups()
    after = transcript[injected.end():]
    unwound = re.search(
        rf"^SLIME_MEM adaptive retired task={task} instance={re.escape(instance)} "
        r"entitlement=(\S+) returned_pages=0 entitlement_committed=\d+ quarantined=0$",
        after,
        re.MULTILINE,
    )
    if unwound is None:
        fail(prefix + "the failed construction did not return its bound incarnation")
    bindings = [
        match
        for match in re.finditer(
            rf"^SLIME_MEM entitlement task=(\d+) instance={re.escape(instance)} "
            r"entitlement=(\S+) incarnation=(\d+) ",
            transcript,
            re.MULTILINE,
        )
    ]
    if len(bindings) != 2:
        fail(prefix + "the subject was not bound exactly once per construction attempt")
    failed, retried = bindings
    if failed.group(1) != task or retried.group(1) == task:
        fail(prefix + "the retried construction reused the failed task")
    if failed.group(2) != retried.group(2):
        fail(prefix + "the retry bound a different entitlement")
    if int(retried.group(3)) != int(failed.group(3)) + 1:
        fail(prefix + "the retry did not bind the next incarnation")
    if retried.start() < injected.end() + unwound.end():
        fail(prefix + "the retry bound before the failed incarnation was released")
    if not re.search(
        r"^\[private-adaptive:io-supervisor\] spawn refused, retrying$", after, re.MULTILINE
    ):
        fail(prefix + "the spawner never observed the refused construction")


def check_adaptive_plane(transcript: str, policy: dict) -> list[tuple]:
    check_adaptive_markers(transcript, ADAPTIVE_CHAINS, "adaptive plane: ")
    check_adaptive_bindings(transcript, policy)
    check_adaptive_incarnations(transcript, policy)
    check_adaptive_io_window(transcript)
    steps = check_adaptive_schedule(transcript, policy)
    check_adaptive_holder_reports(transcript, steps)
    check_adaptive_lifecycle(transcript, policy)
    return steps


def check_adaptive_determinism(first: str, second: str, steps: list[tuple]) -> None:
    """The same schedule, on two boots of one image, answered the same way.

    This is the arm's whole determinism claim and it cannot be made by one boot.
    Compared semantically rather than as whole transcripts: task identities and
    timer diagnostics legitimately differ between runs, while a step's subject,
    delta, outcome, extent and window base may not.
    """
    prefix = "adaptive plane: "
    replay = adaptive_steps(second)
    if replay != steps:
        for left, right in zip(steps, replay, strict=False):
            if left != right:
                fail(prefix + f"two boots answered step {left[0]} differently: {left} vs {right}")
        fail(prefix + "two boots ran different schedules")
    digests = [
        re.findall(r"^\[private-adaptive\] schedule steps=\d+ served=\d+ refused=\d+ "
                   r"digest=(0x[0-9a-f]{16})$", text, re.MULTILINE)
        for text in (first, second)
    ]
    if len(digests[0]) != 1 or digests[0] != digests[1]:
        fail(prefix + "the schedule digest is missing or differs between boots")


def check_adaptive_refusal(transcript: str) -> None:
    """The over-guaranteed composition must fail closed, before publication.

    A late failure is not equivalent: the decision record's whole claim is that
    a guarantee is funded before anything observes the graph, so this arm
    refuses a transcript that staged, spawned, activated, or reported any task
    at all.
    """
    prefix = "adaptive refusal: "
    match_marker_contract(
        transcript,
        tuple(
            (label, tuple("(?m)^" + pattern + "$" for pattern in patterns))
            for label, patterns in ADAPTIVE_REFUSAL_CHAINS
        ),
        (),
        lambda message: fail(prefix + message),
    )
    for pattern in ADAPTIVE_REFUSAL_FORBIDDEN:
        found = re.search(pattern, transcript)
        if found is not None:
            fail(prefix + f"a refused admission still published {found.group(0)!r}")
    refusal = re.search(
        r"^SLIME_MEM FAIL adaptive guarantee exceeds inventory required=(\d+) "
        r"available=(\d+) published=0$",
        transcript,
        re.MULTILINE,
    )
    if refusal is None:
        fail(prefix + "no fail-closed refusal was reported")
    required, available = (int(value) for value in refusal.groups())
    if required <= available:
        fail(prefix + "admission refused a guarantee its own inventory could fund")


# The adaptive plane attaches one real transport, because its IO holder binds a
# device and maps its MMIO: a plane with none would prove the window excluded a
# mapping nobody could have made.
ADAPTIVE_DEVICE_ARGUMENTS: tuple[str, ...] = (
    "-drive",
    "if=none,file=/dev/zero,format=raw,id=d0",
    "-device",
    "virtio-blk-device,drive=d0",
)


def run_adaptive_plane_arm(platform: str) -> None:
    """MEM-ADAPTIVE: two boots of the declared-policy plane, then the negative.

    Two boots rather than one, because the acceptance is about a *repeated*
    request schedule producing the same grant/refusal sequence. One boot can
    only show that a schedule ran.
    """
    section, qemu_binary = PLATFORMS[platform]
    profile = load_qemu_profile(fail, PINS, section)
    suffix = "-rv64" if platform == "qemu-riscv-virt" else ""
    variant = f"sel4-private-memory-adaptive{suffix}"
    policy = declared_policy(ADAPTIVE_FIXTURES[platform])
    try:
        built = build_closure_image(variant)
    except ClosureImageError as error:
        fail(str(error))
    if (
        built.build_result.get("platform") != platform
        or built.build_result.get("targetProfile") != TARGET_PROFILES[platform]
    ):
        fail("adaptive image: closure target differs from requested platform")
    digest = sha256_file(built.image, fail)
    if digest != built.digest():
        fail("adaptive image: packaged digest differs from build result")
    transcripts = []
    for boot_number in (0, 1):
        transcript = boot(
            profile,
            section=section,
            qemu_binary=qemu_binary,
            image=built.image,
            additional_arguments=ADAPTIVE_DEVICE_ARGUMENTS,
        )
        if sha256_file(built.image, fail) != digest:
            fail("adaptive image: packaged bytes changed during execution")
        (ROOT / "build" / f"{variant}-boot{boot_number}.log").write_text(
            transcript + "\n", encoding="utf-8"
        )
        transcripts.append(transcript)
    steps = check_adaptive_plane(transcripts[0], policy)
    check_adaptive_plane(transcripts[1], policy)
    check_adaptive_determinism(transcripts[0], transcripts[1], steps)

    # The negative composition builds only for the reference architecture its
    # spec declares; the RV64 arm inherits the same refusal through the same
    # admission path and does not restate it.
    refused_cases = 0
    if platform == CLOSURE_PLATFORM:
        declared_policy(ADAPTIVE_OVERCOMMIT_FIXTURE)
        try:
            negative = build_closure_image("sel4-private-memory-adaptive-overcommit")
        except ClosureImageError as error:
            fail(str(error))
        refusal = boot(
            profile,
            section=section,
            qemu_binary=qemu_binary,
            image=negative.image,
            additional_arguments=ADAPTIVE_DEVICE_ARGUMENTS,
        )
        (ROOT / "build" / "sel4-private-memory-adaptive-overcommit.log").write_text(
            refusal + "\n", encoding="utf-8"
        )
        check_adaptive_refusal(refusal)
        refused_cases = 1
    served = sum(1 for entry in steps if entry[3])
    print(
        f"private-memory adaptive workload finished: {variant} image={digest}; "
        f"{ADAPTIVE_STEPS} scheduled requests over {len(policy['subjects'])} declared "
        f"subject(s) and {len(policy['entitlements'])} entitlement(s) produced the same "
        f"{served} grant(s) and {ADAPTIVE_STEPS - served} refusal(s) on two boots, and "
        f"{refused_cases} over-guaranteed composition(s) refused admission before publication"
    )


def run_adaptive_lifecycle_arm(platform: str) -> None:
    """The adaptive plane under one injected revoke failure.

    Its own closure rather than a case of the ordinary arm: the injection is
    compiled into the root, so the image that carries it is a different build
    key and must not be mistaken for the plane's ordinary evidence.
    """
    section, qemu_binary = PLATFORMS[platform]
    profile = load_qemu_profile(fail, PINS, section)
    variant = "sel4-private-memory-adaptive-lifecycle"
    policy = declared_policy(ADAPTIVE_FIXTURES[platform])
    try:
        built = build_closure_image(variant)
    except ClosureImageError as error:
        fail(str(error))
    digest = sha256_file(built.image, fail)
    if digest != built.digest():
        fail("adaptive lifecycle image: packaged digest differs from build result")
    transcript = boot(
        profile,
        section=section,
        qemu_binary=qemu_binary,
        image=built.image,
        additional_arguments=ADAPTIVE_DEVICE_ARGUMENTS,
    )
    (ROOT / "build" / f"{variant}.log").write_text(transcript + "\n", encoding="utf-8")
    check_adaptive_construction_failure(transcript)
    check_adaptive_quarantine(transcript)
    check_adaptive_incarnations(transcript, policy)
    print(
        f"private-memory adaptive lifecycle finished: {variant} image={digest}; "
        "one construction failure after binding released its incarnation to a retried "
        "spawn, and one injected revoke failure quarantined a live incarnation, "
        "refunded nothing, and exactly one retry returned its pages"
    )


def run_adaptive_arm(platform: str, arm: str) -> None:
    section, qemu_binary = PLATFORMS[platform]
    profile = load_qemu_profile(fail, PINS, section)
    suffix = "-rv64" if platform == "qemu-riscv-virt" else ""
    variant = f"sel4-private-memory-{arm}{suffix}"
    try:
        built = build_closure_image(variant)
    except ClosureImageError as error:
        fail(str(error))
    if built.build_result.get("platform") != platform or built.build_result.get("targetProfile") != TARGET_PROFILES[platform]:
        fail(f"{arm} image: closure target differs from requested platform")
    digest = sha256_file(built.image, fail)
    if digest != built.digest():
        fail(f"{arm} image: packaged digest differs from build result")
    transcript = boot(profile, section=section, qemu_binary=qemu_binary, image=built.image)
    if sha256_file(built.image, fail) != digest:
        fail(f"{arm} image: packaged bytes changed during execution")
    (ROOT / "build" / f"{variant}.log").write_text(transcript + "\n", encoding="utf-8")
    {
        "cspace": check_cspace_execution,
        "metadata": check_metadata_lifecycle,
        "bootstrap": check_bootstrap_boundaries,
        "elastic": check_idle_and_guarantee,
        "fragmentation": check_mixed_fragmentation,
        "rollback": check_failure_rollback,
        "conservation": check_cross_holder_conservation,
    }[arm](transcript)
    print(f"private-memory {arm} workload finished: {variant} image={digest}")


def run_stress_arm(platform: str) -> None:
    section, qemu_binary = PLATFORMS[platform]
    profile = load_qemu_profile(fail, PINS, section)
    suffix = "-rv64" if platform == "qemu-riscv-virt" else ""
    for variant, validator in (
        (f"sel4-private-memory-stress{suffix}-injected", check_stress_workload),
        (f"sel4-private-memory-heap-stress{suffix}", check_heap_stress_workload),
    ):
        try:
            built = build_closure_image(variant)
        except ClosureImageError as error:
            fail(str(error))
        if built.build_result.get("platform") != platform or built.build_result.get("targetProfile") != TARGET_PROFILES[platform]:
            fail("stress image: closure target differs from requested platform")
        digest = sha256_file(built.image, fail)
        if digest != built.digest():
            fail("stress image: packaged digest differs from build result")
        transcript = boot(profile, section=section, qemu_binary=qemu_binary, image=built.image)
        if sha256_file(built.image, fail) != digest:
            fail("stress image: packaged bytes changed during execution")
        (ROOT / "build" / f"{variant}.log").write_text(transcript + "\n", encoding="utf-8")
        validator(transcript)
        check_capacity_conservation(transcript)
        check_backing_ledger(transcript)
        print(f"private-memory stress workload finished: {variant} image={digest}")


def run_capacity_arm(platform: str, *, isolation: bool = False) -> None:
    variant = "private-memory-isolation" if isolation else "private-memory-1g"
    section, qemu_binary = PLATFORMS[platform]
    profile = load_qemu_profile(fail, PINS, section)
    if platform == CLOSURE_PLATFORM:
        try:
            built = build_closure_image("sel4-" + variant)
        except ClosureImageError as error:
            fail(str(error))
        image = built.image
        if built.build_result.get("platform") != platform or built.build_result.get("targetProfile") != TARGET_PROFILES[platform]:
            fail("capacity image: closure target differs from requested platform")
        digest = sha256_file(image, fail)
        if digest != built.digest():
            fail("capacity image: packaged digest differs from closure build result")
        identity = built.identity
    else:
        command = [sys.executable, str(BUILD_SCRIPT), f"--{variant}-plane", "--platform", platform]
        subprocess.run(command, cwd=ROOT, check=True)
        image = ROOT / "build" / f"slime-sel4-{variant}-{platform}.elf"
        manifest = image.with_suffix(".identity.json")
        verify_image_identity(image=image, manifest=manifest, variant=variant, fail=fail)
        record = json.loads(manifest.read_text(encoding="utf-8"))
        check_capacity_image_profile(record, platform, profile)
        digest = sha256_file(image, fail)
        identity = sha256_file(manifest, fail)
    print(f"[capacity identity] platform={platform} target={TARGET_PROFILES[platform]} image={digest} build={identity}", flush=True)
    transcript = boot(profile, section=section, qemu_binary=qemu_binary, image=image)
    if sha256_file(image, fail) != digest:
        fail("capacity image: packaged bytes changed during execution")
    (ROOT / "build" / f"{variant}-{platform}.log").write_text(transcript + "\n", encoding="utf-8")
    if isolation:
        check_private_isolation(transcript)
    else:
        check_capacity_workload(transcript)
    check_capacity_conservation(transcript)
    check_backing_ledger(transcript)
    print(f"private-memory capacity workload finished on {platform}")


# MEM-ADAPTIVE's inventory matrix. One closure per architecture supplies the
# root and every component; each pinned inventory row repackages that exact
# root with its own kernel, device tree and loader.
MATRIX_FIXTURES = {
    "qemu-arm-virt": GENERATION_COMPOSITIONS / "sel4-private-memory-matrix.zti",
    "qemu-riscv-virt": GENERATION_COMPOSITIONS / "sel4-private-memory-matrix-rv64.zti",
}
# Emulated time for the largest row's all-small walk dominates; the bound is a
# hang detector, not a performance claim.
MATRIX_TIMEOUT = 3600
MATRIX_GUARANTEE = 1024
MATRIX_BULK_UNIT = 64 * 512
MATRIX_SMALL_UNIT = 511
MATRIX_SCHEDULES = (
    ("bulk", (("private-matrix-bulk", 0, MATRIX_BULK_UNIT),)),
    ("small", (("private-matrix-small", 1, MATRIX_SMALL_UNIT),)),
    (
        "mixed",
        (
            ("private-matrix-bulk", 1, MATRIX_BULK_UNIT),
            ("private-matrix-small", 2, MATRIX_SMALL_UNIT),
        ),
    ),
)
MATRIX_CYCLES = 20
MATRIX_CYCLE_PAGES = 4095
# Where each schedule's requests begin and end in the transcript. A schedule's
# adjudications are only attributable to it between these two markers.
MATRIX_SEGMENTS = {
    "bulk": (
        r"^\[private-matrix:bulk\] verified incarnation=0 base=0x[0-9a-f]+ pages=0 ",
        r"^\[private-matrix\] exhausted schedule=bulk ",
    ),
    "small": (
        r"^\[private-matrix:bulk\] end incarnation=0 ",
        r"^\[private-matrix\] exhausted schedule=small ",
    ),
    "mixed": (
        r"^\[private-matrix:small\] end incarnation=1 ",
        r"^\[private-matrix\] exhausted schedule=mixed ",
    ),
}
MATRIX_CHAINS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "matrix bulk schedule, idle maximum and guarantee under pressure",
        (
            r"SLIME_MEM policy entitlements=2 subjects=3 guarantee_pages=1024 .*",
            r"\[private-matrix\] guarantee pressure redeemed=1 beyond_promise_served=0 pages=1024",
            r"\[private-matrix\] idle maximum subject=private-matrix-small pages=0 refused=1",
            r"\[private-matrix:small\] end incarnation=0 pages=0 refused=1 kind=exit",
            r"\[private-matrix\] reserve spawn subject=private-matrix-small incarnation=1 pages=0 "
            r"refused=1",
            r"\[private-matrix\] exhausted schedule=bulk subject=private-matrix-bulk incarnation=0 "
            r"pages=\d+ served=\d+ refused=\d+ final_delta=1",
            r"\[private-matrix\] resident schedule=bulk pages=\d+ bytes=\d+ guaranteed=1024 "
            r"bulk=\d+ small=0",
            r"\[private-matrix:bulk\] end incarnation=0 pages=\d+ refused=\d+ kind=exit",
        ),
    ),
    (
        "matrix all-small schedule",
        (
            r"\[private-matrix\] exhausted schedule=small subject=private-matrix-small incarnation=1 "
            r"pages=\d+ served=\d+ refused=\d+ final_delta=1",
            r"\[private-matrix\] resident schedule=small pages=\d+ bytes=\d+ guaranteed=1024 "
            r"bulk=0 small=\d+",
            r"\[private-matrix:small\] end incarnation=1 pages=\d+ refused=\d+ kind=exit",
        ),
    ),
    (
        "matrix mixed simultaneous schedule and reuse cycles",
        (
            r"\[private-matrix\] exhausted schedule=mixed subject=private-matrix-bulk incarnation=1 "
            r"pages=\d+ served=\d+ refused=\d+ final_delta=1",
            r"\[private-matrix\] exhausted schedule=mixed subject=private-matrix-small incarnation=2 "
            r"pages=\d+ served=\d+ refused=\d+ final_delta=1",
            r"\[private-matrix\] resident schedule=mixed pages=\d+ bytes=\d+ guaranteed=1024 "
            r"bulk=\d+ small=\d+",
            r"\[private-matrix\] cycles begin count=20 pages=4095",
            r"\[private-matrix\] cycle=19 victim=private-matrix-small incarnation=\d+ end=exit "
            r"reuser=private-matrix-bulk incarnation=\d+ pages=4095 zeroed=1 peer_pages=1024",
            r"\[private-matrix:guaranteed\] end incarnation=0 pages=1024 refused=1 kind=exit",
            r"\[private-matrix\] complete schedules=3 cycles=20",
            HEALTHY_MARKER,
        ),
    ),
)
MATRIX_FAILURES: tuple[str, ...] = (
    r"SLIME_ROOT FATAL",
    r"SLIME_MEM FAIL",
    r"\[private-matrix\] FAIL",
    r"\[private-matrix:[a-z]+\] FAIL",
    r"SLIME_GRAPH task reclaim incomplete task=\d+",
)
# Resources whose shortfall is a count running out. A refusal naming one must
# report more required than available, or it refused a request that fit.
MATRIX_COUNTED = ("ordinary-bytes", "extent-records", "transaction-extents")


def matrix_policy(fixture: Path) -> dict:
    """The matrix composition's policy, which must declare no capacity.

    The arm's claim is that one declaration reaches different capacities on
    different inventories. That holds only if the capacity-bearing subjects
    are pool-relative and the guaranteed one is the only fixed number, so the
    declaration is checked for exactly that shape rather than trusted.
    """
    manifest = decoded_manifest(fixture)
    policy = manifest.get("privateMemoryPolicy")
    if policy is None or manifest.get("privateMemoryBudget"):
        fail(f"{fixture.name}: the matrix needs an adaptive policy and no fixed budget")
    entitlements = {entry["name"]: entry for entry in policy["entitlements"]}
    subjects = {entry["instance"]: entry for entry in policy["subjects"]}
    for instance in ("private-matrix-bulk", "private-matrix-small"):
        subject = subjects.get(instance)
        if (
            subject is None
            or subject["maximumMode"] != "pool"
            or entitlements[subject["entitlement"]]["maximumMode"] != "pool"
            or entitlements[subject["entitlement"]]["guaranteePages"] != 0
        ):
            fail(f"{fixture.name}: {instance} is not an unguaranteed pool-relative subject")
    if subjects["private-matrix-bulk"]["entitlement"] != subjects["private-matrix-small"]["entitlement"]:
        fail(f"{fixture.name}: the bulk and small subjects must contend for one entitlement")
    guaranteed = subjects.get("private-matrix-guaranteed")
    if (
        guaranteed is None
        or entitlements[guaranteed["entitlement"]]["guaranteePages"] != MATRIX_GUARANTEE
        or guaranteed["maximumPages"] <= MATRIX_GUARANTEE
    ):
        fail(f"{fixture.name}: the guaranteed subject does not declare its promise below its maximum")
    return policy


def matrix_segment(transcript: str, schedule: str, prefix: str) -> str:
    start, end = MATRIX_SEGMENTS[schedule]
    opening = re.search(start, transcript, re.MULTILINE)
    closing = re.search(end, transcript, re.MULTILINE)
    if opening is None or closing is None or closing.start() < opening.end():
        fail(prefix + f"the {schedule} schedule has no bounded transcript segment")
    return transcript[opening.end() : closing.start()]


def matrix_walk(segment: str, instance: str, unit: int, prefix: str) -> dict[str, int | str]:
    """One holder's exhaustion walk, reconstructed from the root's own lines.

    The walk's shape is the claim: a refused request is followed by one of
    half the size, a served one by the same size, and the walk ends at a
    refused single page whose limit line names the resource that ran out.
    """
    lines = [
        match
        for match in re.finditer(
            r"^SLIME_MEM adaptive (grant|refused|limit) task=\d+ instance=(\S+) (.*)$",
            segment,
            re.MULTILINE,
        )
        if match.group(2) == instance
    ]
    delta = unit
    pages = 0
    served = refused = 0
    limit: dict[str, int | str] | None = None
    pending_limit = False
    for match in lines:
        kind, fields = match.group(1), match.group(3)
        values = dict(re.findall(r"(\w+)=(\S+)", fields))
        if kind == "limit":
            if not pending_limit or int(values["delta"]) != delta:
                fail(prefix + f"{instance}: a limit line follows no refusal of its delta")
            pending_limit = False
            limit = {
                key: (value if key == "resource" else int(value))
                for key, value in values.items()
            }
            if delta != 1:
                delta //= 2
            continue
        if pending_limit:
            fail(prefix + f"{instance}: a refusal carries no limit line")
        if limit is not None and limit["delta"] == 1:
            fail(prefix + f"{instance}: a request followed the one-page refusal")
        if int(values["delta"]) != delta:
            fail(
                prefix + f"{instance}: requested {values['delta']} pages where the walk "
                f"required {delta}"
            )
        if kind == "grant":
            if int(values["previous"]) != pages or int(values["pages"]) != pages + delta:
                fail(prefix + f"{instance}: a grant disagrees with the extent it grew")
            pages += delta
            served += 1
        else:
            if int(values["pages"]) != pages:
                fail(prefix + f"{instance}: a refusal changed the extent")
            refused += 1
            pending_limit = True
    if limit is None or limit["delta"] != 1 or pending_limit:
        fail(prefix + f"{instance}: the walk did not end at a refused single page")
    return {"pages": pages, "served": served, "refused": refused, **limit}


def check_matrix_limit(walk: dict, instance: str, reserve: int, prefix: str) -> None:
    """The one-page refusal must be a real shortfall, not an invented one.

    Whatever named it, a refused single page is only exhaustion if nothing
    beyond the operational reserve is left: the allocator's residual may hold
    the reserve (less what constructions since admission drew from it) and at
    most one page and its table of ledger dust. More than that is ordinary
    memory a fitting request could not reach.
    """
    resource = walk["resource"]
    if walk["inventory_bytes"] > reserve + 2 * 4096:
        fail(
            prefix + f"{instance}: refused one page for {resource} while the allocator held "
            f"{walk['inventory_bytes']} bytes against a {reserve}-byte reserve"
        )
    if resource in ("maximum", "reservation", "entitlement"):
        fail(prefix + f"{instance}: exhaustion was a declaration ({resource}), not the inventory")
    if resource in MATRIX_COUNTED and walk["required"] <= walk["available"]:
        fail(
            prefix + f"{instance}: refused one page for {resource} with {walk['available']} "
            f"available and {walk['required']} required"
        )
    if resource == "ledger-pool" and walk["ledger_pool"] >= 2 * 4096:
        fail(prefix + f"{instance}: the ledger refused a page while holding {walk['ledger_pool']} bytes")


# Census fields that name free or reusable capacity, compared across cycles.
# `retired` counts reclamations and legitimately rises every cycle.
MATRIX_CENSUS_EXEMPT = frozenset({"retired"})


def matrix_census(line: str) -> dict[str, int]:
    return {
        key: int(value)
        for key, value in re.findall(r"(\w+)=(\d+)", line)
        if key not in MATRIX_CENSUS_EXEMPT
    }


def check_matrix_cycles(transcript: str, prefix: str) -> dict[str, int]:
    """Twenty deaths, each followed by another holder's zeroed reuse, without drift.

    The holder lines prove the bytes: a reuser that read a non-zero served word
    or a peer whose pattern moved emits a failure marker instead of its cycle
    line. This proves the accounting: the root's census after each cycle's
    last reclamation must equal the census before the first cycle, field for
    field, so no slot, descriptor, extent, anchor or byte leaks per cycle.
    """
    begin = re.search(r"^\[private-matrix\] cycles begin ", transcript, re.MULTILINE)
    assert begin is not None
    censuses = [
        (match.start(), matrix_census(match.group(0)))
        for match in re.finditer(r"^SLIME_MEM census .*$", transcript, re.MULTILINE)
    ]
    before = [census for position, census in censuses if position < begin.start()]
    if not before:
        fail(prefix + "no census precedes the reuse cycles")
    baseline = before[-1]
    cycles = list(
        re.finditer(
            r"^\[private-matrix\] cycle=(\d+) victim=(\S+) incarnation=(\d+) end=(fault|exit) "
            r"reuser=(\S+) incarnation=(\d+) pages=(\d+) zeroed=1 peer_pages=(\d+)$",
            transcript,
            re.MULTILINE,
        )
    )
    if [int(match.group(1)) for match in cycles] != list(range(MATRIX_CYCLES)):
        fail(prefix + "the reuse cycles did not run exactly once each, in order")
    faults = 0
    for match in cycles:
        cycle = int(match.group(1))
        victim, end, reuser = match.group(2), match.group(4), match.group(5)
        expected = ("private-matrix-bulk", "private-matrix-small")
        if (victim, reuser) != (expected if cycle % 2 == 0 else expected[::-1]):
            fail(prefix + f"cycle {cycle}: capacity was not reused by a different holder")
        if end != ("fault" if cycle % 4 < 2 else "exit"):
            fail(prefix + f"cycle {cycle}: the victim did not end by its scheduled path")
        faults += end == "fault"
        if int(match.group(7)) != MATRIX_CYCLE_PAGES or int(match.group(8)) != MATRIX_GUARANTEE:
            fail(prefix + f"cycle {cycle}: a holder or the peer reported the wrong extent")
        after = [census for position, census in censuses if position < match.start()]
        drift = {
            key: (baseline.get(key), after[-1].get(key))
            for key in sorted(set(baseline) | set(after[-1]))
            if baseline.get(key) != after[-1].get(key)
        }
        if drift:
            fail(prefix + f"cycle {cycle}: the census drifted from its pre-cycle baseline: {drift}")
    return {"cycles": len(cycles), "faults": faults}


def check_matrix_inventory(transcript: str, kernel_memory: list[dict], prefix: str) -> None:
    """Every ordinary range root admitted lies inside the kernel's compiled memory.

    The kernel's memory ranges exclude every device region of the device tree,
    so a range outside them would be device memory counted as ordinary.
    """
    ranges = [
        (int(start, 16), int(size))
        for start, size in re.findall(
            r"^SLIME_ROOT ordinary range=\d+ paddr=(0x[0-9a-f]+) bytes=(\d+)$",
            transcript,
            re.MULTILINE,
        )
    ]
    memory = [(int(entry["start"], 16), int(entry["end"], 16)) for entry in kernel_memory]
    if not ranges:
        fail(prefix + "the root reported no ordinary ranges")
    for start, size in ranges:
        if not any(low <= start and start + size <= high for low, high in memory):
            fail(prefix + f"ordinary range {start:#x}+{size} lies outside the kernel's memory")


def check_matrix_transcript(transcript: str, reserve: int, prefix: str) -> dict[str, object]:
    """Validate one inventory row's boot and return what it measured."""
    check_adaptive_markers(transcript, MATRIX_CHAINS, prefix)
    for pattern in MATRIX_FAILURES:
        if re.search(pattern, transcript) is not None:
            fail(prefix + f"failure marker {pattern!r}")
    ordinary = re.search(r"^SLIME_ROOT ordinary ranges=\d+ bytes=(\d+) ", transcript, re.MULTILINE)
    policy = re.search(r"^SLIME_MEM policy .* pool_bytes=(\d+)$", transcript, re.MULTILINE)
    if ordinary is None or policy is None:
        fail(prefix + "the root reported no ordinary inventory or no admitted pool")
    result: dict[str, object] = {
        "ordinary": int(ordinary.group(1)),
        "pool": int(policy.group(1)),
        "schedules": {},
    }
    for schedule, walkers in MATRIX_SCHEDULES:
        segment = matrix_segment(transcript, schedule, prefix)
        walks = {}
        for instance, incarnation, unit in walkers:
            walk = matrix_walk(segment, instance, unit, prefix + f"{schedule}: ")
            check_matrix_limit(walk, instance, reserve, prefix + f"{schedule}: ")
            reported = re.search(
                rf"^\[private-matrix\] exhausted schedule={schedule} subject={instance} "
                rf"incarnation={incarnation} pages=(\d+) served=(\d+) refused=(\d+) ",
                transcript,
                re.MULTILINE,
            )
            if reported is None or tuple(int(value) for value in reported.groups()) != (
                walk["pages"],
                walk["served"],
                walk["refused"],
            ):
                fail(prefix + f"{schedule}: {instance}'s own report disagrees with the root's")
            walks[instance] = walk
        resident = re.search(
            rf"^\[private-matrix\] resident schedule={schedule} pages=(\d+) bytes=(\d+) "
            rf"guaranteed=(\d+) bulk=(\d+) small=(\d+)$",
            transcript,
            re.MULTILINE,
        )
        assert resident is not None
        pages, byte_count, guaranteed, bulk, small = (int(value) for value in resident.groups())
        if pages != guaranteed + bulk + small or byte_count != pages * 4096:
            fail(prefix + f"{schedule}: the resident total is not its holders' sum")
        held = {
            "private-matrix-bulk": bulk,
            "private-matrix-small": small,
        }
        for instance, walk in walks.items():
            if held[instance] != walk["pages"]:
                fail(prefix + f"{schedule}: {instance} verified an extent it was not granted")
        result["schedules"][schedule] = {"resident": pages, "walks": walks}
    result["cycles"] = check_matrix_cycles(transcript, prefix)
    # Returned extents adopted for infrastructure must stay in one census
    # category, or conservation cannot close once every holder retired.
    check_capacity_conservation(transcript, prefix)
    return result


def load_sel4_builder():
    import importlib.util

    spec = importlib.util.spec_from_file_location("matrix_sel4_builder", BUILD_SCRIPT)
    if spec is None or spec.loader is None:
        fail(f"cannot load {BUILD_SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules["matrix_sel4_builder"] = module
    spec.loader.exec_module(module)
    return module


def run_matrix_arm(platform: str, only: int | None = None) -> None:
    """MEM-ADAPTIVE's inventory matrix on one architecture.

    The root and components are built once, through the closure, and every row
    boots those exact bytes. What varies is only what seL4 compiles the memory
    map into, so a capacity difference between rows is the inventory's doing.
    A launcher-only control then boots the pinned row's image with the largest
    row's RAM, and must observe the pinned row's inventory unchanged.
    """
    section, qemu_binary = PLATFORMS[platform]
    profile = load_qemu_profile(fail, PINS, section)
    suffix = "-rv64" if platform == "qemu-riscv-virt" else ""
    variant = f"sel4-private-memory-matrix{suffix}"
    reserve = matrix_policy(MATRIX_FIXTURES[platform])["reserve"]["bytes"]
    try:
        built = build_closure_image(variant)
    except ClosureImageError as error:
        fail(str(error))
    if (
        built.build_result.get("platform") != platform
        or built.build_result.get("targetProfile") != TARGET_PROFILES[platform]
    ):
        fail("matrix image: closure target differs from requested platform")
    root_digest = sha256_file(built.root, fail)
    builder = load_sel4_builder()
    target = builder.PLATFORMS[platform]
    pins = builder.load_pins()
    product = builder.platform_memory_mib(target)
    rows = builder.inventory_rows(target)
    identities: dict[int, dict] = {}
    measured: dict[int, dict] = {}
    for row in rows:
        if only is not None and row != only:
            continue
        output = ROOT / "build" / "inventory" / variant / f"mem{row}"
        if output.exists():
            shutil.rmtree(output)
        identity = builder.package_inventory_image(
            pins, target, row, root_elf=built.root, output=output
        )
        if identity["root"]["sha256"] != root_digest:
            fail(f"matrix {row} MiB: packaged a root other than the closure's")
        if row == product and identity["image"]["sha256"] != built.digest():
            fail(
                f"matrix {row} MiB: the pinned row's repackaged image differs from the "
                "closure's, so the row is not the image the closure built"
            )
        image = output / "image.elf"
        print(
            f"[matrix identity] platform={platform} memory_mib={row} "
            f"image={identity['image']['sha256']} kernel={identity['kernel']['sha256']} "
            f"dtb={identity['dtb']['sha256']} loader={identity['loader']['sha256']} "
            f"root={root_digest} abi={identity['abi']}",
            flush=True,
        )
        transcript = boot(
            profile,
            section=section,
            qemu_binary=qemu_binary,
            image=image,
            memory_mib=row,
            timeout=MATRIX_TIMEOUT,
        )
        if sha256_file(image, fail) != identity["image"]["sha256"]:
            fail(f"matrix {row} MiB: packaged bytes changed during execution")
        log = ROOT / "build" / f"{variant}-mem{row}.log"
        log.write_text(transcript + "\n", encoding="utf-8")
        print(f"[matrix transcript] memory_mib={row} sha256={sha256_file(log, fail)}", flush=True)
        measured[row] = check_matrix_transcript(transcript, reserve, f"matrix {row} MiB: ")
        check_matrix_inventory(transcript, identity["kernelMemory"], f"matrix {row} MiB: ")
        identities[row] = identity
        schedules = measured[row]["schedules"]
        print(
            f"[matrix row] platform={platform} memory_mib={row} "
            f"ordinary={measured[row]['ordinary']} pool={measured[row]['pool']} "
            + " ".join(
                f"{name}_resident={value['resident']}" for name, value in schedules.items()
            )
            + " "
            + " ".join(
                f"{name}:{instance}:limit={walk['resource']}"
                for name, value in schedules.items()
                for instance, walk in value["walks"].items()
            ),
            flush=True,
        )
    if only is not None:
        fail(f"matrix: only the {only} MiB row ran; a partial matrix qualifies nothing")
    check_matrix_rows(identities, measured, rows, product)

    # Launcher-only control: more emulated RAM, the pinned row's kernel.
    pinned = ROOT / "build" / "inventory" / variant / f"mem{product}" / "image.elf"
    control = boot(
        profile,
        section=section,
        qemu_binary=qemu_binary,
        image=pinned,
        memory_mib=rows[-1],
        timeout=MATRIX_TIMEOUT,
    )
    (ROOT / "build" / f"{variant}-launcher-only.log").write_text(control + "\n", encoding="utf-8")
    observed = check_matrix_transcript(control, reserve, "matrix launcher-only: ")
    check_matrix_inventory(control, identities[product]["kernelMemory"], "matrix launcher-only: ")
    check_matrix_launcher_only(observed, measured[product])
    largest = measured[rows[-1]]["schedules"]["bulk"]["resident"] * 4096
    print(
        f"private-memory matrix finished on {platform}: {len(rows)} inventories "
        f"({', '.join(f'{row} MiB' for row in rows)}) booted one root {root_digest[:16]} "
        "and one component set; verified bulk residency "
        + " < ".join(
            str(measured[row]["schedules"]["bulk"]["resident"]) for row in rows
        )
        + f" pages, {largest} bytes on the largest row; a launcher-only "
        f"{rows[-1]} MiB boot of the {product} MiB kernel kept its inventory"
    )


def check_matrix_rows(
    identities: dict[int, dict], measured: dict[int, dict], rows: tuple[int, ...], product: int
) -> None:
    prefix = "matrix: "
    if len(rows) < 3 or rows[0] >= product or rows[-1] <= product:
        fail(prefix + "the rows do not bracket the pinned inventory")
    for key in ("kernel", "dtb", "loader", "image"):
        digests = [identities[row][key]["sha256"] for row in rows]
        if len(set(digests)) != len(rows):
            fail(prefix + f"two rows share a {key}, so they are not distinct inventories")
    for key in ("abi",):
        if len({identities[row][key] for row in rows}) != 1:
            fail(prefix + "rows disagree on the userspace ABI")
    if len({identities[row]["root"]["sha256"] for row in rows}) != 1:
        fail(prefix + "rows booted different roots")
    for field in ("ordinary", "pool"):
        values = [measured[row][field] for row in rows]
        if values != sorted(set(values)):
            fail(prefix + f"{field} does not strictly increase with the inventory: {values}")
    for schedule, _ in MATRIX_SCHEDULES:
        values = [measured[row]["schedules"][schedule]["resident"] for row in rows]
        if values != sorted(set(values)):
            fail(prefix + f"{schedule} residency does not grow with the inventory: {values}")
    if max(measured[rows[-1]]["schedules"][name]["resident"] for name, _ in MATRIX_SCHEDULES) * 4096 <= 1 << 30:
        fail(prefix + "the largest row verified no more than 1 GiB resident")


def check_matrix_launcher_only(observed: dict, pinned: dict) -> None:
    """More launcher RAM under the same kernel must change nothing root sees."""
    if observed["ordinary"] != pinned["ordinary"] or observed["pool"] != pinned["pool"]:
        fail(
            "matrix launcher-only: a larger emulator changed the root's inventory "
            f"({observed['ordinary']}/{observed['pool']} against "
            f"{pinned['ordinary']}/{pinned['pool']})"
        )
    for schedule, _ in MATRIX_SCHEDULES:
        if observed["schedules"][schedule]["resident"] != pinned["schedules"][schedule]["resident"]:
            fail(f"matrix launcher-only: {schedule} residency moved without a kernel change")


def main() -> None:
    parser = argparse.ArgumentParser(description="Check mixed-size private memory on seL4")
    parser.add_argument(
        "--arm",
        choices=(
            "ceiling", "cycles", "capacity", "isolation", "stress", "cspace", "metadata",
            "bootstrap", "elastic", "fragmentation", "rollback", "conservation", "adaptive",
            "adaptive-lifecycle", "matrix",
        ),
        default="ceiling",
        help="which qualification to run: the declared ceiling or MEM-64M's reuse cycles",
    )
    parser.add_argument(
        "--platform",
        choices=sorted(PLATFORMS),
        default="qemu-arm-virt",
        help="the pinned QEMU profile and image to build and boot",
    )
    parser.add_argument(
        "--matrix-row",
        type=int,
        help="development only: boot one inventory row of the matrix arm; the run then fails",
    )
    arguments = parser.parse_args()
    if arguments.arm == "adaptive":
        run_adaptive_plane_arm(arguments.platform)
        return
    if arguments.arm == "adaptive-lifecycle":
        run_adaptive_lifecycle_arm(arguments.platform)
        return
    if arguments.arm == "matrix":
        run_matrix_arm(arguments.platform, arguments.matrix_row)
        return
    if arguments.arm in (
        "cspace", "metadata", "bootstrap", "elastic", "fragmentation", "rollback", "conservation",
    ):
        run_adaptive_arm(arguments.platform, arguments.arm)
        return
    if arguments.arm == "stress":
        run_stress_arm(arguments.platform)
        return
    if arguments.arm == "isolation":
        run_capacity_arm(arguments.platform, isolation=True)
        return
    if arguments.arm == "capacity":
        run_capacity_arm(arguments.platform)
        return
    if arguments.arm == "cycles":
        run_cycles_arm(arguments.platform)
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
    check_backing_ledger(transcript)
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
        (ROOT / "build" / f"private-memory-large-map-{arguments.platform}.log").write_text(
            large_map + "\n", encoding="utf-8"
        )
        check_large_map_retry(large_map)
        check_declared_is_installed(large_map, declared)
        check_measured_ceiling(large_map, declared)
        check_only_declared_pages_were_charged(large_map, declared)
        check_segmented_capacity_report(
            large_map,
            profile,
            section,
            TARGET_PROFILES[arguments.platform],
            sum(declared.values()),
        )
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
