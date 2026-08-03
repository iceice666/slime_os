# P2.2 — AArch64 exception vectors, fault decoding, and `svc` entry

| Field | Value |
|---|---|
| Date | 2026-08-03 |
| Kind | Change |
| Status | Verified |
| Scope | `kernel/src/arch/aarch64/{trap,paging,mod}.rs`, `kernel/src/bringup_aarch64.rs`, `kernel/build.rs`, `scripts/check/check-aarch64-{trap,boot}.py`, `docs/syscall-abi.md`, `Justfile`, `roadmap/07-architecture-portability.md` |
| Roadmap | P2.2, P2 |
| Gates | `just aarch64_trap_check`, `just aarch64_boot_check`, `just x86_portability_check`, `just test` |
| Trigger | P2.1 reached EL1 with memory online but installed no vector table: any fault escalated silently and `cpu::breakpoint()` was a one-way trip. |
| Baseline | AArch64 booted to `[bringup] aarch64 EL1 vertical slice reached` and exited; no exception was ever taken, no `svc` served. |

## Summary

AArch64 now takes exceptions. The architected 16-slot EL1 vector table is
installed at `VBAR_EL1`; entry stubs save the full `UserFrame` P1 defined
(`x0`–`x30`, `SP_EL0`, `ELR_EL1`, `SPSR_EL1`), call Rust under AAPCS64, and
restore the possibly-mutated frame before `eret`. `ESR_EL1.EC` is decoded into
the existing architecture-neutral `UserFaultReason` vocabulary with no AArch64
fault taxonomy added. An `svc #0` issued from EL0 dispatches into the shared
`kernel/src/syscall/mod.rs` body — the same function x86 calls — and its result
returns in `x0`. `DAIF` masking and the idle path are implemented and observed.
`just aarch64_trap_check` asserts all of this as ordered PL011 markers.

