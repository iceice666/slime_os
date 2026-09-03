# P6.1: x86-64 seL4 builds reproducibly and admits nothing else

| Field | Value |
|---|---|
| Date | 2026-09-02 |
| Kind | Change |
| Status | Verified |
| Scope | Target-profile contract, pc99 seL4 kernel profile and pins, repo-owned x86-64 Rust target specifications, seL4/generation/C-component build paths, `slime-root` and component-runtime x86-64 arms, and the architecture-portability boundary |
| Roadmap | P6.1 |
| Gates | `just x86_64_sel4_image_check`, `just architecture_contract_check`, `just lint_sel4_root_x86_64` |
| Trigger | P6 became the active architecture lane after the Milk-V Duo lane closed, and P6.1 is its first slice |
| Baseline | The product built for AArch64 and RV64 seL4 only; x86-64 retained the retired custom kernel's `x86_64-qemu-virtio` profile for rollback decoding and had no current seL4 target, kernel, or build path |

## Summary

The repository now reproducibly builds an admitted x86-64 seL4 kernel, root task,
child fixture, and generation for one pinned QEMU pc99 profile, and refuses every
mis-qualified executable before mapping. `x86_64-sel4-qemu-pc99` and the exact
`x86_64-sel4-framework13-ai300` profiles are declared behind a distinct
`SLIME_X86_64_SEL4_V1` ABI, so neither can be confused with the retired trap ABI
that shares their architecture and page profile. This makes **no boot claim**: the
platform is on seL4 pc99's native Multiboot2 route, so it produces no packaged
image at all and its identity manifest says so. P6.2 owns the GRUB file tree, the
OVMF pin, and the first boot; P6.3 owns proving the x86-64 arms landed here
actually behave.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Target contract | Added `SLIME_X86_64_SEL4_V1` (ABI 6), `X86_64_SEL4` (feature 256), and profiles 8/9 to `contracts/target-profile/v1/schema.zt`; regenerated bindings with `just boot_gen` | An x86-64 seL4 executable and a retained custom-kernel one differ by ABI, feature set, and profile id rather than being interchangeable on a shared architecture |
| Executable admission | Removed x86-64's `current()` default of `x86_64-qemu-virtio` in `boot-contracts/src/target_profile.rs` | x86-64 now has three profiles, so an unset `SLIME_TARGET_PROFILE` must fail closed instead of silently qualifying a new seL4 image against the retired ABI |
| seL4 kernel profile | Added `sel4/config/qemu-pc99.cmake` deriving from the pinned `X64_verified.cmake`, plus `[qemu_pc99]` and `[observed_prefix_qemu_pc99]` in `sel4/pins.toml` | A kernel bump moves the inherited settings instead of leaving a hand-copied table stale, and the product-required overrides are stated where a reader and the pin checker both see them |
| Rust target specifications | Added repo-owned `sel4/targets/x86_64-sel4-{roottask-,}minimal.json`, pinned against the rust-sel4 originals they derive from | The upstream pair's `-sse,-sse2` plus `rustc-abi = "softfloat"` has no LLVM lowering for the 128-bit integer arithmetic release-signature verification performs; both halves of the derivation are hashed so neither copy nor original can drift silently |
| Build paths | `Platform` gained `emulated`, `boot_route`, `child_target_key`, and an optional `loader_target_key`; the loader and packaging steps are skipped on the Multiboot2 route; `write_manifest` omits fields the platform does not produce | The identity manifest asserts only about artifacts that exist — no fabricated device tree, platform description, loader, or image |
| Root and component runtime | Added x86-64 arms for IOAPIC interrupt acquisition, page-fault access decoding, an HPET monotonic source, `fs_base` thread indexing, and ELF architecture admission; introduced `slime-root/src/{vm_attributes,irq_control}.rs` as single owners for the two things that genuinely differ per architecture | Seven `frame_map` call sites and two IRQ acquisitions no longer each carry their own architecture conditional, so a new profile cannot be added to some of them and missed in others |
| Freestanding C components | Added `start-x86_64.S`, `component-x86_64.ld`, and an x86-64 entry in `build-c-component.py`; replaced the runtime's `#else`-defaulted spin instruction with an explicitly selected `SLIME_SPIN_HINT` | The previous `#else` silently emitted AArch64's `yield` on x86-64; an unhandled architecture is now a compile error rather than an assembler failure far from its cause |
| Portability boundary | Rewrote `check-architecture-portability.py` to forbid privileged x86 mechanism by enumerated vocabulary — ring-0 and port instructions, control/debug/GP registers, relocation and ELF constants — instead of the bare architecture name, and to match code rather than comments | P6 extends P1's boundary for the admitted seL4 x86-64 path. A `cfg(target_arch)` arm is the boundary working; `cli`, `lgdt`, `%cr4`, or `R_X86_64_*` in a neutral tree is not |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| An executable qualified for another architecture, ABI, page profile, machine profile, or feature set is admitted | `just x86_64_sel4_image_check` | The gate reports which axis was accepted; the underlying `boot-contracts` test asserts a *distinct* error per axis, so a single generic refusal fails it |
| The build stops being byte-reproducible | `just x86_64_sel4_image_check` | Two normalized full builds are compared across kernel, root, child, C component, generation, and identity; the failure names each differing artifact and both digests |
| The pc99 kernel profile, machine pin, or CPU model drifts | `just sel4_pin_check` | The profile comparison names each disagreeing CMake entry; the CPU check asks the installed QEMU to expand the pinned model and fails if it does not report `fsgsbase`, which the kernel's `KernelFSGSBase "inst"` boot path requires |
| A repo-owned x86-64 target specification drifts from its upstream original | `just sel4_pin_check` | `check_x86_64_target_derivation` names the differing keys; only `features` and the absence of `rustc-abi` are the admitted delta |
| Privileged x86 mechanism reappears in an architecture-neutral tree | `just x86_portability_check` | The offending file, line, and source text are printed; 23 fault-injected privileged constructs were each observed to fail the gate |
| The x86-64 arms stop compiling or gain lints | `just lint_sel4_root_x86_64` (in `lint_all`) | `lint_all` previously compiled only AArch64, so these arms were unlinted; clippy runs with `-D warnings` against both x86-64 target specifications |
| A retained profile's identity collides with the new ones | `just architecture_contract_check` | Duplicate ids or names, and any profile absent from the expected set, fail |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just x86_64_sel4_image_check` | Pass: 6 embedded executables carry `x86_64-sel4-qemu-pc99`; wrong architecture, ABI, page profile, machine profile, and feature set each refused with a distinct error; two normalized full builds byte-identical across child, component, generation, identity, kernel, root | Direct |
| Built artifacts | `kernel.elf` `18e676b8…` (1225752 bytes), root `476d0a50…` (678704), child `fc34cb7f…` (31688), generation identity `c521a98d…` (275852); all three ELFs report `ELF 64-bit LSB executable, x86-64` | Direct |
| Identity manifest | Records `boot_route: multiboot2`, no `image`, no `elf.loader`, no `elf.payload_tool`, no `config.dtb`, no `config.platform_info` — the pc99 prefix installs no `support/` directory | Direct |
| `cargo test -p boot-contracts --all-features` | Pass: 336 tests, including the new `an_x86_64_sel4_payload_refuses_every_wrong_qualification` | Direct |
| `slime-root` host tests against the pc99 prefix | Pass: 214/214. `just test_sel4_root` itself cannot run on this x86-64 host — bindgen cannot parse the AArch64 prefix's inline-asm register names — which is pre-existing and unrelated to this change; CI runs that target on arm64. The pc99 prefix makes the same suite reachable here | Direct |
| `just sel4_root_boot_check` | Pass: ordered generation, timer, task, IPC, fault, and ready markers on `qemu-arm-virt`, after the fixture-probe changes | Direct |
| `just sel4_qemu_image_check`, `just riscv64_qemu_image_check` | Pass: both retained profiles still build and package their own images | Direct |
| `just architecture_contract_check` | Pass, with 9 declared profiles | Direct |
| `just x86_portability_check` | Pass over 225 neutral Rust files. Fault-injected 23 privileged constructs — `cli`, `sti`, `hlt`, `lgdt`, `lidt`, `inb`, `outb`, `rdmsr`, `wrmsr`, `xsetbv`, `swapgs`, `invlpg`, `wbinvd`, `vmxon`, `iretq`, `sysretq`, `%cr4`, `%dr7`, `%r15`, `EFER`, `R_X86_64_64`, `qemu-system-x86_64`, `0x8664` — and observed each one fail the gate | Direct |
| `just lint_all` (now including `lint_sel4_root_x86_64`), `just fmt_check_all` | Pass | Direct |
| `just test_host`, `just sel4_pin_check`, `just ruff`, `just typos` | Pass | Direct |

