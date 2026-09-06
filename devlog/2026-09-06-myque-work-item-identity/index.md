# Work-item identity moves from roadmap headings to MyQue UUIDs

| Field | Value |
|---|---|
| Date | 2026-09-06 |
| Kind | Change |
| Status | Verified |
| Scope | `.tasks/`, `scripts/lib/{roadmap_inventory,work_items}.py`, `scripts/migrate-roadmap-to-myque.py`, `scripts/check/{check-devlog,check-work-items}.py`, `just/quality.just`, `flake.nix`, `.github/workflows/ci.yml`, `AGENTS.md`, `devlog/{README,TEMPLATE}.md`, `roadmap/` |
| Work items | 01a07486-0f9c-7aac-a688-b2a01e5d5c29 |
| Gates | `just tasks_check`, `just devlog_check`, `just ruff`, `just typos` |
| Trigger | Reviewing a handoff plan to replace roadmap-heading identity with the finished `work-item/v1` tracker in `mozufu/myque` |
| Baseline | 266 work items declared by Markdown headings under `roadmap/`; `check-devlog.py` scraping those headings for identity and carrying a 26-line hard-coded exception for `B29`/`B30`; 288 devlog entries referencing 240 distinct roadmap ids as foreign keys |

## Summary

Work-item identity was the text of a Markdown heading. `check-devlog.py`
established the id set by scraping `roadmap/*.md`, validated 837 devlog
references against it, and carried `LEGACY_ROADMAP_ID_COLLISIONS` — four frozen
heading signatures — because `B29` and `B30` had each been allocated twice, so
those ids identified nothing. Identity now lives in `.tasks/items/`, one
`work-item/v1` file per item under a canonical UUIDv7, with 262 unique roadmap
ids retained as display-only MyQue keys and the four colliding items
deliberately keyless. `check-devlog.py` resolves a reference against that store
and no longer parses a heading; the collision machinery is deleted rather than
carried forward. No merged devlog entry was rewritten.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `.tasks/items/` | 272 canonical `work-item/v1` files: 266 roadmap declarations, 5 keyless deferred follow-ups, and this migration's own item | Identity is an immutable UUID, not a heading |
| `.tasks/legacy-roadmap-ids.json` | Committed map of 262 unique roadmap ids to UUIDs, plus both collisions with their source coordinates and the one devlog reference resolved from evidence | Pre-migration references resolve without headings staying authoritative |
| `scripts/lib/roadmap_inventory.py` | Parses declarations, states, closure dates, parents, and `**Depends on:**` prose | One narrow reader of the legacy format, used by the migration only |
| `scripts/migrate-roadmap-to-myque.py` | Allocates a UUIDv7 per declaration, backdated to the item's closure date, and writes the store and the map | The import is deterministic and re-runnable from the same input |
| `scripts/lib/work_items.py` | Resolves a reference against `.tasks/items/` filenames; exposes the shallow fields policy checks need | The filename set is the identity set, per specification §3 |
| `scripts/check/check-devlog.py` | Resolves work-item references through the store; `ROADMAP_HEADING`, `ROADMAP_ID_DECLARATION`, `LEGACY_ROADMAP_ID_COLLISIONS`, `prove_roadmap_collision_guard`, `roadmap_ids`, and heading-derived `KNOWN_IDS` deleted | No checker derives identity from a heading |
| `scripts/check/check-work-items.py` | Runs `myque check`, then repository policy: backlog-first, legacy-map integrity, undated-closure disclosure | Store validity stays MyQue's; policy stays Slime OS's |
| `just/quality.just` | `tasks_check`, `tasks_list`, `tasks_next`, `tasks_graph` | The MyQue binary dependency is confined to one target |
| `flake.nix` | `myque` input with `inputs.nixpkgs.follows`; `myque-bin` in the default shell | One nixpkgs, one GHC closure |
| `.github/workflows/ci.yml` | New `work_items` job with Nix quick-install plus a flake-keyed store cache | `docs_gates` stays a checkout-plus-`just` job |
| `roadmap/`, `AGENTS.md`, `devlog/{README,TEMPLATE}.md` | Non-authoritative banners; identity, backlog, and front-matter guidance rewritten | Guidance names the canonical store |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A devlog names a work item that does not exist | `just devlog_check` | `resolves to no work item in .tasks/items/ and is absent from the legacy id map` |
| A colliding id is silently resolved to one allocation | `just devlog_check` | `a roadmap id that was allocated twice; no evidence resolves which item this entry meant` |
| The legacy map drifts from the store | `just tasks_check` | `legacy id <id> maps to <uuid>, which is not in .tasks/items` |
| A collision is reintroduced as a key | `just tasks_check` | `must not be carried as a key: the UUID distinguishes the two items` |
| A synthetic closure date reads as an observation | `just tasks_check` | `closed on the migration date without saying the original date is unrecorded` |
| Store-level identity, cycle, or schema breakage | `just tasks_check` (`myque check`) | `myque check reported findings: …` |

