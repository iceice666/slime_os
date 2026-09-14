# Memory capacity qualification

**Canonical work item:** `01a07a2c-9c4f-7ca0-8417-aba2b48ce16b`

## Goal

Extend the completed private-memory mechanism beyond its current target-qualified
64 MiB holder envelope without turning emulator RAM size into a capability claim.
This is a bounded qualification workload, not a general promise of large-memory
or physical-board support.

## Required slices

1. mixed 2 MiB and 4 KiB private backing with exact page-count growth and no
   implicit live-page promotion;
2. bounded extent metadata, honest root-CSpace/task-arena sizing, and multiple
   reclaimable backing extents per task;
3. target-bound budget publication and a 64 MiB working set, with payload cost
   separated from allocator and mapping overhead;
4. kernel, installed-prefix, closure, and launcher agreement on usable ordinary
   memory beyond the old AArch64 window;
5. four simultaneous 256 MiB holders on both QEMU reference architectures,
   including isolation and repeated exit/fault reclamation.

The canonical child UUIDs and dependency edges are in the work-item store; this
page does not restate their state.

## Invariants

- Capacity is keyed by exact target-profile name and bounded by generation data.
- Virtual reservation, owned backing extents, committed quota pages, frame
  objects, page tables, slots, and metadata are reported separately.
- Legal small growth requests must fit the published worst-case metadata and
  object envelope; best-case large-frame arithmetic cannot justify a ceiling.
- Private memory remains non-transferable, zero-filled before reuse, writable,
  non-executable where the target mapping API can enforce it, and fully reclaimed
  on task death.
- A failed large mapping may fall back to base pages only after its unmapped
  whole extent is safely reusable; no duplicate quota backing is allowed.
- Both QEMU architectures require their own execution evidence. Neither result
  qualifies physical RAM.

## Exclusions

No swap, overcommit, RAM hotplug, live shrink, 1 GiB frames, automatic page
promotion, larger shared buffers, larger component images, SMP, additional
worker threads, or physical-machine support is included.

## Qualification method

The capacity run must combine simultaneous residency with repeated clean-exit
and deliberate-fault lives. It checks every re-served word for zero, writes and
reads the complete working set to detect aliasing, compares resource watermarks
across incarnations, records backing shape, and confirms another holder's
account is unchanged.

The method is derived from the current private-memory reclamation invariant; one
historical run's exact watermark values are not expected constants.

## Exit condition

On the qualified 2 GiB AArch64 QEMU platform and the existing 3 GiB RV64 QEMU
platform, four simultaneously resident 256 MiB private holders remain isolated,
stay inside exact declared budgets, recover from exhaustion, and repeatedly
return all pages, objects, slots, and accounting after both exit and fault.

## Owning references

- [`../architecture/private-memory.md`](../architecture/private-memory.md)
- `contracts/private-memory-budget/v1/`
- `slime-root/src/private_memory.rs`
- `components/runtime/src/private_heap.rs`
- `.tasks/items/01a07a2c-9c4f-7ca0-8417-aba2b48ce16b.md`