**This entry claims P2.2 only.** No component is scheduled, no per-task address
space is switched, and no interrupt is delivered. The EL0 evidence comes from a
bounded bring-up probe that builds its own TTBR0 root, runs a handful of
instructions at EL0, and tears the root down. Component execution, isolation,
and fault attribution to a task are P2.3.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Vector table | New 2048-byte, 2048-aligned 16-slot table in `global_asm!`; each slot is `b <stub>` plus padding. `install()` writes `VBAR_EL1` and asserts the alignment the architecture requires rather than trusting the linker. | An AArch64 exception has somewhere architected to go. |
| Frame save/restore | `EXCEPTION_ENTRY` saves `x0`–`x30`, `SP_EL0`, `ELR_EL1`, `SPSR_EL1` into a stack-allocated `UserFrame`, keeps SP 16-byte aligned across the `bl`, and restores every field on the way out. A `const` assertion block pins each field offset against `size_of`/`offset_of`, so a Rust-side layout change cannot silently desynchronize the assembly. | The frame saved on entry is the frame restored on return, including handler mutations. |
| ABI boundary | `trap_dispatch` is `extern "C"`, matching the x86 counterpart and the AAPCS64 argument registers the stub loads (`x0`=slot, `x1`=frame, `x2`=ESR, `x3`=FAR). | A hand-written assembly call boundary names its ABI instead of relying on the unstable Rust ABI. |
| Fault decoding | `decode_sync_fault` maps EC `0x00`/`0x3c`, `0x20`/`0x21`/`0x24`/`0x25`, and `0x22`/`0x26` onto the pre-existing `UserFaultReason` variants, with `Unknown(ec)` as the catch-all. | Both architectures report faults in one vocabulary. |
| Syscall entry | Slot-8 synchronous `EC=0x15` with `imm16 == 0` enters the shared dispatcher; the probe stage selects the fixture path only while the bring-up probe is running. | `svc` reaches the architecture-neutral syscall body, not an AArch64 stub. |
| EL0 origin dispatch | A synchronous fault from a lower EL terminates the current task through `task::terminate` with its decoded reason; an EL1 fault reports and halts. | User faults are attributed to the faulting task; kernel faults are not silently absorbed into one. |
| Deliberate `brk` gating | The EL1 `BRK` handler that advances `ELR_EL1` past the trap only fires while `PROBE_STAGE == PROBE_EXPECT_EL1_BRK`. Outside that window a compiler-emitted `brk` — the panic and UB paths LLVM emits — falls through to the kernel-fault branch. | A debug trap probe cannot turn every compiler-emitted trap into a silent no-op. |
| Shareability | `PTE_INNER_SHAREABLE` added to `PTE_LEAF`, matching stage-0's `DESC_INNER_SHAREABLE` for normal memory. | A kernel-built leaf descriptor and a stage-0-built one describe the same memory the same way. |
| TLB maintenance | New `paging::flush_tlb_all()`; a TTBR write does not implicitly invalidate AArch64 TLB entries, so the probe invalidates after installing its root and again after restoring the previous one, before releasing frames. | Frames returned to the allocator are not reachable through a stale translation. |
| Instruction coherency | Probe code bytes are cleaned to PoU by the `CTR_EL0.DminLine` stride, then `ic iallu` / `dsb ish` / `isb`. | The CPU fetches the instructions that were written, not whatever the I-cache held. |
| Gating | The probe runs only under `SLIME_AARCH64_TRAP_CHECK=1`, plumbed through `build.rs` and `option_env!`, mirroring the x86 precedent. | Default boot behavior is unchanged; the probe is verification code, not product boot. |
| Documentation | `docs/syscall-abi.md` moved the AArch64 register mapping from "P2.2 must implement" to "implemented and observed", naming the gate. | The ABI document reports what runs, not what is planned. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Vector table not installed, misaligned, or a slot dispatches wrongly | `just aarch64_trap_check` | The `vectors installed vbar=` marker is missing, or a fault marker appears out of order. |
| Fault decoding regresses to the wrong reason | `just aarch64_trap_check` | The `el1 sync ec=0x3c reason=UndefinedOp` or `el0 sync … elr=0x400004` marker fails to match. |
| A syscall argument register stops carrying its value | `just aarch64_trap_check` | The `svc nr=1 args=0x1111…,0x2222…,0x3333…,0x4444…,5 result=-4` marker fails to match. |
| Frame restore drops a register or the handler mutation | `just aarch64_trap_check` | The `frame restored gprs=31 sp=0x402000 handler_mutation=0x4d55544154454432` marker fails to match. |
| `DAIF` masking or restoration breaks | `just aarch64_trap_check` | The `daif entry_masked=true enabled_window=true masked_inside=true restored_enabled=true final_masked=true` marker fails to match. |
| Probe leaks into default boot | `just aarch64_boot_check` | Trap markers appear in a run that did not set the env gate. |
| AArch64 mechanism leaking into neutral code | `just x86_portability_check` | The allowlist names the file and token, or the cross build fails. |
| x86 behavior changed by the shared-code edits | `just test` | Assertion count departs from baseline. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just aarch64_trap_check` | **Passed.** Observed in order: `vectors installed vbar=0xffffffff90000800`; `daif entry_masked=true enabled_window=true masked_inside=true restored_enabled=true final_masked=true`; `el1 sync ec=0x3c reason=UndefinedOp elr=0xffffffff90017258`; `svc nr=1 args=0x1111111111111111,0x2222222222222222,0x3333333333333333,0x4444444444444444,5 result=-4`; `el0 sync ec=0x3c reason=UndefinedOp elr=0x400004`; `frame restored gprs=31 sp=0x402000 handler_mutation=0x4d55544154454432`; `complete`. Run ended through semihosting, not the timeout. | Direct |
| Shared-dispatch evidence, not a stub | The probe's `svc` is `SYS_SEND` with `a4 = MAX_CAPS_PER_MSG + 1`; `result=-4` is `ERR_INVALID_ARG` produced by the bounds check inside `kernel/src/syscall/mod.rs`, the same body x86 calls. An architecture-specific stub could not produce it. | Direct |
| `just aarch64_boot_check` | **Passed.** Default boot is unchanged and emits no trap markers; the wrong-target generation is still rejected with `Target(ProfileMismatch)` before executable mapping. | Direct |
| `just test` | Passed; x86 corpus unchanged. | Direct |
| `just x86_portability_check` | Passed; 188 neutral Rust files scanned, both crates cross-build for `aarch64-unknown-none`. | Direct |
| `just architecture_contract_check`, `just generation_check`, `just contracts_check`, `just test_host`, `just fmt_check_all`, `just lint_all`, `just ruff` | Passed. | Direct |
| Independent review, round 1 | Ten findings. Substantive ones fixed: `trap_dispatch` lacked `extern "C"` on an assembly call boundary; the TLB was not invalidated after TTBR0 writes, so probe frames could be reached through stale translations after release; probe leaf descriptors omitted `nG`, letting EL0-accessible global entries outlive the frames; `complete_user_probe` returned to EL1 with `DAIF` cleared, contradicting the invariant this slice asserts; page-table frames were released without zeroing on two construction bail-outs; load-bearing `debug_assert`s were compiled out of the release gate that runs them; and the probe ran on every AArch64 boot rather than behind an env gate. | Direct |
| Independent review, round 2 | Terminated with an output failure after analysis, but produced one decisive result before it: the generic EL1 `BRK` handler would swallow **compiler-emitted** `brk` instructions — the panic and UB traps LLVM emits — by advancing `ELR_EL1` past them, turning a kernel panic into a silent skip. Fixed by gating that handler on the explicit probe stage. | Direct |
| Independent review, round 3 | **No findings; verdict `correct`, confidence 0.78.** Re-traced the vector table size and alignment, frame offsets against the const-assert block, SP alignment across the `bl`, register save/restore coverage, the restricted `BRK` gate, ESR decoding, shared-dispatch evidence, probe root lifetime and cache/TLB ordering, `PTE_INNER_SHAREABLE` compatibility with stage-0, `DAIF` handling, and default-boot gating. | Direct |
| AArch64 components, scheduling, interrupts, timer | **Not attempted.** No component is launched, no address space is switched per task, no interrupt is delivered. Those are P2.3 and P2.4. | — |
| Raspberry Pi 5 | **Not attempted.** QEMU `virt` only. | — |

## Decisions

- Decision: Prove EL0 with a bounded, self-contained probe that builds and releases its own TTBR0 root, rather than waiting for P2.3's task machinery.
- Rationale: P2.2's required checks name `svc` and frame preservation, and those are only observable from EL0. Deferring them would leave the slice claiming an unobserved exit condition — exactly what P2.1's decomposition existed to prevent.
- Rejected alternative: asserting the register mapping from an EL1-issued `svc`, which cannot exercise `SP_EL0`, the lower-EL vector slots, or the EL0 fault path.

- Decision: Gate the deliberate EL1 `BRK` handler on an explicit probe stage instead of handling `EC=0x3c` at EL1 generically.
- Rationale: LLVM emits `brk #1` for panics and UB checks. A generic handler that advances `ELR_EL1` past any EL1 `BRK` converts every one of those into a silent no-op that continues executing into whatever the compiler assumed was unreachable. The probe wants exactly one `brk`, at one moment; nothing else should be absorbed.
- Rejected alternative: matching on the `BRK` immediate, which distinguishes `#0` from `#1` today but is a compiler implementation detail, not a contract.

