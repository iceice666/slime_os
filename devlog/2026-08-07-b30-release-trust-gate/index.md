# B30 — `release_trust_check` was red, unregistered, and half-blind

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Defect |
| Status | Verified |
| Scope | `scripts/check/check-release-trust.py`, `AGENTS.md` |
| Roadmap | B30 |
| Gates | `just release_trust_check`, `just ruff`, `just typos` |
| Trigger | The previous entry claimed `apply_rotation` was untested; checking that claim found an existing gate for it, and running the gate found it broken |
| Baseline | `just release_trust_check` aborting with `AttributeError` before any assertion; target absent from `AGENTS.md`'s gate index |

## Summary

Three defects in one gate, each hiding the next. The gate **could not run**, so
nobody noticed it was **not registered**, so nobody noticed that its rotation
refusals **never reached the Rust decoder** they were supposed to be testing.

## Observable symptom

```
AttributeError: module 'release_trust' has no attribute 'ROTATION_BYTES'
error: recipe `release_trust_check` failed on line 557 with exit code 1
```

Thirteen `expect_error` cases — signed staging, replay refusal, rotation
continuity, rollback, promotion — were unreachable.

## Investigation log

1. The previous devlog entry recorded `apply_rotation` as an untested follow-up.
   Before acting on that, grepped for existing coverage and found
   `scripts/check/check-release-trust.py`, which builds real Ed25519 rotations.
2. Ran `just release_trust_check`: `AttributeError` on `ROTATION_BYTES`.
   `release_trust.py` re-exports generated constants but omits the four
   `ROTATION_*` names and `MAX_TRUST_KEYS`.
3. Checked `AGENTS.md:61-77`, the canonical gate index. The target is absent.
   That is why a red gate persisted.
4. Fixed the imports; the gate passed. Then tested whether it *discriminates*:
   deleted the `replacement_version != current.version + 1` branch from
   `apply_rotation`. **The gate stayed green.**
5. Read `verify_rotation` (`:181`) — a pure-Python reimplementation of the same
   rules. Only the valid rotation is handed to the `verify_release` example, so
   the three continuity assertions prove the *fixture* is malformed, never that
   `release.rs` refuses it.
6. Routed every refusal through `apply_rotation`. The replacement injection now
   failed. The `previous_version` injection still passed.
7. Read the two continuity fixtures: both vary the **signature counts**
   (`KEY_PATHS[:1]` vs `[:2]`), so neither reaches the `previous_version` branch.
8. Added a stale-previous fixture as `(2, 3)`. Still green — because with
   `current.version == 1`, a replacement of 3 is caught by the *replacement*
   branch first. Corrected to `(2, 2)`, which keeps the replacement valid and
   isolates the branch under test. Now caught.

## Root cause

A missing import made the gate unrunnable; its absence from the gate index let
that persist; and a Python mirror of the decoder's rules meant its negative cases
never exercised the decoder. The third is the substantive one: a check that
reimplements what it verifies can only test its own fixtures.

## Changes

- `check-release-trust.py` loads the generated constants through a new
  `CONTRACTS` handle. Deliberately **not** by widening `release_trust.py`'s
  imports — those names are unused in its own body, which ruff correctly flags
  F401.
- `rust_rotation` / `expect_rust_rotation_refused`: every rotation refusal now
  goes through `apply_rotation` *and* the Python mirror.
- A stale-`previous_version` fixture, `(2, 2)`, with a comment explaining why not
  `(2, 3)`.
- `AGENTS.md` gate index now lists `just release_trust_check`.

## Regression guards

Each continuity branch is guarded by its own fixture, verified by injection:

| Injection | Result |
|---|---|
| delete `replacement_version != current.version + 1` | `apply_rotation accepted version-skip` |
| delete `previous_version != current.version` | `apply_rotation accepted stale-previous` |

Both observed failing, then reverted. That the two injections trip *different*
fixtures is the discrimination the gate previously lacked entirely.

## Verification

`just release_trust_check` — passes, printing "signed staging, replay, rotation,
rollback, and promotion passed". `just ruff` — clean. `just typos` — clean.

## Decisions

**`CONTRACTS` in the check, not re-exports in the library.** Forwarding names a
module does not use is an unused import; the linter is right and the check is the
honest consumer.

**Keep the Python mirror.** It is a second opinion on the fixture, which is worth
having — the defect was that it was the *only* opinion. Both now run.

**`(2, 2)` not `(2, 3)`.** Recorded prominently because the first attempt got it
wrong and still passed under injection, which is exactly the failure mode this
entry is about.

## Open risks and follow-ups

`verify_signatures`, `verify_generation`, and `verify_for_staging` still have no
negative case reaching Rust — the same shape as the rotation defect, in the same
file. `apply_rotation`'s `replacement.validate()?` also has no fixture: deleting
it left the gate green, because every replacement root in the corpus is
well-formed. A fixture with a malformed replacement root would close that.

## Artifacts and provenance

All observations are from `just release_trust_check` on `aarch64-apple-darwin` at
this entry's date. Injections were made by editing
`boot-contracts/src/release.rs`, running the gate, and restoring from a copy — no
injection remains in the tree.
