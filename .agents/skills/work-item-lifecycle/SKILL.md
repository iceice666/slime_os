---
name: work-item-lifecycle
description: Rules for creating, planning, gating, recording evidence for, and closing Slime OS work items in the canonical `.tasks/` store through MyQue and devloop, and for deciding what a PR may carry (planning, implementation, evidence-only, process). Use before running any `just devloop` or `myque` command, before opening a PR that adds or touches `.tasks/`, `.devloop/`, or a checker, when judging backlog-first, or when a review finding cites planning-first or self-grading.
---

# Work-item lifecycle

`AGENTS.md` keeps the four-line summary. This skill is the full rule set.
`CONTRIBUTING.md` is the human-facing walkthrough of the same workflow and
`docs/getting-started/06-work-item-lifecycle.md` runs both lifecycles command
by command; neither overrides what is stated here.

## The store

`.tasks/` is the canonical record of work-item identity, state, hierarchy,
and dependencies, managed by MyQue (https://github.com/mozufu/myque) at the
`work-item/v2` envelope. Active work is `.tasks/items/<UUID>.md`; a done or
cancelled item may be *retired* to a minimal `.tasks/terminal/<UUID>.json`
record after its exact final bytes are retained in Git, and its identity,
relationships, and readiness still resolve offline from that record.

- Canonical identity is the UUID; a cancelled, unknown, or corrupt identity
  is never done.
- Human keys such as `C9.4`, `IO4`, and `B92` are display aliases: optional,
  mutable, and never resolved through by a checker, devlog reference,
  dependency edge, or generated view.
- Do not allocate an id by scanning for the next number, and do not create a
  persistent relationship using a human key. `myque` allocates a UUIDv7
  locally and writes relationships as UUIDs.
- `just tasks_list` / `tasks_next` / `tasks_graph` are generated views, never
  authoritative.

## Spec-driven items

An item's frontmatter is an open record: MyQue owns an enumerated key set and
preserves every other top-level namespace verbatim. `devloop` is this
repository's only consumer namespace.

**Body.** An item that carries `devloop` is *spec-driven*: its body after the
title is exactly one fenced `zti` `dev-spec/v1` payload
(https://github.com/mozufu/devloop) under devloop's `fenced-zti/v1` profile,
and its record carries the profile, pinned schema and helper identities,
requirements digest, admission identity, and evidence. `just tasks_check`
validates those bodies through devloop; `myque check` still owns store
validity. `.devloop/examples/` holds a worked spec;
`docs/decisions/spec-driven-work-item-bodies.md` records what this repository
owns and what it defers.

**Mandatory for new work.** Every item whose canonical UUIDv7 identity is at
or after `2026-10-01T00:00:00Z` must carry a `devloop` record, with no
exemption by kind, tag, or state; `just tasks_check` refuses the store
otherwise. Create such work with `just devloop admit`, never `myque new`.
Items created before that instant — including every legacy `work-item/v1`
item — remain valid prose and never become subject, even if reopened.

**Gates.** Admit, start, gate, and complete spec-driven work with
`just devloop` (`admit`, `validate`, `start`, `gate`, `human`, `eligible`,
`complete`, `render`, all by canonical UUID). Gate identities and their
declared typed observations live in `.devloop/policy.json`, never in
requirement text.

- `just-target` is the general gate: it takes the recipe name from the
  execution inputs (`{"justTarget": "<recipe>"}`) and reports a single
  `passed` boolean, so an acceptance can bind to any published `just` target
  without a bespoke adapter.
- Prefer `just-observations` when a requirement is a count rather than a
  pass: the same recipe runs, its checker reports typed observations through
  `scripts/lib/devloop_observations.py` (cases observed, negative controls
  refused, bytes delivered, stale handles rejected), and the acceptance's
  predicate names the expected number. That number is the planning item's
  *to* state, and the checker that produces it must be under `codePaths`.
- A human gate takes a typed Observation list, not a bare boolean mapping;
  `just devloop human` refuses the wrong shape.

**Closure pinning.** `just tasks_check` refuses a tracked file that no
`codePaths` entry covers unless `check-work-items.py` exempts it by decision,
so a checker, recipe, pin, or fixture cannot change without staling evidence
recorded against it. Execution inputs live in `.devloop/inputs/<item>.json`
and land with the planning item. At `eligible` and `complete`, approval
refuses unless that file, the recipe closure it names, and the scripts and
`scripts/lib` modules those recipes run match `origin/main` byte for byte, so
an implementation branch cannot rewrite the checker that grades it.

The converse binds the planning side. When the work *is* a change to a
checker, recipe, or `scripts/lib` module, that change is a planning change
and lands with the item, and the item's acceptances bind only closures the
implementation will not edit: `tasks_check`, a sibling recipe, or a typed
human gate. An acceptance bound to the closure under repair can never become
eligible.

Each devloop validation compiles and links a native binary, so
`just tasks_check` costs seconds once an item is spec-driven. A direct
`myque close` without `--expected` bypasses devloop's eligibility boundary by
design.

## Backlog first

Backlog defects are the items tagged `backlog`. Resolve, defer, or block every
open one before starting a new track milestone; a green verification suite is
a precondition for milestone work, not a milestone itself. `deferred` means
postponed by decision and `blocked` means waiting on something outside this
repository — both satisfy the rule; `open` and `active` do not.
`just tasks_check` enforces that ordering and `just tasks_next` lists what is
actionable.

A new defect is spec-driven like any other new work: admit it with
`just devloop admit <spec>.zti --title "<title>" --kind bug --tag backlog`,
then close it with `just devloop complete`. A pre-cutoff defect was created
with `myque new` and still closes with `myque close`, which records the
closure date. Either way, record the exit condition that was *observed* in
the item's body — a milestone closes on observed evidence, never on
implementation status alone.

When one PR lands several items, record a `depends` edge wherever one item's
acceptance runs a recipe another item must first repair, so `tasks_next`
reflects the order the evidence can actually be collected in.

## What a PR may carry

`scripts/check/check-pr-scope.py` classifies every PR in CI against the union
of base and head `codePaths`.

| PR class | May carry | Needs |
| --- | --- | --- |
| **planning** | the work item; its exam (inputs, recipe, checker, `scripts/lib` judge); a *new* `contracts/<name>/v<N>/schema.zt` the exam binds to; derived closure/test-run regeneration those force | nothing on `main` first |
| **implementation** | product code, `gen_rust.zt`, generated bindings, system specs, a modified schema | the canonical item already on `main` |
| **evidence-only** | an item's `devloop.evidence` and nothing else | the implementation already on `main` |
| **process** | CI workflows outside `codePaths`, `CONTRIBUTING.md`, `AGENTS.md`, `.agents/`, `docs/`, other paths outside `codePaths` | no item |

- A planning PR never carries product code; the checker refuses the mix.
- The CI, closeout, and retirement workflow files are
  covered by `codePaths`, so their edits stale shared-tool evidence and
  require landed-item ownership.
- Within the product tree or grader closure, only a genuine typo, broken
  link, format-only change, or mechanical maintenance with no behavioral or
  project-state consequence skips the landed-item requirement; judge
  semantics, not line count.
- Never use GitHub auto-close keywords (`Closes`, `Fixes`, `Resolves`) to
  complete work. Work items are not projected into GitHub Issues
  (`docs/decisions/no-github-issue-projection.md`); name the implemented
  item's UUID in the PR description.
- A PR that changes a closure's input set regenerates the closure sources
  in its own derived commit within the same PR (see the generated-code rule
  in `AGENTS.md`). A recipe change regenerates nothing. Test-run records name
  their closure and do not change.

## Evidence and closeout

Do not run `just devloop complete` for a merged implementation:
`myque-closeout.yml` proposes that transition from `main`, trying every
recorded evidence context.

An *evidence-only* PR therefore is a request to close the item whenever it
records fresh passing evidence for every acceptance. Its description names
the base commit and execution identity it claims and states that closeout may
follow. It does not record fresh evidence while declaring that completion is
not claimed; if the intent is only to preserve older observations, do not
include a current-base batch.

A human gate's observations are recorded only with the operator's explicit
authorization; a passing automated run never infers a human confirmation.

**Author restatement is paused.** A separate author-written explanation is
not a merge prerequisite. PRs still state the change, claim, risks, exact
verification, and known limits, and reviewers check those claims against the
diff and observed evidence. Do not invent an author quote or label
agent-written text as "In the author's words". The pause does not waive
planning-first ownership, acceptance evidence, review, CI, or human
authorization.

## History and roadmap

The old frozen backlog index is archived, not a live obligation. New and
reopened defects belong only to the canonical store. Historical evidence uses
the repository identity, full commit, and original path described in
`docs/history.md`; archived task snapshots never participate in current
validation.

`roadmap/` retains explicitly identified unextracted requirements and their
surrounding historical context. Its classification index routes extracted
subjects to `docs/architecture/` and `docs/plans/`; update those owners rather
than maintaining parallel milestone prose. State, hierarchy, and dependencies
belong only to the store.
