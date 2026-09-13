# The work-item store hard cutover: deleting the pre-MyQue identifier system

| Field | Value |
|---|---|
| Date | 2026-09-07 |
| Kind | Change |
| Status | Verified |
| Scope | 288 devlog front matters, `devlog/README.md`, `devlog/2026-09-03-cp15-closure-cutover/index.md`, `scripts/check/{check-devlog,check-work-items}.py`, `scripts/lib/{work_items,markdown_anchors}.py`, deleted `.tasks/legacy-roadmap-ids.json`, `scripts/migrate-roadmap-to-myque.py`, `scripts/lib/roadmap_inventory.py`, `AGENTS.md`, `roadmap/{README,00-backlog,07-architecture-portability}.md`, `README.md`, `docs/README.md`, `docs/getting-started/{01-orientation,02-build-and-run,04-first-change}.md`, `docs/directions/README.md`, `.github/PULL_REQUEST_TEMPLATE/` |
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
| `README.md` | Replaced the per-track "M1–M4 complete, IO0–IO7 complete, RP0–RP2 complete" status list — the same mirroring mechanism at repository-front-page scale — with a capability summary: what runs, what has physical evidence, what fails closed and why, and what is deferred by decision | The front page describes capability and evidence, not item state |
| Getting-started and directions docs | `roadmap/` described as holding "problem statements" in four places (`docs/getting-started/{01-orientation,02-build-and-run}.md`, `docs/directions/README.md`, `README.md`) → architectural rationale, with the problem statement named as the item's | No reader is routed to `roadmap/` for a live defect's problem statement |
| `check-work-items.py` (review) | Deleted the `state in {done, cancelled}` requirement on indexed headings, and replaced the `headings == len(seen)` equality — which `0 == 0` satisfied after deleting every entry — with a floor at the 94 landed headings | A frozen index routes into the store without constraining it, and the anchor set cannot silently empty |
| `check-devlog.py` (review) | Link fragments are validated against real headings and explicit `<a id>`/`<a name>` anchors, instead of being stripped with `partition("#")[0]` | An anchored inbound link cannot be broken by a reworded heading |
| `check-devlog.py` (review) | Front matter parses as ordered rows, rejects duplicate fields, and requires the field list to *equal* `FIELD_ORDER` — not merely match on its first eight distinct keys | "Exactly these eight fields" is the checked claim, and no trailing row can override an earlier value |
| `roadmap/07-architecture-portability.md` | Added two retained `<a id>` anchors (`p4--raspberry-pi-5-board-bring-up`, `p549-and-c810`) for four merged devlog links whose fragments never matched a heading in 20 revisions of the file | Four inbound URLs resolve without editing frozen devlog bodies |
| `devlog/2026-09-03-cp15-closure-cutover/index.md` | Moved a stray `\| Correction \|` front-matter row into the `## Corrections` section the format reserves for it; the loose parser had permitted it | Corrections live where `devlog/README.md` says, and the entry now matches the exact field set |
| `check-work-items.py` (review 2) | The backlog index parses as heading-delimited *sections* validated by position. The previous pass collected resolved `B<N>` keys into a set, so the first `B29` marked the second `B29` verified — a non-unique display key merging two records' evidence | A duplicate display key cannot vouch for another section |
| `scripts/lib/markdown_anchors.py` (review 2) | New module owning anchor computation, with 11 executable controls `check-devlog.py` runs before trusting it: fences (backtick, tilde, and longer-fence nesting) suppress headings, inline code keeps its literal text, duplicate headings take `-1`/`-2` suffixes, explicit anchors count, closing hashes do not | The rule that decides whether a link is live is itself proven, in both directions |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A key creeps back into `Work items` as a durable reference | `just devlog_check` | `Work items names 'MQ2', which is not a UUID` |
| A referenced item is deleted or renamed | `just devlog_check` | `Work items names '<uuid>', which is not a work item in .tasks/items/` |
| `Roadmap` front matter reappears, by replacement *or* as an extra row | `just devlog_check` | `front matter missing Work items` / `front matter carries unknown field(s) Roadmap` |
| A duplicate front-matter row silently overrides an earlier value | `just devlog_check` | `front matter repeats Work items; a repeated row silently overrides the first one's value` |
| A landed backlog heading loses its item, including one of a duplicated key | `just tasks_check` | `backlog index: B29 (heading 69 of 94) has no ...Item...` / `0 \`### B<N>\` heading(s), fewer than the 94 that landed` |
| A reworded heading breaks an inbound anchored link | `just devlog_check` | `link ...#b9--… names no heading or explicit anchor in roadmap/00-backlog.md` |
| The store itself becomes invalid | `just tasks_check` → `myque check` | schema, graph, alias, and state findings, reported with MyQue's own status distinction |
| The anchor rules themselves regress | `just devlog_check` | `anchor control: inline code keeps its literal underscores: expected anchor 'b30--release_trust_check-was-red'` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just devlog_check` | pass — `291 entries, 291 indexed` | Direct |
| `just tasks_check` | pass — `274 items, 94 frozen backlog headings indexed` (legacy-id count no longer reported; nothing maps) | Direct |
| `just ruff` | pass — `All checks passed!` | Direct |
| `just typos` | pass, exit 0 | Direct |
| All 273 store UUIDs written into a Markdown table and spell-checked | exit 0, no false positives from hex groups | Direct |
| Mutation: `Work items \| MQ2` | fails, names the key and demands the UUID | Direct |
| Mutation: `Work items` naming `01a07762-…-000000000000` | fails as absent from the store | Direct |
| Mutation: `\| Work items \|` → `\| Roadmap \|` | fails as missing `Work items` | Direct |
| Mutation: eight correct rows plus a *trailing* `\| Roadmap \| C9.4 \|` | fails — `carries unknown field(s) Roadmap`. Before the strict parser this passed | Direct |
| Mutation: eight correct rows plus a *trailing* `\| Work items \| none \|` | fails — `repeats Work items`. Before the strict parser this passed *and* silently replaced the real UUID with `none` | Direct |
| Mutation: delete a referenced item file | `devlog_check` fails on the now-absent UUID | Direct |
| Mutation: add a `backlog`-tagged item present in no roadmap file | `tasks_check` passes — `274 items, 94 … headings` | Direct |
| Mutation: reword an *unlinked* heading (`### B92 …`) | both checkers pass: heading text carries no identity and no link names it | Direct |
| Mutation: reword the *linked* `### B9 …` heading | `devlog_check` fails — `link …#b9--… names no heading or explicit anchor`. Before fragment validation this passed | Direct |
| Mutation: delete every `### B<N>` entry, keeping the file and `## Resolved` | `tasks_check` fails — `0 heading(s), fewer than the 94 that landed`. Before the pinned floor this passed, because `0 == 0` | Direct |
| Mutation: reopen an item behind a landed heading, no active milestone | `tasks_check` **passes**: the frozen index no longer vetoes a store transition | Direct |
| Mutation: same reopen with `P5.4.2` active | fails once, and only as `backlog-first: P5.4.2 is active while backlog items are neither resolved nor explicitly deferred: B1` — the policy that owns the question | Direct |
| Full-tree audit of all 77 devlog `#fragment` links against GitHub slug rules | 4 pre-existing broken anchors found and restored via `<a id>`; the remaining 73 resolve | Direct |
| Mutation: strip the *second* `B29` section's `**Item:**` line, heading kept | `tasks_check` fails — `B29 (heading 69 of 94) has no …Item…`. Under the key-set pass this passed, because the first `B29` had already marked the key resolved | Direct |
| Mutation: same for the second `B30` | `tasks_check` fails — `B30 (heading 68 of 94)`. Also passed before | Direct |
| Control: strip the non-duplicated `B28`'s `**Item:**` line | fails both before and after — the old pass was specific to duplicated keys | Direct |
| Mutation: a link to a heading that exists *only* inside a fenced example | `devlog_check` fails; the fence manufactures no anchor. The first anchor implementation accepted it | Direct |
| Mutation: link `#tracks-1` with two `## Tracks` headings present | `devlog_check` **passes**; duplicate suffixes are computed. The first implementation rejected this valid link | Direct |
| Mutation: link the inline-code heading `` ### B30 — `release_trust_check` … `` | `devlog_check` **passes**; the underscore survives. The first implementation stripped it, producing `releasetrustcheck` | Direct |
| Control: regress `slug()` to strip code spans as markup | `devlog_check` fails with two `anchor control:` findings before reaching any entry | Direct |
| Control: regress fence tracking to ignore fences | `devlog_check` fails with three `anchor control:` findings | Direct |
| `markdown_anchors.controls()` — 11 cases, both directions | 0 failures. Two of them caught bugs in the first draft of this module: a `\x00` sentinel that punctuation-stripping deleted, and fence tracking that ignored fence length | Direct |
| Repository survey for latent exposure | 37 inline-code headings contain underscores and 39 slugs repeat across roadmap track files, so both rules are load-bearing rather than hypothetical | Direct |
| `scripts/generate/generate-system-image-closures.py --check` | `50 system-image closures are current` | Direct |
| `git status` over the fifteen declared release-input trees plus `just/` and `Justfile` | empty; no closure input changed | Direct |

