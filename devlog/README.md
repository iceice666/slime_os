# Slime OS development log

This directory is the curated, chronological record of investigations, regressions, design decisions, and verification results. It complements—not replaces—the canonical roadmap, focused incident reports, raw transcripts, and machine-readable evidence.

## Goals

- Make regressions searchable by symptom, root cause, affected check, and guard.
- Preserve the evidence chain from observation through fix and verification.
- Record decisions that change future debugging or CI practice.
- Separate directly observed results from inherited reports and unresolved hypotheses.

## Layout

Every entry is a folder. There is no flat-file form.

```text
devlog/
  README.md                      This file: the format contract and the entry index
  TEMPLATE.md                    The section skeleton every index.md is written from
  YYYY-MM-DD-short-topic/
    index.md                     The curated entry; the only file other documents link to
    transcript.txt               Optional: the raw, never-edited session log
    <focused-report>.md          Optional: a deep analysis too long for the entry body
    <capture>.{log,jsonl,txt}    Optional: serial captures, model output, traces
```

Rules:

- **One shape.** A folder even when it has no evidence siblings, so adding a transcript later never moves the entry or breaks an inbound link.
- **Folder name** is `YYYY-MM-DD-short-topic`, lowercase kebab-case, dated by first investigation, not by fix. Backlog and milestone entries lead with the id: `2026-07-26-b3-…`, `2026-07-25-c7-6-…`.
- **`index.md` is the only entry point.** Other documents link to `devlog/<folder>/`; they never link an evidence sibling directly except to cite a specific transcript section.
- **Every sibling is linked** from `index.md`'s *Artifacts and provenance*. An unreferenced file in an entry folder is a checker failure.
- **Evidence lives with its entry.** Do not add a shared `assets/` or `transcripts/` directory; provenance travels with the write-up.
- One entry may cover several related failures when they share an investigation or verification campaign (`2026-07-26-b7-b8-budget-hygiene/`).

## Entry format

Write `index.md` from [TEMPLATE.md](TEMPLATE.md): an `# H1` title, the front-matter table, then `##` sections in template order.

### Front matter

Exactly these eight fields, in this order:

| Field | Content |
|---|---|
| `Date` | `YYYY-MM-DD`; must equal the folder's date prefix. |
| `Kind` | `Defect`, `Change`, `Audit`, or `Decision` — selects the required sections. |
| `Status` | One token from the status vocabulary below. Nothing else: no parentheticals, no dates, no prose. |
| `Scope` | Subsystems, files, and checks touched. |
| `Roadmap` | Comma-separated milestone/backlog ids (`C7.4, B3`), each resolving to a real `roadmap/` heading, or `none`. |
| `Gates` | The narrowest `` `just <target>` `` commands guarding this entry's claim, or `none`. Every name must be a real Justfile target. |
| `Trigger` | Commit, change, or first observed condition. |
| `Baseline` | Last known-good behavior or invariant. |

`Roadmap` and `Gates` exist so "which entry covers C7.4?" and "what evidence backs `transfer_check`?" are answerable by reading the index and grepping front matter, instead of reading twenty bodies.

A `Status` of `Fixed`, `Verified`, or `Monitoring` asserts an observed result, so it requires at least one gate. `Gates` names the *guards*, not every command run — the exhaustive list belongs in *Verification*. Repository hygiene targets (`fmt_check`, `lint`, `framework_safety_check`, and the `_components` variants) are assumed on every permanent Rust change and are not listed as gates.

### Kinds

| Kind | Use for | Required sections |
|---|---|---|
| **Defect** | A regression, bug, or wrong claim: something behaved incorrectly. | Every section in the template. |
| **Change** | A milestone or feature landing correctly the first time. | Summary, Changes, Regression guards, Verification, Decisions, Open risks and follow-ups, Artifacts and provenance. |
| **Audit** | A verification campaign over existing work, whether or not it finds defects. | Summary, Observable symptom, Investigation log, Changes, Verification, Open risks and follow-ups, Artifacts and provenance. |
| **Decision** | A design, sequencing, or architecture decision, typically `Proposed`. | Summary, Changes, Decisions, Open risks and follow-ups, Artifacts and provenance. |

Sections beyond the required set are welcome when they carry evidence; drop the ones your kind does not require rather than filling them with "n/a". Sections always keep template order, and no heading outside the template may be introduced without extending `TEMPLATE.md` and the checker together.

Every **Defect** entry must identify:

1. **Trigger and baseline** — what changed and what previously worked.
2. **Observable symptom** — exact command, exit code, serial marker, fault, or timeout.
3. **Root cause** — source-level mechanism, not the first visible crash site.
4. **Fix** — changed invariant or behavior.
5. **Regression guard** — the narrowest check that would fail if the bug returns.
6. **Verification** — commands and observed results, with inherited evidence labeled.
7. **Artifacts** — reports, serial logs, debugger captures, traces, or transcripts.
8. **Open risks** — anything not established by the recorded evidence.

## Status vocabulary

- **Investigating** — reproduced, root cause not established.
- **Root-caused** — mechanism established, fix incomplete.
- **Fixed** — implementation changed, narrow reproduction passes.
- **Verified** — affected behavior and relevant regression guards pass.
- **Monitoring** — resolved, but awaiting broader or physical evidence.
- **Proposed** — design or tooling decision not yet implemented.

## Entry immutability

A published entry is a fixed record of what was observed, not a live tracker. Once an entry is committed:

