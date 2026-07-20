# 17. Per-component energy accounting

| | |
| --- | --- |
| Status | parked |
| Route | hardware |
| Depends on | M7 daily-driver quality goals; the capability-matrix horizon questions whether accounting is authority or read-only telemetry (EnergyAccount row) |
| Enables | background power budgets carried as grants; battery-policy as manifest data |
| Now | Paper: the authority-vs-telemetry question the horizon poses is the design exercise, answerable without hardware. |

## Motivation

Scheduler-attributed energy per component and per channel activity,
with policy such as background power budgets carried as grants. On a
daily-driven laptop, "which component ate the battery" is currently
unanswerable, and "this background service may not exceed a power
budget" is unenforceable. Carrying budgets as grants makes energy
policy generation data — declared, auditable, rollbackable — like every
other resource decision.

## What exists today

- The scheduler is the natural attribution point (per-component run
  time already exists as a scheduling concept); nothing energy-related
  is measured. [INFERENCE: no accounting rows exist in the matrix.]
- The capability-matrix horizon carries the entry's core question
  verbatim: an EnergyAccount object with READ rights, and whether
  accounting is authority at all or read-only telemetry.
- [entry 25](25-resource-accounts.md) designs the general account
  mechanism; energy is a candidate quantity, or a deliberately
  separate axis — the split is part of the design.
- M7 (not implemented) owns daily-driver hardware bring-up, including
  the power telemetry this consumes.

## Design sketch

Two layers with a deliberate boundary. Measurement: the scheduler
attributes active time per component, and channel activity is charged
to endpoints' owners; hardware energy counters (RAPL-class, battery
controller) convert time-and-activity into energy estimates. This layer
is telemetry — numbers, no authority.

Policy: a generation-declared budget per component (or per supervision
subtree), enforced by throttling past the budget — scheduling policy in
userspace, per the policy-free-kernel invariant. The horizon's question
is where the boundary lands: if a budget causes throttling, the
EnergyAccount is acting as authority and belongs in the rights grammar;
if it only informs a userspace policy service, it stays read-only
telemetry and out of the matrix. The register's exit condition —
throttled past its declared budget — implies the authority reading,
with the enforcement mechanism itself living in userspace policy.

## Open questions

- The horizon question itself: authority or read-only telemetry — and
  if authority, what are the rights bits?
- Attribution of shared work: a service processing another component's
  request charges whom (caller, callee, split)?
- Budget window: energy per boot, per wall-clock window, or per
  session — and how does rollback treat accumulated consumption?
- Throttling semantics: hard scheduling denial versus priority
  degradation, and who declares which per component.

## Exit-condition sketch

On the Framework target, a busy-looping background component is
throttled past its generation-declared energy budget; accounting is
readable per component.

## Probe guidance

Paper: resolve the authority/telemetry boundary as a matrix amendment
proposal, define the attribution rules (including the shared-service
case), and sketch the budget schema in the manifest. Hardware
validation waits for M7.