Each of the first five was verified by mutation, not by inspection: see
*Verification*.

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just tasks_check` | `work-item check passed: 272 items, 262 legacy ids mapped` | Direct |
| `just devlog_check` | `devlog check passed: 288 entries, 288 indexed` — the pre-migration baseline | Direct |
| `git diff --stat devlog/` after the checker rewrite | Empty: no merged entry body modified | Direct |
| `git diff --stat roadmap/` | 13 files, 70 insertions, 1 deletion — banners only, no item content removed | Direct |
| Mutation: devlog reference `Z99` | Failed with the unresolvable-reference message | Direct |
| Mutation: `B30` in an entry with no recorded resolution | Failed closed, naming the ambiguity | Direct |
| Mutation: `B30`'s resolved entry, and a UUID under a renamed `Work items` field | Both passed | Direct |
| Mutation: legacy map pointed at a non-existent UUID | Failed with the drift message | Direct |
| Mutation: undated-closure disclosure stripped from one item | Failed with the disclosure message | Direct |
| `ruff check scripts/` | `All checks passed!` | Direct |
| `typos` | Clean over 272 UUID-named files | Direct |
| `nix develop --command myque check` | `checked 271 work item(s): no findings` on the locked `db1c81f`, confirming the store does not depend on the unpublished CLI fixes | Direct |
| Dependency cycle `P3 → P3.E → P3` | Caught by `myque check` on the first import; fixed at the source (see *Decisions*) | Direct |
| `myque list --state <s>` across all six states | 218 done, 42 deferred, 8 cancelled, 3 blocked, 1 active, 0 open — 272 total | Direct |
| `myque next` | Empty. The only advanceable item is `P5.4.2` (`active`), and `M5.7`/`H1`/`RP3` are blocked on hardware evidence that does not exist | Direct |

## Decisions

- Decision: The legacy id map is committed data; merged devlog front matter is not rewritten.
- Rationale: `AGENTS.md` requires a landed entry to be preserved, with corrections appended rather than edited in. Rewriting 837 references across 288 entries would violate that and would put 36-character UUIDs into a 288-row index table. A committed map removes heading parsing just as completely, because the map is data rather than a parse of live documents.
- Rejected alternative: Rewriting every historical `Roadmap` field to UUIDs, as the original plan proposed.

- Decision: `check-devlog.py` reads `.tasks/items/` filenames; it does not shell out to `myque`.
- Rationale: The specification makes the filename equal the canonical id and makes any disagreement a `myque check` finding, so the filename set is the identity set. It also keeps `devlog_check` binary-free, so `docs_gates` remains a checkout-plus-`just` job instead of acquiring a GHC closure. Delegating would have meant 837 subprocess spawns or screen-scraping a padded table.
- Rejected alternative: Resolving each reference through `myque show`.

- Decision: Dependency edges come from each item's `**Depends on:**` first sentence, with the track diagram filling in only items whose prose is silent.
- Rationale: 137 items declare dependencies in prose — three times the coverage of `roadmap/README.md`'s diagram, whose 67 hops touch six endpoints (`Backlog`, `Foundations`, `Duo`, `RV64`, `H1V1`, `Framework`) that are prose aggregates with no item behind them. Reading a whole paragraph over-collects: `P3` depends on `P5`, then explains that "P3.D independently supplies the physical board loop that P3.E consumes", which as edges made `P3` depend on its own children and closed a cycle `myque check` caught on the first import. The diagram then contributes 5 edges — `CP0→CP1`, `CP2→CP3`, `IO4→R1`, `RP8→R1`, `R1→R2` — for items that declare no `**Depends on:**` line at all, each corroborated in the item's own body (`CP1`'s names `CP0`; `R1`'s says it "expands R0"). It is read narrowly: solid `-->` hops only, so `X1 -.->|only if chosen| RP6` stays out rather than asserting a choice nobody made, and prose wins wherever prose speaks — `RP0`'s line names `P0`/`P1` where the diagram draws `RP0 --> RP1`, and `C9`'s names `C9.2`/`C9.4` where the diagram draws the coarser `C9 --> IO0`.
- Rejected alternative: Prose only, which would have left five items edgeless; or the diagram wholesale, which would have imported conditional hops and coarser restatements as hard prerequisites.

- Decision: 34 items whose closure date is unrecoverable carry the migration date and say so in their body, enforced by a gate.
- Rationale: `work-item/v1` requires `closed` on a terminal item, and no status line, section text, or linked devlog entry dates these 34 — mostly M-track work predating the devlog. Recovery found 108 dates on status lines, 79 from linked devlog folders, and 4 elsewhere. Inventing the rest would contradict the rule that a closure is an observed fact, so each states plainly that its original date is unrecorded.
- Rejected alternative: Backfilling from `git log` file dates, which record when a file changed, not when work closed.

- Decision: The undated-closure gate is scoped to items the migration imported, identified through the legacy map.
- Rationale: Its first form flagged any item closed on the migration date, which fired on this change's own milestone the moment `myque close` recorded a genuine, observed closure. Left unscoped, the gate would pressure the next author into writing a disclaimer that is not true — a checker manufacturing the uncertainty it exists to expose. Scoping keeps the mutation test green: stripping the disclosure from an imported item still fails.
- Rejected alternative: Exempting the date, or asking authors to close items on a different day.

- Decision: The five deferred backlog follow-ups become keyless `followup` items.
- Rationale: They are the only open work the backlog admits to, tracked as prose bullets under one shared heading with no id. Omitting them would silently lose that work from the canonical store; minting `B61a` would be the artificial naming this migration exists to avoid. Each records its owning item as a dependency, which is the relation the prose states.
- Rejected alternative: Leaving them in Markdown, or inventing ids.

- Decision: An item's state comes from its own `**Status:**`, except that a track the roadmap's status table defers overrides an otherwise-available item to `deferred`.
- Rationale: Item-level status alone put 33 explicitly-postponed milestones into `myque next` — `H8`'s desktop shell, `A1`'s revocation work, `X1`'s Linux personality — because `H2` reads "Not started" on its own line while the hardware track above it is deferred. A store that answers "what is next" with work the repository has postponed manufactures exactly the false confidence the roadmap's own sequencing exists to prevent. The override is read from `roadmap/README.md`'s track table rather than each track's preamble, because the preamble disagrees: `06-authority-trust.md` opens "Planned; A1–A5 are not implemented" while the table says "Deferred", and the table is the column a reader consults to decide what to start. Only `open` is overridden — a deferral says nothing about work already finished, abandoned, or blocked on something concrete — so `H1`, `RP3`, and `M5.7` stay `blocked` and the 226 items already in a terminal state are untouched.
- Rejected alternative: Item status alone, which over-reports actionable work; or a hard-coded list of deferred tracks, which would drift from the roadmap the moment a track resumes.

## Open risks and follow-ups

- [ ] `flake.lock` pins myque `db1c81f`, which predates the store-relative abbreviation fix. `myque check` is unaffected and this store validates under both revisions, but `myque list` on that revision prints the first UUID group for every keyless item — one repeated token for the nine keyless items here, because a UUIDv7's first group is the top 32 bits of its millisecond timestamp and advances only every 65.536 s. Run `nix flake update myque` once the fix is published.
- [ ] `myque-bin` has no published binary cache, so a cold `work_items` CI run builds it through GHC: 117 fetched paths, 516 MiB, 3.4 GiB unpacked on `x86_64-linux`. The job is cached on `flake.lock`; publishing to a binary cache would remove the cost.
- [ ] 50 devlog links carry `roadmap/` heading anchors. They still resolve because the item sections were kept, but `check-devlog.py` validates only the file, not the anchor, so deleting a section later would rot them silently.
- [ ] `roadmap/README.md`'s track table says "CP0–CP10 complete; CP11–CP15 planned", but each of CP11–CP15 declares Complete or Delivered in its own section, and the store carries them as `done`. The summary is stale, not the items. Left as found rather than edited here, because reconciling a track narrative is a separate change from moving identity — but a generated or manual view contradicting the canonical store is exactly what must not persist.
- [ ] 5 items — `C8.13.1`, `C8.13.2`, `P3.D`, `IO2`, `IO4` — are `done` with named unfinished residue, faithfully reproducing the roadmap's deliberately narrowed exit conditions. Readiness propagates from that `done`, so a consumer must read the exit condition rather than the state alone.
- [ ] `devlog/README.md` remains a 288-row central index every new entry appends to, so branch-concurrent devlog entries still conflict even though work items no longer do.

## Artifacts and provenance

- Focused report: this entry
- Raw transcript: none; every result above is a checker exit line reproduced from the gates named in *Verification*
- Serial/debugger/model output: none — no guest code runs in this change
- Related work item: `01a07486-0f9c-7aac-a688-b2a01e5d5c29` (`MQ1`)
