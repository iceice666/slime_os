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
| 2026-07-24 | [Root workspace and tooling layout](2026-07-24-root-workspace-layout/index.md) | Verified | Root Cargo workspace, categorized host tooling, LLDB helper, artifact paths |
| 2026-07-24 | [Kernel layout and generated bindings cleanup](2026-07-24-kernel-layout-cleanup/index.md) | Verified | Kernel subsystem directories, shared Zutai Rust bindings, contract generators |
| 2026-07-24 | [Stage-0 boot-check hangs](2026-07-24-boot-check-hangs/index.md) | Verified | Stack guard, vmm walkers, dango termination, gen-99 init, bootstate model, generation build |
| 2026-07-24 | [generation_cmd_check wrong target](2026-07-24-generation-cmd-check-wrong-target.md) | Verified | Generation staging fixture, `just generation_cmd_check` |
| 2026-07-24 | [C7.1 generation v3 + u64 rights](2026-07-24-c7-1-generation-v3-u64-rights.md) | Verified | Generation format v3, capability rights, spawn-grant ABI, host builder/checkers |
| 2026-07-24 | [C7 finer decomposition](2026-07-24-c7-finer-decomposition.md) | Proposed | C7.2–C7.7 state surfaces, dependencies, and verification gates |
| 2026-07-24 | [Multi-architecture roadmap boundary](2026-07-24-multi-architecture-roadmap/index.md) | Proposed | Exact target/artifact contracts, x86-64 boundary, AArch64-first, RV64, MCU companion scope |
| 2026-07-24 | [B2 scheduler Blocked task state](2026-07-24-b2-blocked-task-state/index.md) | Verified | Blocked task state, SYS_WAIT wait-set, IPC/input/supervision wakes, on_idle rework, copy_from_current grant bound |
| 2026-07-24 | [C7.2 shared-buffer factory allocation](2026-07-24-c7-2-shared-buffer-factory.md) | Verified | SharedBufferFactory object, RIGHT_BUFFER_CREATE, bounded shared-buffer table, shared contiguous allocator, syscall ABI, host builder/checkers |
| 2026-07-24 | [C7.3 shared-buffer accounting](2026-07-24-c7-3-shared-buffer-accounting.md) | Verified | Shared-buffer budget contract, per-holder quotas charged to supervision subtree, reclamation on terminate, generation-decode validation, host bindings/checkers |
| 2026-07-24 | [C7.4 shared-buffer mapping and read-only sealing](2026-07-24-c7-4-shared-buffer-mapping.md) | Verified | Map/unmap/seal syscalls, user-half vmm primitives, irreversible Arc-shared seal, mapping quota + MAX_MAPPINGS, teardown-before-free, host allowlist |
| 2026-07-25 | [C7.5 shared-buffer loan/return lifecycle](2026-07-25-c7-5-shared-buffer-loan.md) | Verified | Loan/map/return/revoke syscalls, RIGHT_BUFFER_LOAN + SharedBufferLoan object, receiver-bound single-return identity, retained-while-loaned pages, MAX_LOANS, peer-fault reclamation, v2/v3 rights masks, host allowlist |
| 2026-07-25 | [C7.6 versioned sample descriptor](2026-07-25-c7-6-sample-descriptor.md) | Verified | Zutai sample-descriptor contract, WireSampleDescriptor bindings, valid_sample_descriptor bounds, DESCRIPTOR_LEN==MAX_MSG, loan-referenced payload over shared buffer, QEMU gate |
| 2026-07-25 | [C7.7 sample-plane integration and isolation](2026-07-25-c7-7-sample-plane-integration.md) | Verified | Two-component sample-plane composition over C7.2–C7.6, real-channel descriptor exchange, four quota classes, peer-death reclamation, unrelated-channel isolation, retained-v2 decode probe, `just sample_plane_check` |

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
