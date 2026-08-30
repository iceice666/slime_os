# B84: two driver instances of one executable, one device each

| Field | Value |
|---|---|
| Date | 2026-08-28 |
| Kind | Change |
| Status | Verified |
| Scope | IO1 device authority (`slime-root/src/io_resource.rs`, `slime-root/src/graph_runtime/services/io_resource.rs`, `slime-root/src/ipc.rs`), `contracts/io-resource/v1`, `contracts/block-authority/v1` numbering, `contracts/generation-manifest/v1` budget schema, the `virtio-blk-driver` and its runtime surface, and the `sel4-recovery` and `sel4-transfer` planes end to end |
| Roadmap | B84, B83 |
| Gates | `just sel4_recovery_plane_check`, `just sel4_transfer_check` |
| Trigger | B83 left `sel4-recovery` and `sel4-transfer` on the root's `BlockTransact` path because neither could be expressed against the userspace driver |
| Baseline | All eight block-holding planes except these two reached storage through the userspace driver; these two held two block capabilities each and were served by `console.rs::serve_block_transact` |

## Summary

IO1 granted one device per driver instance, and it did so by fixing three of the four
device-touching identities to literals. `install_driver` used `DeviceId(1)`,
`MmioRegionId(1)`, and `IrqSourceId(1)` unconditionally, and every arm of
`serve_io_resource` that needed a device reached for `DeviceId(1)` again. The only
place a device identity was ever *derived* was the wrong place: the caller's own
capability byte, which is a positional index the root assigns per instance, so two
instances of one driver executable carry identical bytes and cannot be told apart.
There was also a latent IRQ defect: bindings were stored per MMIO granule, while QEMU
puts two virtio disks in one 4 KiB granule, so the second device's bind could not have
succeeded. Both planes now declare two driver instances; each instance's device is
stated in its IO1 budget and threaded through every operation. The planes pass with
their read-only disks refusing writes by the driver's ring authority, and the root's
block path is now unreachable from every seL4 composition.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/io-resource/v1/schema.zt` | `BudgetEntry` gains `device`; entry grows 60→64 bytes | Which transport an instance drives is stated, not inferred; the record was already keyed by authenticated instance identity |
| `boot-contracts/src/io_resource.rs` | `DriverQuota.device`; `decode` refuses two holders naming one device (`DuplicateDevice`); `validate_against` refuses `device >= maxima.device` | A transport is exclusive; an ordinal beyond the platform's device count names nothing |
| `slime-root/src/graph_runtime/services/io_resource.rs` | `install_driver` derives `DeviceId`, `MmioRegionId`, and `IrqSourceId` from the declared ordinal; every former `DeviceId(1)` literal in `serve_io_resource` now reads `service.table.device(driver)`; the adapter keys IRQ slots per device rather than per granule; `BIND` answers the installed zero-based ordinal via `checked_sub` | Every per-device identity comes from the authenticated install, never from a positional byte or a caller word |
| `slime-root/src/io_resource.rs` | New `ResourceTable::device(driver)`; host test `two_drivers_each_hold_their_own_device` | Two instances install cleanly on distinct devices; shared resource identities across drivers are refused |
| `slime-root/src/ipc.rs` | `read_block_ring_authority` gates on the declared EXECUTABLE name and returns only the caller's own device's rows, with the cursor counting filtered rows | Each driver reads exactly the authority it enforces; a second block driver still requires its own declared name |
| `components/runtime` + `virtio-blk-driver` | `io_device_bind` surfaces the device; the driver keys authority lookups on the root's answer instead of a constant | Both instances run identical bytes and differ only in what the root tells them |
| `blockRingAuthority.device` | Unified to zero-based across all eleven compositions, matching the budget's numbering | One convention across adjacent manifest records |
| `sel4-recovery.zti` / `sel4-transfer.zti` | Two driver instances each (`virtio-blk-primary`/`virtio-blk-guard`, `virtio-blk-receiver`/`virtio-blk-source`), each with its own peer endpoint, hardware grant set, notification triple, IO budget, and ring-authority row | Read-only disks are read-only rings; the two disks are exclusive devices |
| `components/system/init/src/dispatch.rs` | Recovery and transfer pass their crossing factory grants | Spawned clients receive what the generation declared for them |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Two instances aliasing one transport | `cargo test -p slime-root --lib two_drivers_each_hold_their_own_device`; `decode`'s duplicate-device scan | Second install returns `Duplicate` or `WrongDevice` |
| A device ordinal beyond the platform | `validate_against` at admission | `UnsatisfiableIoResourceBudget` before any driver boots |
| A driver reading another device's authority rows | Per-device filter in `read_block_ring_authority` | Row count per instance is exactly its own device's rows |
| The guard/source disks becoming writable | `just sel4_recovery_plane_check`, `just sel4_transfer_check` | Whole-image SHA-256 change or a write accepted |
| The six already-migrated planes regressing | Their six gates plus `io_block_check`, `io_driver_authority_check`, `io_link_check` | Any gate failing after the numbering unification |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `cargo test -p boot-contracts io_resource` | 5 passed (includes two new device tests) | Direct |
| `cargo test -p slime-root --lib` | 208 passed across 19 modules (B23 pin updated 207→208 with rationale) | Direct |
| `just sel4_transfer_check` | PASS; source byte-identical, receiver changed only in the two BootState sectors, write refusal produced by driver rights | Direct |
| `just sel4_recovery_plane_check` | PASS; guard readable, write refused with `STATUS_BAD_RIGHTS`, guard SHA-256 unchanged, signature present | Direct |
| `just io_block_check`, `io_driver_authority_check`, `io_link_check`, `sel4_storage_check`, `sel4_store_check`, `sel4_rollback_check`, `replay_check`, `sel4_generation_check`, `sel4_filesystem_check` | All PASS after the numbering unification | Direct |
| `just sel4_gate_control_check` | 45 gates reject 1781 mutated transcripts and layouts (was 1779); recovery and transfer pins moved 11→12 with rationale | Direct |
| Frozen CP1 fixture unaffected | `valid.zti` declares no `ioResourceBudget`; the 60→64 resize cannot reach it | Direct |

## Decisions

- **Decision:** The device ordinal lives in the IO1 budget record, keyed by authenticated
  instance identity, rather than on the grant record or derived from declaration order.
  **Rationale:** The budget record is already the exact record `install_driver` consumes,
  so the ordinal is declared where it is used. A grant-record field would need one ordinal
  repeated four times per instance; declaration order is exactly what the reverted attempt
  proved cannot be trusted.
  **Rejected alternative:** Growing the generation v5 grant record from 32 bytes with a
  new ordinal field — a wire-format change to the most-decoded record in the system for
  information the budget already carries.

- **Decision:** One-based `DeviceId` inside the root; zero-based in the manifest, the
  budget, the `BIND` reply, the authority table, and the driver.
  **Rationale:** Zero means "no device" throughout the resource table and
  `declare_quota` refuses it, so one-based identities can never be mistaken for absent
  ones. The `BIND` reply converts with `checked_sub` rather than `- 1`.
  **Rejected alternative:** Zero-based everywhere, which would make a legitimate device 0
  indistinguishable from "no device" inside the table.

- **Decision:** `blockRingAuthority.device` moved from one-based to zero-based across all
  eleven compositions.
  **Rationale:** Two numbering conventions in adjacent manifest records is a defect
  waiting to be written; the driver looks its rings up against the ordinal the budget
  declared, so both must speak one language.
  **Rejected alternative:** Bending the root's filter to compare `device + 1`, which would
  have preserved the inconsistency and pushed the conversion into the most security-
  sensitive check.

- **Decision:** IRQ slots are keyed per device, not per granule.
  **Rationale:** QEMU places two virtio disks in one 4 KiB granule at `0xa003e00` and
  `0xa003c00`. Per-granule keying made the second device's bind fail as "already bound"
  while holding the first device's interrupt; per-device keying with SPIs derived from
  each transport's own physical address gives two distinct interrupts.
  **Rejected alternative:** Leaving the granule keying and hoping the planes stay
  single-device.

## Open risks and follow-ups

- [ ] B83's residue is now unblocked and is the only open item naming the root's block
      path: `slime-root/src/virtio_blk.rs`, `console.rs::serve_block_transact`, the
      `ConsoleKind::BlockTransact` label, and the runtime wrappers are dead product code
      in every seL4 composition. Deleting them is a deliberate cutover with its own
      gate consequences, not part of this entry.
- [ ] The three x86-reference components still calling `block_transact*` live in the
      frozen CP1 fixture `valid.zti`, not in any seL4 composition. Their fate is tied to
      that fixture's own migration, recorded with the CP1 work.
- [ ] The boot selector's pre-admission bootstrap-device read path remains the one
      acknowledged pre-decode exception; it does not use the device-ordinal machinery.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: the two plane-migration agents (`TransferPlane`, `RecoveryPlane`) ran
  their gates to green; Main re-ran both gates independently after.
- Serial/debugger/model output: the recovery and transfer gate transcripts show the
  driver-produced rights refusal markers and the host-side disk invariants.
- Related roadmap item: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md) B84
  (resolved), B83 (residue narrowed).

## Corrections

- 2026-08-29: this entry's residue item above — "B83's residue is now unblocked
  and is the only open item naming the root's block path ... Deleting them is a
  deliberate cutover with its own gate consequences, not part of this entry." —
  is closed. That cutover landed on 2026-08-29, deleting
  `slime-root/src/virtio_blk.rs`, `console.rs::serve_block_transact`, the
  `ConsoleKind::BlockTransact` label, and the runtime wrappers. The body above is
  left as written; see
  [`devlog/2026-08-29-b83-root-block-path-deleted/`](../2026-08-29-b83-root-block-path-deleted/index.md).
