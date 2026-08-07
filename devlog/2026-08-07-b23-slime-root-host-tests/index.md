# B23 — `slime-root`'s unit tests were run by no gate

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root/Cargo.toml`, `slime-root/src/lib.rs`, `slime-root/src/{main,channel,generation}.rs`, `Justfile` |
| Roadmap | B23, P5.4.1, P5.4 |
| Gates | `just test_sel4_root` |
| Trigger | B23, opened during the B16 fix and inherited by P5.4.1; every P5.4.2+ slice would inherit it again |
| Baseline | Nine seL4 gates passing; 102 `#[test]` functions compiled by nothing |

## Summary

`slime-root`'s 102 unit tests were compiled by nothing and run by nothing, for
two independent reasons: no Justfile target named the crate, and it could not
have run anyway — `main.rs` is unconditionally `#![no_std]`/`#![no_main]`, the
package declared no lib target, and the crate built only for a seL4 JSON target
with no `libtest`. Fixed by splitting the mechanism modules into a library the
binary links, which `just test_sel4_root` runs on the host with the count
asserted. The first run immediately paid for itself: **three latent defects
surfaced**, all of them tests that had been silently wrong since a signature or
guard changed under them — nine `push` call sites stale since P5.3.2, an
`elf_header` fixture asserting the wrong `classify` arm since `target` gained a
length guard, and a `qualified` fixture whose tail size was a literal that no
longer matched.

The count is 102 where B23 recorded 98, across the same 13 modules: B16's fix
added four supervision tests after that entry was written. A small illustration
of the blind spot itself — a number nothing could check had already drifted.

## Observable symptom

- Command: `cargo test -p slime-root` — before this change, no such target
  existed. **Inherited evidence** (the original B23 entry, now in the resolved
  log): `--all-targets` failed with 103 instances of `can't find crate for
  'test'`. Not re-observed here and no longer reproducible, since the lib target
  that would have to be absent now exists.
- Expected: the modules `main.rs` describes as "bounded, pure, and unit-tested
  in place" have their tests run by some gate.
- Observed: `just test_host` ran `boot-contracts` and `slime-proto` only.
  `slime-root` appeared in the Justfile solely in its fmt and clippy targets.
- Exit/fault/serial evidence: [`host-test-run.log`](host-test-run.log) — the
  102 tests now running; [`fault-injection-dropped-test.log`](fault-injection-dropped-test.log)
  — the count assertion refusing 101.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `sel4` builds for `aarch64-apple-darwin` given `SEL4_PREFIX` | The split need not exclude the seL4-touching modules. All 13 covered modules can run, not just the eight that are sel4-free — so the library is the whole mechanism surface rather than a subset |
| 2 | `sel4-root-task` pulls `sel4-alloca`, whose inline `.section … %progbits` is ELF-only and fails to assemble on Mach-O | Scoped to `cfg(target_os = "none")`. Only the binary needs it; the library never links it, and the seL4 build is unchanged |
| 3 | First compile: **9 errors**, all `push` called with 3 args against a 4-arg signature | `transferable` was added in P5.3.2 (`c474d25`) and every test call site kept the old form. Bit-rotted for five milestones, invisible because nothing compiled them |
| 4 | `generation::tests::aarch64_elf64_is_loadable` failed | `classify` reaches its bare-ELF arm only when `component_image::target` answers `BadMagic`; a blob shorter than `LEGACY_HEADER_LEN` (32) answers `Truncated` and falls to `Unrecognized`. The fixture was 20 bytes, so it had been asserting the wrong arm since `target` gained that guard |
| 5 | Widening `elf_header` then broke `a_qualified_elf_image_is_loadable` | `qualified` sized its tail with a literal `20`. Now derived from the same `ELF_TAIL` constant, so widening one fixture cannot silently truncate the other |
| 6 | `channel::tests` imported `WaitTarget` unused | A fourth, harmless instance of the same class: a test changed, the import did not, nothing noticed |
| 7 | A trivially-panicking test in a fresh `cargo new` crate aborts identically on this host | The `fatal runtime error: failed to initiate panic` seen while debugging is a host toolchain defect, **not** a property of this change or of `panic = "abort"`. Recorded so a later reader does not chase it |
| 8 | Cargo reports `` `panic` setting is ignored for `test` profile `` | A `[profile.test] panic = "unwind"` added while chasing (7) was inert and was removed rather than left as cargo-cult |

## Root cause

Two independent blockers, which is why the entry called it a crate-structure
change rather than a test change.

**Nothing invoked them.** No `cargo test` invocation in the Justfile named
`slime-root`; CI runs `just test_host`, which listed two other crates.

**They could not have run.** A package with only a `[[bin]]` whose `main.rs` is
`#![no_main]` exposes no unit-test target at all, and the crate's only build
configuration was a custom seL4 JSON target with no `libtest`. So even a gate
that named the crate would have failed to build a test harness.

