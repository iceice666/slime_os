# P5.4.2c — M5.9's recovery reconstruction, in userspace

| Field | Value |
|---|---|
| Date | 2026-08-08 |
| Kind | Change |
| Status | Verified |
| Scope | `components/bins/src/bin/{sel4-recovery-probe,init}.rs`, `components/bins/{Cargo.toml,build.rs}`, `components/bins/src/default_boot_layout.rs`, `contracts/generation/v1/fixtures/sel4-recovery.zti`, `scripts/lib/recovery_index.py`, `scripts/build/{boot_layout,build-generation,build-sel4,build-store-fixture}.py`, `scripts/check/check-sel4-{recovery-plane,boot-layout,gate-controls}.py`, `Justfile` |
| Roadmap | P5.4.2, P5.4, M5.9 |
| Gates | `just sel4_recovery_plane_check`, `just sel4_rollback_check`, `just sel4_store_check`, `just contracts_check`, `just generation_check`, `just sel4_boot_layout_check`, `just sel4_gate_control_check`, `just test_sel4_root`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` |
| Trigger | M5.4 and M5.6 were in userspace; recovery was the last M5 gap with a portable surface |
| Baseline | `recovery::reconstruct` was reachable only from the oracle's kernel, behind syscall gating |

## Summary

A userspace component now performs M5.9 reconstruction: it refuses two corrupt
BootState slots, decodes a signed recovery index, retrieves and re-hashes every
state object in the index's closure from the content-addressed store,
reconstructs a bootable root into both slots at sequences 1 and 2, and converges
when run again.

The other half of M5.9 is containment, and it is the half worth building a plane
for. The exit condition says reconstruction must produce a verified root
*"without modifying any device not named by an explicit capability"*. So the
gate attaches a **second disk** that no capability the component holds names.
The component tries to write it, the root refuses, and the gate hashes the guard
image before and after. The refusal is the capability model — there is no slot
number that reaches that disk — and the hash is what proves nothing happened.

The oracle does this in the kernel behind a syscall gated on `GenerationControl`
plus a selected block capability. Here **the capability is the gate**.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `sel4-recovery-probe.rs` | The plane's subject: refuse, decode, verify, reconstruct, converge, be contained | M5.9's properties hold with the policy above the root |
| `scripts/lib/recovery_index.py` | `build_recovery_index` + `binding_identity`, moved out of `build-generation.py` | One encoder for the product and the fixture |
| `build-store-fixture.py` | A `recovery` variant: two corrupt slots and a signed index | The gate boots a disk in the state recovery exists for |
| generation 26, `SEL4_RECOVERY_LAYOUT`, build wiring | The plane's artifact | The gate boots what it asserts about |

### A generated-file mistake worth recording

The encoder first went into `scripts/lib/boot_contracts.py`, which carries an
`@generated` banner. `just contracts_check` caught it immediately —
`generate-boot-bindings.py --check` reported the file stale — and `just boot_gen`
erased the addition, which is exactly what should happen.

It moved to a hand-written `scripts/lib/recovery_index.py` beside the generated
constants it consumes. The repo rule exists for this: generated files are
outputs, and anything added to one is deleted by the next generator run, quietly
if nothing checks.

### Why the index is trimmed before decoding

The index is read in whole sectors, and `RecoveryIndex::decode` bounds on exact
length — so the zero padding after the record decoded as a truncated index. The
record declares its own total length at `RECOVERY_INDEX_TOTAL_LEN_OFFSET`, so
the component trims to that and validates it against what it read. Getting this
wrong the first time (offset 140, which is `state_first_lba`) produced exactly
the symptom it should: `fail: index decode`.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| **A device no capability names is modified** | the guard disk is hashed before and after, and its signature re-checked | "the ungranted guard disk changed" |
| Recovery runs on a root it should have refused | two corrupt slots must produce `NoValidBootState` | "a corrupt pair produced a root" |
| A malformed index is acted on | decode failure is a hard failure, before any write | "index decode" |
| A closure with a missing object reconstructs anyway | every named object must be present in the store | "a state object the index names is absent" |
| A corrupted state object passes | `get` re-hashes the complete payload | "a state object failed verification" |
| Reconstruction leaves one root | both slots must decode after | "a reconstructed slot does not decode" |
| An interrupted retry diverges | a second reconstruction must produce identical slots | "reconstruction is not idempotent" |
| The reconstructed root is assumed, not read | it is re-selected off the device | "reconstructed root" |
| Reconstruction scribbles elsewhere | GPT, MBR, store region, and the index are compared byte for byte | "reconstruction modified …" |
| The gate loses evidence | `just sel4_gate_control_check`, pinned at 12 markers | a mutated transcript is accepted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_recovery_plane_check` | Pass; 12 markers, guard disk byte-identical | Direct |
| `just sel4_rollback_check`, `just sel4_store_check` | Pass; the structures it shares a partition with are intact | Direct |
| `just contracts_check`, `just generation_check` | Pass; the moved encoder produces the same product index | Direct |
| `just sel4_gate_control_check` | Pass; 19 gates reject mutated transcripts and layouts | Direct |
| `just sel4_boot_layout_check` | Pass; 16 plane layouts match their fixtures | Direct |
| The other eighteen seL4 plane gates | Pass | Direct |
| `just test_sel4_root`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |
| Signature verification on the recovery index | Not covered — see below | — |

