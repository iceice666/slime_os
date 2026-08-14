# P5.4.3 (part) — the transfer manifest decoder had no tests

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Verified |
| Scope | `boot-contracts/src/transfer.rs` |
| Roadmap | P5.4.3, P5.4, P5.4.1 |
| Gates | `just test_host`, `just miri`, `just fmt_check_all`, `just lint_all` |
| Trigger | P5.4.3's M6.7 transfer gap, while looking for a slice of the M6 service class that needs neither a block device nor a new root mechanism |
| Baseline | `boot-contracts` at 116 host tests; `transfer.rs` had **zero** |

## Summary

`boot-contracts::transfer` decodes the transfer manifest — the format that
carries a generation, its object and state tables, its release record, and its
metadata across a persistence boundary. It defines seven distinct error variants
and enforces a self-excluding SHA-256 over the whole manifest, and **nothing
exercised any of it**. Thirteen tests now do, host-tested and Miri-clean, with
three fault injections confirmed failing.

## Changes

Thirteen tests added in the crate's existing shape, matching `store_disk.rs`: a
builder producing the smallest manifest that populates every section, one
positive test reading back every field the decoder promises, then a refusal
corpus with one test per contract.

The builder seals the digest through a `seal` helper kept separate from it
deliberately, so a test can corrupt a byte *after* sealing and observe `BadHash`
rather than passing on a stale hash. No production code was changed.

Each test defends a property a plausible bug would break:

| Property | Why it is load-bearing |
|---|---|
| every field round-trips | without it the refusal corpus passes on a decoder that refuses everything |
| a zero parent is *absent* | the rollback chain reads this to decide whether a predecessor exists; conflating zero with a real hash invents one |
| one flipped byte fails the digest | the hash covers the manifest with its own field zeroed, so any byte outside it must be caught |
| wrong version or header size is refused | a future format must not be read with this version's offsets |
| reserved bytes must be zero | reserved space is the extension point; ignoring it silently accepts a newer producer's meaning |
| the offset chain is pinned | each section starts at the end of the one before, and `total_len` must equal the slice handed over — together these stop a manifest naming bytes outside itself |
| counts respect the generation ceiling | refused at the header, before any per-entry offset is computed from them |
| payload flag and offset must agree | a flag with no offset names nothing; an offset with no flag means a producer set one and not the other |
| a state must travel | a state that does not travel has no business in a transfer, and an undefined flag bit means an uninterpretable producer |
| entry padding must be zero | same extension-point reason as the header's |

## Regression guards

`just test_host` and `just miri` already run this crate, so the thirteen are
gated by existing targets. No new gate was registered: per `AGENTS.md` a gate is
worth adding only when it covers something an existing one does not, and these
would have been a second name for the same command.

## Verification

`just test_host` — **129 passed**, up from 116. `just miri` — 129 passed under UB
checking, including all thirteen. `just fmt_check_all`, `just lint_all`,
`just typos`, `just machete` all pass.

Three fault injections, each confirmed to make the suite fail before being
reverted:

| Injection | Result |
|---|---|
| drop the `HEADER_FIELDS_END..HEADER_LEN` reserved-byte check | reserved-byte test fails |
| replace the state-flag validation with `if false` | 10 of 13 pass; state-flag test fails |
| guard the digest comparison with `false &&` | digest test fails |

On this host a failing assertion aborts with `fatal runtime error: failed to
initiate panic, error 5` instead of reporting cleanly. That is a **pre-existing
host toolchain defect**, not a property of these tests; the abort is still an
unambiguous failure signal, and the passing count before it identifies which
test broke.

## Decisions

**Unit evidence, not a QEMU gate.** The manifest's validity is decidable from
bytes alone, which `roadmap/01-foundations.md:141` says must remain unit
evidence. This is the same shape P5.4.2 took for the store superblock.

**No production code touched.** The decoder was already correct on every property
tested; the gap was coverage, not behaviour. Adding an assertion to
`transfer.rs`'s logic would have been inventing scope.

**`seal` separate from the builder.** Folding it in would make every corruption
test re-seal implicitly and quietly turn the digest test into a tautology.

## Open risks and follow-ups

P5.4.3's exit condition is every M6 gap having an observed seL4 gate. This closes
the decoder half of M6.7 as unit evidence only. The service-level behaviour — a
transfer actually crossing a boundary on seL4 — has no mechanism owner:
`slime-root/src/ipc.rs:207-215` answers nine operations with
`Mediation::Unavailable`, and the directory plane alone owns three. So M6.7 stays
open as a whole, as do M6.3 through M6.6, and M6.1's generation-v2 determinism
partial is untouched.

Worth recording because it shaped this slice: the M6.1 partial was examined
first and **not** taken. `boot-contracts::generation` already has four v2 tests
(decode, component ceiling, v3-only rights rejection, stage0 admission), and the
oracle's `spawn_authority.rs` has no determinism assertion to port — so writing
one would have been inventing a property rather than closing a recorded gap.

## Artifacts and provenance

All thirteen tests live in `boot-contracts/src/transfer.rs` under
`#[cfg(test)] mod tests`. Counts quoted above are from `just test_host` and
`just miri` on `aarch64-apple-darwin` at this entry's date. The fault-injection
results were observed by editing `transfer.rs`, running
`cargo test --manifest-path boot-contracts/Cargo.toml transfer`, and restoring
from a copy — no injection remains in the tree.