- Decision: Put the probe behind `SLIME_AARCH64_TRAP_CHECK` rather than running it on every AArch64 boot.
- Rationale: it maps EL0-accessible pages, switches the translation root, and takes deliberate faults. That is verification scaffolding, and B11 established that scaffolding does not belong in the product boot path.
- Rejected alternative: unconditional execution, which was the first implementation and which the round-1 review flagged.

- Decision: Add `flush_tlb_all()` and call it around every root switch.
- Rationale: unlike x86's CR3 write, an AArch64 TTBR write does not implicitly invalidate TLB entries. Frames released back to the allocator while a stale EL0-accessible translation still resolves to them are a use-after-free the MMU performs on your behalf.
- Rejected alternative: relying on the root switch alone, which is the x86 intuition and is wrong here.

## Open risks and follow-ups

- [ ] `complete_user_probe` returns to EL1 with all four `DAIF` bits masked, widening `A`/`F` beyond the entry state, and leaves `SP_EL0` pointing at the torn-down probe stack. Harmless because `exit_qemu` immediately follows and nothing reads `SP_EL0` at EL1, but P2.3 must restore the true entry state when a real task returns.
- [ ] An unexpected EL1 fault reaches `hlt_loop()`, so the gate fails by 600-second timeout rather than a fast deterministic exit. The gate still fails, just slowly.
- [ ] SIMD/FP state is not saved or restored across exception entry, matching ABI revision 1, which excludes it. The AArch64 context-switch slice must admit that state before a component may depend on it.
- [ ] `KERNEL_HALF_START` in `memory/vmm.rs` still describes x86's single-root layout. P2.3 must move that split behind `arch::paging` before the first real EL0 task exists, or `free_user_half` will leak the upper half of every user root. The probe sidesteps this by building its root by hand.
- [ ] Fault attribution calls `task::terminate`, which requires a current scheduler entry. No AArch64 task exists yet, so that path is unexercised; P2.3 is the first slice that can observe it.
- [ ] No interrupt is delivered on AArch64. `DAIF` masking is observed, but nothing is waiting behind it until P2.4 brings up GICv3 and the generic timer.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none retained; the serial transcript is reproduced by `just aarch64_trap_check`, which prints it.
- Serial/debugger/model output: AArch64 evidence observed through `just aarch64_trap_check` and `just aarch64_boot_check`; x86 evidence through `just test`.
- Related roadmap item: [`P2.2`](../../roadmap/07-architecture-portability.md#p22--exception-vectors-fault-decoding-and-svc-entry).
