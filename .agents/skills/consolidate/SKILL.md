---
name: consolidate
description: Weekly consolidation ("dream") of what the project learned into its owning documentation — reads the work items completed, PRs merged, and review threads since the last run, folds what they established into `docs/architecture/`, `docs/glossary.md`, and proposed `docs/decisions/`, reports claims no test, contract, or decision owns instead of adding them, and opens one documentation PR whose description is the human-readable digest of the week. Use when asked to consolidate, dream, write the weekly digest, or catch the documentation up after a batch of items closed.
---

# Consolidation

Work items, PRs, and review threads record what happened. The owning
documentation records what is true now. This skill is the backstop between
them, and it gives the people on the project one short thing to read each
week.

It does not replace the same-PR rule in `AGENTS.md`: a product PR that
changes a contract, wire, or operating boundary still updates its owning
`docs/architecture/` page itself. Consolidation catches what that rule
missed and what only becomes visible across several PRs. A boundary change
that reached `main` without its page is corrected here and named in the
digest, so a person can see that the same-PR rule was missed.

| Layer | Where | Who changes it |
| --- | --- | --- |
| raw record | `.tasks/items/`, `.tasks/terminal/`, merged PRs, review threads, commit history | never changed by this skill |
| consolidated | `docs/architecture/`, `docs/glossary.md`, `docs/decisions/` | this skill proposes; a person merges |
| schema | `AGENTS.md`, `HUMAN.md`, this file | people only |

A spec-driven item's body is not kept current after it closes. What it
established survives here or in a test, contract, or decision record. The
item itself stays in the store as history.

## What this skill never does

- Edit code, tests, `scripts/`, `.tasks/`, `.devloop/`, `AGENTS.md`,
  `HUMAN.md`, any skill, or any `docs/` file outside `docs/architecture/`,
  `docs/glossary.md`, and `docs/decisions/`. Stale text elsewhere is
  reported under "Needs a person" with the one-line fix it needs.
- Accept a decision. New decision records are `status: proposed`.
- Turn a claim into a fact by writing the test that would prove it. A
  missing test is reported; writing it is a work item.
- Add a checker, gate, exam, or recipe for its own output. If the digest
  turns out to need enforcement, a person decides that later.
- Rewrite a page wholesale. Make the smallest edit that makes the page true.

## Window

The window runs from the merge commit of the previous consolidation PR (title
prefix `docs(repo/consolidate)`), exclusive, to `origin/main`. With no
previous run, it covers the last 14 days.

Work the previous run deferred is part of this window. Read the previous
consolidation PR's `## Not done` section
(`gh pr list --state merged --search 'in:title "docs(repo/consolidate)"'
--json number,title,mergedAt,body`, then take the most recently merged one
whose title starts with `docs(repo/consolidate)`). Every line marked
`Deferred:` names an item, PR, or page to process in this run as if it
were inside the window. Lines that give a reason it will never be done (a
cancelled item, a process PR) are not carried.

```sh
git fetch origin main
BASE=$(git log origin/main --grep='^docs(repo/consolidate)' -1 --format=%H)
# no previous run: BASE=$(git rev-list -1 --before='14 days ago' origin/main)
git log --first-parent --format='%h %ad %s' --date=short "$BASE"..origin/main   # merged PRs

# items whose state changed in the window, one line per item with its new state
git log -p --format= "$BASE"..origin/main -- .tasks/items/ \
  | awk '/^\+\+\+ b\//{f=substr($2,3)} /^\+state: (done|cancelled)/{print $2, f}' | sort -u

# review threads, per PR; --paginate fetches every page, --jq emits one JSON
# object per line (read the whole stream with `jq -s`)
gh api --paginate "repos/iceice666/slime_os/pulls/<n>/comments?per_page=100" \
  --jq '.[] | {id, in_reply_to_id, path, line, user: .user.login, body}'   # inline threads
gh api --paginate "repos/iceice666/slime_os/issues/<n>/comments?per_page=100" \
  --jq '.[] | {id, user: .user.login, body}'                                  # conversation
gh api --paginate "repos/iceice666/slime_os/pulls/<n>/reviews?per_page=100" \
  --jq '.[] | select(.body != "") | {id, user: .user.login, body}'           # review bodies
```

