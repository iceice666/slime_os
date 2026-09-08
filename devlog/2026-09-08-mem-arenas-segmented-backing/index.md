# MEM-ARENAS: capacity-scaled CSpace and segmented task backing

| Field | Value |
|---|---|
| Date | 2026-09-08 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root` task allocation, private growth and cleanup, seL4 QEMU CSpace profiles, closure identities, and memory/reclamation gates |
| Work items | 01a07a2d-003d-77fc-9df8-dda85ed9a083 |
| Gates | `just private_memory_check`, `just sel4_reclamation_check`, `just sel4_capability_layout_check`, `just sel4_boot_layout_check` |
| Trigger | MEM-ARENAS milestone implementation |
| Baseline | Each task owned one power-of-two untyped arena and private slot metadata scaled poorly against the maximum page count |

## Summary

Task ownership now uses one static extent plus bounded, independently reclaimable private-data and page-table extents. Allocation records are root-wide and compact, QEMU profiles expose a root CSpace sized with the matching metadata and kernel-memory cost, and the runtime reports an admission-time four-holder 256 MiB capacity plan against actual ordinary memory, descriptor tables, graph slots, and free CSlots. Both QEMU architectures reached the existing public private-memory ceiling and reclaimed the segmented backing.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Root allocator | Replaced fixed per-task allocation arrays and monolithic arenas with compact root-wide allocation records and typed reclaimable extents | Metadata is bounded by simultaneous ownership rather than every task multiplied by the global page maximum |
| Construction and growth | Provisioned private backing, descriptors, and CSlots before task publication; transactional growth revokes touched extents on failure | A published quota is reachable and failed growth cannot consume a second hidden reservation |
| Cleanup | Made partial construction and task-death revocation retryable, serial-bound, and exactly-once before slots return | Failed revocation remains owned; stale arena identities cannot reclaim reused backing |
| Capacity reporting | Reported payload, tables, alignment, mapped/reusable RAM, allocation/extent descriptors, root image/metadata/stack/heap, graph slots, free CSlots, and ordinary memory | The four-holder sizing claim is checked against live resource capacities rather than best-case frame counts |
| Platform profiles | Raised only AArch64 and RV64 QEMU root CNodes to 19 bits and pinned their rebuilt prefixes; retained the Duo-specific descriptor envelope | QEMU capacity does not impose GiB-target metadata or CSpace costs on the physical small target |
| Verification | Extended private-memory execution to RV64, added capacity evidence to the mutation-tested marker contract, and extended reclamation evidence for segmented extents | Architecture and cleanup claims are observed through existing owning gates |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Quota cannot reach its declared ceiling on either QEMU architecture | `just private_memory_check` | Missing growth/refusal/capacity marker, resource shortfall, or non-quiescent cleanup |
| Task exit leaks extents or CSlots | `just sel4_reclamation_check` | Reclamation census fails to return and reuse segmented ownership |
| Wider root CSpace changes child capability placement | `just sel4_capability_layout_check` | A declared, missing, extra, aliased, wrong-slot, wrong-type, or wrong-rights capability is accepted |
| Generated boot layouts drift | `just sel4_boot_layout_check` | Any plane differs from its frozen resolved layout |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test_sel4_root` | 216/216 tests passed across 19 modules | Direct |
| `just private_memory_check` | AArch64 and RV64 each observed 24 markers across seven causal chains; the 512-page public quota was reachable and exactly bounded | Direct |
| `just sel4_reclamation_check` | Segmented task extents and root CSlots were reclaimed and reused | Direct |
| `just sel4_root_boot_check` | Ordered generation, timer, task, IPC, fault, and ready markers observed | Direct |
| `just sel4_capability_layout_check` | Every child CSpace matched the admitted plan and all six mutations were refused | Direct |
| `just sel4_boot_layout_check` | 31 plane layouts matched their fixtures | Direct |
| `just sel4_gate_control_check` | Marker deletion, reordering, and failure controls passed, including independent capacity evidence | Direct |
| `just sel4_pin_check` | Source, toolchain, target, config, and installed prefix pins matched | Direct |
| `just contracts_check` | Contract models, bindings, closure records, and generation checks passed | Direct |
| `just generation_check` | Deterministic generation validation passed | Direct |
| `just fmt_check_all` | Rust formatting passed | Direct |
| `just lint_all` | Rust clippy stack passed with warnings denied | Direct |
| `just ruff` | Python checks passed | Direct |

## Decisions

- Decision: use pre-provisioned 2 MiB private-data extents and separate page-table extents, while preserving exact 4 KiB growth semantics.
- Rationale: this removes monolithic power-of-two overreservation without letting lazy mapping disguise an unreserved quota promise.
- Rejected alternative: sizing by large-frame best case or installing maximum-page arrays per task; either fails legal one-page growth or spends root image/CSlot capacity before tasks run.

## Open risks and follow-ups

- Larger public ceilings remain owned by MEM-64M and MEM-1G; this milestone changes mechanism and sizing evidence only.
- Physical RAM-map qualification remains owned by MEM-PLATFORM; QEMU success is not board evidence.
- Shared-buffer capacity, ELF limits, swap, overcommit, live shrink, 1 GiB frames, SMP, and extra worker threads remain excluded.

## Artifacts and provenance

- [Work item](../../.tasks/items/01a07a2d-003d-77fc-9df8-dda85ed9a083.md)
- [Memory-capacity architecture](../../roadmap/02-core-runtime.md#memory-capacity)
- [Planning decision](../2026-09-07-memory-capacity-milestones/index.md)
- Implementation: [`slime-root/src/object_allocator.rs`](../../slime-root/src/object_allocator.rs), [`slime-root/src/task.rs`](../../slime-root/src/task.rs), and [`slime-root/src/private_memory.rs`](../../slime-root/src/private_memory.rs)
- Gates: [`scripts/check/check-sel4-private-memory-plane.py`](../../scripts/check/check-sel4-private-memory-plane.py) and [`scripts/check/check-sel4-reclamation-plane.py`](../../scripts/check/check-sel4-reclamation-plane.py)

## Corrections

- 2026-09-08: the *Changes* row stating that transactional growth "revokes
  touched extents on failure" describes the mechanism as it landed here, and
  that mechanism was wrong: private data extents are bump-allocated and shared
  across growths, so revoking a touched extent also destroyed committed pages
  the caller still held. Rollback now releases only the transaction's own
  in-flight allocations. Two related failure-path defects — a failed mapping
  publishing its still-occupied CSlot as empty, and growth suspending sibling
  worker threads — were fixed in the same pass. See
  [private growth: the rollback boundary, failed-map ownership, and worker IPC](../2026-09-08-private-growth-rollback-and-worker-ipc/index.md).
- 2026-09-08: the `just test_sel4_root` row records 216/216 as observed here.
  The asserted count is now 219.
