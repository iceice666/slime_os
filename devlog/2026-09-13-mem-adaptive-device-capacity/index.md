# Device-derived private-memory capacity, not another fixed ceiling

| Field | Value |
|---|---|
| Date | 2026-09-13 |
| Kind | Decision |
| Status | Proposed |
| Scope | Private-memory policy/admission, task backing, CSpace/metadata, platform inventory; research and work-item records only |
| Work items | 01a09b65-4386-7b79-a64a-f263de037b8d |
| Gates | `just tasks_check`, `just devlog_check` |
| Trigger | User clarified that the goal is automatic use of device-available RAM, not a fixed 64 MiB or 1 GiB private-memory envelope, and requested research plus a new work item |
| Baseline | Current source has target-qualified 64 MiB/128 MiB QEMU ceilings; the separate simultaneous 1 GiB qualification remains open |

## Summary

The allocator already consumes seL4 BootInfo ordinary RAM, but capacity is bounded
by static target rows, full-quota spawn reservations and fixed management tables.
Changing only a page ceiling would not deliver the requested behavior. The proposed
continuation separates guaranteed backing, elastic request authorization and actual
commitment: an explicitly authorized task can use a device-derived common pool on
demand without reserving its entire maximum at spawn. This is a design proposal,
not an implemented or qualified runtime feature. The existing fixed-capacity epic
keeps its scope; the new item owns adaptive behavior.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Canonical work-item store | Created MEM-ADAPTIVE with UUID `01a09b65-4386-7b79-a64a-f263de037b8d`, related it to the existing capacity epic, and declared the completed segmented-backing and ordinary-inventory prerequisites through MyQue | New scope has independent identity and observable acceptance; existing qualification is not silently redefined |
| Research | Traced ordinary-memory admission, physical reservation, virtual-window construction, policy validation and runtime heap growth | Available RAM, authorization, guarantees and mapped pages are not interchangeable quantities |
| Devlog index | Registered this proposed decision | Research evidence is separate from tracker state and future runtime evidence |

No runtime source, generated contract, platform pin or existing milestone body was
changed. Only the existing epic's reciprocal relationship metadata was updated.
The frozen backlog index and architectural roadmap were intentionally unchanged.

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `myque list --tag memory-capacity` and `myque list --state open` | Existing adaptive-capacity item was absent; MEM-CAPACITY and MEM-1G were open, the mechanism/platform prerequisites done | Direct tracker observation |
| Source-path investigation below | Full-quota reservation and static capacity coupling found in implementation, not inferred from old direction documents | Direct source observation; no execution claim |
| Python arithmetic over recorded ordinary byte counts | ARM: 1511.053 MiB / 386829 whole 4 KiB pages; RV64: 3047.173 MiB / 780076 pages. RV64 payload-only page count exceeds 524288 slots by 255788 | Direct calculation over inherited inventory; not a backing-fit or runtime test |
| `just tasks_check` | Passed: 300 items, 94 frozen backlog headings indexed | Direct |
| `just devlog_check` | Passed: 328 entries, 328 indexed | Direct |
| `typos` over the new item and this entry | Passed, exit 0 | Direct |

The inventory values themselves are inherited from the
[ordinary-memory qualification](../2026-09-13-mem-platform-ordinary-inventory/index.md):
1584454000 bytes on ARM and 3195192256 on RV64. They are not a new measurement of
free memory. The calculation excludes new metadata, task objects, page tables,
alignment and operational reserves; it only establishes that RAM bytes cannot be
converted directly into an affordable all-base-page capacity. No QEMU, runtime
tests, Rust formatter or linter was run for this research/documentation-only change.

## Decisions

- **Keep a separate adaptive work item.** The fixed-capacity epic qualifies four
  simultaneous 256 MiB holders; that is useful regression evidence, not the desired
  permanent architecture limit. The new item depends on actual completed mechanism
  and inventory prerequisites, not on an artificial requirement to finish the fixed
  1 GiB workload before adaptive research can proceed. Existing qualification must
  still be preserved or rerun when shared implementation changes.
- **Use ordinary BootInfo RAM as resource authority.**
  [`ObjectAllocator::initialize`](../../slime-root/src/object_allocator.rs) at lines
  1312-1362 separates ordinary and device ranges. Account resources already excluded
  by seL4 only once, then subtract actual subsequent commitments and declared
  reserves. A formula using installed RAM minus guessed overhead is not admission.
- **Do not equate lazy mappings with unreserved memory.**
  [`TaskTable` construction](../../slime-root/src/task.rs), lines 739-780, provisions
  private backing before publication. The allocator's `PrivateBackingLayout` and
  `provision_private_backing` at lines 610-699 and 2118-2154 reserve 2 MiB data extents,
  4 KiB table extents and worst-case object slots/descriptors for the entire quota.
  [INFERENCE] Resolving bigger immutable quotas alone would improve static sizing,
  but idle holders could still strand the spare RAM the user wants usable.
