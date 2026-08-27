# Language-neutral component ABI and freestanding C proof

| Field | Value |
|---|---|
| Date | 2026-08-27 |
| Kind | Change |
| Status | Verified |
| Scope | Component startup ABI, root/runtime constants, external component admission, C runtime support, C probe generation, QEMU gate |
| Roadmap | none |
| Gates | `just sel4_c_runtime_check`, `just contracts_check` |
| Trigger | Replacing Dango with a Lisp shell requires a non-Rust implementation path that obeys the same capability and lifecycle boundary as Rust components. |
| Baseline | Component images were ELF and therefore language-neutral in principle, but startup slots, transfer descriptors, and console labels were exposed only through Rust code and no non-Rust component booted through the product graph. |

## Summary

The component boundary is now generated from one Zutai contract for Rust and C consumers. A freestanding AArch64 C runtime enters at `_start`, uses the generated C slot, descriptor, and message-label constants, writes through the granted console endpoint, exits through the root lifecycle endpoint, and is admitted as an external implementation rather than a Cargo package. The dedicated seL4 generation boots this component under QEMU and reaches a healthy terminal graph, establishing a concrete bootstrap path for a non-Rust Lisp runtime.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| ABI contract | Added `contracts/component-runtime-abi/v1/schema.zt` and generated Rust/C bindings for startup geometry, CSpace slots, transfer descriptors, native capability regions, and console labels. | Every persisted or cross-process component ABI fact has one versioned Zutai source of truth. |
| Rust consumers | Replaced duplicated runtime and root constants with `boot_contracts::component_runtime_abi` values and compile-time geometry assertions. | Rust root and component runtime cannot silently disagree with non-Rust bindings. |
| C runtime | Added a freestanding AArch64 entry, linker layout, register-only seL4 calls, console write, and lifecycle exit support. | A non-Rust component needs no Rust runtime, allocator, TLS setup, or ambient service discovery. |
| External admission | Extended generation and image builders with explicit component-spec roots and `implementation-name=ELF` mappings. | External code enters only when its spec, digest, target, and named artifact all agree; missing and unused mappings fail closed. |
| Behavioral proof | Added `sel4-c-runtime.zti`, a minimal C probe, and `sel4_c_runtime_check`. | Language neutrality is observed through the real loader, root task, capability graph, console endpoint, lifecycle exit, and QEMU boot. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Generated Rust and C consumers drift from the Zutai runtime ABI. | `just contracts_check` | Binding generation check or contract corpus fails. |
| An external ELF cannot be admitted, launched, call its granted endpoint, or terminate cleanly. | `just sel4_c_runtime_check` | Build/admission fails, the C ready marker is absent, a fault appears, or the healthy terminal graph marker is absent. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_c_runtime_check` | Passed: the external freestanding C ELF entered through the generated component ABI, wrote through the console endpoint, and exited cleanly. | Direct |
| `just test_host` | Passed boot-contract and protocol host suites. | Direct |
| `just contracts_check` | Passed after external-only manifests were separated from the workspace-build corpus. | Direct |
| `just fmt_check_all` | Passed. | Direct |
| `just lint_all` | Passed with warnings denied. | Direct |
| `just ruff` | Passed. | Direct |

## Decisions

- Decision: Use C as the first non-Rust component proof and as the bootstrap implementation language for the future Lisp runtime.
- Rationale: C exposes the actual AArch64/seL4 ABI without another managed runtime, while remaining a practical implementation language for a compact interpreter.
- Rejected alternative: Implement the Lisp interpreter in Rust first and claim language neutrality from the ELF loader alone; that would not exercise generated foreign-language bindings or foreign startup code.
- Decision: Keep external-only generation manifests selectable but outside `SEL4_MANIFESTS`, the corpus whose members must build from workspace packages without extra inputs.
- Rationale: External implementations require explicit ELF and component-spec inputs; treating them as ordinary workspace manifests makes unrelated contract checks fail for a package that intentionally does not exist.
- Rejected alternative: Add a dummy Cargo package or fallback ELF path; either would weaken the non-Rust proof or make admission depend on implicit host state.

## Open risks and follow-ups

- [ ] The C runtime currently proves startup, inline console IPC, and lifecycle exit only; the Lisp shell will need generated C helpers for transfer-window payloads, endpoint calls, input, and spawn without duplicating wire layouts.
- [ ] The host compiler is a Nix-wrapped Clang that emits harmless cross-target and macOS minimum-version warnings; the produced freestanding ELF boots correctly, but a dedicated cross-toolchain would remove that noise.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none.
- Serial/debugger/model output: observed through `just sel4_c_runtime_check`; no sibling capture retained.
- Related roadmap item: none.
