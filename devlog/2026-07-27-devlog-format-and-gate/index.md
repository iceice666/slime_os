# Devlog format contract, uniform layout, and enforcement gate

| Field | Value |
|---|---|
| Date | 2026-07-27 |
| Kind | Change |
| Status | Verified |
| Scope | `devlog/README.md`, `devlog/TEMPLATE.md`, all 20 existing entry folders, `scripts/check/check-devlog.py`, `Justfile`, `AGENTS.md`, `roadmap/README.md` |
| Roadmap | none |
| Gates | `just devlog_check` |
| Trigger | Requested review of the devlog's format and file organization |
| Baseline | Two coexisting entry shapes, one template for four kinds of entry, prose-only cross-references, and no validation of any of it |

## Summary

The devlog is mandatory under `AGENTS.md` but was the only load-bearing
directory in the repository with no checker, and it had drifted accordingly: 9
of 20 entries were flat `.md` files and 11 were folders, though only 2 folders
actually held evidence siblings; `roadmap/` hardcoded both shapes in its links;
one entry's `Status` was a prose sentence rather than a vocabulary token; and
the roadmap ids and gate names an entry bore on were recoverable only by reading
its body. This change makes the folder shape universal, adds `Kind`, `Roadmap`,
and `Gates` to the front matter, makes the required section set a function of
`Kind`, and lands `just devlog_check` so every one of those rules fails loudly
instead of drifting. No entry body was edited: the migration is `git mv` plus
front-matter insertion, preserving the immutability rule the format itself
declares.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Entry layout | 11 flat `YYYY-MM-DD-topic.md` entries became `YYYY-MM-DD-topic/index.md` via `git mv`; the folder shape is now unconditional | One shape; adding evidence to an entry later never moves it or breaks an inbound link |
| Front matter | Added `Kind`, `Roadmap`, and `Gates` to all 20 entries; fixed field order and set | "Which entry covers C7.4?" and "what backs `transfer_check`?" are answerable from front matter, not prose |
| `devlog/2026-07-26-c7-audit/` | `Status` narrowed from `Verified (B3 isolated and fixed 2026-07-26 — see Corrections; B4–B8 open)` to `Verified` | `Status` is a vocabulary token; the narrative it carried already lives in that entry's `## Corrections` and in `roadmap/00-backlog.md` |
| `devlog/TEMPLATE.md` | Declared the four kinds and their required sections; sections not required by a kind are deleted rather than filled with "n/a" | A `Decision` entry no longer implies a missing `Root cause`; a `Defect` entry cannot silently omit one |
| `devlog/README.md` | Rewrote the layout, front-matter, and kind contract; regenerated the index with `Kind`, `Status`, and `Roadmap` columns | The format contract is stated where the entries live, and the index carries the routing fields |
| `scripts/check/check-devlog.py` | New checker: layout, naming, front-matter set/order, vocabularies, `Roadmap` ids against roadmap headings, `Gates` against Justfile targets, per-kind sections and order, sibling linkage, index agreement, and repo-wide `devlog/...` link health | The format is enforced rather than aspirational |
| `Justfile`, `AGENTS.md`, `roadmap/README.md` | Added `just devlog_check` and documented the new rules | The gate is discoverable and named in the repository-wide gate list |
| `roadmap/00-backlog.md`, `roadmap/02-core-runtime.md` | Repointed 4 links left stale by the migration | No dangling references |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A new entry lands as a flat file, or with an evidence-free folder later becoming a folder-with-evidence and moving | `just devlog_check` | `flat entry file; every entry is a folder with an index.md` |
| `Status` or `Kind` drifts off-vocabulary again | `just devlog_check` | `Status '…' is not one of …` |
| An entry claims `Verified` without naming a gate that observed it | `just devlog_check` | `Status Verified claims an observed result but Gates is none` |
| `Roadmap`/`Gates` reference a renamed milestone or deleted Justfile target | `just devlog_check` | `matches no roadmap/backlog heading` / `is not a Justfile target` |
| A kind-required section is dropped, renamed, or reordered | `just devlog_check` | `Kind X requires a '## Y' section` / `sections are out of template order` |
| Evidence file added to an entry folder but never linked | `just devlog_check` | `evidence file X is not referenced from index.md` |
| Entry renamed/moved, leaving stale links in `roadmap/`, `AGENTS.md`, or another entry — including from a not-yet-committed entry | `just devlog_check` | `references devlog/…, which does not exist` |
| README index and entry disagree after a `Status` change | `just devlog_check` | `README index status '…' != entry Status '…'` |
| An unescaped `\|` splits a table row, silently changing what it says | `just devlog_check` | `table row has N cells where its header has M` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just devlog_check` | `devlog check passed: 21 entries, 21 indexed`, exit 0 | Direct |
| Fault injection: off-vocabulary `Status` | Rejected | Direct |
| Fault injection: unknown `Kind` | Rejected | Direct |
| Fault injection: `Roadmap` id `Z9` | Rejected | Direct |
| Fault injection: `Gates` naming `no_such_check` | Rejected | Direct |
| Fault injection: `Verified` with a `Gates` value of `none` | Rejected | Direct |
| Fault injection: renamed `## Verification` heading | Rejected (unknown section + missing required section) | Direct |
| Fault injection: `Kind`/`Status` order swapped | Rejected | Direct |
| Fault injection: index status disagreeing with entry | Rejected | Direct |
| Fault injection: `Date` not matching folder prefix | Rejected | Direct |
| Fault injection: dead relative link in an entry | Rejected | Direct |
| Fault injection: a flat `YYYY-MM-DD-scratch.md` beside the entry folders | Rejected | Direct |
| Fault injection: unlinked `orphan.log` sibling | Rejected | Direct |
| Fault injection: `roadmap/00-backlog.md` pointing at a pre-migration path | Rejected | Direct |
| Fault injection: unregistered entry folder | Rejected (missing sections + not registered) | Direct |
| Fault injection: unescaped `\|` splitting a Verification row | Rejected | Direct |
| Fault injection: dead `devlog/…` link inside an untracked new entry | Rejected before commit | Direct |
| Tree restored after every injection; baseline re-run | `devlog check passed: 21 entries, 21 indexed` | Direct |