- **Separate guarantee from elastic permission.** Proposed policy explicitly opts
  holders into an elastic pool and permits pool-relative maxima. Minima/guaranteed
  resources are affordable before launch; excess is acquired transactionally as
  needed and can fail under contention. No successful grow lacks physical backing,
  and authorized maxima are not simultaneous guarantees. This is not swap or
  overcommit of committed/guaranteed bytes. No fairness weights, OOM killer or new
  policy service are introduced merely to perform this research.
- **Keep policy deterministic and resolution local to root.** Current
  [system composition](../../scripts/lib/system_spec.py), lines 1123-1148, derives
  absolute quotas and validates static target rows; root
  [generation admission](../../slime-root/src/generation.rs), lines 1241-1279,
  repeats target-bound validation. The proposed versioned Zutai policy stores
  authorization, not host-resolved free RAM. Root enforces the declared policy from
  its inventory. Host checks prove policy structure, not live capacity. Existing
  absolute semantics must not silently become best effort; version handling,
  mutually conflicting policy objects and old-root rejection need an explicit
  migration decision.
- **Management resources must scale too.**
  [`object_allocator.rs`](../../slime-root/src/object_allocator.rs), lines 56-218,
  has fixed slot/descriptor/extent bounds; lines 1023-1109 pack a 19-bit slot and
  six size bits below flags starting at bit 25. Increasing slot width alone would
  overlap fields. The runtime region's leaf bitmap is also compile-time sized
  ([`private_memory.rs`](../../slime-root/src/private_memory.rs), lines 54-66 and
  125-178). The new scope must solve bounded bootstrap/pool sizing, not allocate
  maximum-device-sized static arrays or relabel these limits as adaptive capacity.
- **Reserve virtual addresses, not every elastic physical byte.**
  [`private_window`](../../slime-root/src/child_vspace.rs), lines 720-783, currently
  derives placement and span from a compile-time window. Native addresses must
  remain stable while a device-derived maximum changes the reservation at spawn;
  large-frame alignment and window length are separate constraints. Every mapping
  path must continue to exclude the entire unbacked reservation.
- **Retain growth and lifetime safety.** The public
  [`private_memory_grow`](../../components/runtime/src/syscall.rs), lines 830-852,
  already specifies zeroed, non-executable pages, fixed base and atomic refusal.
  [`Heap::grow`](../../components/runtime/src/private_heap.rs), lines 319-355,
  retries a failed batch with exact demand; it need not learn the machine's RAM
  size merely to allocate. Capacity reporting may need a versioned interface, but
  transient free RAM must not be advertised as a guarantee. In-task `free` is not
  system decommit; live shrink remains a separate contract decision.
- **Do not confuse adaptation with hardware discovery.**
  [`dump_device_tree`](../../scripts/build/build-sel4.py), lines 657-698, uses pinned
  RAM at kernel build time. QEMU qualification must vary kernel-visible inventories
  under exact platform identities while retaining unchanged adaptive policy and
  root/component binaries where ABI permits. This does not prove one boot image
  can discover arbitrary installed RAM, or qualify a physical board's memory map.

## Open risks and follow-ups

- [ ] The new work item owns the contract shape, deterministic contention semantics,
  instance/subtree entitlement accounting, bootstrap metadata strategy and all
  runtime implementation. This research does not settle byte layouts or add code.
- [ ] Mixed 4 KiB/2 MiB capacity must account real fallback costs. A bulk-only
  large-frame result cannot conceal CSlot exhaustion or guarantee arbitrary small
  growth. Report and explain any capacity difference instead of counting all RAM
  as equivalent payload.
- [ ] Required retained leaf tables during rollback must stay charged and reusable;
  newly acquired elastic backing needs safe return without revoking peer/live data.
  Global ordinary untyped watermarks are monotonic, so deleting a frame alone does
  not make those bytes reusable by the existing global allocator.
- [ ] Operational reserves must cover actual admitted commitments and restart needs;
  guessed percentages or double-subtracted boot costs can strand usable RAM or
  falsely admit guarantees. Low-memory boot itself is an acceptance boundary.
- [ ] Physical-board capacity remains unqualified without inventory and execution
  on that board. Generic allocation mechanism and verified hardware support are
  different claims; no board is relabeled by this research.

## Artifacts and provenance

- Canonical item: [MEM-ADAPTIVE](../../.tasks/items/01a09b65-4386-7b79-a64a-f263de037b8d.md).
- Related fixed qualification: [MEM-CAPACITY](../../.tasks/items/01a07a2c-9c4f-7ca0-8417-aba2b48ce16b.md)
  and [MEM-1G](../../.tasks/items/01a07a2d-00a1-727c-a548-130a4475f5bb.md).
- Inherited runtime inventory: [MEM-PLATFORM evidence](../2026-09-13-mem-platform-ordinary-inventory/index.md).
- Source links above are the research evidence. No new serial transcript or
  simulated runtime result was produced; the arithmetic is reproduced by integer
  division of the two inherited byte counts by 4096 and comparison with 524288.
