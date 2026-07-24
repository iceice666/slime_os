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

## Entry format

Create entries as `YYYY-MM-DD-short-topic.md` using [TEMPLATE.md](TEMPLATE.md). One entry may cover several related failures when they share an investigation or verification campaign.

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

## Evidence rules

- Prefer exact `just` targets and exit results over prose such as “tests passed.”
- Mark results copied from an older report as **inherited evidence** and link the source.
- Mark unobserved conclusions as **[INFERENCE]**.
- Preserve raw logs outside the curated entry when they are large; link them rather than pasting them.
- Never place credentials, account banners, tokens, or unrelated terminal metadata in curated entries.
- Roadmap completion remains authoritative in `roadmap/`; devlog entries explain how conclusions were reached.
