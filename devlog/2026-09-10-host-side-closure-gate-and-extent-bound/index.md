# A host-side closure gate and the extent bound's unasserted contract coupling

| Field | Value |
|---|---|
| Date | 2026-09-10 |
| Kind | Defect |
| Status | Fixed |
| Scope | Host-only closure prefix validation, SDK corpus export provenance, the removed synthetic SDK release fixture and refresh script, `slime-root/src/object_allocator.rs`'s extent bound, and the 52 regenerated system-image closures with their 48 re-blessed test-run records |
| Work items | none |
| Gates | `just system_image_builder_check`, `just system_image_closure_check`, `just component_sdk_system_image_check`, `just test_sel4_root`, `just private_memory_check`, `just contracts_check`, `just system_test_run_check` |
| Trigger | PR #25 review against `b3b83fcb` reported two P1 and two P2 findings |
| Baseline | `b3b83fcb`'s kernel-derived table sizing; initial fixes in `484aaa80`, followed by the source-provenance cutover within the same unmerged review round |

## Summary

Two findings against `b3b83fcb` are fixed here. The first is a real gate
regression introduced by `52570122`'s own fix: `refresh-closure-sdk-release.py`
calls `render()` before consulting `--check`, and `render()` packages both QEMU
prefixes, so `system_image_builder_check` — previously host-side — acquired two
complete seL4 platform builds as a precondition for comparing a checked-in
record. The second is narrower than reported: the small-kernel extent table is
*not* undersized at today's ceilings, because the reported distribution assumes
48 quota-bearing holders while `contracts/private-memory-budget/v1` admits 32,
but the bound's sufficiency rested entirely on that contract constant with
nothing asserting the relationship, and this bound is not checked during
admission. The initial fix split check and refresh modes and asserted the
extent bound. The subsequent source-provenance investigation removes the
release-shaped closure input altogether: closures bind their own build inputs,
while the outer SDK release alone records publication provenance. Prefix
validation remains host-only and now hashes the five committed prefix files
against the pins rather than comparing two metadata declarations. Three
findings are addressed; the worker-sentinel finding remains declined.

## Observable symptom

- Command: `just system_image_builder_check` in a checkout without installed seL4 prefixes; separately, PR #25's review of `b3b83fcb` on the extent bound.
- Expected: a gate that compares checked-in records against checked-in inputs runs host-side; a table bound whose sufficiency depends on another contract's constant fails loudly when that constant moves.
- Observed: the gate aborts in `render()` before any comparison — in a clean worktree with no submodules, `FileNotFoundError` on `deps/rust-sel4/support/targets/aarch64-sel4-minimal.json`, and with submodules present, `component_sdk.py:1287`'s refusal naming the missing `build/sel4-prefix`; the extent bound had no assertion tying `MAX_TASK_EXTENTS` to `MAX_HOLDERS`, and exceeding it fails partway through `provision_extent` with `ArenaTableFull` rather than at admission.
- Exit/fault/serial evidence: both refusals reproduced in a detached `git worktree` at this commit; the extent-bound mutation reproduced through `cargo check` against the rpi5 prefix. Quoted under Verification.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `refresh-closure-sdk-release.py:125` calls `render()` unconditionally, above the `--check` branch at `:128` | The check mode did strictly more work than deciding staleness required |
| 2 | `render()` → `component_sdk.export` → `export_prefix_asset`, which fails at `component_sdk.py:1287` unless `build/sel4-prefix/bin/kernel.elf` and `build/sel4-riscv64-prefix/bin/kernel.elf` exist | The dependency is on installed *build outputs*, not on anything a checkout carries |
| 3 | Nothing else in the gate needs one: closures declare the checked-in `contracts/system-image-closure/v1/inputs/sel4-prefix` (`generate-system-image-closures.py:48`), `build-system-image.py:210` takes the prefix from `resolved.artifacts["prefix"]`, and `check-system-image-builder.py` references `build/sel4*` nowhere | The gate was host-side before `52570122` and regressed to build-dependent |
| 4 | The five prefix fields the record carries are copied verbatim out of `sel4/pins.toml` by `export_prefix_asset`; only `archiveHash` and `treeHash` are packaging outputs | Staleness of the pinned identities is decidable from the record and the pin table alone |
| 5 | `boot-contracts/src/generated/private_memory_budget.rs:7` caps holders at 32, enforced in `decode` (`holder_count > MAX_HOLDERS` → `BadBounds`), and `launched.rs:72` refuses a second live task per instance | The reported 48-holder distribution is not admissible; the arithmetic does not hold today |
| 6 | Exhausting the real worst case — 3 × 512-page + 29 × 1-page holders = 1565 pages ≤ 2048, giving `3*3 + 29*2 = 67` private extents, `+48` static `= 115 ≤ 144` | The small branch covers every admissible budget with 29 records spare |
| 7 | Raising `MAX_HOLDERS` to 48 makes the required count 147 > 144, and `provision_extent` (`object_allocator.rs:1686`) is where that surfaces — after `admit_total_slots` has passed | The reviewer's structural point is real even though the instance is not: the coupling was load-bearing and unasserted |

