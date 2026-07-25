# Root workspace and tooling layout

| Field | Value |
|---|---|
| Date | 2026-07-24 |
| Status | Verified |
| Scope | Root Cargo workspace, host tooling layout, LLDB helper, build artifact paths |
| Trigger | Repository root mixed Rust workspaces, host tools, and an ignored local debug script without a common layout |
| Baseline | Kernel, stage-0, components, and boot contracts used separate lockfiles/workspace roots; host scripts were flat under `scripts/` |

## Summary

The Rust projects now share one root Cargo workspace and lockfile while preserving subsystem-specific target configuration: kernel commands still run from `kernel/`, component commands from `components/`, and stage-0 commands from `stage0/`. Host tooling is grouped under `scripts/build/`, `scripts/check/`, `scripts/generate/`, and `scripts/lib/`; the LLDB helper is tracked at `tools/debug/lldb-attach.sh`. Deterministic generation, contracts, formatting, lint, and QEMU suites all pass after the cutover.

## Observable symptom

- Command: `cd kernel && cargo build --release -p slime_os-kernel`
- Expected: root-workspace kernel binary at `target/x86_64-unknown-none/release/slime_os-kernel`.
- Observed during migration: `rust-lld: error: cannot find linker script linker.ld`.
- Exit/fault/serial evidence: the first workspace build exited nonzero before linking; after the fix it finished successfully and produced the expected ELF.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Root `Cargo.toml` resolved six intended members | Nested component workspace could be removed cleanly |
| 2 | Kernel rustc used the workspace root as linker cwd | The relative `-Tlinker.ld` flag was no longer valid |
| 3 | Components already emit an absolute linker-script path from `CARGO_MANIFEST_DIR` | Kernel adopted the same established pattern in `kernel/build.rs` |
| 4 | File moves dropped executable mode bits | Entry scripts and LLDB helper were restored to mode 755 |
| 5 | `just generation_check`, `just contracts_check`, formatter, clippy, and QEMU all passed | The layout cutover preserves generation and runtime behavior |

## Root cause

Separate Cargo workspace roots made relative target configuration and per-project artifact paths appear local. Promoting those crates into one root workspace changes Cargo's rustc/linker working directory and artifact root. Any cwd-relative linker script or hard-coded `{crate}/target` path therefore violates the new workspace invariant.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `Cargo.toml`, `Cargo.lock` | One root workspace with six members and shared profiles/lockfile | Dependency resolution and build metadata have one root authority |
| `kernel/build.rs`, `kernel/.cargo/config.toml` | Emit the kernel linker script as an absolute manifest-relative path | Kernel links independently of Cargo's selected cwd |
| `Justfile`, build/check scripts | Select packages explicitly and use root `target/` artifacts | Mixed targets are never built implicitly; paths match workspace output |
| `scripts/{build,check,generate,lib}` | Group entry points and shared modules; update dynamic imports and generator outputs | Tool purpose and dependency direction are visible from the filesystem |
| `tools/debug/lldb-attach.sh` | Track LLDB helper and resolve the root workspace kernel | Debug entry point is reproducible instead of ignored local state |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Workspace member drift | `cargo metadata --no-deps --format-version 1` | Missing or unintended workspace member |
| Broken host tool imports/paths | `just generation_check` | Import error, missing script, or non-deterministic artifact |
| Stale generated bindings | `just contracts_check` | Generator freshness or boot-contract test failure |
| Target/linker configuration drift | `just test` | Compile/link failure or QEMU suite failure |
| Rust style/lint regression | `just fmt_check`, `just fmt_check_components`, `just lint`, `just lint_components` | Nonzero formatter or clippy exit |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `cargo metadata --no-deps --format-version 1` | Six expected workspace members | Direct |
| Kernel/components/stage-0 release builds | All finished successfully in root `target/` | Direct |
| `just generation_check` | Two builds byte-identical; boot store passed | Direct |
| `just contracts_check` | Model scenarios passed; bindings current; 12 Rust tests passed | Direct |
| `just fmt_check`; `just fmt_check_components` | Passed | Direct |
| `just lint`; `just lint_components` | Passed with warnings denied | Direct |
| `just test` | QEMU suites passed; exit 0 | Direct |

## Decisions

- Decision: keep `kernel/`, `stage0/`, and `components/` at root and invoke Cargo from each subsystem when its `.cargo/config.toml` is required.
- Rationale: root workspace unifies dependency state without conflating target-specific compiler and linker configuration.
- Rejected alternative: run every Cargo command from root with one global target config; kernel, UEFI stage-0, host tests, and component binaries require incompatible targets and linker flags.

## Open risks and follow-ups

- None observed within the exercised QEMU and host-tool paths.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none; verification results are curated above.
- Serial/debugger/model output: command output observed directly during this change.
- Related roadmap item: none; repository maintenance only.
