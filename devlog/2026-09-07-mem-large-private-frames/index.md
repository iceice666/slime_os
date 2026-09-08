# Mixed-size private frames preserve exact growth semantics

| Field | Value |
|---|---|
| Date | 2026-09-07 |
| Kind | Change |
| Status | Verified |
| Scope | Private-memory arena planning, AArch64/RV64 VSpace mappings, task accounting, component probe, and seL4 plane gate |
| Work items | 01a07a2d-0017-7efd-9c18-ada16cb8789e |
| Gates | `just private_memory_check`, `just test_sel4_root`, `just sel4_root_boot_check`, `just sel4_gate_control_check`, `just fmt_check_all`, `just lint_all` |
| Trigger | MEM-LARGE required one aligned 512-page private extent on both supported QEMU architectures without changing the page-count ABI or ceilings |
| Baseline | Private growth mapped only 4 KiB frames and the owning plane checker executed AArch64 only |

## Summary

Private growth can now back an empty, aligned 512-page request with one architecture-qualified 2 MiB frame while preserving 4 KiB backing for small or unaligned growth. Task admission reserves the sequential bulk-then-incremental physical watermark and the simultaneous CSlot population needed when a failed mapping retains reusable backing. Leaf page tables remain lazy so they cannot preclude a block mapping; workers are suspended around the address-space transaction, and resume failure is reported without replacing an already-committed growth result. The existing 512-page task and 2048-page aggregate ceilings, page-count syscall ABI, stable virtual base, read/write permissions, and execute-never policy are unchanged. Both supported QEMU architectures and the final root, gate-control, lint, and format stacks pass.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| seL4 object dispatch | Reuses rust-sel4's architecture-qualified frame and intermediate-table mapping APIs; failed final table capabilities are deleted so seL4 finalization detaches them | Private-memory code remains target-generic without carrying an unpinned dependency patch |
| Child VSpace | Aligns the private window to the 2 MiB frame span, plans upper translation levels across it, and leaves the leaf table absent until a 4 KiB mapping needs it | An aligned block mapping is available at first growth, while existing image and thread mappings remain unchanged |
| Arena allocator | Models object-size alignment, provisions a bounded private-backing CSlot pool, and retains safely unmapped frames or leaf tables for deterministic retry | Failed attempts preserve the task's monotonic seL4 untyped cursor without silently losing reusable physical backing or exhausting root CSpace |
| Private growth | Selects one 2 MiB frame for an empty aligned full-window request, otherwise lazily installs one leaf table and exact 4 KiB frames; suspends sibling workers while mappings are uncommitted and tracks actual backing shape | Page accounting remains exact even though kernel object counts differ, and no worker can observe a partially committed address-space change |
| Probe and checker | The probe touches every 4 KiB subpage, verifies zero-fill and retained writes, and the checker requires `large_frames=1 base_frames=0 leaf_tables=0`; `just private_memory_check` invokes the same checker on `qemu-arm-virt` and `qemu-riscv-virt` | Each ISA produces independent runtime evidence rather than inheriting an ARM result |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A leaf table occupies the entry needed by a 2 MiB frame | `just test_sel4_root` | Private-window table-span or arena-plan host tests fail |
| Bulk growth silently falls back to 512 base frames or misreports its backing | `just private_memory_check` | Missing `large_frames=1 base_frames=0 leaf_tables=0` growth marker on either QEMU target |
| A failed growth consumes reusable backing, CSlots, or mappings | `just test_sel4_root` | Retained-backing reuse, sequential arena-watermark, CSlot-reservation, or growth-transaction cases fail |
| Marker deletion, reordering, or explicit failure is accepted | `just sel4_gate_control_check` | The private-memory gate's pinned 23-marker contract or mutation controls stop refusing bad evidence |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test_sel4_root` | 216 host unit tests passed with the repository's asserted module/count contract | Direct |
| `just private_memory_check` | AArch64 and RV64 each observed 23 markers across 7 causal chains; the declared 512-page ceiling used exactly one 2 MiB frame | Direct |
| `just sel4_root_boot_check` | Ordered generation, timer, task, IPC, fault, and ready markers observed on the pinned AArch64 product | Direct |
| `just sel4_gate_control_check` | Missing, reordered, and explicit-failure evidence remained rejected, including the private-memory marker contract | Direct |
| `just lint_all` | Rust and Python lint stack passed with warnings denied | Direct |
| `just fmt_check_all` | Repository Rust formatting checks passed | Direct |

## Decisions

- Decision: use a large frame only when the entire aligned 512-page window is requested while still empty.
- Rationale: this is the one state where no live leaf table or base-page mapping can conflict with the block entry, and it preserves exact tail-growth behavior without promotion.
- Rejected alternative: silently fall back to 512 base frames when aligned large-frame retype fails. That would promise success outside the arena construction selected for the request and hide the unavailable contiguous object the milestone requires callers to observe.
- Decision: invoke the existing private-memory checker once per QEMU platform from `just private_memory_check`.
- Rationale: one checker owns the marker contract and data-flow assertions; separate invocations keep target evidence independent without creating a checker per milestone.
- Rejected alternative: treat the AArch64 closure as RV64-qualified. A closure names one target profile and prefix, so the RV64 arm uses the same build orchestrator with a distinct private-memory variant; it installs the RV64 prefix first, rebuilds the composition for that profile, and writes a separate image.

## Open risks and follow-ups

- The current public reservation is exactly one 2 MiB span. Any later target-qualified window-size increase must preserve at least 2 MiB virtual alignment and rerun the block-mapping case rather than infer it from a larger span.
- Larger private ceilings, segmented arena ownership, live shrink, automatic promotion, and large shared buffers remain explicitly outside this milestone.

## Artifacts and provenance

- [MEM-LARGE work item](../../.tasks/items/01a07a2d-0017-7efd-9c18-ada16cb8789e.md)
- [Private-memory mechanism](../../slime-root/src/private_memory.rs)
- [Arena allocator](../../slime-root/src/object_allocator.rs)
- [Child VSpace construction](../../slime-root/src/child_vspace.rs)
- [Component probe](../../components/testkit/private-memory-probe/src/main.rs)
- [Owning QEMU checker](../../scripts/check/check-sel4-private-memory-plane.py)

## Corrections

- 2026-09-08: this entry's *Changes* rows describe suspending sibling workers
  around the address-space transaction. That mechanism was removed: in the
  pinned seL4, `suspend` begins with `cancelIPC` and `restart` resumes from the
  restart PC, so it cancels and re-runs a worker's outstanding `Call` rather
  than pausing it transparently. The transaction's own claim — that no worker
  observes a partially committed address-space change — is instead carried by
  the region's page count being published only on commit. See
  [private growth: the rollback boundary, failed-map ownership, and worker IPC](../2026-09-08-private-growth-rollback-and-worker-ipc/index.md).
- 2026-09-08: the *Verification* row recording 216 host unit tests remains the
  observed result of that run. The asserted count is now 219, after three
  record-level ownership cases were added by the entry linked above.