Review bodies can carry findings that sit outside the diff. Skip the ones
that only repeat an inline finding or are tool boilerplate.

**Closed items.** An item closes when its `state` in `.tasks/items/` becomes
`done` or `cancelled`. Retirement into `.tasks/terminal/` is a separate,
later workflow, so new files there do not show what closed in this window.
Retirement runs daily and deletes the item file, keeping only a summary
in the terminal record. Its `history.commit` points into the history
archive, not this repository. To read a retired item's requirements and
evidence, recover the file from `main` as it was just before retirement:

```sh
c=$(git log -1 --format=%H --diff-filter=D origin/main -- "$path")
git show "$c^:$path"
```

- A `done` spec-driven item has completed through devloop, and its
  requirements are candidates for claims (Pass 1).
- A `done` legacy prose item (pre-cutoff, with no spec-driven body) closed
  through `myque close` with no acceptances. Its `## Closure evidence`
  section is what it observed. A property stated there is a candidate claim
  only if the section names the test or recipe that observed it; otherwise
  it goes under "Needs a person" as unverified.
- A `cancelled` item never completed. It produces no claim, even if it
  recorded passing evidence before it was cancelled. List it under "Not
  done" with the item that superseded it, if any.
- Re-admissions are linked by the `supersedes:` field in an item's front
  matter; copies with the same title are a hint, not the proof. Count the
  chain once, at its newest item, and name the cancelled copies in a single
  "Not done" line. If the newest item is still open, the chain produces
  nothing this window; say so on that line.
- An item that closed in the window whose implementing PR merged before
  `BASE` is still ingested. Its PR's threads are read like a window PR's.

**Product or process.** An item closes on `main` through a
`chore(repo/tasks)` closeout PR, which is a process PR. Judge an item by its
implementing PR, not by the closeout PR. The implementing PR carries the
`myque:implements=<UUID>` trailer when `myque-gh pr link` was run; otherwise,
search merged PR bodies for the UUID (`gh pr list --state merged --search
<UUID>`). Planning PRs that mention the UUID also match; the implementing PR
is the match whose diff touches product paths.

Product paths are the shipped OS tree: `slime-root/`, `components/`,
`boot-contracts/`, `sel4/`, and the `contracts/` that define formats the OS
reads or writes. `codePaths` in `.devloop/policy.json` is broader: it also
covers CI workflows, devloop contracts, and `verification/`, which is right
for staling evidence but not for this. A product PR is a window PR whose
diff touches product paths, whatever its title scope. The trailer is an HTML
comment in the PR body (`<!-- myque:implements=<UUID> -->`), not a commit
trailer, so read it with `gh pr view <n> --json body`, not `git log`.

**Thread budget.** Read the inline threads and conversation of every product
PR in full, including the author's replies. A reply that says what was
reproduced, what changed, and which test now fails without the fix is the
most precise statement of an invariant the project has. Process PR threads
(planning, closeout, `repo/` scope) are scanned, not read: search them for
`Decision:` and "known limit", and read only the threads those searches
hit. When Pass 2 has a candidate term, search them again for it. A fix can
also be described in the PR body instead of a thread reply, for example when
the human relayed the findings in one comment; read the body's Known limits
and verification sections for test names. A stop-and-ask handoff and a
human's `Decision:` answer are usually conversation comments.

Read the `## Decisions` section of every window PR's body, product or
process. A durable choice is often recorded there, without the `Decision:`
token, which is reserved for a human's answer in a thread.

## Pass 1: ingest

For each `done` item and each product PR in the window:

