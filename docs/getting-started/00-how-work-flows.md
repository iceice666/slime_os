# How work flows

One page for a person who has not yet internalised the repository's habits.
Every rule below is stated once, with the reason it exists and the program
that enforces it; the pages it links to own the details. If you already know
why a grader change needs a human approval, skip to the
[decision tree](#which-path-am-i-on).

## The one idea behind all of it

Project state is data that programs check, not prose that people remember.
What the system promises lives in `specs/<area>.zt`; what has been observed
lives on the `evidence` branch; `just spec_status` computes the difference.
No person and no agent writes "done". Almost every convention that looks like
ceremony is there so one of those checks can be honest:

| Convention | What would break without it |
|---|---|
| A behavior change changes the spec | The promise and the code drift apart with nothing to notice |
| Exam, spec diff and implementation in one PR | Reviewers read a diff without the promise it serves |
| A grader change needs a human approval | The branch being graded could rewrite its own exam |
| Merge is not completion | A landed diff proves nothing about a QEMU run, a hardware boot, or a refusal |
| Evidence keyed by a closure digest | A record would outlive the inputs it was taken on |
| Closures regenerated with every source edit | A gate would prove a binary nobody can rebuild |

The rest of this page is the timeline those conventions produce.

## The timeline

```text
  idea / bug report
        │
        ▼
  ① read the backlog ───── just spec_status --unenforced; failing first
        │
        ▼
  ② write the requirement ─ specs/<area>.zt: h.req ... { h.observedWhere ... }
        │
        ▼
  ③ write the exam ──────── a just target; run it under just spec_record <target>
        │
        ▼
  ④ implement ───────────── edit the owning source; regenerate closures; just spec_status --local
        │
        ▼
  ⑤ one PR ──────────────── spec diff + exam + implementation; CI judges the grader set
        │
        ▼
  ⑥ evidence ───────────── CI on main runs claimed targets, writes the evidence branch
```

### ① Read the backlog

`just spec_status --unenforced` lists every requirement that is not
enforced: failing, claimed, or advisory. A `failing` requirement in an area
is fixed, or marked `planned` with a note, by the next PR that touches that
area.

- **Why:** a feature built on a red suite closes on evidence nobody can
  distinguish from the pre-existing failures.
- **Enforced by:** the review order in the
  [spec-review skill](../../.agents/skills/spec-review/SKILL.md); the number
  is always visible.

### ② Write the requirement

Open the area file that owns the behavior and add, rewrite or remove
requirements. A requirement is one sentence of promise, a rationale, and
acceptances that name a `just` target and the counts it must report.

- **Why:** a promise written as data can be checked; prose can only be
  asserted. The target is named in the acceptance, so the same requirement
  cannot be quietly re-pointed at an easier check without a spec diff.
- **Enforced by:** `just spec_check` refuses an id that is not a UUIDv7, a
  target `just` does not declare, a non-planned requirement without an
  automated acceptance.
- **Read:** [Carrying a change through the living specs](06-spec-lifecycle.md)
  shows the helpers.

### ③ Write the exam

An exam is an ordinary recipe. A checker reports typed observations;
`scripts/spec/record` turns them into the record's counts and takes
`passed` from the exit status.

- **Why:** a checker cannot vouch for itself, and a count the acceptance
  pins (exactly four refusals, at least three controls) is harder to fake
  than an exit code.
- **Read:** [Add a gate](07-add-a-gate.md); the
  [verification-checkers skill](../../.agents/skills/verification-checkers/SKILL.md)
  owns negative controls.

### ④ Implement

Route the change through [`docs/task-to-file.md`](../task-to-file.md) — read
the named module root, not a grep across the tree — and edit the owning
source. Two things catch most newcomers:

- **Generated outputs are not sources.** Anything beginning with
  `@generated`, `boot-contracts/src/generated/`, derived manifests, and the
  system-image closures are outputs of a `schema.zt`, a system spec, or a
  generator. Change the source and rerun the generator
  ([AGENTS.md](../../AGENTS.md#generated-code-rule) lists them).
- **Closures name their inputs; the checkout supplies the digests.** A closure
  source under `contracts/system-image-closure/v3/closures/` names each input
  tree by path only. Every seL4 gate resolves it in your checkout, so an edit to
  root or component code needs no regeneration. Rerun
  `python3 scripts/generate/generate-system-image-closures.py` only when a
  closure's input set changes, then run the narrowest gate that exercises the
  change. [Your first change](04-first-change.md) shows what a resolved build
  records.

Finish with the repository-wide checks that apply: `just fmt_check_all` and
`just lint_all` for Rust, `just docs_check` and `just typos` for
documentation, `just spec_check` if `specs/` changed, and
`just spec_status --local` to see the requirement enforced from your own
record before anyone else does.

### ⑤ One PR

Use the [change](../../.github/PULL_REQUEST_TEMPLATE/change.md) or
[system-change](../../.github/PULL_REQUEST_TEMPLATE/system-change.md)
template. Its `Verification` table wants exact commands, the observed result,
and an evidence class: *direct* (you ran it), *inherited* (a prior recorded
run), or *inference* (not observed). Name the requirement ids the PR adds
or changes.

- **Judged by:** `scripts/spec/check --base`. Changing an area's
  `invariants`, an existing `contracts/**/schema.zt`, or a file in the grader
  set of any bound target needs an approval from a login in `HUMAN_OWNERS`.
  Splitting the grader change into its own PR does not help: the rule judges
  every bound target at the base.
- **Never** write `Closes #123` to complete work: GitHub does not own
  completion here.

### ⑥ Evidence

After the merge, the Evidence workflow resolves every bound target's closure
digest, runs the targets with no record at that digest, and commits the
records to the `evidence` branch. Nothing lands on `main`.

- **Why:** evidence is bound to the recipe definitions, the grader scripts,
  the image derivation ids and the toolchain pins. Change any of them and the
  record no longer applies; the target runs again.
- **Human and hardware:** an `h.human` acceptance waits for
  `scripts/spec/human ... --signer <login>`; a target CI cannot run waits
  for its operator's `scripts/spec/record <target> --signer <login>`. The
  signature, verified against `specs/allowed_signers`, is what status trusts.

## Which path am I on?

```text
Is the change a genuine typo, broken link, format-only edit, or mechanical
maintenance with no behavioural or project-state consequence?
├─ yes → TRIVIAL: no spec diff; open the PR; applicable checks still run.
│        Judge semantics, not line count. When unsure, it is not trivial.
└─ no
   ├─ Does a planned requirement already promise it?
   │  └─ yes → bind it: replace h.planned with h.req and its acceptances; ③ ④ ⑤.
   ├─ Is it a defect in something enforced?
   │  └─ yes → BUG: tighten or add the acceptance that would have caught it,
   │            see it fail, then ④ ⑤. The spec diff is the regression test's name.
   └─ otherwise → FEATURE / REFACTOR / DOCS: the full ② – ⑤.
```

Two cases people misfile:

- *"Substantial documentation"* is not trivial. A page that states a
  procedure or a limitation is project state; it changes with the spec or the
  architecture page that owns it.
- *A checker that needs to change* is a grader change: it asks for a human
  approval in ⑤, and the approval is the decision, not a formality.

## Working with an agent

Much of this repository's day-to-day work is done by telling a coding agent
what you want and letting it follow the conventions above.
[AGENTS.md](../../AGENTS.md) is the compressed, agent-facing statement of
the same rules plus the code map; it is written for density, not for a
first read. You do not need to read it to contribute — this page and
[CONTRIBUTING.md](../../CONTRIBUTING.md) are the human versions — but it is
worth knowing what an agent is doing when it goes quiet after "add X":

1. It runs `just spec_status --unenforced` and reads what is failing (①).
2. It finds the area that owns the behavior, writes or binds the requirement
   and the exam (② ③).
3. It routes through `docs/task-to-file.md`, edits the owning source,
   regenerates closures, and runs the narrowest gate under `spec_record` (④).
4. It fills the PR template with a verification table and separates what it
   observed from what it inferred (⑤).

Review an agent's work against that list, not against the diff alone. The
usual failure modes are the same as a person's: editing a generated file,
changing behavior without a spec diff, presenting a green unrelated gate as
evidence, or calling a merged PR done. The checks catch most of these; the
ones they do not catch are ⑤'s evidence classes and the spec diff's honesty.

## Where the details live

| Question | Page |
|---|---|
| The rules that bind a PR, exactly | [CONTRIBUTING.md](../../CONTRIBUTING.md) |
| The spec helpers, the CLI, and what status means | [Carrying a change through the living specs](06-spec-lifecycle.md) |
| Which file owns my change | [Task-to-file index](../task-to-file.md), [Recipes](11-recipes.md) |
| What a gate is and how to add one | [Add a gate](07-add-a-gate.md) |
| Why living specs replaced work items | [Living specs replace work items](../decisions/living-spec-devloop.md) |
| Why the commit, spec, and docs own different records | [Development record ownership](../decisions/development-record-ownership.md) |
| A term I do not recognise | [Glossary](../glossary.md) |
