---
name: consolidate
description: Weekly consolidation ("dream") of what the project learned into its owning documentation — reads the work items closed, PRs merged, and review threads since the last run, folds what they established into `docs/architecture/`, `docs/glossary.md`, and proposed `docs/decisions/`, marks claims no test, contract, or decision owns, and opens one documentation PR whose description is the human-readable digest of the week. Use when asked to consolidate, dream, write the weekly digest, or catch the documentation up after a batch of items closed.
---

# Consolidation

Work items, PRs, and review threads record what happened. The owning
documentation records what is true now. This skill moves the second forward
from the first, on a schedule, so that no single PR has to remember to, and
so that the people on the project have one short thing to read each week.

| Layer | Where | Who changes it |
| --- | --- | --- |
| raw record | `.tasks/terminal/`, merged PRs, review threads, commit history | never changed by this skill |
| consolidated | `docs/architecture/`, `docs/glossary.md`, `docs/decisions/` | this skill proposes; a person merges |
| schema | `AGENTS.md`, `HUMAN.md`, this file | people only |

A spec-driven item's body is not kept current after it closes. What it
established survives here or in a test, contract, or decision record; the
item itself stays in `.tasks/terminal/` as history.

## What this skill never does

- Edit code, tests, `scripts/`, `.tasks/`, `.devloop/`, `AGENTS.md`, `HUMAN.md`, or any skill.
- Accept a decision. New decision records are `status: proposed`.
- Make a claim verified by writing the test that would verify it. A missing
  test is reported; writing it is a work item.
- Add a checker, gate, exam, or recipe for its own output. If the digest
  turns out to need enforcement, a person decides that later.
- Rewrite a page wholesale. Edits are the smallest that make the page true.

## Window

The window starts at the merge commit of the previous consolidation PR
(title prefix `docs(repo/consolidate)`) and ends at `origin/main`. With no
previous run, use the last 14 days.

```sh
git fetch origin main
BASE=$(git log origin/main --grep='^docs(repo/consolidate)' -1 --format=%H)
# no previous run: BASE=$(git rev-list -1 --before='14 days ago' origin/main)
git log --format='%h %s' "$BASE"..origin/main                                 # merged PRs
git log --diff-filter=A --name-only --format= "$BASE"..origin/main -- .tasks/terminal/   # closed items
gh api "repos/iceice666/slime_os/pulls/<n>/comments?per_page=100"             # review threads, per PR
```

Read review threads in full, including the author's replies: a reply that
says what was reproduced, what changed, and which test now fails without the
fix is the most precise statement of an invariant the project has.

## Pass 1: ingest

For each item closed and each product PR merged in the window:

1. **Find the owning page.** Use the row in `docs/task-to-file.md`. If no
   architecture page owns the subsystem, list it under "Needs a person" and
   move on; do not create a page without that decision.
2. **Boundary.** If the page no longer describes what the code accepts and
   refuses, correct it.
3. **Invariants.** For each review finding that was fixed with a test, decide
   whether it states a lasting property of the subsystem (a refused input
   changes no state; a queue never holds more than its declared capacity) or
   a one-off defect. Lasting properties are added to the page as claims
   (format below). Several findings that are instances of one property become
   one claim citing their tests.
4. **Limitations.** A case a thread declared out of scope, or a known limit
   the PR description states, goes in the page's `## Limitations`.
5. **Decisions.** A design choice made in a thread or PR with consequences
   beyond that PR (for example, "storage may exceed the declared
   configuration") and no record in `docs/decisions/` gets a short proposed
   record: context, the choice as made, the alternative, the consequence seen
   so far. It is listed under "Needs a person".

Process and verification PRs (`repo/` scope) are read only for vocabulary
(pass 2) and for decisions about the product. They do not produce
architecture claims.

## Pass 2: lint

Over every page the window touched, plus the glossary:

- **Unowned claims.** `docs/architecture/README.md` requires every behavior
  claim to point to its owner. A claim with no implementation, contract,
  reference, or gate pointer is marked unverified.
- **Broken owners.** A cited test name, path, recipe, or record that no
  longer exists. Search for it before reporting; a rename is fixed in place.
- **Contradictions.** Between two pages, or between a page and the code the
  window changed. Check the code; correct the page if the page is wrong. If
  the code looks wrong, do not change either: report it under "Needs a person".
- **Vocabulary.** A term used in two or more review threads or PR titles in
  the window and absent from `docs/glossary.md` gets an entry of at most two
  sentences plus its owner, in the glossary's existing style. A term that
  cannot be defined in two sentences is not forced; it is listed under
  "Needs a person" as possibly over-built.
- **Duplicates.** The same property stated on two pages is kept on the owning
  page and linked from the other.

## Claim format

```markdown
- A refused frame leaves no key alias declared.
  Owner: `a_frame_refused_for_its_size_leaves_no_alias_behind`
  (`components/lib/src/zenoh_links/scenarios.rs`).
- ⚠ Unverified: link storage may exceed the declared configuration.
  No test or decision record owns this.
```

Owners are tests, contracts, code paths, recipes, or decision records. Do not
cite PR numbers or describe review history on the page: `change-records`
keeps history out of current documentation. The PR description is where the
history of this run goes.

## Output: one PR

Branch `consolidate/<YYYY-MM-DD>`, title
`docs(repo/consolidate): <first date>–<last date>`. Run `just docs_check`
before opening it. This is a process PR with no work item.

Keep the diff under about 300 lines. If the window produced more, consolidate
the pages with the most product change and list the rest under "Not done" for
the next run.

The description is written for a person who did not read any of the window's
PRs. Use this shape and nothing longer:

```markdown
## What changed in the system

<!-- At most ten bullets, plain language, one per real change in behavior or
     boundary. "The Zenoh link now accepts fragmented messages up to the size
     its configuration declares." Not "landed #322". -->

## Needs a person

<!-- Every unverified claim, proposed decision, unowned subsystem, suspected
     code defect, and term that resisted a two-sentence definition. Each entry
     gives two or three options and a recommendation, so it can be answered
     with "Decision: <option>. Because <reason>." -->

## Pages touched

<!-- One line per page: what was added, corrected, or marked. -->

## Not done

<!-- Items or pages skipped and why. -->
```

A person answers "Needs a person" entries in the PR thread. Apply each answer
in the same PR: an accepted decision changes its record to `accepted`, a
claim the person drops is removed, a claim they keep without an owner stays
marked unverified and becomes a follow-up item they choose whether to admit.