## Decisions

- Decision: keep `check_backlog_index_resolves()`, but strip its terminal-state requirement.
- Rationale: the dual-write pressure came from *prose* — `roadmap/00-backlog.md` telling the reader to add a heading on close, and `AGENTS.md` repeating it — not from the checker, so the instruction was deleted and the link guard kept. Review then caught that the guard reached too far the other way: requiring `state in {done, cancelled}` let a frozen document veto a legitimate reopening in the canonical store, inverting the authority this cutover established. Link integrity and permanent closure are two invariants, and only the first belongs to a historical index. Whether a reopened defect blocks milestone work is `check_backlog_first()`'s question, and it answers it — the reopen mutation now fails only there, and only while a milestone is active.
- Rejected alternative: keeping the state check because "every entry below is closed" was true when written. That sentence was a snapshot the checker had been promoted into enforcing forever.

- Decision: validate link fragments rather than stripping them, and pin the landed heading count.
- Rationale: the first round claimed the backlog file survives to protect anchored inbound links, but nothing checked anchors — `check-devlog.py` did `target.partition("#")[0]`, and the heading/resolution count guard was satisfied by `0 == 0` when every entry was deleted. Both holes were reproduced before fixing. Fragment validation closes the first; a floor on the landed heading set closes the second. Auditing all 77 fragment links then surfaced 4 that had been broken since they were written — never matching any heading in 20 revisions of `07-architecture-portability.md` — so the addresses were restored with retained anchors rather than editing frozen devlog bodies.
- Rejected alternative: a separate link checker. Fragment validity is the same invariant as link validity, in the checker that already owns it.