1. **Find the owning page.** Take the code paths that carry the item's or
   PR's behavior change. Skip paths changed only mechanically (a renamed
   macro, a moved import, regenerated outputs); a PR that touched 30 files
   to rename one call usually has one owning page. Find their row in
   `docs/task-to-file.md`, then the `docs/architecture/` page that cites
   that row's starting file or follow-on files (search `docs/architecture/`
   for the path), using the index in `docs/architecture/README.md` to
   choose between candidates. The page that
   names the code as its owner wins over one that only mentions it. If no
   page owns the subsystem, list it under "Needs a person" with the page it
   would most plausibly belong to. Do not create a page without that
   decision.
2. **Boundary.** If the page no longer describes what the code accepts and
   refuses, correct it.
3. **Invariants.** A lasting property of the subsystem (a refused input
   changes no state; a queue never holds more than its declared capacity)
   becomes a claim on the page. Claims come from two sources, and both must
   be owned:
   - a requirement of a `done` item whose acceptance has passing evidence,
     owned by the recipe that evidence ran. An evidence entry names its gate
     and an `inputs:` digest, not the recipe: the digest is the SHA-256 of
     a `.devloop/inputs/*.json` file
     (`shasum -a 256 .devloop/inputs/*.json | grep <digest>`), and that
     file's `justTarget` is the recipe. A passing recipe proves only that it
     exited successfully. Before citing it, find where it asserts this
     property: a host test, a serial marker or chain in the plane script or
     `scripts/lib/sel4_gate_markers.py`, or a check in the judge it runs.
     Cite that assertion with the recipe. If nothing in the recipe asserts
     it, the requirement is not a claim; it goes under "Needs a person" as
     unverified;
   - a review finding fixed with a test, owned by that test. Decide whether
     it states a lasting property or a one-off defect, and promote only the
     lasting ones.

   Several sources that are instances of one property become one claim
   citing all their owners. Before adding a claim, check whether the page
   already states it; an item whose implementing PR landed in an earlier
   window may have been consolidated from that PR already.
4. **Limitations.** Only a limit someone declared goes in the page's
   `## Limitations`: a PR's "Known limits" section, a limit the item states,
   or a thread where the human declared one with `Decision:`. A section that
   plays that role under another name (for example `## Qualification
   limits`) counts; create `## Limitations` only if the page has neither.
   A case a review raised and the author declined as "outside the item" is
   an unclaimed follow-up proposal, not evidence the product lacks the
   behavior. Check the code before writing anything about it. If the code
   defines the behavior, it is not a limitation; if you cannot tell, list
   it under "Needs a person".
5. **Decisions.** A design choice with consequences beyond its PR (for
   example, "storage may exceed the declared configuration") and no record in
   `docs/decisions/` gets a proposed record in the format
   `docs/decisions/README.md` requires: status `proposed`, context and the
   choice as made, alternatives and trade-offs, consequences and revisit
   conditions, and the work-item UUIDs and code or contract references.
   Follow an existing record's layout. If the source records no
   alternative, write the plausible ones and label them as this run's
   inference, for the person to confirm. Add the record to the README's
   `## Records` index, and list it under "Needs a person".

Process and verification PRs produce no architecture claims. They are read
only for vocabulary (Pass 2) and for decisions about the product.

## Pass 2: lint

Lint the pages this run edits, the pages that own the code carrying the
window's product behavior changes (the same paths as Pass 1 step 1), and the
glossary. Name any other page left unchecked under "Known limits".

- **Unowned claims.** `docs/architecture/README.md` requires every behavior
  claim to point to its owner, so this run never adds a claim without one.
  An existing claim with no implementation, contract, reference, or gate
  pointer is left as it is and listed under "Needs a person", with the
  options of finding its owner or removing it.
- **Broken owners.** A cited test name, path, recipe, or record that no
  longer exists. Search for it before reporting; a rename is fixed in place.
