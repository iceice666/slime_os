# RP1 — Target-qualified build and admission path

| Field | Value |
|---|---|
| Date | 2026-08-03 |
| Kind | Change |
| Status | Verified |
| Scope | RPi5 target profile, executable-closure admission fixtures, component build-cache isolation, AArch64 syscall ABI, generation and transfer checks |
| Roadmap | RP1 |
| Gates | `just rpi5_artifact_check`, `just x86_portability_check`, `just generation_check`, `just test`, `just product_boot_check`, `just test_host` |
| Trigger | RP0 fixed the exact Raspberry Pi 5 DDS/node contract, while P0/P1 supplied generic target-qualified images and architecture boundaries without an RP1-specific closure gate. |
| Baseline | P0 rejected generic wrong-target kernel/component images, but no gate bound the RP0 DDS runtime and two node artifact names to `aarch64-rpi5`, distinguished the board interrupt profile, or isolated profiles sharing one Cargo target. |

## Summary

RP1 binds the future DDS runtime and two ROS-compatible node executable roles
from the RP0 contract to one exact `aarch64-rpi5` artifact profile before any
executable mapping. The focused gate builds deterministic authenticated closure
fixtures through the production generation/release encoders, rejects x86 and
same-ISA QEMU kernels and every named DDS/node component, proves target-specific
executable and complete-generation identities while preserving neutral resource
identity, and prevents AArch64 QEMU and Raspberry Pi 5 component builds from
sharing one Cargo output directory. The target profile now distinguishes the
RPi5 GICv2 board contract from QEMU's GICv3 contract.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Target profile | Added a distinct AArch64 GICv2 required-feature bit for `aarch64-rpi5`; QEMU retains GICv3. Generated Rust/Python bindings carry the new exact mask. | Two AArch64 profiles cannot agree on serialized executable qualification while requiring incompatible interrupt-controller contracts. |
| RP1 gate | Added `rpi5_artifact_check`, deriving the DDS runtime and publisher/subscriber artifact names from the RP0 fixture and constructing their target-qualified component closure with production generation/release encoders. | The frozen demo artifact roles are admitted only under the exact RPi5 profile; x86 and QEMU substitutions fail closed. |
| Identity and determinism | The gate compares two normalized RPi5 generations and boot stores, asserts target changes alter each executable object plus release/generation identity, and confirms neutral resource bytes and digests remain identical. | Target-specific executable identity does not leak into architecture-neutral resources, and normalized builds remain reproducible. |
| Build isolation | Component Cargo target directories now include the exact profile name; transfer cleanup uses the same canonical path helper. | Profiles sharing `aarch64-unknown-none` cannot reuse or delete each other's component outputs. |
| ABI documentation | Clarified the AArch64 `svc` memory boundary, NZCV behavior, normative general-register preservation, and the currently unsupported SIMD/FP preservation contract. | The userspace runtime ABI is explicit without claiming P2.2/P2.3 execution evidence. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Wrong-architecture or same-ISA artifacts enter an RPi5 generation | `just rpi5_artifact_check` | Kernel or named DDS/node component mismatch is admitted, or the expected structured target-profile rejection changes. |
| RPi5 and QEMU executable qualification collapses to the same board contract | `just rpi5_artifact_check`, `just architecture_contract_check` | GICv2/GICv3 masks drift, same-ISA profile separation disappears, or RP0 board metadata disagrees with the target profile. |
| Target-specific output becomes nondeterministic or rewrites neutral resources | `just rpi5_artifact_check`, `just generation_check` | Repeated generation/boot-store bytes differ, executable digests do not change with target, or neutral resource identity changes. |
| Profile build caches collide | `just rpi5_artifact_check` | QEMU and RPi5 component target paths share a profile directory. |
| Existing x86 execution or architecture boundary regresses | `just test`, `just product_boot_check`, `just x86_portability_check` | QEMU tests, product boot, source allowlist, or AArch64 neutral cross-build fails. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just rpi5_artifact_check` | Passed; RP0 names resolved, exact RPi5 GICv2 profile matched, x86 and QEMU kernel plus every DDS/node component substitution was rejected, deterministic generation/boot-store bytes matched, and target/neutral identities behaved as declared. | Direct |
| `just x86_portability_check` | Passed; 188 neutral Rust files scanned and the neutral kernel/runtime cross-built for AArch64. | Direct |
| `just generation_check` | Passed; repeated x86 generation and boot-store builds remained byte-identical and admitted. | Direct |
| `just test` | Passed; kernel QEMU unit and integration corpus completed. | Direct |
| `just product_boot_check` | Passed; the healthy product vertical slice remained at 45 capability slots without verification scaffolding. | Direct |
| `just test_host` | Passed; boot-contracts and slime-proto host tests completed. | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos`, `just devlog_check` | Passed after formatting and adding the architectural `AttrIndx` spelling to the repository dictionary. | Direct |
| Independent review | Initial review found profile-feature drift, stale transfer-cache cleanup, ABI overstatement, and magic generation offsets. All were fixed; follow-up verdict was approved with no remaining introduced issue. | Direct |

## Decisions

- Decision: Treat the RP0 DDS runtime and two node names as executable closure roles in RP1, without implementing DDS or node behavior.
- Rationale: RP1 owns target-qualified build/admission; RP5/RP6 own the runtime and node implementations. The gate must prove any future executable under those names traverses the existing component-image admission path, not claim those later behaviors exist.
- Rejected alternative: Add placeholder DDS/node binaries, which would violate the no-stub rule and blur RP1 with RP5/RP6.

- Decision: Encode the RPi5 GICv2 requirement as a distinct executable feature bit.
- Rationale: Exact profile ids already separate machines, but the serialized qualification must not claim QEMU's GICv3 feature for a board contract that requires GIC-400/GICv2.
- Rejected alternative: Keep identical feature masks and rely only on profile id, which would preserve rejection while leaving the declared feature set false.

## Open risks and follow-ups

- [ ] RP3 must bind the remaining RP0 firmware, device-tree, media, board-revision, and physical boot evidence to the `aarch64-rpi5` image path; RP1 proves artifact admission, not a board boot.
- [ ] RP5/RP6 must implement the DDS runtime and node executables under the exact RP0 names and package them as ordinary target-qualified generation components; no DDS/ROS execution is claimed here.
- [ ] RP2/P2.2/P2.3 must execute and observe the documented AArch64 syscall preservation rules.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none retained.
- Serial/debugger/model output: x86 QEMU evidence was observed through `just test` and `just product_boot_check`; RP1's RPi5 evidence is host-side artifact admission only, with no physical-board or AArch64 component execution claim.
- Related roadmap item: [`RP1`](../../roadmap/09-rpi5-ros2-demo.md#rp1--target-qualified-build-and-admission-path).
