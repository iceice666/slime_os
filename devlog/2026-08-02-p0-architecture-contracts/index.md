# P0 — Architecture, target, and executable-artifact contracts

| Field | Value |
|---|---|
| Date | 2026-08-02 |
| Kind | Change |
| Status | Verified |
| Scope | Target profiles, kernel/component image revisions, generation admission, stage-0 admission, profile-aware builders, syscall ABI documentation |
| Roadmap | P0 |
| Gates | `just architecture_contract_check`, `just generation_check`, `just rollback_check`, `just product_boot_check`, `just test`, `just test_host`, `just fmt_check_all`, `just lint_all` |
| Trigger | P0 implementation |
| Baseline | Executable images and host builders assumed the x86-64 QEMU profile; component headers had no architecture qualification, kernel V1 qualification was implicit, and stage-0 did not bind the generation closure to one exact declared profile before mapping executable bytes. |

## Summary

P0 makes the generation, release, kernel image, and every component executable identify one exact target profile. Versioned Zutai contracts define target profiles and architecture-qualified kernel/component image revisions. Builders select Cargo targets, linker inputs, artifact paths, ELF machines, load layouts, and image qualification from that profile. Stage-0 and kernel activation paths reject unknown or mismatched profiles, architectures, ABIs, page profiles, required feature sets, and executable revisions before mapping or activation. Retained V1 images keep their exact legacy x86-64 meaning. The x86 QEMU regression, deterministic generation, product boot, host contract suite, and repository static gates pass; this milestone does not claim an AArch64 kernel boot.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Target contract | Added a versioned Zutai target-profile table for x86-64 QEMU, AArch64 QEMU, Raspberry Pi 5, and deferred RV64 QEMU profiles, with exact architecture, ABI, page, feature, ELF, load-layout, Cargo, and launch metadata. | A target name resolves to one bounded exact qualification; unknown or nearby names fail closed. |
| Executable formats | Added Zutai kernel-image V2 and component-image V2 headers carrying architecture, ABI, page profile, exact target profile, and required features; generated Rust/Python layouts remain the wire source of truth. | An authenticated executable cannot be reinterpreted for another ISA, ABI, page layout, or same-ISA machine profile. |
| Legacy rollback | Kept bounded decoding for kernel/component V1 and assigned those bytes only the historical `x86_64-qemu-virtio` meaning. | Retained rollback artifacts keep their original semantics instead of becoming architecture-neutral. |
| Admission | Bound stage-0 generation selection, kernel decode, kernel component decode, activation, transfer, recovery, and task spawn to the current exact profile before executable mapping or activation. | The whole executable closure is compatible with the generation and release target at every execution path. |
| Builders | Parameterized Cargo target, profile environment, linker selection, artifact paths, ELF-machine checks, page alignment, preferred/load bases, and emitted headers from the selected profile. | Builders emit only profile-valid images and reject incompatible ELF/layout inputs before packaging. |
| Identity and determinism | Included target-qualified executable bytes in authenticated object, generation, release, and boot-store identities while leaving architecture-neutral resource payloads unchanged. | Equal normalized target inputs reproduce exactly; changing the target changes executable and complete-generation identity without rewriting neutral resources. |
| ABI documentation | Documented the shared semantic syscall contract and architecture-specific x86-64, AArch64, and RV64 calling-convention status. | ISA entry mechanics cannot silently change syscall numbers, errors, rights checks, bounds, or transfer semantics. |
| Verification | Added `architecture_contract_check` with generated-binding freshness, exact-profile rejection, legacy meaning, malformed/future revision rejection, cross-profile checks, and manifest/build admission coverage. | Contract drift or permissive target fallback fails the focused milestone gate. |
| Review hardening | Kept pending-attempt persistence ahead of untrusted generation/release admission, added a corrupt-release QEMU rollback case, exercised same-ISA rejection through the real component checker, and added deterministic AArch64 component rustflags. | A malformed pending closure drains its bounded retry window instead of boot-looping forever; profile checks and cross-target builds remain executable rather than nominal assertions. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Unknown, wrong-ISA, wrong-ABI, wrong-page, wrong-feature, or same-ISA/wrong-profile executable admission | `just architecture_contract_check` | Target-profile, kernel-image, component-image, stage-0, or generated-contract rejection test fails. |
| Target/profile changes fail to affect authenticated output, or neutral resources drift | `just generation_check` | Repeated builds differ, target identity is not exact, or generation/resource validation fails. |
| A pending release fails verification before userspace and traps rollback forever | `just rollback_check` | Corrupt-release QEMU fixture does not durably drain attempts to zero. |
| Legacy x86 boot or product generation regresses | `just test`, `just product_boot_check` | QEMU semantic corpus or healthy product vertical slice fails. |
| Host decoder or protocol regression | `just test_host` | Boot-contract or slime-proto unit suite fails. |
| Rust formatting or warning regression | `just fmt_check_all`, `just lint_all` | Formatter diff or denied Clippy warning. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just architecture_contract_check` | Passed; contract/binding freshness, target-profile admission, legacy/future image rejection, generation manifest validation, and exact cross-profile checks completed. | Direct |
| `just generation_check` | Passed; two normalized x86 target builds were byte-identical and the generated boot store admitted the exact target-qualified closure. | Direct |
| `just rollback_check` | Passed; a deliberately corrupted pending release failed in stage-0 and durably drained the pending window to zero, then the existing runtime-unhealthy case returned to known-good. | Direct |
| `just product_boot_check` | Passed; the product generation reached the healthy QEMU vertical slice with 45 declared capability slots and no verification scaffolding. | Direct |
| `just test` | Passed; 7 kernel QEMU behavioral tests completed. | Direct |
| `just test_host` | Passed; boot-contracts and slime-proto host suites completed. | Direct |
| `just fmt_check_all` | Passed. | Direct |
| `just lint_all` | Passed with warnings denied. | Direct |
| `just ruff` and `just typos` | Passed. | Direct |
| `just deny` and `just machete` | Passed; cargo-deny reported only unmatched configured allowances and cargo-machete found no unused dependencies. | Direct |
| `just miri` | Passed for host-testable boot-contracts and slime-proto crates. | Direct |
| `SLIME_TARGET_PROFILE=aarch64-qemu-virt cargo check … --target aarch64-unknown-none` | Not completed: the installed Rust toolchain lacks `core` for `aarch64-unknown-none`. The P0 gate instead exercises AArch64 qualification with generated profiles and synthetic ELF/image fixtures; no AArch64 runtime claim is made. | Direct |

## Decisions

- Decision: Make an exact target profile id a required image qualification in addition to architecture, ABI, page profile, and feature fields.
- Rationale: AArch64 QEMU and Raspberry Pi 5 share an ISA and ABI but must reject each other's executable closure and load policy.
- Rejected alternative: Infer a target from architecture or ABI, which would admit same-ISA artifacts for the wrong machine profile.
- Decision: Preserve V1 kernel and component bytes as exact legacy x86-64 QEMU artifacts.
- Rationale: This retains the rollback window without reinterpreting historical bytes or adding an ambient architecture default.
- Rejected alternative: Treat V1 as architecture-neutral or drop it immediately; the former is unsafe and the latter breaks declared rollback compatibility.
- Decision: Keep syscall semantics shared while documenting architecture entry conventions separately.
- Rationale: Portability changes register/trap mechanics, not capability checks, bounds, errors, or transfer semantics.
- Rejected alternative: Fork syscall tables per ISA, which would create semantic drift before architecture bring-up.

## Open risks and follow-ups

- [ ] P1 must extract the existing x86-64 architecture boundary without changing its observed behavior.
- [ ] P2 must install/use the AArch64 Rust target, implement the architecture path, and observe the first `aarch64-qemu-virt` QEMU boot; P0 proves artifact qualification, not execution on Arm.
- [ ] RP1 consumes P0/P1 to target-qualify the DDS runtime and ROS node executable closure before the RPi5 demo path can advance.
- [ ] Physical Raspberry Pi 5 launch remains unproven until P4/RP3 and later physical evidence gates.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none retained.
- Serial/debugger/model output: x86 QEMU evidence was observed through `just test` and `just product_boot_check`; no AArch64 or physical-board serial evidence is claimed.
- Related roadmap item: [`P0`](../../roadmap/07-architecture-portability.md#p0-architecture-target-and-executable-artifact-contracts).
