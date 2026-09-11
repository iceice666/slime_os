# MEM-ARENAS: recycle failed large-frame backing within the admitted quota

| Field | Value |
|---|---|
| Date | 2026-09-11 |
| Kind | Defect |
| Status | Verified |
| Scope | Root private backing, kernel revoke seam, capacity qualification, and generated closure identities |
| Work items | 01a07a2d-003d-77fc-9df8-dda85ed9a083 |
| Gates | `just test_sel4_root`, `just private_memory_check`, `just sel4_reclamation_check`, `just sel4_root_boot_check`, `just sel4_gate_control_check`, `just sel4_duo_image_check` |
| Trigger | Reopened four-holder sizing result reported ordinary_layout=0 and fit=0 |
| Baseline | Existing 512-page runtime and reclamation evidence remained valid; four-holder 2 GiB sizing was invalid |

## Summary

A failed large-frame mapping retained a typed 2 MiB frame, so backing plans reserved a second data extent for a later base-page retry. Four 256 MiB holders therefore reserved more than 2 GiB before static task costs. The allocator now revokes only the dedicated extent of a reusable, unmapped large frame before retyping base pages. The same admission-time RAM reservation supports both shapes. The AArch64 qualification now reports `ordinary_layout=1 fit=1` inside the unchanged 2 GiB QEMU profile.

## Observable symptom

