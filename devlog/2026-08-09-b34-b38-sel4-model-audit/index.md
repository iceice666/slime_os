# B34–B38 — seL4 model-cutover audit

| Field | Value |
|---|---|
| Date | 2026-08-09 |
| Kind | Audit |
| Status | Root-caused |
| Scope | seL4 generation admission and launch, boot selection, full-graph gate termination, capability layout, dependency activation, and task resource reclamation |
| Roadmap | P5.4.9, B34, B35, B36, B37, B38 |
| Gates | `just sel4_boot_check`, `just sel4_generation_check`, `just test_sel4_root` |
| Trigger | A post-cutover review asked whether unnatural seL4 mechanisms came from Slime's retained capability, component, generation, IPC, and task models |
| Baseline | B33 recorded the seL4 cutover gates as green, with the custom kernel retired and `slime-root` owning the surviving runtime mechanism |

## Summary

The audit found five model-level defects or latent resource failures. The current generation format conflates executable catalogue entries with initial component instances, so `slime-root` launches every loadable component before `init` spawns a second copy of the same graph. The full-graph gate stops at the first generic fabric-idle line and therefore truncates before the unique system-level outcome. Separately, the product image still embeds its generation at root-task compile time instead of selecting it from BootState, non-bootstrap slot and dependency contracts are implicit rather than executed as declared data, and task reclamation does not make monotonic untyped memory or root CSlots reusable. B34–B38 now track the required clean cutovers; no runtime fix is claimed by this entry.

## Observable symptom

- Command: `just sel4_boot_check`
- Expected: the current full-graph image reaches init's supervision transfer, launches one instance graph, provisions every role, and reaches one healthy blocked-idle terminal state with no failed component.
- Observed: exit 1 after the checker stopped at the first `[fabric] idle: parked on control endpoints`, reporting missing marker `\[init\] fabric boot supervision transferred`.
- Exit/fault/serial evidence: `python3 scripts/check/check-sel4-boot-plane.py --no-build` reproduced the same failure. Continuing the same image manually showed root-launched `fabric-service` task 16 fail `spawn` as ungranted, followed by nonzero exits across that first graph; init task 19 then transferred supervision and continued provisioning a second graph.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `launch_component_graph` iterates every `Admission::loadable_plans()` entry, constructs it, and later activates every constructed task. | A loadable component record is treated as an initial instance whether or not it exists only so another component can spawn it. |
| 2 | The full-graph `init` independently spawns the subscribers, fabric, remaining participants, and route workers from executable capabilities. | The image contains two live copies of the graph; the root-launched copy lacks the spawn-time composition and fails. |
| 3 | `check-sel4-boot-plane.py` treats any fabric-idle line as terminal and terminates QEMU immediately. | A non-unique component marker can hide later failure or success and cannot prove system health. |
| 4 | `slime-root/build.rs` and `main.rs` select generation bytes through `include_bytes!`, while the generation plane only changes BootState sectors on an attached disk. | The durable transition service is not connected to the next product boot's generation selection. |
| 5 | Dependency records are structurally decoded but not consulted by launch; non-bootstrap slots follow grant iteration order; task cleanup deletes capabilities without returning allocator slots or untyped capacity. | Three legacy-model conventions remain runtime assumptions rather than declared, reusable seL4 mechanisms. |

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Backlog | Opened B34 for executable/instance separation and duplicate graph construction. | Tracks the requirement that one declared initial instance produces one task. |
| Boot selection | Opened B35 for a BootState-driven seL4 boot selector and removal of the inert generation `kernelObject`. | Tracks the requirement that a committed durable selection controls the next boot. |
| Gate integrity | Opened B36 for a unique supervisor terminal marker and failure-complete transcript collection. | Tracks the requirement that a green full-graph gate observes one system outcome, not an arbitrary component line. |
| Declared composition | Opened B37 for executed dependency DAGs and explicit per-instance capability bindings. | Tracks the requirement that launch and slot ABI follow authenticated data rather than table order. |
| Resource lifecycle | Opened B38 for reusable root CSlots and reclaimable per-task untyped arenas. | Tracks the requirement that repeated spawn/exit consumes bounded live resources rather than a boot-lifetime watermark. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test_sel4_root` | Passed: 124/124 tests across 13 modules. This establishes the focused table contracts but does not cover the duplicate full-graph composition. | Direct |
| `just sel4_boot_check` | Failed reproducibly with the missing init supervision-transfer marker after the checker accepted the first generic fabric-idle line. | Direct |
| `python3 scripts/check/check-sel4-boot-plane.py --no-build` | Failed at the same marker against the already-built image, ruling out a one-off rebuild difference. | Direct |
| Manual boot of `build/slime-sel4-boot.elf` beyond the checker's early terminal | Observed the root-launched fabric fail its ungranted worker spawn and the first graph exit nonzero before init's second graph completed supervision transfer. | Direct |
| `just sel4_generation_check` | Passed: an unprivileged client drove durable generation operations through the manager and could not reach the block device directly. This does not establish next-boot selection. | Direct |

## Open risks and follow-ups

- [ ] B36 must restore a trustworthy full-graph gate first; changing only the terminal regex cannot close B34.
- [ ] B34 requires a generation-format cutover separating executable records from initial instance records and defining launch ownership.
- [ ] B35 requires a rebooting QEMU scenario where BootState selection changes the generation that the seL4 product actually launches.
- [ ] B37 requires builder rejection for cyclic or inconsistent instance graphs and fixture-checked layouts for every initial instance.
- [ ] B38 requires a lifetime test crossing the present monotonic allocation ceiling while live resource usage stays bounded.

## Artifacts and provenance

- Focused report: this entry is the focused audit record.
- Raw transcript: none retained; the decisive current-tree serial observations and exact commands are recorded above.
- Serial/debugger/model output: QEMU serial was observed directly from `just sel4_boot_check`, its `--no-build` reproduction, and a manually continued boot of the same packaged image.
- Related roadmap item: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md) B34–B38 and [`roadmap/07-architecture-portability.md#p549-and-c810`](../../roadmap/07-architecture-portability.md#p549-and-c810).
