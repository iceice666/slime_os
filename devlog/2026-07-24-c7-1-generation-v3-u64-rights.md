# C7.1 — Generation format v3 and u64 rights

| Field | Value |
|---|---|
| Date | 2026-07-24 |
| Status | Verified |
| Scope | Generation wire format, capability rights, spawn-grant ABI, host builder/checkers |
| Trigger | Roadmap C7 decomposition; C7.1 lands the v3 foundation later C7 slices consume |
| Baseline | Generation format v2 with a flat `u32` rights bitset (bits 0-23 used) |

## Summary

C7 needs a `SharedBufferFactory` object plus lifecycle rights that would exhaust
the 24-used bits of the `u32` rights bitset. C7.1 introduces generation format
v3, which widens the capability-grant `rights` field to a little-endian `u64`,
and widens the kernel capability rights model (`Rights = u64`) and the
`SpawnGrant` syscall ABI to match. Format v2 remains decodable for the bounded
rollback window: `Generation::decode` selects the record layout and
authority-manifest hash width from the format version, so a retained v2
known-good generation still decodes and its signed release still verifies. The
grandfathered generic `RIGHT_MAP` (bit 9) is renamed to the object-specific
`RIGHT_BUFFER_MAP`. No new rights bits are gated yet — those arrive with the
`SharedBufferFactory` object in C7.2.

## Observable symptom

Not a regression; this is planned foundation work. Exit condition from the
roadmap: a v3 generation is byte-identical across two builds, boots the vertical
slice with `u64` rights, and a retained v2 known-good artifact still decodes and
boots; unsupported versions and unknown rights bits fail closed.

- Command: `just generation_check`, `just contracts_check`, `just transfer_check`, `just test`
- Expected: deterministic v3 builds; v2 retained decode; QEMU vertical slice boots
- Observed: all pass (see Verification)

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `u32` rights bitset has only bits 24-31 free; C7 needs a new object family with several lifecycle rights | Widen rights to `u64` and bump the generation format |
| 2 | v2 and v3 generation headers and all record sizes are identical; only the grant record's internal layout differs (rights `u32`@+12 / transferable@+16 vs rights `u64`@+12 / transferable@+20, both in a 32-byte record) | A single decoder can serve both versions by branching grant decode on the format version |
| 3 | `authority_manifest_identity` hashes `rights` at its native width; a naive widening would break retained v2 release verification | Branch the hashed rights width by version (v2 → 4 bytes, v3 → 8 bytes) in both the Rust decoder and the Python `release_trust.py` helper |
| 4 | `build-transfer.py` only ever ingests a freshly built v3 source generation and unpacks it with the v3 `GENERATION_GRANT` struct | Its authority hash is v3-only; a v2 branch there would be dead code that raises on the widened field (reviewer P1) |

## Root cause

Not a defect. The violated constraint was capacity: the `u32` rights bitset
cannot express C7's shared-buffer authority without colliding with existing
bits, and existing meanings must not change within a format version.

## Changes

| Area | Change | Restored/established invariant |
|---|---|---|
| `contracts/generation/v3/` | New Zutai schema + renderer; grant `rights` is 8 bytes, `transferable` at +20, record stays 32 bytes | Zutai remains the single source of truth for the wire layout |
| `boot-contracts/src/generation.rs` | Dual-version decode (`MAGIC_V3`/`MAGIC_V2`), `version` field, version-branched `grant()` and `authority_manifest_identity`; `Rights = u64`, `Grant.rights: Rights` | Retained v2 generations decode and verify unchanged |
| `kernel/src/capability/mod.rs` | `Rights = u64`; all `RIGHT_*` and `Capability`/`derive`/`valid_rights` use it; `RIGHT_MAP` → `RIGHT_BUFFER_MAP` | Object-specific naming rule satisfied; rights carry >32 bits |
| `kernel/src/task,syscall,bootstrap` | `SpawnGrant.rights: Rights`; syscall grant-array size and directory rights reads widened to `u64` | Kernel ABI carries the widened rights end to end |
| `components/runtime`, `components/bins` | `Rights` alias re-exported; `SpawnGrant`, `init`, `spawn-service` rights widened | Userspace grant ABI matches the kernel |
| `scripts/{release_trust,build-transfer,check-contracts,generate-boot-bindings,check-no-storage-authority}.py` | v3 bindings; version-aware host authority hash; v3 invalid-layout guard; boot-contracts lib tests wired into `contracts_check`; `RIGHT_BUFFER_MAP` allowlist | Host builder/checkers agree with the Rust decoder |
| `docs/capability-matrix.md` | Rights documented as `u64`; `BUFFER_MAP` row; rule 8 covers v3 + retained v2; `RIGHT_MAP` debt cleared | Matrix tracks the object/rights surface in the same change |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Non-deterministic v3 build | `just generation_check` | `cmp` mismatch between two builds |
| Retained v2 stops decoding / verifying | `boot-contracts` lib tests via `just contracts_check`; `just transfer_check` | `retained_v2_generation_still_decodes` fails; transfer `BadClosure` |
| Unsupported version / bad magic accepted | `boot-contracts` lib tests | `unsupported_version_fails_closed` / `wrong_magic_fails_closed` fails |
| Rights surface drift | `just framework_safety_check` | capability rights allowlist mismatch |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just generation_check` | pass (byte-identical gen-1/gen-2/boot-store) | Direct |
| `just contracts_check` (incl. 12 boot-contracts lib tests) | pass | Direct |
| `just transfer_check` | pass (install, pending boot, promotion, rollback) | Direct |
| `just test` (QEMU) | pass (exit 0; vertical slice boots v3) | Direct |
| `just fmt_check` / `just fmt_check_components` | pass | Direct |
| `just lint` / `just lint_components` | pass (`-D warnings`) | Direct |
| `just framework_safety_check` | pass | Direct |

## Decisions

- Decision: Keep v2 and v3 record sizes identical; branch only the grant field layout and hashed rights width on the decoded format version.
- Rationale: A single decoder path with one `version` discriminant is simpler and less error-prone than a parallel v2 module, and keeps the retained-rollback path exercised by the same tests.
- Rejected alternative: Bump only the magic and reinterpret the same 32-bit rights field as zero-extended — rejected because it cannot express bits ≥32 that C7's object family needs.
- Decision: `build-transfer.py` hashes rights v3-only (no version branch).
- Rationale: It only ever ingests a freshly built v3 source generation; a v2 branch is unreachable dead code that would raise on the widened field (reviewer P1).

## Open risks and follow-ups

- [ ] C7.2 must add the `SharedBufferFactory` object and gate shared-buffer creation; C7.4 subsequently gates `RIGHT_BUFFER_MAP`/`RIGHT_BUFFER_WRITE`, which remain ungated per `docs/capability-matrix.md`.
- [ ] The v2 rollback window is bounded but not yet time/generation-limited in code; retention is currently unconditional decode support.

## Artifacts and provenance

- Related roadmap item: `roadmap/02-core-runtime.md` (C7.1)
- Reviewer verdict: `history://C71Review` (P1 on `build-transfer.py`, applied)
- Wire contract: `contracts/generation/v3/`