- Decision: the backlog index is parsed as heading-delimited sections identified by position, never keyed by `B<N>`.
- Rationale: the first fix still collected resolved keys into a set, so with two `B29` sections the first one's UUID marked the second verified — reproduced by stripping the second `B29`'s and second `B30`'s `**Item:**` lines, both of which passed. That is the migration's original sin in miniature: a non-unique human key merging two records. Position is the only identifier available that is actually unique, and it appears in the failure text (`B29 (heading 69 of 94)`) because the key alone cannot say which section broke. Inventing new keys to disambiguate would re-introduce exactly the durable-key coupling this cutover removed.
- Rejected alternative: allocating `B29a`/`B29b`. That mints new durable keys for two frozen link targets and changes anchors 8 devlog entries depend on.

- Decision: anchor computation moves to `scripts/lib/markdown_anchors.py` with executable controls, and its scope is declared rather than described as "GitHub's rules".
- Rationale: review found three defects in the inline implementation — fenced examples manufactured anchors, duplicate headings had no suffix, and inline code lost its underscores — each of which either accepts a dead link or rejects a live one, invisibly at the destination. A rule that decides link validity has to be verifiable, so the 11 cases are executable and `check-devlog.py` runs them before trusting the computation; regressing either rule fails the gate, observed both ways. Two of those controls immediately caught bugs in my own first draft of the module. The docstring now names what is *outside* scope (Setext headings, raw `<h1>`–`<h6>`) instead of implying full fidelity, because a silent under-approximation was the original failure mode.
- Rejected alternative: a full CommonMark dependency. Anchor generation is not CommonMark — it is GitHub's post-processing of it — and adding a parser would neither answer the question nor be verifiable without these same cases.

- Decision: parse front matter as ordered rows and require the field set to equal `FIELD_ORDER` exactly.
- Rationale: the previous parser built a dict and compared the first eight keys, so it verified "the first eight distinct fields are right", not "the fields are exactly right". A trailing `Roadmap` row passed, and a trailing duplicate `Work items | none` passed *while overwriting the real UUID* — the reference silently became `none` with every check still green. Rejecting duplicates before dict conversion is what makes "exactly these eight fields" the checked claim `devlog/README.md` always stated.
- Rejected alternative: keeping the prefix comparison and adding a separate unknown-field scan, which would leave the override bug intact.

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
- [ ] Fragment validation covers links written *inside* devlog entries, which is where the anchored inbound links live. Fragments in `roadmap/`, `docs/`, and `README.md` pointing at each other are unchecked; the four broken anchors this round found were all in devlog entries, so the uncovered direction has no measured defect rate.
- [ ] Anchors for duplicate headings are positional, so `#deliverables-7` in a roadmap track file moves when a section is inserted above it. `just devlog_check` catches the resulting break, but only after the fact; `roadmap/README.md` now advises an explicit `<a id>` over relying on a duplicate's index. 39 slugs repeat across those files.
- [ ] `markdown_anchors` covers ATX headings, inline code, duplicate suffixes, and explicit anchors. Setext headings and raw `<h1>`–`<h6>` are declared out of scope rather than handled; neither appears in this repository today, and adding one would silently under-approximate until a control is added for it.

## Artifacts and provenance

- Focused report: none; the changes and their mutation evidence are in this entry.
- Raw transcript: none retained.
- Serial/debugger/model output: none; no guest code runs in these gates.
- Related work item: `01a07967-a93b-7145-a633-25ed418b3d69` (`MQ3`), depending on `01a07486-0f9c-7aac-a688-b2a01e5d5c29` (`MQ1`, the migration) and `01a07762-1202-7450-b60b-31b4210da8da` (`MQ2`, the backlog index).
