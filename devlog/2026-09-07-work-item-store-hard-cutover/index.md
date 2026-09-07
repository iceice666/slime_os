# The work-item store hard cutover: deleting the pre-MyQue identifier system

| Field | Value |
|---|---|
| Date | 2026-09-07 |
| Kind | Change |
| Status | Verified |
| Scope | 288 devlog front matters, `devlog/README.md`, `scripts/check/{check-devlog,check-work-items}.py`, `scripts/lib/work_items.py`, deleted `.tasks/legacy-roadmap-ids.json`, `scripts/migrate-roadmap-to-myque.py`, `scripts/lib/roadmap_inventory.py`, `AGENTS.md`, `roadmap/{README,00-backlog}.md`, `docs/README.md`, `docs/getting-started/04-first-change.md`, `.github/PULL_REQUEST_TEMPLATE/` |
| Work items | 01a07967-a93b-7145-a633-25ed418b3d69 |
| Gates | `just tasks_check`, `just devlog_check`, `just ruff`, `just typos` |
| Trigger | The MQ1/MQ2 migration left a compatibility layer that could still reconstruct the store from `roadmap/`, and documentation that still named `roadmap/` authoritative for completion |
| Baseline | `.tasks/items/` canonical since MQ1, but 288 devlog entries carried `Roadmap` front matter resolved through a committed legacy map, and `roadmap/README.md` mirrored aggregate item counts by hand |

## Summary

The MyQue migration was structurally complete but architecturally unfinished: a
262-entry legacy id map, an ambiguity resolver for the twice-allocated
`B29`/`B30`, and a `--force`-capable importer meant `roadmap/` was still a
latent second authority over work-item identity. This change consumes that
compatibility and deletes it. All 288 pre-migration devlog entries had their
`Roadmap` front-matter row rewritten to a `Work items` row of the same items'
UUIDs, after which the legacy map, the importer, and the roadmap inventory it
read were removed — 1136 lines of one-shot machinery, plus 92 lines of
migration-only checker policy. Both checkers now accept UUIDs only, keys resolve
nowhere, and no code path exists by which editing `roadmap/` creates or mutates a
work item. The documentation contradictions went with it, including
`AGENTS.md`'s "Roadmap completion stays authoritative in `roadmap/`" and
`roadmap/README.md`'s hand-maintained state table, which had already drifted
(claiming 218 done against an actual 219 — evidence that updating the numbers
was the wrong fix).

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Devlog metadata | Rewrote 288 `\| Roadmap \| C7.4, B30 \|` rows into `\| Work items \| <uuid>, <uuid> \|` using the committed map; `B30` in `2026-08-07-b30-release-trust-gate` resolved from the map's recorded `devlog_references` evidence, not by picking an allocation | UUID is the only persistent work-item reference |
| Devlog index | Renamed the `README.md` index column `Roadmap` → `Keys` and documented it as a display column that resolves nothing | Keys stay useful for human search without becoming references |
| `check-devlog.py` | Deleted `WORK_ITEM_FIELDS`, legacy resolution, and the ambiguity path; `FIELD_ORDER` is unconditionally `Date, Kind, Status, Scope, Work items, Gates, Trigger, Baseline`; a non-UUID and an unknown UUID now fail with distinct messages | The checker derives no identity from keys or roadmap headings |
| `work_items.py` | Deleted `LEGACY_MAP`, `legacy()`, `ambiguous_ids()`, and entry-specific resolution. Also dropped `resolve()`: collapsed to a UUID-shape-plus-membership test it would have merged two distinct failures into one answer, so callers match `UUID` and test `identities()` directly | The module answers only "is this UUID in the store" |
| `check-work-items.py` | Deleted `check_migration_auditability()` and `check_undated_closures_declare_themselves()` — both were scoped by the legacy map and cannot be defined without it; the 34 imported items retain their "original closure date is unrecorded" disclaimers as item text | Only repository policy remains: `myque check`, backlog-first, frozen-index integrity |
| Deletions | `.tasks/legacy-roadmap-ids.json` (307 lines), `scripts/migrate-roadmap-to-myque.py` (413), `scripts/lib/roadmap_inventory.py` (416) | No roadmap-to-store reconstruction path exists |
| `roadmap/00-backlog.md` | Reframed as a frozen index of pre-cutover anchors; deleted the instruction to add a heading when closing an item and the `F1`–`F5` follow-up table that duplicated store state | A new defect is representable in MyQue alone |
| `roadmap/README.md` | Replaced the `Current state` table (per-track status + "next open gate" + aggregate counts) with a `Tracks` table of ownership and boundary; stripped `complete`/`deferred`/`cancelled` strings from all 50 Mermaid nodes and two edge labels | `roadmap/` mirrors no mutable state and is not a dependency graph |
| `AGENTS.md` | Deleted the roadmap-completion-authority sentence and the legacy-map paragraph; stated the one-way data flow explicitly | No live guidance contradicts the store's authority |
| PR templates | `Work item (key or UUID)` → UUID required, key permitted parenthetically | The resolvable reference is the UUID |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A key creeps back into `Work items` as a durable reference | `just devlog_check` | `Work items names 'MQ2', which is not a UUID` |
| A referenced item is deleted or renamed | `just devlog_check` | `Work items names '<uuid>', which is not a work item in .tasks/items/` |
| `Roadmap` front matter reappears | `just devlog_check` | `front matter missing Work items` |
| A landed backlog anchor points at a deleted or reopened item | `just tasks_check` | `backlog index: B1 names <uuid>, which is not in .tasks/items` / `its item is open` |
| The store itself becomes invalid | `just tasks_check` → `myque check` | schema, graph, alias, and state findings, reported with MyQue's own status distinction |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just devlog_check` | pass — `290 entries, 290 indexed` | Direct |
| `just tasks_check` | pass — `273 items, 94 frozen backlog headings indexed` (legacy-id count no longer reported; nothing maps) | Direct |
| `just ruff` | pass — `All checks passed!` | Direct |
| `just typos` | pass, exit 0 | Direct |
| All 273 store UUIDs written into a Markdown table and spell-checked | exit 0, no false positives from hex groups | Direct |
| Mutation: `Work items \| MQ2` | fails, names the key and demands the UUID | Direct |
| Mutation: `Work items` naming `01a07762-…-000000000000` | fails as absent from the store | Direct |
| Mutation: `\| Work items \|` → `\| Roadmap \|` | fails as missing `Work items` | Direct |
| Mutation: delete a referenced item file | `devlog_check` fails on the now-absent UUID | Direct |
| Mutation: add a `backlog`-tagged item present in no roadmap file | `tasks_check` passes — `274 items, 94 … headings` | Direct |
| Mutation: rename an indexed heading `### B92` → `### B999` | `tasks_check` passes: heading text carries no identity | Direct |
| Mutation: reopen an item behind a landed heading | fails four ways, including `backlog index: B1 is under '## Resolved' but its item is open` | Direct |
| `scripts/generate/generate-system-image-closures.py --check` | `50 system-image closures are current` | Direct |
| `git status` over the fifteen declared release-input trees plus `just/` and `Justfile` | empty; no closure input changed | Direct |