- **Contradictions.** Between two pages, or between a page and the code the
  window changed. Check the code, and correct the page if the page is wrong.
  If the code looks wrong, do not change either; report it under "Needs a
  person".
- **Vocabulary.** A glossary entry is warranted when the term appears in the
  window's product PRs or their items, is absent from `docs/glossary.md`, and
  a reader of an architecture page or of this digest would need it. Use at
  most two sentences plus its owner, in the glossary's existing style. Do not
  add a term merely because it was frequent; project names and ordinary
  words do not qualify. If a term cannot be defined in two sentences, do not
  force it; list it under "Needs a person" as possibly over-built.
- **Duplicates.** If two pages state the same property, keep it on the
  owning page and link to it from the other. Near-identical bullets within
  one page are merged.

## Claim format

Follow the page's existing idiom for owners. A page with an owners list or
a verification section, under whatever heading (`Owners:`, `Current
owners:`, `## Bounds and verification`), gets new owners there, and its
claims read like its existing ones. Name the test next to the claim it owns
in that list, so the link between them survives. Use the per-claim form
only on pages without an idiom:

```markdown
- A refused frame leaves no key alias declared.
  Owner: `a_frame_refused_for_its_size_leaves_no_alias_behind`
  (`components/lib/src/zenoh_links/scenarios.rs`).
```

A claim with no owner does not go on the page. It goes under "Needs a
person" in the PR description, until a person names an owner or drops it.

Owners are tests, contracts, code paths, recipes, or decision records. Do not
cite PR numbers or describe review history on the page: `change-records`
keeps history out of current documentation. The PR description is where the
history of this run goes.

## Output: one PR

Use branch `consolidate/<YYYY-MM-DD>`, dated with the committer date of
`origin/main`, and title `docs(repo/consolidate): <first date>–<last date>`,
where the dates are the committer dates of `BASE` and `origin/main`. This
is a process PR with no work item.

Run `just docs_check` and `just typos` before opening it, in a checkout whose
`deps/zutai` submodule is initialized; maintained docs link into it.

Keep the diff under about 300 lines. If the window produced more,
consolidate the pages with the most product change. List the rest under
"Not done", each line starting with `Deferred:`, so the next run picks it
up.

The description is written for a person who did not read any of the window's
PRs. Use this shape and nothing longer; the last three sections are the
record every PR carries:

```markdown
## What changed in the system

<!-- At most ten bullets, plain language, one per real change in behavior or
     boundary. "The Zenoh link now accepts fragmented messages up to the size
     its configuration declares." Not "landed #322". -->

## Needs a person

<!-- Every unverified claim, proposed decision, unowned subsystem, suspected
     code defect, stale text outside this skill's edit set, boundary change
     that reached main without its architecture page, and term that resisted
     a two-sentence definition. Each entry gives two or three options and a
     recommendation, or says why none is defensible, so it can be answered
     with "Decision: <option>. Because <reason>." -->

## Pages touched

<!-- One line per page: what was added, corrected, or marked. -->

## Not done

<!-- Cancelled items, items or pages skipped, threads not read, and why.
     Start a line with `Deferred:` when the next run must pick it up. -->

## Risk

<!-- What a reader of the touched pages could now believe wrongly if a claim
     here is wrong, and which claims rest on an assertion this run found but
     did not run. "None beyond Needs a person" is an answer. -->

## Verification

<!-- `just docs_check` and `just typos` with their observed results, and that
     no runtime tests were run. -->

## Known limits

<!-- What this run did not establish: claims left unverified, threads not
     read, pages not checked against code. -->
```

A person answers "Needs a person" entries in the PR thread. Apply each answer
in the same PR:
- an accepted decision changes its record to `accepted`;
- a claim the person drops stays out of the page, or is removed if it was
  already there;
- a claim the person keeps but no one can own stays out of the page, and
  becomes a follow-up item they choose whether to admit.
