# 15. Zutai-defined state migrations

| | |
| --- | --- |
| Status | parked |
| Route | sync |
| Depends on | M5.6b state-transaction semantics (complete); Zutai evaluation in the build pipeline (host-side is acceptable) |
| Enables | [entry 29](29-schema-state-merge.md) (merge is migration's multi-machine generalization) |
| Now | Host-side prototype legal today: a v1→v2 migration in Zutai dry-run against fixture state bindings. The activation-path application waits for a milestone that stages state transitions. |

## Motivation

State schema upgrades expressed as pure Zutai transformations are
deterministic, dry-runnable before activation, and covered by the same
rollback contract as the boot graph. Today a schema change would be an
implicit, unauditable rewrite of state bytes; as Zutai transformations,
migrations become generation data — inspectable, replayable, and
reverted by the same mechanism that reverts the components reading the
state.

## What exists today

- M5.6b (complete) fixes the semantics migrations must respect: state
  bindings carry one of five policies (immutable, ephemeral, preserve,
  snapshotBeforeUpgrade, discardOnRollback), snapshot epochs pair each
  generation with a complete state set, and GC reachability protects
  retained roots.
- `snapshotBeforeUpgrade` is the policy that names this entry's runtime
  shape: upgrade snapshots the old binding before the new generation
  touches it, which is exactly the input a migration transforms.
- Zutai is already the schema language for IPC contracts (M5.2a,
  complete); migrations extend the same language from wire formats to
  state-at-rest.
- No build-pipeline Zutai evaluation exists yet; the register explicitly
  allows the host-side form first.

## Design sketch

A migration is attached to a schema version pair (v1→v2) as a pure
Zutai function from old binding bytes to new binding bytes. Pure means:
no clock, no entropy, no I/O — the same determinism discipline
[entry 3](03-nondeterminism-as-capabilities.md) defines, enforced here
by the evaluator rather than by grants.

Dry-run: the builder (or a host tool) applies the migration to a fixture
binding — or to a snapshot of real state — and reports the result
without sealing it, so a generation author sees the migrated state
before activation. Apply: during activation, the pending generation's
declared migrations run against the snapshotted bindings; the migrated
result seals as a new object in the new epoch. Rollback restores the
pre-migration binding per policy, which M5.6b's reachability already
guarantees is retained.

Failure semantics: a migration that errors or fails validation leaves
the old epoch intact and the pending generation fails health — the same
failure accounting as any other activation fault.

## Open questions

- Who writes the v1→v2 transform — the component author shipping the new
  schema (and how is it reviewed), or the generation author?
- Dry-run reporting shape: diff of decoded state, or just success/failure
  plus the result hash?
- Migration chains (v1→v3 through v2): composed automatically, or must
  each adjacent pair be declared?
- Validation: does the migrated output get checked against the v2 schema
  structurally before sealing?

## Exit-condition sketch

A schema v1→v2 migration written in Zutai dry-runs against a fixture
state binding, then applies during activation; rollback restores the v1
binding per policy.

## Probe guidance

Host-side today: implement the Zutai evaluator path for one fixture
schema pair, dry-run it against a fixture binding, and check
determinism (same input bytes → same output bytes across runs). The
probe's output is the migration declaration format plus measured
evaluation bounds, which decide where activation-time evaluation may
live.
