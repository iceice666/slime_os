# P1 — x86-64 architecture boundary extraction

| Field | Value |
|---|---|
| Date | 2026-08-02 |
| Kind | Change |
| Status | Verified |
| Scope | `kernel/src/arch/`, new `kernel/src/platform/`, `kernel/src/memory/vmm.rs`, `kernel/src/syscall/mod.rs`, `kernel/src/task/mod.rs`, `kernel/src/drivers/`, `kernel/src/time/`, `components/runtime/src/arch/`, `stage0/src/arch/`, `scripts/check/check-x86-portability.py`, `docs/syscall-abi.md` |
| Roadmap | P1 |
| Gates | `just x86_portability_check`, `just test`, `just product_boot_check`, `just rollback_check`, `just architecture_contract_check`, `just generation_check`, `just contracts_check`, `just test_host`, `just fmt_check_all`, `just lint_all` |
| Trigger | P1 implementation; P0 left the executable-artifact contracts target-qualified but the kernel still named x86 mechanism throughout architecture-neutral code. |
| Baseline | 191 `just test` assertions passing and a healthy 45-slot `just product_boot_check` vertical slice, measured on this tree before the change. |

## Summary

x86-64 trap frames, context switching, control registers, page tables, TLB
maintenance, interrupt masking, port I/O, the APIC, the UART, and the QEMU
debug-exit device now sit behind an explicit `arch/x86_64` boundary, with
PC-class machine assembly (ACPI, PCI ECAM, i8042, ACPI power) separated again
into `kernel/src/platform/`. Architecture-neutral code reaches a mechanism only
through names `arch::mod` selects by target. The syscall layer stopped reading
`rax`/`rdi`/`rsi`/`rdx`/`r10`/`r8` and now uses semantic accessors implementing
the `docs/syscall-abi.md` calling convention, so one syscall body serves every
architecture. `just x86_portability_check` enforces the boundary two ways: a
source allowlist over the neutral trees, and a real `cargo build` of the neutral
kernel library and component runtime for `aarch64-unknown-none`. Observable x86
behavior is unchanged — the same 191 assertions pass and the product generation
reaches the same healthy 45-slot slice. This milestone does not boot AArch64.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Architecture selection | `arch/mod.rs` cfg-dispatches `x86_64` / `aarch64` and re-exports one mechanism surface (`cpu`, `paging`, `trap`, `context`). `lib.rs`'s unconditional `pub use arch::x86_64::{…}` became a target-selected re-export, so every leak became a type error rather than a convention. | Neutral code names a mechanism, never an ISA. |
| CPU mechanism | New `arch/x86_64/cpu.rs` consolidates port I/O, MSR access, interrupt masking, `without_interrupts`, idle/park, breakpoint, debug exit, and the SIMD entry baseline. Four duplicate `outb`/`inb` pairs and two duplicate `without_interrupts` implementations collapsed into one each. | One definition per mechanism; a masking-policy change cannot drift between copies. |
| Page tables | `arch/x86_64/paging.rs` owns the table shape, entry encoding, root register, and TLB instruction. `memory/vmm.rs` keeps the walk, allocation discipline, permission checks, and teardown, parameterized by those constants. Neutral callers name intent (`PTE_DEVICE`) rather than a bit position. | Page-table *format* is architecture-specific; mapping *policy* is not. |
| Privilege transitions | The two hand-written `global_asm!` context-switch stubs moved from `task/mod.rs` to `arch/x86_64/context.rs`, with the switch scratch stack. The frame byte offsets the assembly encodes are now pinned by `const` assertions in `arch/x86_64/trap.rs`. | A field reorder fails to compile instead of silently corrupting every privilege transition. |
| Syscall ABI | `UserFrame` gained `syscall_number()`, `arg(0..=4)`, `set_return()`, `set_aux_return()`, `from_user()`, `for_user_entry()`, and `zeroed()`. All 189 raw register reads in `syscall/mod.rs` were rewritten to use them; `USER_TOP` now comes from `arch::trap::USER_ADDRESS_TOP`. | One semantic syscall contract with a per-architecture register mapping, as `docs/syscall-abi.md` specifies. |
| Platform split | ACPI, PCI ECAM, and ACPI power moved out of `arch/x86_64` into `kernel/src/platform/`; i8042 controller bring-up split from `drivers/input.rs` into `platform/i8042_keyboard.rs`, leaving decoding, the queue, and the waiter neutral. `drivers/device_discovery.rs` gives the boot graph a bus-neutral device list. | ACPI/PCI/UEFI policy is not the interface an AArch64 device-tree platform must implement. |
| AArch64 surface | `arch/aarch64/` supplies the same mechanism surface with real constants (stage-1 descriptor encodings, `SPSR_EL1` mode bits, `x8`/`x0`–`x4` syscall registers) and `unimplemented!()` bodies naming P2. `components/runtime/src/arch/aarch64.rs` carries a real `svc #0` stub. | Neutral code can be built for a second target; nothing claims AArch64 runs. |
| Stage-0 | Page-table construction, `enable_nxe`, and the kernel entry transfer moved to `stage0/src/arch/x86_64.rs`, leaving generation selection, BootState, release authorization, and rollback neutral in `main.rs`. Every extracted path still returns `BootError`. | Stage-0's verified-generation flow is shared; its entry mechanism is per-profile. |
| Encoding polarity | `is_block`, `is_writable`, and `make_read_only` are predicates on the paging boundary rather than exported bitmasks, because x86 *sets* a bit to mean block and to permit writes while AArch64 *clears* one for both. `PTE_HUGE` is no longer exported. | A single shared mask cannot express opposite polarities; a predicate can. |
| Verification | Added `just x86_portability_check`: a source allowlist over five neutral trees plus a `cargo build` of the neutral kernel and runtime for `aarch64-unknown-none`. `rust-toolchain.toml` declares that target so a fresh checkout reproduces the gate. | Boundary drift fails a gate rather than surviving review. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| New x86 assembly, register, control-register, ELF, or QEMU assumption in neutral code | `just x86_portability_check` | The allowlist names the file, line, and token; or the AArch64 cross build fails to assemble. |
| Undeclared `cfg(target_arch)` dispatch growing a silent x86-only path in neutral code | `just x86_portability_check` | The dispatch scan rejects any `cfg` outside the declared dispatch points. |
| Boundary extraction changing observable x86 behavior | `just test`, `just product_boot_check` | Assertion count departs from 191, or the product slice stops reaching 45 healthy slots. |
| A mis-transformed syscall argument or return | `just test`, `just product_boot_check` | The QEMU corpus exercises IPC, spawn, supervision, shared buffers, directories, and storage over the rewritten handlers. |
| Frame layout drifting from the context-switch assembly | Compile-time `const` assertions in `arch/x86_64/trap.rs` | The kernel fails to compile with the offending offset named. |
| Stage-0 entry path regression | `just rollback_check`, `just generation_check` | Boot selection, corrupt-release drain, or deterministic build fails. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test` | Passed; 191 assertions across 20 QEMU test binaries, 0 failed — identical to the pre-change baseline measured on this tree. | Direct |
| `just product_boot_check` | Passed; healthy vertical slice in 45 capability slots, none of the 17 scaffolding components declared — identical to baseline. | Direct |
| `just x86_portability_check` | Passed; 186 Rust files scanned with no x86 mechanism outside the boundary, and both `slime_os-kernel` and `slime-rt` built for `aarch64-unknown-none`. | Direct |
| Gate-fires probe: `core::arch::asm!("hlt")` added to `kernel/src/ipc/mod.rs` | The allowlist rejected it with file, line, and reason. Reverted. | Direct |
| Gate-fires probe: `frame.rax` read added to `kernel/src/ipc/mod.rs` | The source scan did *not* catch it; the AArch64 cross build did, failing with `E0609`. Reverted. This is why the gate builds rather than checks. The scan's register-field rule was then broadened so it catches this too. | Direct |
| Gate-fires probe: `cfg(all(target_arch = "x86_64", …))` constant divergence in `kernel/src/ipc/mod.rs`, single-line and multi-line | Both rejected after hardening. The reviewer demonstrated the original shape-matching regex admitted composite `cfg` forms — precisely the per-architecture semantic divergence the roadmap forbids — and neither gate half saw it. The scan now brace-matches whole `cfg` extents across lines. Reverted. | Direct |
| Independent review of the mechanical syscall rewrite | A reviewer inverted the transform and diffed the normalized result against `git show HEAD:kernel/src/syscall/mod.rs`: zero token differences across all 187 register references, including the `SYS_SPAWN`/`SYS_SUPERVISION_STATUS`/`SYS_INPUT_READ` handlers where argument-`rdx` and auxiliary-return-`rdx` could have been conflated. | Direct |
| `just rollback_check` | Passed; a failing pending generation returned to known-good. | Direct |
| `just architecture_contract_check` | Passed; P0's target-profile, image-revision, and cross-profile admission contracts still hold. | Direct |
| `just generation_check` | Passed; two normalized builds byte-identical, boot store admits the exact target-qualified closure. | Direct |
| `just contracts_check` | Passed; 19 fixtures and 18 generation/profile pairs agree with the boot-layout resource. | Direct |
| `just test_host` | Passed; boot-contracts and slime-proto host suites. | Direct |
| `just fmt_check_all`, `just lint_all` | Passed with warnings denied across kernel, components, stage0, and boot-contracts. | Direct |
| `just ruff` | Passed for the new check script. | Direct |
| `just typos` | Fails only on an untracked 62 MB editor-session HTML dump in the repository root (`omp-session-*.html`), which is unrelated to this change and not part of the tree. No repository source or document has a typo hit. | Direct |
| AArch64 execution | **Not attempted.** No AArch64 kernel boots, schedules, or runs a component. The cross build proves the boundary, not the port. | — |

## Decisions

- Decision: Make the AArch64 gate a `cargo build`, not a `cargo check`.
- Rationale: rustc validates explicit inline-assembly register operands at type-check time but sends mnemonics to LLVM only during codegen. A probe confirmed `asm!("hlt")` passes `cargo check --target aarch64-unknown-none` and fails `cargo build`. A check-based gate would have been vacuous for exactly the mechanism it exists to catch.
- Rejected alternative: `cargo check`, which is faster and was the obvious reading of "can be type-checked for AArch64".

- Decision: Give `arch/aarch64` the full mechanism surface with `unimplemented!()` bodies, rather than scoping the gate to a hand-listed neutral subset.
- Rationale: one unambiguous command covers the whole neutral library, and an exclusion list would need maintaining and could quietly grow to hide a leak.
- Rejected alternative: a neutral-only crate or feature, which is a larger structural change than a boundary extraction should make.

- Decision: Separate PC-class platform assembly into `kernel/src/platform/` instead of leaving it in `arch/x86_64/`.
- Rationale: P1's own deliverable requires that ACPI/PCI/UEFI policy not become the interface AArch64/RPi5 must implement. ACPI table parsing and PCI ECAM contain no ISA mechanism; they are machine description, and a device-tree platform replaces them without touching the ISA files.
- Rejected alternative: leaving them under `arch/x86_64`, which would have conflated "not portable" with "instruction set".

- Decision: Keep the syscall table identical across targets, with a `BlockDevice` that admits no transport on a target without one.
- Rationale: the roadmap requires one semantic contract per logical syscall. A syscall that disappears on another architecture would be a second contract. An absent transport is an ordinary `DeviceNotFound` outcome the services already handle.
- Rejected alternative: `cfg`-gating the storage syscalls, which compiles but forks the ABI.

- Decision: Admit exactly one x86 token outside the boundary — `feature(abi_x86_interrupt)` in `kernel/src/lib.rs`.
- Rationale: crate features must be declared at the crate root and cannot live in the module that uses them. It is `cfg`-gated on the target and named explicitly in the check, so any *other* occurrence still fails.
- Rejected alternative: weakening the ABI-feature pattern, which would have let a genuine leak through.

## Open risks and follow-ups

- [ ] P2 must implement every `unimplemented!()` in `kernel/src/arch/aarch64/` and `stage0`'s AArch64 loader, then observe the first `aarch64-qemu-virt` boot. The stubs are a compile-time surface, not a partial port.
- [ ] The AArch64 stage-1 descriptor encodings in `arch/aarch64/paging.rs` are written from the architecture reference but have never been loaded by hardware; P2 must validate them against a running EL1 before any mapping claim. The polarity inversions review surfaced (block and writability are *clear*-bit on AArch64, *set*-bit on x86) are now handled by boundary predicates rather than shared masks, but the bit positions themselves remain unverified.
- [ ] `drivers/device_discovery.rs` returns an empty list on non-x86. P2 supplies device-tree discovery; until then no AArch64 target can find a block device, which is correct but means storage gates cannot run there.
- [ ] The `time::apic` name is retained on both architectures for the timer slot. P2 should rename it to something architecture-neutral once the generic timer lands.
- [ ] The AArch64 cross build emits ten dead-code warnings for the scan-code decoder, `remap_page_in`, and `leaf_flags_in` — all reachable only from the x86-gated platform. Harmless today, but they must be resolved before any AArch64 lint gate can deny warnings (P2).
- [ ] An untracked 62 MB `omp-session-*.html` in the repository root breaks `just typos`. It is an editor artifact, not project content; it should be removed or ignored independently of this milestone.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none retained.
- Serial/debugger/model output: x86 QEMU evidence observed through `just test`, `just product_boot_check`, and `just rollback_check`. No AArch64 or physical-board evidence is claimed.
- Related roadmap item: [`P1`](../../roadmap/07-architecture-portability.md#p1-x86-64-architecture-boundary-extraction).
