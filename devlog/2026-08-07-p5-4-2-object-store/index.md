# P5.4.2 (part) — the object store's crash consistency, made portable

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Verified |
| Scope | `boot-contracts/src/object_store.rs`, `boot-contracts/src/lib.rs` |
| Roadmap | P5.4.2, P5.4, P5.4.1, M5.4 |
| Gates | `just test_host`, `just miri`, `just lint_all`, `just fmt_check_all` |
| Trigger | The remainder of `object_store.rs`'s thirty-two assertions, after the superblock and GPT slices took the parts decidable from bytes alone |
| Baseline | `boot-contracts` at 180 host tests; the object store lived only in `kernel/src/storage/object_store.rs` with **zero** tests |

## Summary

P5.4.1 recorded `object_store.rs` as a total gap and said the part needing a block
device could not move. That turned out to be **half wrong**: `ObjectStore` reads
and writes through a three-method `BlockIo` trait, not a device handle, so an
in-memory disk satisfies it — including one that fails at a chosen write.

Ten tests now cover append/commit, crash consistency at every commit boundary,
slot alternation, monotonic sequence, and content-addressed integrity.

## Changes

The module was moved with one import change and no logic changes:
`crate::block_proto::SECTOR_SIZE` → `crate::store_disk::SECTOR_BYTES`. It sits
behind the same default-off `gpt` feature, because it needs `alloc` and
`gpt::Partition`.

`MemoryDisk` implements `BlockIo` over a `Vec<[u8; 512]>` with a
`fail_write_after` counter. That counter is what makes crash consistency testable:
the commit protocol is write-record-sectors, flush, write-superblock, flush, and
each boundary can now be interrupted in turn.

| Property | Why it is load-bearing |
|---|---|
| an object round-trips through a **reopen** | the entry must be rebuilt from the committed root and record area, not from in-memory state |
| identical content twice is idempotent | content addressing; asserted on `append_lba` so a second record would be caught |
| an interrupted append leaves the previous root committed | the crash-consistency claim, at all three boundaries of a two-sector put |
| both slots hold *different* roots, and the newest wins | destroying the newest must fall back one commit, not to genesis |
| no valid superblock is a refusal | a store that re-genesised here would silently discard every committed object |
| a corrupted payload is caught on read **and** by scrub | the two paths verify independently |
| an intact store scrubs clean | the control for the line above |
| an oversized payload writes nothing | asserted on the write counter, so a rejected put cannot leave a partial record |
| never writes outside its partition | compared byte-for-byte, which is what catches a `first_lba` off-by-one |
| a partition too small is refused | rather than opened into an unusable state |

## Regression guards

`just test_host` and `just miri` already run this crate. No new gate.

## Verification

`just test_host` — **190 passed**, up from 180. `just lint_all`,
`just fmt_check_all`, `just typos`, `just machete` pass.

| Injection | ok-count |
|---|---|
| baseline | 10 |
| skip the read hash check | **8** |
| reuse the same superblock slot instead of alternating | **9** |
| drop the record flush before the superblock write | **10** — not caught |

**The third is recorded as uncovered rather than presented as passing.** An
in-memory disk cannot model flush ordering: a write is durable the moment it lands
in the `Vec`, so removing the flush between the record and the superblock changes
nothing observable. Catching it needs a disk that reorders or discards unflushed
writes — a real property, and the honest next step for this slice.

The slot-alternation test earned its strength the hard way: the first version only
checked that a damaged slot did not produce a *newer* root, and the slot-reuse
injection passed against it. Rewriting it to assert both slots carry different
sequences, and that destroying the newest falls back to the older, is what made it
discriminate.

## Decisions

**Moved, not reimplemented**, for the same reason as the GPT slice: the oracle's
logic is the specification, and a one-import diff is reviewable in a way a rewrite
is not.

**Behind the `gpt` feature rather than a new one.** It needs `alloc` and
`gpt::Partition`; a second feature would let someone select a combination that
does not compile.

**Fixed the test, not the store, when boundary 2 did not fail.** The first loop
assumed four write boundaries; a five-byte payload occupies one record sector, so
a put issues two writes. The payload is now deliberately two sectors and the loop
covers exactly three boundaries. Worth recording because the instinct on a failing
new test is to suspect the code under test, and here the test was wrong.

## Open risks and follow-ups

P5.4.2's exit condition is every M5 gap having an **observed seL4 gate**. This is
unit evidence, and nothing on seL4 calls the store: `slime-root`'s object allocator
skips every device untyped, so there is no MMIO region and no DMA-capable frame to
reach a disk with.

Flush ordering is untested, as above. So is the interaction between GPT recovery
and store opening — `validate_store_partition` and `ObjectStore::open` are tested
separately but never composed, which is the shape a real mount would take.

## Artifacts and provenance

Ten tests in `boot-contracts/src/object_store.rs` under `#[cfg(test)] mod tests`.
Counts from `just test_host` on `aarch64-apple-darwin` at this entry's date.
Injections were made by editing the module, running
`cargo test --manifest-path boot-contracts/Cargo.toml --features gpt
object_store::tests`, and restoring from a copy — none remains in the tree.
