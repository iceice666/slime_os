---
name: change-records
description: Where a Slime OS change is recorded and documented — what the commit, PR, and work item each own; when a decision record, plan, or direction is warranted and what it must contain; what belongs in an implementation comment versus owning docs; and where historical investigations live. Use when deciding whether to write a devlog or decision record, when updating `docs/architecture/`, `docs/decisions/`, `docs/plans/`, or `docs/directions/`, when writing or reviewing code comments, or when a review finding says the owning documentation was not updated.
---

# Change records and documentation

`AGENTS.md` keeps the two-sentence summary. This skill is the full rule set.
The accepted ownership split is recorded in
`docs/decisions/development-record-ownership.md`.

## What each record owns

| Record | Owns |
| --- | --- |
| commit and PR | the change, claim, risk, review surface, exact verification, known limits |
| canonical work item | scope, state, and the exit conditions actually observed |
| `docs/architecture/` | current subsystem rationale and the boundary as it is |
| `docs/decisions/` | long-lived cross-module choices that need a durable record |
| `docs/plans/` | unimplemented designs and qualification requirements |
| `docs/directions/` | exploratory directions |
| beside the implementation | local invariants |
| `slime_os-history` archive (`docs/history.md`) | historical investigations and raw evidence |

Ordinary features, bug fixes, refactors, and milestone completions do not
require a separate devlog entry. Distinguish direct observations from
inherited evidence and unobserved inference, and bind hardware or image
claims to the target, revision, and binary identity that was tested.

## Owning documentation

Update the owning current documentation when a contract, operating
procedure, boundary, or limitation changes. A behavior change at a contract,
wire, or operating boundary updates the owning `docs/architecture/` page in
the same PR; a diff that reverses what that page states (a message the page
says is refused that the code now accepts) is incomplete, and a reviewer
reports it as a non-blocking finding against the PR.

`docs/task-to-file.md` names the owning page per kind of change; read the
row before searching.

## Decision records

A decision record states:

- context;
- the decision;
- alternatives and trade-offs;
- consequences;
- revisit conditions;
- status: `proposed`, `accepted`, or `superseded`;
- relevant work-item UUIDs and code references.

Moving an old proposal never makes it accepted. A decision record landed by
a planning PR is `proposed` until the implementation and its evidence land.

## Implementation comments

A comment should state what must remain true, not argue that the author was
right.

Keep implementation comments for:

- correctness or safety invariants not evident from types or code;
- ownership and ordering requirements;
- ABI constraints;
- capability or security boundaries;
- platform constraints;
- why an apparently simpler local implementation would violate a current
  invariant.

Do not put in implementation comments: investigation history, failed
approaches, reviewer findings, mutation campaigns, historical alternatives,
speculative designs, verification results, or narration of the code.

Place local invariants beside the implementation and stable subsystem
rationale in the owning `docs/` or contract documentation. Prefer one to
three precise sentences over defensive paragraphs. Historical investigation
records explain how an older conclusion was reached and are referenced
rather than duplicated.

## Historical investigations

Historical investigations and raw evidence live in the private
`slime_os-history` archive described in `docs/history.md`. Preserve observed
results and bytes; new exceptional investigations belong there, not in the
product tree. Ordinary changes require neither an investigation nor a
complete transcript. Product checks validate current local knowledge, not
archived entries against today's recipes or tasks.
