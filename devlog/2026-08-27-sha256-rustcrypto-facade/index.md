# Two hand-written SHA-256 implementations replaced by one RustCrypto facade

| Field | Value |
|---|---|
| Date | 2026-08-27 |
| Kind | Change |
| Status | Verified |
| Scope | `boot-contracts/src/sha256.rs`, `boot-contracts/Cargo.toml`, `components/runtime/src/lib.rs`, deleted `components/runtime/src/sha256.rs`, `Cargo.lock` |
| Roadmap | none |
| Gates | `just test_host`, `just miri`, `just test_sel4_root`, `just sel4_storage_check`, `just sel4_store_check`, `just sel4_transfer_check`, `just deny` |
| Trigger | The workspace carried two independent hand-written FIPS 180-4 SHA-256 implementations — a streaming one in `boot-contracts` and a one-shot one in `slime-rt` — while `sha2 0.10.9` was already resolved in `Cargo.lock` through `ed25519-dalek` and already covered by `deny.toml`'s trust-chain policy |
| Baseline | Every digest the workspace computes — generation identity, bootstate and transfer checksums, boot-layout/fabric/lifecycle identities, object-store content addresses, boot-selector directory roots, and the probes' fixed expected digests — was produced by hand-written compression functions |

## Summary

`boot_contracts::sha256` is now a thin wrapper over `sha2::Sha256` and is the workspace's only SHA-256. Its public surface is unchanged — `Sha256::{new, update, finalize}`, `Sha256::default()`, and the free `digest(&[u8]) -> [u8; 32]` — so no caller in `boot-contracts`, `slime-root`, or the components changed. `components/runtime/src/sha256.rs` is deleted; `slime_rt::sha256` is now a re-export of `boot_contracts::sha256::digest`, keeping the exact call signature the four probe call sites already use. Roughly 230 lines of unaudited cryptographic code left the repository, and the digest values are byte-identical: the existing eight-test module in `boot-contracts` — NIST vectors, the one-million-`a` vector fed in unaligned 7-byte chunks, every split point, three-way splits, and every length adjacent to the padding boundary — passes unchanged, and three QEMU planes verify digests against frozen constants on real aarch64 seL4.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `boot-contracts/src/sha256.rs` | Replaced the `INITIAL`/`K` tables, the four-field `Sha256` struct, its buffering `update`/padding `finalize`, and the private `compress` with a newtype over `sha2::Sha256`; the `#[cfg(test)]` module is untouched and now serves as the equivalence proof | One SHA-256 implementation in the workspace, and it is a maintained, widely reviewed one |
| `boot-contracts/Cargo.toml` | Added `sha2 = { version = "0.10.9", default-features = false }` as a non-optional dependency | The hasher every contract module uses unconditionally has an unconditional dependency; `std` stays off for the `no_std`, allocator-free component link |
| `components/runtime/src/lib.rs` | Deleted `mod sha256;` and re-pointed `pub use sha256::sha256;` at `boot_contracts::sha256::digest as sha256` | The component runtime names the same hasher as the boot contracts instead of carrying a second one |
| `components/runtime/src/sha256.rs` | Deleted | A second one-shot implementation with its own duplicated round-constant table no longer exists to drift |
| `Cargo.lock` | Records the new direct `boot-contracts -> sha2` edge; the resolved version stays `0.10.9` | `--locked` builds, which every QEMU gate uses, resolve unchanged |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The new backend produces different digests than the retired implementations | `just test_host` | Any of the eight `sha256::tests` fails against its FIPS 180-4 vector or one-shot reference |
| Streaming state differs across `update` boundaries or at the padding edge | `just test_host` | `every_split_point_agrees_with_one_shot`, `three_way_splits_agree_with_one_shot`, or `the_padding_boundary_is_handled_at_every_adjacent_length` fails |
| `sha2` introduces UB reachable from contract code | `just miri` | Miri reports UB inside `boot-contracts` or `sha2` |
| Persisted formats stop validating (bootstate slots, transfer bundles, recovery roots, boot-layout identities) | `just test_sel4_root`, `just test_host` | A checksum, identity, or root test in `slime-root` or `boot-contracts` fails |
| The re-exported `slime_rt::sha256` disagrees on real aarch64 seL4 | `just sel4_storage_check`, `just sel4_store_check`, `just sel4_transfer_check` | A probe fails its frozen `EXPECTED_DIGEST`, a content-hash `stat` lookup, an append-identity check, or a transfer object digest |
| The dependency graph gains an advisory, license, or unpinned-source problem | `just deny` | `cargo deny` reports a non-ok section |

## Verification

Run from the repository root, in order; each reported success before the next ran.

