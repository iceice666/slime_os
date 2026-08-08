# The hash primitives everything trusts had no conformance tests

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Verified |
| Scope | `boot-contracts/src/sha256.rs`, `boot-contracts/src/crc32.rs` |
| Roadmap | P5.4.1, P5.4 |
| Gates | `just test_host`, `just miri`, `just fmt_check_all`, `just lint_all`, `just typos` |
| Trigger | Continuing the `boot-contracts` audit for modules with real logic and zero tests, after `transfer.rs`, `recovery.rs`, and `release.rs` |
| Baseline | `boot-contracts` at 157 host tests; `sha256.rs` and `crc32.rs` had **zero** |

## Summary

Both hash primitives are hand-rolled and had no tests. `sha256` computes
generation identity, release signing payloads, recovery state roots, transfer
manifest digests, boot-layout digests, and fabric-graph identity; `crc32` protects
bootstate and the store. Eleven tests now anchor them to published vectors.

The specific risk is that **every other test in this crate compares one of these
digests against another**, so a self-consistent but wrong implementation passed
all 157 of them. Nothing tied either primitive to the real algorithm.

## Changes

Eight `sha256` tests and three `crc32` tests. No production code changed.

| Property | Why it is load-bearing |
|---|---|
| FIPS 180-4 short vectors (`""`, `"abc"`, 448-bit) | the only assertions anchoring SHA-256 to the standard rather than to itself |
| the one-million-`'a'` vector, fed in 7-byte slices | exercises the 64-bit length counter and thousands of block boundaries, with `update`'s buffered path live throughout instead of only aligned blocks |
| every split point agrees with one-shot | `update` has a buffered branch, a `chunks_exact` fast path, and a handoff between them; a mishandled boundary shows up here and nowhere else |
| three-way splits | enters the buffered path *with a partial block already held*, which a two-split test can miss |
| padding boundary at 55/56/64/65 | at 55 the length field shares the final block, at 56 it cannot; off-by-one there yields a plausible digest for most inputs |
| empty `update` is transparent | a caller feeding an empty slice between real ones must not change the digest |
| `Default` matches `new` | both must give the initial state, not a zeroed one |
| distinct inputs differ | including a trailing byte and a transposition |
| CRC-32/IEEE vectors incl. the `0xcbf43926` check value | a wrong polynomial or reflected/normal mix-up is self-consistent and otherwise invisible |
| all 256 byte values in one input | a single bad table entry would otherwise only surface for inputs containing that byte |
| flip, transposition, length change | exactly the corruptions bootstate and the store use CRC to detect |

Every expected digest was generated from Python's `hashlib` and `zlib` rather than
written from memory, then asserted against this implementation.

## Regression guards

`just test_host` and `just miri` already run this crate. No new gate registered.

## Verification

`just test_host` — **168 passed**, up from 157. `just miri` — 168 under UB
checking (1910s; the million-byte vector dominates). `just fmt_check_all`,
`just lint_all`, `just typos`, `just machete` pass.

Three fault injections, each reverted after being observed. The signal is the
count of tests reporting `ok`, because a failing assertion aborts the harness on
this host:

| Injection | ok-count |
|---|---|
| sha256 baseline | 8 |
| padding boundary `> 56` → `> 57` | **3** |
| drop `self.length` accumulation in `update` | **3** |
| crc32 baseline | 3 |
| polynomial `0xEDB88320` → `0xEDB88321` | **1** |

Each injection collapses most of its suite, which is what conformance tests
should do: unlike a structural corpus, there is no partial credit for a hash that
is wrong.

## Decisions

**Reference values come from `hashlib`/`zlib`, not memory.** The four padding-
boundary digests were initially written by hand; all eight were regenerated and
compared before being committed. A conformance test asserting a remembered
constant tests nothing.

**No production code touched.** Both primitives were already correct on every
published vector; the gap was that nothing said so.

**An appended byte rather than a one-letter variant.** The original assertion
compared `"abc"` against a single-letter mutation, which `just typos` flagged as a
misspelling of an English word. Rather than suppress the lint, the assertion was
changed to `b"abc\x01"` plus a transposition (`b"cba"`), which tests the same
property — distinct inputs give distinct digests — without a dictionary word.

## Open risks and follow-ups

`boot-contracts/src/handoff.rs` remains at zero *runtime* tests and needs none:
it is `#[repr(C)]` struct declarations plus a `const _: () = { … }` block asserting
every `size_of` and `offset_of` against the schema-generated constants. That is
strictly stronger than a unit test, and it was verified rather than assumed —
widening `HandoffFramebuffer::bpp` from `u16` to `u32` fails
`cargo build --manifest-path boot-contracts/Cargo.toml` with three errors, and the
tree builds clean again once reverted. With that, no module in this crate has
logic without coverage.

`sha256` has no test for a length crossing `u32`, because `update` accumulates
into a `u64` and reaching that boundary needs 4 GiB of input — not something a
unit test should allocate. The 64-bit counter is exercised to one million bytes.

## Artifacts and provenance

All eleven tests live under `#[cfg(test)] mod tests` in their respective modules.
Counts are from `just test_host` and `just miri` on `aarch64-apple-darwin` at this
entry's date. Fault-injection ok-counts were observed by editing the module,
running `cargo test --manifest-path boot-contracts/Cargo.toml <module>::tests`,
and restoring from a copy — no injection remains in the tree.
