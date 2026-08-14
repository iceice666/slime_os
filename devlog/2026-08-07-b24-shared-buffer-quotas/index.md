# B24 — a shared-buffer quota was never released

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root/src/shared_buffer.rs`, `slime-root/src/main.rs`, `scripts/check/check-sel4-supervision-plane.py` |
| Roadmap | B24, B22, B16, P5.4.1 |
| Gates | `just sel4_supervision_check` |
| Trigger | P5.4.1's lifetime-vs-live bounds class audit, which opened B24 |
| Baseline | Nine seL4 gates passing; `MAX_CHARGE_HOLDERS = 96` never reached by any declared generation |

## Summary

`SharedBufferTable::quotas` had no free path. `declare_quota` reuses a slot only
for the same `HolderId`, `construct_child` keys it by task id, and
`TaskTable::next_id` never rewinds — so a graph that spawned and reaped
repeatedly presented a fresh holder every time and `MAX_CHARGE_HOLDERS` (96)
bounded the holders a boot could **ever** construct rather than those live at
once. This is the third instance of B16's defect shape, and the one B16's own
sweep implicitly cleared by naming `charges` (which is correct) without naming
its sibling one line below. Fixed by `release_quota`, called from
`reclaim_dead_task` after charge settlement. Observed on the existing
supervision plane: 38 holders constructed, `quotas=0` at teardown, and
`quotas=38` with the release disabled.

## Observable symptom

- Command: none before this change — the defect was latent, exactly as B16's
  and B22's were.
- Expected: a graph that constructs holders and reaps them leaves no ceiling
  bound to a task that no longer exists.
- Observed (fault injection, release disabled):
  `SLIME_GRAPH loans served=0 loans=0 mappings=0 regions=0 transit=0 orphans=0
  aliases=0 quotas=38` — thirty-eight ceilings retained for dead tasks.
- Exit/fault/serial evidence:
  [`fault-injection-no-release.log`](fault-injection-no-release.log).

The `quotas=` field is this change's own reporting. Before it, the condition
produced no output at all: the entries simply accumulated, and the boot that
eventually crossed 96 would have failed a spawn with `DestinationSlotsExhausted`
for a reason unrelated to anything the caller did.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | P5.4.1's class audit swept every bounded table in `slime-root/src` and found `quotas` (`shared_buffer.rs:502`) with no assignment of `None` anywhere | The third lifetime-bounded table. B16's sweep had named "the shared-buffer orphan and charge tables" as correct — `charges` is (freed at `:1782-1784`), `quotas` was not, and was not named |
| 2 | `quotas` is *keyed* per-task but *declared* per-component at boot (`main.rs:1256`) and per-task only on the spawn path (`main.rs:3092`) | Why B16's per-task sweep missed it, and why the class audit was the right instrument rather than another one-at-a-time find |
| 3 | A quota has exactly one holder, and that holder is a task | Unlike B22 and B16 this needs **no derived predicate**: when the task is gone the entry is unreachable, with no second place it can still be named from. Channels and termination records both needed a sweep because a capability can outlive or travel independently of the task; a quota cannot |
| 4 | The supervision plane already constructs 38 holders over one boot | The defect's shape is a spawn/reap loop, which that plane already is. No tenth image is needed — asserting on an existing gate is stronger, since it also proves the fix does not perturb what that gate already covers |
| 5 | 35 spawns cost 2321 root CSlots (~66 each) against 3457 available | **Crossing the 96 bound is unreachable.** CSlots are deliberately never returned (`task.rs:165-167`), so a boot exhausts them near 52 tasks. B24's exit condition as written — "constructs more than `MAX_CHARGE_HOLDERS` holders" — cannot be observed on this platform, and the condition is amended rather than the evidence stretched |

## Root cause

`declare_quota` (`shared_buffer.rs:574-587`) finds a slot by matching the holder
and otherwise takes a fresh `None`. Nothing ever wrote `None` back:
`commit_teardown` clears mappings, loans, and regions; `reclaim_holder` and
`advance_epoch` never mention `quotas`.

The violated invariant is the one `MAX_CHARGE_HOLDERS` is sized against. It is
`MAX_SHARED_BUFFERS + MAX_MAPPINGS` — a bound derived from what can be *held*,
which only makes sense as a bound on live holders. Measuring lifetime
constructions against it is measuring the wrong quantity, which is precisely
B16's and B22's root cause restated.

Not an innocent bystander: the constant is fine and the failure mode is a
bounded refusal rather than a silent drop. The defect is that the table had no
reclamation path at all.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `shared_buffer.rs` | `release_quota(HolderId) -> bool` drops the entry bound to one holder; `quota_count()` reports what remains | `MAX_CHARGE_HOLDERS` bounds holders *live at once* |
| `main.rs` | `reclaim_dead_task` releases the quota after `reclaim_holder`, reporting `SLIME_GRAPH quota released task=N live=M` | The ceiling outlives every charge made against it and is dropped only once nothing can be charged again |
| `main.rs` | The loan terminal marker gains `quotas=` | A retained ceiling is visible rather than silent |
| `check-sel4-supervision-plane.py` | Requires `quotas=0` on the terminal line | B24's exit condition is observable |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The release stops running | `just sel4_supervision_check` | `missing marker: every constructed holder released its declared quota` |
| The release moves before charge settlement | `just sel4_supervision_check` | `holder reclaim incomplete` — the table would charge a holder it can no longer bound |
| The `quotas=` append disturbs the gates matching the loan line | `just sel4_loan_check`, `sel4_sample_check`, `sel4_stream_check` | Their terminal markers stop matching |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_supervision_check` | Pass — 38 `quota released` lines, `quotas=0` at teardown — [`supervision-plane-quotas.log`](supervision-plane-quotas.log) | Direct |
| Fault injection: release disabled | Fails with `quotas=38` and the named missing marker — [`fault-injection-no-release.log`](fault-injection-no-release.log) | Direct |
| `just sel4_root_boot_check`, `sel4_component_graph_check`, `sel4_channel_check`, `sel4_loan_check`, `sel4_spawn_check`, `sel4_sample_check`, `sel4_stream_check`, `sel4_crossing_check` | All pass — the `quotas=` append did not disturb the gates matching the loan line | Direct |
| `just test_sel4_root` | Pass — 102/102 | Direct |
| `just contracts_check`, `just generation_check`, `just devlog_check` | Pass | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | Pass | Direct |
| A graph crossing `MAX_CHARGE_HOLDERS` itself | **Not observed, and unreachable** — see investigation step 5. Root CSlots exhaust near 52 tasks | Unobserved, with reason |