| Command/scenario | Result | Evidence class |
|---|---|---|
| `cargo check -p boot-contracts --all-features` | Passed; `Cargo.lock` now lists `boot-contracts` dependencies as `ed25519-dalek`, `sha2`, with `sha2` still at `0.10.9` | Direct |
| `cargo test --manifest-path boot-contracts/Cargo.toml --all-features --lib sha256` | Passed; 8 of 306 tests selected, all green — `nist_vectors_match`, `the_long_nist_vector_matches_when_fed_unaligned`, `every_split_point_agrees_with_one_shot`, `three_way_splits_agree_with_one_shot`, `the_padding_boundary_is_handled_at_every_adjacent_length`, `empty_updates_are_transparent`, `default_matches_new`, `distinct_inputs_give_distinct_digests` | Direct |
| `just test_host` | Passed; the full host aggregate including every contract-format checksum and identity test | Direct |
| `just miri` | Passed in 400 s; `boot-contracts --all-features` under Miri reported no UB. `cpufeatures` reports the aarch64 SHA-2 extension absent under `cfg(miri)`, so Miri interpreted `sha2`'s portable compression path, never an `asm!` block | Direct |
| `just test_sel4_root` | Passed; 184/184 across 19 modules, covering `boot_selector.rs`'s `verify_directory_checksum` and `directory_root` | Direct |
| `just sel4_storage_check` | Passed; 9 markers, a component verified sector 0 against the frozen 32-byte `EXPECTED_DIGEST` through a granted block capability, three refusal arms held, and the flushed write survived to the image | Direct |
| `just sel4_store_check` | Passed; 14 markers on the happy path — object retrieved by content hash, append committed at sequence 3 and durable across re-open, scrub verified every payload — plus the damaged-superblock fallback, interrupted-append, GPT-conflict, and no-valid-root refusal arms | Direct |
| `just sel4_transfer_check` | Passed; 11 markers, manifest digest and object closure verified before any write, a tampered manifest failed its digest, staging promoted only on health confirmation, source left byte-identical | Direct |
| `just fmt_check_all` | Passed | Direct |
| `just lint_all` | Passed; clippy with warnings denied across components, boot-contracts, and the seL4 product crates | Direct |
| `just deny` | Passed; advisories, bans, licenses, and sources all ok. Only pre-existing `license-not-encountered` warnings for unmatched allowances | Direct |

## Decisions

- Decision: keep `boot_contracts::sha256`'s exact public shape rather than exposing `sha2::Digest` to callers.
  Rationale: about 25 call sites across `boot-contracts`, `slime-root`, and the components construct a hasher, feed domain-separated fields, and compare a `[u8; 32]`. Preserving `new`/`update`/`finalize` and the plain array output made this a two-file change with zero caller churn, and it keeps `GenericArray` out of contract signatures.
  Rejected alternative: importing `sha2::{Digest, Sha256}` directly at every call site, which would put a `GenericArray` return type and a trait import into every contract module for no benefit.
- Decision: make `sha2` a non-optional dependency of `boot-contracts`, with `default-features = false`.
  Rationale: unlike `ed25519-dalek`, which only `release.rs` needs behind `release-crypto`, `sha256` is used unconditionally by `generation`, `bootstate`, `transfer`, `recovery`, `boot_layout`, `fabric_graph`, and six identity modules. `default-features = false` keeps `std` off, which is required: this crate is `#![no_std]` and links into component binaries with no allocator.
  Rejected alternative: a feature gate, which would have to be enabled by every consumer anyway and would let a `--no-default-features` build fail to compile the contract modules.
- Decision: do not enable `sha2`'s `force-soft` feature.
  Rationale: the default configuration passed Miri unchanged, because `cpufeatures` reports features absent under `cfg(miri)`; `force-soft` would remove the hardware-accelerated backend on every real target for no observed benefit. This was the pre-planned contingency for a Miri failure that did not occur.
  Rejected alternative: enabling it preemptively, which would slow every real aarch64 digest to defend against a hypothetical.
- Decision: `Sha256::new` is no longer `const`.
  Rationale: `sha2::Sha256::new` is not a `const fn`, and no caller used the old `const` qualifier in a const context — every one of them binds `let mut hasher = Sha256::new();` in a normal function body, which `just lint_all` and the full host and QEMU gate set confirm.
  Rejected alternative: keeping a `const fn` constructor by retaining hand-written state, which would defeat the point of the change.

## Open risks and follow-ups

- [ ] `slime-rt`'s SHA-256 now arrives through `boot-contracts`, so the store-plane components link `sha2` where they previously linked a local module. Code size was not measured; if a component's image budget becomes tight, compare `sha2` default against `force-soft` before assuming the facade is at fault.
- [ ] `deps/zutai` carries its own `Sha256` uses; they are a vendored dependency and outside this change.

## Artifacts and provenance

- Focused report: none; this entry is the focused record
- Raw transcript: command output observed directly in the implementation session; no separate frozen transcript was added
- Serial/debugger/model output: the three QEMU plane checks' marker transcripts were observed inline in the gate output summarized under *Verification*; no capture file was retained
- Related roadmap item: none
