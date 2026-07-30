# Verification tooling: full-crate gates, dependency checks, and CI

| Field | Value |
|---|---|
| Date | 2026-07-30 |
| Kind | Change |
| Status | Verified |
| Scope | stage0 panic paths, workspace lint config, Justfile gates, deny/machete/miri/ruff/typos setup, GitHub Actions |
| Roadmap | none |
| Gates | `just lint_all`, `just fmt_check_all`, `just deny`, `just machete`, `just miri`, `just test_host`, `just ruff`, `just typos` |
| Trigger | Audit found no clippy config, no dependency auditing, no CI, and stage0/boot-contracts outside every lint gate |
| Baseline | `just lint`/`just lint_components` gated kernel and components only; stage0 carried 7 unwrap sites and 4 unchecked slice indexes on the boot path |

## Summary

The repository previously gated only kernel and components with clippy, had no dependency auditing over the ed25519/curve25519/sha2 trust chain, no unused-dependency scanning, no UB checking, no Python lint over `scripts/`, and no CI. This change eliminates every panicking path in stage-0 and denies panic-class lints there at the crate level, adds a workspace clippy configuration inherited by all six crates, wires cargo-deny, cargo-machete, Miri, ruff, and typos into the dev shell and Justfile, and adds a GitHub Actions workflow running the non-QEMU gates on push and pull request. cargo-machete immediately found two dead kernel dependencies (`volatile`, `uart_16550`) and exposed that the apparently-unused `sha2` dependency is load-bearing: it forces `force-soft` onto the sha2 that ed25519-dalek pulls in transitively, whose SIMD path crashes LLVM on bare-metal dev-profile builds.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `stage0/src/{lib,main}.rs` | Every unwrap and unchecked slice index replaced with a `BootError` return; crate-level deny of `clippy::{unwrap_used,expect_used,panic,indexing_slicing}` | Stage-0 fails closed through `BootError`; a malformed boot store cannot panic the selector |
| `clippy.toml`, root `Cargo.toml`, per-crate `[lints]` | Workspace lint table (`mem_forget`, `todo`, `dbg_macro`, `disallowed_methods` denied; `core::mem::forget` disallowed) inherited by all members | Leak-based accounting bypass and debug residue are compile errors in every crate |
| `Justfile` | `lint_stage0`, `lint_boot_contracts`, `lint_all`, `fmt_check_all`, `lint_pedantic`, `deny`, `machete`, `miri`, `test_host`, `ruff`, `typos` | Every workspace crate sits behind a named format and lint gate |
| `kernel/Cargo.toml`, `stage0/Cargo.toml` | Dead `volatile`/`uart_16550` removed; `sha2 force-soft` pinned in both bare-metal leaf crates with a machete-ignore documenting why | Dependency list matches actual use; the LLVM SIMD crash cannot resurface by dropping the pin |
| `deny.toml`, `_typos.toml`, `ruff.toml`, `flake.nix`, `rust-toolchain.toml` | cargo-deny (advisories, licenses, bans, source pinning to crates.io plus the two submodule remotes), typos (frozen transcripts excluded), ruff config, tools and miri in the dev shell | A vulnerable, yanked, or unpinned-source dependency in the release trust chain fails a named gate |
| `scripts/` | ruff cleanup: unused imports dropped; the two equal-length zips (fabric-graph route rows/routes, devlog index header/cells) made `strict=True` | Silent zip truncation becomes an error instead of a malformed artifact |
| `.github/workflows/ci.yml` | Six parallel jobs: fmt+clippy, host tests, Miri, contracts+devlog checks, deny+machete, ruff+typos | Non-QEMU gates run on every push and pull request instead of on memory |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| New panic path in stage-0 | `just lint_stage0` | clippy deny error on unwrap/expect/panic/indexing |
| Vulnerable or unpinned dependency | `just deny` | RUSTSEC advisory, license, ban, or source error |
| Dead or missing dependency declaration | `just machete` | unused-dependency report |
| UB in boot-contracts decode or slime-proto validation | `just miri` | Miri UB diagnostic |
| Host-script lint or spelling drift | `just ruff`, `just typos` | nonzero lint exit |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just fmt_check_all`, `just lint_all` | Pass, all four crates | Direct |
| `just deny`, `just machete`, `just ruff`, `just typos` | Pass | Direct |
| `just miri` | 50 boot-contracts + 3 slime-proto tests pass under Miri | Direct |
| `just test_host` | Pass | Direct |
| `just contracts_check`, `just generation_check`, `just devlog_check` | Pass | Direct |
| `just bootstate_trace_check` | 3 durable transitions conform; stalled-attempt, wrong-root promotion, retained-root collection rejected | Direct |
| `just directory_check` | Vertical slice healthy | Direct |
| GitHub Actions workflow | YAML-validated only; never executed on a runner | Unobserved conclusion |

## Decisions

- Decision: keep `lint_pedantic` (undocumented unsafe blocks, lossy casts, arithmetic side effects) advisory-only rather than gating.
  - Rationale: roughly 400 unsafe blocks lack `SAFETY:` comments and ~2000 `as` casts exist; gating now would either block all work or force a mass mechanical edit nobody reviews. The intended path is module-by-module burn-down, then promotion into `[workspace.lints.clippy]`.
  - Rejected alternative: deny immediately with per-module `allow`s — hides the backlog inside attribute noise.
- Decision: exclude QEMU checks from CI.
  - Rationale: the QEMU suites are the local verification path and depend on OVMF/limine/xorriso plumbing not validated on hosted runners; a red CI from runner drift would erode trust in the gates that do run.
  - Rejected alternative: nested-virtualization runners — unverified, and QEMU-suite ownership stays local per AGENTS.md.
- Decision: pin `sha2 force-soft` in kernel and stage0 despite neither importing it, with machete-ignore.
  - Rationale: removing it reintroduces an LLVM "Do not know how to split the result of this operator" crash in dev-profile bare-metal builds via ed25519-dalek's transitive sha2.
  - Rejected alternative: patching sha2 features at the workspace level — `[patch]` cannot add features, and a workspace-wide dependency would leak into host crates that do not need it.

## Open risks and follow-ups

- [ ] The CI workflow has never run on a real runner; expect first-run fixups (action versions, toolchain install shape).
- [ ] `fmt`/`fmt_check` still cover only the kernel; `fmt_check_all` is the complete gate. Consider renaming or folding.
- [ ] Burn down `lint_pedantic` findings (SAFETY comments first, `kernel/src/memory` casts second) and promote cleaned lints into the workspace table.

## Artifacts and provenance

- Focused report: none; this entry is self-contained.
- Raw transcript: not captured.
- Serial/debugger/model output: gate outputs summarized in Verification.
- Related roadmap item: none.
