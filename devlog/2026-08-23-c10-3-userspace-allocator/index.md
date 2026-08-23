# C10.3: a second allocator, and a reuse assertion that was quoting the code under test

| Field | Value |
|---|---|
| Date | 2026-08-23 |
| Kind | Change |
| Status | Verified |
| Scope | New `components/runtime/src/private_heap.rs` and `private_heap_probe.rs`; `components/runtime/src/{lib.rs,runtime.rs}`; `components/runtime/Cargo.toml`; new `components/bins/private-heap-probe/`; `contracts/generation/v1/fixtures/sel4-private-memory.zti`; `scripts/build/build-generation.py`; `scripts/check/{check-sel4-private-memory-plane,check-component-crate-split,check-sel4-gate-controls}.py`; `Justfile`; `Cargo.toml` |
| Roadmap | C10.3, C10, C10.1, C10.2, C10.4, C7.3, CP3, B5, B23, B63 |
| Gates | `just private_memory_check`, `just component_crate_split_check`, `just sel4_gate_control_check`, `just lint_all`, `just test_sel4_root`, `just contracts_check`, `just generation_check` |
| Trigger | C10.3 was the next roadmap milestone with met dependencies: the backlog is empty, C10.2 closed the same day, and every other open item is an undecomposed parent (C9) or gated on physical hardware (M5.7, P4/RP3) |
| Baseline | C10.1 and C10.2 made a generation-declared page quota the live ceiling, but the only way to reach it was `private_memory_grow`, which hands back raw pages. No component could allocate a `Vec` inside its own quota; the C10.2 probe measured its ceiling by growing one page at a time |

## Summary

A component now allocates through ordinary Rust collections inside its
generation-declared ceiling. `components/runtime/src/private_heap.rs` is a
`GlobalAlloc` over the task-private region: first-fit over an address-ordered
free list, coalescing on both boundaries when a block returns, with a growth
appended at the tail so it merges into the trailing free block instead of
fragmenting it.

Two decisions shaped it. It is a **second** allocator rather than a
configurable one, because `#[global_allocator]` is a single symbol per link and
the choice belongs to the component: CP3's store-plane bump allocator is right
for a component that opens a partition, indexes it, answers a bounded number of
requests and exits, and a free list there is pure cost. And **batching is
userspace policy over a per-page ABI** — `GROWTH_PAGES` is four granules and
`grow` retries at the exact size when a batch is refused, so a component with
three pages left is never denied an allocation its ceiling can still serve.

