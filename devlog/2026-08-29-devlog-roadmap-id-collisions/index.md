# Devlog roadmap ID collision guard

| Field | Value |
|---|---|
| Date | 2026-08-29 |
| Kind | Change |
| Status | Verified |
| Scope | `scripts/check/check-devlog.py`, `devlog/README.md` |
| Roadmap | none |
| Gates | `just devlog_check`, `just ruff` |
| Trigger | Review found `roadmap_ids()` collapsed repeated roadmap/backlog IDs into a set, so a new duplicate heading could silently make one devlog ID ambiguous |
| Baseline | `just devlog_check` validated that a devlog Roadmap ID existed, but not that it resolved to one canonical roadmap heading |

## Summary

`just devlog_check` now indexes canonical roadmap and backlog declarations before resolving devlog front matter and rejects any ID declared more than once, reporting every conflicting file and line. The two already-published B29 and B30 collisions are frozen by exact path and heading text: they remain readable historical anchors, but any alteration or third allocation fails the gate.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Roadmap heading parser | Distinguished canonical ID declarations (`B85`, `C8.13.1`, `P5.4.final`) from descriptive headings such as `M5 acceptance and verification stack` and range headings such as `P5.4.2 … P5.4.n` | Only headings that allocate an ID participate in uniqueness checks |
| Collision validation | Collected every declaration with path and line, then rejected IDs with multiple declarations | A new roadmap/backlog ID resolves to exactly one heading |
| Historical compatibility | Admitted only the exact four existing B29/B30 headings as frozen legacy collisions | Existing immutable anchors remain valid without permitting future collisions |
| Regression probe | Added a startup probe containing two canonical `Z99` declarations plus a descriptive `Z99 acceptance` heading | The collision parser and grouping path must demonstrably detect the duplicated ID without misclassifying descriptive headings |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A new roadmap or backlog heading reuses an allocated ID | `just devlog_check` | `Roadmap id '…' is declared more than once: <path>:<line>, …` |
| The parser begins treating acceptance-stack headings as declarations | `just devlog_check` startup probe | `devlog check's roadmap id collision guard failed its probe` |
| A historical B29/B30 heading changes or gains a third declaration | `just devlog_check` | The exact legacy signature no longer matches and the ordinary collision error is emitted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `python3 scripts/check/check-devlog.py` | Passed before the devlog entry was added: `devlog check passed: 254 entries, 254 indexed` | Direct |
| `ruff check scripts/check/check-devlog.py` | `All checks passed!` | Direct |
| `just devlog_check` | Passed after registering this entry | Direct |
| `just ruff` | Passed after the final checker and devlog changes | Direct |

## Decisions

- Decision: recognize declarations only when the ID is followed by the repository's title separators (`—`, `--`, or `:`).
- Rationale: the prior first-token parser intentionally accepted broad headings for reference resolution, but that grammar also reads `### M5 acceptance and verification stack` as a second declaration. Collision checking needs a narrower allocation grammar.
- Rejected alternative: renumber or rewrite the published B29/B30 headings. Backlog headings are immutable anchors; exact legacy signatures contain the exception without normalizing new collisions.

## Open risks and follow-ups

- [ ] B29 and B30 remain intrinsically ambiguous historical IDs. Their exact headings are frozen and no additional declaration can pass, but existing devlog front matter containing only `B29` or `B30` cannot distinguish the first allocation from the second.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none.
- Serial/debugger/model output: none; documentation tooling only.
- Related roadmap item: none.
