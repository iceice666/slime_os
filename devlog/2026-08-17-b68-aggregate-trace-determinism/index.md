# B68 — the determinism gate was comparing one scheduling interleaving, and grouping by worker was not enough

| Field | Value |
|---|---|
| Date | 2026-08-17 |
| Kind | Defect |
| Status | Verified |
| Scope | `scripts/check/check-sel4-fabric-aggregate.py` (`records_by_participant`, `check_determinism`) |
| Roadmap | B68, C8.15, B55 |
| Gates | `just sel4_fabric_aggregate_check`, `just sel4_gate_control_check` |
| Trigger | The closing full-suite sweep after the structural audit's eleven items landed |
| Baseline | C8.15 recorded as complete on "280 byte-identical trace records across four boots" |

## Summary

`sel4_fabric_aggregate_check` failed about one run in four. It is the gate that
proves C8.15's determinism claim, so a flaky version of it does not prove that
claim — and on a passing run it reported success for a property it had not
established. The comparison zipped the flat record list positionally, which asserts
one *interleaving* of concurrent activity. Fixing it took two attempts: grouping by
emitting worker was insufficient, because a worker's `[trace]` prefix names its
*sink* and one sink aggregates several record kinds. Grouped by `(worker, kind)`
the gate passes 10/10, still compares all 280 records, and still fails when one
record's content is perturbed.

## Observable symptom

- Command: `just sel4_fabric_aggregate_check`
- Expected: two boots of one composition produce byte-identical semantic traces
- Observed, ~1 run in 4:

  ```
  normal concurrent schedule: trace record 12 differs between boots -- the semantic
  trace depends on scheduling, so it cannot serve as a comparison baseline.
    first:  [trace] subscriber-b kind=resource order=data … event=13 high_water=2
    second: [trace] operation    kind=route    order=data … route=9011f5515bf9f4fe
  ```

- Exit/fault/serial evidence: both records are legitimate; the boots interleave
  them differently. Reproduced on a clean tree at `a5160f3` with every working
  change stashed — 3 of 4 consecutive runs passed.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Failed once in the closing sweep, passed on 2 retries | Flaky, not a hard break |
| 2 | Stashed all changes, ran 4× at `a5160f3`: 3 passed, 1 failed identically | Pre-existing; not from B57–B67, none of which touch trace emission |
| 3 | Read the diff: two *different workers*' records at the same index | The comparison asserts merge order across concurrent workers |
| 4 | Grouped by emitting worker, compared per-worker sequences | 5/6 — better, still failing |
| 5 | Read the new failure: `stream`'s record 2 was `kind=fault order=peer-death` vs `kind=qos order=time`, **both at `sequence=2`** | One `[trace]` prefix is a *sink*, not a single activity; two kinds race into it |
| 6 | Grouped by `(worker, kind)` | 0/10 — worse |
| 7 | Read that failure: `[trace] publisher complete capacity=64 …` has no `kind=` | Terminal records need their own group, not rejection |
| 8 | Handled the terminal form; ran 10× | 10/10 |
| 9 | Perturbed one field of one record in the second boot | Still fails, naming the worker and kind — the weaker comparison is not vacuous |

## Root cause

Two layers, the second only visible after fixing the first.

`check_determinism` compared `zip(left, right)` over the flat record list. That
asserts that concurrently-running workers emit in the same *order* across boots,
which is scheduling, not a property of the trace. C8.15's claim is that each
worker's semantic sequence is reproducible.

Grouping by the worker named in the `[trace]` prefix was still wrong, because that
name identifies the *sink* a worker writes to, and one sink receives records of
several kinds from independent activities — a QoS timer and a peer-death fault, in
the observed case, both assigned `sequence=2` by their own counters. Their arrival
order at the shared sink is scheduling too.