- Inherited reopened evidence: `ordinary_layout=0 fit=0` in the [prior correction](../2026-09-08-mem-arenas-segmented-backing/index.md#corrections).
- Source arithmetic: four private reservations totaled 2,149,580,800 bytes, already 2 MiB above 2 GiB.
- Expected: four 256 MiB holder plans plus the staged graph fit the platform's remaining physical regions and actual descriptor/CSlot capacities.
- Direct corrected evidence: `required_reserved=1077936128 ordinary_available=1577654544 ordinary_layout=1 fit=1`.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Both runtime layout and wide planner retained duplicate data backing for failed large maps | Allocation ordering alone could not recover the required capacity |
| 2 | An unmapped reusable large frame occupies its entire data extent | Revoking that extent cannot invalidate committed base pages or another object |
| 3 | Restricted qualification compiled artificially small descriptor tables | Use the actual platform table sizing for the positive capacity claim; retain exhaustion controls and the real Duo build |
| 4 | A direct replay initially used the intermediate loader ELF and timed out | Replay the packaged `image.elf`, matching the owning checker's input; no runtime conclusion drawn from the wrong artifact |
| 5 | Capability-layout baseline passed, but all six negative cases still named the old base closure identity after regeneration | Extend the existing closure generator to refresh their typed base references through the canonical closure compiler |
| 6 | Fresh review found the large-map probe retried another large mapping rather than exercising native conversion | Retry with one page followed by 511 pages and require ordered same-task/base frame-shape evidence; five semantic mutations guard that evidence |

## Root cause

Retaining a typed large frame is necessary until its capability is destroyed, but retaining a second full physical reservation is not. The former implementation conflated those requirements. Per-region alignment checks correctly exposed the impossible four-holder plan rather than accepting aggregate ordinary bytes alone.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Private backing | One data extent plus one leaf-table extent per span | Both bulk and one-page growth use reserved quota backing without doubling RAM |
| Retry conversion | Revoke only a reusable, unmapped large frame's exclusively owned data extent | Failed revocation leaves ownership and retry state intact; committed pages remain untouched |
| Accounting | Reset extent watermark and live object/byte counts only after successful revoke | Exactly-once accounting while retaining the task's extent anchor and preallocated CSlots |
| Qualification | Use real platform descriptor tables and require positive fit | Four-holder capacity is demonstrated against staged graph resource use, not a synthetic capacity total |
| Controls | Exercise allocation, extent, CSlot, ordinary RAM, layout, and small-table rejection | An undersized resource cannot be hidden by a claimed fit bit |
| Identity | Regenerate closure and test-run records; refresh six negative build cases from the compiled current base closure | Build input identities and mutation base references remain exact |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Failed revoke loses backing or double-counts its release | `just test_sel4_root` | Two failed revokes followed by 512 one-page growths must retain the parent and end with exactly 513 live objects |
| Failed large map cannot retry using base pages | `just private_memory_check` | AArch64 retry, zero-fill, survival, and quota evidence missing |
| Positive sizing exceeds actual capacity | `just private_memory_check`, `just sel4_gate_control_check` | Any required resource exceeds availability, impossible region layout, or contradictory fit |
| GiB metadata leaks into small targets | `just sel4_duo_image_check` | Actual 12-bit-kernel root image fails to build/package |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test_sel4_root` | 233/233 host tests passed across 19 modules | Direct |
| `just private_memory_check` | AArch64: three image cases; RV64: normal image; 512-page ceiling reached, AArch64 large-map fallback and incremental rollback passed | Direct |
| Packaged AArch64 large-map image replay | Four 256 MiB plans fit actual remaining regions; `ordinary_layout=1 fit=1`; failed large-map probe reached 512 pages with one retry | Direct |
| `just sel4_gate_control_check` | Marker and semantic mutation controls passed | Direct |
| `just sel4_duo_image_check` | Actual Duo root, child, kernel, and loader built and packaged | Direct; not a physical boot |
| `just ruff` | Python checks passed | Direct |
| `just sel4_reclamation_check` | Segmented task extents and root CSlots reclaimed and reused | Direct |
| `just sel4_root_boot_check` | Root boot path passed | Direct |
| `just fmt_check_all`, `just lint_all` | Formatting and warnings-denied clippy stack passed | Direct |
| `just contracts_check`, `just generation_check` | Contracts and deterministic generation checks passed | Direct |
| `just system_test_run_check`, `just system_image_closure_aggregate_check` | Frozen runs and closure ownership checks passed | Direct |
| `just sel4_capability_layout_check` | Baseline and all six negative mutations passed after refreshing base identities | Direct |
| `just sel4_boot_layout_check` | All 31 plane layouts matched frozen fixtures | Direct |
| `just sel4_pin_check`, `just system_image_builder_check` | Platform pins and generated closure/builder checks passed | Direct |
| `just devlog_check`, `just tasks_check` | 308 indexed entries; 289 work items and 94 frozen backlog headings validated | Direct |

The final campaign ran all commands above in one successful sequence. An earlier
campaign exhausted its shell deadline during boot-layout image builds; rerunning
without that external deadline completed the entire stack.

The live sizing report separates 1,073,741,824 payload bytes, 2,097,152 page-table bytes, zero internal data alignment waste, and 2,097,152 bytes of static clone backing. It requires 263,520 allocation descriptors, 1,028 extent descriptors, and 264,548 CSlots against 264,268, 1,060, and 521,240 available respectively. Root image is 7,901,184 bytes; root allocator/task metadata is 4,623,832 bytes; configured stack is 1,048,576 bytes and heap is 524,288 bytes. Per-region placement additionally accounts for allocation alignment beyond the reported internal data waste.

## Decisions

- Reuse the admitted extent rather than reserve duplicate data or raise QEMU RAM.
- Preserve reusable base frames and mapped leaf tables across failed transactions; the revoke path applies only to whole-extent large frames.
- Keep public private-memory ceilings, shared-buffer ownership, component ELF limits, and physical kernel CNode sizes unchanged.

## Open risks and follow-ups

- The four-holder result is sizing against live boot resources, not four live 256 MiB allocations. Larger public windows and simultaneous live holders remain separate milestones.
- The existing special failure images are AArch64-only; RV64 evidence here covers the normal public-quota path.
- Duo compilation and packaging do not claim physical runtime qualification.

## Artifacts and provenance

- [Canonical work item](../../.tasks/items/01a07a2d-003d-77fc-9df8-dda85ed9a083.md)
- [Memory capacity architecture](../../roadmap/02-core-runtime.md#memory-capacity)
- [Prior invalidated capacity evidence](../2026-09-08-mem-arenas-segmented-backing/index.md#corrections)
- Raw command output was captured in the implementation session. Existing landed raw evidence was not modified.
