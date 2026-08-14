# B34–B38 — seL4 model cutover and lifecycle closure

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Change |
| Status | Verified |
| Scope | Generation v4 executable/instance contracts, declared launch and capability layout, graph health gating, BootState boot selection, and reclaimable task allocation |
| Roadmap | P5.4.9, B34, B35, B36, B37, B38 |
| Gates | `just generation_check`, `just sel4_boot_selection_check`, `just sel4_reclamation_check`, `just sel4_gate_control_check`, `just test_sel4_root` |
| Trigger | The B34–B38 audit proved duplicate graphs, an early non-unique terminal, compile-time generation selection, implicit non-bootstrap layout, and monotonic task allocation |
| Baseline | The seL4 cutover ran components, but loadable executables were also root instances, BootState did not control the next boot, and repeated task lifetimes could exhaust root resources |

## Summary

The generation format now separates executable catalogue entries from declared instances. Instance ownership, autostart, dependencies, health, quotas, and explicit capability bindings drive both root launch and authorized spawn, so executable-only images are inert and grant order is no longer a slot ABI. The supervisor emits one required-instance health terminal and the gates collect through that record or a failure. The product selector reads durable BootState and a signed boot bundle from the granted block device across fresh QEMU boots. Task construction uses reclaimable arenas and reusable root CSlots, with a stress plane crossing the former lifetime watermark. B34–B38 are verified and closed.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Generation contract | Cut the binary format to v4 `Executable`, `Instance`, dependency, and explicit binding records; removed the inert generation kernel object. | Catalogue membership does not imply a running task, and every initial instance has authenticated ownership and layout. |
| Launch and spawn | Root stages only root-owned autostart instances; declared child instances are resolved by owner plus executable and receive capabilities at explicit child-local slots. Deferred channel ends are pre-created once and installed when their declared instance launches. | Each declared instance is constructed once by its owner; boot and spawn share one layout contract. |
| Composition validation | Builder and decoder validate owner/dependency acyclicity, required-health closure, binding completeness, and stable bootstrap/component projections. | Table and grant ordering carry no hidden ABI meaning. |
| Graph gate | Added a unique supervisor `SLIME_GRAPH HEALTHY` record covering required live/completed/failed counts; migrated the graph checker and gate-control mutation corpus. | A green gate observes the system outcome after all required causal chains, not an arbitrary component-idle line. |
| Boot selection | Added a disk-backed selector over GPT, BootState, release, boot-bundle identity, and generation closure; preserved the active release store during generation management. Added DMA completion ordering before status consumption. | A durable selection controls the next product boot, failed attempts survive reboot, and promotion follows health confirmation. |
| Reclamation | Added reusable root CSlots, per-task reclaimable untyped arenas, cleanup integration for success/fault/unwind, and lifetime accounting. | Repeated bounded spawn/exit consumes bounded live resources rather than monotonic boot-lifetime watermarks. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Executable/instance conflation or layout drift | `just generation_check` | Contract, deterministic build, layout projection, or native graph boot fails. |
| Early or forged graph success | `just sel4_gate_control_check` | Missing, reordered, duplicated-idle, or failed-instance mutation is accepted. |
| BootState not controlling a fresh boot | `just sel4_boot_selection_check` | Pending attempts do not persist, rollback selects the wrong release, or promotion occurs without health. |
| Task lifetime exhaustion or capability aliasing | `just sel4_reclamation_check` | Forced unwind, clean cycles, fault reuse, arena/slot accounting, or post-watermark spawn fails. |
| Root mechanism regression | `just test_sel4_root` | Host model/unit contract fails. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just generation_check` | Passed: Zutai model/contracts, 176 boot-contract tests, generated layouts, native seL4 component graph, and two isolated byte-identical generation builds. | Direct |
| `just sel4_boot_selection_check` | Passed: fresh QEMU processes consumed pending attempts durably, exhausted to known-good, promoted only after health, and changed only BootState sectors. | Direct |
| `just sel4_reclamation_check` | Passed: construction unwind plus 80 clean spawn/exit cycles, fault cleanup, successful reuse, and bounded live resources. | Direct |
| `just sel4_gate_control_check` | Passed: 27 gate definitions and 1102 missing/reordered/failure mutations rejected. | Direct |
| `just test_sel4_root` | Passed: 128/128 tests across 13 modules. | Direct |
| `just test_host` | Passed: boot-contract and protocol host suites. | Direct |
| `just fmt_check_all` | Passed. | Direct |
| `just lint_all` | Passed with warnings denied. | Direct |
| `just ruff` | Passed. | Direct |

## Decisions

- Decision: use a clean v4 generation cutover with no v1 runtime compatibility shim.
- Rationale: retaining two launch models would preserve the ambiguity that caused B34 and make explicit instance layout non-authoritative.
- Rejected alternative: infer child slots from spawn request or grant order; this recreates the hidden ABI B37 removed.
- Decision: keep the boot selector minimal and immutable while the selected generation remains dynamic disk data.
- Rationale: BootState must select the runtime without making the selector itself mutable policy.
- Rejected alternative: compile a different root ELF per selected generation; that cannot prove durable next-boot selection.

## Open risks and follow-ups

- None for B34–B38. Physical Framework boot and internal-NVMe safety remain outside these backlog exit conditions and are still governed by their roadmap evidence requirements.

## Artifacts and provenance

- Focused report: this entry is the closure report for the audit in [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../2026-08-09-b34-b38-sel4-model-audit/index.md).
- Raw transcript: none retained; exact gate results are summarized above.
- Serial/debugger/model output: QEMU serial was observed directly through the generation, boot-selection, reclamation, and gate-control targets.
- Related roadmap item: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md) B34–B38 and [`roadmap/07-architecture-portability.md#p549-and-c810`](../../roadmap/07-architecture-portability.md#p549-and-c810).
