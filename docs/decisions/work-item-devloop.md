# Work-item devloop v1.5

**Status:** Accepted
**Related work item:** `devloop-v1.5-bootstrap` (the last V1 item; its UUID is in `.tasks/items/`)
**Supersedes at cutover:** [`mandatory-spec-driven-work-items.md`](mandatory-spec-driven-work-items.md)
and [`spec-driven-work-item-bodies.md`](spec-driven-work-item-bodies.md)

## Decision

v1.5 keeps the four V1 stages: planning, implementation, evidence and
closeout. It keeps the V1 pull-request classes. It changes only the six cost
sources V1 measured, listed below. Only one ratchet ships, R4. R1, R2, R3 and
the `Devloop-Role` trailer are not built. A grader change needs an approval
from `HUMAN_OWNERS` (V4).

Several pull requests per feature are deliberate. The planning pull request
merges first, so the spec and the exam reach `main` before the implementation.

## Premises

- People may lead development, or agents may. Every rule must hold in both
  modes. A rule that holds in one mode only states when it degrades and what
  replaces it.
- After the move to a private GitLab, another team continues development.
  That team has none of the earlier sessions' context. The rules must be
  readable from this record and the validator alone.
- Speed is not the first goal. Three pull requests per feature are
  acceptable. Re-records, bookkeeping commits and fields nobody reads are not.
- The validator uses no GitHub-only API. Every host dependency is one of the
  five functions in `scripts/spec/forge.py`.

## What V1 cost, and what v1.5 does

| V1 cost | Measured | v1.5 |
| --- | --- | --- |
| `repo/tasks` bookkeeping in the last 200 commits | 123 | Evidence goes to the `evidence` branch, not to a pull request. Closeout is one pull request per feature. |
| Evidence re-records per feature | about 5 | A record is keyed by its target's closure digest. There is no 24-hour TTL. |
| Paths evidence binds | 312 `codePaths` | Each target binds its own grader set. |
| Item body median | 8 KB, about 70% encoding | `task/v2` Zutai items built from helpers. |
| Behavior specs hidden in closed items | structural | `tasks_spec` derives the current promises from area files and terminal items (M4). |
| Fixing one grader | a new work item | A planning pull request edits an open item's exam directly. |

## Rules

`scripts/spec/check --base <sha>` is the only judge. Its inputs are the base
commit, the head checkout, the evidence commit, the pull request's author and
the approvals on the current head. Equal inputs give an equal verdict. The
judge's code comes from the base commit, so a pull request cannot change the
judge that judges it.

| Rule | Refuses |
| --- | --- |
| V1 class | A pull request with more than one class, or a path the class table does not list. |
| V2 planning first | An implementation path that no `owns` covers of an item that is on base and `#active` on head. |
| V3 no self-grading | An implementation pull request that touches the grader set of any bound target. |
| V4 grader approval | A grader-set change, an area invariant change, an existing contract schema change, or a change to `scripts/spec/`, `HUMAN_OWNERS`, `AGENT_ACCOUNTS` or `specs/allowed_signers`, without one approval on the head from `HUMAN_OWNERS` by someone other than the author. Any pull request while `HUMAN_OWNERS` lists fewer than two people. |
| V5 legal intent | An `intent` change outside `EDGES`. Entering `#deferred` or `#cancelled` without a V4 approval. |
| V6 closeout | A move to `terminal/` without a signed done witness, unless the item is `#cancelled`. Any later change to a terminal item. |
| V7 backlog first | A milestone, or an item under one, entering `#active` while a backlog bug is `#planned` or `#active`, or while a promise fails. |
| V8 R4 floor (M5) | A lowered `negativeControlsRefused` predicate, without an approved R4 known limit. |
| V9 provenance | A `#human` quote shorter than 8 characters or a blanket answer, or a source that is a pull request number. Warning until three real features close. |
| V10 frozen promises | An added, changed or removed area requirement. An item's `supersedes` replaces one instead. |

A **bound target** is one that an item in `.tasks/items/` that is not
cancelled, a terminal item nobody superseded, or an area requirement binds.
A target **bound by a promise** is one of the last two kinds only. A
planning pull request edits the exam of an open item directly. An exam bound
by a promise changes only with a new item. Both need a V4 approval.

`#planned` items are never recorded. The evidence workflow claims a target
only once its item is `#active`, so `main` never carries an expected failure.
A failing promise counts as an open backlog bug for V7. It blocks milestones,
not unrelated pull requests.

## Who is who

`HUMAN_OWNERS` lists the people who may approve: `iceice666`, `LmanTW` and
`CG-AA`. At the GitLab move, it lists at least two people of the team that
continues. `AGENT_ACCOUNTS` lists the agent accounts. The validator tells a
person from an agent by the pull request's author account, never by a
trailer.

On GitHub, agents have no account of their own, and `AGENT_ACCOUNTS` is empty.
The person whose account an agent session pushes with is the author, and
cannot approve that pull request. This is a known limit until M6.

## Until cutover

V1 judges `main` until cutover: `just tasks_check`, `check-pr-scope.py` and
`just devloop`. Two judges that both block would make every pull request
satisfy two classifications. So M0 to M3 land under V1 rules, as one V1 item,
`devloop-v1.5-bootstrap`. Its exam is `just spec_e2e`. `scripts/spec/check`
runs in CI from M2 and reports without blocking.

Before cutover, `task/v2` items exist only in the `spec_e2e` fixture.
`.tasks/items/` holds no `.zt` file.

## Consequences

- Cutover removes the `myque` and `devloop` pins, `.devloop/`, the MyQue
  closeout and retirement workflows, `check-pr-scope.py`,
  `check-work-items.py` and the work-item-lifecycle skill. It makes
  `scripts/spec/check` a required check.
- The closeout bot needs its own token: a GitHub App or machine user now, a
  GitLab project access token after the move. A pull request opened with the
  workflow's built-in token triggers no checks, so V6 would never run.
- The evidence signing key stays in the `evidence` environment, which only
  `main` pushes reach. A key rotation adds a line to `specs/allowed_signers`
  and keeps the old one, so old records still verify.
- A pull request can edit the workflow that runs the judge. Before `check`
  blocks, the host must run it from a trusted definition: a ruleset-required
  workflow or `pull_request_target` on GitHub, a protected pipeline
  definition on GitLab.

## Revisit when

Process changes only when one of these holds, or after three real features:

1. Fewer than 2 of the last 10 grader approvals asked for a change. V4 has
   become a stamp: V8 becomes the main defence.
2. Accounts in `AGENT_ACCOUNTS` open more than 70% of implementation pull
   requests, measured from M6. Switch to agent-led mode and re-evaluate R2.
3. A terminal item and `tasks_spec` disagree on one behavior. `supersedes`
   is not enough; re-evaluate the area freeze.
4. One feature costs more than one bookkeeping commit. Inspect the closeout
   bot or the evidence workflow.

## Alternatives not taken

- Record a failing run right after planning merges, to show the exam
  discriminates. Each planning merge would cost a QEMU run, and most new
  targets fail at build time, so the signal is weak.
- A typed observation from the approver. The count of requested changes
  measures the same thing without a ritual for the next team.
- Measuring control strength. That is R2's scope.
