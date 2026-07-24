# Kernel layout and generated bindings cleanup

| Field | Value |
|---|---|
| Date | 2026-07-24 |
| Status | Verified |
| Scope | `kernel/src`, shared Zutai Rust bindings, contract generators |
| Trigger | Kernel implementation files and generated bindings were interleaved in one source root |
| Baseline | `kernel/src` had 51 top-level Rust files and four checked-in `gen.rs` binding copies |

## Summary

The kernel crate exposed a flat source tree that mixed architecture code, drivers, runtime policy, storage mechanisms, protocols, and support utilities. Four Zutai-generated protocol surfaces were also duplicated under `kernel/src`. The implementation is now grouped by subsystem while preserving the existing public module paths, and the kernel consumes the single generated `slime-proto` modules shared with userspace. The focused QEMU test, contract checks, formatting, and lint checks pass.

## Observable symptom

- Command: inspect `kernel/src` and tracked generated files.
- Expected: subsystem boundaries are visible; one checked-in Rust binding surface per protocol.
- Observed: 51 top-level Rust files and duplicate kernel `gen.rs` outputs for block, component image, generation management, and store protocols.
- Exit/fault/serial evidence: no runtime fault; this was a maintainability and source-of-truth defect.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `kernel/target` is ignored and has no tracked files. | Build output was not the repository defect. |
| 2 | Four `kernel/src/**/gen.rs` files were tracked and generated from the same Zutai schemas as `components/proto`. | Kernel copies were redundant binding surfaces. |
| 3 | Kernel callers already accessed those bindings through hand-written wrapper modules. | Wrappers could switch to `slime-proto` without changing caller APIs. |
| 4 | Architecture, driver, runtime, storage, protocol, and support files had clear dependency clusters. | A directory-only regrouping could preserve behavior and public module names. |

## Root cause

The crate grew milestone by milestone without a stable subsystem directory boundary. Protocol generators wrote one output into the kernel and another into the component protocol crate, creating duplicate checked-in products from one schema. Both patterns increased navigation cost and allowed generated copies to drift independently.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `kernel/src` | Grouped implementation under `arch/x86_64`, `drivers`, `runtime`, `storage`, `protocol`, and `support`; retained crate-root re-exports. | Directory layout communicates ownership without an API migration. |
| Protocol bindings | Removed kernel-local generated block, component, generation-management, and store binding copies; kernel wrappers import `slime-proto`. | One checked-in Rust binding surface per shared protocol. |
| Zutai schemas/generators | Updated output paths and freshness checks to target `components/proto/src`. | Schema generation and `--check` agree with the single source location. |
| Contract references | Updated current contract and roadmap path references for moved kernel files. | Active documentation points at the maintained layout. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Shared bindings drift from Zutai schemas | `just contracts_check` | A generator `--check` reports stale or missing output. |
| Kernel wrapper breaks shared binding use | `cargo test --release --test component_image -- -display none` in `kernel/` | QEMU component image contract test fails. |
| Module regrouping breaks compilation | `just lint` | Clippy compilation or warnings fail. |
| Formatting changes across workspaces | `just fmt_check && just fmt_check_components` | Rustfmt reports a diff. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `cargo test --release --test component_image -- -display none` (`kernel/`) | 13/13 QEMU tests passed | Direct |
| `just contracts_check` | BootState model scenarios, binding freshness checks, and 12 boot-contract tests passed | Direct |
| `just fmt_check && just fmt_check_components` | Passed | Direct |
| `just lint && just lint_components` | Passed with warnings denied | Direct |

## Decisions

- Decision: preserve existing crate-root module names through re-exports while changing physical file ownership.
- Rationale: callers and tests need no broad rename; the directory structure still becomes explicit.
- Rejected alternative: generate bindings only into ignored build output. Clean checkouts and editors would then require generation before compilation, weakening repository reproducibility.

## Open risks and follow-ups

- [ ] `memory`, `time`, `syscall`, `task`, `capability`, and `ipc` remain existing top-level subsystem directories; architecture-specific code inside them should move only as part of the planned architecture-boundary work.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: not retained; exact commands and observed results are listed above.
- Serial/debugger/model output: QEMU component-image test output and BootState model output were observed directly during this change.
- Related roadmap item: `roadmap/07-architecture-portability.md`.
