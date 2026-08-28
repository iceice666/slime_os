# B83 — six storage planes leave the root's virtio-blk path

| Field | Value |
|---|---|
| Date | 2026-08-28 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/block-authority/v1/`, `boot-contracts/src/block_authority.rs`, `slime-root/src/{ipc,generation}.rs`, `slime-root/src/graph_runtime/services.rs`, `components/lib/src/block_io.rs`, `components/services/virtio-blk-driver/`, six compositions and their clients and gates, `components/system/init/src/{main,dispatch}.rs`, `scripts/build/{boot_layout,generation_resources,build-generation}.py` |
| Roadmap | B83, B84 |
| Gates | `just sel4_storage_check`, `just sel4_store_check`, `just sel4_rollback_check`, `just replay_check`, `just sel4_generation_check`, `just sel4_filesystem_check`, `just io_block_check`, `just sel4_gate_control_check` |
| Trigger | B83: IO2 proved a userspace virtio-blk driver matched the root's block behaviour, but the root's implementation was still the product path for every storage-family plane. |
| Baseline | Before this entry the root served every admitted component's block request through `console.rs::serve_block_transact`, gating each one on the badge-derived caller's own `BlockDevice` capability. |

## Summary

Six of the eight storage-family planes now reach their devices through the
supervised userspace `virtio-blk-driver` over IO0 rings instead of the root's
synchronous `BlockTransact`. The prerequisite that blocked three earlier
attempts was authority, not plumbing: an IO0 ring is shared memory, so a
submission carries a request, a buffer slice, and a lease but no rights
identity, and a driver serving a ring has no caller badge to derive one from.
`contracts/block-authority/v1` closes that by declaring each client ring's
rights in the generation; the driver reads the table through the root's
identity-gated cursor-paged path and answers `STATUS_BAD_RIGHTS` for a write on
a read-only ring — a status `io-queue/v1` has always defined and nothing had
ever produced.

Two planes did **not** migrate. `sel4-recovery` and `sel4-transfer` each need
two attached devices, and IO1 grants exactly one device per driver instance. The
root's `virtio_blk.rs` and `serve_block_transact` therefore remain, and B83's
deliverable "remove the root's virtio-blk command/descriptor implementation from
the product path" is still unmet. That residue is recorded as B84 with its
mechanism analysis rather than forced.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/block-authority/v1/` | New Zutai schema: one entry binds one holder, on one device, to one ring, with independent read/write bits and a sector ceiling | A ring's rights are declared by the generation, not asserted by a submission |
| `boot-contracts/src/block_authority.rs` | Bounded decoder, 15 host tests, ordering strictly ascending on `(device, ring)` alone | Two holders naming one ring is unrepresentable, so a driver can always say whose rights a submission carries |
| `slime-root/src/ipc.rs` | `read_block_ring_authority` (label 69): identity gate on the declared block driver, cursor-paged canonical entry bytes | The root authenticates who may read the table and bounds the bytes; it reads no block right |
| `slime-root/src/ipc.rs` | `resolve_notification_slot` accepts `notification:@<ordinal>+<role>` | A service declared twice can resolve its own bindings without naming a grant |
| `components/lib/src/block_io.rs` | One shared synchronous `BlockIo` client over IO0 + `block/v2` | Eight probes stop hand-rolling cursor discipline, lease bookkeeping, and DMA direction |
| `components/services/virtio-blk-driver/` | Reads the authority table at start; refuses an unauthorized submission before admission | The gate the root applied per request is reapplied per ring, in userspace |
| Six compositions | `block` grants deleted; ring + driver + `blockRingAuthority` declared | Each client's rights are preserved exactly, not widened |
| `scripts/build/generation_resources.py` | `build_block_ring_authority`, sorted and deduplicated on `(device, ring)` | The encoder and the decoder agree on the ordering that makes duplicates impossible |

Three defects in the IO2 driver surfaced only once a *synchronous* client used
it, because the batch client had masked them:

- the virtqueue used-ring index was compared against zero rather than a retained
  cursor, so every request after the first read its predecessor's completion;
- `FLUSH` built a three-descriptor chain with a zero-length data descriptor,
  which virtio rejects — the batch client never issued a flush;
