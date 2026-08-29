# IO6: bit-precise proofs of the IO substrate's wire arithmetic

| Field | Value |
|---|---|
| Date | 2026-08-29 |
| Kind | Change |
| Status | Verified |
| Scope | `components/proto/src/io_queue_proofs.rs` (new), `components/proto/src/lib.rs`, `components/proto/build.rs` (new), `verification/io-proofs/` (new), `just/quality.just`, `roadmap/11-io-substrate.md` |
| Roadmap | IO6, IO5, IO0 |
| Gates | `just kani_io_proofs`, `just lint_all`, `just fmt_check_all`, `just test_host`, `just miri` |
| Trigger | IO5 shipped two bounded models that explicitly disclaimed the wire layer; the question was whether to keep going with `zutai model-check` or add implementation-level proof |
| Baseline | IO0–IO4 observed per-schedule under QEMU; IO5's models quantify over interleavings of an abstraction; wire arithmetic covered only by fixed-input `#[test]`s |

## Summary

IO5's models close the *all-interleavings* quantifier over an abstraction of the
IO0 substrate, and deliberately disclaim the wire layer — sequence encoding,
slot arithmetic, bounds — on the grounds that a model restating field offsets
would be a second, drifting copy of the contract. That reasoning was right, but
it left a real gap: the disclaimed obligations are themselves universally
quantified, over *values* rather than schedules, and a fixed-input `#[test]`
closes them no better than a single QEMU schedule closes an interleaving claim.

IO6 closes that gap with eighteen Kani harnesses over the shipped `slime-proto`
source, guarded by `just kani_io_proofs`. All eighteen verify (57 checks, 14 s);
eighteen mutations of the real source each produce their counterexample; two
gate controls confirm fail-closed behavior. The IO track now has three
non-interchangeable layers: one real schedule (plane gates), all interleavings
of an abstraction (IO5 models), all values through the shipped Rust (IO6
proofs).

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `components/proto/src/io_queue_proofs.rs` | New. Eighteen `#[kani::proof]` harnesses over slot arithmetic, header cursor safety, mapping bounds, status totality, slice bounds, and `Outstanding` lease lifetime | The preconditions `queue_slot_index` and the occupancy subtractions rely on are mechanically tied to the code that establishes them |
| `components/proto/src/lib.rs` | Six lines: `#[cfg(kani)] mod io_queue_proofs;` plus rationale | Harnesses live beside the code they check, per `deps/rust-sel4/crates/sel4/bitfield-ops` |
| `components/proto/build.rs` | New, dependency-free. `cargo::rustc-check-cfg=cfg(kani)` | `just lint_all` denies warnings and includes `unexpected_cfgs`; without this every product build of the crate fails on a cfg it cannot know is intentional |
| `verification/io-proofs/Cargo.toml` | New. `[lib] path` points at `components/proto/src/lib.rs`; empty `[workspace]` keeps it out of the root workspace | Kani compiles the *shipped* source, so verified-versus-shipped drift is not representable |
| `just/quality.just` | New `kani_io_proofs` recipe beside `miri`, with a tool-presence check and a harness-count assertion | A proof gate that can silently verify nothing is not a gate |
| `roadmap/11-io-substrate.md` | New IO6 section; status header, verification stack, and definition of done extended | The third layer and its boundary are recorded, not implied |

### Why a separate proof crate rather than an MSRV change

Kani 0.67.0 — the version already pinned by
`deps/rust-sel4/hacking/nix/scope/kani/default.nix` — ships its own toolchain,
nightly-2025-11-21. This repository pins nightly-2026-05-26, and `slime-proto`
declares `rust-version = 1.96`. Cargo refuses to build a package whose declared
`rust-version` exceeds the compiler in hand:

```
error: rustc 1.93.0-nightly is not supported by the following package:
  slime-proto@0.1.0 requires rustc 1.96
```

Three routes were available and two were rejected. Lowering the declared MSRV
would put a falsehood in a shipped manifest to accommodate a verification tool.
Copying the code under proof into a proof crate would create precisely the drift
this repository's generated-code rule exists to prevent — the verified artifact
and the shipped artifact would be two files that merely start out equal.

`CARGO_RESOLVER_INCOMPATIBLE_RUST_VERSIONS=allow` was tried and does not apply:
it affects dependency *resolution*, not the build-time check. Standalone `kani`
(no cargo) bypasses the check but offers no `--edition` flag, so it cannot
compile this edition-2024 crate — 336 syntax errors.

The chosen route points `[lib] path` at the real `src/lib.rs` under a manifest
declaring no MSRV. Kani compiles the same bytes the product compiles. There is
one file, so there is nothing to drift.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A harness silently stops being compiled and the gate still reports success | `just kani_io_proofs` harness-count assertion | `expected 18 harnesses, ran 17` |
| Proof file drifts from shipped source | `verification/io-proofs/Cargo.toml` points at `components/proto/src/lib.rs` | Structurally impossible: one file |
| Harnesses leak into a product build | `#[cfg(kani)]` gate; proof crate outside the root workspace | `just lint_all`, `just miri`, `just test_host` |
| `kani` cfg breaks the deny-warnings lint gate | `components/proto/build.rs` declares it | `unexpected_cfgs` under `-D warnings` |
| Kani absent on a contributor's machine | Recipe checks `command -v cargo-kani` first | Named error plus the two install commands, not a cryptic cargo failure |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just kani_io_proofs` | exit 0 — `18 successfully verified harnesses, 0 failures`, 57 checks, 14 s | Direct |
| 18 source mutations, one per harness | Each produces a `Status: FAILURE` counterexample in the matching harness | Direct |
| Control: proof module disabled (`#[cfg(any())]`) | Gate exit 1 | Direct |
| Control: one `#[kani::proof]` attribute deleted | Gate exit 1 via count assertion, while Kani reported `VERIFICATION:- SUCCESSFUL` | Direct |
| `just lint_all` | exit 0 (after `build.rs`; exit 101 before it) | Direct |
| `just fmt_check_all` | exit 0 (after `rustfmt`; exit 1 before) | Direct |
| `just test_host` | exit 0 | Direct |
| `just miri` | exit 0 | Direct |
| `git diff` on `io_queue_ring.rs` after all mutation runs | Byte-identical to `HEAD` | Direct |