Documentation and host-tooling change only; no kernel, component, contract, or
generation code was touched, so no QEMU gate was run.

## Decisions

- **Decision:** One entry shape — a folder — even with no evidence siblings.
- **Rationale:** The old "flat file when there is no evidence" rule made an
  entry's path a function of whether a transcript happened to be retained. 6 of
  8 existing folders had no siblings, so the rule was already not being followed,
  and adding evidence to a flat entry would have silently invalidated every
  inbound link.
- **Rejected alternative:** Keeping both shapes and teaching the checker to
  accept either — it preserves exactly the ambiguity that produced the drift.

- **Decision:** `Kind` selects the required section set.
- **Rationale:** One template for regressions, feature landings, audits, and
  design decisions forced either empty "n/a" sections or undetectable omissions:
  `2026-07-24-c7-finer-decomposition` legitimately has no `Root cause`, while a
  defect entry missing one is a real gap. Encoding the difference lets the
  checker require a root cause exactly where one is owed.
- **Rejected alternative:** Separate template files per kind — four skeletons
  drift apart, and an entry's kind would stop being machine-readable.

- **Decision:** `Roadmap` and `Gates` are structured front-matter fields
  validated against `roadmap/` headings and Justfile targets.
- **Rationale:** These are the devlog's two real query axes. Validating them
  turns a renamed milestone or deleted check target into a checker failure
  instead of a stale link discovered years later.
- **Rejected alternative:** A generated cross-reference index — more machinery,
  and it would still not notice an id that never existed.

- **Decision:** Migrate with `git mv` plus front-matter insertion; edit no entry
  body.
- **Rationale:** The format's own immutability rule freezes published bodies.
  A reorganization that rewrote them to fit a new template would violate the
  rule it is establishing.

## Open risks and follow-ups

- [ ] `Gates` was derived from each entry's existing *Regression guards* section,
  falling back to the first non-hygiene targets in *Verification* where that
  section named none. The values are accurate to what those entries recorded,
  but they were not re-observed by re-running each gate; `just devlog_check`
  validates that the target names exist, not that they still pass.
- [ ] `Roadmap` for `2026-07-24-kernel-layout-cleanup` and
  `2026-07-24-root-workspace-layout` is `none`: both cite a roadmap *file*
  rather than a milestone id. If either is later tied to a P-track milestone,
  update the field.
- [ ] The checker resolves `Roadmap` ids against any `##`/`###` roadmap heading
  token. That is deliberately permissive — it catches typos and renames, not an
  id pointed at the wrong track.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: not retained; the checker's fault-injection results are
  tabulated in *Verification* above and are reproducible by re-running
  `just devlog_check` against the same mutations.
- Serial/debugger/model output: none; no guest code was run.
- Related roadmap item: none — repository documentation and tooling only.
- Format contract: [`devlog/README.md`](../README.md),
  [`devlog/TEMPLATE.md`](../TEMPLATE.md).
