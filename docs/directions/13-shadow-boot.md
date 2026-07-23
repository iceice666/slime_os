# 13. Shadow boot

| | |
| --- | --- |
| Status | parked |
| Route | updates |
| Depends on | M5.6 rollback and health semantics and M6 spawn machinery (complete); the canonical [roadmap](../../roadmap/README.md) names it a follow-up |
| Enables | pre-activation health checking that never spends a real boot attempt |
| Now | Paper: shadow sub-graph manifest design is legal today. M6 supplies the spawn machinery, but the constrained shadow environment itself does not exist yet. |

## Motivation

A pending generation can be health-checked in a constrained sub-graph or
guest VM before real activation consumes a boot attempt. Today's
contract is already safe — a failing pending generation rolls back — but
it spends attempts and a real boot cycle to learn what a rehearsal could
have shown. Shadow boot moves the failure earlier: obviously broken
generations are rejected with the attempt counter untouched, reserving
real activations for candidates that pass rehearsal.

## What exists today

- M5.6 (complete): the attempt counter and its durable
  decrement-before-transfer semantics define exactly what shadow boot
  must *not* touch; the health service defines the verdict vocabulary a
  shadow check should anticipate.
- M5.5 (complete): generations are self-describing — a shadow manifest
  can be derived from the pending one mechanically.
- Missing: the constrained environment itself. M6 (complete) supplies
  spawn and endpoint-minting machinery for a reduced component graph; a
  guest VM alternatively needs virtualization support from
  [Foreign X2](../../roadmap/05-foreign-workloads.md).

## Design sketch

Two environment options with different costs. Constrained sub-graph:
boot the pending generation's kernel with a reduced manifest — the
health-relevant components wired against membranes or fixtures
([entry 7](07-schema-interposition.md) supplies them), devices replaced
by virtual counterparts. The verdict comes from the same health service,
running in the shadow graph. Guest VM: boot the full generation
verbatim inside a VM; higher fidelity, much larger dependency.

The safety invariant is one-directional information flow: the shadow
run may read the pending generation's objects but must not write any
durable state the real boot graph depends on — attempts, state bindings,
or GC roots. Shadow state is `ephemeral` by construction.

Failure semantics: a shadow failure rejects the pending generation with
a structured report (which health check, what evidence), and the
BootState attempt counter is untouched — the register's exit condition.
A shadow *pass* is advisory: real activation still consumes attempts,
since the shadow environment is deliberately not the real one.

## Open questions

- Sub-graph versus guest VM: which environment does the first
  implementation target now that M6 machinery is available?
- How is the shadow manifest derived from the pending one — declared
  reduction rules, or a per-generation shadow section?
- Fidelity boundary: which health checks are meaningful in a shadow
  graph (device-backed ones are not) and does a shadow pass mean
  anything for them?
- Resource cost: does a shadow boot charge an account
  ([entry 25](25-resource-accounts.md)) once accounts exist?

## Exit-condition sketch

A deliberately unhealthy pending generation fails its shadow health
check and is rejected with the real BootState attempt counter untouched.

## Probe guidance

Paper today: define the shadow manifest derivation rules and the
one-directional state rules (what the shadow may read, never write),
and map which existing health checks survive the constrained
environment. An execution probe may build on M6's completed spawn machinery.
