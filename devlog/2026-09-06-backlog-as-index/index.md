# The backlog file becomes an index over the work-item store

| Field | Value |
|---|---|
| Date | 2026-09-06 |
| Kind | Change |
| Status | Verified |
| Scope | `roadmap/00-backlog.md`, `.tasks/items/`, `scripts/check/check-work-items.py`, `scripts/lib/work_items.py`, `AGENTS.md`, `README.md`, `roadmap/README.md` |
| Work items | 01a07762-1202-7450-b60b-31b4210da8da |
| Gates | `just tasks_check`, `just devlog_check`, `just ruff`, `just typos` |
| Trigger | A documentation pass found `roadmap/00-backlog.md` restating state the MyQue store already owned |
| Baseline | 107655 bytes of backlog file, 94 resolved entries carrying problem statement, exit condition, and status in full alongside the same content in `.tasks/items/` |

## Summary

The MyQue migration moved work-item identity into `.tasks/items/` but left the
backlog file carrying all 94 resolved entries verbatim, so every closure existed
twice with no mechanism keeping the copies agreeing. A sentence-level audit
found the store already a strict superset for 93 of 94 entries — and found that
the 94th, `B29`'s second allocation, had been silently truncated from 2168 to
570 characters by the migration, losing the sole record of a defect that has no
devlog entry. That content is restored. The resolved log is now 94 headings,
each carrying its devlog link and item UUID and nothing else, which keeps all 8
anchored devlog links resolving while removing 80KB of duplicated state.
`check_backlog_index_resolves` fails the build if a heading stops resolving.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `.tasks/items/019fd7cd-8c00-782b…` | `B29`'s second allocation regains the full investigation the migration truncated | The one defect with no devlog entry keeps a complete record |
| `roadmap/00-backlog.md` | 94 resolved bodies collapse to heading + devlog link + item UUID; head rewritten around `myque new`/`close` | State lives in exactly one place |
| `roadmap/00-backlog.md` | Deferred follow-ups become a table keyed `F1`–`F5` | A prose bullet is not addressable; a key is |
| `.tasks/items/01a0724c-5400-…` (×5) | Keys `F1`–`F5` assigned; `F4`/`F5` titles repaired where the migration truncated them mid-word | `myque list` no longer renders five items as one repeated token |
| `scripts/check/check-work-items.py` | `check_backlog_index_resolves` validates every heading against the store | A dangling index entry fails the build instead of rotting |
| `scripts/lib/work_items.py` | `open_backlog` no longer counts `blocked` as blocking | A defect waiting on the outside world cannot wedge every milestone |
| `AGENTS.md` | States the index contract and the `myque` commands that maintain it | The next author is told the workflow, not left to infer it |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A heading names an item that was deleted or never existed | `just tasks_check` | `backlog index: B92 names <uuid>, which is not in .tasks/items` |
| A heading is added without its `**Item:**` line | `just tasks_check` | `94 resolved heading(s) but 93 resolve to an item` |
| An indexed item is reopened while its heading sits under `## Resolved` | `just tasks_check` | `B92 is under '## Resolved' but its item is open` |
| The file is deleted, breaking 75 devlog entries | `just tasks_check` | `roadmap/00-backlog.md is missing: 75 devlog entries link into it` |
| A devlog link into a heading stops resolving | `just devlog_check` | `dead relative link` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just tasks_check` | pass — 272 items, 262 legacy ids mapped, 94 backlog entries indexed | Direct |
| `just devlog_check` | pass — 289 entries, 289 indexed | Direct |
| `just ruff` | pass | Direct |
| `just typos` | pass | Direct |
| Sentence-level audit of 94 headings against their items | store is a strict superset for all 94 after the `B29` repair | Direct |
| GitHub-slug check of all 8 anchored devlog links against 97 surviving anchors | all resolve | Direct |
| Mutation: dangling UUID in the index | fails, naming `B92` | Direct |
| Mutation: heading with no `**Item:**` line | fails, reporting 94 vs 93 | Direct |
| Mutation: indexed item reopened | fails, naming `B92` and its state | Direct |
| Mutation: backlog file deleted | fails, naming the 75 dependent entries | Direct |

## Decisions

- Decision: keep every `### B<N> — <title>` heading verbatim and collapse only the bodies.
- Rationale: 75 devlog entries link into the file and 8 anchor a specific heading. Devlog entries are immutable, so the anchors are a fixed external contract; the bodies are not.
- Rejected alternative: delete the resolved log outright. Smaller, but the 8 anchored links would silently degrade to landing at page top, and `check-devlog.py:295` validates the file rather than the fragment, so nothing would report it.
- Rejected alternative: keep the 100KB and add a text-divergence gate. That makes duplication machine-enforced rather than removed, and forces every future closure to be written twice.

- Decision: the index line carries the item UUID, not the human key.
- Rationale: `B29` and `B30` were each allocated twice, so their four entries cannot be distinguished by key. A UUID also survives a key being changed or dropped, which `AGENTS.md` explicitly permits.

- Decision: `blocked` satisfies the backlog-first rule.
- Rationale: it means the item waits on something outside this repository, which no milestone sequencing resolves. Counting it as blocking would wedge every milestone behind work nobody here can start. Only `open` and `active` block now.

## Open risks and follow-ups

- [ ] The five `F` keys were assigned by this change rather than by the migration. They are display aliases like any other, but the devlog entry that created them is this one, not the migration's.

## Artifacts and provenance

- Focused report: none; the audit was scripted and its results are in Verification.
- Raw transcript: none retained.
- Serial/debugger/model output: none — this change touches no runtime path.
- Related work item: `01a07762-1202-7450-b60b-31b4210da8da` (`MQ2`)