## Decisions

- **Decision:** Attach a real second disk rather than assert the refusal from
  the transcript.
  **Rationale:** the marker proves the component asked. M5.9 is a claim about
  what reached the device, and the only honest check is an image comparison. A
  root that logged a refusal and wrote anyway passes the marker.

- **Decision:** Probe the ungranted device through a slot holding no capability,
  rather than adding a "wrong device" capability.
  **Rationale:** that *is* the property. The component holds one block
  capability; there is no slot number that names the guard disk. Manufacturing a
  capability to the second disk in order to have it refused would be testing a
  rights check instead of the capability model.

- **Decision:** Reuse the store fixture's partition and the rollback plane's
  slot layout.
  **Rationale:** recovery reconstructs the same BootState structure the rollback
  plane writes, out of the same object store the store plane opens. Three planes
  on one on-disk layout is the layout being real rather than three fixtures
  drifting.

- **Decision:** One encoder in `scripts/lib/recovery_index.py`, shared by the
  generation builder and the fixture builder.
  **Rationale:** a fixture that encoded the index independently could pass while
  disagreeing with what the product writes — the drift class the Zutai rule
  exists to prevent.

## Open risks and follow-ups

- [ ] **The index is not signature-verified here.** M5.9 says "signed removable
      recovery"; this plane decodes and bounds the index and verifies the state
      closure it names, but the Ed25519 threshold check that makes it *signed*
      lives in `boot_contracts::release` behind the `release-crypto` feature and
      is not wired into the component. Trust is currently "the index that is on
      the disk", which is weaker than the milestone.
- [ ] Reconstruction is not interrupted. M5.9 requires an interrupted incomplete
      reconstruction to be rejected; the two-write sequence is designed for it
      and nothing exercises the interruption, the same gap M5.6's injections
      have.
- [ ] The generation and executable closure is not verified — the oracle's
      `reconstruct` checks parent closure, executable identities, and release
      continuity before writing. This plane verifies the *state* closure only.
- [ ] The guard disk is unpartitioned and never read. A partitioned guard with
      its own store would be a stronger decoy.

## Artifacts and provenance

- Gate output, the full transcript, and the guard-disk comparison:
  [`recovery-check.txt`](recovery-check.txt).
- The BootState structure it reconstructs:
  [`devlog/2026-08-08-p5-4-2c-rollback-plane/`](../2026-08-08-p5-4-2c-rollback-plane/index.md).
- The object store it verifies the closure against:
  [`devlog/2026-08-08-p5-4-2c-object-store/`](../2026-08-08-p5-4-2c-object-store/index.md).
- The recovery index decoder's host tests:
  [`devlog/2026-08-07-p5-4-2-recovery-index/`](../2026-08-07-p5-4-2-recovery-index/index.md).
- Related roadmap item: P5.4.2 in
  [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md).
