# B48 — the schedule record was there all along

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/generation/v1/schema.zt`, `scripts/build/build-generation.py`, `boot-contracts/src/generation.rs`, `slime-root/src/{task,main}.rs`, `sel4/config/qemu-arm-virt.cmake`, `contracts/generation/v1/fixtures/sel4-qos.zti` |
| Roadmap | B48 |
| Gates | `just sel4_qos_check`, `just sel4_root_boot_check`, `just sel4_component_graph_check` |
| Trigger | B48: every child ran at `CHILD_PRIORITY = 254` with no scheduling authority in generation data. |
| Baseline | Builder packed a constant priority; the root ignored the record. |

## Summary

B48's fix names the priority half first, and it is done: child priority is
authenticated generation data, applied on both the boot-graph and spawn paths,
and observed in the running graph. The MCS half is deferred with the assurance
decision recorded rather than left blank. Two exit clauses need MCS and cannot
be met without it, so the item stays open on those.

## Changes

- **`Instance.priority?`** in the manifest schema, bounded `0..=254`.
- **The builder writes it** into the `ScheduleRecord` it was already emitting
  with a hardcoded 100, and refuses an out-of-range value with the reason.
- **`Generation::instance_priority`** resolves instance → process → thread →
  schedule.
- **Both construction paths** pass it to `tcb_set_sched_params`;
  `task::admit_priority` refuses anything at or above the root's own.
- **`SLIME_GRAPH schedule`** records instance, priority, and the default.
- **`sel4-qos.zti`** declares `fabric-intruder` at 100.
- **`qemu-arm-virt.cmake`** carries the MCS decision and its cost.

## Regression guards

- `a_priority_at_or_above_the_root_is_refused` covers the root's own bound,
  including `CHILD_PRIORITY` itself as admissible and `Word::MAX` as not.
- The builder's bound was verified by setting the intruder to 255 and
  observing `priority 255 outside 0..=254`.
- `SLIME_GRAPH schedule` makes the applied value observable, so a regression
  to the constant would show in every plane's transcript.

## Verification

| Check | Result |
|---|---|
| `just sel4_qos_check` | pass; transcript shows `fabric-intruder priority=100` against five peers at 254 |
| `just sel4_root_boot_check` | pass |
| `just sel4_component_graph_check` | pass |
| All 30 seL4 gates | pass |
| `cargo test -p slime-root --lib` | 146 passed |
| `just contracts_check`, `just generation_check`, `just test_host`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos` | clean |

## Decisions

**Per-thread, resolved through the plan.** The obvious shortcut is to read
priority off the instance record. The `ScheduleRecord` is already per-thread so
that a process with several can differentiate them, and reading it from the
instance would flatten that the first time one does.

**Refused, not clamped, in two places.** The builder bounds a manifest; the
root bounds a generation that did not come from the builder. Clamping would let
such a generation run at a priority it did not ask for, silently. A child at or
above the root's priority is not merely impolite — it can keep the service loop
from running, so every other child blocks behind it on a root that never
answers.

**Marker, not a field on `staged`.** The priority a thread runs at is not
observable from anything else in the transcript, and a declaration nothing can
check is indistinguishable from the constant it replaced. A separate record
also avoids re-pinning `staged` in every gate that asserts it verbatim.

**MCS deferred with a recorded reason.** seL4's functional-correctness proofs
do not cover MCS on AArch64. This repository's claim is upstream seL4 with its
assurance intact, so enabling it in a config file would trade a verified kernel
for a scheduling feature without anyone deciding to. `budget_us` and
`period_us` stay zero rather than carrying figures the kernel cannot enforce —
an authenticated number the system does not honour is worse than an absent one.

## Open risks and follow-ups

- Two exit clauses remain: budget and period as authenticated data, and one
  budget-exhausting client not starving a higher-criticality service. Both need
  MCS. Timeout faults likewise have no kernel mechanism to reach a handler.
- Only one fixture declares a non-default priority. The mechanism works for any
  of them, but nothing forces a plane to exercise it, so a regression that
  reverted to the constant would only fail the QoS plane.

## Artifacts and provenance

- Commit: `cb7ded5`.
