# A single-profile SDK export could not publish, and five more comment-ownership fixes

| Field | Value |
|---|---|
| Date | 2026-09-12 |
| Kind | Defect |
| Status | Verified |
| Scope | `scripts/lib/component_sdk.py` corpus publication, `scripts/lib/component_sdk_system.py` corpus profile requirement, `scripts/check/check-component-sdk-system-image.py` control, plus comment ownership in `slime-root/src/object_allocator.rs`, `components/testkit/private-memory-probe/src/main.rs`, `scripts/generate/generate-system-test-runs.py`, `sel4/config/qemu-arm-virt.cmake`, `sel4/config/qemu-riscv-virt.cmake`, `scripts/check/check-sel4-reclamation-plane.py` |
| Work items | 01a07a2d-003d-77fc-9df8-dda85ed9a083 |
| Gates | `just component_sdk_system_image_check`, `just system_image_closure_check`, `just system_test_run_check`, `just sel4_reclamation_check`, `just private_memory_check`, `just test_sel4_root`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just devlog_check` |
| Trigger | PR #25 review of `1c5eff7a` reported that every supported single-profile SDK export except AArch64 QEMU fails, plus five implementation comments carrying milestone, review, or campaign narrative |
| Baseline | Branch head `1c5eff7a`, where `component_sdk.export` unconditionally stages the `sel4-channel` corpus and `export_asset` requires the exported profile set to contain that corpus's AArch64 QEMU target |

## Summary

`build-component-sdk.py --profile` accepts any `PROFILE_PLATFORMS` entry as a
standalone repeatable selection, but `component_sdk.export` staged the
`sel4-channel` system corpus on every export regardless of which profiles were
selected. That corpus's closure targets `aarch64-sel4-qemu-virt`, and
`export_asset` requires exactly one matching row in `profile_records` with a
matching prefix tree hash, so any single-profile export naming `aarch64-rpi5`,
either RV64 profile, or the H1V1 profile aborted with "canonicalized prefix does
not match the selected exported profile". Four of the five supported profiles
could not be published alone. The corpus is now staged only when the release
exports the profile the corpus's own closure names, read from that closure
rather than restated. Five comments were separately trimmed to their invariants.

## Observable symptom

- Command: `component_sdk.export(dest, profiles=("riscv64-sel4-qemu-virt",), …)`, the library call behind `build-component-sdk.py --profile riscv64-sel4-qemu-virt`
- Expected: an SDK release carrying the RV64 profile
- Observed: `ComponentSdkError: canonicalized prefix does not match the selected exported profile`
- Exit/fault/serial evidence: raised from `component_sdk_system.export_asset`; reproduced directly against the branch head before the fix and passing after, with `systems: []` and `verify_tree`/`verify_digests` both clean

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `export` builds `system_records` as an unconditional one-element list | The corpus is a fixed part of every release, not a profile-dependent one |
| 2 | `contracts/system-image-closure/v2/closures/sel4-channel.zti` declares `target.profile = "aarch64-sel4-qemu-virt"` | The corpus is inherently single-target |
| 3 | `export_asset` requires `len(selected) == 1` against that profile and an equal prefix tree hash | Any export omitting the AArch64 QEMU profile is refused, correctly, at the wrong layer |
| 4 | `ComponentSdkRelease.systems` is `List SystemImageAsset` with `maxSystems = 4`, and `systems` is a keyed structural compatibility axis | The record already expresses "a release carries a different number of corpora"; an empty list is a legal shape |
| 5 | Every consumer — `verify_digests`, the compatibility axis walk, and the SDK's own `sdk-system-image.py` — iterates or looks up by name, and the entry tool already fails with "this release declares no systems" | No consumer requires a corpus to exist; the tool's own message anticipated this case |

## Root cause

Publication of the corpus was unconditional while the corpus itself is bound to
one target profile. The authority for which profile that is already exists in
the closure the corpus publishes, so the export decided one thing (always
publish) while the record it produced asserted another (this corpus resolves
against the selected profile's prefix).

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Corpus requirement | `component_sdk_system.required_profile(source)` reads the target profile from the corpus closure | The corpus's target is read from the artifact that declares it, never restated |
| Corpus publication | `export` stages the corpus only when that profile is among the exported profiles | A release publishes a corpus exactly when it exports the profile that corpus's closure resolves against |
| Provenance refusal | `export_asset`'s selected-profile and prefix-hash checks are unchanged | A staged corpus still proves its prefix provenance against the outer release record |
| Gate control | `check-component-sdk-system-image.py` derives the required profile and requires the AArch64 QEMU export to publish exactly the corpus | Retargeting the corpus moves the gate's expectation with it rather than silently passing |
| Comments | `MAX_PLANNED_PRIVATE_PAGES`, the probe's worker loop, the run generator's closure-reachable table, both QEMU CNode configs, and the reclamation descriptor assertion state only their invariants | Implementation comments state what must remain true |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The corpus is published by a release that cannot resolve its prefix | `just component_sdk_system_image_check` | `canonicalized prefix does not match the selected exported profile` |
| A release exporting the corpus profile silently stops publishing it | same gate | `a release exporting the corpus profile did not publish the corpus` |
| The corpus is retargeted without moving the gate | same gate | `the published corpus requires '<profile>', but this gate exports '<profile>'` |
| Descriptor leaks at task release | `just sel4_reclamation_check` | `N allocation descriptor(s) survived reclamation of every task` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `riscv64-sel4-qemu-virt` alone | Exports; `systems: []`; `verify_tree` and `verify_digests` pass. Raised `ComponentSdkError` before the fix | Direct |
| `aarch64-sel4-qemu-virt` alone | Exports; `systems: ['sel4-channel']`; both verifications pass | Direct |
| `("riscv64-sel4-qemu-virt", "aarch64-sel4-qemu-virt")` | Exports; corpus published once | Direct |
| `python3 scripts/check/check-component-sdk-system-image.py` | Two immutable releases built the corpus closure with no `slime_os` checkout, the current release booted its declared QEMU test run, rollback reproduced every artifact identity, both provenance refusals held, and the new corpus-profile control passed | Direct |
| `just component_sdk_export_check` | SDK 1.0.0 exported twice byte-identically; an SDK-built external ELF booted the QEMU component graph | Direct |
| `just system_image_closure_check` | Contracts, resolution, identity, isolation, and bytes verified; closure `90b6eb5a7591` built | Direct |
| `just system_test_run_check` | 48 records, 46 resolving a closure | Direct |
| `just sel4_reclamation_check` | Segmented task extents and root CSlots reclaimed and reused | Direct |
| `just private_memory_check` | AArch64 and RV64: 23 markers across 7 causal chains each | Direct |
| `just test_sel4_root` | 235/235 across 19 modules | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff` | Passed | Direct |