### The mutation that justifies two of the harnesses

Most mutations fail loudly. The instructive one does not: replacing

```rust
(sequence as usize) & (slot_count - 1)
```

with

```rust
((sequence as usize) & (slot_count - 1)) / 2
```

keeps every produced index strictly *in bounds*, so a bounds-only proof stays
green while two live sequences alias onto one slot — the overwrite IO5's model
forbids at the abstract level. It is caught only by
`queue_slot_index_is_modular` and
`distinct_live_sequences_occupy_distinct_slots`. A suite containing just the
bounds harness would have passed this mutation and looked complete.

### The control that justifies the count assertion

Deleting a single `#[kani::proof]` attribute leaves Kani reporting
`VERIFICATION:- SUCCESSFUL` — because the seventeen remaining harnesses do all
pass. Only the gate's own count check turns that into a failure. Without it, a
proof file that quietly stopped being compiled would report success forever.

## Decisions

- **Decision:** Add Kani as a third verification layer rather than extending the
  IO5 models to cover wire arithmetic.
  **Rationale:** The models' disclaimer is load-bearing. A model that restates
  field offsets, magics, and slot lengths becomes a hand-maintained copy of the
  contract that can drift from the generated codec — exactly what the repository's
  Zutai-first rule forbids. Kani checks the real code, so no correspondence
  argument is needed at all.
  **Rejected alternative:** More `#[test]` cases. They sample the value space
  the way extra QEMU arms sample the schedule space: more evidence, quantifier
  still open.

- **Decision:** Keep IO5's models; do not treat IO6 as superseding them.
  **Rationale:** A Kani harness drives one entry point, not two parties
  interleaving over time. The `leadsTo` liveness rules and all-schedules
  reachability obligations are inexpressible as a per-function harness. The two
  layers answer different questions: the models know about time and cannot see
  `u64` wraparound; the proofs see wraparound and know nothing about time.
  **Rejected alternative:** Replacing the models with proofs, which would have
  silently dropped every liveness claim.

- **Decision:** Point the proof crate at the real source instead of lowering
  `slime-proto`'s declared MSRV.
  **Rationale:** The declared MSRV is a true statement about the product
  toolchain. A verification tool's older bundled nightly is not a reason to make
  a shipped manifest wrong.
  **Rejected alternative:** Copying the code under proof, which would create a
  verified artifact distinct from the shipped one.

- **Decision:** `kani_io_proofs` stands outside `contracts_check`, beside `miri`.
  **Rationale:** It needs a separate toolchain plus CBMC — the same reason
  `miri` is not in an aggregate gate. Folding it in would make `contracts_check`
  fail on any machine without Kani installed.

- **Decision:** Declare the `kani` cfg in `build.rs`, not `[lints.rust]`.
  **Rationale:** Cargo rejects a package that both takes `[lints] workspace =
  true` and overrides lints locally. `build.rs` is the mechanism
  `bitfield-ops` uses, and a dependency-free one preserves the zero-dependency
  shape that lets `slime-rt` depend on this crate.

## Open risks and follow-ups

- [ ] `Outstanding` proofs are evidence about `Outstanding<2>` only: `N` is a
  const generic fixed at compile time. The capacity-independent lifetime
  argument remains the IO5 model's. Proving a symbolic `N` would need Kani
  support this version lacks.
- [ ] The shared-memory entry points — `Queue::submit`, `take_request`,
  `complete`, `take_completion` — are covered indirectly, through the header
  invariants and slot arithmetic they all depend on, not by symbolic execution
  over a full mapping. A harness allocating a symbolic mapping is possible and
  unmeasured; cost unknown.
- [ ] No `slime-root` code is proved. `io_resource.rs` charge accounting stays
  IO5-modelled and plane-observed. IO1's per-access MMIO subrange arithmetic is
  a plausible next Kani target.
- [ ] Kani is not in the Nix flake's `devShell`; contributors install it via
  `cargo install --locked kani-verifier && cargo kani setup`. The recipe names
  those commands on absence. Wiring the vendored
  `deps/rust-sel4/hacking/nix/scope/kani/` derivation into this repository's
  flake would make the gate reproducible rather than machine-local.
- [ ] `just kani_io_proofs` is not registered in any aggregate gate, so it runs
  only when invoked. Deliberate while Kani is a local install; revisit if it
  enters the flake.

## Artifacts and provenance

- Harnesses: `components/proto/src/io_queue_proofs.rs`
- Proof crate: `verification/io-proofs/Cargo.toml`
- Gate: `just/quality.just`, recipe `kani_io_proofs`
- Kani version: 0.67.0, matching `deps/rust-sel4/hacking/nix/scope/kani/default.nix` (`rev = "kani-0.67.0"`); bundled toolchain nightly-2025-11-21
- Harness placement precedent: `deps/rust-sel4/crates/sel4/bitfield-ops/src/lib.rs`, cfg declaration precedent `.../bitfield-ops/build.rs`
- Related roadmap item: [IO6](../../roadmap/11-io-substrate.md), preceded by [IO5](../../roadmap/11-io-substrate.md)
- Preceding entry: [`devlog/2026-08-29-io5-substrate-models/`](../2026-08-29-io5-substrate-models/index.md)
