# A structural audit of the green tree: two defects, eight debts, three rejected claims

| Field | Value |
|---|---|
| Date | 2026-08-17 |
| Kind | Audit |
| Status | Root-caused |
| Scope | `slime-root/src/{main,ipc,graph,generation,directory,console}.rs`, `components/runtime/src/syscall.rs`, `components/bins/{Cargo.toml,src/bin/init.rs}`, `boot-contracts/src/generation.rs`, `scripts/build/build-generation.py`, `scripts/check/` (31 plane gates), `contracts/generation/`, `roadmap/00-backlog.md` |
| Roadmap | B57, B58, B59, B60, B61, B62, B63, B64, B65, B66, B40, B46, B55, B56 |
| Gates | none |
| Trigger | B56 closed the backlog and C8.15 closed the C8 track at `35a95b2`, leaving no open item and no red gate; asked what structural defects remain |
| Baseline | Every gate green at `35a95b2`; backlog empty since B56 |

## Summary

With the backlog empty and every gate green, the question was not "what is
broken" but "what will cost us later." Seven read-only scouts partitioned the
tree by ownership; every load-bearing measurement they returned was then
re-derived directly, which mattered — **three of their claims were wrong and are
rejected here**, and two findings turned out to be real defects with wrong
observable semantics rather than mere debt. Ten items opened as B57–B66. The
sharpest single finding is B57: `RIGHT_ALL` has two definitions that differ by
exactly one bit, and admission uses the wider one, so a grant carrying an
undefined rights bit passes every check in both the root and the oracle. Nothing
was changed in code; this entry records the evidence and the opened items.

## Observable symptom

- Command: `just` gate suite at `35a95b2`, plus `roadmap/00-backlog.md`
- Expected: nothing to report — backlog empty, C8 closed, all gates green
- Observed: no failing gate, but ten structural findings, two of which are
  defects that no existing gate can detect
- Exit/fault/serial evidence: none — this audit ran no guest code. Every claim
  below is a source measurement, not a boot observation.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Backlog `## Open` was `(none)`; `roadmap/README.md` recorded B1–B56 resolved and the C8 track closed | Reframed the question from open defects to architectural debt |
| 2 | Seven scouts dispatched by ownership area (root binary, buffer mechanism, components, generation builder, gate scripts, syscall ABI, contracts) | Parallel coverage without serial file-by-file reading |
| 3 | Re-derived `RIGHT_ALL` both ways: python union `0x3fdffff`, rust bit-width `0x3ffffff` | Bit 17 differs; admission uses the wider spelling → B57, a real defect |
| 4 | Counted `const RIGHT_` declarations mechanically: 23 names across **97** sites, spanning root, boot-contracts, and ~14 userspace components | Scout said "four files"; the true blast radius is far larger → B59 |
| 5 | Compared the two `mod *_labels` tables and the two `ERR_*` tables value by value | Both agree *today*; the defect is the absent enforcement, not a live mismatch → B59 |
| 6 | Checked the three hardcoded offsets in `check-architecture-contract.py` against generated `boot_contracts.py` | All three (112/200/368) have generated names, and the file already imports them → B58 |
| 7 | Traced `just run` → `sel4_qemu_image_check` → `build-sel4.py:1336` → `SLIME_ROOT_FIXTURE=1` → `build.rs:28-30` → `main.rs:751` `#[cfg(not(slime_root_fixture))]` | The default image excludes the product dispatch path entirely → B61 |
| 8 | Measured the legacy fixture dispatch stack function by function | 458 lines of second, parallel recv/decode/dispatch loop → B61 |
| 9 | Pairwise line similarity over all 435 `sel4-*.zti` fixture pairs | Nine pairs >85%; `diff sel4-traffic sel4-fault` is **one line** in 1882 → B62 |
| 10 | Counted plane-gate harness reuse | 30 gates launch QEMU; `harness.run_qemu` used by **0**; `match_marker_contract` by **2** → B63 |
| 11 | Read `boot-contracts/src/generation.rs:586` and the `contracts/generation/` version dirs | Equality version gate, v1–v5 present, only v5 wired; rollback across a bump is unaddressed → B64 |
| 12 | Grepped every reference to `component/v1`, `kernel-image/v1`, `generation/v{2,3,4}` | Only two prose mentions (`check-generation-v5.py:6`, `components/component.ld:1`) → five dead schema trees, folded into B64 |
| 13 | **Measured** pairwise code-line similarity of the four `*_broker.rs` modules | 19.2% / 15.7% on two pairs, 0.8–1.6% cross-pair — scout's "one skeleton four times" **rejected** |
| 14 | Read how `SpawnGrant` actually crosses the boundary (`sel4_transport.rs:687-696`) | Encoded field by field; `#[repr(C)]` is not the wire layout — scout's schema-violation claim **rejected**; the duplicated `16` constant is the real coupling, folded into B59 |
| 15 | Counted marker literal sites: 242 across 7 files, 225 in `main.rs` alone | Concentrated, not scattered across modules as reported — scout's framing **rejected**; not opened as an item |
| 16 | Checked whether the builder can actually emit bit 17 (`build-generation.py:3146`, `:432-470`) | It cannot: unknown right names fail, and per-kind masks narrow further — B57 is an admission/builder asymmetry, not a live exploit |

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `roadmap/00-backlog.md` | Opened B57–B66 with problem, evidence, proposed fix, and exit condition each; added a dated preamble naming the audit and the rejected claims | The backlog is again the authoritative list of what must clear before a new track gate |
| `roadmap/README.md` | Backlog row now reads `B1–B56 resolved; B57–B66 opened`, names B57 and B59 as the next gates; track-map node updated | The index no longer claims an empty backlog |
| Code | **None.** This entry is an audit; no source was modified | — |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `RIGHT_ALL` recomputed from both source spellings | `0x3fdffff` vs `0x3ffffff`, differing bit 17 | Direct |
| `const RIGHT_` census over root, boot-contracts, components | 23 names, 97 sites, all values agree except `RIGHT_ALL` | Direct |
| Label-table comparison, `main.rs:71-122` vs `syscall.rs:24-62` | 22 shared names, zero disagreements; `fixture_labels::DIRECTIVE` root-only | Direct |
| `112`/`200`/`368` matched against generated `GENERATION_HEADER_*_OFFSET` | All three resolve; zero literals lack a generated name | Direct |
| `diff sel4-traffic.zti sel4-fault.zti` | One hunk: `generation = 36` → `40` | Direct |
| Fixture similarity matrix, 435 pairs | 9 pairs >85%; three at 99.9% | Direct |
| Plane-gate harness census | 31 gates, 15192 lines; `run_qemu` 0 users; `boot()` defined 30× in 23 distinct bodies | Direct |
| Legacy fixture dispatch measured per function | 458 lines across 11 functions, `main.rs:5337-5864` | Direct |
| Broker pairwise code-line similarity | 19.2%, 15.7%, and four pairs at 0.8–1.6% | Direct |
| Marker literal census | 242 sites in 7 files; 225 in `main.rs` | Direct |
| Referenced Justfile targets checked to exist | 20 checked; `sel4_recovery_check` did not exist and was corrected to `sel4_recovery_plane_check` | Direct |
| Any gate run | **Not run.** No code changed, so no runtime claim is made | — |

