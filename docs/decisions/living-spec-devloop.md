# Living specs replace work items, devloop and the closeout bot

**Status:** Accepted
**Decided by:** the project's human owner, 2026-11-07, delegating the option
choices to the agent (`proposal/devloop-redesign/decisions.md`)
**Supersedes:** the records *Spec-driven work-item bodies*, *Mandatory
spec-driven work items* and *No GitHub issue projection* — all three removed
with the store they described; their text is at the commit `specs/legacy.zt`
names as `frozenAt`, under `docs/decisions/`.

## Context

Of the last 200 commits on `main`, 123 were `repo/tasks` bookkeeping: evidence
PRs, closeout PRs, retirement batches. A feature took four PRs on average. A
grader repair needed its own work item. Evidence lived in the tree, bound to a
code-identity digest over 312 paths and a 24-hour TTL, so it re-recorded five
times per feature. Three external tools (MyQue, devloop, myque-gh) owned the
formats, and the CI that ran them was GitHub-specific.

The redesign keeps five invariants the old loop protected and drops every
mechanism that was only a means:

| # | Invariant | Now kept by |
| --- | --- | --- |
| 1 | done is decided by a machine, never claimed | there is no done state; `scripts/spec/status` computes each acceptance from records on the `evidence` branch |
| 2 | the code under test cannot change its own grader | `scripts/spec/check --base`: a change inside a bound target's grader set, an invariant, or an existing contract schema needs an approval from `HUMAN_OWNERS` |
| 3 | evidence binds to an exact input closure | records are keyed by a closure digest over recipe definitions, grader scripts, image derivation ids and toolchain pins; no TTL |
| 4 | people decide scope, invariants and known limits | `invariants`, `decisions` and `openQuestions` live in the area file; `nonGoals` too |
| 5 | work tracking is offline, in the repository, UUID-identified | requirement ids are UUIDv7 in `specs/<area>.zt`; `specs/legacy.zt` indexes the retired store |

## Decision

- `specs/<area>.zt` is a living document of what one plane or subsystem
  promises. The unit of work is a requirement inside it. A feature is one PR
  carrying the spec diff, the exam and the implementation.
- An exam is an ordinary `just` target. `scripts/spec/record` wraps it and
  writes an `observation/v1` record keyed by the target's closure digest.
- Records live on the orphan branch `evidence`, written by the Evidence
  workflow after every push to `main`, never in the tree. Human answers and
  operator-run hardware records are signed with `ssh-keygen -Y sign` against
  `specs/allowed_signers`.
- Status has four values per acceptance: enforced, failing, claimed,
  advisory. The backlog is `scripts/spec/status --unenforced` plus `roadmap/`.
- `.tasks/`, `.devloop/`, MyQue, devloop, myque-gh, the PR classes, the
  closeout and retirement workflows and their checkers are removed.

## Alternatives considered

- **`refs/notes/evidence` instead of a branch.** Rejected: branch protection
  does not cover notes, and a lookup must walk history instead of one path.
- **One spec file per work item.** Rejected: done requirements freeze in
  separate files and "what the system promises" needs replaying N files. The
  item model is a special case of the living spec; the reverse migration is
  not.
- **`specs/legacy/` keeping the old format.** Rejected: every future reader
  would need two parsers. The index keeps identity; the full text stays in Git.
- **Product closure as one digest over the whole product tree.** Rejected as
  the default: every product change would re-run every plane. QEMU planes use
  the derivation id of the image they boot; other targets declare
  `[metadata("inputs", ...)]`; a target with neither falls back to the coarse
  product tree digest and reports `coarse`.
- **Host-native approval rules alone.** Rejected: an agent account can
  approve another agent. The workflow passes approver logins; `check --base`
  requires one from `HUMAN_OWNERS`.

## Consequences

- The first full evidence run re-examines every requirement rehomed from a
  done item. A grader that rotted reports `failing`; the item model never
  did this.
- Shared verification libraries sit in most closures, so editing one asks
  for a human approval. That is where a person should look.
- A planned requirement rehomed from an open item carries `note`; its
  acceptances are not bound until an exam exists.
- Rewording a requirement's statement or acceptance invalidates its signed
  human records; rewording a decision or invariant does not.

## Revisit when

- the approval rate on pull requests exceeds roughly one in three;
- one area file grows past about sixty requirements (merge by
  `docs/architecture/` page instead of by plane);
- a reader needs evidence without the CLI (`status --json` is the interface).
