# 1. Authority diff as a build-pipeline gate

| | |
| --- | --- |
| Status | parked |
| Route | authority |
| Depends on | M5.5 (complete: manifest format v2 with 1:1 rights strings) |
| Enables | [entry 27](27-policy-carrying-generations.md) (its CI gate becomes a boot gate) |
| Now | Fully legal host-side tooling: parse two manifests, diff grants, gate CI. No kernel involvement. Named as an M5.5 follow-up in the canonical [roadmap](../../roadmap/README.md). |

## Motivation

Every grant in the system is manifest-declared, so the authority delta
between two generations is computable without running anything: which
component gained which rights on which object kind. Today a rights-widening
change looks identical to any other manifest edit in review. The diff makes
authority growth an explicit, reviewable event: CI requires a sign-off
artifact whenever any component's closure of grants grows, so widening is
never a side effect of an unrelated change.

For agent components this is the primary governance lever. An agent's tool
set changes across generations; without the diff, "the agent can now write
to the object store" hides inside a model-or-prompt update.

## What exists today

- M5.5 landed generation format v2 with machine-readable, 1:1 rights
  strings; `just generation_check` and `just contracts_check` already
  validate deterministic generation output and manifest contracts.
- The rights vocabulary each grant string maps to is defined by
  `../capability-matrix.md`; rights are a flat `u32` with bits 15–31 free.
- The widening/narrowing order the diff needs is exactly what
  [entry 24](24-rights-algebra-model.md) is modeling: derive narrows only,
  no transfer widens. The diff's definition of "grow" should reuse that
  algebra rather than inventing a syntactic one.
- [entry 9](09-grant-graph-introspection.md) builds the general query
  engine; the diff is two engine snapshots plus a set difference.

## Design sketch

Builder emits, alongside each generation, a normalized authority view:
per component, the set of (object-kind, rights) pairs derived from the
manifest, in canonical order. `just generation_diff A B` compares the two
views and prints per-component additions and removals, where an addition
is any pair not implied by the old closure under the narrow-only algebra —
adding `READ|WRITE` where only `READ` existed is widening; splitting a
grant into two narrower ones is not.

The CI gate: a build that widens any component's rights fails unless the
tree carries a sign-off artifact naming the widened component, the added
rights, and a reason. The artifact is versioned with the generation so the
audit trail survives rollback: rolling back to an older generation also
rolls back to its (narrower) authority view, which is the correct
semantics.

Diff granularity is per component identity from the manifest, not per
task; the debt register notes component identity must come from the
manifest in format v2, which this consumes.

## Open questions

- Is the sign-off artifact a file in the repo, or metadata attached to the
  generation object (and later covered by entry 23's provenance)?
- How are renamed components matched across generations — by manifest
  identity only, or is a rename itself a flagged event?
- Should removal of rights also require sign-off when it could break a
  downstream component's declared dependency, or is silent narrowing
  always safe?

## Exit-condition sketch

`just generation_diff A B` prints per-component grant changes; a build
that widens rights without the sign-off file fails.

## Probe guidance

Not needed as a probe: dependencies are landed and the work is ordinary
host tooling. Promote directly with the exit condition above once
[entry 24](24-rights-algebra-model.md) fixes the widening definition the
diff compares against, or implement against the syntactic definition and
reconcile when 24 lands.

## References

- [seL4 capDL](https://docs.sel4.systems/projects/capdl/) — static
  authority description this diff operates over.
