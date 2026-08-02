# P2.1 — AArch64 firmware handoff, EL1 entry, and translation tables

| Field | Value |
|---|---|
| Date | 2026-08-03 |
| Kind | Change |
| Status | Verified |
| Scope | `roadmap/07-architecture-portability.md` P2 decomposition, `stage0/src/arch/aarch64.rs`, `kernel/src/arch/aarch64/`, `kernel/src/arch/boot_context.rs`, `kernel/src/memory/vmm.rs`, `kernel/src/bringup_{x86_64,aarch64}.rs`, `kernel/src/drivers/frame_buffer.rs`, `kernel/.cargo/config.toml`, `kernel/scripts/build-iso.sh`, `scripts/build/build-generation.py`, `scripts/check/check-aarch64-boot.py`, `flake.nix`, `rust-toolchain.toml` |
| Roadmap | P2.1, P2 |
| Gates | `just aarch64_boot_check`, `just x86_portability_check`, `just test`, `just product_boot_check`, `just rollback_check`, `just architecture_contract_check`, `just generation_check`, `just contracts_check`, `just test_host`, `just fmt_check_all`, `just lint_all` |
| Trigger | P1 established the architecture boundary and proved neutral code builds for AArch64; nothing had executed an AArch64 instruction. |
| Baseline | 191 `just test` assertions and a healthy 45-slot `just product_boot_check` slice on x86-64; no non-x86 execution of any kind. |

## Summary

Slime OS boots on AArch64. `qemu-system-aarch64 -machine virt` loads the
stage-0 UEFI loader, which selects and verifies an `aarch64-qemu-virt`
generation, builds stage-1 translation tables, and enters the kernel at EL1 with
the MMU and both caches enabled; the kernel brings up physical and virtual
memory over the direct map, allocates from a working heap, and reports the
generation and BootState the verified loader chose. This is the first non-x86
instruction this project has executed.