## Decisions

- Decision: Own the two x86-64 Rust target specifications in `sel4/targets/` rather than using rust-sel4's.
- Rationale: Upstream pairs `-sse,-sse2` with `rustc-abi = "softfloat"`, which LLVM cannot lower the 128-bit integer arithmetic `curve25519-dalek` performs; `slime-root` links it through `boot-contracts`' `release-crypto` feature, so the root fails in codegen. Hardware float is also the *correct* configuration here: seL4 pc99 sets `CONFIG_HAVE_FPU` and saves x87/SSE state per thread through `XSAVE`, unlike the AArch64 and RV64 kernels which export no FP context. Both the copy and the original are hashed, and the checker asserts the exact one-field delta.
- Rejected alternative: Drop `release-crypto` from the x86-64 root. That would remove release-signature verification from one profile only, making its generation admission weaker than every other platform's for a toolchain reason.

- Decision: Model pc99 as a distinct `boot_route` rather than as another loader platform.
- Rationale: The pinned rust-sel4 kernel loader implements Arm and RISC-V assembly only, and seL4 pc99 already consumes Multiboot modules. Making the route explicit lets the build skip the loader and packaging steps and lets the identity manifest omit an image that does not exist, instead of recording placeholder fields a later gate would trust.
- Rejected alternative: Add x86-64 assembly to the vendored loader. That would fork a pinned dependency to reimplement a boot contract the kernel already provides.

