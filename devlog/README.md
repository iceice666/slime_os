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
- **Mutable:** the front-matter `Status` field as the situation evolves (e.g. `Verified` → `Monitoring` once physical evidence lands), and cross-links in *Open risks and follow-ups*. Keep the live truth in `roadmap/` and `roadmap/00-backlog.md`; the entry only points at those canonical homes, so downstream state changes never require editing the frozen body. The direction is now explicit both ways: `roadmap/` holds current status and the observed exit condition; this entry holds the investigation and evidence behind it. Neither restates the other.

When `Status` changes, update the same entry's row in the index below; the checker requires the two to agree.

## Evidence rules

- Prefer exact `just` targets and exit results over prose such as "tests passed."
- Mark results copied from an older report as **inherited evidence** and link the source.
- Mark unobserved conclusions as **[INFERENCE]**.
- Preserve raw logs as evidence siblings rather than pasting them into the entry; never edit one after the fact.
- Never place credentials, account banners, tokens, or unrelated terminal metadata in curated entries.
- Roadmap completion remains authoritative in `roadmap/`; devlog entries explain how conclusions were reached.

## Checking

`just devlog_check` (`scripts/check/check-devlog.py`) enforces everything above that is mechanically checkable: folder shape and naming, front-matter field set and order, `Kind`/`Status` vocabulary, `Roadmap` ids against real roadmap headings, `Gates` against real Justfile targets, required sections per kind and their order, table rows whose cell count matches their header (an unescaped `|` inside a cell silently splits the row, so write `\|`), sibling files linked from their entry, index/entry agreement on date and status, and every `devlog/...` path referenced anywhere in the repository resolving to a real file. It runs no guest code, so it is cheap enough to run on any documentation change.

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
| 2026-07-27 | [C8.1 — Deterministic interface schemas and native bindings](2026-07-27-c8-1-interface-schemas/index.md) | Change | Verified | C8.1 |
| 2026-07-27 | [C8.2 — Generation graph, QoS, and aggregate admission](2026-07-27-c8-2-fabric-graph-admission/index.md) | Change | Verified | C8.2 |
| 2026-07-27 | [C8.3 — Attenuated endpoint provisioning and control plane](2026-07-27-c8-3-fabric-authority/index.md) | Change | Verified | C8.3 |
| 2026-07-27 | [D1–D7 — Native development, live update, and on-device build roadmap](2026-07-27-d1-d7-native-development-roadmap/index.md) | Decision | Proposed | D1, D2, D3, D4, D5, D6, D7, P0 |
| 2026-07-28 | [C10 — Bounded private component memory](2026-07-28-c10-private-component-memory/index.md) | Decision | Proposed | C10, C10.1, C10.2, C10.3, C10.4, B9, C7 |
| 2026-07-28 | [C8.4 — Bounded many-to-many streams](2026-07-28-c8-4-bounded-streams/index.md) | Change | Verified | C8.4 |
| 2026-07-28 | [B9 — terminated tasks are never reaped, so their frames never return](2026-07-28-b9-task-frame-reclamation/index.md) | Defect | Verified | B9, C10 |
| 2026-07-28 | [C8.5 — Reliable, retained, and timed QoS](2026-07-28-c8-5-fabric-qos/index.md) | Change | Verified | C8.5 |
| 2026-07-28 | [C8.6 — Bounded native calls](2026-07-28-c8-6-bounded-native-calls/index.md) | Change | Verified | C8.6 |
| 2026-07-29 | [C8.7 — Native operations](2026-07-29-c8-7-native-operations/index.md) | Change | Verified | C8.7 |
| 2026-07-30 | [C8.8 — Filtered introspection and declared interposition](2026-07-30-c8-8-filtered-introspection-interposition/index.md) | Change | Verified | C8.8 |
| 2026-07-30 | [C8.9–C8.15 — Full-graph fabric integration decomposition](2026-07-30-c8-9-integration-decomposition/index.md) | Decision | Proposed | C8, C8.9, C8.10, C8.11, C8.12, C8.13, C8.14, C8.15 |
| 2026-07-30 | [C8.9 — Typed full-profile and resource-bound closure](2026-07-30-c8-9-typed-fabric-profile/index.md) | Change | Verified | C8.9 |
| 2026-07-30 | [Verification tooling: full-crate gates, dependency checks, and CI](2026-07-30-verification-tooling/index.md) | Change | Verified | none |
| 2026-07-30 | [C8.10 groundwork — Declared route-worker partition and wait-source bounds](2026-07-30-c8-10-route-worker-partition/index.md) | Change | Verified | C8.10 |
| 2026-07-31 | [C8.10 — Collision-free full-graph boot and live bounded route workers](2026-07-31-c8-10-full-graph-boot/index.md) | Change | Verified | C8.10 |
| 2026-07-31 | [RPi5 ROS 2 two-node roadmap pivot](2026-07-31-rpi5-ros2-roadmap-pivot/index.md) | Decision | Proposed | RP0, RP1, RP2, RP3, RP4, RP5, RP6, RP7, RP8, R0, P0, P1, P2, P4 |
| 2026-07-31 | [Boot capability layout is a positional convention, not generation data](2026-07-31-boot-layout-positional-coupling/index.md) | Decision | Proposed | B10, B11, P1, P2, RP2, C8, C8.10 |
| 2026-07-31 | [Boot-layout equivalence baseline](2026-07-31-boot-layout-baseline/index.md) | Change | Verified | B10 |
| 2026-08-01 | [Init's capability layout resolves from generation data](2026-08-01-boot-layout-resolution/index.md) | Change | Verified | B10 |
| 2026-08-01 | [B11 — Product boot profiles exclude verification scaffolding](2026-08-01-b11-product-boot-profiles/index.md) | Defect | Verified | B11 |
| 2026-08-02 | [RP0 — Raspberry Pi 5 ROS 2 demo contract](2026-08-02-rp0-demo-contract/index.md) | Change | Verified | RP0 |
| 2026-08-02 | [P0 — Architecture, target, and executable-artifact contracts](2026-08-02-p0-architecture-contracts/index.md) | Change | Verified | P0 |
| 2026-08-02 | [P1 — x86-64 architecture boundary extraction](2026-08-02-p1-x86-boundary-extraction/index.md) | Change | Verified | P1 |
| 2026-08-03 | [P2.1 — AArch64 firmware handoff, EL1 entry, and translation tables](2026-08-03-p2-1-aarch64-boot/index.md) | Change | Verified | P2.1, P2 |
| 2026-08-03 | [RP1 — Target-qualified build and admission path](2026-08-03-rp1-target-qualified-artifacts/index.md) | Change | Verified | RP1 |
| 2026-08-03 | [P2.2 — AArch64 exception vectors, fault decoding, and `svc` entry](2026-08-03-p2-2-aarch64-traps/index.md) | Change | Verified | P2.2, P2 |
| 2026-08-03 | [P5.1 — Substituting seL4 for the custom microkernel](2026-08-03-p5-1-sel4-cutover/index.md) | Change | Verified | P5.1 |
| 2026-08-04 | [P5.2 — Native component images on seL4](2026-08-04-p5-2-native-component-images/index.md) | Change | Verified | P5.2 |
| 2026-08-04 | [P5.3.1 — The channel plane on seL4](2026-08-04-p5-3-1-channel-plane/index.md) | Change | Verified | P5.3.1, P5.3, P5.5, B12 |
| 2026-08-04 | [P5.3.2 — The loan plane and generation-declared quotas on seL4](2026-08-04-p5-3-2-loan-plane/index.md) | Change | Verified | P5.3.2, B13 |
| 2026-08-05 | [P5.3.3 — Child construction and supervision on seL4](2026-08-05-p5-3-3-spawn-plane/index.md) | Change | Verified | P5.3.3, B13, B10 |
| 2026-08-05 | [P5.3.4 — The C7 sample plane composed on seL4](2026-08-05-p5-3-4-sample-plane/index.md) | Change | Verified | P5.3.4, P5.3, B14 |
| 2026-08-05 | [P5.5.1 — Narrow-on-transfer provisioning on seL4](2026-08-05-p5-5-1-typed-fabric/index.md) | Change | Verified | P5.5.1, P5.5, B15, B17 |
| 2026-08-05 | [P5.5.2 — The full stream plane, unmodified, on seL4](2026-08-05-p5-5-2-stream-plane/index.md) | Change | Verified | P5.5.2, P5.5, B17 |
| 2026-08-06 | [B19 — the seL4 prefix pins bound the dev-shell derivation hash, not the toolchain](2026-08-06-b19-sel4-prefix-pin-shell-coupling/index.md) | Defect | Verified | B19 |
| 2026-08-06 | [B20 — the prefix pin held for one platform at a time](2026-08-06-b20-cross-platform-kernel-identity/index.md) | Defect | Verified | B20, B19 |
| 2026-08-06 | [B21 — the toolchain was pinned by name, so each host resolved a different binary](2026-08-06-b21-cross-toolchain-binary-selection/index.md) | Defect | Verified | B21, B20, B19 |
| 2026-08-07 | [B16 — a supervision termination record was never reclaimed](2026-08-07-b16-supervision-records/index.md) | Defect | Verified | B16, B22, B23, P5.4 |
| 2026-08-07 | [P5.4 — decomposed into an equivalence inventory before deletion](2026-08-07-p5-4-decomposition/index.md) | Decision | Proposed | P5.4, P5.4.1, B12, B16, B22, B23 |
| 2026-08-07 | [P5.4.1 — the oracle equivalence inventory](2026-08-07-p5-4-1-oracle-inventory/index.md) | Audit | Verified | P5.4.1, P5.4, B22, B12, B23 |
| 2026-08-07 | [B23 — `slime-root`'s unit tests were run by no gate](2026-08-07-b23-slime-root-host-tests/index.md) | Defect | Verified | B23, P5.4.1, P5.4 |
| 2026-08-07 | [B24 — a shared-buffer quota was never released](2026-08-07-b24-shared-buffer-quotas/index.md) | Defect | Verified | B24, B22, B16, P5.4.1 |
| 2026-08-07 | [ROS 2 transport: self-built DDSI-RTPS/XCDR vs Zenoh](2026-08-07-ros2-transport-zenoh-vs-dds/index.md) | Decision | Proposed | R0, R1, R2, C9, X1, P5 |
| 2026-08-07 | [P5.4.4 — C8.2 aggregate fabric-graph admission on seL4](2026-08-07-p5-4-4-fabric-graph-admission/index.md) | Change | Verified | P5.4.4, P5.4, P5.4.1, C8.2 |
| 2026-08-07 | [P5.4.10 (part) — the component-image segment corpus, made portable](2026-08-07-p5-4-10-segment-corpus/index.md) | Change | Verified | P5.4.10, P5.4, P5.4.1, P0 |
| 2026-08-07 | [P5.4.10 (part) — per-pair QoS compatibility at admission](2026-08-07-p5-4-10-qos-pair-admission/index.md) | Change | Verified | P5.4.10, P5.4.4, P5.4, C8.2 |
| 2026-08-07 | [P5.4.10 (part) — B10's boot layout, frozen on seL4](2026-08-07-p5-4-10-sel4-boot-layout/index.md) | Change | Verified | P5.4.10, P5.4, P5.4.1, B10 |
| 2026-08-07 | [P5.4.10 (part) — two rows that need no seL4 gate](2026-08-07-p5-4-10-reclassified-rows/index.md) | Audit | Verified | P5.4.10, P5.4.1, C7.1, B11 |
| 2026-08-07 | [P5.4.10 (part) — C8.4's structural arm: the shape the generation fixed](2026-08-07-p5-4-10-graph-shape/index.md) | Change | Verified | P5.4.10, P5.4, P5.4.1, C8.4 |
| 2026-08-07 | [P5.4.10 (part) — B9's conservation, in the shape seL4 can hold it](2026-08-07-p5-4-10-slot-conservation/index.md) | Change | Verified | P5.4.10, P5.4, P5.4.1, B9 |
| 2026-08-07 | [P5.4.10 — C8.1's tag collision and C8.3's graph provenance](2026-08-07-p5-4-10-collision-and-provenance/index.md) | Change | Verified | P5.4.10, P5.4, P5.4.1, C8.1, C8.3 |
| 2026-08-07 | [P5.4.2 (part) — M5.4's superblock validator, made portable](2026-08-07-p5-4-2-store-superblock/index.md) | Change | Verified | P5.4.2, P5.4, P5.4.1, M5.4 |
| 2026-08-07 | [P5.4.3 (part) — the transfer manifest decoder had no tests](2026-08-07-p5-4-3-transfer-manifest/index.md) | Change | Verified | P5.4.3, P5.4, P5.4.1 |
| 2026-08-07 | [P5.4.2 (part) — the recovery index decoder had no tests](2026-08-07-p5-4-2-recovery-index/index.md) | Change | Verified | P5.4.2, P5.4, P5.4.1, M5.4 |
| 2026-08-07 | [The release record and trust root had no tests](2026-08-07-release-trust-root-tests/index.md) | Change | Verified | P5.4.1, P5.4 |
| 2026-08-07 | [B30 — `release_trust_check` was red, unregistered, and half-blind](2026-08-07-b30-release-trust-gate/index.md) | Defect | Verified | B30 |
| 2026-08-07 | [The hash primitives everything trusts had no conformance tests](2026-08-07-hash-primitive-conformance/index.md) | Change | Verified | P5.4.1, P5.4 |
| 2026-08-07 | [The seL4 gates had no negative control](2026-08-07-sel4-gate-negative-control/index.md) | Change | Verified | P5.4.1, P5.4 |
| 2026-08-07 | [B12 — the component build's `--remap-path-prefix` named a path that does not exist](2026-08-07-b12-component-remap/index.md) | Defect | Verified | B12 |
| 2026-08-07 | [B25 (part) — a second supervision handle for one task](2026-08-07-b25-supervision-derive/index.md) | Change | Verified | B25, P5.4.6, P5.4 |
| 2026-08-07 | [P5.4.2 (part) — GPT redundancy and recovery precedence, made portable](2026-08-07-p5-4-2-gpt-validation/index.md) | Change | Verified | P5.4.2, P5.4, P5.4.1, M5.4 |
| 2026-08-07 | [P5.4.2 (part) — the object store's crash consistency, made portable](2026-08-07-p5-4-2-object-store/index.md) | Change | Verified | P5.4.2, P5.4, P5.4.1, M5.4 |
| 2026-08-07 | [B28 — the QoS plane needed more root iterations, not a bug fix](2026-08-07-b28-iteration-budget/index.md) | Defect | Verified | B28, P5.4.5 |
| 2026-08-07 | [P5.4.5 (part) — C8.5's arms that already ran, now asserted](2026-08-07-p5-4-5-qos-arms/index.md) | Change | Verified | P5.4.5, P5.4, P5.4.1, C8.5 |
| 2026-08-07 | [P5.4.6 (part) — the C8.6 call plane builds and boots; the broker's slot model does not fit](2026-08-07-p5-4-6-call-plane/index.md) | Change | Root-caused | P5.4.6, P5.4, P5.4.1, C8.6 |
| 2026-08-07 | [P5.4.6 — the C8.6 call plane's real blocker is spawn-grant semantics, not slot numbering](2026-08-07-p5-4-6-call-spawn-semantics/index.md) | Defect | Root-caused | P5.4.6, B25, C8.6 |
| 2026-08-07 | [B26 — the boot-layout dump reported the grant's rights, hiding a too-permissive layout row](2026-08-07-b26-layout-declared-rights/index.md) | Defect | Verified | B26, B10, P5.4.6 |
| 2026-08-07 | [P5.4.5 (part) — a monotonic-time channel makes three C8.5 arms fire on seL4](2026-08-07-p5-4-5-qos-clock/index.md) | Change | Root-caused | P5.4.5, C8.5, B25 |
| 2026-08-08 | [B25 and P5.4.6 — endpoint copies close the seL4 native-call plane](2026-08-08-b25-endpoint-copy-call-plane/index.md) | Defect | Verified | B25, P5.4.6, P5.4, C8.6 |
| 2026-08-08 | [P5.4.7 — C8.7 bounded native operations on seL4](2026-08-08-p5-4-7-operation-plane/index.md) | Change | Verified | P5.4.7, P5.4, C8.7 |
| 2026-08-08 | [P5.4.8 — C8.8 filtered introspection and declared interposition on seL4](2026-08-08-p5-4-8-visibility-plane/index.md) | Change | Verified | P5.4.8, P5.4, C8.8 |
| 2026-08-08 | [P5.4.9 — C8.9 and C8.10 on seL4: the full C8 graph in one generation](2026-08-08-p5-4-9-full-graph-boot/index.md) | Change | Verified | P5.4.9, P5.4, C8.9, C8.10 |
| 2026-08-08 | [P5.4.2a — a device resource substrate for `slime-root`](2026-08-08-p5-4-2a-device-substrate/index.md) | Change | Verified | P5.4.2, P5.4, M5.1 |
| 2026-08-08 | [P5.4.2b — a virtio-blk transport for `slime-root`](2026-08-08-p5-4-2b-virtio-blk/index.md) | Change | Verified | P5.4.2, P5.4, M5.2, M5.3 |
| 2026-08-08 | [P5.4.2c (part) — a userspace component reaches the disk](2026-08-08-p5-4-2c-storage-plane/index.md) | Change | Verified | P5.4.2, P5.4, M5.2, M5.3 |
| 2026-08-08 | [P5.4.2c — M5.4's object store, moved to userspace](2026-08-08-p5-4-2c-object-store/index.md) | Change | Verified | P5.4.2, P5.4, M5.4 |
| 2026-08-08 | [P5.4.2c — M5.6's rollback contract, in userspace](2026-08-08-p5-4-2c-rollback-plane/index.md) | Change | Verified | P5.4.2, P5.4, M5.6 |
| 2026-08-08 | [P5.4.2c — M5.9's recovery reconstruction, in userspace](2026-08-08-p5-4-2c-recovery-plane/index.md) | Change | Verified | P5.4.2, P5.4, M5.9 |
| 2026-08-08 | [P5.4.3 — M6.5's generation commands, in userspace](2026-08-08-p5-4-3-generation-plane/index.md) | Change | Verified | P5.4.3, P5.4, M6.5 |
| 2026-08-08 | [P5.4.3 — M6.3's directory mechanism, in the root](2026-08-08-p5-4-3-directory-plane/index.md) | Change | Verified | P5.4.3, P5.4, M6.3 |
| 2026-08-08 | [P5.4.3 — M6.3's filesystem service, and the oracle's client unmodified](2026-08-08-p5-4-3-filesystem-plane/index.md) | Change | Verified | P5.4.3, P5.4, M6.3 |
| 2026-08-08 | [P5.4.3 — input mediation, and four defects a console session exposed](2026-08-08-p5-4-3-input-mediation/index.md) | Change | Verified | P5.4.3, P5.4, M6.4 |
| 2026-08-08 | [P5.4.3 — M6.6's powerbox, and a placement order that is an ABI](2026-08-08-p5-4-3-powerbox-plane/index.md) | Change | Verified | P5.4.3, P5.4, M6.6 |
| 2026-08-08 | [P5.4.3 — M6.4's Dango session, and slot layout as declared data](2026-08-08-p5-4-3-dango-plane/index.md) | Defect | Verified | P5.4.3, P5.4, M6.4 |
| 2026-08-08 | [P5.4.3 — M6.7's generation transfer, and two devices in one granule](2026-08-08-p5-4-3-transfer-plane/index.md) | Change | Verified | P5.4.3, P5.4, M6.7 |
| 2026-08-08 | [P5.4.final — auditing whether `kernel/` can be deleted](2026-08-08-p5-4-final-deletion-audit/index.md) | Audit | Verified | P5.4.final, P5.4 |
| 2026-08-09 | [P5.4.final — retire the custom kernel](2026-08-09-p5-4-final-kernel-retirement/index.md) | Change | Verified | P5, P5.4, P5.4.final, B31 |
| 2026-08-09 | [B32 — park scenario receivers on their endpoints](2026-08-09-b32-parked-scenario-receivers/index.md) | Defect | Verified | P5.4.6, P5.4.7, B32 |
| 2026-08-09 | [B33 — seL4 kernel cutover review remediation](2026-08-09-b33-cutover-review-remediation/index.md) | Audit | Verified | P5.4.final, B33 |
| 2026-08-09 | [B34–B38 — seL4 model-cutover audit](2026-08-09-b34-b38-sel4-model-audit/index.md) | Audit | Root-caused | P5.4.9, B34, B35, B36, B37, B38 |
| 2026-08-10 | [B34–B38 — seL4 model cutover and lifecycle closure](2026-08-10-b34-b38-model-cutover/index.md) | Change | Verified | P5.4.9, B34, B35, B36, B37, B38 |
| 2026-08-10 | [seL4 native-capability-model handoff](2026-08-10-sel4-native-model-handoff/index.md) | Decision | Proposed | P5, P5.4, P5.5, C8, B34, B35, B36, B37, B38 |
| 2026-08-10 | [B39 — generation v5 header cutover: boot action, host checkers, fabric provenance](2026-08-10-b39-generation-v5-checker-cutover/index.md) | Defect | Verified | B39 |
| 2026-08-10 | [B40 — child CSpaces sized and populated from the admitted plan](2026-08-10-b40-native-child-cspaces/index.md) | Change | Verified | B40 |
| 2026-08-10 | [B41 prerequisite — the dango plane's declarations](2026-08-10-b41-dango-plane-declarations/index.md) | Defect | Verified | B41 |
| 2026-08-10 | [Probe planes — the run token, the idle instance, and slot zero](2026-08-10-probe-plane-run-tokens/index.md) | Defect | Verified | B41 |
| 2026-08-10 | [B41 — a console endpoint per process](2026-08-10-b41-console-endpoint/index.md) | Change | Verified | B41 |
| 2026-08-10 | [B42 — the supervision handle becomes the lifecycle identity](2026-08-10-b42-lifecycle-identity/index.md) | Change | Verified | B42 |
| 2026-08-10 | [B43 — a component's second block device was silently its first](2026-08-10-b43-block-device-renumbering/index.md) | Defect | Verified | B43 |
| 2026-08-10 | [B43 — block requests answered where the devices live](2026-08-10-b43-block-service-endpoint/index.md) | Change | Verified | B43 |
| 2026-08-10 | [B44 — the generation and recovery labels were never reachable](2026-08-10-b44-policy-labels-deleted/index.md) | Change | Verified | B44 |
| 2026-08-10 | [B45 — directory inspect and commit move; derive cannot](2026-08-10-b45-directory-service-split/index.md) | Change | Verified | B45 |
| 2026-08-10 | [B46 — four defect classes between the fabric planes and their scenarios](2026-08-10-b46-fabric-plane-admission/index.md) | Defect | Root-caused | B46 |
| 2026-08-10 | [B46 — a declared grant and a minted one at the same slot](2026-08-10-b46-minted-control-channels/index.md) | Defect | Verified | B46 |
| 2026-08-10 | [B51 — a collected instance is not a new one](2026-08-10-b51-respawn-provenance/index.md) | Defect | Verified | B51 |
| 2026-08-10 | [Blessing the layouts found two controls that were not controlling](2026-08-10-boot-layout-and-gate-controls/index.md) | Defect | Verified | none |
| 2026-08-10 | [B48 — the schedule record was there all along](2026-08-10-b48-declared-priority/index.md) | Change | Verified | B48 |
| 2026-08-10 | [B47 — three assumptions kept the process/thread split notional](2026-08-10-b47-thread-plan/index.md) | Change | Verified | B47 |
| 2026-08-10 | [B52 — the loan plane loaned to peers that never launched](2026-08-10-b52-loan-plane-peers/index.md) | Defect | Verified | B52 |
| 2026-08-10 | [B41 — why the root cannot yet have a second dispatcher](2026-08-10-b41-second-dispatcher-blocker/index.md) | Audit | Verified | B41, B43, B44, B45 |
| 2026-08-10 | [B47 runtime threads: a process runs two of them](2026-08-10-b47-runtime-threads/index.md) | Change | Verified | B47 |
| 2026-08-10 | [B49: the stress graph found the ceiling admission was not checking](2026-08-10-b49-object-budget/index.md) | Defect | Verified | B49 |
| 2026-08-10 | [B48: a busy thread declared below its peer does not starve it](2026-08-10-b48-per-thread-priority/index.md) | Change | Verified | B48, B47 |
| 2026-08-12 | [B48 — defer AArch64 MCS until its proof is complete](2026-08-12-b48-mcs-assurance/index.md) | Decision | Proposed | B48 |
| 2026-08-12 | [B46 — native endpoint framing must fail closed](2026-08-12-b46-native-endpoint-framing/index.md) | Defect | Verified | B46 |
| 2026-08-12 | [B46's native IPC cutover, and the slot namespaces it exposed](2026-08-12-b46-native-ipc-cutover/index.md) | Change | Monitoring | B46, B50 |
| 2026-08-12 | [B46 — an arena returns a CSlot the kernel still finds occupied](2026-08-12-b46-arena-slot-occupancy/index.md) | Defect | Verified | B46, B50 |
| 2026-08-13 | [R2 — the builder assigns declared slots, and init reads its grant count](2026-08-13-r2-declared-slot-allocation/index.md) | Change | Verified | B50, B46 |
| 2026-08-13 | [The QoS plane's fixture cutover, and three dead counters behind it](2026-08-13-qos-plane-fixture-cutover/index.md) | Change | Monitoring | B46, B50 |
| 2026-08-13 | [The cutover's real defect class: code written against `ERR_WOULDBLOCK`](2026-08-13-b46-blocking-ipc-semantics/index.md) | Defect | Fixed | B46 |
| 2026-08-13 | [B50 — deleting `endpointCreate`, and what it did not unblock](2026-08-13-b50-endpoint-create-deletion/index.md) | Change | Verified | B50, B46 |
| 2026-08-13 | [B46 — the two mechanisms rendezvous IPC actually needs](2026-08-13-b46-multi-source-wait/index.md) | Change | Monitoring | B46 |
| 2026-08-13 | [B46 — all seven fabric planes run on native seL4 IPC](2026-08-13-b46-native-ipc-completion/index.md) | Change | Verified | B46 |
| 2026-08-14 | [B50 — a minted endpoint named an object nobody could create](2026-08-14-b50-minted-endpoint-deletion/index.md) | Change | Verified | B50, B46 |
| 2026-08-14 | [B53, B54 — a line one byte past the message bound, and a component that never ends](2026-08-14-b53-b54-last-two-planes/index.md) | Defect | Verified | B53, B54 |
| 2026-08-14 | [Zutai's field-pun shorthand, and why `schemaFields` cannot reach these contracts](2026-08-14-zutai-field-pun-adoption/index.md) | Change | Verified | none |
| 2026-08-15 | [C8.11 — a deterministic trace, and the five ways a silent record hides](2026-08-15-c8-11-semantic-trace/index.md) | Change | Verified | C8.11, B55 |
| 2026-08-15 | [B55 — the full-graph boot plane refused its own first spawn, then six more defects behind it](2026-08-15-b55-full-graph-boot-restoration/index.md) | Defect | Verified | C8.10, B55 |
| 2026-08-15 | [C8.12 — one graph, every mismatch, and the two mutual waits it took to serve it](2026-08-15-c8-12-matrix/index.md) | Change | Verified | C8.12 |
| 2026-08-15 | [C8.13 — concurrent cross-plane traffic, nine gaps C8.10's parked boot never exercised, and an honestly partial exit](2026-08-15-c8-13-traffic/index.md) | Change | Verified | C8.13 |
| 2026-08-16 | [C8.13 — two more resource classes, and why the other two real signals still can't ship](2026-08-16-c8-13-queue-history-evidence/index.md) | Change | Verified | C8.13 |
| 2026-08-16 | [C8.13 — a saturation fixture, and which declared ceilings a manifest field can actually prove](2026-08-16-c8-13-saturation-ceilings/index.md) | Change | Verified | C8.13 |
| 2026-08-16 | [C8.13 — the QoS-timed clock wiring the last pass reverted, done in one coordinated change](2026-08-16-c8-13-qos-timed-traffic/index.md) | Change | Verified | C8.13 |
| 2026-08-16 | [C8.13 — why `resourceEvent` and `resourceLoan` are structural walls, not scenario gaps](2026-08-16-c8-13-resource-event-loan-walls/index.md) | Audit | Root-caused | C8.13 |
| 2026-08-16 | [C8.13 — `historyDepth` was wrongly grouped as unconsumed; `queueDepth` and `capabilitySlots` genuinely are](2026-08-16-c8-13-declared-fields-audit/index.md) | Audit | Root-caused | C8.13 |
| 2026-08-16 | [C8.13.1 — a self-scoped occupancy query, and the counter that could not move](2026-08-16-c8-13-1-shared-buffer-occupancy/index.md) | Change | Verified | C8.13.1, C8.13 |
| 2026-08-16 | [C8.13.2 — four participants report their own occupancy; three measurably cannot](2026-08-16-c8-13-2-participant-occupancy/index.md) | Change | Verified | C8.13.2, C8.13 |
| 2026-08-16 | [Post-seL4 documentation reconciliation, and RP2 rescoped to what seL4 does not already supply](2026-08-16-post-sel4-doc-reconciliation/index.md) | Decision | Proposed | RP2, RP3, C8.2, C7.7, M5.1, M5.4, M5.6, P5 |
| 2026-08-17 | [C8.13.3 — the one declared ceiling with no signal, and the two slot spaces it turned out to have](2026-08-17-c8-13-3-capability-slot-occupancy/index.md) | Change | Verified | C8.13.3, C8.13 |
| 2026-08-17 | [C8.14 — the fault envelope was already being driven; nothing asserted it](2026-08-17-c8-14-fault-isolation/index.md) | Change | Verified | C8.14, C8.13 |
| 2026-08-17 | [C8.15 — the C8 parent close, and the C8.9 gate the audit found red](2026-08-17-c8-15-fabric-aggregate/index.md) | Change | Verified | C8.15, C8.9, C8 |
| 2026-08-17 | [A structural audit of the green tree: two defects, eight debts, three rejected claims](2026-08-17-structural-audit/index.md) | Audit | Root-caused | B57, B58, B59, B60, B61, B62, B63, B64, B65, B66, B40, B46, B55, B56 |
| 2026-08-17 | [B57, B58 — a rights mask that admitted a bit nobody named, and the gate that found a third defect](2026-08-17-b57-b58-rights-vocabulary/index.md) | Defect | Verified | B57, B58, B59, B67, B40 |
| 2026-08-17 | [B67 — two negative controls aimed at a slot the audit declares, and the second one hid behind the first](2026-08-17-b67-blind-negative-controls/index.md) | Defect | Verified | B67, B40, B57 |
| 2026-08-17 | [B59, B66 — one contract for the syscall ABI, and 97 rights declarations becoming one](2026-08-17-b59-b66-syscall-abi-contract/index.md) | Change | Verified | B59, B66, B57, B46 |
| 2026-08-17 | [B60 — two scoping mistakes on the way to asserting one slot number](2026-08-17-b60-control-plane-authority/index.md) | Change | Verified | B60, B55, B56 |
| 2026-08-17 | [B64 — the rollback answer was already in the code; four of the five "dead" schemas were live](2026-08-17-b64-format-coexistence/index.md) | Change | Verified | B64, B50 |
| 2026-08-17 | [B62 — the proposed fix was impossible, so the delta moved to the layer that already had one](2026-08-17-b62-fixture-deltas/index.md) | Change | Verified | B62, B55 |
| 2026-08-17 | [B61 — `just run` was booting a verification fixture, and one half of the fix needs a seam that does not exist](2026-08-17-b61-product-image-and-dispatch/index.md) | Change | Verified | B61, B23, B46 |
| 2026-08-17 | [B63 — 82 copies of three pure functions, and the flake that surfaced while verifying them](2026-08-17-b63-gate-helper-consolidation/index.md) | Change | Verified | B63, B55 |
| 2026-08-17 | [B65 — four plane launchers moved out of init.rs, and why the binary collapse should not happen yet](2026-08-17-b65-plane-modules/index.md) | Change | Verified | B65, B60 |
| 2026-08-17 | [B68 — the determinism gate was comparing one scheduling interleaving, and grouping by worker was not enough](2026-08-17-b68-aggregate-trace-determinism/index.md) | Defect | Verified | B68, C8.15, B55 |
| 2026-08-17 | [ROS 2 transport pivot: bounded Zenoh replaces self-built DDSI-RTPS](2026-08-17-ros2-transport-zenoh-pivot/index.md) | Decision | Verified | R0, R1, R2, RP0, RP1, RP5, RP6, B69 |
| 2026-08-17 | [Roadmap/devlog record boundary: collapse finished-work narrative out of `roadmap/`](2026-08-17-roadmap-record-boundary/index.md) | Decision | Verified | none |
| 2026-08-17 | [Component platform track: component/system specs as data, and out-of-tree components as the forcing proof](2026-08-17-component-platform-track/index.md) | Decision | Proposed | CP0, CP1, CP2, CP3, CP4, CP5, B70, RP4, B65 |
| 2026-08-18 | [CP0 — component specification model](2026-08-18-cp0-component-spec-model/index.md) | Change | Verified | CP0, B70 |
| 2026-08-18 | [CP1 — system specification model and generation derivation](2026-08-18-cp1-generation-derivation/index.md) | Change | Verified | CP1, B70 |
| 2026-08-18 | [CP2 — runtime-resolved component binding](2026-08-18-cp2-runtime-binding-query/index.md) | Change | Verified | CP2, B70 |
| 2026-08-18 | [CP2 — capability-role query axis](2026-08-18-cp2-capability-role-axis/index.md) | Change | Verified | CP2, B70 |
| 2026-08-18 | [B71 — boot-layout binary/Rust drift](2026-08-18-b71-boot-layout-binary-drift/index.md) | Defect | Verified | B70, B71 |
| 2026-08-20 | [B75: three planes asserted a peer-death property that only a race produced](2026-08-20-b75-stream-peer-death-race/index.md) | Defect | Monitoring | B75, B74, C8.5, C8.14, C8.15 |
| 2026-08-20 | [B74: one flaky gate was two defects, and the silent one hid behind a deliberate suppression](2026-08-20-b74-aggregate-flake/index.md) | Defect | Fixed | B74, B75, C8.15 |
| 2026-08-19 | [Retiring seventeen dead profile symbols, and the limits check that could not fail](2026-08-19-dead-profile-symbol-retirement/index.md) | Change | Verified | B70, CP2 |
| 2026-08-19 | [Retiring FABRIC_PARTICIPANTS, and the gate that outlived what it guarded](2026-08-19-fabric-participants-retirement/index.md) | Change | Verified | B70, CP2, B74 |
| 2026-08-19 | [Visibility plane reads its participant facts from the graph](2026-08-19-fabric-graph-visibility-join/index.md) | Change | Verified | B70, B72 |
| 2026-08-19 | [Supervision binding naming convention](2026-08-19-supervision-binding-naming/index.md) | Change | Verified | B70, CP2 |
| 2026-08-19 | [Scoping the fabric-graph read against measured consumer needs](2026-08-19-fabric-graph-read-scope/index.md) | Audit | Verified | B70, CP2 |
| 2026-08-19 | [Fabric-graph read: authority shape and what each option reaches](2026-08-19-fabric-graph-read-options/index.md) | Decision | Verified | B70, CP2 |
| 2026-08-19 | [CAPABILITY GRAPH READ: serving the fabric graph to its declared holder](2026-08-19-fabric-graph-read/index.md) | Change | Verified | B70, CP2 |
| 2026-08-19 | [Self-scoped graph rows, and the first consumers off the generated table](2026-08-19-fabric-graph-self-view/index.md) | Change | Verified | B70, CP2 |
| 2026-08-19 | [Matrix plane reads its visibility policy from the graph](2026-08-19-fabric-graph-matrix-visibility/index.md) | Change | Verified | B70, CP2, B73 |
| 2026-08-19 | [Interposition hop identity moves from broker constants to root admission](2026-08-19-interposition-hop-identity/index.md) | Defect | Verified | B70 |
| 2026-08-19 | [Init asks which handle a child declares, not how many](2026-08-19-owned-minted-shape-selection/index.md) | Change | Verified | B70, CP2 |
| 2026-08-19 | [B72: the visibility plane's QoS records are decoded and frozen](2026-08-19-b72-frozen-visibility-view/index.md) | Defect | Verified | B72 |
| 2026-08-20 | [B73: the matrix plane's graph-wide view is read, not just counted](2026-08-20-b73-matrix-graph-view/index.md) | Defect | Verified | B73 |
| 2026-08-20 | [B76: `IpcError::PeerDead` had no producer, the call clock's death was inferred from the wrong task, and removing the endpoint arm exposed a real parking deadlock](2026-08-20-b76-peer-death-cleanup/index.md) | Defect | Fixed | B76 |
| 2026-08-20 | [B75: what a determinism gate may compare — separating a trace's declared content from its observed sampling](2026-08-20-b75-observed-vs-declared-trace-fields/index.md) | Decision | Verified | B75, C8.15 |
| 2026-08-20 | [RP2: one generation carrying the data path and the component graph, and the two arms that were never observed](2026-08-20-rp2-demo-scoped-arm-slice/index.md) | Change | Verified | RP2 |
| 2026-08-21 | [B70's boot-action query: which composition am I booted into](2026-08-21-b70-boot-action-query/index.md) | Change | Verified | B70, CP2 |
| 2026-08-21 | [CP3: one crate per component, and the three Cargo behaviors that shaped it](2026-08-21-cp3-crate-per-component/index.md) | Change | Verified | CP3, B70, B65 |
| 2026-08-21 | [CP4: content-bound external artifacts enter an ordinary signed generation](2026-08-21-cp4-external-artifact-admission/index.md) | Change | Verified | CP4 |
| 2026-08-22 | [CP5: two out-of-tree data-path components boot through a pinned SDK](2026-08-22-cp5-out-of-tree-component-sdk/index.md) | Change | Verified | CP5 |
| 2026-08-22 | [B70 closes: the last nine `include!` sites, and the stack the ceilings overflowed](2026-08-22-b70-profile-include-closure/index.md) | Change | Verified | B70, CP2 |
| 2026-08-22 | [The open milestones still specified against the kernel P5 deleted](2026-08-22-roadmap-retired-kernel-audit/index.md) | Audit | Verified | C9, C10, C10.1, C10.2, C10.3, P3, P4, D2, D4, M1, M2, RP2, CP2, CP5, B70 |
| 2026-08-23 | [C10.1: a task-private growable region, and the two accounting inverses it needed](2026-08-23-c10-1-private-memory-mechanism/index.md) | Change | Verified | C10.1, C10, C10.2, C7.3, B9, B23 |
| 2026-08-23 | [C10.2: a second budget rather than a fifth column, and the gate that was mutating its own prose](2026-08-23-c10-2-private-memory-budget/index.md) | Change | Verified | C10.2, C10, C10.1, C10.3, C7.3, B5, B8, B55, B63, B68 |
| 2026-08-23 | [C10.3: a second allocator, and a reuse assertion that was quoting the code under test](2026-08-23-c10-3-userspace-allocator/index.md) | Change | Verified | C10.3, C10, C10.1, C10.2, C10.4, C7.3, CP3, B5, B23, B63 |
| 2026-08-24 | [C10.4: the first product component on the private region, and two demands a fixed array had been absorbing](2026-08-24-c10-4-adoption-and-leak-evidence/index.md) | Change | Verified | C10.4, C10, C10.1, C10.2, C10.3, C7.3, CP1, CP3, B9, B23, B63, B70 |
| 2026-08-24 | [Planning C9: every mechanism exists, none of it reaches a component — and two deliverables the platform cannot hold](2026-08-24-c9-decomposition/index.md) | Decision | Proposed | C9, C9.1, C9.2, C9.3, C9.4, C9.5, C9.6, C10, C10.4, C8.11, C8.15, RP5, A3, D3, D4, B46, B48 |
| 2026-08-24 | [Reassessing MCS: the cost is per-target, and the QEMU build already left the verified set](2026-08-24-mcs-cost-reassessed/index.md) | Decision | Proposed | C9, C9.3, B48, B77 |
| 2026-08-24 | [B77: two readers admitted a CPU budget neither of them could honour](2026-08-24-b77-undeclarable-cpu-budget/index.md) | Defect | Verified | B77, B48, C9, C9.3 |
| 2026-08-24 | [P4 Raspberry Pi 5 bring-up: a second seL4 platform, and the assurance it costs](2026-08-24-p4-rpi5-board-bringup/index.md) | Change | Verified | P4, RP3, P5, P5.1 |
| 2026-08-24 | [C9.1: a root-brokered clock service, and the register wall it cannot cross](2026-08-24-c9-1-clock-authority/index.md) | Change | Verified | C9.1, C9 |
| 2026-08-25 | [C9.2: one block, one badge word, and the map a waiter cannot compute](2026-08-25-c9-2-bounded-wait-sets/index.md) | Change | Verified | C9.2, C9, C9.1, C10.4, RP5, B23, B70, B76 |
| 2026-08-25 | [C9.3: a class is a priority, and the band that names it](2026-08-25-c9-3-declared-scheduling-class/index.md) | Change | Verified | C9.3, C9, C9.1, C9.2, C9.4, B48, B71, B77 |
| 2026-08-25 | [C9.4: the root charges the bound, and userspace decides the restart](2026-08-25-c9-4-supervised-restart/index.md) | Change | Verified | C9.4, C9, C9.1, C9.2, C9.3, B71, B76 |
| 2026-08-25 | [Planning CP6–CP10: one source tree, one generated SDK, and tested release pairs](2026-08-25-cp6-cp10-sdk-release-plan/index.md) | Decision | Proposed | CP6, CP7, CP8, CP9, CP10 |
| 2026-08-25 | [CP6–CP10: one exporter, one published mirror, and a consumer that can roll back](2026-08-25-cp6-cp10-component-sdk-releases/index.md) | Change | Verified | CP6, CP7, CP8, CP9, CP10, CP5 |
| 2026-08-26 | [C9.5: recorded means captured, and the one grant that carries the recording](2026-08-26-c9-5-typed-recording-replay/index.md) | Change | Verified | C9.5, C9, C9.1, C9.2, C9.4, C8.11, C8.15, B23, B57, B70, B71 |
