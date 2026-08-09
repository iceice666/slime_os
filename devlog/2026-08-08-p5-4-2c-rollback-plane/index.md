# P5.4.2c — M5.6's rollback contract, in userspace

| Field | Value |
|---|---|
| Date | 2026-08-08 |
| Kind | Change |
| Status | Verified |
| Scope | `boot-contracts/src/bootstate.rs`, `stage0/src/lib.rs`, `components/bins/src/bin/{sel4-rollback-probe,init}.rs`, `components/bins/{Cargo.toml,build.rs}`, `components/bins/src/default_boot_layout.rs`, `contracts/generation/v1/fixtures/sel4-rollback.zti`, `scripts/build/{boot_layout,build-generation,build-sel4}.py`, `scripts/check/check-sel4-{rollback-plane,boot-layout,gate-controls}.py`, `Justfile` |
| Roadmap | P5.4.2, P5.4, M5.6 |
| Gates | `just sel4_rollback_check`, `just sel4_store_check`, `just sel4_boot_layout_check`, `just sel4_gate_control_check`, `just test_sel4_root`, `just test_host`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` |
| Trigger | M5.4 landed in userspace; rollback was the next M5 gap needing durable slots |
| Baseline | BootState transitions were reachable only from the oracle's `generation_service` |

## Summary

A userspace component now walks the whole M5.6 transition model on two durable
BootState slots on a real device: stage a pending generation with two attempts,
consume both durably (the oracle's `2 → 1 → 0`), find them exhausted, roll back
to known-good, confirm rollback is idempotent, refuse promotion with a wrong
running identity or a stale release sequence, and promote the running
generation.

Every commit is **older-slot-first** — written to the slot the boot did not
select — which is what makes the M5.6 invariant hold: no transition overwrites
the only valid root. The probe re-reads the other slot after each attempt
commit, so a commit that wrote its own slot fails there rather than silently
leaving one root.

The oracle does this in the kernel: `generation_service::rollback_reply`
performs the transition and `persist_transition` writes the alternate slot. Here
the root mediates sectors and the model is `boot_contracts::bootstate` — the
same code stage-0 selects with.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `boot-contracts/src/bootstate.rs` | `Slot`, `SelectedBootState`, `SelectionError`, `select_bootstate` moved here from `stage0` | The selection rule lives beside the record it selects |
| `boot-contracts/src/bootstate.rs` | Six tests for the moved rule | It had **none** in `stage0` |
| `stage0/src/lib.rs` | Re-exports the types, maps two refusals onto `BootError` | One rule, one implementation |
| `sel4-rollback-probe.rs` | The plane's subject: the full transition sequence on disk | M5.6's properties hold with the policy above the root |
| generation 25, `SEL4_ROLLBACK_LAYOUT`, build wiring | The plane's artifact | The gate boots what it asserts about |

### Why the selection rule moved

`select_bootstate` lived in `stage0`, which depends on `uefi` — so a component
could not reach it, and reimplementing twenty-five lines of "highest sequence
wins, one damaged slot tolerated, two valid slots at one sequence is a hard
reject" in the probe would have been two implementations of one contract.

It is not stage-0's rule. A generation-management component applies the same
one. Moving it to `boot-contracts` put it beside `BootState` itself, and
revealed that the rule had **no tests at all** — six now cover highest-sequence
selection from either slot, a tolerated damaged slot, identical slots, the
conflicting-sequence refusal, the no-valid-slot refusal, and `Slot::other` being
the commit target.

### What the fixture shares

The store fixture is reused unchanged: this plane needs a validated GPT
partition, and the BootState slots sit above the object store's record area on
the same partition. The gate proves they do not collide by comparing the object
store's region byte for byte before and after.

A consequence worth knowing: the plane requires the slot region to start empty,
so a second boot against the same image correctly fails at `empty slots
refused`. The gate builds a fresh fixture per run.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A commit overwrites the selected root | every commit targets `slot.other()`, and the probe re-reads the other slot after each attempt | "the previous root was overwritten" |
| A transition never reached the device | committed sequences must be strictly increasing | "committed sequences are not strictly increasing" |
| Writes were acknowledged but not persisted | both slots are checked for the record magic in the host image after the boot | "N of 2 BootState slots carry the record magic" |
| An attempt is consumed after exhaustion | the exhausted refusal is asserted | `attempts exhausted` missing |
| Rollback is not idempotent | a second rollback must return an identical state | "rollback is not idempotent" |
| A component confirms a generation it is not | `promote_pending` with a wrong identity must be refused | "wrong running generation accepted" |
| The accepted release walks backwards | a stale release sequence must be refused | "stale release accepted" |
| A corrupt device boots something | an empty slot region must produce no root | "an empty region produced a root" |
| The transitions scribble elsewhere | the GPT, MBR, store region, and everything past the slots are compared byte for byte | "the transitions modified …" |
| The gate loses evidence | `just sel4_gate_control_check`, pinned at 16 markers | a mutated transcript is accepted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_rollback_check` | Pass; 16 markers, 6 durable transitions at sequences `[1,2,3,4,5,7]` | Direct |
| `just sel4_store_check` | Pass; the store on the same partition is untouched | Direct |
| `cargo test -p boot-contracts --lib --features gpt` | Pass; 197 tests, 6 new for the moved rule | Direct |
| `just sel4_gate_control_check` | Pass; 18 gates reject mutated transcripts and layouts | Direct |
| `just sel4_boot_layout_check` | Pass; 15 plane layouts match their fixtures | Direct |
| The other sixteen seL4 plane gates | Pass | Direct |
| `just test_sel4_root`, `just contracts_check` | Pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |
| M5.6's interruption injections | Not covered — see below | — |
| M5.9 recovery | Not ported | — |

