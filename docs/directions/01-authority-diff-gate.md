# 1. Authority diff as a build-pipeline gate

| | |
| --- | --- |
| Route | authority |
| Enables | [entry 27](27-policy-carrying-generations.md) (its CI gate becomes a boot gate) |

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

## Design sketch

Builder emits, alongside each generation, a normalized authority view:
per component, the set of (object-kind, rights) pairs derived from the
manifest, in canonical order. A proposed `generation_diff` command compares two
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

## References

- [seL4 capDL](https://docs.sel4.systems/projects/capdl/) — static
  authority description this diff operates over.
