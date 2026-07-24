# Slime OS development log

This directory is the curated, chronological record of investigations, regressions, design decisions, and verification results. It complements—not replaces—the canonical roadmap, focused incident reports, raw transcripts, and machine-readable evidence.

## Goals

- Make regressions searchable by symptom, root cause, affected check, and guard.
- Preserve the evidence chain from observation through fix and verification.
- Record decisions that change future debugging or CI practice.
- Separate directly observed results from inherited reports and unresolved hypotheses.

## Entries

| Date | Entry | Status | Scope |
|---|---|---|---|
| 2026-07-24 | [Stage-0 boot-check hangs](2026-07-24-boot-check-hangs/index.md) | Verified | Stack guard, vmm walkers, dango termination, gen-99 init, bootstate model, generation build |

## Entry format

Create each entry as a folder `YYYY-MM-DD-short-topic/` with a curated `index.md` written from [TEMPLATE.md](TEMPLATE.md). Keep the entry's focused reports, raw transcript, and other evidence as siblings inside that folder so the write-up and its provenance travel together. A single-file `YYYY-MM-DD-short-topic.md` is acceptable when an entry has no accompanying evidence files. One entry may cover several related failures when they share an investigation or verification campaign.

Every regression entry should identify:

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
- **Mutable:** the front-matter **Status** field as the situation evolves (e.g. `Verified` → `Monitoring` once physical evidence lands), and cross-links in **Open risks and follow-ups**. Keep the live truth in `roadmap/` and `roadmap/00-backlog.md`; the entry only points at those canonical homes, so downstream state changes never require editing the frozen body.

## Evidence rules

- Prefer exact `just` targets and exit results over prose such as “tests passed.”
- Mark results copied from an older report as **inherited evidence** and link the source.
- Mark unobserved conclusions as **[INFERENCE]**.
- Preserve raw logs outside the curated entry when they are large; link them rather than pasting them.
- Never place credentials, account banners, tokens, or unrelated terminal metadata in curated entries.
- Roadmap completion remains authoritative in `roadmap/`; devlog entries explain how conclusions were reached.
