# P3: RV64 QEMU reaches architecture-neutral seL4 parity

| Field | Value |
|---|---|
| Date | 2026-08-31 |
| Kind | Change |
| Status | Verified |
| Scope | RV64 target profile, seL4 QEMU build and loader route, component runtime, root architecture boundaries, and cross-platform plane checkers |
| Roadmap | P3 |
| Gates | `just riscv64_qemu_check` |
| Trigger | P3 became the prerequisite reference profile for the active Milk-V Duo P3.E bring-up lane |
| Baseline | The maintained seL4 product and architecture-neutral acceptance corpus ran only on AArch64 QEMU; RV64 retained no target-qualified root or component path |

## Summary

The pinned `qemu-system-riscv64 virt` profile now boots upstream seL4, admits a `riscv64-sel4-qemu-virt` generation, runs `slime-root` and RV64 component images, and replays the selected architecture-neutral root, wait/wake, sample, generation, and rollback corpus. Target identity is carried by the generated profile contract and checked before executable mapping. Architecture-specific timer, device mapping, ELF, component entry, and loader details stay behind explicit AArch64/RV64 boundaries. P3 is verified; this establishes the QEMU reference required by P3.E, not Milk-V Duo hardware support.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Target contract and generated bindings | Added the `riscv64-sel4-qemu-virt` architecture, ABI, page, feature, and profile records and regenerated boot bindings | A generation and every executable name one exact admitted target rather than inheriting the host or AArch64 defaults |
| seL4 build and pins | Added the pinned RV64 QEMU machine, toolchain, seL4 configuration, DTB normalization, artifact paths, and kernel-loader packaging route | One reproducible upstream seL4 profile owns the RV64 mechanism baseline |
| Root and component runtime | Added RV64 syscall, entry, linker, timer/RTC, device-map, fault-register, VSpace, and ELF handling while retaining shared lifecycle and authority mechanisms | ISA-specific mechanism remains explicit; shared contracts preserve the same capability, fault, wait, reclamation, generation, and rollback semantics |
| Generation builder | Made component target selection and external C component construction profile-qualified | RV64 components cannot be mixed with retained x86 or AArch64 artifacts |
| Verification checkers | Parameterized the existing root, wait-set, sample, generation, and rollback checkers by platform and added the aggregate `riscv64_qemu_check` gate | RV64 reuses the owning semantic checks instead of creating a parallel weaker corpus |
| Review fixes | Preserved completed device-untyped retypes on partial failure, returned typed errors for unattached RTC registers, kept AArch64 QEMU firmware behavior unchanged, and removed privileged `wfi` from U-mode components | Error paths no longer corrupt allocator accounting, panic at the timer boundary, regress AArch64 boot, or trap component idle loops |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| RV64 kernel, root, generation, or component path stops booting | `just riscv64_qemu_check` | Missing ordered root-ready markers or a build/boot failure on `qemu-riscv-virt` |
| Target identity is guessed or a foreign artifact is mapped | `just riscv64_qemu_check` | Wrong-profile admission mutation succeeds or the identity manifest disagrees with the packaged image |
| Architecture-neutral semantics diverge | `just riscv64_qemu_check` | Root, wait-set, sample, generation, rollback, or architecture-portability sub-gate fails |
| AArch64 mechanism regresses during shared-path changes | `just sel4_root_boot_check` | The pinned AArch64 root no longer reaches its ordered ready marker |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just riscv64_qemu_check` | Passed: pinned seL4/root boot plus the isolation, wait/wake, sample, generation, rollback, and architecture-portability corpus completed on `qemu-riscv-virt` | Direct |
| `just sel4_root_boot_check` | Passed: AArch64 still reached ordered generation, timer, task, IPC, fault, and ready markers | Direct |
| `just test_host` | Passed | Direct |
| `just test_sel4_root` | Passed: 213/213 tests across 19 modules | Direct |
| `just contracts_check` | Passed after regenerating the target-profile boot bindings and updating the rebuilt Slisp artifact digest | Direct |
| `just generation_check` | Passed: two isolated builds produced byte-identical admitted generation and boot-store artifacts | Direct |
| `just fmt_check_all` | Passed | Direct |
| `just lint_all` | Passed with warnings denied | Direct |
| `just ruff` | Passed | Direct |

## Decisions

- **Decision:** Keep QEMU RV64 and Milk-V Duo as distinct target profiles.
- **Rationale:** The QEMU `virt` PLIC, RTC, memory map, firmware route, and available RAM do not establish CV1800B or C906 behavior.
- **Rejected alternative:** Reuse one generic RV64 profile and infer the board from the running machine; that would weaken signed target identity and permit unsupported mechanism to be guessed.

- **Decision:** Extend existing plane checkers with a platform parameter rather than add RV64-specific copies.
- **Rationale:** P3 claims semantic parity, so both architectures must execute the same assertions and mutation logic.
- **Rejected alternative:** Add a boot-only RV64 gate; it would not establish the wait, sample, generation, or rollback exit condition.

## Open risks and follow-ups

- [ ] P3.E must separately qualify the Duo memory fit, CV1800B PLIC S-mode context, and C906 MAEE/page-table state before implementing or claiming the physical platform port.
- [ ] No P3 result establishes Milk-V Duo firmware, interrupt, storage, USB, network, display, sensor, actuator, or physical reliability behavior.

## Artifacts and provenance

- Focused report: none; the implementation, gate, and observed command results are recorded here.
- Raw transcript: none added.
- Serial/debugger/model output: the QEMU serial assertions were collected and checked by `just riscv64_qemu_check`; no separate immutable capture was added.
- Related roadmap item: [P3 RV64 QEMU vertical slice](../../roadmap/07-architecture-portability.md).
