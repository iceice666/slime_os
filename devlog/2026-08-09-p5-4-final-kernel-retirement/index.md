# P5.4.final — retire the custom kernel

| Field | Value |
|---|---|
| Date | 2026-08-09 |
| Kind | Change |
| Status | Verified |
| Scope | `kernel/`, `slime-root/`, `components/`, `boot-contracts/`, `scripts/`, `Justfile`, workspace and CI orchestration |
| Roadmap | P5, P5.4, P5.4.final, B31 |
| Gates | `just sel4_root_boot_check`, `just sel4_component_graph_check`, `just sel4_gate_control_check`, `just test_host`, `just test_sel4_root`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos`, `just devlog_check` |
| Trigger | B31 was the last blocker to deleting the frozen custom-kernel oracle |
| Baseline | `kernel/` remained a workspace member and roughly two dozen legacy checks still built or booted it |

## Summary

P5 is closed. The custom kernel, its legacy component transport, and its build and check orchestration are removed. The surviving product is the upstream-seL4 image with `slime-root` owning dynamic mechanism and userspace components owning policy.

The deletion audit's six findings were not collapsed into a claim of identical mechanisms. Portable contracts moved to host-testable crates, product behavior is observed on seL4, seL4-supplied internals were explicitly reclassified, and physical NVMe qualification remains open and fails closed rather than inheriting the retired QEMU path's evidence.

## Changes

- Removed `kernel/` from the workspace and deleted the directory.
- Removed custom-kernel Justfile recipes, check scripts, harness artifact selection, CI targets, component architecture trap wrappers, and the legacy syscall transport.
- Made generation construction seL4-manifest-only; no custom-kernel ELF is required.
- Kept historical gate names only where they resolve to a real seL4 or host successor. Framework/NVMe targets exit non-zero until their product paths exist.
- Completed component-wrapper admission in `boot_contracts::component_image`, including ABI, reserved-field, stack, ELF-size, segment, entry, W^X, and footprint checks.
- Added product-side foundation and failure evidence: independent frame allocation with exact accounting, clean-exit and deliberate-fault isolation in one boot, exact task and shared-buffer reclamation, and panic/abort/kernel-fault failure markers.
- Converted the three terminal receive spins tracked by the second B31 to endpoint waits on their passing call and operation planes.
- Updated repository guidance, roadmap state, foundations/hardware caveats, and CI around the seL4 product.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| seL4 root boot loses foundation, fault-isolation, or reclamation evidence | `just sel4_root_boot_check` | Missing or reordered required marker, or any failure marker. |
| Native component launch or declared grants regress | `just sel4_component_graph_check` | A component fails to launch, bind, or exit cleanly. |
| A plane gate accepts missing or contradictory evidence | `just sel4_gate_control_check` | Any mutated transcript or layout is accepted. |
| Portable component-image admission weakens | `just test_host` | A malformed wrapper or segment case is admitted. |
| Product mechanism tests, formatting, or lints regress | `just test_sel4_root`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | Non-zero exit. |
| Roadmap/devlog closure becomes structurally inconsistent | `just devlog_check` | Invalid field, section, gate, link, or index registration. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_root_boot_check` | Pass; ordered generation, allocator, frame-independence, timer, clean-exit, deliberate-fault, protection, reclamation, and ready markers | Direct |
| `just sel4_component_graph_check` | Pass; five native ELF components launched with declared grants and bounded unsupported-operation handling | Direct |
| `just sel4_gate_control_check` | Pass; 26 gates rejected 1,017 mutated transcripts and layouts | Direct |
| `just test_host` | Pass; 203 `boot-contracts`/protocol tests including the component-image admission corpus | Direct |
| `just test_sel4_root` | Pass; 114/114 host tests across 13 modules | Direct |
| `just fmt_check_all` | Pass after formatting the rollback probe import order | Direct |
| `just lint_all` | Pass | Direct |
| `just ruff` | Pass | Direct |
| `just typos` | Pass | Direct |
| `just devlog_check` | Pass | Direct |

## Decisions

- **Decision:** Reclassify free-frame reuse as non-equivalent on the monotonic seL4 root allocator, while requiring exact per-task CSlot accounting and zero live resources.
  **Rationale:** A free-count comparison would be flat by construction and could pass without reclamation. Exact owned-range and resource-table accounting observes the invariant the product can violate.

- **Decision:** Do not port custom PMM/VMM/heap/APIC tests.
  **Rationale:** Those test mechanisms supplied by seL4. Product acceptance stays at the boundary: allocations are independent and accounted, VSpaces isolate children, mapping rights fault correctly, timers arrive, and faults remain attributable.

- **Decision:** Retire the custom AArch64 stage-0/EL1 gate as historical P2.1 evidence.
  **Rationale:** The seL4 product boots through its pinned loader. Both UEFI stage-0 targets remain compiled and linted, but their custom-kernel transfer is no longer a product runtime path.

- **Decision:** Fail the historical NVMe and Framework gate names closed.
  **Rationale:** The deleted QEMU/custom-kernel transport did not satisfy M5.7's physical evidence. A passing alias would silently weaken the storage-safety contract.

## Open risks and follow-ups

- M5.7 remains blocked: no seL4 NVMe userspace driver or observed removable-media Framework boot exists.
- P4 remains physical evidence: a QEMU seL4 pass does not qualify Raspberry Pi 5 hardware.
- RP2's current text still describes custom-kernel mechanisms and must be rewritten around the seL4 boundary before implementation.

## Artifacts and provenance

- Roadmap closure: [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md), P5 and P5.4.final.
- Backlog closure: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md), B31.
- Prior deletion audit: [`devlog/2026-08-08-p5-4-final-deletion-audit/`](../2026-08-08-p5-4-final-deletion-audit/index.md).
- Verification output was observed in this cutover session; no inherited legacy-kernel pass is used to claim the final state.
