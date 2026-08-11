# B52 — the loan plane loaned to peers that never launched

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Defect |
| Status | Verified |
| Scope | `contracts/generation/v1/fixtures/sel4-loan.zti`, `components/bins/src/bin/init.rs`, `slime-root/src/main.rs`, `scripts/check/check-sel4-loan-plane.py` |
| Roadmap | B52 |
| Gates | `just sel4_loan_check`, `just sel4_sample_check`, `just sel4_spawn_check`, `just sel4_reclamation_check` |
| Trigger | `just sel4_loan_check` failed at `[init] loan plane fail: loan`; found by auditing Justfile targets no previous turn had run. |
| Baseline | Red since before the v5 cutover, verified at `8745d18~1`. |

## Summary

The loan plane loans to two peers and launched neither. A loan names its
receiver as the unique live holder of the channel's other end, so both were
refused `absent-or-ambiguous` before anything about loans was exercised. The
two peers needed different answers, and fixing them exposed four assertions in
the gate that had never run — one of which found a real asymmetry in the root.

## Observable symptom

- Command: `just sel4_loan_check`
- Expected: a sealed subrange loaned, mapped read-only, returned, reclaimed.
- Observed: `SLIME_GRAPH loan refused task=0 slot=3 class=absent-or-ambiguous`,
  then `[init] loan plane fail: loan`.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `sample-receiver` declared, never spawned | `drive_loan_plane`'s docstring said the mechanism did not exist "until P5.3.3" — it does now |
| 2 | Spawn refused `ungranted` | Init had no `exec` grant over it |
| 3 | Then `declared-count requested=0 bindings=1` | Init holds the receiver's channel end and must pass it |
| 4 | Then `class=budget live=0 budget=0` | `spawnBudget` was 0 |
| 5 | Receiver scenario runs; fails at `strand loan` | Same defect, second peer: `console` |
| 6 | Console spawn refused `held-rights requested=0x2 held=0x1` | Init holds the *producer* end and cannot pass a consumer side |

## Root cause

Two peers, two different reasons.

`sample-receiver`'s channel end is a binding init holds, so it can be passed at
spawn — three declarations were missing: the `exec` grant, the binding in the
request, and budget.

`console`'s two ends are the opposite. The generation declares console the
*target* of `dango-output` and `powerbox-client`, so the root installs its
consumer ends directly and init holds only the producer sides. A spawn grant
conveys authority the parent holds; init never holds these. Console is
root-owned autostart instead — which is also the shape the strand arm wants,
since a spawned-but-idle peer gives a deterministic uncollected queue where an
absent one gives a refusal before the loan is recorded.

## Changes

- `sel4-loan.zti`: `init-console` removed in favour of `owner = "root"` on the
  console instance; `init-sample-receiver` exec grant added; `spawnBudget = 1`.
- `init.rs`: spawns `sample-receiver` before the *unsealed* probe, not after —
  that probe was being refused `absent-or-ambiguous` rather than `unsealed`,
  the right outcome for the wrong reason.
- `check-sel4-loan-plane.py`: budget parse made field-order independent; quota
  parse moved to `instance=`/`executable=`; two counts updated.
- `main.rs`: the boot path emits the per-instance quota record.

## Regression guards

- The gate's budget check now actually runs, comparing all three declared
  ceilings against the transcript rather than matching nothing.
- The boot path's per-instance quota record makes a wrong ceiling visible in
  every plane's transcript, not just for spawned children.

## Verification

| Check | Result |
|---|---|
| `just sel4_loan_check` | pass (was red before the v5 cutover) |
| `just sel4_sample_check`, `sel4_spawn_check`, `sel4_reclamation_check` | pass |
| 28 further plane gates | pass |
| `just contracts_check`, `sel4_boot_layout_check`, `sel4_gate_control_check` | pass |
| `cargo test -p slime-root --lib` | 145 passed |
| `just lint_all`, `fmt_check_all`, `ruff`, `typos` | clean |

## Decisions

**Root-owned autostart for console, not a spawn.** I tried the spawn three
times — full rights, declared rights, no grants — and each refusal was correct.
The generation says init holds producer ends; no arrangement of spawn grants
conveys a consumer end it does not have. The refusals were the mechanism
working, and the right move was to stop arguing with them.

**The unsealed probe moved.** It is one of the plane's real claims — an
unsealed region cannot be loaned — and it was passing on a different refusal
entirely. Ordering it after the spawn is what makes it test what it says.

**The boot path emits per-instance quotas.** This started as a stale-marker fix
and turned out to be an asymmetry: quotas were declared for root-launched
instances and reported only in aggregate, so the gate comparing declared
ceilings to observed ones could only ever see spawned children. Both paths now
print the same record.

## Open risks and follow-ups

- The gate's budget parse is still a regex over fixture text. It is field-order
  independent now, but a fixture that renamed a field would silently match
  nothing again — the same failure it had, in a new place.
- `boot_layout.py`'s `CONSOLE_SLOT` resolves to 1 for this plane, which is
  `sample-receiver`'s slot. Nothing reads it here, so it is harmless today;
  it is a name-keyed table that does not know which plane is asking.

## Artifacts and provenance

- Commit: `b2b564a`.
- The "red before the v5 cutover" claim was verified by checking out
  `8745d18~1` and running the gate, not inferred from history.