The innocent site is the trace emission itself: nothing was emitting
nondeterministically. Every record in both boots was correct; only the comparison
was wrong.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `records_by_participant` | New: groups records by `(worker, kind)`, with terminal `[trace] X complete` records under a `complete` pseudo-kind | The gate compares what is deterministic |
| `check_determinism` | Compares per-group sequences; asserts the worker/kind *set* matches and each group's length matches before comparing contents | A boot that dropped a whole activity cannot pass by comparing what remains |
| Both | The 280-record total stays pinned | A plane that stopped emitting still cannot compare equal to itself |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A worker's own per-kind sequence becomes scheduling-dependent | `just sel4_fabric_aggregate_check` | `<worker>'s <kind> record N differs between boots` |
| A boot drops an entire worker or activity | Worker/kind set comparison | `the two boots emitted traces for different participants` |
| A worker emits a different number of records | Per-group length check | `<worker>/<kind> emitted N records in the first boot and M in the second` |
| A plane stops emitting entirely | Pinned 280-record total | `emitted N trace records, expected 280` |
| The weaker comparison becomes vacuous | Verified by perturbation this session | Would have passed the mutation |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Pre-existence at `a5160f3`, clean tree | 3/4 runs passed, 1 failed identically | Direct |
| `just sel4_fabric_aggregate_check` after the fix | **10/10 consecutive runs** | Direct |
| Records still compared | 280 across both schedules, unchanged | Direct |
| Not vacuous: perturb `sequence=2` → `sequence=99` in the second boot | Fails with `call's call record 1 differs between boots`; reverted | Direct |
| `just sel4_gate_control_check` | pass — 32 gates reject 1227 mutations | Direct |
| `just ruff`, `just typos` | pass | Direct |

## Decisions

- Decision: weaken the comparison to per-worker-per-kind rather than making the
  trace totally ordered by construction.
  Rationale: a construction-assigned global sequence would mean a shared counter
  across independent workers, which is either a synchronisation point in the fabric
  or a root-mediated service — real mechanism added to make a *gate* simpler. The
  determinism C8.15 wants to claim is that each activity's semantics are
  reproducible; that is what is now compared. This is the same choice B55 made for
  the boot plane's racy cross-task markers.
  Rejected alternative: a total order — it changes the product to suit the test.

- Decision: assert the worker/kind set and per-group lengths, not just contents.
  Rationale: any grouping comparison can be passed by having fewer groups. Without
  the set check, a boot that dropped an entire activity would compare its remaining
  groups and pass. This is the failure mode that makes weakened comparisons
  dangerous, so it is closed explicitly.

- Decision: verify by perturbation before claiming the fix.
  Rationale: the item being fixed is "a gate that does not prove what it claims", so
  shipping a *weaker* gate without demonstrating it still catches divergence would
  repeat the defect in the other direction.

- Decision: group terminal records under a `complete` pseudo-kind rather than
  excluding them. Rationale: there is exactly one per worker and it carries the
  sink's capacity and drop counts — real evidence. Excluding it would stop comparing
  whether a boot dropped records.

## Open risks and follow-ups

- [ ] **[INFERENCE]** The trace emission itself is judged deterministic — only the
  comparison was wrong — because every record in both boots was well-formed and the
  per-group sequences match across 10 runs. No independent proof that a worker's
  counter assignment is deterministic under all schedules was taken; 10 runs is
  evidence, not exhaustion.
- [ ] The `EXPECTED_TRACE_RECORDS` total (280) is still a hand-pinned constant, the
  same shape B63 flagged for marker counts. It is load-bearing here: it is what stops
  a plane that stopped emitting from passing.
- [ ] Whether other gates compare concurrently-produced output positionally was not
  audited. B63 fixed one such case in `sel4_call_check` and this is a second; a
  systematic pass over every gate's ordering assumptions has not been done.

## Artifacts and provenance

- Focused report: none; the sweep that found B68 closed
  [the structural audit](../2026-08-17-structural-audit/index.md)'s eleven items.
- Raw transcript: none preserved; both failure messages are quoted above and the
  flake is reproducible by reverting `records_by_participant` to a flat `zip`.
- Serial/debugger/model output: quoted inline — the record-12 worker disagreement,
  `stream`'s `kind=fault`/`kind=qos` collision at `sequence=2`, and the
  `[trace] publisher complete` line with no `kind=`.
- Related roadmap item: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md) — B68
  in the resolved log; the backlog is clear.
  [B55](../2026-08-15-b55-full-graph-boot-restoration/index.md) established the
  compare-what-is-causal principle this reuses, and
  [B63](../2026-08-17-b63-gate-helper-consolidation/index.md) applied it to
  `sel4_call_check` earlier the same day.