The interesting failure was in my own evidence rather than in the allocator. The
first version of the gate asserted reuse from the probe's `reuse_growths`
field — a number produced by the allocator under test. The reviewer named it
precisely: an allocator that lost its freed spans and grew again while
under-counting itself would report `reuse_growths=0` and pass, and the
docstring already claimed the opposite ("both halves are read from the root's
records"). The probe now brackets its reuse phase with a console line and the
gate counts the root's own growth records inside that window.

## Changes

| Area | Change | Established boundary |
|---|---|---|
| `components/runtime/src/private_heap.rs` (new) | `Header`/`FreeBlock`, `Heap::{start,release,take,grow}`, `PrivateHeap` + lock, `private_heap_stats` | The declared quota is reachable by `Vec`/`Box`/`String`; growth happens only when the free list cannot serve a request |
| `components/runtime/src/private_heap_probe.rs` (new) | Three-phase startup self-check in the C7 shared-buffer probe's shape; emits a reuse-phase boundary line | Ships beside the allocator, not in `components/lib`: it calls `alloc`, and the split gate pins that the shared library never names an allocator feature |
| `components/runtime/src/lib.rs`, `Cargo.toml` | `private-heap` feature; `compile_error!` on `heap` + `private-heap` | Two allocators that cannot coexist fail at compile time rather than at link |
| `components/bins/private-heap-probe/` (new) | One image, two declared outcomes; discovers which it is by asking its own allocator for a base | A build flag would pass against a root that stopped honouring declarations |
| `contracts/generation/v1/fixtures/sel4-private-memory.zti` | One new executable, two root-autostart instances, one at 24 pages and one omitted | Same plane as C10.2: both milestones assert properties of the same declared budget |
| `scripts/build/build-generation.py` | `PRIVATE_HEAP_COMPONENTS` and a third build group | Feature unification here does not over-link, it fails to compile |
| `scripts/check/check-component-crate-split.py` | Two independent allocator groups; refuses a crate declaring both | A component gaining an allocator must move groups in the same change |
| `scripts/check/check-sel4-private-memory-plane.py` | Two new chains, the reuse boundary as a required marker, `check_growth_was_batched_and_reused`, a relaxed charge bound | Declared-vs-charged, batching, and reuse — each measured against the root's records |
| `Justfile` | `lint_sel4_root` split into three `cargo metadata`-derived clippy invocations, `slime-rt` in every group | A single pass enables both allocators and dies before reporting a lint |

### The charge check had to be relaxed, deliberately

C10.2's `check_only_declared_pages_were_charged` required `charged ==
declared`. That is exactly right for a probe that grows one page at a time until
refused — it necessarily lands on its ceiling. It is wrong for an allocator,
which takes what its collections need: 22 of 24 here. Requiring equality would
make the gate fail whenever the batching policy changed, which is the one thing
C10.3 deliberately left in userspace. The bound is now "never above the ceiling,
and never zero for a holder that declared one", the second half being what keeps
the relaxation from admitting a milestone that silently does nothing. The C10.2
half is unweakened: its probes still land exactly on their ceilings, and
`check_measured_ceiling` still asserts that equality for them.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The declared ceiling stops being reachable by ordinary Rust | `just private_memory_check` | missing `private-heap quota live pages=… reuse_growths=0 leaked=0` |
| The allocator asks per allocation | `just private_memory_check` | `a growth of one page ([1, 4, 5, 9]), so the allocator is asking per allocation` — observed by injection |
| Freed memory stops being reused | `just private_memory_check` | `the root served ['2'] more page(s) during the reuse phase` — observed by injection |
| The probe stops bracketing its reuse phase, emptying that window | `just private_memory_check` | `missing marker: \[private-heap-probe\] private-heap reuse phase begins` — observed by injection |
| Exhaustion becomes a fault, hang, or silent truncation | `just private_memory_check` | `private-heap exhausted`/`private-heap failed` failure markers, or the missing post-refusal report |
| A refusal poisons the heap | the probe itself | `FAIL refusal poisoned the heap` / `FAIL refused request charged a page` |
| A holder the budget omits allocates anything | `just private_memory_check` | `FAIL allocated with no declared quota`, and the per-holder charge bound |
| A root installs no ceiling at all for a declared instance | `just private_memory_check` | `missing marker: SLIME_MEM quota … instance=private-heap-denied` — observed by injection |
| Two allocators reach one link | `just component_crate_split_check`, `slime-rt/lib.rs` | group mismatch by name, or a `compile_error!` |
| The allocator's unsafe code stops being linted | `just lint_all` / `just lint_pedantic` | `slime-rt` is named in every group; a path dependency cargo merely checks is not linted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just private_memory_check` | Passed. 19 markers across 6 causal chains. Granted holder: growths `4+4+5+9 = 22` of 24, all four before the reuse boundary, `reuse_growths=0 leaked=0`, then `cause=quota detail=QuotaExceeded { pages: 22, delta: 257, quota: 24 }` and a post-refusal report. Omitted holder: `denied pages=0 growths=0 refused=1` | Direct |
| Injection: a growth served inside the reuse window, both accounts kept consistent | Refused: `the root served ['2'] more page(s) during the reuse phase` — the arm bites alone, not via the count-agreement arm | Direct |
| Injection: only the *first* growth reduced to one page | Refused: `a growth of one page ([1, 4, 5, 9])` — which `max(served) < 2` would have accepted | Direct |
| Injection: reuse boundary line removed | Refused twice — by the marker contract and by the reuse arm | Direct |
| Injection: either new instance's `SLIME_MEM quota` record removed | Refused by name from chain 1 | Direct |
| Injection: denied holder charged the granted holder's pages | Refused: `private-heap-denied: the root charged 9 page(s) against a declared quota of 0` | Direct |
| Injection: granted holder charged nothing | Refused: `the allocator never grew its region` | Direct |
| `just component_crate_split_check` | Passed: 54 crates, 6 store allocator, 1 private-region, each matching the builder's group | Direct |
| `just sel4_gate_control_check` | 34 gates reject 1331 mutated transcripts and layouts (1318 before C10.2, 1328 mid-milestone) | Direct |
| `just lint_all`, `fmt_check_all`, `ruff`, `typos` | Passed | Direct |
| `private_heap.rs` under `undocumented_unsafe_blocks` + `cast_possible_truncation` | Clean after the review fixes. `lint_pedantic` as a whole remains red at 134 pre-existing errors in `deps/` and `boot-contracts`, unchanged by this milestone | Direct |
| `just test_sel4_root`, `test_host`, `contracts_check`, `generation_check`, `sel4_root_boot_check`, `sel4_boot_layout_check`, `sel4_component_graph_check` | Passed; 149/149 host tests, 26 boot layouts unchanged, byte-identical double build | Direct |
| "A zero-quota component is byte-identical to its pre-C10 build" | Structural, not measured: `private-heap` is opt-in per crate and the only crate declaring it is new, so no pre-existing image changes | Inherited from the feature graph |

## Decisions

- Decision: a second `GlobalAlloc` behind a mutually exclusive feature, not a widened `BumpHeap`.
- Rationale: `#[global_allocator]` is one symbol per link, so the choice is per component and cannot be a runtime switch. The two allocators also want opposite things: the bump allocator's module comment records that its allocation shape is open-index-answer-exit, where a free list "would only add a failure mode". A component bound by a small declared ceiling is the inverse — reuse is the only way it runs past its first burst.
- Rejected alternative: make `BumpHeap` fall back to the private region when its `.bss` arena is exhausted. One allocator, no feature, and a component whose memory comes from two places with different lifetimes and one accounting.

- Decision: batching in userspace, the ABI in single pages.
- Rationale: the milestone asked for this explicitly, and the reason is worth recording. `GROWTH_PAGES` is a tradeoff between syscalls and slack that depends on a page profile the contract should not know about. Keeping it here means a later profile change edits one constant and no fixture. The retry-at-exact-size arm is what stops batching from becoming a lowered ceiling: a component with three pages left, refused a four-page batch, must still get its three.

- Decision: the probe discovers which instance it is by asking its allocator for a base.
- Rationale: C10.2's precedent, for C10.2's reason. A build flag, or a manifest read, would let the probe pass against a root that had stopped honouring declarations, because both sides would be quoting the same source.

- Decision: assert reuse and batching from the root's records, not the probe's counters.
- Rationale: found by the review, and the sharpest finding in it. My `check_growth_was_batched_and_reused` docstring claimed both halves came from the root, but the reuse phase ran *before* the report line the window was measured against, so the window was empty and the only real evidence was `reuse_growths=0` inside the component's own line — a number the allocator under test computes. The probe now emits a boundary line before phase 3 and the gate counts root records between it and the report. The component can bracket a phase but cannot fake which side of a bracket the root's own records fall on.

- Decision: test `min(served)`, not `max(served)`, for batching.
- Rationale: also from the review. A purely demand-driven allocator would still show a large growth for the probe's biggest single reallocation, so the maximum asserts nothing; its *first* growth is one page for the first small allocation. The minimum discriminates and is independent of whatever `GROWTH_PAGES` happens to be.

- Decision: `grow` derives the appended span from the root's reply, not from its own counter.
- Rationale: the review found that `appended = base + self.backed * GRANULE` silently assumed this allocator was the region's only user, while `private_memory_grow` is a public component API that `private-memory-probe` already calls directly. A lagging counter would name an already-backed range, put it on the free list, and hand a caller a pointer into live memory. The suggested fix compared the two counts; taking the reply's `previous.pages` instead makes the divergence unrepresentable — a page grown behind the allocator's back simply never joins its list, and `backed` re-anchors on the next growth.

- Decision: yield rather than spin while the heap lock is held.
- Rationale: the review's third finding, and the one I would not have found. The lock is held across two blocking root IPCs, the plane runs one core, and `slime-root/src/task.rs` admits a per-thread worker priority independent of the main thread's — so a worker the generation gave the higher priority would spin forever while the runnable holder is never scheduled. That is precisely the hang the module's header promises cannot happen. `slime_rt::yield_now` is a bare `seL4_Yield`, allocates nothing, and moves the yielding thread to its queue's tail.

- Decision: `slime-rt` is named in every clippy group.
- Rationale: the second review finding, and a self-inflicted one. Splitting the component lint pass by allocator feature left `slime-rt` named only in the plain group, where both allocator modules are `cfg`'d out — so the ~450 new lines of unsafe pointer arithmetic were linted by nothing at all, in the one place in the tree that most needs `undocumented_unsafe_blocks`. A path dependency cargo checks is not a package cargo lints.

- Decision: safety comments state the concrete invariant, not "as in `free_bytes`".
- Rationale: the round-2 note. The back-references were on exactly the blocks carrying the module's alignment and minimum-size induction, so the invariant lived only in prose the lint cannot check, where a future edit breaks it silently. Each now names what it relies on: the block start is `ALIGN`-aligned, the span is at least `MIN_BLOCK`, and why.

## Open risks and follow-ups

- [ ] The lock is held across a blocking root IPC. Yielding fixes the priority-inversion livelock, but a component with a worker still serializes both threads' allocations behind a syscall. Splitting the growth request out of the critical section is the real fix and needs care: two threads must not both grow for the same shortfall.
- [ ] `dealloc` trusts the `Header` below the pointer. That is the `GlobalAlloc` contract and every allocator does it, but a component that overruns an allocation by a few bytes corrupts the *next* allocation's header rather than faulting, and the resulting free-list damage is silent. A guard word would make the overrun observable at the cost of `ALIGN` bytes per allocation; worth considering when a real workload lands.
- [ ] Only one plane exercises the allocator, and its 24-page quota is small. Fragmentation behaviour under a long-lived mixed-size workload — the case first-fit plus coalescing exists for — is untested; the probe's phases are deliberately simple enough to assert exactly.
- [ ] "Byte-identical to its pre-C10 build" is asserted from the feature graph, not measured. A gate comparing an untouched component's ELF digest across the change would make it an observation. The generation's own object digests already contain the material for that.
- [ ] `lint_pedantic` remains red at 134 pre-existing errors, so it cannot gate the new module even though it now reaches it. `private_heap.rs` is clean under both advisory lints today; nothing keeps it that way.
- [ ] `GROWTH_PAGES` is one constant for every component. A component whose declared quota is smaller than a batch relies entirely on the retry-at-exact-size arm; nothing warns a fixture author that a quota below four pages makes the batching policy inert.

## Artifacts and provenance

- Focused report: none; every finding and decision is recorded above with its source cross-reference.
- Raw transcript: not retained. Every result in *Verification* is reproducible from the named `just` target, and each injection is described precisely enough to repeat (the mutation and the observed refusal string).
- Serial output: not retained as a sibling; the load-bearing marker lines are quoted verbatim in *Verification* and reproducible with `just private_memory_check`.
- Review: two rounds of a read-only reviewer pass over the uncommitted diff. Round 1 returned "incorrect" with seven findings — three P2 defects in the implementation (`grow` trusting its own counter, the spin lock across blocking IPC, the lint gap) and four in the evidence (reuse asserted from the probe's self-report, `max` instead of `min` for batching, the terminal regex missing the new probe's FAIL, and two instances absent from the ceiling chain). All seven are applied and recorded in *Decisions*. Round 2 returned "correct", confirmed both substantive fixes were correct rather than merely different, and added one non-blocking note — the back-referenced safety comments — which is also applied.
- Related roadmap items: [C10.3](../../roadmap/02-core-runtime.md), [C10](../../roadmap/02-core-runtime.md)
- Predecessors: [`devlog/2026-08-23-c10-2-private-memory-budget/`](../2026-08-23-c10-2-private-memory-budget/index.md) (the budget this allocator spends), [`devlog/2026-08-23-c10-1-private-memory-mechanism/`](../2026-08-23-c10-1-private-memory-mechanism/index.md) (the growth operation underneath it), [`devlog/2026-07-28-c10-private-component-memory/`](../2026-07-28-c10-private-component-memory/index.md) (the design decision)