## Decisions

- **Decision:** Move `select_bootstate` to `boot-contracts` rather than
  duplicate it in the probe.
  **Rationale:** two implementations of one selection rule is exactly the defect
  class that produces a bootloader and a service disagreeing about which root is
  live. The move also gave the rule its first tests.

- **Decision:** Re-read after every commit instead of trusting the write.
  **Rationale:** the state the gate reports is what a fresh boot would select
  off the device. A transition model verified against its own return value
  proves the model, not the persistence — and persistence is the half M5.6 is
  actually about.

- **Decision:** Reuse the store fixture's partition rather than a second
  partition for BootState.
  **Rationale:** it matches the oracle's layout, where the boot store and the
  object store share a device, and it makes the non-collision testable: the gate
  compares the store's region byte for byte across the rollback boot.

- **Decision:** Assert strictly increasing sequences in addition to per-marker
  pins.
  **Rationale:** the per-marker pins alone would not catch a no-op commit if two
  adjacent steps happened to expect the same number. The sequence check is
  independent of which numbers are expected.

## Open risks and follow-ups

- [ ] M5.6's **interruption injections** are not covered: interrupt before
      pending metadata, during either slot write, after pending commit, after
      attempt commit but before transfer, during promotion, during rollback
      update, during state snapshot, and during GC. Each needs the write path to
      fail at a chosen point; `boot_contracts::object_store`'s host tests do this
      with a `fail_write_after` mock disk, and the device path has no equivalent.
      This is the largest remaining M5.6 gap.
- [ ] State policies and GC (`immutable`, `ephemeral`, `preserve`,
      `snapshotBeforeUpgrade`, `discardOnRollback`) are not exercised. The
      oracle covers them in `kernel/tests/generation_manager.rs`, and they need a
      generation with state bindings rather than the transition model alone.
- [ ] Health-signal classification — component exit, fault, timeout, peer loss,
      explicit unhealthy — and denying health confirmation to unprivileged
      components are M5.6 requirements this plane does not touch. The
      supervision planes cover the classification half on seL4 already; the
      authorization half needs a generation-management capability.
- [ ] M5.9 recovery is not ported.

## Artifacts and provenance

- Gate output, the full transition transcript, and the image comparisons:
  [`rollback-check.txt`](rollback-check.txt).
- The object store this shares a partition with:
  [`devlog/2026-08-08-p5-4-2c-object-store/`](../2026-08-08-p5-4-2c-object-store/index.md).
- The block path underneath:
  [`devlog/2026-08-08-p5-4-2c-storage-plane/`](../2026-08-08-p5-4-2c-storage-plane/index.md).
- Related roadmap item: P5.4.2 in
  [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md).
