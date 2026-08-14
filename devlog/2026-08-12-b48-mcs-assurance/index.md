# B48 — defer AArch64 MCS until its proof is complete

| Field | Value |
|---|---|
| Date | 2026-08-12 |
| Kind | Decision |
| Status | Proposed |
| Scope | seL4 AArch64 scheduling configuration and generation schedule claims |
| Roadmap | B48 |
| Gates | none |
| Trigger | B48 required an explicit assurance decision before enabling MCS |
| Baseline | QEMU AArch64 uses the non-MCS upstream seL4 configuration and enforces declared per-thread priority only |

## Summary

Slime OS will not enable seL4 MCS on AArch64 while upstream records its functional-correctness proof as in progress. The selected kernel therefore enforces authenticated per-thread priority, but generation records keep budget and period at zero and do not claim timeout-fault delivery. This preserves the repository's upstream-seL4 assurance boundary instead of trading it silently for scheduling-context features.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Kernel configuration | Keep `KernelIsMCS OFF` with the assurance rationale beside the setting | The product does not imply that an unverified AArch64 MCS kernel satisfies the same assurance claim as the selected non-MCS configuration |
| Generation builder | Continue emitting zero `budget_us` and `period_us` values | Persisted schedule data states only what the selected kernel can enforce |
| Backlog disposition | Explicitly defer the MCS-only clauses rather than restoring the single-priority fallback | B48 can distinguish completed priority/preemption work from unavailable MCS guarantees |

## Decisions

- Decision: Defer AArch64 MCS, scheduling contexts, budget/period enforcement, and timeout-fault endpoints.
- Rationale: Upstream `deps/sel4/CAVEATS.md` states that functional-correctness proofs for MCS on AArch64 are in progress. Enabling it would weaken the project's assurance claim. Declared per-thread priority already removes the unsafe all-children-at-one-priority fallback and the sample plane directly observes preemption.
- Rejected alternative: Enable MCS because the implementation is supported and generally stable. Runtime support is not equivalent to the proof boundary this repository claims.

## Open risks and follow-ups

- [ ] Revisit when upstream ships a functional-correctness proof for the selected AArch64 MCS configuration; then add scheduling-context allocation, authenticated nonzero budget/period admission, passive-server donation, timeout endpoints, and a QEMU gate that exhausts a budget and observes the declared timeout handler.
- [ ] The current starvation proof is single-core (`-smp 1`); SMP scheduling remains outside the observed claim.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none.
- Serial/debugger/model output: `just sel4_sample_check` and `just sel4_qos_check` results are recorded in the B48 priority entries.
- Related roadmap item: `roadmap/00-backlog.md` B48.
