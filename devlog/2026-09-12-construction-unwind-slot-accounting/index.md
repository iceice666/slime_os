# Pre-publication construction unwinds dropped the slots they reclaimed

| Field | Value |
|---|---|
| Date | 2026-09-12 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root/src/task.rs` construction unwind and reclamation accounting, `slime-root/src/object_allocator.rs` host arena seam, `just/quality.just` host-test count, regenerated system-image closures and test-run records |
| Work items | 01a07a2d-003d-77fc-9df8-dda85ed9a083 |
| Gates | `just test_sel4_root`, `just sel4_reclamation_check`, `just sel4_root_boot_check`, `just system_image_closure_check`, `just contracts_check`, `just fmt_check_all`, `just lint_all` |
| Trigger | PR #25 review of `2b3c03a993d3` reported that both early construction-failure branches discard `CleanupRecord::revoke`'s return value |
| Baseline | Branch head `af6c84d6`, where `TaskTable::reclaim` and the construction-body failure path both charge `reclaimed_slots` and the two early branches do not |

## Summary

`TaskTable::create`'s descriptor-preflight and private-provisioning failure
branches called `cleanup.revoke(allocator)` and dropped its `usize` result,
while the construction-body branch and `TaskTable::reclaim` both added it to
`self.reclaimed_slots`. A task that failed at either early boundary therefore
returned its root CSlots to the allocator — which reissues them immediately —
without the table's reclamation ledger recording them, so `reclaimed_slots()`
and the `SLIME_ROOT READY ... reclaimed_slots=` marker it feeds permanently
under-reported the slots the pool had already handed back. Every failure path
now reaches the allocator through one `unwind_construction` helper that owns
the charge, and a host regression measures the ledger against what the
allocator returned.

## Observable symptom

- Command: source review of `slime-root/src/task.rs:728-751` against `:989-996` and `:1114-1124`.
- Expected: every completed pre-publication revoke adds its released slot count to `TaskTable::reclaimed_slots`.
- Observed: the descriptor-preflight and `provision_private_backing` branches matched only on `Err`, discarding the `Ok(usize)` count.
- Exit/fault/serial evidence: no runtime marker exposes the divergence, because no composition in the corpus fails at either early boundary; the defect is reachable only when the global descriptor pool is within `threads` entries of exhaustion or extent provisioning fails.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `CleanupRecord::revoke` returns `Result<usize, TaskError>`, and `release_task_arena` returns the arena's `slot_len` before clearing it | The count is the authoritative number of CSlots returned to the pool |
| 2 | `reclaim` and the construction-body branch add it; the two early branches use `if let Err(...)` | Two of four revoke sites drop a value the other two treat as load-bearing |
| 3 | `main.rs:1541` prints `tasks.reclaimed_slots()` in the `SLIME_ROOT READY` marker, which `check-sel4-root-boot.py:588` cross-checks against per-task cleanup slot counts | An under-count is observable at the gate, once a composition reaches an early failure |
| 4 | The three failure sites had drifted into three copies of one unwind sequence | A single helper removes the class of defect, not just the instance |

## Root cause

`TaskTable::create` grew its two early failure branches separately from the
construction-body branch, and each copy re-derived the unwind by hand. The
branch bodies were identical except for the accounting line the first two
omitted, so nothing structural forced the ledger and the pool to agree: the
allocator's `release_task_arena` freed the slots unconditionally while only
some callers recorded it.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Construction unwind | Added `TaskTable::unwind_construction`, which builds the cleanup record, revokes, charges `reclaimed_slots` on success, and retains the record for `retry_failed_construction` on failure | Every pre-publication revoke has exactly one accounting path |
| Failure branches | Routed the descriptor preflight, `provision_private_backing`, and construction-body failures through that helper | A released CSlot is reusable and recorded together, never one without the other |
| Host seam | Added `ObjectAllocator::arena_owning_slots_for_test`, registering an arena's identity and slot ownership without the static-extent retype `begin_task_arena` needs a live kernel for | Host regressions exercise the production accounting body |
| Host test count | Corrected `expected` from 233 to 235 | The asserted count matches the suite; see Decisions |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| An unwind path drops its released slot count | `just test_sel4_root` — `every_construction_unwind_charges_the_slots_the_allocator_returned` | `reclaimed_slots()` diverges from the sum the allocator returned |
| A failed revoke silently loses its retry record | same test's `has_failed_construction` assertion | A completed revoke leaves a record the table would retry |
| The forced construction-unwind boot path regresses | `just sel4_reclamation_check` | `[init] reclamation construction unwind returned` missing, or terminal allocator accounting drifts from the quiescent snapshot |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test_sel4_root` | 235/235 passed across 19 modules | Direct |
| Mutation: `unwind_construction` reverted to discarding the count | `every_construction_unwind_charges_the_slots_the_allocator_returned` aborts on its assertion; restored source passes | Direct |
| `just sel4_reclamation_check` | Booted `sel4-reclamation-unwind` on AArch64 QEMU; construction unwind, exit, fault reuse, and terminal accounting markers observed in order, with `reusable_private_ram` nonzero | Direct |
| `just sel4_root_boot_check` | Ordered generation, timer, task, IPC, fault, cleanup, and ready markers observed | Direct |
| `just system_image_closure_check` | Contracts, resolution, identity, isolation, and bytes verified after regenerating every closure and re-blessing 48 test-run records | Direct |
| `just contracts_check` | Passed | Direct |
| `just fmt_check_all`, `just lint_all` | Passed with warnings denied | Direct |

## Decisions

- Decision: extract one `unwind_construction` helper rather than adding the accumulation line to each branch.
- Rationale: the three sites were already identical copies of one sequence, and the defect was precisely that one copy diverged. Three corrected copies preserve the conditions for the next divergence.
- Rejected alternative: charging inside `CleanupRecord::revoke`. The record does not own the table, and `retry_failed_construction`'s deferred charge would then be double-counted or need a second suppression path.

- Decision: correct `expected` in `test_sel4_root` from 233 to 235 as part of this change.
- Rationale: the gate was already failing at branch head `af6c84d6`. `af19537d` added two host tests (`runtime_capacity_rejects_each_reservation_shortfall` and `failed_large_extent_revoke_retains_ownership_then_reaches_base_page_quota`) but raised the asserted count by one, so `test_sel4_root` reported `ran 234 tests, expected 233` before this change. Leaving it red would mean this entry's own primary gate could not run.
- Rejected alternative: fixing the drift in a separate commit. The count assertion is a single line whose correct value depends on this change's new test; splitting it would leave one of the two commits with a knowingly red gate.

## Open risks and follow-ups

- [ ] `devlog/2026-09-11-smp-comment-and-capacity-merge/index.md` records `just test_sel4_root` at 233/233 for commit `2b3c03a9`. That observation is accurate for its baseline's asserted count but the suite ran 234 cases there; the discrepancy is the drift corrected here, not a second defect.
- [ ] No composition in the corpus fails at the descriptor preflight, so the corrected accounting on that specific branch is proven by host measurement rather than QEMU evidence. The construction-body branch, which shares the helper, is QEMU-exercised by `sel4_reclamation_check`.

## Artifacts and provenance

- Focused report: none; the change is local to `slime-root/src/task.rs`.
- Raw transcript: none retained; all gate output is reproducible from the commands in Verification.
- Serial/debugger/model output: `just sel4_reclamation_check` and `just sel4_root_boot_check` serial transcripts, reproducible from the pinned `qemu-arm-virt` profile.
- Related work item: [MEM-ARENAS](../../.tasks/items/01a07a2d-003d-77fc-9df8-dda85ed9a083.md)