## Decisions

- Decision: keep `check_backlog_index_resolves()` rather than deleting it with the rest of the backlog-file policy.
- Rationale: the dual-write pressure came from *prose* — `roadmap/00-backlog.md` telling the reader to add a heading on close, and `AGENTS.md` repeating it — not from the checker, which only validates that landed headings still resolve. 8 devlog links are anchored at specific headings, so this is the only guard against a silently dangling anchor. Deleting it would have removed a real invariant while leaving the actual dual-write instruction in place.
- Rejected alternative: dropping the check and relying on `myque check`, which knows nothing about `roadmap/`.

- Decision: `roadmap/README.md`'s state table is removed, not generated from MyQue.
- Rationale: a generator plus drift check would add a top-level mechanism to restate data the store already serves through `just tasks_list`, and would put `roadmap/` back in the data flow. `AGENTS.md` reserves new checkers for genuinely new mechanisms.
- Rejected alternative: rendering the table from `myque query` at check time.

- Decision: keys stay in `devlog/README.md`'s index column; only the front matter is UUID-only.
- Rationale: 485 key mentions in that column name items whose keys do not appear in the entry title, so replacing them with 36-character hex would destroy the only eye-scannable "which entry covers C8.14?" surface while protecting nothing — the column resolves nothing. This is exactly the `key = display alias, uuid = reference` split.
- Rejected alternative: converting the column to UUIDs for uniformity.

- Decision: `devlog/README.md` and `AGENTS.md` now name a **Migratable** class of front-matter machine fields.
- Rationale: rewriting 288 landed entries contradicted the immutability rule as written. Re-expressing a resolvable reference in a new format asserts nothing new, which is materially different from editing a conclusion; without stating that bound, the next author would read a rule forbidding what just happened. Prose is explicitly excluded — entries may keep saying `roadmap/` was authoritative when they were written.
- Rejected alternative: leaving `Roadmap` accepted forever to preserve the letter of the rule.

- Decision: the rewrite script was run once and not committed.
- Rationale: committing it would leave a second mechanism capable of writing devlog metadata from a map that no longer exists. Its inputs were the committed map and the store, both in Git history, and its output is 288 reviewable diffs.
- Rejected alternative: adding `scripts/migrate-devlog-front-matter.py` alongside the deleted importer.

## Open risks and follow-ups

- [ ] `roadmap/` still carries 183 `**Status:**` lines in per-milestone bodies. They are past-tense records of observed outcomes, which `roadmap/README.md` now scopes as frozen evidence rather than live state, but nothing mechanically prevents a future author from editing one as if it were a tracker. No gate covers this.
- [ ] `flake.nix` pins a MyQue revision predating the store-relative abbreviation fix, so `myque list` prints a repeated token for the ten keyless items. `myque check` is unaffected. Unchanged by this work; run `nix flake update myque` once the fix is published.

## Artifacts and provenance

- Focused report: none; the changes and their mutation evidence are in this entry.
- Raw transcript: none retained.
- Serial/debugger/model output: none; no guest code runs in these gates.
- Related work item: `01a07967-a93b-7145-a633-25ed418b3d69` (`MQ3`), depending on `01a07486-0f9c-7aac-a688-b2a01e5d5c29` (`MQ1`, the migration) and `01a07762-1202-7450-b60b-31b4210da8da` (`MQ2`, the backlog index).