## Decisions

- Decision: a **direct release**, not a derived sweep.
- Rationale: B16 and B22 both needed predicates because their entries can be
  named by a capability that outlives the task — a supervision handle held by a
  second parent, a channel end parked in `Transit`. A quota is bound to exactly
  one holder and that holder is a task, so "the task is gone" is complete
  information. Adding a sweep here would be cargo-culting the shape of the two
  fixes rather than their reasoning.
- Rejected alternative: sweeping lazily on `ChargesExhausted`, matching B22.
  It would only ever fire on a boot that cannot happen (step 5), so the code
  would be permanently unexercised.

- Decision: assert on the **existing supervision plane** rather than build a
  tenth image.
- Rationale: that plane is already the deepest spawn/reap loop in the corpus,
  which is the exact shape this defect needs, and asserting there also proves
  the fix perturbs nothing it already covers. A tenth image would have added a
  generation, a fixture, a boot-layout row — the base layout has one slot left —
  and a build variant to observe strictly less.

- Decision: **amend B24's exit condition** rather than stretch the evidence.
- Rationale: the condition I wrote when opening it asked for a graph crossing
  96 holders. Step 5 establishes that is unreachable while root CSlots are never
  returned. Zero-at-teardown over 38 holders is what the platform can carry, and
  the fault injection is what makes it non-vacuous. Recording the amendment is
  the honest move; quietly asserting something weaker under the original wording
  would not be.

## Open risks and follow-ups

- [ ] **The 96 bound itself is still unobserved.** The fix is verified by
      release-per-holder and zero-at-teardown, not by surviving the ceiling. If
      root CSlot reuse ever lands, a plane crossing 96 becomes constructible and
      is worth building then.
- [ ] **Root CSlots are the real lifetime bound on this platform** — ~66 per
      task against 3457, so ~52 tasks per boot. Deliberate and documented
      (`task.rs:165-167`), not a defect, but it is now the *binding* constraint
      on graph longevity, ahead of every table this class audit examined.
      Recorded because P5.4.1's audit classified it as acceptable-monotonic
      without quantifying it.
- [ ] `orphans` remains freed only by `retry_orphans`, which the root never
      calls. Recorded in P5.4.1 as a weaker fourth case; unchanged here.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript:
  [`supervision-plane-quotas.log`](supervision-plane-quotas.log) — the passing
  boot with 38 releases.
- Serial/debugger/model output:
  [`fault-injection-no-release.log`](fault-injection-no-release.log).
- Related roadmap item: [B24](../../roadmap/00-backlog.md) (resolved),
  [P5.4.1](../../roadmap/07-architecture-portability.md) (the audit that opened
  it), [B22](../../roadmap/00-backlog.md) and
  [B16](../../roadmap/00-backlog.md) (the same defect shape, resolved earlier).
