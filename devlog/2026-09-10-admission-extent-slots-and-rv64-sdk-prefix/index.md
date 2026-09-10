# Admission extent CSlots and the stale RV64 SDK prefix

| Field | Value |
|---|---|
| Date | 2026-09-10 |
| Kind | Defect |
| Status | Fixed |
| Scope | `slime-root` generation slot admission, private backing layout, the closure corpus SDK release record, and the system-image builder gate |
| Work items | none |
| Gates | `just test_sel4_root`, `just system_image_builder_check`, `just private_memory_check`, `just sel4_root_boot_check` |
| Trigger | PR #25 review against `2d30362` reported two P2 defects: admission omitted private extent parent slots, and the RV64 SDK profile advertised a pre-19-bit-CNode kernel |
| Baseline | Generation admission reserved private allocation descriptors exactly, and the AArch64 SDK profile named this commit's rebuilt prefix |

## Summary

Two independent accounting defects, both of the same shape: a value that must
track another was maintained by hand at one site and not the other.
`admit_total_slots` counted a holder's allocation descriptors but not the root
CSlot each of its extents retains for its parent untyped, leaving a
two-or-three-slot-per-holder margin in which a graph admits and then dies in
task staging with `SlotsExhausted`, children already running. Separately, the
closure corpus's SDK release record advertised the RV64 kernel and configuration
hashes from before this branch raised the QEMU root CNode to 19 bits, while its
AArch64 sibling in the same file had been updated. Both are fixed; the SDK
record is now generated from the tree rather than hand-edited, and the
generator's `--check` mode gates it.

## Observable symptom

- Command: PR #25 review of `2d30362`; locally `python3 scripts/build/refresh-closure-sdk-release.py --check`.
- Expected: admission reserves every root CSlot task staging will consume, and each advertised SDK profile names the prefix `sel4/pins.toml` pins.
- Observed: admission's private term was `allocation_descriptors` only; `sdk-release.json`'s `riscv64-sel4-qemu-virt` profile declared kernel `ef24f1…` / config `8d0ef4…` against pinned `79e627…` / `e306b1…`.
- Exit/fault/serial evidence: the refresh checker reports `is stale (prefix drift: riscv64-sel4-qemu-virt)`; the installed prefix at `build/sel4-riscv64-prefix` hashes to the pinned values, confirming the record and not the prefix was wrong.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `provision_extent` calls `take_slot()` for each new extent's retained parent untyped, and `PrivateBackingLayout::for_quota` yields two extents per nonzero quota, three at 512 pages | Admission's per-holder total was short by exactly the extent count |
| 2 | `plan_task_backing` already added `extent_descriptors` into `required_cslots`; only the generation-side admission path omitted it | The defect was one missing term at one call site, not a wrong layout |
| 3 | Re-running `component_sdk.export` for both QEMU profiles reproduced the committed record byte-for-byte except the four RV64 prefix fields, `prefixSet`, and `treeIdentity` | The record is machine-derivable; the committed one had been hand-edited for AArch64 only |
| 4 | `generate-system-image-closures.py` returns `None` for any profile not matching `aarch64-sel4*`, so no closure references the RV64 profile | Nothing in the verification chain ever read the stale entry, which is why it survived |

## Root cause

