# C10.1: a task-private growable region, and the two accounting inverses it needed

| Field | Value |
|---|---|
| Date | 2026-08-23 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/syscall-abi/v1/schema.zt`, `docs/syscall-abi.md`, generated `components/proto/src/syscall_abi.rs` and its pin test; new `slime-root/src/private_memory.rs`; `slime-root/src/{child_vspace,object_allocator,task,ipc,main}.rs`; `slime-root/child/src/main.rs`; `scripts/check/{check-sel4-root-boot,check-sel4-gate-controls}.py`; `Justfile` |
| Roadmap | C10.1, C10, C10.2, C7.3, B9, B23 |
| Gates | `just sel4_root_boot_check`, `just test_sel4_root`, `just sel4_gate_control_check` |
| Trigger | C10.1 was the next roadmap milestone whose dependencies were met: the backlog is empty, and every other open item is either an undecomposed parent (C9) or gated on physical hardware this environment does not have (M5.7 Framework, P4/RP3 Raspberry Pi 5) |
| Baseline | A component's working memory was fixed at build time — stack from the `SLIMECME` header, `.data`/`.bss` from the linked ELF, no `GlobalAlloc` in `slime-rt` and no operation yielding a page — so every buffer was sized for its worst case in every generation carrying that component |

## Summary

Components can now grow one task-private region on demand. The root reserves a
fixed 2 MiB window per child at spawn — address space and translation tables
only — and backs it page by page through one new operation,
`LIFECYCLE PRIVATE MEMORY GROW` (label 43), which answers the page count before
the growth plus the window base. Growth is all-or-nothing, every page is
freshly zeroed and mapped user/read-write/execute-never, the base never moves,
and every page returns when the task dies.

Two pre-existing accounting asymmetries had to be closed first, and neither was
visible until something allocated from a task arena *while the task ran* —
C10.1 is the first such caller. `CleanupRecord.slots` was a construction-time
snapshot that `just sel4_root_boot_check`'s conservation arm proved had diverged
from what the arena actually returned; and `ArenaRecord::push_slot` had no
inverse at all, so a growth that failed part way could not give its CSlots back.
The review found the second one twice: once in the unwind loop, and again on the
sub-path where the retype succeeds and the *mapping* fails, where the frame was
never even recorded as taken.

The generation-declared budget is deliberately **not** here — that is C10.2. So
every declared instance and every spawned child sits at deny-by-default zero,
and the live evidence runs on the root's own embedded fixture child against a
compiled-in four-page ceiling, on exactly the terms `main.rs` already records
for the C7 shared-buffer phase's `SHARED_QUOTA`.

## Changes

| Area | Change | Established boundary |
|---|---|---|
| `contracts/syscall-abi/v1` | `operation "lifecycle" "PRIVATE_MEMORY_GROW" 43` | One operation, declared in the contract both crates generate from; the primary is the previous page count, the auxiliary the window base |
| `docs/syscall-abi.md` | Label 43's operands, result convention, five refusal causes, and the authority argument | `contracts_check` fails when a declared label is undocumented, so the doc could not lag the contract |
| `components/proto/tests/syscall_abi.rs` | Label list 29 → 30 entries | The pin test passed *because* 43 was absent from its list — the exact silent drift its own header says it exists to refuse |
| `slime-root/src/private_memory.rs` (new) | `Region` (base, reservation, quota, pages), `Table` (root-wide total and grant tallies), `GrowError`'s five causes, `arena_reservation`, `back_page`, `unwind` | The root tracks a page count and never an allocation; allocation policy stays in `slime-rt` |
| `slime-root/src/child_vspace.rs` | `private_window` places the window a guard granule above the thread pages, aligned up to its own 2 MiB span; `thread_mapped_span` now spans it; `ChildVSpace::private_base` carries it | One arithmetic decides where the window is, and the existing table mapper and arena planner both already cover it — so a growth needs leaf frames only and can never want a table nothing allocated |
| `slime-root/src/object_allocator.rs` | `MAX_TASK_SLOTS` += `MAX_REGION_PAGES`; new `ArenaRecord::release_last` and `ObjectAllocator::release_last_in` | An arena's slot table gains the inverse it lacked, refusing anything but its top entry because a bump allocator can only rewind at its watermark |
| `slime-root/src/task.rs` | `Task::private_memory`, `TaskTable::private`, `create`'s `private_memory_pages`, `grow_private_memory`, `reclaim` | The region lives on the task record, so it is reclaimed atomically with it and there is no second table to keep in step |
| `slime-root/src/task.rs` | `reclaim` reports the count the arena revoke returned, not `CleanupRecord.slots` | The per-task and aggregate markers state the same fact again |
| `slime-root/src/ipc.rs` | Label 43 routes to `SERVICE_LIFECYCLE`; 43 left the routes-nowhere control and 44 took its place | A private heap is a property of being a task, so it is gated on the one service every launched instance declares |
| `slime-root/src/main.rs` | Both dispatchers serve the operation; `SLIME_MEM` records for each growth, each refusal with its named cause, the child's report, the adjudication, and the teardown; `MAX_SERVICE_ITERATIONS` 8 → 16 | The root adjudicates from its own page accounting, which the child cannot see and cannot forge |
| `slime-root/child/src/main.rs` | `private_memory_phase`: a size query, two growths, a zero read over both pages, a pattern that must survive the second growth, the quota refusal, and the intact-region recheck | Every access goes through the base the root *answered*, so a fixture cannot assert its own copy of the loader's arithmetic |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Growth stops being bounded by the declared quota | `just sel4_root_boot_check` | `SLIME_MEM FAIL child reported 0x1f, missing 0x60 of 0x7f` — observed by injection |
| A growth relocates the base, invalidating live native pointers | `just sel4_root_boot_check` | `SLIME_MEM FAIL child reported 0x2f, missing 0x50 of 0x7f` — observed by injection |
| A page is charged but never returned | `just sel4_root_boot_check` | `SLIME_MEM FAIL teardown left pages=N grown=… reclaimed=…` |
| The root reports one base and maps another | `just sel4_root_boot_check` | `the private-memory window base moved across records: [...]` — the check folds in the addresses the child actually dereferenced, not only the ones it was told |
| A query or a refusal silently charges a page | `just sel4_root_boot_check` | `SLIME_MEM FAIL N growth grant(s), expected exactly 2` |
| The new markers are deleted or weakened | `just sel4_gate_control_check` | Pinned marker count 43 → 58; the control rejects a gate whose table shrank |
| An out-of-order unwind mis-accounts an arena | `just test_sel4_root` | `an_arena_slot_table_shrinks_only_from_its_top` — observed to fail with the LIFO guard removed |
| A module loses coverage | `just test_sel4_root` | Pinned count 131 → 146 |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_root_boot_check` | Passed. 58 ordered markers, 15 of them C10.1's. Observed: `base=0x400000 quota=4`, growths `0→2→4`, `survived=0x4d454d5f42415345`, `cause=quota detail=QuotaExceeded { pages: 4, delta: 1, quota: 4 }`, child `result=-5` and still alive, `flags=0x7f`, `enforced quota=4 pages=4 grants=2 grown=4 reclaimed=0`, `teardown grown=4 reclaimed=4 pages=0` | Direct |
| Injection: quota bound deleted from `Region::admit` | Gate failed, naming `missing 0x60 of 0x7f` — the quota-refused and refusal-had-no-effect observations | Direct |
| Injection: growth re-backs from the base (`vaddr = base + count * GRANULE`) instead of the tail | Gate failed, naming `missing 0x50 of 0x7f` — the surviving pattern | Direct |
| Injection: `release_last`'s `*recorded == slot` guard removed | `an_arena_slot_table_shrinks_only_from_its_top` failed | Direct |
| Injection: label 43 renumbered to 99 in the pin array | `operation_labels_are_frozen` failed | Direct |
| `just test_sel4_root` | 146/146 across 16 modules | Direct |
| `just sel4_gate_control_check` | 33 gates reject 1295 mutated transcripts and layouts (was 1227 before the milestone's two marker additions) | Direct |
| `just sel4_boot_layout_check` | 26 plane layouts match their frozen fixtures unchanged — the window is address space, so no component's resolved capability layout moved | Direct |
| `just generation_check` | Passed, including two isolated builds producing byte-identical `generation.bin` and `boot-store.bin` | Direct |
| `just contracts_check` | Passed; documents all 30 declared operations | Direct |
| `just test`, `just test_host` | Passed | Direct |
| `just sel4_sample_check`, `sel4_loan_check`, `sel4_spawn_check`, `sel4_reclamation_check` | Passed — the shared-buffer, loan, spawn, and arena-reuse paths under the widened `MAX_TASK_SLOTS` | Direct |
| `just sel4_stress_check`, `just sel4_traffic_check` | Passed — the 23-instance ceiling graph and 19 concurrent participants, the widest arena and slot pressure the repository has | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | Passed | Direct |
| Frame exhaustion part-way through a multi-page growth returning every frame | **Not observed.** The unwind path is reasoned and its bookkeeping half is unit-tested, but no gate drives a real mid-growth retype or map failure — see *Open risks* | Inherited from code reading |

## Decisions

- Decision: reserve the window's *address space and tables* at spawn, but allocate its frames on demand.
- Rationale: `child_vspace::thread_mapped_span` already spans the window, so `map_intermediate_tables` builds its tables at construction and `ChildImage::vspace_arena_plan` charges them in the same arithmetic. A growth therefore allocates leaf frames only and can never need a table nothing planned. Charging the tables a second time in `private_memory` would size every arena for objects it allocates once, and — worse — let the two sides disagree.
- Rejected alternative: plan the frames at spawn too. That charges every component for memory it may never ask for.

- Decision: size the reservation to exactly one AArch64 level-2 span (512 pages, 2 MiB) and align the base to it.
- Rationale: the window then costs one extra leaf page table per child and no more. An unaligned base straddles two, which `a_two_mib_window_spans_exactly_one_leaf_table`'s successor test pins.
- Rejected alternative: a granule-aligned base immediately above the guard. Cheaper to compute, twice the table cost, and the arithmetic is the same length either way.

- Decision: pass quota `0` to every declared instance and every spawned child; run C10.1's live evidence on the root's embedded fixture.
- Rationale: the authority to grow is a generation-declared budget resource, and adding one is C10.2's exit condition. Answering a nonzero default here would be ambient authority — a component holding pages no manifest named. The fixture is an ELF the root embeds at compile time, so no generation resource can name it, which is the identical situation `SHARED_QUOTA` documents for the C7 shared-buffer phase.
- Rejected alternative: grant a default quota to declared instances so the mechanism is exercised on a component plane. It would make C10.2 a *narrowing* of live authority rather than the introduction of it.

- Decision: answer the window base in the reply's auxiliary word rather than letting the caller derive it.
- Rationale: the root chose the base; a component recomputing the loader's arithmetic is the compile-time coupling B70 removed everywhere else. It also makes the gate's base-agreement check meaningful — the root's record, the answered base, and the address the child dereferenced are three independent sets that must agree.

- Decision: make `release_last_in` last-allocated-only and *checked*, rather than documenting the precondition.
- Rationale: an arena is a bump allocator, so only the object at the watermark can be rewound; releasing from the middle either strands bytes or later hands out an overlapping region. A refusal makes an out-of-order unwind a caught bug instead of silent corruption. The rewind is also conservative by construction: `plan_allocation` aligns each object's start *up*, so subtracting the size recovers the aligned start and any padding stays consumed.

- Decision: `back_page` returns its own frame when the mapping fails.
- Rationale: found by review. The retype has already charged an arena slot by the time `frame_map` runs, and `grow` only unwinds frames it was handed back — so a frame stranded there is invisible to `unwind`, and it is the *likelier* failure (a bad vaddr fails at the map, not the retype). It also compounds: the stranded slot sits at the arena's top, so the next `release_last_in` names a different slot, is refused by its own guard, and silently disables slot recovery for that arena.

## Open risks and follow-ups

- [ ] Frame exhaustion mid-growth is unobserved. The milestone's required check "frame exhaustion part-way through a multi-page growth returns every frame it had taken, observable as an unchanged free-frame count" is satisfied by construction and by `an_arena_slot_table_shrinks_only_from_its_top`, but no gate injects a real retype or map failure. Closing it needs a fault-injection seam of the kind B61 records this repository lacking for object invocations. C10.4's spawn/exit frame-drift measurement is the natural place.
- [ ] `MAX_TOTAL_PAGES` (2048, 8 MiB) is never reached by any current composition, so the root-wide ceiling's refusal arm is unit-tested but not observed on a boot. It becomes reachable once C10.2 grants real quotas to several components at once.
- [ ] The private-memory quota is a `usize` parameter threaded through `TaskTable::create`. C10.2 replaces its source with a generation resource; the parameter should not outlive that as a caller-chosen number.
- [ ] `MAX_TASK_SLOTS` now reserves `MAX_REGION_PAGES` per task whether or not the generation grants any quota, because a slot table sized to one manifest would refuse a later one at its first growth. That is 512 slots of headroom per arena against a 262144-slot pool; if arena count ever approaches that pool, this becomes the term to revisit.
- [ ] `just sel4_root_boot_check`'s slot-conservation arm caught the `CleanupRecord.slots` divergence, but the fix means the marker now reports the revoke's count for *every* task. No other gate pinned the old snapshot semantics; if one ever did, it would have been asserting a prediction rather than an outcome.

## Artifacts and provenance

- Focused report: none; every finding and decision is recorded in the tables above with its source cross-reference.
- Raw transcript: not retained. Every result in *Verification* is reproducible from the named `just` target, and each injection is described precisely enough to repeat (the mutation, the file, and the observed failure string).
- Serial/debugger/model output: not retained as a sibling; the load-bearing marker lines are quoted verbatim in *Verification* and are reproducible with `just sel4_root_boot_check`.
- Review: two rounds of a read-only reviewer pass over the uncommitted diff. Round 1 returned four findings (the unwind slot leak, the `reclaim` ordering, the test-count arithmetic, and the gate's base vacuity); round 2 confirmed those closed and returned two more (the `back_page` sub-path leak, and one new test that asserted its own inlined mutations rather than the function). All six are applied and recorded in *Decisions* and *Changes*.
- Related roadmap item: [C10.1](../../roadmap/02-core-runtime.md), [C10](../../roadmap/02-core-runtime.md)
- Predecessors: [`devlog/2026-07-28-c10-private-component-memory/`](../2026-07-28-c10-private-component-memory/index.md) (the design decision this implements), [`devlog/2026-08-22-roadmap-retired-kernel-audit/`](../2026-08-22-roadmap-retired-kernel-audit/index.md) (which rewrote C10.1 off the retired kernel's vocabulary)
