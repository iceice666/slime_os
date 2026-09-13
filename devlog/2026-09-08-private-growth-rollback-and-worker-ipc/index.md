# Private growth: the rollback boundary, failed-map ownership, and worker IPC

| Field | Value |
|---|---|
| Date | 2026-09-08 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root` private growth, arena allocator transaction state, task growth entry point, embedded child fixture, private-memory probe, and the private-memory plane gate |
| Work items | 01a08116-3c0c-7495-ad7a-5f9419f76b49 |
| Gates | `just private_memory_check`, `just test_sel4_root`, `just sel4_root_boot_check`, `just sel4_boot_layout_check`, `just sel4_gate_control_check`, `just fmt_check_all`, `just lint_all` |
| Trigger | Review of the private-memory capacity work reported three source-level correctness defects against `928be3d1042` |
| Baseline | Growth rolled back by revoking a whole extent's parent untyped, a failed mapping returned its object as `Empty`, and growth suspended sibling worker threads |

## Summary

Three defects broke invariants private growth claims. Rollback revoked the
parent untyped of any extent holding an in-flight allocation, and private data
extents are bump-allocated rather than isolated per transaction, so a failed
second growth destroyed backing an earlier committed growth still owned while
the page counts said nothing had changed. A failed mapping marked its retyped
object `Empty` and reusable without deleting the capability, so the allocator
and the kernel disagreed about whether the CSlot was occupied and the retry was
refused rather than served. Growth suspended sibling workers, which in the
pinned seL4 cancels an outstanding IPC and restarts it, so a successful growth
could re-execute another thread's non-idempotent request. Rollback is now scoped
to the transaction's own in-flight objects, a failed mapping retains a typed
unmapped object for the retry, and growth no longer touches any sibling thread.

## Observable symptom

- Command: `just private_memory_check` on `qemu-arm-virt`, with the two injected
  failure images the gate builds.
- Expected: a failed growth leaves every committed page readable; a failed
  mapping is retryable; a growth changes no other thread's IPC.
- Observed (pre-fix, by source-path analysis): a failed second growth revoked
  the committed page's own extent; a first-growth map failure left a
  kernel-occupied CSlot marked empty and reusable; `grow_private_memory`
  suspended and resumed workers unconditionally.
- Exit/fault/serial evidence: the two failure arms had no runtime case at all —
  the injected-failure closure built a graph root, so the fixture arm carrying
  the sentinel probe was compiled out and never ran. Establishing the evidence
  was part of the fix.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `unwind_private_transaction` revoked `extent.parent` and cleared every allocation record belonging to the arena, not just in-flight ones | Committed objects in the same bump-allocated extent were destroyed by an unrelated failure |
| 2 | `commit_private_transaction` only clears `IN_FLIGHT`; it does not move committed objects to another revocation boundary | Extent-level revoke can never be the rollback boundary while extents are shared across growths |
| 3 | `reset_private_in` set `PrivateObjectKind::Empty` plus reusable and decremented live counts, with no `delete`/`revoke` | Allocator and kernel disagreed on slot occupancy; the pinned seL4 requires an empty destination CSlot, so the retry was refused |
| 4 | `back_large` marks `IN_FLIGHT` only after a successful map, so an empty region's first large-frame map failure produced no in-flight record | The outer unwind had nothing to find; recovery had to be local to the failed map |
| 5 | Pinned seL4 `suspend` starts with `cancelIPC`; `restart` sets `ThreadState_Restart` | `TCB_Suspend`/`Resume` is not a transparent pause, so it cannot be used as a quiescence mechanism around growth |
| 6 | The rollback case's closure carried root role `private-memory-fail-second-allocation` but not fixture mode, so the boot launched the component graph | The injected failure landed on whichever component grew first, and the sentinel probe in the embedded child never executed |

## Root cause

The rollback boundary was the *extent*, while the ownership unit is the
*allocation record*. Private data extents are bump-allocated and shared by
successive growths, and nothing moves a committed object out of the extent a
later transaction allocates from, so "revoke the parent of any extent with an
in-flight object" necessarily includes committed objects. Separately, failure
handling treated bookkeeping as authoritative over kernel state: `reset_private_in`
published a slot as empty and reusable on a path that performed no kernel
delete or revoke, and growth treated `TCB_Suspend`/`TCB_Resume` as semantically
neutral when the kernel defines suspend to cancel IPC and restart to re-run from
the restart PC.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Arena allocator | `unwind_private_transaction` now walks only records marked `IN_FLIGHT`: it unmaps an in-flight frame and retains it behind its existing capability as a reusable unmapped object of its own type, and retains an in-flight leaf table mapped, because the retry maps through the same fixed region | A failed growth destroys only the objects that growth created |
| Arena allocator | `reset_private_in` keeps ownership: the record stays private and correctly typed and becomes an unmapped reusable object, so the CSlot is never advertised as free to retype into while the kernel holds a capability there | Allocator bookkeeping cannot claim a slot the kernel still considers occupied |
| Task growth | `grow_private_memory` no longer suspends or resumes sibling threads; the doc comment records why suspension cannot be used here and that every thread in the process shares the task badge | Growing memory does not alter another thread's IPC state |
| Injected failure arm | The rollback role builds in fixture mode and arms its one-shot retype failure from the fixture service loop, on a transaction that already holds a committed granule; the fixture verdict expects that arm's one page and one grant | The injected failure lands on the request the case is about instead of the first growth in the boot |
| Probe | The private-memory probe's worker serves exactly one RPC and then parks, rather than looping around a body that always parks, and issues its own growth request after the RPC completes | A repeated delivery is observable instead of absorbed by another iteration, and a worker's growth is shown to be adjudicated against its task's region |
| Plane gate | The checker accepts the fixture root's `SLIME_ROOT READY` terminal and matches the fixture's sentinel value | The rollback case's serial evidence is actually collected |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A failed growth destroys a committed page's backing | `just private_memory_check` | The rollback case's `SLIME_CHILD mem rollback preserved pages=1 … survived=0x4d454d5f42415345` is missing, or `SLIME_CHILD mem rollback failed` appears |
| A failed mapping leaves a slot the retry cannot use | `just private_memory_check` | The large-map case does not report `retries=1` with one coherent backing object |
| Growth disturbs a sibling thread's IPC | `just private_memory_check` | The granted probe stops reporting `worker_rpc_once=1`, or reports `FAIL worker IPC repeated` |
| A worker's own growth is not adjudicated against its task's region | `just private_memory_check` | The granted probe stops reporting `worker_grow_refused=1`, or reports `FAIL worker growth was not adjudicated` |
| The in-flight/committed distinction or retained failure state regresses in the records | `just test_sel4_root` | The three record-level ownership cases fail |
| Marker deletion, reordering, or explicit failure is accepted | `just sel4_gate_control_check` | The private-memory marker contract stops refusing bad evidence |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just private_memory_check` (`qemu-arm-virt`) | 24 markers across 7 causal chains and 3 image cases; the injected second-allocation failure recorded one refusal with `allocated: 1` and the fixture read its sentinel back; the large-map failure retried with `retries=1`; the granted probe reported `worker_rpc_once=1 worker_grow_refused=1` | Direct |
| `just test_sel4_root` | 219/219 host tests across 19 modules, including the three new record-level ownership cases | Direct |
| `just sel4_root_boot_check` | Ordered generation, timer, task, IPC, fault, and ready markers observed on the pinned AArch64 product | Direct |
| `just sel4_boot_layout_check` | 31 plane layouts matched their frozen fixtures | Direct |
| `just sel4_gate_control_check` | Marker deletion, reordering, and explicit-failure controls remained refused | Direct |
| `just contracts_check` and `just generation_check` | Contract models, bindings, closure records, and deterministic generation validation passed | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff` | Formatting and lint stacks passed with warnings denied | Direct |
| RV64 (`qemu-riscv-virt`) private-memory arm | Not run in this pass; the injected-failure cases are AArch64-only by construction in the gate | Unobserved |

## Decisions

- Decision: make the rollback boundary the allocation record, not the extent.
- Rationale: extents are bump-allocated and shared across growths, so only the
  per-object in-flight marker distinguishes this transaction's objects from
  memory the caller already holds; unmapping and retaining an in-flight frame
  keeps the untyped watermark monotonic and leaves the retry a coherent object.
- Rejected alternative: give each transaction its own revocation boundary. That
  makes extent-level revoke correct again, but it means one extent per growth,
  which the capacity plan would have to reserve for every legal incremental
  growth of every holder.
- Decision: on a failed mapping, retain the object rather than converting it to
  `Empty`.
- Rationale: kernel state is authoritative. Publishing an occupied CSlot as
  empty makes the retry fail on the kernel's empty-destination requirement, and
  cleanup that has not been confirmed cannot be accounted as completed.
- Rejected alternative: delete the capability and then mark the record empty.
  Correct only if the delete is confirmed; on delete failure ownership must be
  retained anyway, so retention is the single path that is always right here.
- Decision: remove worker suspension from growth entirely.
- Rationale: the pinned kernel's `suspend` cancels IPC and `restart` re-runs
  from the restart PC, so there is no transparent-pause reading of it; mappings
  are published one kernel operation at a time and the region's page count is
  only updated on commit, so a sibling either sees the old count or the new one.
- Rejected alternative: suspend only workers not currently in an IPC. The
  kernel exposes no such predicate to the root, and any check would race the
  thread it is checking.
- Decision: build the injected-failure rollback role as a fixture root and arm
  the failure from the request that carries the case.
- Rationale: the sentinel probe lives in the embedded child, and the plane's
  granted component grows one aligned 512-page block, which cannot express
  "commit one page, then fail the second." Arming per request also stops the
  one-shot failure from being consumed by an unrelated boot-time growth.
- Rejected alternative: add a small-quota instance to the shared composition.
  That changes a frozen boot-layout fixture and every closure identity on the
  plane to obtain a case the existing fixture root already expresses.

## Open risks and follow-ups

- The injected-failure cases run on `qemu-arm-virt` only. RV64 inherits the
  fixes but not the failure-arm evidence.
- Rollback retains an unwound frame as reusable backing rather than returning it
  to the extent's watermark, so a long sequence of failed growths holds
  descriptors and CSlots until the task dies. Bounded by the per-holder
  reservation, but it is retention rather than reclamation.

## Artifacts and provenance

- [Work item](../../.tasks/items/01a08116-3c0c-7495-ad7a-5f9419f76b49.md)
- Implementation: [`slime-root/src/object_allocator.rs`](../../slime-root/src/object_allocator.rs), [`slime-root/src/private_memory.rs`](../../slime-root/src/private_memory.rs), [`slime-root/src/task.rs`](../../slime-root/src/task.rs)
- Fixture arm: [`slime-root/child/src/main.rs`](../../slime-root/child/src/main.rs) and [`slime-root/src/fixture_runtime.rs`](../../slime-root/src/fixture_runtime.rs)
- Probe: [`components/testkit/private-memory-probe/src/main.rs`](../../components/testkit/private-memory-probe/src/main.rs)
- Gate: [`scripts/check/check-sel4-private-memory-plane.py`](../../scripts/check/check-sel4-private-memory-plane.py)
- Preceding milestone entries: [MEM-LARGE](../2026-09-07-mem-large-private-frames/index.md) and [MEM-ARENAS](../2026-09-08-mem-arenas-segmented-backing/index.md)