## Decisions

- Decision: publish no corpus for a release that does not export the corpus's profile, rather than making the corpus conditional on a matching closure or declaring AArch64 QEMU mandatory.
- Rationale: `systems` is already a bounded list and a keyed structural compatibility axis, and every consumer looks up by name. An empty list is the shape the record was built to express. Declaring the QEMU profile mandatory would remove a published capability — four profiles can be built and pinned standalone — to avoid a case the record already handles.
- Rejected alternative: selecting a closure matching whichever profile is exported. Only one non-AArch64 system spec exists in the corpus (`reference.zti`, `x86_64-qemu-virtio`), and `generate-system-image-closures.py` emits closures only for `aarch64-sel4*` targets, so there is no closure to select. Inventing per-profile corpora would be new mechanism for a case no consumer asks for.

- Decision: read the required profile from the corpus closure rather than naming it as a constant.
- Rationale: the closure is the artifact that declares the target, and it is already compiled by the export path. A constant beside it would be the same fact stated twice, which is the failure mode the earlier `LARGE_DESCRIPTOR_TABLES` allowlist had.
- Rejected alternative: a module-level `REQUIRED_PROFILE = "aarch64-sel4-qemu-virt"`. Cheaper, and wrong for the same reason the review rejected the per-platform CNode allowlist.

## Open risks and follow-ups

- [ ] `just component_sdk_system_image_check` transitively runs `component_sdk_prefix_check`, which fails in this working tree because the installed `build/sel4-rpi5-prefix` predates the current `sel4/pins.toml` (`cd840a2d…` against the pinned `f421bc27…`). Reproduced at branch head with these changes stashed, so it is environmental prefix staleness and not a consequence of this change; the owning checker was run directly instead. Clearing it needs `just sel4_rpi5_image_check`.

## Artifacts and provenance

- Focused report: none; each change is local to its named file.
- Raw transcript: none retained; every gate above is reproducible from the listed command.
- Serial/debugger/model output: `just sel4_reclamation_check`, `just private_memory_check` (AArch64 and RV64), and the SDK system-image gate's QEMU test-run boot.
- Related work item: [MEM-ARENAS](../../.tasks/items/01a07a2d-003d-77fc-9df8-dda85ed9a083.md)
- Preceding investigation: [Comment ownership across four files, and closure decoding through the shared evaluator](../2026-09-12-comment-ownership-and-closure-evaluator-cache/index.md)
