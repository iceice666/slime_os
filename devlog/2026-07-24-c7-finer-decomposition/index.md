# C7 — Finer shared-sample-plane decomposition

| Field | Value |
|---|---|
| Date | 2026-07-24 |
| Kind | Decision |
| Status | Proposed |
| Scope | `roadmap/02-core-runtime.md`, C7 sequencing and verification gates |
| Roadmap | C7 |
| Gates | `just sample_plane_check` |
| Trigger | C7.2–C7.4 still combined multiple independently risky state machines |
| Baseline | C7.1 complete; remaining work grouped into factory/accounting, lifecycle/descriptor, and final integration |

## Summary

The remaining C7 plan is split from three slices into six. Each new slice introduces one primary state surface—factory authority, quota accounting, mapping/sealing, loan ownership, descriptor validation, then integration—and owns a narrow QEMU verification target. C7.1 remains unchanged and the parent C7 gate now closes at C7.7.

## Observable symptom

- Expected: each milestone has one primary invariant, a narrow executable check, and an exit condition observable without completing the next slice.
- Observed: old C7.2 combined capability design, allocation, four quota classes, supervision accounting, and fault reclamation; old C7.3 combined mapping, sealing, loan ownership, descriptor schema, and large-payload transfer.

## Changes

| Area | Change | Established invariant |
|---|---|---|
| C7.2 | Shared-buffer factory, existing-buffer authority, bounded allocation/release | Buffer identity and creation authority are independently testable |
| C7.3 | Generation budgets and supervision-subtree accounting | Resource ownership and quota isolation precede mapping/loan state |
| C7.4 | Map/unmap and irreversible read-only sealing | Page-table and mutability transitions have one gate |
| C7.5 | Loan/return and fault reclamation | Outstanding ownership and peer-failure cleanup have one gate |
| C7.6 | Versioned Zutai sample descriptor | Wire validation is isolated from final topology composition |
| C7.7 | Two-component integration and isolation | Parent C7 exit condition remains end-to-end |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Slice silently depends on unfinished later behavior | Per-slice exit condition and named `just` target | Exit condition cannot run at the stated dependency boundary |
| Resource state machines become one review unit again | C7.2–C7.6 each own one primary surface | A slice requires unrelated mapping, loan, or descriptor changes |
| Parent acceptance weakens during decomposition | `just sample_plane_check` remains C7.7-owned | Final test omits a quota class, peer death, unrelated channel, or retained-v2 boot |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Documentation consistency review | C7.1–C7.7 sequence, dependencies, checks, and cross-references aligned | Direct |
| Runtime tests | Not run; documentation-only design change | Direct |

## Decisions

- Decision: preserve completed C7.1 and number new work C7.2–C7.7 rather than renaming history.
- Rationale: stable completed milestone references; clean cutover for every unstarted slice.
- Decision: separate mapping/sealing, loan ownership, and descriptor validation.
- Rationale: these fail through different mechanisms—page tables, distributed lifecycle state, and wire decoding—and need independent proofs.
- Rejected alternative: keep C7.2–C7.4 and add internal checklists. Checklists do not provide independently closable gates or bounded review units.

## Open risks and follow-ups

- [ ] Each planned `just` target must be added with its implementation slice; no target exists yet.
- [ ] C7.3 must define the budget as a Zutai generation resource payload authenticated through v3's existing object table; no ad-hoc serialized fields or implicit v4 format change.

## Artifacts and provenance

- Related roadmap item: `roadmap/02-core-runtime.md` (C7)
- Related completed foundation: `devlog/2026-07-24-c7-1-generation-v3-u64-rights/`