P2 as written was one milestone covering an entire second architecture, whose
exit condition cannot be partially observed. It is decomposed into P2.1–P2.6 on
the same principle that split C7 and C8.9, each sub-slice owning one mechanism
and one gate. **This entry claims P2.1 only.** No component runs, no syscall is
served, and no interrupt is delivered on AArch64; those are P2.2–P2.4. The x86
corpus is unchanged at 191 assertions.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Milestone scope | Decomposed P2 into P2.1–P2.6 with per-slice gates and exit conditions. The parent's deliverables stay authoritative for the aggregate; no sub-slice may claim them. | A milestone's exit condition is observable, or it is decomposed until it is. |
| Stage-0 AArch64 | New `stage0/src/arch/aarch64.rs`: two-root stage-1 tables (`TTBR0_EL1` identity, `TTBR1_EL1` kernel image and direct map), MAIR/TCR configuration, MMU enable, and the EL1 transfer. Generation selection, BootState, release authorization, and rollback stay neutral in `main.rs`. Every fallible step returns a `BootError`; the crate's no-panic, no-indexing rules hold. | One verified-boot flow, with only entry mechanism per profile. |
| Attribute indices | Corrected the kernel's `MAIR_EL1` attribute indices, which were inverted relative to the register stage-0 programs: `PTE_CACHE_DISABLE` selected cacheable normal memory and `PTE_WRITE_THROUGH` selected device memory. Both sides now name the indices explicitly and point at each other. | A descriptor names an index; the register defines what it means. The two cannot drift silently. |
| Architecture mechanism | Replaced P1's `unimplemented!()` stubs with real `DAIF` masking, `WFI` idle, `TTBR` access, `TLBI` maintenance, PL011, `CPACR_EL1` SIMD enable, semihosting exit, and `CurrentEL`/`SCTLR_EL1`/`TCR_EL1` reporting. | The boundary P1 declared is now implemented, not just shaped. |
| Handoff decoding | Moved the stage-0 handoff decoder from `arch/x86_64/boot.rs` into `arch/boot_context.rs`. It reads contract bytes and was never x86 mechanism; x86 keeps only its Limine test-harness path. | Both architectures decode the same handoff bytes through one decoder. |
| Leaf descriptors | Added `PTE_LEAF` to the paging boundary: the structural bits a valid leaf needs, independent of the permissions a caller asks for. On x86 that is the present bit; on AArch64 it is additionally the page-type bit and access flag. | A caller expressing *permissions* does not have to know what makes a descriptor structurally valid on each architecture. |
| Translation roots | Added `kernel_root()` and `root_for()`. x86 has one root for both halves; AArch64 splits them across `TTBR0_EL1` and `TTBR1_EL1`, so a kernel-half mapping must start its walk from the kernel root. | Neutral mapping code selects a root by address instead of assuming one root serves the whole space. |
| Headless boot | A machine with no framebuffer is now a legitimate configuration: stage-0 encodes absence as a zero address and geometry, and the framebuffer console stays uninitialized rather than forming a slice from a null pointer. An unsupported *present* framebuffer still fails closed. | A serial-only machine boots; a device we cannot describe correctly is still refused. |
| Bring-up | Split `kernel_main` into `bringup_x86_64.rs` and `bringup_aarch64.rs`. The device set and initialization order differ per machine and have no useful shared sequence. | Each architecture owns its bring-up order; both converge on the neutral runtime. |
| Build path | AArch64 kernel builds as a PIE with `-Z build-std`; `build-iso.sh` selects the stage-0 target and the UEFI removable-media boot filename by profile; `SLIME_TARGET_PROFILE` retargets the generation manifest through its own field so every downstream target check still runs. | One component graph produces a target-qualified generation for either profile. |
| Dev shell | `flake.nix` installs both AArch64 Rust targets and exports AAVMF firmware. | A fresh `nix develop` can run the gates that exist. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| AArch64 boot regresses at any stage | `just aarch64_boot_check` | An ordered serial marker is missing or out of order, a failure marker appears, or the run times out instead of exiting. |
| x86 behavior changed by the shared-code edits | `just test`, `just product_boot_check`, `just rollback_check` | Assertion count departs from 191, or the product slice stops reaching 45 healthy slots. |
| New architecture mechanism leaking into neutral code | `just x86_portability_check` | The allowlist names the file and token, or the cross build fails. |
| Target qualification weakened by the retarget override | `just architecture_contract_check`, `just generation_check` | Profile admission or deterministic build fails. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just aarch64_boot_check` | **Passed.** Observed under `qemu-system-aarch64 -machine virt -cpu cortex-a72 -m 512M` with AArch64 UEFI firmware: stage-0 selected slot A and emitted its BootState trace, verified the generation and kernel, and transferred; the kernel reported `exception level EL1`, `mmu=1 dcache=1 icache=1 t0sz=16 t1sz=16`, `direct map offset=0xffff800000000000`, `PMM: 111363 / 117521 frames free`, a working heap, and the generation identity and BootState stage-0 chose. The run ended through semihosting, not the timeout. | Direct |
| Gate-fires probe: suppressed the final bring-up marker | The gate failed with `missing expected boot marker`. Reverted. | Direct |
| Wrong-target rejection | A real `x86_64-qemu-virtio` generation wrapped in an AArch64 boot image was refused by the AArch64 loader with `Target(ProfileMismatch)`, and never reached kernel execution. Asserted by the same gate. | Direct |
| Gate-fires probe: planted `cfg(target_arch = "x86_64")` dispatch in `memory/vmm.rs` | Rejected, confirming the narrowed portability allowlist still covers that file. Reverted. | Direct |
| `just test` | Passed; 191 assertions, 0 failed — identical to baseline. | Direct |
| `just product_boot_check` | Passed; healthy vertical slice in 45 capability slots — identical to baseline. | Direct |
| `just x86_portability_check` | Passed; 188 neutral Rust files scanned, both crates cross-build for `aarch64-unknown-none`. It rejected three AArch64 system-register reads that had been written into neutral bring-up code, which were moved behind `arch::cpu`. | Direct |
| `just rollback_check` | Passed; a failing pending generation returned to known-good. | Direct |
| `just architecture_contract_check`, `just generation_check`, `just contracts_check` | Passed; P0's profile admission and deterministic-build contracts still hold. | Direct |
| `just test_host`, `just fmt_check_all`, `just lint_all`, `just ruff` | Passed. | Direct |
| Independent review of the diff | Nine findings, all resolved or recorded. Three substantive: the AArch64 stage-0 loader was **never linted** (`lint_stage0` ran clippy only for `x86_64-unknown-uefi`, leaving the crate-level `unwrap`/`expect`/`panic`/`indexing_slicing` denials unchecked on ~450 lines — now both targets are linted and the `identity_op` it surfaced is fixed); **`MAIR_EL1` indices were inverted** between stage-0 and the kernel, so `PTE_DEVICE` would have mapped MMIO cacheable the first time P2.5 mapped a virtio-mmio window, which no boot-path test could catch; and **two required checks were claimed but unobserved**, of which wrong-target rejection is now a real gate scenario and malformed-mapping moved to the slice that can exercise it. Smaller: the exit-status comment overclaimed, the portability allowlist was widened to whole files where a lint-suppression exemption sufficed, the cache-line size was a hardcoded assumption, the absent-framebuffer zero address was rebased onto the direct map making its guard dead, and the paging module still claimed to be unvalidated. | Direct |
| AArch64 components, syscalls, interrupts, storage | **Not attempted.** No component is launched, no `svc` is served, no interrupt is delivered, and no block device exists on this target. | — |
| Raspberry Pi 5 | **Not attempted.** This is QEMU `virt` only. A QEMU pass cannot close a physical-board milestone. | — |

## Decisions

- Decision: Decompose P2 into P2.1–P2.6 rather than attempting the whole architecture in one slice.
- Rationale: its exit condition is all-or-nothing — either the full vertical slice runs or none of it is evidence — so a partial attempt could not be honestly reported. The repository already split C7 and C8.9 for the same reason.
- Rejected alternative: one large commit, which would have been unreviewable and would have left the roadmap silent about where the boundary fell.

- Decision: Add `PTE_LEAF` to the paging boundary instead of making callers pass complete descriptors.
- Rationale: found by a fault. Neutral code builds leaf flags from *intent* (`PTE_USER | PTE_PRESENT` for a read-only user page), which happens to be a complete x86 descriptor but omits AArch64's page-type bit and access flag. The structural bits belong to the architecture, not the caller.
- Rejected alternative: adding the bits to every neutral flag site, which would have put architecture knowledge in exactly the code the boundary exists to keep neutral.

- Decision: Add `root_for()` rather than keeping one active root.
- Rationale: also found by a fault. AArch64 selects `TTBR0_EL1` or `TTBR1_EL1` by address, so a kernel-half mapping walked from the active root writes a descriptor the CPU never consults — the heap mapping silently went nowhere.
- Rejected alternative: making AArch64 use a single root, which fights the architecture and would forfeit the cheaper address-space switch it exists to provide.

- Decision: Read the data cache line size from `CTR_EL0` rather than assuming 64 bytes.
- Rationale: the page tables must be cleaned to the point of coherency before the cache is disabled, and stepping by more than the true line size silently skips lines, leaving stale descriptors the walker then reads. `CTR_EL0.DminLine` exists because the value is implementation-defined; `aarch64-qemu-virt` and `aarch64-rpi5` are different CPUs, so a constant that holds for one is not evidence for the other. The field is 4 bits wide, so the derived stride is at least 4 bytes and the clean loop always advances.
- Rejected alternative: the 64-byte constant the first implementation used, which was an unverified assumption on the boot-critical path.

- Decision: Disable the MMU before installing the new translation configuration.
- Rationale: UEFI on AArch64 hands off with the MMU *already enabled*, unlike x86 UEFI where stage-0 simply replaces CR3. `TCR_EL1`/`TTBR*_EL1` cannot be reconfigured while translation is live. The tables must also be cleaned to the point of coherency first, since they were written through a cacheable mapping and the walker reads memory directly once the cache is off.
- Rejected alternative: writing the registers with translation live, which hangs with no diagnostic.

- Decision: Keep the identity window executable at EL1 while the direct map is not.
- Rationale: stage-0 executes from the identity map, and the instruction that re-enables the MMU fetches its successor through those descriptors. Marking them `PXN` kills the CPU at exactly that instruction with no output. The direct map is data only, so it stays non-executable.
- Rejected alternative: a uniform non-executable mapping, which is what the first attempt did and why it failed.

- Decision: Treat an absent framebuffer as a valid configuration rather than a boot failure.
- Rationale: the `virt` machine booted headless has no GOP, and serial is the diagnostic channel that must always exist. A framebuffer that is present but undescribable still fails closed.
- Rejected alternative: requiring a framebuffer, which would have made the gate depend on a display the profile does not need.

## Open risks and follow-ups

- [ ] P2.2 must install exception vectors before anything can fault, take an interrupt, or issue `svc`. Until then `cpu::breakpoint()` escalates rather than returning, and `enable_interrupts` has no vector to deliver to. Both say so at their definitions.
- [ ] The AArch64 kernel requires `-Z build-std` because the precompiled `aarch64-unknown-none` sysroot is non-PIC. This is a nightly-only unstable flag on the critical path; it is passed per-invocation because `[unstable]` in `.cargo/config.toml` has no per-target form and applying it to x86 breaks that build with a duplicate-lang-item error.
- [ ] The PL011 base, RAM base, and machine parameters are pinned constants, not device-tree discovered. P2.5 replaces them; `aarch64-rpi5` will require it.
- [ ] `boot::rsdp_address()` carries the platform-description pointer and is named for its x86 ACPI origin. On AArch64 that is a device tree. Renaming it belongs with P2.5.
- [ ] The `time::apic` name is still used for the timer slot on both architectures; P2.4 renames it when the generic timer lands.
- [ ] `KERNEL_HALF_START` in `memory/vmm.rs` describes x86's single-root table layout. AArch64 splits the halves across two roots, so P2.3 must move that split behind `arch::paging` before the first EL0 task exists or `free_user_half` will leak the upper half of every user root. Unreachable today (no AArch64 tasks); recorded as a P2.3 deliverable.
- [ ] The gate does not observe a malformed or unsupported mapping failing with a structured error. `check_translation_support()` runs only on the happy path, and `BootError::UnsupportedTranslation` is never produced. That required check moved to the slice that can exercise it.
- [ ] `PTE_USER`, `PTE_READ_ONLY`, and `PTE_DEVICE` are unexercised: no EL0 mapping exists until P2.3 and no kernel device mapping until P2.5. Their `AttrIndx` correspondence with `MAIR_EL1` has no boot-path test.
- [ ] `TCR_EL1.IPS` requests a 40-bit physical address range, verified against `ID_AA64MMFR0_EL1` at boot. A machine advertising less fails with `UnsupportedTranslation` rather than producing tables the MMU refuses.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none retained; the boot transcript is reproduced by `just aarch64_boot_check`, which prints it.
- Serial/debugger/model output: AArch64 evidence observed through `just aarch64_boot_check`; x86 evidence through `just test`, `just product_boot_check`, and `just rollback_check`. No physical-board evidence is claimed.
- Related roadmap item: [`P2.1`](../../roadmap/07-architecture-portability.md#p21--firmware-handoff-el1-entry-and-translation-tables).