## Open risks and follow-ups

- [ ] B57 — one `RIGHT_ALL`, computed as an enumerated union; new B40 mutation setting bit 17, refused and observed.
- [ ] B58 — replace three literal header offsets with the generated names already imported.
- [ ] B59 — `contracts/syscall-abi/v1/`: labels, errors, rights, spawn-grant record; delete 97 rights sites and the second label table. Subsumes B57.
- [ ] B60 — build-time assertion that pinned control slots match derived profile order (preventive half), then move control-plane/supervision derivation into the schema.
- [ ] B61 — product dispatch into `lib.rs` for host tests; `just run` must boot a product graph, not the two-fixture proof.
- [ ] B62 — base-plus-delta composition at the `.zti` level; collapse the traffic/fault/saturation trio.
- [ ] B63 — plane gates onto `harness.run_qemu` and `match_marker_contract`; marker expectations into blessable fixtures; derive `GATES` counts.
- [ ] B64 — decide and gate generation-format coexistence vs. explicit non-rollback-safety; delete five dead schema trees.
- [ ] B65 — collapse the call/operation fixture binary families; move `drive_*_plane` out of `init.rs`.
- [ ] B66 — delete `CHANNEL_CAPACITY`; give the wait-source ceiling one home.
- [ ] **[INFERENCE]** B57 is judged unexploitable by any current `.zti` fixture because the builder rejects unnamed rights and masks per capability kind. That reasoning is a source reading, not an observed refusal — the B40 mutation named in B57's exit condition is what would make it observed.
- [ ] The three rejected scout claims are recorded in the investigation log above rather than opened as items. If a fifth fabric protocol lands, revisit step 13: the `call`/`operation` pair at 19.2% and the `matrix`/`visibility` pair at 15.7% may become worth factoring pairwise, but not as one four-way abstraction.
- [ ] No item here is a QEMU-observable claim, so none carries a gate in front matter. Each opened item names its own exit gates in the backlog.

## Artifacts and provenance

- Focused report: none; the findings are recorded inline above and, per item, in `roadmap/00-backlog.md` under B57–B66.
- Raw transcript: none preserved; every measurement in this entry is reproducible from the cited file and line.
- Serial/debugger/model output: none — this audit ran no guest code.
- Related roadmap item: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md) B57–B66; prior art for the recurring mechanisms is B40 (mutation coverage), B46 (native IPC cutover residue), [B55](../2026-08-15-b55-full-graph-boot-restoration/index.md) (stale fixture vs. derivation rule), and [B56](../2026-08-17-c8-15-fabric-aggregate/index.md) (a gate asserting a contradiction).
