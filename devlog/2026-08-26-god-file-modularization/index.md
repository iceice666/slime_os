# God-file modularization by runtime ownership

| Field | Value |
|---|---|
| Date | 2026-08-26 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root` boot/runtime and fixture orchestration, init fabric composition, generation resource and fabric-profile construction, host and QEMU verification |
| Roadmap | none |
| Gates | `just test_sel4_root`, `just sel4_root_boot_check`, `just generation_check` |
| Trigger | The largest handwritten source files mixed unrelated policy, dispatch, platform, and orchestration responsibilities. |
| Baseline | Product behavior and serialized generation output were already correct; this change had to preserve them while establishing module ownership boundaries. |

## Summary

The largest handwritten source files were split along existing ownership boundaries without changing the generation format, syscall ABI, capability model, or boot behavior. Root startup delegates graph execution to `graph_runtime` and fixture request/fault adjudication to `fixture_runtime`; platform probing, console dispatch, and graph-global state have dedicated modules; service mechanisms are separated into spawn, policy, and capability/buffer modules. Init separates authenticated action dispatch and fabric-plane composition from its remaining plane drivers. Generation resource encoding and fabric-profile resolution are isolated from final image assembly. All affected host, contract, generation, lint, and QEMU boot gates passed.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Root entry point | Moved product graph runtime out of `slime-root/src/main.rs` into `slime-root/src/graph_runtime.rs`. | `main.rs` owns staged boot orchestration rather than every runtime mechanism. |
| Platform and console | Added `graph_runtime/platform.rs` and `graph_runtime/console_runtime.rs`. | Device probing and console dispatch each have one explicit owner. |
| Root services | Added `graph_runtime/services.rs` with `services/spawn.rs`, `services/policy.rs`, and `services/capability.rs`. | Spawn/lifecycle, policy/status, and capability/shared-buffer dispatch no longer share one flat implementation file. |
| Root fixtures | Added `slime-root/src/fixture_runtime.rs` for fixture IPC, supervised fault classification, shared-region setup, and phase adjudication. | Root startup stages resources and delegates the fixture protocol instead of owning both. |
| Graph state | Added `slime-root/src/graph_runtime/state.rs` for generation-global catalogues, ELF staging, capability-export state, and quota lookup. | Graph orchestration no longer defines mutable mechanism state inline. |
| Init | Added `components/bins/init/src/dispatch.rs` and `components/bins/init/src/fabric_planes.rs`. | Authenticated action selection and C8/C9 fabric composition are separate from component entry and unrelated plane drivers. |
| Generation builder | Added `scripts/build/generation_resources.py` and `scripts/build/generation_fabric.py`. | Resource encoding and fabric-profile validation/encoding are separate from manifest admission and final image assembly. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Module visibility or callsite migration breaks root code | `just test_sel4_root` | Root crate fails to compile or any of the 183 host tests fails. |
| Refactoring changes actual root startup behavior | `just sel4_root_boot_check` | Ordered generation, timer, task, IPC, fault, or ready markers are absent or reordered in QEMU. |
| Builder extraction changes contracts or generation bytes | `just generation_check` | Contract checks, deterministic generation construction, component graph boot, or generation identity checks fail. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test_sel4_root` | Passed: 183/183 tests across 19 modules. | Direct |
| `just sel4_root_boot_check` | Passed: ordered generation, timer, task, IPC, fault, and ready markers observed on pinned `qemu-arm-virt`. | Direct |
| `just sel4_stream_check` | Passed: the 57-marker stream transcript and all declared participants were observed. | Direct |
| `just sel4_boot_check` | Passed: one generation launched every C8 role through the collision-free layout and settled to idle. | Direct |
| `just contracts_check` | Passed: model checks, generated bindings, 280 boot-contract tests, layouts, and ABI checks were current. | Direct |
| `just generation_check` | Passed: deterministic generation and seL4 component-graph checks completed. | Direct |
| `just fmt_check_all` | Passed. | Direct |
| `just ruff` | Passed. | Direct |
| `just lint_all` | Passed with warnings denied. | Direct |

## Decisions

- Decision: split by mechanism ownership and existing call boundaries, not by target file size.
- Rationale: cohesive modules preserve the capability and generation model and keep runtime dependencies visible; arbitrary equal-sized chunks would only relocate the god file.
- Rejected alternative: introduce façade traits or compatibility wrappers around moved functions. Direct module-private calls keep the cutover complete and avoid a second API layer.

## Open risks and follow-ups

- [ ] `scripts/build/build-generation.py` still owns manifest admission, component builds, seL4 plan encoding, final generation assembly, boot-store construction, and CLI orchestration; split these only at a boundary that does not create circular builder callbacks.
- [ ] `slime-root/src/graph_runtime/services.rs` remains the common request loop; avoid further extraction unless it removes a concrete dispatch responsibility rather than reducing line count alone.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none retained.
- Serial/debugger/model output: observed through the verification commands listed above; no sibling capture retained.
- Related roadmap item: none.
