# B65 — four plane launchers moved out of init.rs, and why the binary collapse should not happen yet

| Field | Value |
|---|---|
| Date | 2026-08-17 |
| Kind | Change |
| Status | Verified |
| Scope | `components/bins/src/bin/init.rs`, new `components/bins/src/{loan,spawn,crossing,supervision}_plane.rs`, `scripts/check/check-sel4-supervision-plane.py` |
| Roadmap | B65, B60 |
| Gates | `just sel4_loan_check`, `just sel4_spawn_check`, `just sel4_crossing_check`, `just sel4_supervision_check` |
| Trigger | The structural audit measured 21 plane launchers making up 895 of `init.rs`'s 2286 lines |
| Baseline | Every plane's edit shared one 2286-line file with every other plane's |

## Summary

`init.rs` held 21 `drive_*_plane` launchers in 2286 lines. The four largest
self-contained ones moved into their own `#[path]`-included modules, taking
`init.rs` to 1644 lines. Doing it surfaced a coupling the audit had not noted:
init's slot numbers arrive by `include!` of a *generated per-generation* boot
layout into `init.rs`'s own scope, so every extracted module must reach back
through `super::` — inherent, not a bad split. The audit's other half, collapsing
the 12 call/operation fixture binaries, is deliberately **not** done: those
binaries are tasks named in `.zti` fixtures with declared capability slots and
gate markers, so collapsing them rewrites the authority-declaring layer B60 has
just made a checked invariant, to save Cargo targets rather than logic.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `loan_plane.rs` (346 lines) | B49/B13 loan and quota-ceiling plane plus its private helpers | A plane's edit no longer shares a file with 20 others |
| `spawn_plane.rs` (187) | P5.3.3 spawn and supervised collection | same |
| `supervision_plane.rs` (86) | P5.3.3/B16 handles, records, reclamation | same |
| `crossing_plane.rs` (84) | B22 endpoint authority across a spawn boundary | same |
| `init.rs` | 2286 → 1644 lines; `spawn_or_fail` and `PEER_PARK_YIELDS` published back as genuinely shared | Shared helpers are visible as shared |
| `check-sel4-supervision-plane.py` | Derives its bound from `supervision_plane.rs`, which now declares it | The gate reads the module that owns the constant |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| An extracted plane stops behaving identically | `just sel4_loan_check`, `sel4_spawn_check`, `sel4_crossing_check`, `sel4_supervision_check` | Each plane's own marker chain |
| A helper is moved that another plane needs | The compiler — every extraction was built before the next | `cannot find function … in this scope` |
| A gate derives a bound from a file that no longer declares it | The gate itself | `cannot derive MAX_RECORDS or SUPERVISION_LOOP_CHILDREN` |
| An unextracted plane regresses | `just sel4_boot_check`, `sel4_sample_check`, `sel4_channel_check`, `sel4_reclamation_check` | Their own chains |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `init.rs` line count | 1644, from 2286 — 642 removed | Direct |
| `just sel4_loan_check` | pass — sealed subrange loaned, mapped read-only, returned once, all four quota classes refused at ceiling+1 | Direct |
| `just sel4_spawn_check`, `sel4_crossing_check` | pass | Direct |
| `just sel4_supervision_check` | pass after repointing its bound source — more tasks over a lifetime than `MAX_RECORDS` holds, every live handle answered | Direct |
| `just sel4_boot_check`, `sel4_sample_check`, `sel4_channel_check`, `sel4_reclamation_check` | pass — unextracted planes unaffected | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff` | pass, after removing an import the move orphaned | Direct |

## Decisions

- Decision: extract one module at a time with a build between each.
  Rationale: which helpers are plane-private versus shared is not obvious from
  reading — `spawn_or_fail` and `PEER_PARK_YIELDS` both looked local and are used by
  `init.rs` itself. The compiler answers that question exactly, but only if each
  extraction is compiled before the next; doing all four at once would have produced
  one undifferentiated pile of errors.

- Decision: reach generated slot constants through `super::` rather than importing a
  layout module. Rationale: the boot layout is `include!`d per-binary from `OUT_DIR`
  because it is generated per generation. There is no path naming it independently of
  the binary it was generated for, so `super::` is the only reference that stays
  correct across generations. Documented in each module's header so the next reader
  does not try to "fix" it.

- Decision: extract four launchers, not 21.
  Rationale: 10 of the remaining ones are under 60 lines and four are 3-line wrappers
  around `launch_fabric_graph`. Each extraction costs its own `super::` import block,
  so for a 3-line launcher the module header would exceed the code. The four that
  moved are where a whole plane's logic was genuinely separable.

- Decision: **do not** collapse the 12 call/operation fixture binaries.
  Rationale: each is a task named in `.zti` fixtures, granted specific capabilities
  at declared slots, and asserted by name in gate marker chains. Collapsing them
  means rewriting six fixtures' executable and grant tables, the boot-layout slot
  assignments those produce, and every gate marker naming a component — and the
  per-role *logic* is already shared through `fabric_call_scenario.rs` and
  `fabric_operation_scenario.rs`, so the saving is in Cargo targets, not code. B60
  has just made those slot assignments a build-time checked invariant, which raises
  the cost of churning them. Deferred with the work costed rather than attempted
  half-way.
  Rejected alternative: collapsing only the `-b`/`-b-restart` variants — the audit
  itself recorded (and I confirmed) that those exist to make a fan-in a *real*
  fan-in between distinct components, so merging them would weaken what the gates
  prove.

## Open risks and follow-ups

- [ ] 17 launchers remain in `init.rs`. Small individually, but the file still grows
  with each new plane, and the hand-written `match startup_arg` still needs an arm
  per plane. Table-driven dispatch from the generation manifest was not attempted.
- [ ] The 52-binary fixture population is untouched. Its own entry should cost the
  fixture, boot-layout, and gate-marker work before anyone starts.
- [ ] **[INFERENCE]** The four extractions are judged behaviour-preserving because
  each plane's own gate passes with its full marker chain. No byte-level comparison
  of the built component images before and after was taken, so "pure code motion" is
  inferred from gate equivalence rather than from identical binaries.
- [ ] `check-sel4-reclamation-plane.py` still greps `init.rs` for
  `RECLAMATION_LOOP_CHILDREN`. Correct today, and it will break the moment that plane
  is extracted — the same trap `supervision` hit. A gate deriving a bound by grepping
  a source file is fragile regardless of which file it names.

## Artifacts and provenance

- Focused report: none; the audit that opened B65 is
  [the structural audit entry](../2026-08-17-structural-audit/index.md), which
  measured the launcher line counts and the binary classification.
- Raw transcript: none preserved; line counts are reproducible with `wc -l`, and
  each gate result from its named `just` target.
- Serial/debugger/model output: the gates' own summaries are quoted in
  *Verification*.
- Related roadmap item: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md) —
  B65 in the resolved log with the deferred binary collapse costed; the backlog is
  now clear. [B60](../2026-08-17-b60-control-plane-authority/index.md) is why
  churning the fixtures' slot assignments now costs more than it did.
