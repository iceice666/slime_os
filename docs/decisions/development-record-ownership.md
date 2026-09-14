# Development record ownership

**Status:** Accepted
**Related work item:** `01a09f86-10dc-73fd-8b6e-d127999945d0`

## Context

Ordinary changes were recorded in commits, pull requests, canonical MyQue items,
owning documentation, and a mandatory chronological devlog entry. The duplicate
record increased review cost and made current knowledge harder to distinguish
from the history of how it was discovered. Removing the devlog requirement must
not weaken evidence-based completion or make historical evidence mutable.

## Decision

Commits and pull requests record the change, claim, risk, review surface, exact
verification, and known limits. The canonical work item records scope, state,
relationships, and the exit conditions actually observed. Current contracts,
operation, and limitations live in their owning documentation or schema.

A separate decision record is reserved for important long-lived cross-module
choices. It states its status, context, decision, alternatives and trade-offs,
consequences, revisit conditions, and relevant work-item UUIDs and code
references. Ordinary changes do not require one.

The existing `devlog/` corpus remains immutable historical evidence during the
archive transition. Expensive reusable investigations and unusual verification
campaigns may still add a curated entry, but a devlog is not required for a
feature, bug fix, milestone completion, or work-item closure.

## Alternatives and trade-offs

- **Keep mandatory devlogs:** preserves one uniform chronology, but duplicates
  active records and keeps product work coupled to historical formatting.
- **Put every decision in PR text:** minimizes files, but loses a stable owner
  for choices that must survive branch and review-system changes.
- **Make the history repository immediately authoritative:** reaches the target
  layout sooner, but would create an unverified dependency before archive bytes,
  permissions, links, and restoration are proven.

The chosen split keeps routine work direct while retaining a durable record only
where future maintainers need the rationale independently of one implementation.

## Consequences

- A normal non-trivial change can pass repository policy without adding a
  devlog entry.
- Evidence requirements remain strict: direct and inherited evidence are
  distinguished, inference is labeled, and hardware or image claims identify
  the tested target and binary.
- Existing devlog entries, raw artifacts, links, and checker rules remain in
  place until the verified archive cutover.
- `docs/decisions/` is not a tracker or a per-PR diary. MyQue remains the only
  live work-state store.

## Revisit conditions

Revisit this split if reviewers cannot determine a change's claim and evidence
from the PR plus work item, if cross-module decisions repeatedly lack a stable
owner, or if the history archive cannot preserve immutable evidence without
becoming a product build or test dependency.

## References

- `AGENTS.md` — repository-wide record and evidence rules.
- `CONTRIBUTING.md` — contributor and pull-request workflow.
- `scripts/check/check-devlog.py` — retained corpus validation and active-policy
  link/UUID checks during the transition.
- `01a09f85-6721-791c-b8a1-11c007a35391` — parent repository-split epic.
