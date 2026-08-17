# B67 — two negative controls aimed at a slot the audit declares, and the second one hid behind the first

| Field | Value |
|---|---|
| Date | 2026-08-17 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root/src/task.rs` (`ChildSlots`, `audit_child_cspace`, B40 mutation arms), `just sel4_capability_layout_check` |
| Roadmap | B67, B40, B57 |
| Gates | `just sel4_capability_layout_check`, `just sel4_boot_check`, `just sel4_boot_layout_check` |
| Trigger | Running B57's verification sweep found `sel4_capability_layout_check` red on its `extra` arm |
| Baseline | The gate reported "all 6 negative mutations refused" at some earlier point; two of the six had stopped perturbing what they named |

## Summary

`just sel4_capability_layout_check` boots six deliberately corrupted child
CSpaces and requires `audit_child_cspace` to refuse each. Two arms were aiming at
slot 4, which is `CHILD_SLOT_CNODE` — a slot the audit *declares*. `extra` copied
a capability there and the audit correctly stayed silent, so the gate reported the
audit had accepted a mutation. Fixing that advanced the gate to `wrong_slot`,
which diverted the fault capability to the same slot; because slot 4 is already
occupied, the mint failed during *construction* and the audit never ran at all.
Both defects are one root cause: slot arithmetic that restates a subset of the
predicate it is trying to violate. `ChildSlots` now owns `declares` and
`first_undeclared`, and the audit walk plus both arms go through them. All six
mutations are refused, and each arm was proven non-vacuous by weakening the audit
and watching the gate fail.

## Observable symptom

- Command: `just sel4_capability_layout_check`
- Expected: `all 6 negative mutations refused`
- Observed: `capability layout check: the audit accepted a mutated CSpace: a capability was installed into an undeclared slot (--cfg slime_b40_mutate_extra)`, exit 1 after the first three arms passed.
- Exit/fault/serial evidence: after fixing `extra`, the next arm failed
  differently — `the mutation was not refused as a CSpace mismatch: a declared
  capability was installed at the wrong slot (--cfg slime_b40_mutate_wrong_slot)`.
  Booting that image directly produced the decisive line:

  ```
  SLIME_ROOT FATAL SLIME_GRAPH FAIL instance init construction failed:
    Mint { slot: 4, error: DeleteFirst }
  ```

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Gate red on `extra` while running B57's sweep | Could have been caused by B57's rights change |
| 2 | Stashed every working change, re-ran at `beff860` | Same failure — pre-existing, independent of B57/B58 |
| 3 | Read the mutation's slot choice (`task.rs:1060-1064`): excludes `{0, service, fault, tcb}` | It excludes `{0,1,2,3}` |
| 4 | Read the audit's declared set (`task.rs:1075-1078`): `{service 1, console 32, fault 3, tcb 2, CHILD_SLOT_CNODE 4}` | The two sets disagree at slot 4 and slot 32 |
| 5 | `CHILD_SLOT_CNODE = 4` (`task.rs:66`), `CHILD_SLOT_CONSOLE = 32` (`:72`) | `find` returns 4 — declared — so the arm never created undeclared occupancy |
| 6 | Promoted the predicate to `ChildSlots::declares` + `first_undeclared`, routed the audit and `extra` through them | `extra` now refused; gate advanced to `wrong_slot` |
| 7 | `wrong_slot` failed with a *different* message — not accepted, not a CSpace mismatch | A second defect, previously masked by the first |
| 8 | Built the `wrong_slot` image and booted it directly | `Mint { slot: 4, error: DeleteFirst }` — construction failed before the audit ran |
| 9 | Read the arm: `child_slots.fault.wrapping_add(1) % (1 << cnode_size_bits)` = 3+1 = 4 | Same root cause: slot arithmetic ignoring the declared set |
| 10 | Routed `wrong_slot` through `first_undeclared` too | All six arms refused |
| 11 | Weakened the audit three ways to prove the arms bite (below) | Each weakening produced a gate failure; all reverted |

## Root cause

One mechanism, two instances: a negative control that computes its victim slot by
restating part of the predicate it exists to violate.

`audit_child_cspace` declares a slot occupied when it is `service`, `console`,
`fault`, or — for a self-managed child — `tcb` or `CHILD_SLOT_CNODE`. Neither
mutation consulted that. `extra` hardcoded a four-element exclusion list;
`wrong_slot` did modular arithmetic on the fault slot. Both landed on
`CHILD_SLOT_CNODE`.

The two then failed in opposite directions, which is why one hid the other:

- `extra` produced a *declared* slot being occupied, which is exactly what the
  audit expects, so the audit passed and the gate reported acceptance.
- `wrong_slot` targeted a slot that is not merely declared but already **filled**,
  so `seL4_CNode_Mint` returned `DeleteFirst` and construction aborted. The gate
  saw a refusal — from the wrong mechanism — and correctly rejected it as not a
  `CSpaceMismatch`.

`console` at slot 32 was missing from `extra`'s exclusion list as well; it was
latent only because 4 is selected first. Deriving the set removes that too rather
than adding a fifth hand-written exclusion.

The innocent site is `mint_child_slot`: returning `DeleteFirst` for an occupied
destination is correct kernel behaviour and correct error propagation. It is not
the defect; it is the mechanism that exposed it.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/src/task.rs` — `ChildSlots` | Added `declares(slot, expect_tcb)`, the single source for "the plan declares this slot" | The audit and its negative controls cannot disagree about the declared set |
| `slime-root/src/task.rs` — `ChildSlots` | Added `first_undeclared(cnode_size_bits, expect_tcb)`, `#[cfg]`-gated to the two mutation builds | A mutation needing an undeclared slot asks for one instead of computing one |
| `slime-root/src/task.rs` — `audit_child_cspace` | The walk and the `extra` arm both call the shared methods | `extra` occupies a genuinely undeclared slot |
| `slime-root/src/task.rs` — fault mint | `wrong_slot` diverts to `first_undeclared` instead of `fault + 1` | The perturbation stays a *layout* error the audit can catch, not a construction failure |
| `roadmap/00-backlog.md` | B67 resolved with both defects and the bite-proofs recorded | The backlog records what was proven, including the second defect found behind the first |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A mutation arm silently stops perturbing anything | `just sel4_capability_layout_check` | The arm is reported accepted, or refused by the wrong mechanism |
| A new declared slot is added but a mutation's exclusion list is not updated | Structural: there is no second list — `declares` is the only one | Would require editing the audit predicate itself |
| `audit_child_cspace` regresses on undeclared occupancy | `extra` and `wrong_slot` arms, both proven to depend on it | Gate reports the audit accepted a mutated CSpace |
| `audit_child_cspace` regresses on a declared slot left empty | `missing` arm, proven to depend on it | Gate reports the audit accepted a deleted capability |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_capability_layout_check` | pass — `all 6 negative mutations refused`, each named | Direct |
| Weaken audit: `if occupied && !declared` → ignore | `extra` reported accepted → the arm bites | Direct |
| Weaken audit: blind it at the slot both arms target | `extra` reported accepted → both arms depend on that check (`extra` fails first, since they share the victim slot) | Direct |
| Weaken audit: `if !occupied && declared` → ignore | `missing` reported accepted → that half of the predicate is load bearing | Direct |
| All weakenings reverted, gate re-run | pass — `all 6 negative mutations refused` | Direct |
| `just test_sel4_root` | pass — 118/118 across 14 modules | Direct |
| `just sel4_boot_check` | pass — 30 markers, 5 chains, 21-slot layout, 19 tasks, none exited | Direct |
| `just sel4_boot_layout_check` | pass — 25 plane layouts match their fixtures | Direct |
| `just sel4_root_boot_check` | pass | Direct |
| `just fmt_check_all`, `just lint_all` | pass | Direct |
| Pre-existence at `beff860` | Reproduced with all working changes stashed | Direct |

## Decisions

- Decision: put `declares`/`first_undeclared` on `ChildSlots` rather than passing a
  closure from `audit_child_cspace` to the mutation arms.
  Rationale: `wrong_slot` mutates at the *mint* site (`task.rs:706`), hundreds of
  lines before the audit runs, so a closure local to the audit could not reach it.
  The slot set is a property of the layout, which is what `ChildSlots` is.
  Rejected alternative: a module-level `const` list of declared slots —
  `CHILD_SLOT_CNODE` is declared only for a self-managed child, so the set depends
  on `expect_tcb` and is not constant.

- Decision: exclude slot 0 from `first_undeclared`, and record why at the
  definition. Rationale: the audit does require slot 0 empty, so occupying it would
  be refused — but for two reasons at once, since it also breaks the
  null-capability invariant. A negative control should isolate one property.
  Rejected alternative: including it and accepting the ambiguity; the whole point
  of these arms is to attribute a refusal to a specific check.

- Decision: gate `first_undeclared` behind
  `#[cfg(any(slime_b40_mutate_extra, slime_b40_mutate_wrong_slot))]`.
  Rationale: it exists only for the mutation builds and is dead code in the
  product image, which `lint_all` denies warnings for.