## Root cause

Two independent causes. The gate regression is a mode conflation: one function
produced the record and the caller decided afterwards whether to write it, so
the expensive path ran for both modes even though only the rewriting mode needs
an installed prefix. `52570122` correctly moved this file from hand maintenance
to generation and correctly gated it where the closures resolve, but wiring the
gate to the generator instead of to the generator's *inputs* imported the
generator's build requirements into a host-side check.

The extent bound's cause is an unstated cross-contract dependency.
`MAX_TASK_EXTENTS` is sufficient only because `contracts/private-memory-budget/v1`
caps holders at 32; that constant lives in another contract, admission does not
check this bound, and no assertion connected the two. The bound was correct and
undefended — a state that reads as correct-by-accident and fails as a
mid-provisioning error rather than a refusal.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Closure gate | `check_committed()` compares the committed record's advertised profile set and each profile's five pinned prefix identities against `sel4/pins.toml` via the new `pinned_prefix_facts()`; `render()` now runs only on the rewriting path | Deciding whether a checked-in record is current needs only checked-in inputs; the closure gates stay host-side |
| Gate documentation | `system_image_builder_check`'s comment records that `--check` compares against the pin table while refreshing repackages the prefixes | The asymmetry between the two modes is stated where the gate is declared |
| Extent bound | `max_admissible_private_extents()` derives the widest private-extent population from `MAX_HOLDERS` and `MAX_TOTAL_PAGES`, and a `const _` assertion requires `MAX_TASK_EXTENTS` to cover it plus one static extent per task | A budget this root admits cannot exhaust the extent table partway through provisioning |
| Generated inputs | 52 closures regenerated and 48 test-run records re-blessed for the moved `just` and `slime-root` input identities | Every closure identity matches the tree it names |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A closure gate acquires a build-output dependency again | `just system_image_builder_check` from a checkout without installed prefixes | the gate aborts before its first comparison |
| The committed SDK release record's pinned prefixes drift from `sel4/pins.toml` | `just system_image_builder_check` | the check names the profile, field, observed value, and pinned value |
| A profile is silently added to or dropped from the record | `just system_image_builder_check` | the advertised profile set is reported against the exported set |
| `MAX_HOLDERS` or `MAX_TOTAL_PAGES` rises past what the extent table can hold | any compile of `slime-root` for the affected platform | `error[E0080]` naming the extent table and the budget contract |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Clean detached `git worktree` at this commit, no `build/`, no submodules — pre-fix script | `FileNotFoundError` on `aarch64-sel4-minimal.json`, rc=1: the reported failure, before any comparison | Direct |
| Same worktree, post-fix script | passes: names both pinned prefixes | Direct |
| Mutation in that worktree: RV64 `kernelHash` reverted to `ef24f1…` (this thread's originating defect) | refused, naming profile, field, and pinned value | Direct |
| Mutation: RV64 profile dropped from `profiles` | refused, reporting advertised set against exported set | Direct |
| Mutation: AArch64 `kernelConfigHash` zeroed | refused | Direct |
| Mutation: contract `MAX_HOLDERS` set to 48, `cargo check -p slime-root` against `build/sel4-rpi5-prefix` (12-bit) | `error[E0080]: evaluation panicked: extent table cannot hold one static extent per task plus the widest private-extent population contracts/private-memory-budget/v1 admits`; restored source compiles clean | Direct |
| `just system_image_builder_check` | 52 closures resolve with distinct identities matching their system specs; 1 closure built twice byte-identically | Direct |
| `just system_test_run_check` | 48 records correspond one-to-one with plane gates, 46 resolving a closure; 5 named controls refused | Direct |
| `python3 scripts/check/check-system-image-aggregate.py` | all 52 closures exercised by an owning gate; 6 named drift controls refused | Direct |
| `python3 scripts/check/check-system-spec.py` | 43 systems derived semantically identical to their fixtures; 21 named mutations refused | Direct |
| `just contracts_check` | passed, including 310 host tests | Direct |
| `just test_sel4_root` | 229/229 across 19 modules | Direct |
| `just private_memory_check` | AArch64: 23 markers across 7 causal chains and 3 image cases. RV64: 23 markers across 7 causal chains and 1 image case | Direct |
| `just fmt_check_all`, `just lint_all`, `ruff check scripts/build/refresh-closure-sdk-release.py` | passed | Direct |

The following evidence covers the subsequent source-provenance cutover, not a
re-run of the allocator changes above:

| Command/scenario | Result | Evidence class |
|---|---|---|
| Pre-cutover scratch mutations through `check_committed()` | Invented source SHA, version `999.0.0`, and empty `systems` all accepted; the check established pin agreement, not release provenance | Direct |
| Post-cutover owning closure checker functions `check_refusals`, `check_prefix_pins`, `check_target_spec_path` | Ten malformed closures refused; changing each of five prefix files and re-identifying the tree still refused; a same-name target spec copied outside its canonical path refused | Direct |
| Owning closure checker functions `check_identity_boundaries`, `check_bounds` | Executable identity changes, marker-oracle isolation, and closure/test-run bounds passed | Direct |
| `python3 scripts/check/check-component-sdk-system-image.py` | Two real SDK exports built the declared closure; current and rollback images booted through the declared QEMU run; rollback reproduced every build-result artifact identity; missing selected profile and mismatched exported prefix refused | Direct |
| `just system_test_run_check` | 48 records, 46 closure associations, five negative controls passed | Direct |
| `just system_image_closure_aggregate_check` | All 52 closures covered by an owning gate; six drift controls refused | Direct |
| `just contracts_check` | Passed, including 310 host tests | Direct |
| `just ruff`, `just fmt_check_all` | Passed; no Rust implementation changed in this cutover | Direct |
| Full-corpus resolver invocation through eval | Interrupted at the 600-second eval deadline; not accepted as verification evidence | Direct |
| `just generation_check` | Passed: two isolated generation builds produced byte-identical generation and boot-store artifacts; four resealed CPU-budget mutations refused | Direct |
| Legacy `target.sdkRelease` field presented to the new resolver | Refused by exact target-field validation; no compatibility shim remains | Direct |
| Fresh read-only review of the settled closure/resolver/generator/SDK exporter cutover | No findings; independently checked retained target/prefix authority, selected-profile refusal, lazy import direction, and complete removal of the old field | Direct |
| `just system_image_builder_check` | Passed: 52 current closures resolve with distinct identities and matching system manifests; the selected closure built twice byte-identically through the canonical builder | Direct |

## Decisions

- Decision: split the script by mode rather than making the check tolerate a missing prefix.
- Rationale: the two modes genuinely need different inputs. Rewriting repackages prefixes and must keep `export_prefix_asset`'s verification, which is what refuses a prefix not rebuilt for the current pins; deciding staleness of pinned identities needs only the record and the pin table. A check that skipped verification when the prefix happened to be absent would pass most loudly exactly where it is least informed.
- Rejected alternative: compare `archiveHash`/`treeHash` under `--check` too. Those are packaging outputs of an installed prefix, so requiring one to answer whether a checked-in record is current is the dependency being removed.
- Decision: assert the extent bound against contract-derived worst case rather than raise `MAX_TASK_EXTENTS`.
- Rationale: the bound is sufficient for every admissible budget with 29 records spare, so raising it would spend `.bss` — which is root CSpace in this image, per the previous entry — against a distribution no generation can present. The defect was the missing assertion, not the number.
- Rejected alternative: check the extent bound during admission. That would make admission depend on a table whose occupancy is a runtime property, where a compile-time assertion over fixed ceilings answers the same question before an image ships.
- Decision: remove `TargetSelection.sdkRelease`, its synthetic JSON fixture, and the refresh script rather than restamp a current export with another commit or local version.
- Rationale: the closure already binds profile, platform, toolchain, rust-sel4 commit, prefix tree, and target-spec artifact. Publication inventory is not needed to build the image. An enclosing release cannot be embedded in its own system archive without a digest cycle; the all-zero system placeholder was bypassing that cycle, not representing an asset.
- The generator and resolver reuse the existing profile mapping and source pin table. Resolution additionally checks the actual five prefix artifacts and the canonical target-spec path; content-addressed prefix and target-spec references remain load-bearing.
- `component_sdk.export` passes its computed profile records into `component_sdk_system.export_asset`. The corpus exporter compares its canonicalized prefix against that actual selected exported profile before emitting the system asset. The outer release then binds the resulting archive, closure, and test-run identities without an embedded release record.
- No new local-release format, optional provenance shim, or publication version is introduced. The current closure contract is cut over in place and all generated consumers are migrated together.

## Open risks and follow-ups

- [x] The `sourceCommit` finding is closed by removing the release-shaped local input, not by claiming a new published SDK. Investigation found canonical SDK tags only through `sdk-v3.0.0`; the old `3.1.0/c42ae22` fixture was a local candidate export, not evidence of hosted publication. Earlier descriptions of that fixture as a published release were inaccurate. No SDK was published during this change.
- [ ] `plan_task_backing`'s `>512`-page branch still omits the per-span fallback extent and large-frame descriptors, unchanged across three rounds; the ceiling-raising milestone owns it with the frozen capacity markers.
- [ ] The probe worker RPC's sentinel payloads remain uncontracted. Refused as a finding: they carry no fields and are compared for equality only, and the standing testkit convention sends such sentinels as literals (`crossing-peer/src/main.rs:27,34,39`, `echo-agent/src/main.rs:33`). Making every testkit sentinel a contract would be a repository-wide convention change, not a single probe's fix.

## Artifacts and provenance

- Focused report: none.
- Raw transcript: none.
- Serial/debugger/model output: command output from this session, quoted in Verification.
- Related work item: none.
- Provenance references: [canonical SDK tags](https://github.com/iceice666/slime_os-component_sdk/tags), `contracts/component-sdk-release/v1/schema.zt`, and the reverse-drift reconstruction in `scripts/check/check-component-sdk-release.py`.
- The cutover SDK check creates local immutable test repositories, not hosted releases. Its build/boot evidence does not claim a new public version or a reverse-drift run from a committed cutover revision.
