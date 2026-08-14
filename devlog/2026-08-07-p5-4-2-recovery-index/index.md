# P5.4.2 (part) — the recovery index decoder had no tests

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Verified |
| Scope | `boot-contracts/src/recovery.rs` |
| Roadmap | P5.4.2, P5.4, P5.4.1, M5.4 |
| Gates | `just test_host`, `just miri`, `just fmt_check_all`, `just lint_all` |
| Trigger | Auditing `boot-contracts` for modules with real logic and zero tests, after the same audit found `transfer.rs` |
| Baseline | `boot-contracts` at 129 host tests; `recovery.rs` had **zero** |

## Summary

`boot-contracts::recovery` decodes the recovery index — the record naming which
generation to recover to, the LBA span holding its state objects, and a
content-addressed root over every state binding. It defines eight error variants
and enforces a strict ordering rule plus a SHA-256 state root, and **nothing
exercised any of it**. Thirteen tests now do, host-tested and Miri-clean, with
three fault injections confirmed failing.

This is M5.4's recovery-precedence surface as unit evidence. P5.4.1 recorded
`object_store.rs` as a total gap covering "GPT redundancy and recovery
precedence, content-addressed integrity … monotonic sequence", and the index is
the part of that decidable from bytes alone.

## Changes

Thirteen tests in the crate's established shape, matching `store_disk.rs` and
`transfer.rs`: a builder producing the smallest index that exercises ordering as
well as the root (two entries), one positive test reading back every field, then
a refusal corpus with one test per contract. No production code changed.

`seal` recomputes the state root and is kept out of the builder deliberately, so
a test can corrupt an entry *after* sealing and observe `BadStateRoot` instead of
passing against a root recomputed to match.

| Property | Why it is load-bearing |
|---|---|
| every field round-trips | without it the refusal corpus passes on a decoder that refuses everything |
| an empty state table is valid | a generation binding nothing still needs a recovery target; its root is the hash of no input, asserted `!= [0; 32]` so an all-zero root cannot pass as "empty" |
| a corrupted entry fails the root | the root is the integrity link between index and objects; checked on all three covered fields |
| bindings strictly ascend | makes lookup decidable and duplicates structurally impossible — equal is as wrong as descending |
| zero identity or schema version refused | a zero identity names nothing and a zero version is not a version |
| target generation and root non-zero | without them there is nothing to recover *to* |
| an inverted LBA span refused | describes a region no reader can walk; an equal pair is one sector and stays legal |
| short **and** oversized are `Truncated` | `MAX_BYTES` is the upper bound, not just the header the lower |
| wrong version or header size refused | a future format must not be read with this version's offsets |
| required flags and reserved bytes | extension points, each with its **own** error so the two are distinguishable |
| count and length must agree | an index cannot claim entries it does not carry |
| `binding_identity` is domain-separated | asserted against a bare `SHA-256(name)`, so removing the prefix or length is caught |

## Regression guards

`just test_host` and `just miri` already run this crate, so the thirteen are
gated by existing targets. No new gate registered: per `AGENTS.md` a gate is
worth adding only when it covers something an existing one does not.

## Verification

`just test_host` — **142 passed**, up from 129. `just miri` — 142 under UB
checking, including all thirteen. `just fmt_check_all`, `just lint_all`,
`just typos` pass.

Three fault injections, each reverted after being observed. Because the harness
aborts on a failing assertion on this host, the signal used is the count of tests
reporting `ok`:

| Injection | ok-count |
|---|---|
| baseline | 13 |
| drop `entry.binding_identity <= previous` | **12** |
| guard the state-root comparison with `false &&` | **12** |
| drop the domain prefix and length from `binding_identity` | **12** |
| restored | 13 |

Each injection silences exactly one test, which is the discrimination a corpus
needs: a test that fails for every injection is testing the harness, not the
contract.

## Decisions

**Unit evidence, not a QEMU gate.** The index's validity is decidable from bytes
alone, which `roadmap/01-foundations.md:141` requires stay unit evidence. Same
shape as the store superblock and the transfer manifest.

**No production code touched.** The decoder was already correct on every property
tested; the gap was coverage.

**`seal` separate from the builder.** Folding it in would make every corruption
test re-seal implicitly and turn the state-root test into a tautology.

**The empty-table test asserts a non-zero root.** Asserting only `state_count == 0`
would pass against a decoder that skipped the root entirely for empty tables,
which is exactly the shortcut worth catching.

## Open risks and follow-ups

P5.4.2's exit condition is every M5 gap having an observed seL4 gate, including
the store's behaviour under interruption at each append/commit boundary. This
closes none of that: append/commit behaviour, GPT partition validation, and the
five `Mediation::Unavailable` planes all need a block device `slime-root` does
not have — its object allocator skips every device untyped
(`object_allocator.rs`, `descriptor.is_device()`), so it holds no MMIO region and
no DMA-capable frame. M5.4 stays open; only its byte-decidable part is now
defended.

## Artifacts and provenance

All thirteen tests live in `boot-contracts/src/recovery.rs` under
`#[cfg(test)] mod tests`. Counts are from `just test_host` and `just miri` on
`aarch64-apple-darwin` at this entry's date. Fault-injection ok-counts were
observed by editing `recovery.rs`, running
`cargo test --manifest-path boot-contracts/Cargo.toml recovery`, and restoring
from a copy — no injection remains in the tree.
