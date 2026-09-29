# How work flows

One page for a person who has not yet internalised the repository's habits.
Every rule below is stated once, with the reason it exists and the program
that enforces it; the pages it links to own the details. If you already know
why a planning PR lands before an implementation PR, skip to the
[decision tree](#which-path-am-i-on).

## The one idea behind all of it

Project state is data that programs check, not prose that people remember.
The canonical record of *what work exists, what state it is in, and what
counted as finishing it* lives in `.tasks/` under
[MyQue](https://github.com/mozufu/myque), and a growing set of checks —
`just tasks_check`, [devloop](https://github.com/mozufu/devloop) approval,
CI — refuse a tree that contradicts that record. Almost every convention that
looks like ceremony is there so one of those checks can be honest:

| Convention | What would break without it |
|---|---|
| Work item before implementation | Nothing to bind evidence or a PR to; "done" becomes an opinion |
| Planning PR merges before implementation PR | The branch being graded could rewrite its own exam (see below) |
| Backlog defects first | A milestone built on a known-red suite records evidence nobody trusts |
| Merge is not completion | A landed diff proves nothing about an exit condition that needs a QEMU run, a hardware boot, or a refusal |
| Closures regenerated with every source edit | A gate would prove a binary nobody can rebuild |

The rest of this page is the timeline those conventions produce.

## The timeline

```text
  idea / bug report
        │
        ▼
  ① check the backlog ─── open backlog defects?  → resolve, defer, or block them first
        │
        ▼
  ② admit a work item ─── just devloop admit spec.zti ...   (.tasks/items/<UUID>.md)
        │
        ▼
  ③ planning PR ────────── .tasks/ + .devloop/inputs/ only; merges to main
        │
        ▼
  ④ start & implement ─── just devloop start <UUID>; edit; regenerate closures; run the gate
        │
        ▼
  ⑤ implementation PR ─── template; myque-gh pr link; CI green; merge
        │
        ▼
  ⑥ record evidence ────── just devloop gate / human ...; just devloop complete <UUID>
        │
        ▼
  ⑦ (later) retire ─────── maintenance PR moves the done item to .tasks/terminal/
```

### ① Check the backlog

Items tagged `backlog` are known defects. Every open one is resolved,
`deferred` (postponed by a recorded decision), or `blocked` (waiting on
something outside this repository) before new milestone work starts.

- **Why:** a milestone that starts on a red suite closes on evidence nobody
  can distinguish from the pre-existing failures.
- **Enforced by:** `just tasks_check` refuses the store; `just devloop start`
  refuses while an open backlog defect exists.
- **Look:** `just tasks_next` lists what is actionable.

### ② Admit a work item

Every non-trivial change implements a canonical item. From
`2026-10-01T00:00:00Z` every new item is *spec-driven*: its body is a
`dev-spec/v1` requirements payload, and its acceptances name gates from
[`.devloop/policy.json`](../../.devloop/policy.json).

```sh
just devloop admit spec.zti --title "Describe the work" --admission <token> --policy .devloop/policy.json --kind task
echo '{"justTarget": "<recipe>"}' > .devloop/inputs/<UUID>.json
just tasks_check
```

- **Why:** an item written as data can be *checked* for completion; prose
  can only be asserted. The recipe that grades the work is named in the
  inputs file, never in requirement text, so the same requirement cannot be
  quietly re-pointed at an easier check.
- **Enforced by:** `just tasks_check` refuses a post-cutoff item without a
  `devloop` record, and refuses a tracked file no `codePaths` entry covers.
- **Never:** invent a UUID, pick "the next number" for a human key, or use
  `myque new` for new work. Human keys such as `B92` are display aliases;
  every persistent reference is the UUID.
- **Read:** [Carrying a work item](06-work-item-lifecycle.md) walks the
  commands; [`.devloop/examples/`](../../.devloop/examples/) is a complete
  worked spec.

### ③ Planning PR first

Open a PR that contains only the item and its inputs file, and merge it
before opening an implementation PR.

- **Why:** at `eligible` and `complete`, devloop approval refuses unless the
  inputs file, the recipe it names, every recipe in that recipe's dependency
  chain, the scripts they run, and the `scripts/lib` modules those import are
  byte-for-byte what `origin/main` carries. The exam has to be on `main`
  before the answer is graded; an implementation branch may add checkers for
  *other* work but cannot rewrite the one that decides its own acceptance.
  Combining item and implementation in one PR would defeat that.
- **Enforced by:** `scripts/check/devloop-approval.py`, run inside
  `just devloop start | eligible | complete`.
- **Scope:** one landed item can cover several implementation PRs within its
  scope; a new planning item is not required for every PR.

### ④ Start and implement

```sh
just devloop start <UUID> --policy .devloop/policy.json
```

Then route the change through [`docs/task-to-file.md`](../task-to-file.md)
— read the named module root, not a grep across the tree — and edit the
owning source. Two things catch most newcomers:

- **Generated outputs are not sources.** Anything beginning with
  `@generated`, `boot-contracts/src/generated/`, derived manifests, and the
  system-image closures are outputs of a `schema.zt`, a system spec, or a
  generator. Change the source and rerun the generator
  ([AGENTS.md](../../AGENTS.md#generated-code-rule) lists them).
- **Closures record source identity.** Every seL4 gate first checks that the
  source trees it will build match the recorded digests under
  `contracts/system-image-closure/v2/closures/`. After editing root or
  component code, run
  `python3 scripts/generate/generate-system-image-closures.py`, then the
  narrowest gate that exercises the change. [Your first change](04-first-change.md)
  makes you collide with this on purpose.

Finish with the repository-wide checks that apply: `just fmt_check_all` and
`just lint_all` for Rust, `just docs_check` and `just typos` for
documentation, `just tasks_check` if `.tasks/` changed.

### ⑤ Implementation PR

Use the [change](../../.github/PULL_REQUEST_TEMPLATE/change.md) or
[system-change](../../.github/PULL_REQUEST_TEMPLATE/system-change.md)
template. Its `Verification` table wants exact commands, the observed result,
and an evidence class: *direct* (you ran it), *inherited* (a prior recorded
run), or *inference* (not observed). After opening, link the PR to the item
with the pinned `myque-gh pr link` invocation from the template's source
comment; the tool writes a trailer you never hand-edit.

- **Never** write `Closes #123` / `Fixes #123` / `Resolves #123` against a
  bot-projected Issue: GitHub does not own completion here, and the
  auto-close would lie.
- **Why the ceremony:** the projected Issue, the milestone, and the
  "implements" link are all derived from `.tasks/` and this trailer by
  `myque-gh`; a hand edit is overwritten or, worse, becomes a second source
  of truth.

### ⑥ Record evidence, then complete

Merging landed the diff. The item closes only when its acceptances hold
against evidence recorded on the exact inputs that were tested:

```sh
just devloop gate  <UUID> <ACCEPTANCE> --policy .devloop/policy.json --inputs .devloop/inputs/<UUID>.json --target <target> --image <image>
just devloop human <UUID> <ACCEPTANCE> ... --observations <observed>.json --observer <who>   # operator-only obligations
just devloop complete <UUID> --policy .devloop/policy.json --inputs .devloop/inputs/<UUID>.json --target <target> --image <image>
```

- **Why:** evidence is bound to the requirements digest, code closure, policy,
  inputs, target, image, and run epoch. Change any of them and it no longer
  applies; the newest entry for an obligation decides it, so a later failing
  run is never rescued by an earlier pass. A human obligation (a physical
  Framework boot, for example) stays pending until an operator records a
  real observation — nothing synthesises one.
- **Enforced by:** `just devloop complete` refuses while any mandatory
  obligation is unmet. `myque close` is the pre-cutoff path and bypasses this
  by design; never use it on a spec-driven item.
- You record the gate evidence on the branch; `complete` runs on `main`.
  After the implementation PR merges, `myque-closeout.yml` runs
  `just devloop complete` there for every item devloop accepts and opens a
  `.tasks`-only PR with the transition; merge it and the projector closes
  the Issue. Details in
  [Carrying a work item](06-work-item-lifecycle.md#closeout-prs).

### ⑦ Retire (optional, later)

A done item may leave the live tree for a minimal
`.tasks/terminal/<UUID>.json` record through a maintenance PR, after its
final bytes are retained in Git. Retirement is storage housekeeping, proves
nothing, and is covered in [Carrying a work item](06-work-item-lifecycle.md#retirement-maintenance-prs).

## Which path am I on?

```text
Is the change a genuine typo, broken link, format-only edit, or mechanical
maintenance with no behavioural or project-state consequence?
├─ yes → TRIVIAL: no item; open the PR; applicable checks still run.
│        Judge semantics, not line count. When unsure, it is not trivial.
└─ no
   ├─ Is there an open item on main whose scope already covers it?
   │  └─ yes → EXISTING: skip ② ③; `just devloop start` (or `myque start`
   │            for a pre-cutoff item), then ④ ⑤ ⑥ under that UUID.
   ├─ Is it a defect?
   │  └─ yes → BUG: ② with `--kind bug --tag backlog`, then ③ ④ ⑤ ⑥.
   │            It joins the backlog, so it blocks new milestone work until
   │            resolved, deferred, or blocked — which is the point.
   └─ otherwise → FEATURE / REFACTOR / DOCS: the full ① – ⑥.
```

Two cases people misfile:

- *"Substantial documentation"* is not trivial. A page that states a
  procedure or a limitation is project state; it takes an item.
- *A checker that needs to change* is a planning change, not an
  implementation detail, because it is the exam. Land it in ③.

## Working with an agent

Much of this repository's day-to-day work is done by telling a coding agent
what you want and letting it follow the conventions above.
[AGENTS.md](../../AGENTS.md) is the compressed, agent-facing statement of
the same rules plus the code map; it is written for density, not for a
first read. You do not need to read it to contribute — this page and
[CONTRIBUTING.md](../../CONTRIBUTING.md) are the human versions — but it is
worth knowing what an agent is doing when it goes quiet after "add X":

1. It runs `just tasks_next` and reads the `backlog`-tagged items (①).
2. It looks for an existing item covering the request; failing that it
   drafts a `dev-spec/v1` payload and admits it (②), and proposes a planning
   PR before writing code (③).
3. It routes through `docs/task-to-file.md`, edits the owning source,
   regenerates closures, and runs the narrowest gate (④).
4. It fills the PR template with a verification table and separates what it
   observed from what it inferred (⑤).

Review an agent's work against that list, not against the diff alone. The
usual failure modes are the same as a person's: editing a generated file,
skipping the planning PR "because it is small", presenting a green unrelated
gate as evidence, or calling a merged PR done. The checks catch most of
these; the ones they do not catch are ⑤'s evidence classes and ⑥'s judgement
that an exit condition was actually observed.

## Where the details live

| Question | Page |
|---|---|
| The rules that bind a PR, exactly | [CONTRIBUTING.md](../../CONTRIBUTING.md) |
| Every `devloop` and `myque` command, in order | [Carrying a work item](06-work-item-lifecycle.md) |
| Which file owns my change | [Task-to-file index](../task-to-file.md), [Recipes](11-recipes.md) |
| What a gate is and how to add one | [Add a gate](07-add-a-gate.md) |
| Why the requirements body is data | [Spec-driven work-item bodies](../decisions/spec-driven-work-item-bodies.md) |
| Why the commit, item, and docs own different records | [Development record ownership](../decisions/development-record-ownership.md) |
| A term I do not recognise | [Glossary](../glossary.md) |
