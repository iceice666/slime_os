# Memory capacity milestones: mechanism before larger working sets

| Field | Value |
|---|---|
| Date | 2026-09-07 |
| Kind | Decision |
| Status | Proposed |
| Scope | Memory-capacity work items, core-runtime architecture, direction 34 |
| Work items | 01a07a2c-9c4f-7ca0-8417-aba2b48ce16b, 01a07a2d-0017-7efd-9c18-ada16cb8789e, 01a07a2d-003d-77fc-9df8-dda85ed9a083, 01a07a2d-0060-7d52-8e8d-1c7f45246f8d, 01a07a2d-0081-7df4-b7fe-38b933057b4a, 01a07a2d-00a1-727c-a548-130a4475f5bb |
| Gates | `just tasks_check`, `just devlog_check` |
| Trigger | User requested milestones for letting Slime OS use more memory |
| Baseline | Static source review: 2 MiB/task and 8 MiB aggregate private ceilings; no runtime measurements in this planning session |

## Summary

Register a memory-only epic with five observable milestones: large-frame backing,
scalable task ownership/accounting, the first larger component working set,
a consistent ARM kernel/launcher memory platform, and a simultaneous aggregate
workload. All implementation exit conditions remain unobserved targets. The
completed private-memory track is a prerequisite, not a parent reopened to
hold unfinished work; the new epic depends on every child.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Work-item store | Created one epic and five milestones through MyQue, with UUID parent/dependency edges | Store alone owns identity, state, dependencies, and exit conditions |
| Core-runtime roadmap and index | Added memory-capacity rationale and links into the store | Architectural sequencing does not become a second state table |
| Direction 34 | Routed its memory portion to the work items; corrected the ARM RAM distinction and live-shrink assumption | Paper design is not runtime evidence or an implicit authority extension |
| Earlier capacity decision | Migrated only `Roadmap: none` to `Work items: none` after the document checker reported the obsolete machine field | Historical prose and evidence remain unchanged under the permitted front-matter migration |
| Independent plan review | Made the existing arena-alignment planner, component/system-spec quota cutover, probe bounds, and capability-matrix updates explicit; added shared-prefix integration and measured-headroom failure policy | No hidden successor dependency, silent scope reduction, or cross-architecture evidence substitution |

## Decisions

- Use a named qualification workload: an exact 64 MiB private region first,
  then four simultaneous 256 MiB holders. These are planned acceptance sizes,
  not a claim that current Slime can run them or that a product needs that size.
  An ordinary-heap case separately accounts for allocator overhead.
- Preserve exact 4 KiB quota semantics with 2 MiB block backing where legal.
  The small/incremental growth path, lazy page tables, alignment waste, and
  transactional unwind are required work, not details hidden by a 512-fold
  frame-count calculation. Automatic live-page promotion is excluded.
- Split scalable CSpace/metadata and segmented task backing from the ceiling
  change. Static review of `slime-root/src/task.rs` shows the entire private
  quota is currently included in one power-of-two arena reservation. Large
  frames alone do not remove that reservation cost. Admission must budget the
  declared growth envelope rather than assume every request is one large block.
- Bind ceilings to the target through Zutai-owned inputs and migrate builder,
  root, VSpace, and runtime together. Keep small-board ceilings separate. A
  private region remains non-transferable; loaning it is not an alternative
  implementation of shared-buffer capacity.
- Run the ARM platform branch independently. Static review of
  `scripts/build/build-sel4.py` and `just/product.just` found a 1024 MiB
  kernel-build DTB versus a 2048 MiB product launcher. Qualification must
  allocate, touch, and reclaim a frame in the added ordinary-memory range;
  a larger command-line number is not evidence.
- Require independent AArch64 and RV64 QEMU evidence. The current private-memory
  checker launches only AArch64, so extending that owning mechanism is explicit
  work. No checker-per-milestone expansion, no physical-board support claim.
- Do not add a baseline-only milestone, swap, overcommit, hotplug, live shrink,
  larger shared buffers or ELF images, 1 GiB frames, SMP, or more worker threads.
  Backlog-first ordering and green validation remain implementation preconditions.

## Open risks and follow-ups

- [INFERENCE] Bounded segmented backing and capacity-scaled metadata can make
  the named working sets fit, but fragmentation, small-request object counts,
  CNode RAM, and page-table overhead must be measured by the implementation.
- Reusable physical capacity must recover as well as logical page counters:
  repeated failed growth or alignment padding cannot leak space until exit.
  Failed revocation must remain owned and retryable.
- Target-profile/schema versioning and prefix identity must move together;
  increasing only a Rust constant or QEMU `-m` is explicitly insufficient.
- No runtime tests or QEMU boots were run for this documentation-only plan.
  Work-item/document checks validate structure, not any memory-capacity claim.

## Artifacts and provenance

- [Capacity epic](../../.tasks/items/01a07a2c-9c4f-7ca0-8417-aba2b48ce16b.md) and its five UUID-linked children.
- [Architecture and sequencing](../../roadmap/02-core-runtime.md#memory-capacity).
- [Original capacity direction](../../docs/directions/34-capacity-ceilings.md).
- [Earlier capacity decision](../2026-09-02-capacity-ceilings-register/index.md)
  is background, not inherited runtime evidence for the new targets.
- Static sources: [private quota contract](../../contracts/private-memory-budget/v1/schema.zt),
  [private growth](../../slime-root/src/private_memory.rs),
  [root allocation](../../slime-root/src/object_allocator.rs),
  [task construction](../../slime-root/src/task.rs),
  [platform build](../../scripts/build/build-sel4.py),
  [product launcher](../../just/product.just), and
  [current private-memory checker](../../scripts/check/check-sel4-private-memory-plane.py).
