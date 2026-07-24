# Multi-architecture roadmap boundary

| Field | Value |
|---|---|
| Date | 2026-07-24 |
| Status | Proposed |
| Scope | Roadmap dependencies, executable contracts, x86-64/AArch64/RV64 boundaries, ROS 2 and embedded targets |
| Trigger | Decision to prepare Slime OS for ROS 2 devices and embedded Linux-class hardware |
| Baseline | The implemented kernel, stage-0, component builder, and QEMU harness are x86-64-specific; userspace authority and protocol contracts are largely ISA-independent |

## Summary

Slime OS will retain x86-64 as its deterministic reference architecture, add AArch64 as the first non-x86 product-oriented architecture, and add RV64 as the second architecture profile. The roadmap now requires executable artifacts and generations to identify an exact target, requires the existing x86 implementation to be extracted behind an enforced architecture/platform boundary before H2 or C9 establishes more low-level contracts, and treats MCU-class systems without the required isolation model as bounded external companions rather than weakened kernel ports. This entry records a proposed design decision; no new architecture has been implemented or booted.

## Observable symptom

- Command: Documentation and source inspection only.
- Expected: Determine whether future architecture work could reuse current contracts without silently treating x86 mechanisms as universal.
- Observed: Capability, generation, BootState, release, C7/C8, and ROS protocol semantics can remain shared, but executable formats and low-level implementation paths do not currently identify or isolate their architecture.
- Exit/fault/serial evidence: None; no runtime verification was performed for this documentation decision.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `kernel/.cargo/config.toml` and `components/.cargo/config.toml` select `x86_64-unknown-none`. | A second target requires explicit build/profile selection rather than another implicit default. |
| 2 | `kernel/src/trap.rs`, `kernel/src/task/mod.rs`, `kernel/src/memory/vmm.rs`, `kernel/src/interrupts.rs`, and `kernel/src/time/apic.rs` encode x86 registers, `iretq`, CR3, four-level page tables, IDT/APIC/PIT, and x86 interrupt state. | The kernel needs a real architecture boundary; conditional compilation around individual instructions would preserve the coupling. |
| 3 | `components/runtime/src/syscall.rs` defines the userspace ABI as `int 0x80` with x86-64 registers. | Syscall semantics can remain shared, but each ISA needs a documented trap and calling convention. |
| 4 | `scripts/build-generation.py`, `components/component.ld`, and `kernel/linker.ld` accept and emit only x86-64 ELF layouts. | Build and executable validation must be selected by a signed exact target profile. |
| 5 | Component and kernel image schemas carry ABI versions but no explicit architecture ID. | A versioned Zutai format change is required before non-x86 executable generations are safe. |
| 6 | ROS R1/R2 are already userspace wire profiles over C8/H6, and ROS 2 Jazzy admits ARM64 as a primary platform. | AArch64 has direct robot-product value without moving ROS graph or DDS policy into the kernel. |

## Root cause

The project began with one deterministic x86-64 QEMU/Framework target, so architecture identity was implicit in Cargo targets, linkers, builders, stage-0 page tables, kernel trap frames, and device bring-up. The architecture-neutral authority and protocol design did not itself create the coupling; executable and privileged mechanism boundaries remained implicit because no second ISA existed to force them explicit.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Roadmap | Add `07-architecture-portability.md` with P0–P4 gates. | Every architecture claim has bounded deliverables, checks, and an observable exit condition. |
| Artifact design | Require architecture-qualified component/kernel images and exact generation targets. | A valid hash cannot make code executable on the wrong ISA or ABI. |
| Kernel sequencing | Place x86 boundary extraction before H2 and C9. | PCI/APIC/CR3/register layouts cannot become universal contracts by accident. |
| ROS sequencing | Keep R1/R2 deterministic on the reference QEMU target, then replay the same corpus on AArch64 for heterogeneous evidence. | Wire compatibility remains independent from kernel ISA and physical evidence. |
| Embedded scope | Limit the Slime kernel to 64-bit MMU systems and make MCU-class devices bounded companions. | The capability/isolation model is not weakened to claim broader hardware coverage. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Wrong-architecture executable accepted | Planned `just architecture_contract_check` | Target/image mismatch reaches mapping or execution. |
| x86 assumptions leak back into shared code | Planned `just x86_portability_check` allowlist | x86 instruction, register, linker, or QEMU constant outside admitted files. |
| AArch64 or RV64 diverges semantically | Planned architecture QEMU checks replay shared isolation/B2/C7 corpus | Different capability, error, wake, reclaim, or rollback result. |
| Physical support inferred from emulation | Board-specific P4 evidence gates | Generic ARM/RISC-V claim without named board and recorded run. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Roadmap link, identifier, dependency, and status consistency inspection | Performed as documentation validation; no runtime behavior changed | Direct |
| Kernel/AArch64/RV64 boot or cross-architecture build | Not run; implementation does not yet exist | Direct |

## Decisions

- Decision: Keep x86-64 as the reference, implement AArch64 first, then RV64.
- Rationale: AArch64 has immediate ROS 2 and embedded Linux-class relevance; RV64 must influence contracts now but need not delay the first non-x86 vertical slice.
- Rejected alternative: Finish Framework hardware and robot-runtime work before extracting architecture boundaries. This would allow H2 and C9 to encode more x86-specific device, interrupt, timer, and context assumptions.
- Rejected alternative: Port the same kernel to MCU-class no-MMU systems. This would require a second isolation/security model and make capability claims non-equivalent.
- Rejected alternative: Treat one successful QEMU boot as generic ARM or RISC-V support. Profiles include firmware, privilege, page, interrupt, timer, and device contracts, and physical boards require separate evidence.

## Open risks and follow-ups

- [ ] Clear or explicitly defer the active backlog before opening P0.
- [ ] Implement P0's versioned Zutai artifact contracts and retained-x86 decoding.
- [ ] Extract P1 before H2 or C9 begins.
- [ ] Pin exact QEMU machine/firmware versions when P2 and P3 implementation starts.
- [ ] Select named physical boards only after their QEMU profiles expose the required evidence surface.

## Artifacts and provenance

- Focused report: This entry.
- Raw transcript: None.
- Serial/debugger/model output: None; documentation decision only.
- Related roadmap item: [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md)
