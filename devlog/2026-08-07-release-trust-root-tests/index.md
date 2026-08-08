# The release record and trust root had no tests

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Verified |
| Scope | `boot-contracts/src/release.rs` |
| Roadmap | P5.4.1, P5.4 |
| Gates | `just test_host`, `just miri`, `just fmt_check_all`, `just lint_all` |
| Trigger | Auditing `boot-contracts` for modules with real logic and zero tests; `release.rs` was the largest at 363 lines |
| Baseline | `boot-contracts` at 142 host tests; `release.rs` had **zero** |

## Summary

`boot-contracts::release` decodes the signed release record and validates the
trust root that authorises it. It defines **eighteen** error variants — the
richest surface in the crate — and nothing exercised any of it. Fifteen tests now
cover the always-compiled half: `Release::decode` and `TrustRoot::validate`.

These are the checks that decide whether a release may boot at all, so leaving
them unexercised meant a quorum bug could not be caught by any gate.

## Changes

Fifteen tests in the crate's established shape: a builder for one well-formed
release, one positive test reading back every field, then a refusal corpus. No
production code changed.

| Property | Why it is load-bearing |
|---|---|
| every field round-trips | without it the refusal corpus passes on a decoder that refuses everything |
| a zero parent is *absent* | the rollback chain reads this to find the first release |
| any other length is `BadSize` | a fixed-size record must not be read with a shifted layout |
| wrong version or header size refused | a future format must not be read with this version's offsets |
| empty or oversized target refused | the target binds a release to hardware it may boot; empty names nothing |
| non-UTF-8 target refused | it is compared as a string, so refusing here avoids a comparison that can never match |
| reserved bytes refused in **all three** regions | target slack, header tail, and signature-area slack are separate extension points |
| a required flag refused | same reason, distinct error |
| over-count signatures refused | more than the record holds cannot be addressed |
| a declared slot admits its own bytes **only** | the pair that proves `signature_count` moves the boundary rather than being ignored |
| threshold within `1..=key_count` | zero would accept an unsigned release; above the count could never be met |
| version and key count non-zero | a version of zero is not a version; a root with no keys authorises nothing |
| duplicate or zero key refused | a duplicate lets **one** signer satisfy a threshold of two, defeating the quorum |
| keys past `key_count` are reserved | a key parked there must not become live when the count later grows |

Signature verification, `verify_generation`, `verify_for_staging`, and
`apply_rotation` sit behind the optional `release-crypto` feature and are **not**
covered here; see follow-ups.

## Regression guards

`just test_host` and `just miri` already run this crate. No new gate registered.

## Verification

`just test_host` — **157 passed**, up from 142. `just miri` — 157 under UB
checking. `just fmt_check_all`, `just lint_all`, `just typos`, `just machete`
pass.

Three fault injections, each reverted after being observed. The signal is the
count of `release::tests` reporting `ok`, because a failing assertion aborts the
harness on this host:

| Injection | ok-count |
|---|---|
| baseline | 15 |
| allow `threshold > key_count` | **14** |
| allow duplicate trust keys | **14** |
| ignore signature-area slack | **13** |

The third silences two tests, which is expected: both the over-count test and
the declared-slot pair rest on that check.

## Decisions

**Only the always-compiled half.** `release-crypto` is optional
(`boot-contracts/Cargo.toml:8`), so covering the signature paths would mean
either building real Ed25519 fixtures under a non-default feature or making the
suite feature-conditional. Neither belongs in a slice whose point was covering
what every build already compiles.

**The declared-slot test is a pair, not a single assertion.** Asserting only that
slack is refused would pass against a decoder that rejected *all* signature bytes;
asserting only that a declared slot is admitted would pass against one that
ignored the area entirely. Both together pin the boundary.

**A zero key reports `DuplicateKey`.** That is the existing decoder's behaviour —
the two conditions share an arm — and the test records it rather than asserting a
tidier error that does not exist.

## Open risks and follow-ups

The `release-crypto` half is untested: `verify_signatures`, `verify_generation`,
`verify_for_staging`, and `apply_rotation`. `apply_rotation` is the most
consequential — it advances the trust root, requires `previous_version ==
current.version` and `replacement_version == current.version + 1`, and demands
both the outgoing and incoming roots sign the same payload. That is a quorum
handover with no coverage, and it needs Ed25519 fixtures to test honestly.

This entry claims no roadmap milestone. `release.rs` is not one of P5.4.1's
recorded gaps; it was found by auditing the crate for untested logic, and the
work stands on its own rather than advancing a slice.

## Artifacts and provenance

All fifteen tests live in `boot-contracts/src/release.rs` under
`#[cfg(test)] mod tests`. Counts are from `just test_host` and `just miri` on
`aarch64-apple-darwin` at this entry's date. Fault-injection ok-counts were
observed by editing `release.rs`, running
`cargo test --manifest-path boot-contracts/Cargo.toml release::tests`, and
restoring from a copy — no injection remains in the tree.

## Corrections

**The follow-up above was wrong: `apply_rotation` was not untested.**
`scripts/check/check-release-trust.py` already built real Ed25519 rotations and
asserted three continuity cases. What this entry should have said is narrower and
worse: that gate was **red** — it aborted with
`AttributeError: module 'release_trust' has no attribute 'ROTATION_BYTES'` before
asserting anything — it was **absent from `AGENTS.md`'s gate index**, and its
rotation refusals only ever ran against a Python reimplementation of the rules,
never against `apply_rotation` itself.

Filed and fixed as B30; see
[`devlog/2026-08-07-b30-release-trust-gate/`](../2026-08-07-b30-release-trust-gate/index.md).

The claim that `verify_signatures`, `verify_generation`, and `verify_for_staging`
lack coverage still stands, and B30 sharpens it: they have no negative case that
reaches Rust, which is the same defect shape in the same file.
