# 9. Manifest static analysis and grant-graph introspection

| | |
| --- | --- |
| Status | parked |
| Route | authority |
| Depends on | M5.5 (complete: machine-readable grants); provenance follow-up to M5.1 for the runtime half |
| Enables | [entry 1](01-authority-diff-gate.md), [entry 27](27-policy-carrying-generations.md); audit queries for [entry 18](18-network-authority.md) and [entry 28](28-accelerator-objects.md) |
| Now | The host-side half — a grant-graph query engine over manifests — is fully legal tooling today. Named as an M5.5 follow-up in ROADMAP. |

## Motivation

The component graph and all grants are declarative, so questions like
"which components can reach block-write" or "what is agent X's worst-case
blast radius" are answerable without running the system. This is the
static complement to provenance: provenance answers "why is this allowed"
by walking an explicit grant chain; this answers "what could happen" by
computing reachability over everything the manifest permits.

It is also the seed of the whole authority route. Once the engine exists,
[entry 1](01-authority-diff-gate.md) is two snapshots and a diff,
[entry 27](27-policy-carrying-generations.md) is a predicate over the
graph carried in the generation itself, and future object kinds
([entry 18](18-network-authority.md) destinations,
[entry 28](28-accelerator-objects.md) compute budgets) become auditable
by the same queries the day their matrix rows land.

## What exists today

- M5.5 landed generation format v2 with machine-readable 1:1 rights
  strings; the manifest is deterministic, bounded, and validated by
  `just contracts_check`.
- The rights vocabulary and object kinds are fixed by
  `../capability-matrix.md`; the horizon section lists candidate object
  kinds (Directory, NetworkDestination, EnergyAccount, SharedBuffer
  creation) whose audit questions this engine should absorb without
  redesign.
- M5.1 established that unprivileged components cannot acquire device
  rights (`storage_cap_check`); the runtime half (a live introspection
  service exposing the actual grant graph) needs the provenance
  follow-up, which does not exist yet.
- [entry 24](24-rights-algebra-model.md) is modeling the derive/transfer
  semantics that define what "can reach" means transitively.

## Design sketch

Host-side half: load a manifest, build the directed grant graph —
nodes are components and object kinds; edges are grants labeled with
rights, plus derived edges for every grant a component could produce
under narrow-only derive. Queries are reachability with rights
predicates: "which components can reach (BlockDevice, WRITE)", "the
union of object kinds reachable from agent X", "the minimal set of
components whose removal isolates component Y from Z". All answers are
manifest-deterministic, so the tool's output is itself checkable in CI.

Runtime half (blocked on provenance): an introspection service exposes
the live grant graph — including capabilities minted and derived after
boot — through a read-only schema. The exit condition ties the halves
together: static answers must match runtime provenance on a test graph,
which is also a cross-check of the narrow-only model in
[entry 24](24-rights-algebra-model.md).

Design constraint: the engine answers possibility, not permission. A
component "can reach" a right if the manifest and derive rules allow a
chain to exist; whether the chain was exercised is provenance's
question.

## Open questions

- Does "can reach" model derive as transitive closure over rights
  subsets only, or must it also model capability transfer along declared
  channel endpoints (a component can receive what its declared peers can
  send)?
- Query language: a small fixed set of named queries behind a just
  target, or a general predicate interface?
- Where do channel schemas (`../../contracts/`) tighten the answer — a
  component may hold a channel but the schema bounds what authority can
  cross it?

## Exit-condition sketch

`just authority_query` answers "which components can reach BlockDevice
write" from the manifest alone, matching runtime provenance on a test
graph.

## Probe guidance

Legal today as host tooling: build the graph loader and the reachability
queries over the current format-v2 manifests, and validate answers by
hand against the existing QEMU component graphs (storage slice,
rollback fixture). The probe's output is the engine plus a measured gap
list — which queries the manifest alone cannot answer until provenance
lands — which scopes the runtime half before promotion.

## References

- [seL4 capDL](https://docs.sel4.systems/projects/capdl/) — the static
  authority-description tradition this engine follows.
