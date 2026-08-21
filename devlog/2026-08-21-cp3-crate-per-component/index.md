# CP3: one crate per component, and the three Cargo behaviors that shaped it

| Field | Value |
|---|---|
| Date | 2026-08-21 |
| Kind | Change |
| Status | Verified |
| Scope | `Cargo.toml`, `components/bins/<component>/` ×52 (new), `components/lib/` (was `components/bins/src/`), `components/build-support/` (new), `scripts/build/build-generation.py`, `scripts/lib/component_spec.py`, `scripts/check/check-component-crate-split.py` (new) and 7 retargeted check scripts, `docs/syscall-abi.md`, `Justfile`, `AGENTS.md` |
| Roadmap | CP3, B70, B65 |
| Gates | `just component_crate_split_check`, `just lint_all`, `just fmt_check_all`, `just machete`, `just test_host`, `just generation_check`, `just system_spec_check`, `just component_spec_check`, the 33 `just sel4_*_check` planes, `just sel4_gate_control_check` |
| Trigger | CP3's deliverables: `components/bins` was one crate with 52 hand-listed `[[bin]]` entries and a private manifest parser, so a component could not be built anywhere else |
| Baseline | One package `slime-components`, 52 `[[bin]]`s, `autobins = false`, one `build.rs` privately string-parsing `contracts/generation/v1/fixtures/*.zti`, one `store` feature switching `#[global_allocator]` on for every binary in the crate |

## Summary

`components/bins` is now 52 independent workspace packages, one per component,
each with its own `Cargo.toml`, `build.rs`, and `src/main.rs`. The shared helper
modules moved to a `slime-components` library crate at `components/lib`, and the
generation-manifest parser that used to be private to the old crate's build
script is a documented library, `slime-build-support`, that any component crate —
in-tree or out — depends on from `[build-dependencies]`. A new component is one
new directory: verified by creating one, building it to a 35544-byte ELF, and
deleting it, with no edit to any other component's crate.

Three measured Cargo behaviors shaped the result, and each contradicted the
obvious approach:

1. **Byte-identical component ELFs are impossible under a package rename.**
   `-C metadata` derives from the package name and lands in CGU symbol names
   inside the shipped `.symtab`. CP3's deliverable 4 asked for byte-identical
   output; that clause is unachievable and is amended rather than silently
   dropped.
2. **Feature unification crosses packages in one invocation.** Declaring
   `slime-rt/heap` in only the six store crates does *not* by itself scope the
   allocator: build a plain component in the same `cargo build` and it gets the
   heap too. The builder therefore issues two grouped invocations.
3. **`[profile.release.package."*"]` does not apply to workspace members**, and
   a glob package name is rejected outright, so all 52 components need an
   explicit stanza and a gate to keep them.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `components/bins/<name>/` ×52 | One package each: `slime-component-<name>`, one `[[bin]]` named `<name>`, `test = false`, deps declared per crate | A component is a crate, not a `[[bin]]` of someone else's crate |
| `components/lib/` | The 11 `super::`-free shared modules as `pub mod`s of `slime-components`; the 10 `super::`-coupled ones as source files `#[path]`-included by their 1–3 owners | Shared code lives once; a module that reads its host's profile constants stays host-local |
| `components/build-support/` | The manifest parser, hoisted verbatim, with `configure()` + three `emit_*` entry points and a `SLIME_COMMAND_PROFILE_MANIFEST_PATH` escape hatch | The parser is a documented dependency, not a private module |
| `scripts/build/build-generation.py` | `STORE_COMPONENTS`; `build_rust_components` builds `-p slime-component-<name>` in two feature-grouped invocations | The allocator reaches only the components that declare it |
| `scripts/lib/component_spec.py` | `workspace_binaries()` walks crate directories, failing on a crate whose `[[bin]]` count or name disagrees with its directory | A shipping component cannot become invisible to the spec corpus |
| `scripts/check/check-component-crate-split.py` | New gate, six arms | The split's properties are asserted, not reviewed |
| `docs/syscall-abi.md` | `## Compatibility and versioning` | An out-of-tree crate has a stated contract to build against |
| 7 check scripts | Source-scanning roots and per-component paths retargeted | A guard that scans zero files is worse than none |
| `components/bins/sel4-transfer-probe` | Dead `STATE_FLAG_TRAVEL` import removed | — |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A component grows a private manifest parser again | `just component_crate_split_check` | `build.rs does more than call slime-build-support` |
| A component declares an allocator without moving build groups | `just component_crate_split_check` | `the crates declaring an allocator … are not the builder's store group` |
| A new crate is added without a release-profile stanza, or one is weakened to `opt-level = 3` | `just component_crate_split_check` | `no [profile.release.package] stanza for …` / `release profile is {…}, expected {…}` |
| Two `[[bin]]`s in one crate, or a bin/directory name mismatch | `just component_crate_split_check` | `declares 2 [[bin]] entries` / `the crate directory and its binary must share one name` |
| Shared source left in `components/bins` | `just component_crate_split_check` | `shared source belongs in components/lib` |
| `slime-build-support` renamed into the `slime-component-*` glob | `just component_crate_split_check` | `it must not match the 'slime-component-*' glob` |
| The manifest parser answers a wrong slot | `just test_host` | 4 `slime-build-support` tests |
| A component crate stops compiling for the seL4 target | `just lint_all` | clippy, 52 crates by `-p 'slime-component-*'` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just component_crate_split_check` | pass; 52 crates, 6 with allocator | Direct |
| All 33 `just sel4_*_check` plane gates | all pass | Direct |
| `just sel4_gate_control_check` | pass — every marker gate still fails on missing/reordered/failed evidence | Direct |
| `just generation_check` | two isolated builds byte-identical | Direct |
| `just system_spec_check`, `just component_spec_check`, `just contracts_check` | pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just machete`, `just deny`, `just ruff`, `just typos` | pass | Direct |
| `just test_host` (incl. 4 new), `just test_sel4_root` (131) | pass | Direct |
| Allocator scoping: `nm` on the shipped ELFs | plain `sysinfo` 0 allocator symbols; `sel4-store-probe` 4 | Direct |
| Feature-unification leak: plain + store component in one invocation | linked `slime_rt` rlib gains 6 heap symbols; 0 when grouped | Direct |
| Add-one-directory: scratch crate created, built, deleted | 35544-byte ELF, no other crate edited; gate caught its missing profile stanza | Direct |
| Gate non-vacuity: 6 arms perturbed one at a time | all 6 caught | Direct |
| Profile-arm non-vacuity: `opt-level = 3`, dropped `codegen-units`, weakened library profile | all 3 caught | Direct |
| Parser-test non-vacuity: 3 mutations (positional slot lookup, ignored `exec` right, empty profile accepted) | all 3 caught | Direct |
| `fmt_components` scope: bad formatting injected into `slime-root/src/lib.rs` | `fmt_check_components` passes (out of scope), `fmt_check` fails (in scope) | Direct |

