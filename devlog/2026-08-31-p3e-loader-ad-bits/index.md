# P3.E loader leaves omitted RISC-V accessed and dirty bits

| Field | Value |
|---|---|
| Date | 2026-08-31 |
| Kind | Defect |
| Status | Verified |
| Scope | `deps/rust-sel4/crates/sel4-kernel-loader/add-payload`, `sel4/pins.toml`, RV64 loader images |
| Roadmap | P3.E |
| Gates | `just riscv64_qemu_check`, `just duo_sel4_check` |
| Trigger | The first physical P3.E seL4 campaign reached the loader's `Entering kernel` marker and then emitted no further byte for 180 seconds |
| Baseline | RV64 QEMU accepted the loader's eager Sv39 leaves, but no physical run had exercised those descriptors on a core that faults when `A=0` |

## Summary

The first physical seL4 image reached the kernel loader and stalled at the page-table switch before the load-bearing `SLIME_DUO loader page tables active` marker. The loader emitted valid `V|R|W|X` leaves but left RISC-V `A` and `D` clear. OpenC906 implements the permitted software-maintained access-bit scheme, so the first translated instruction fetch faulted when it encountered `A=0`; QEMU's hardware A/D update had hidden the defect. Rust-seL4 commit `070c6a384a01ce0f6c60e081fddc8c40fb2a6132` now initializes every eager RISC-V leaf with `A=1,D=1`. A rebuilt physical run crossed that exact transition, booted upstream seL4, and entered `slime-root`; it then exposed a separate stopped-RTC defect recorded in [`devlog/2026-09-01-p3e-rtc-counter-start/`](../2026-09-01-p3e-rtc-counter-start/index.md).

## Observable symptom

- Command: `python3 scripts/check/check-duo-sel4.py --no-build --serial /dev/serial/by-id/usb-1a86_USB_Serial-if00-port0 --evidence-dir devlog/2026-08-31-p3e-sel4-milkv-duo`
- Expected: `SLIME_DUO loader page tables active`, upstream seL4 boot, generation admission, and autonomous cold reset.
- Observed: the loader printed `Entering kernel`, then the gate timed out after 180 seconds with no later serial byte.
- Exit/fault/serial evidence: exit 1; [`physical-stall.log`](physical-stall.log).

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | U-Boot hash verification, loader entry, platform decode, payload copy, and the final pre-switch `Entering kernel` marker all appeared | Deployment, FIT integrity, S-mode entry, and payload placement were not the failing boundary |
| 2 | The generated root pointer was `0x800b8000`; branch descriptors pointed to aligned child tables and carried the required XTheadMae memory attributes | Root selection and branch encoding were valid |
| 3 | Every eager leaf ended in `0x0f`, setting `V\|R\|W\|X` but leaving PTE bits 6 and 7 clear | The first translated fetch depended on hardware setting `A` or faulted immediately |
| 4 | RISC-V permits software-maintained A/D bits, and OpenC906 follows that scheme; QEMU had accepted the same pages by updating A/D in hardware | The QEMU reference could pass while physical C906 stopped at the transition |
| 5 | The rebuilt image contains 1,533 non-branch leaves ending in `0xcf`; every leaf has `A\|D` and branch entries remain unchanged | The source fix restores the eager-map invariant without putting reserved bits on non-leaf PTEs |

## Root cause

`RiscVLeafDescriptor::from_level_paddr()` constructed every eager loader mapping with only `V|R|W|X`. That is insufficient on implementations that use the RISC-V software-managed accessed/dirty scheme: an access with `A=0`, or a store with `D=0`, raises a page fault instead of updating the PTE. The loader has no page-fault handler during this transition, so OpenC906 stopped on the first instruction fetch after writing `satp`.

Both identity and higher-half kernel-window mappings share this constructor through `identity_descriptor()` or `descriptor()` and `leaf_descriptor_from_level_paddr()`. The defect was therefore one constructor invariant, not a Duo-only leaf path or an XTheadMae branch problem. Non-leaf descriptors correctly retain only `V` plus their address and memory attributes; A/D are reserved there.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Rust-seL4 RISC-V leaf constructor | Added explicit `set_accessed(true)` and `set_dirty(true)` to every eager R/W/X leaf | A freshly installed loader mapping is immediately executable, readable, and writable on both hardware- and software-managed A/D implementations |
| Loader unit test | Asserts the low bits equal `0xcf` for an eager RISC-V leaf | A future constructor change cannot silently drop `V\|R\|W\|X\|A\|D` |
| Source pin | Advanced the pinned Rust-seL4 fork to `070c6a384a01ce0f6c60e081fddc8c40fb2a6132` | Every built RV64 image identifies and consumes the fixed loader source |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Shared RV64 loader behavior regresses | `just riscv64_qemu_check` | Any root, sample, generation, rollback, or portability sub-gate fails after rebuilding from the pinned loader commit |
| Physical C906 still faults at the page-table switch | `just duo_sel4_check /dev/serial/by-id/usb-1a86_USB_Serial-if00-port0` | `SLIME_DUO loader page tables active` is absent or reordered after `Entering kernel` |
| A/D initialization is removed locally | `cargo test -p sel4-kernel-loader-add-payload` in `deps/rust-sel4` | `riscv_eager_leaf_is_accessed_and_dirty` fails |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Physical pre-fix `check-duo-sel4.py --no-build ...` | Exit 1 after `Entering kernel`; no post-switch marker within 180 seconds | Direct; [`physical-stall.log`](physical-stall.log) |
| Rebuilt table decode | Root `0x800b8000`; 1,533 eager leaves; first leaf `0x7000001fe00000cf`; every leaf has `A\|D` | Direct; [`rebuilt-pte-inspection.txt`](rebuilt-pte-inspection.txt) |
| `cargo test -p sel4-kernel-loader-add-payload` | 4/4 tests passed, including the new A/D invariant and existing XTheadMae leaf/branch tests | Direct |
| `cargo fmt --all --check` in `deps/rust-sel4` | Passed | Direct |
| `just riscv64_qemu_check` | Passed after rebuilding all RV64 planes from Rust-seL4 commit `070c6a38`; the rebuilt identity records that exact commit | Direct |
| Rebuilt physical campaign | Crossed `SLIME_DUO loader page tables active`, booted upstream seL4, entered `slime-root`, and acquired RTC IRQ 17; the A/D failure no longer reproduces | Direct; [`devlog/2026-09-01-p3e-rtc-counter-start/timer-counter-stall.log`](../2026-09-01-p3e-rtc-counter-start/timer-counter-stall.log) |

## Decisions

- **Decision:** Set both `A` and `D` on every eager R/W/X loader leaf, independent of platform.
- **Rationale:** These mappings are created as immediately usable and mutable; pre-setting both bits is valid under either RISC-V A/D scheme and avoids an unavailable transition-time page-fault path.
- **Rejected alternative:** Add a Duo-only exception or page-fault handler. The constructor invariant is architecture-wide, and special-casing the board would leave the same latent fault on other software-managed implementations.

## Open risks and follow-ups

- [x] The rebuilt image crossed the original page-table-switch boundary on physical OpenC906 hardware.
- [x] The existing QEMU CPU model cannot reproduce software-maintained A/D behavior, so the final P3.E physical campaign supplied the load-bearing qualification and passed.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`physical-stall.log`](physical-stall.log).
- Serial/debugger/model output: [`rebuilt-pte-inspection.txt`](rebuilt-pte-inspection.txt) records the decoded rebuilt page tables.
- Related roadmap item: [P3.E](../../roadmap/07-architecture-portability.md#p3e--sel4-on-the-milk-v-duo).
