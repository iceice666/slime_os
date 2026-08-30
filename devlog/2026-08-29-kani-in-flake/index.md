# Kani enters the flake, and the false pass it exposed

| Field | Value |
|---|---|
| Date | 2026-08-29 |
| Kind | Change |
| Status | Verified |
| Scope | `flake.nix`, `flake.lock`, `nix/kani.nix`, `just/quality.just` recipe `kani_io_proofs`, `.github/workflows/ci.yml` job `kani_proofs`, IO6 follow-ups |
| Roadmap | IO6 |
| Gates | `just kani_io_proofs` |
| Trigger | IO6 shipped `just kani_io_proofs` guarded only by a `command -v cargo-kani` check, with Kani installed imperatively per machine; its own follow-up list named flake wiring as the fix |
| Baseline | 18 harnesses verifying against a developer-local `cargo install kani-verifier && cargo kani setup` install, unpinned by this repository |

## Summary

`just kani_io_proofs` is now reproducible: `nix develop .#kani --command just
kani_io_proofs` verifies all 18 harnesses with no imperative setup, against a
bundle pinned by its published sha256 and a toolchain pinned as a store path.
Getting there exposed a defect in the work itself. The first derivation built,
passed its own `--version` install check, and appeared to pass the gate — but the
gate had silently resolved an *ambient* `~/.cargo/bin/cargo-kani` against
`~/.kani`, not the store path. The derivation was in fact broken: Nix's default
`stripPhase` had destroyed the crate metadata in the bundle's prebuilt `.rlib`s,
which surfaces as 422 source-looking errors (`E0786: found invalid metadata files
for crate 'core'`). Two properties now make that class of false pass
unrepresentable: the derivation verifies a real harness at build time rather than
calling `--version`, and the gate invokes `cargo-kani` directly rather than
`cargo kani`, which needed an ambient `cargo` to dispatch through.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `nix/kani.nix` | New. Pins the upstream kani-0.67.0 release bundle per host triple by GitHub's published asset sha256, symlinks the matching toolchain from `rust-bin.nightly."2025-11-21"` at build time, and wraps `kani`/`cargo-kani` with the bundle's CBMC binaries and a C preprocessor on `PATH` | The proof gate's toolchain is a pinned store path, not whatever a developer installed |
| `nix/kani.nix` — `dontStrip` | Nix's default strip destroys prebuilt `.rlib` crate metadata | The bundle Kani verifies against is the bundle upstream published |
| `nix/kani.nix` — `installCheckPhase` | Verifies a real symbolic harness, and asserts the bundle's own `rust-toolchain-version`/`rustc-version` recordings against the pin | A build that cannot verify anything fails at build time |
| `flake.nix` | New `rust-overlay` input; `pkgs` gains its overlay; new `devShells.<system>.kani` and `packages.<system>.kani` | Kani is available without imposing 325 MB on every other shell and CI job |
| `just/quality.just` | `cargo kani` → `cargo-kani`; the absence message names `nix develop .#kani` instead of `cargo install` | The gate cannot reach an ambient install through `cargo`'s subcommand dispatch |
| `.github/workflows/ci.yml` | New `kani_proofs` job: ordinary x64 runner, no submodules, no seL4 prefix, no cargo cache; realizes `.#kani` in its own step, then runs the gate | The proof layer is checked on every push and pull request rather than when a developer remembers |

### Why not the vendored derivation IO6's follow-up proposed

IO6 recorded "wiring the vendored `deps/rust-sel4/hacking/nix/scope/kani/`
derivation into this repository's flake" as the fix. That was attempted and
rejected on two independent grounds:

- It is reachable only from rust-sel4's own package scope. It takes
  `crateUtils`, `vendorLockfile`, `sources`, `assembleRustToolchain`,
  `parseStructuredChannel`, `elaborateRustEnvironment`, and
  `mkMkCustomTargetPathForEnvironment` — an internal graph this flake does not
  instantiate and should not, to bind one tool.
- It would not serve this repository's hosts anyway. Both shells that consume it
  (`shell-for-hacking.nix:47`, `shell-for-makefile.nix:32`) gate it on
  `lib.optionals stdenv.hostPlatform.isx86_64`, as does
  `top-level/aggregates.nix:70`. The development machine here is
  `aarch64-darwin` and CI's seL4 jobs are `ubuntu-24.04-arm`.