Both sites duplicated a value whose authority lives elsewhere.
`admit_total_slots` restated the private per-holder CSlot cost as
`allocation_descriptors`, a formula that was correct only while extents were
free, rather than asking the layout what it costs. `sdk-release.json` was
maintained as a hand-edited artifact although `component_sdk.export` derives
every field in it, and the one consumer that validates prefix identity
(`component_sdk_system.export_asset`) only checks the single profile the
exported closure names — AArch64 — so the RV64 half had no reader at all.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Backing layout | Added `PrivateBackingLayout::extent_parent_slots`, counting the extents a quota provisions | The per-holder CSlot cost is answerable from the layout instead of restated by callers |
| Generation admission | `admit_total_slots` adds descriptors *and* extent parent slots per holder | A graph that admits can be staged; the omitted margin cannot reappear silently |
| SDK release record | Added `scripts/build/refresh-closure-sdk-release.py`, which exports both QEMU profiles into the closure input and refuses a stale record under `--check` | The record is derived from the tree, and every advertised profile names this commit's pinned prefix |
| Builder gate | `just system_image_builder_check` runs the refresh checker before resolving closures | A profile whose prefix lags the pins fails a gate rather than reaching a consumer |
| Closure corpus | Regenerated all 52 closures for the moved SDK release input identity | Closure identities describe the inputs they actually resolve |
| Regression coverage | Added `quota_root_slot_cost_counts_every_extent_parent`; host count is 228 | The admission defect fails deterministically if reintroduced |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Admission stops counting a class of root CSlot task staging consumes | `just test_sel4_root` | `quota_root_slot_cost_counts_every_extent_parent` fails: measured pool consumption exceeds the layout's reported cost |
| An advertised SDK profile's prefix drifts from `sel4/pins.toml` | `just system_image_builder_check` | the refresh checker names the drifted profile and refuses |
| The SDK release record is hand-edited back into a partially updated state | `just system_image_builder_check` | the record no longer matches its export and is reported stale |
| Private growth or root lifecycle regresses under the corrected admission | `just private_memory_check`, `just sel4_root_boot_check` | marker contract or ordered boot markers fail |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test_sel4_root` | 228 passed, 0 failed across 19 modules | Direct |
| Mutation: `extent_parent_slots` forced to `0` | `quota_root_slot_cost_counts_every_extent_parent` fails; restored source passes | Direct |
| `python3 scripts/build/refresh-closure-sdk-release.py --check` (before fix) | Refused, naming `riscv64-sel4-qemu-virt` prefix drift | Direct |
| `python3 scripts/check/check-sel4-pins.py --prefix --platform qemu-riscv-virt` | Exact source, toolchain, target, config, and host pins verified | Direct |
| `just system_image_builder_check` | Record current; 52 closures current and resolving with distinct identities; 1 closure built twice byte-identically | Direct |
| `just private_memory_check` | RV64: 23 markers across 7 causal chains and 1 image case, booted on the rebuilt prefix | Direct |
| `just sel4_root_boot_check` | Ordered generation, timer, task, IPC, fault, and ready markers observed | Direct |
| `just fmt_check_all` | Passed | Direct |
| `just lint_all` | Passed with warnings denied | Direct |
| `ruff check scripts/build/refresh-closure-sdk-release.py` | Passed | Direct |

## Decisions

- Decision: count extent parent slots through a layout method rather than adding a second constant at the admission site.
- Rationale: the layout already decides how many extents a quota provisions; any other site restating it is the defect recurring under a new name.
- Rejected alternative: fold the margin into `ROOT_SLOTS_PER_DECLARED_OBJECT`. That factor is a measured construction cost for declared objects and hiding an exact, separately known reservation inside it makes both unauditable.
- Decision: generate the closure corpus's SDK release record, stubbing only the `systems` row.
- Rationale: a real system row cannot exist in this file — exporting one compiles a closure that declares this very file as an input — but every other field is derivable, and hand maintenance is what produced a record with one fresh and one stale profile.
- Rejected alternative: drop the RV64 profile from the record. It is a profile this repository builds, pins, and boots; removing it to avoid maintaining it would discard working evidence.
- Decision: gate the record in `system_image_builder_check` rather than at publication.
- Rationale: publication runs for releases, while the closures resolve against this file on every contract run; the stale entry survived precisely because no routine reader existed.

## Open risks and follow-ups

- [ ] `plan_task_backing`'s above-512-page branch reserves one data extent per 2 MiB span and no large-frame fallback, so a hypothetical 64/256 MiB plan does not describe a failure-then-small-growth sequence. Reported as P2 in the same review; deliberately not changed here because the runtime ceiling is 512 pages, and widening the planning-only formula would move frozen QEMU capacity markers for a path no runtime can execute.
- [ ] `sdk-release.json` still records version `3.1.0` and source commit `c42ae22f`, the published release it tracks. The build inputs are current; the release identity moves at the next publication.

## Artifacts and provenance

- Focused report: none.
- Raw transcript: none.
- Serial/debugger/model output: command output from this session, quoted in Verification.
- Related work item: none.