- the driver was single-pass, answering one drain and then parking, so a client
  submitting one request at a time was served once and then wedged.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A ring reaches rights its generation did not declare | `just sel4_storage_check` | `ungranted slot refused` marker absent |
| Two holders share one ring | `cargo test -p boot-contracts block_authority` | `two_holders_cannot_share_one_ring` fails |
| A gate silently loses an arm | `just sel4_gate_control_check` | Marker-count pin mismatch |
| Slot placement disagrees across its three statements | `just contracts_check` | `check-boot-layout-resource.py` reports resolver/fixture disagreement |
| A driver leaks a DMA mapping or lease | every migrated plane gate | numeric `SLIME_IO reclaim` parse: nonzero pre, exact reclaim, zero post |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_storage_check` | pass, 12 markers, host-side durability confirmed | Direct |
| `just sel4_store_check` | pass, 17 markers | Direct |
| `just sel4_rollback_check` | pass, 19 markers | Direct |
| `just replay_check` | pass, cross-boot determinism retained | Direct |
| `just sel4_generation_check` | pass, 21 markers | Direct |
| `just sel4_filesystem_check` | pass, 14 markers | Direct |
| `just io_block_check` | pass, IO2 parity unbroken | Direct |
| `just sel4_recovery_plane_check`, `just sel4_transfer_check` | pass on the retained root path | Direct |
| `just sel4_gate_control_check` | 45 gates reject 1779 mutated transcripts, up from 1761 | Direct |
| `just test_sel4_root` | 207/207 | Direct |
| `just contracts_check`, `test_host`, `lint_all`, three `fmt_check*`, `ruff`, `typos`, `devlog_check`, `generation_check`, `component_crate_split_check`, `sel4_qemu_image_check`, `sel4_boot_check`, `sel4_boot_layout_check`, `sel4_capability_layout_check` | pass | Direct |

## Decisions

- **Decision:** Order the authority table on `(device, ring)` alone, not
  holder-first.
  **Rationale:** A ring is one client's channel, so `(device, ring)` identifies
  it globally and a strictly ascending sequence makes two rows for one ring
  unrepresentable.
  **Rejected alternative:** Holder-first ordering, which would have permitted
  two holders to name one ring — leaving the driver unable to say whose rights a
  submission carries, the exact defect the table closes.

- **Decision:** The driver looks the authority up by `(device, ring)`, never by a
  holder it was told.
  **Rationale:** The ring names its client, because the generation grants exactly
  one client that ring's buffer.
  **Rejected alternative:** Asking by holder, which would require learning the
  holder from somewhere — and the only available source is the submission, i.e. a
  client asserting its own authority.

- **Decision:** Client liveness comes from the ring's `driver_state`, not from a
  bounded wake count.
  **Rationale:** `notification_wait` is `seL4_Wait` and blocks; a counter over
  waits never advances against a driver that stopped signalling, so the bound
  would be unreachable exactly when needed.
  **Rejected alternative:** Counting waits, which review showed parks forever on
  the first iteration against a faulted driver.

- **Decision:** Leave `sel4-recovery` and `sel4-transfer` on the root path.
  **Rationale:** Both need two devices; IO1 grants one per driver instance. A
  partial cutover leaves the root half-emptied and some clients redirected, which
  is worse than either end state.
  **Rejected alternative:** Giving `device` grants their own positional index.
  This was attempted and reverted: `declared_capability` passes one index to
  every non-block kind, so incrementing on `Device` shifted each driver's
  `mmioRegion` index and broke every plane's virtio handshake at once.

## Open risks and follow-ups

- [ ] B84: two-device planes cannot leave the root until a typed capability's
      device is declared rather than inferred from position. Blocks the rest of
      B83's deliverable.
- [ ] The root's `virtio_blk.rs` and `console.rs::serve_block_transact` remain
      the product path for the two unmigrated planes. They are not dead code and
      must not be removed until B84 closes.
- [ ] Every migrated plane is QEMU trusted-DMA. No IOMMU, so no containment
      claim is made for a userspace driver programming a device.

## Artifacts and provenance

- Focused report: this entry
- Raw transcript: the plane gates above regenerate their own serial transcripts
- Serial/debugger/model output: `just sel4_storage_check` prints the marker
  chain and the host-side image comparison
- Related roadmap item: [`roadmap/11-io-substrate.md`](../../roadmap/11-io-substrate.md), backlog B83 and B84