The violated invariant is the one `main.rs` states about itself: the modules are
"bounded, pure, and unit-tested in place". They were bounded and pure. The third
clause was false, and step 3 shows it had been false long enough for a signature
change to rot nine call sites unnoticed.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/Cargo.toml` | `[lib] slime_root` + explicit `[[bin]] slime-root`; `sel4-root-task` scoped to `cfg(target_os = "none")` | The mechanism modules have a test target; the binary keeps the `.elf` name the loader expects |
| `slime-root/src/lib.rs` | New crate root declaring the 17 modules `pub` | One library, linked by the binary rather than recompiled — so a host pass is evidence about the shipped root |
| `slime-root/src/main.rs` | Imports the modules from `slime_root` instead of declaring them | Single definition |
| `channel.rs` | `push_undelegated` test shim; nine call sites rewritten; unused import dropped | The P5.3.2 signature change is absorbed once rather than at ten sites |
| `generation.rs` | `ELF_TAIL` constant; `elf_header` padded past `LEGACY_HEADER_LEN`; `qualified`'s tail derived from it | The fixtures reach the arm they assert about |
| `Justfile` | `test_sel4_root`, a gate of its own | B23's exit condition is observable |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A module stops being covered | `just test_sel4_root` | `ran N tests, expected 102; a module lost or gained coverage` |
| A test stops compiling | `just test_sel4_root` | Build failure — which is the whole point; this class was invisible before |
| The lib split breaks the image | `just sel4_crossing_check` and the eight other seL4 gates | The boot markers those gates assert |
| The prefix is missing rather than the gate silently skipping | `just test_sel4_root` | `no installed seL4 prefix at …; run 'just sel4_qemu_image_check' first` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test_sel4_root` | Pass — 102/102 across 13 modules — [`host-test-run.log`](host-test-run.log) | Direct |
| Fault injection: one `transit` test removed | Fails with `ran 101 tests, expected 102` — [`fault-injection-dropped-test.log`](fault-injection-dropped-test.log) | Direct |
| `just sel4_root_boot_check`, `sel4_component_graph_check`, `sel4_channel_check`, `sel4_loan_check`, `sel4_spawn_check`, `sel4_sample_check`, `sel4_stream_check`, `sel4_supervision_check`, `sel4_crossing_check` | All pass — the lib split did not disturb the image | Direct |
| `just contracts_check`, `just generation_check` | Pass | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | Pass | Direct |
| `just test_host` | **Fails on this host, and did so before this change** — its `slime-proto` arm hardcodes `x86_64-unknown-linux-gnu`, which is not installed on `aarch64-apple-darwin`. Confirmed by stashing the change and re-running. Left untouched: this slice adds no arm to it | Direct |

## Decisions

- Decision: a **library the binary links**, not a `cfg(test)` escape in `main.rs`
  and not a separate test-only crate.
- Rationale: the escape does not work — `#![no_main]` and the JSON target's
  missing `libtest` are the blockers, and neither is `cfg`-able. A separate
  crate would duplicate the modules, and a passing test would then be evidence
  about the copy rather than about the root. Linking one library keeps "the
  tested code is the shipped code" true by construction.
- Rejected alternative: splitting out only the eight sel4-free modules. Step 1
  established it was unnecessary, and it would have left `ipc`, `task`, `fault`,
  and `transfer_window` — 23 tests — permanently uncovered for no reason.

- Decision: assert the count in the gate rather than trusting `cargo test` to
  find everything.
- Rationale: B23's exit condition asks for exactly this, and the failure it
  guards is silent. A module that stops being declared in `lib.rs`, or a
  `#[cfg(test)]` block that stops compiling into the harness, reduces the count
  without failing anything. The fault injection shows the assertion binds.
- Cost, stated plainly: the number must be raised deliberately when tests are
  added. That is the intended friction — it is what makes a *removal* visible.

- Decision: `push_undelegated` shim rather than a fourth argument at nine sites.
- Rationale: none of those tests is about delegation, and threading `false`
  through each would put a literal where the reader must decide whether it
  matters. One named shim says once that it does not.

## Open risks and follow-ups

- [ ] **CI does not run this gate**, and cannot: `slime-root`'s tests compile
      against the installed seL4 prefix, which the `test_host` runner does not
      build. It is therefore a standalone gate rather than a `test_host` arm,
      on exactly `lint_sel4_root`'s precedent, and it runs in the pinned
      `nix develop` shell alongside the image and the nine seL4 gates. The
      omission is noted in `.github/workflows/ci.yml` so it reads as a decision.
      The cost is real: a `slime-root` test regression is caught by a developer
      or the release machine, not by a pull request.
- [ ] **`just test_host` cannot run on an `aarch64-apple-darwin` host** — its
      `slime-proto` arm pins `x86_64-unknown-linux-gnu`. Pre-existing, untouched
      here, and unrelated to this change; worth its own backlog item if a second
      non-Linux host appears.
- [ ] **The gate needs the built seL4 prefix**, so it is not runnable from a
      clean checkout without `just sel4_qemu_image_check` first. It refuses
      loudly rather than skipping, on `lint_sel4_root`'s rule, but it does make
      these tests less cheap than `boot-contracts`'s.
- [ ] **`main.rs` itself is still untested**, and deliberately: it is the seL4
      startup staging and dispatch loop, whose behavior needs a running kernel.
      The seL4 gates own it. This change closes the gap where a pure-logic
      regression was caught by neither, not the one where a syscall misbehaves.
- [ ] The three defects this surfaced were all *test* bugs rather than
      production bugs, which is the good case. It is not evidence that no
      production defect was hiding — only that none of the 102 assertions, once
      they could run, found one.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`host-test-run.log`](host-test-run.log) — the full passing
  run.
- Serial/debugger/model output:
  [`fault-injection-dropped-test.log`](fault-injection-dropped-test.log).
- Related roadmap item: [B23](../../roadmap/00-backlog.md) (resolved),
  [P5.4.1](../../roadmap/07-architecture-portability.md) (which recorded this as
  the blind spot every slice inherits).
