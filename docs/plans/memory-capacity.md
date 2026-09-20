# Memory capacity qualification

**Canonical work item:** `01a07a2c-9c4f-7ca0-8417-aba2b48ce16b`

## Goal

Extend the completed private-memory mechanism from the earlier target-qualified
64 MiB holder envelope to four simultaneous 256 MiB holders without turning
emulator RAM size into a capability claim.
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
  Physical targets retain the conservative default: a board's 12-bit root CNode
  cannot hold the descriptor tables a larger holder needs, and no board has run
  the workload.
- Envelopes beyond the published 256 MiB per holder and 1 GiB aggregate remain
  unpublished, as does that envelope on any target other than the two QEMU
  profiles that ran the qualification workload. Publication requires the
  workload to run on the target being published.
- Virtual reservation, owned backing extents, committed quota pages, frame
  objects, page tables, slots, and metadata are reported separately.
- Legal small growth requests must fit the published worst-case metadata and
  object envelope; best-case large-frame arithmetic cannot justify a ceiling.
- Root-image and root-CNode storage consume boot capacity before holders run.
  Include statically sized metadata, rootserver backing, alignment waste, and
  page-table costs; increasing the CNode is not a substitute for scalable
  backing and accounting.
  Small growth must not commit an uncharged large frame, and admitted capacity
  must remain representable by its metadata even under fragmented backing.
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

Budget changes keep schema, generator, builder, root admission, VSpace, and
runtime aligned. Existing checker ownership and closure-derived compositions
remain the integration path; a capacity milestone does not add a top-level
checker.

The method is derived from the current private-memory reclamation invariant; one
historical run's exact watermark values are not expected constants.

## Remaining sizing question

For a representative multi-holder composition, does the admitted task-count
limit become the binding constraint before memory capacity? Qualification must
distinguish that limit from per-holder and aggregate memory exhaustion rather
than infer workload capacity from a memory ceiling alone.

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