- **Frozen:** the curated `index.md` body — summary, investigation log, root cause, changes, verification results — and every evidence sibling (focused reports, `transcript.txt`, captures). Do not rewrite an observed result, a raw log, or the reasoning that led to it. Corrections go in a new dated note appended under a `## Corrections` heading (with the date and what changed), never by editing the original claim.
- **Mutable:** the front-matter `Status` field as the situation evolves (e.g. `Verified` → `Monitoring` once physical evidence lands), and cross-links in *Open risks and follow-ups*. Keep the live truth in `roadmap/` and `roadmap/00-backlog.md`; the entry only points at those canonical homes, so downstream state changes never require editing the frozen body.

When `Status` changes, update the same entry's row in the index below; the checker requires the two to agree.

## Evidence rules

- Prefer exact `just` targets and exit results over prose such as "tests passed."
- Mark results copied from an older report as **inherited evidence** and link the source.
- Mark unobserved conclusions as **[INFERENCE]**.
- Preserve raw logs as evidence siblings rather than pasting them into the entry; never edit one after the fact.
- Never place credentials, account banners, tokens, or unrelated terminal metadata in curated entries.
- Roadmap completion remains authoritative in `roadmap/`; devlog entries explain how conclusions were reached.

## Checking

`just devlog_check` (`scripts/check/check-devlog.py`) enforces everything above that is mechanically checkable: folder shape and naming, front-matter field set and order, `Kind`/`Status` vocabulary, `Roadmap` ids against real roadmap headings, `Gates` against real Justfile targets, required sections per kind and their order, sibling files linked from their entry, index/entry agreement on date and status, and every `devlog/...` path referenced anywhere in the repository resolving to a real file. It runs no guest code, so it is cheap enough to run on any documentation change.

## Entries

| Date | Entry | Kind | Status | Roadmap |
|---|---|---|---|---|
| 2026-07-24 | [B2 — scheduler Blocked task state (busy-poll pathology)](2026-07-24-b2-blocked-task-state/index.md) | Defect | Verified | B2 |
| 2026-07-24 | [Stage-0 boot-check hangs: stack overflow and dango REPL](2026-07-24-boot-check-hangs/index.md) | Defect | Verified | B1, M5.6, M5.6c, M6.3, M6.4 |
| 2026-07-24 | [C7.1 — Generation format v3 and u64 rights](2026-07-24-c7-1-generation-v3-u64-rights/index.md) | Change | Verified | C7.1 |
| 2026-07-24 | [C7.2 — Shared-buffer authority and factory allocation](2026-07-24-c7-2-shared-buffer-factory/index.md) | Change | Verified | C7.2 |
| 2026-07-24 | [C7.3 — Generation quotas and supervision-subtree accounting](2026-07-24-c7-3-shared-buffer-accounting/index.md) | Change | Verified | C7.3 |
| 2026-07-24 | [C7.4 shared-buffer mapping and read-only sealing](2026-07-24-c7-4-shared-buffer-mapping/index.md) | Change | Verified | C7.4 |
| 2026-07-24 | [C7 — Finer shared-sample-plane decomposition](2026-07-24-c7-finer-decomposition/index.md) | Decision | Proposed | C7 |
| 2026-07-24 | [generation_cmd_check corrupted the wrong generation](2026-07-24-generation-cmd-check-wrong-target/index.md) | Defect | Verified | B1 |
| 2026-07-24 | [Kernel layout and generated bindings cleanup](2026-07-24-kernel-layout-cleanup/index.md) | Change | Verified | none |
| 2026-07-24 | [Multi-architecture roadmap boundary](2026-07-24-multi-architecture-roadmap/index.md) | Decision | Proposed | P0, P1, P2, P3, P4 |
| 2026-07-24 | [Root workspace and tooling layout](2026-07-24-root-workspace-layout/index.md) | Change | Verified | none |
| 2026-07-25 | [C7.5 shared-buffer loan/return lifecycle and fault reclamation](2026-07-25-c7-5-shared-buffer-loan/index.md) | Change | Verified | C7.5 |
| 2026-07-25 | [C7.6 versioned sample descriptor](2026-07-25-c7-6-sample-descriptor/index.md) | Change | Verified | C7.6 |
| 2026-07-25 | [C7.7 sample-plane integration and isolation](2026-07-25-c7-7-sample-plane-integration/index.md) | Change | Verified | C7.7 |
| 2026-07-26 | [B3 — C7.5 full-graph boot wedge: shared-buffer table overflowed the kernel stack](2026-07-26-b3-shared-buffer-table-stack-overflow/index.md) | Defect | Verified | B3, C7.5 |
| 2026-07-26 | [B4 — wiring the shared-buffer budget and factory into a real generation](2026-07-26-b4-live-shared-buffer-budget/index.md) | Defect | Verified | B4, C7.2, C7.3, C7.7 |
| 2026-07-26 | [B5 — driving the shared-buffer syscalls from real components](2026-07-26-b5-live-sample-plane/index.md) | Defect | Verified | B5, C7.2, C7.4, C7.5, C7.7 |
| 2026-07-26 | [B6 — scoping the retained-v2 rollback claim to what is provable](2026-07-26-b6-retained-v2-rollback-scope/index.md) | Defect | Verified | B6, C7.1, C7.7 |
| 2026-07-26 | [B7/B8 — manifest rights vocabulary and budget aggregate bounds](2026-07-26-b7-b8-budget-hygiene/index.md) | Defect | Verified | B7, B8, C7.1, C7.3 |
| 2026-07-26 | [C7 milestone audit — boot wedge and unproven exit conditions](2026-07-26-c7-audit/index.md) | Audit | Verified | C7, B3, B4, B5, B6, B7, B8 |
| 2026-07-27 | [Devlog format contract, uniform layout, and enforcement gate](2026-07-27-devlog-format-and-gate/index.md) | Change | Verified | none |
| 2026-07-27 | [C8 — Native typed data fabric decomposition](2026-07-27-c8-sub-milestones/index.md) | Decision | Proposed | C8 |