The upstream bundle is the same artifact `cargo kani setup` installs, and
GitHub publishes a sha256 per asset, so pinning it is a pin on bytes.

### Why `rust-overlay` rather than the shell's rustup installs

`kani-compiler` is a prebuilt `rustc_driver` client: it dynamically links
`librustc_driver-4f10604dc9267822.dylib` and codegens against that sysroot, so
it runs only against the exact nightly it was built with — `rustc 1.93.0-nightly
(53732d5e0 2025-11-20)`, recorded inside the bundle. No nixpkgs channel carries
a dated nightly, and `nixpkgs#kani` does not exist. `rust-bin.nightly."2025-11-21".minimal`
is the pinned source of that build, and produces a byte-identical `rustc
--version` and the same `librustc_driver` soname. The default shell's imperative
`rustup toolchain install` was deliberately not extended to cover it: the whole
point is that this toolchain is a store path.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A `version` bump silently needs a different toolchain | `installCheckPhase` compares the bundle's own `rust-toolchain-version` and `rustc-version` against the pin, and against `rust-bin`'s actual `rustc --version` | Build fails naming both versions and the field to change |
| The bundle installs but cannot verify (the strip defect) | `installCheckPhase` verifies a real symbolic harness | `kani <ver>: smoke harness did not verify`, at build time |
| The gate reaches an ambient install instead of the store path | Gate calls `cargo-kani` directly; the `.#kani` shell ships no `cargo` for subcommand dispatch | `cargo-kani: not found`, naming the nix shell |
| A harness stops being compiled | Pre-existing harness-count assertion (`expected=18`) | `expected 18 harnesses, ran 17` |
| A missing C preprocessor makes the gate machine-dependent | Wrapper pins `stdenv.cc` plus a `gcc`-named shim | `execvp gcc failed` cannot occur; verified in the Nix sandbox, which has no ambient compiler |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `nix build .#kani` | exit 0; `installCheckPhase` verifies its smoke harness (4 checks) | Direct |
| `env -i HOME=<empty> PATH=/usr/bin:/bin nix develop .#kani --command just kani_io_proofs` | exit 0 — `18 successfully verified harnesses, 0 failures`, 57 checks | Direct |
| Same, with `command -v cargo-kani` printed | Resolves to `/nix/store/…-kani-0.67.0/bin/cargo-kani`; ambient `cargo` absent | Direct |
| Control: one `#[kani::proof]` attribute deleted, run in the clean shell | exit 1 — `expected 18 harnesses, ran 17`, while Kani reported `VERIFICATION:- SUCCESSFUL` | Direct |
| Control (the defect): first derivation, with default `stripPhase` | 422 errors, `E0786: found invalid metadata files for crate 'core'` — and its `--version` install check had passed | Direct |
| `nix flake check --no-build` | exit 0 | Direct |
| `nix develop --command …` (default shell) | `RUSTUP_TOOLCHAIN`, `CROSS_COMPILER_PREFIX`, `LIBCLANG_PATH`, and every tool unchanged; `cargo-kani` correctly absent | Direct |
| `nix eval .#packages.{aarch64,x86_64}-linux.kani.drvPath` | Both evaluate | Direct |
| `nixfmt-rfc-style flake.nix nix/kani.nix` | exit 0, then `nix build .#kani` re-verified | Direct |
| `ci.yml` parsed with `yaml.safe_load`; `kani_proofs` job inspected | 11 jobs; `runs-on: ubuntu-latest`, `timeout-minutes: 30`, five steps in the intended order | Direct |
| Simulated `actions/checkout` **without** submodules (tracked files only, empty `deps/*`), then both CI steps run against it | `nix build .#kani` exit 0; `env -i` gate run exit 0 — `18 successfully verified harnesses, 0 failures` | Direct |
| Control: bundle hash corrupted in that clean tree | `nix build .#kani` exit 1 — `hash mismatch in fixed-output derivation`, naming specified vs got; the gate step is never reached | Direct |
| `kani_proofs` job's first real run on x86_64 Linux (PR #11, job 99054927967) | pass in 1m16s — `Complete - 18 successfully verified harnesses, 0 failures, 18 total` | Direct |

No Rust source changed, so no product lint, test, or QEMU gate was rerun.

