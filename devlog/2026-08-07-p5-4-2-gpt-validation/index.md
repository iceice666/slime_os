# P5.4.2 (part) — GPT redundancy and recovery precedence, made portable

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Verified |
| Scope | `boot-contracts/src/gpt.rs`, `boot-contracts/src/lib.rs`, `boot-contracts/Cargo.toml` |
| Roadmap | P5.4.2, P5.4, P5.4.1, M5.4 |
| Gates | `just test_host`, `just miri`, `just lint_all`, `just fmt_check_all` |
| Trigger | P5.4.2's remaining M5.4 surface, looking for the part decidable from bytes after the superblock slice took the first eight assertions |
| Baseline | `boot-contracts` at 168 host tests; GPT validation lived only in `kernel/src/storage/gpt.rs` with **zero** tests |

## Summary

`kernel/src/storage/gpt.rs` validates the partition table the object store lives
in: protective MBR, both header copies, entry-array CRCs, bounds, overlap, and
store-partition selection. It is 336 lines of pure byte parsing, it had no tests,
and it was reachable only from the frozen oracle.

It is now `boot-contracts::gpt`, host-tested and Miri-clean with twelve tests.
That is M5.4's "GPT redundancy and recovery precedence" — named in P5.4.1's
inventory as part of `object_store.rs`'s total gap — as unit evidence.

## Changes

The module was moved with two import changes and no logic changes:
`crate::block_proto::SECTOR_SIZE` → `crate::store_disk::SECTOR_BYTES`, and
`crate::crc32::crc32` resolves to `boot-contracts`' own (itself only tested as of
this session).

It validates through a `SectorReader` closure rather than a device handle, which
is what makes the whole surface testable in memory — every test here builds a
`Vec<[u8; 512]>` and hands out a reader over it.

| Property | Why it is load-bearing |
|---|---|
| a well-formed disk resolves the store partition | without it the refusal corpus passes on a validator that refuses everything |
| protective MBR required | it is what stops a legacy tool seeing free space on a GPT disk; all three ways to break it |
| either copy alone recovers, and says which | redundancy is the point of two copies, and the report is what lets a caller repair the damaged one |
| both damaged is refused | `NoValidCopy`, distinct from either single-copy path |
| two valid copies that disagree are refused | there is no basis for choosing, so picking either could mount the wrong disk; checked on both compared fields |
| overlapping partitions refused | one LBA with two owners means a write through either corrupts the other |
| a partition outside the usable span refused | it would let the store write over GPT metadata |
| missing vs ambiguous store partition | different errors, because a caller can create the first and must never guess between the second |
| header CRC and entries CRC damage independently | they are separate checks; a table corrupted without touching the header must still be caught |
| a device too small for GPT | refused before any read |
| a failing reader is `Device`, not malformed | only one of those is worth retrying |

## Regression guards

`just test_host` and `just miri` already run this crate. No new gate.

## Verification

`just test_host` — **180 passed**, up from 168, with all twelve `gpt::tests`
present. `just miri` — the same twelve run clean under UB checking.
`just lint_all`, `just fmt_check_all`, `just typos`, `just machete`, `just ruff`
pass.

| Injection | ok-count |
|---|---|
| baseline | 12 |
| `Overlap` → `Ok(())` | **0** (fails to compile: the arm's type changes) |
| drop both disagreement comparisons | **11** |
| ignore `check_pmbr`'s result | **11** |

The two that compile each silence exactly one test, which is the discrimination a
corpus needs.

## Decisions

**Feature-gated, and this was not optional.** The first attempt put
`extern crate alloc` at the crate root unconditionally. That built and tested
fine, then `just lint_all` failed **every component binary** with "no global
memory allocator found": components link `boot-contracts` with no allocator at
all. `gpt` and its `alloc` are now behind a default-off `gpt` feature. `test_host`
and `miri` enable all features, so the tests still run.

This is worth recording because the crate's allocation-free property is invisible
until something violates it — nothing in `lib.rs` said so, and the failure surfaces
three crates away.

**Moved, not reimplemented.** The oracle's logic is the specification; rewriting
it would have replaced a tested-by-nothing implementation with a
different tested-by-nothing implementation. Two import lines changed and nothing
else, so the port is reviewable by diff.

**`kernel/src/storage/gpt.rs` left in place.** The oracle is frozen until
P5.4.final. Removing the duplicate is that slice's work, not this one's.

## Open risks and follow-ups

P5.4.2's exit condition is every M5 gap having an **observed seL4 gate**, and this
is unit evidence only. What still needs a block device `slime-root` does not have:
append/commit behaviour at each write boundary, crash consistency, monotonic
sequence, and the five `Mediation::Unavailable` planes.

The port is not yet *used* by anything on seL4 — no component calls
`boot-contracts::gpt`, because none can reach a block device. It is a decoder
waiting for a caller, the same shape the superblock slice left behind.

## Artifacts and provenance

Twelve tests in `boot-contracts/src/gpt.rs` under `#[cfg(test)] mod tests`.
Counts from `just test_host` and `just miri` on `aarch64-apple-darwin` at this
entry's date. Injections were made by editing `gpt.rs`, running
`cargo test --manifest-path boot-contracts/Cargo.toml gpt::tests`, and restoring
from a copy — none remains in the tree.