- Decision: State the x86-64 W^X non-equivalence and make the affected probe absent rather than passing.
- Rationale: `seL4_X86_VMAttributes` is a cache-policy selector with no execute bit — the kernel's `makeUserPTE` never sets NX — so a data mapping is executable on this profile. The fixture's execute probe cannot fault, and had it been left in place the branch would have *succeeded* into arbitrary data. `vm_attributes.rs` owns the statement, the probe is compiled out, the expected probe count drops to one, an inverse assertion fires if an execute fault ever appears, and the phase prints `wx_execute=unenforced`.
- Rejected alternative: Keep the probe and treat its non-fault as a pass. That would report an unenforced mapping as an enforced one.

- Decision: Redefine the architecture-portability gate by privileged vocabulary instead of relaxing it.
- Rationale: The gate banned the token `x86_64` outright, which was right while x86 was retired but would reject the `cfg(target_arch = "x86_64")` arms P6.1 admits — the same arms `aarch64` and `riscv64` already have. Simply deleting the token would have left a near-empty regex; the enumerated instruction and register vocabulary is what keeps it a real boundary, and the fault-injection sweep is what shows it.
- Rejected alternative: Add the new files to an exemption list. An allowlist grows silently and would exempt future privileged mechanism in the same files.

## Open risks and follow-ups

- [ ] The x86-64 root arms are compiled and linted but never executed: no gate boots this profile, by P6.1's design. P6.2 boots it; P6.3 owns proving the IOAPIC acquisition, HPET timer, fault decoding, and `fs_base` thread pointer behave. Every constant they depend on was read from the pinned seL4 source or the IA-PC HPET specification rather than observed running. **[INFERENCE]** until then.
- [ ] The HPET base address `0xfed00000` and IOAPIC pin 2 are pinned QEMU q35 facts. A physical Framework reports its own HPET base in the ACPI HPET table and its own interrupt routing in the MADT; H1 owns discovering both, and the `x86_64-sel4-framework13-ai300` profile has no gate until P6.5.
- [ ] `just test_sel4_root` cannot run on an x86-64 host because bindgen cannot parse the AArch64 prefix's inline-asm register names. Pre-existing and out of scope here, but the pc99 prefix now makes the same suite reachable on this host, so a host-agnostic variant is possible.
- [ ] OVMF is not pinned. P6.1 launches no emulator, so there is nothing for a firmware hash to bind; P6.2 boots QEMU/OVMF and pins it there.
- [ ] The SDK profile table (`scripts/lib/component_sdk.py`) gained no x86-64 row. CP8/CP9 would require a published prefix archive and compatibility evidence for a profile with no boot gate, so it is deliberately deferred rather than exported unproven.

## Artifacts and provenance

- Focused report: none; this entry is the record.
- Raw transcript: none retained.
- Serial/debugger/model output: none — P6.1 makes no boot claim and ran no emulator.
- Related roadmap item: [`roadmap/07-architecture-portability.md` P6.1](../../roadmap/07-architecture-portability.md#p61--x86-64-sel4-target-and-reproducible-pc99-kernel)
- Preceding decision: [`devlog/2026-09-02-p6-amd64-sel4-return/`](../2026-09-02-p6-amd64-sel4-return/index.md)
- Derivation rationale for the repo-owned target specifications: [`sel4/targets/README.md`](../../sel4/targets/README.md)