## Decisions

- **Decision:** A separate `.#kani` devShell, not a package in `default`.
  **Rationale:** The bundle is 325 MB unpacked and exactly one gate uses it.
  Adding it to `default` would impose it on every `nix develop` and on the five
  CI jobs that never run a proof.
  **Rejected alternative:** A single shell, for the convenience of not naming
  `.#kani`.

- **Decision:** Copy the bundle's `bin/` into the store rather than symlink it.
  **Rationale:** Measured, not assumed. A fully symlinked `KANI_HOME` fails —
  macOS resolves `@loader_path` through the realpath, so `kani-compiler` looks
  for `librustc_driver` beside the *original* bundle and dies in dyld. Copying
  `bin/` (54 MB) fixes it; the remaining 271 MB is copied too, for one store
  path rather than a farm whose correctness depends on link resolution order.

- **Decision:** Verify a harness in `installCheckPhase` instead of `--version`.
  **Rationale:** `--version` passed on the strip-broken build, because it never
  compiles anything. Only a real harness exercises the sysroot, the toolchain
  symlink, and the CBMC binaries — the three things that break independently.

- **Decision:** `cargo-kani` in the gate, not `cargo kani`.
  **Rationale:** `cargo kani` needs an ambient `cargo`, and that dispatch is
  exactly how the false pass reached `~/.kani`. The direct binary was verified
  to work with no `cargo` on `PATH` at all.

- **Decision:** The CI job runs on `ubuntu-latest` (x86_64), and realizes the
  derivation in a step separate from the gate.
  **Rationale:** Two reasons, one of them coverage. Nothing in this gate builds
  for AArch64, links libsel4, or boots QEMU, so it does not belong in the arm64
  seL4 pool — and x86_64 Linux is precisely the platform this host could not
  build, so CI is where the Linux bundle and its `autoPatchelfHook` path first
  get exercised. The split step keeps a broken *pin* from being reported as a
  broken *proof*; verified by corrupting the bundle hash, which fails
  `nix build` with a hash mismatch and never reaches the gate.
  **Rejected alternative:** Folding `nix build .#kani` into the gate step, and
  caching `~/.rustup`/`~/.cargo` as the seL4 jobs do — the crate under proof has
  no dependencies and the toolchain comes from the store, so there is nothing
  for those caches to hold.

## Open risks and follow-ups

- [x] **Closed the same day, by CI.** The Linux path was inference when this
  entry was written — this host cannot build for `*-linux`, so only
  `aarch64-darwin` was built and run locally. The `kani_proofs` job's first run
  (PR #11, job 99054927967) verified 18/18 harnesses on x86_64 Linux in 1m16s,
  which makes the `autoPatchelfHook`/`stdenv.cc.cc.lib` path observed rather
  than inferred. `aarch64-linux` and `x86_64-darwin` remain evaluated-only.
- [x] **Closed the same day.** `just kani_io_proofs` now runs in CI as the
  `kani_proofs` job. It remains outside `contracts_check` and every other
  aggregate gate on purpose: those must keep passing on a machine with no Kani
  closure.
- [ ] The `gcc`-named shim points at `stdenv.cc`'s `cc` (clang on Darwin). Only
  preprocessing of `kani_lib.c` is asked of it, which is why this is sound, but
  it is a name-level compatibility shim rather than a real gcc.

## Artifacts and provenance

- Derivation: `nix/kani.nix`
- Shell and package outputs: `flake.nix`, `devShells.<system>.kani` and `packages.<system>.kani`
- Gate: `just/quality.just`, recipe `kani_io_proofs`
- CI job: `.github/workflows/ci.yml`, job `kani_proofs`
- Kani version: 0.67.0, matching the `rev = "kani-0.67.0"` pin in `deps/rust-sel4/hacking/nix/scope/kani/default.nix`; bundled toolchain `nightly-2025-11-21`, `rustc 1.93.0-nightly (53732d5e0 2025-11-20)`
- Upstream release: <https://github.com/model-checking/kani/releases/tag/kani-0.67.0>
- Related roadmap item: [IO6](../../roadmap/11-io-substrate.md)
- Preceding entry: [`devlog/2026-08-29-io6-kani-wire-proofs/`](../2026-08-29-io6-kani-wire-proofs/index.md)