- Decision: fix `wrong_slot` in this change rather than opening B68.
  Rationale: unlike B67-vs-B57, this is not an independent mechanism — it is the
  same root cause in a second call site, and it was unreachable until the first fix
  landed. Splitting it would leave the gate red between two commits.

## Open risks and follow-ups

- [ ] The `aliased` and `wrong_type` arms were not individually bite-proofed; they
  passed both before and after this change, and they are refused by different
  predicates (`InstallLedger` alias detection and `audit_child_types`) than the
  occupancy walk this entry weakened. **[INFERENCE]** They are presumed sound
  because they were already failing-then-passing across B40's history, but that is
  inherited reasoning, not an observation from this session.
- [ ] The gate's per-arm coverage is now derived from one predicate, but
  `check-sel4-gate-controls.py` still hand-pins a marker count per gate (B63). A
  seventh mutation would need that integer updated by hand.
- [ ] B67 was found only because B57's exit condition named a broad sweep. Nothing
  runs the full gate set routinely, so a gate that goes red stays red silently —
  B56 had been red since B55 for the same reason. Not tracked as its own item yet.

## Artifacts and provenance

- Focused report: none; the audit that opened B67 is
  [the structural audit entry](../2026-08-17-structural-audit/index.md), and the
  sweep that found it is recorded in
  [the B57/B58 entry](../2026-08-17-b57-b58-rights-vocabulary/index.md).
- Raw transcript: none preserved; the decisive `Mint { slot: 4, error: DeleteFirst }`
  line is quoted above and regenerable with
  `SLIME_B40_MUTATION=wrong_slot python3 scripts/build/build-sel4.py --boot-plane`
  followed by booting `build/slime-sel4-boot.elf`.
- Serial/debugger/model output: quoted inline; full transcripts regenerable with
  `just sel4_capability_layout_check`, which prints each arm's transcript on failure.
- Related roadmap item: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md) —
  B67 in the resolved log; B40 is the milestone whose mutation series this gate
  guards.