## Decisions

- **Decision:** amend CP3's byte-identical-output clause instead of satisfying it.
  **Rationale:** Cargo's `-C metadata` hash is a function of the package name and
  appears in CGU symbol names inside the shipped `.symtab` — the real
  `sysinfo.elf` carries `sysinfo.5535ed4def8ceda-cgu.0` in an 11616-byte
  `.symtab`. A controlled test holding source, bin name, target, and target dir
  fixed and varying only the package name produced 15 differing bytes, all inside
  that string. Nothing in the repository pins those bytes: the system-spec
  baselines pin `.zti` manifests, and `check-generation-determinism.py` compares
  two builds of the *same* tree. The achievable property — and the one that
  matters — is determinism plus unchanged gate behavior, both observed.
  **Rejected alternative:** stripping symbols from component ELFs to force the
  bytes to match. It would change every shipped image and lose symbolization for
  fault triage, to satisfy a clause by deleting the evidence that contradicts it.

- **Decision:** two cargo invocations grouped by feature set, not one per crate.
  **Rationale:** the allocator scoping CP3 asks for is not achieved by per-crate
  feature declarations alone. Cargo unifies features across every package in one
  invocation, measured on the real crates: building `sysinfo` beside
  `sel4-store-probe` produced a linked `slime_rt` rlib with 6 heap symbols, where
  the grouped build has 0. Two invocations restore the scoping at today's build
  cost; 52 would serialize 52 cargo startups per plane across 29 fixtures.
  **Rejected alternative:** one invocation per component, the deliverable's
  literal first reading.

- **Decision:** the 10 `super::`-coupled shared modules stay `#[path]`-included
  rather than becoming library modules.
  **Rationale:** each reads its *including binary's* generated profile constants
  or helpers — `matrix_broker.rs` uses `super::{FABRIC_TRACE_DEPTH, fail,
  control_clients, release_received}`, `fabric_occupancy_trace.rs` has two
  `const _: () = assert!(super::FABRIC_TRACE_DEPTH …)` guards. That coupling is
  what makes each worker's trace sink its own, and 5 of the 10 are included by
  2+ components. Cross-crate `#[path]` keeps one copy of the source with no
  behavior change.
  **Rejected alternative:** refactoring the coupling away by parameterizing
  ~4600 lines of broker internals — the right end state for CP4/CP5, but a far
  larger blast radius than a packaging change should carry.

- **Decision:** name every release-profile stanza explicitly and gate it.
  **Rationale:** `[profile.release.package."*"]` is accepted but does **not**
  apply to workspace members (verified: members still compiled at `opt-level=3`
  under it), and `[profile.release.package."slime-component-*"]` is rejected as
  an invalid package name. There is no mechanism that covers a glob of packages,
  so the list is real and a gate is the only thing that keeps it complete.

- **Decision:** `slime-build-support`, not `slime-component-build`.
  **Rationale:** the builder and the clippy recipe select components with
  `-p 'slime-component-*'`. Under the first name the glob swept in this
  host-only build-script library and the seL4 clippy pass died on a
  `std`-dependent transitive dependency. The gate now pins the naming.

## Open risks and follow-ups

- [ ] `cargo fmt` supports neither a `-p` glob nor `--exclude`, so
  `fmt_components` derives its package list from `cargo metadata` through a
  private `_component_packages` recipe. If `cargo fmt` gains either, that
  indirection should collapse.
- [ ] 11 crates carry a `[package.metadata.cargo-machete]` ignore because
  `cargo-machete` reads only a crate's own directory and cannot see the
  `#[path]` modules it compiles from `components/lib/src/`. Each ignored
  dependency was verified used by removal, not asserted. If machete learns to
  follow `#[path]`, they should go.
- [ ] B70's remaining 9 `include!` sites are untouched by CP3 and unchanged in
  character: they size fixed arrays at compile time, so no runtime query retires
  them. They are CP4/CP5 declared-capacity work.
- [ ] CP3 did not scope the `store` feature's *documentation* comment in
  `components/runtime/Cargo.toml`, which still describes the crate-wide
  contagion the split removed.

## Artifacts and provenance

- Related roadmap items: [CP3](../../roadmap/10-component-platform.md), [B70](../../roadmap/00-backlog.md), [B65](../../roadmap/00-backlog.md)
- New gate: `scripts/check/check-component-crate-split.py`
- Serial evidence: quoted inline in Verification; no raw transcript retained
- Predecessor entry: [`devlog/2026-08-21-b70-boot-action-query/`](../2026-08-21-b70-boot-action-query/index.md)
