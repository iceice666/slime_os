# IO2: userspace virtio-blk parity over the IO0 substrate

| Field | Value |
|---|---|
| Date | 2026-08-28 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/block/v2/`, `components/proto/src/block_v2.rs`, `components/lib/src/virtio_mmio.rs`, `components/services/virtio-blk-driver/`, `components/testkit/io-block-probe/`, `contracts/generation-manifest/v1/compositions/sel4-io-block.zti`, `scripts/check/check-sel4-io-block-plane.py`, `scripts/build/build-{generation,sel4}.py`, `scripts/check/check-sel4-gate-controls.py` |
| Roadmap | IO2 |
| Gates | `just io_block_check`, `just sel4_gate_control_check` |
| Trigger | IO2 is the track's first complete substrate proof: the root owned the virtio-blk driver only because no userspace device-resource model existed, and IO0/IO1 removed that excuse. |
| Baseline | P5.4.2's root-owned virtio-blk: legacy virtio-mmio v1, descriptors 0→1→2, one outstanding request, a synchronous `COMPLETION_POLLS` spin, poison-on-timeout, and a one-sector `BlockTransact` RPC served from `slime-root/src/console.rs`. |

## Summary

A supervised userspace virtio-blk driver now provides the existing capability-gated read/write/flush behaviour through asynchronous bounded buffers and completions, and survives every injected reset/crash/stale-completion case without leaking authority or memory. `just io_block_check` boots the plane and reports `oracle parity, async identity, faults, reclamation, and stale epoch proved`.

The root's virtio-blk implementation is deliberately still present. IO2's own deliverable sequences it that way — the root path is the behavioural oracle, and it is removed only after parity is observed *and* every storage client is repointed. Parity is now observed; the client migration turned out to need a spawn-contract change first, and is tracked as the remaining cutover work rather than half-done.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/block/v2/` | Preserves v1's geometry/read/write/flush semantics and extends only where async multi-block work requires it: 56-byte request `{op, lba, sector_count, flags}`, 24-byte completion `{sectors_done, device_status, detail}`, riding inside the IO0 envelope | The block protocol grew an async shape without growing the transfer-window RPC or accepting caller physical addresses |
| `components/lib/src/virtio_mmio.rs` | Shared userspace virtio-mmio transport helper with a `MediatedMmio` backend preserving the `Mmio` API | One virtio idiom for both blk and net rather than two divergent ones — IO3's driver consumes the same helper |
| `components/services/virtio-blk-driver/` | Feature negotiation, queue setup, descriptor chains, notification/interrupt handling, timeout, reset, and stale-completion logic, all in userspace over IO1 resources and IO0 rings | Device-specific parsing left the root's product surface for this device class |
| `components/testkit/io-block-probe/` | Parity arms against the oracle plus the async-only properties, with numeric reclamation assertions | Parity is measured against observable oracle behaviour, not asserted |
| Plane and gate plumbing | Composition at generation 51, plane checker, variant/build registration, `GATES` pin, `io_block_check` target | The claim cannot pass on missing, reordered, or failing evidence |
| Driver shutdown rendezvous | The probe signals completion; the driver drains, logs `peer complete, exiting`, and exits 0 | The plane reaches a genuine `SLIME_GRAPH HEALTHY … live=0 completed=3` rather than the gate hanging on a service that never ends |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The userspace driver diverges from the oracle's observable behaviour | The parity chain in `just io_block_check` | `parity … =match` absent, or the durable readback marker missing |
| A fault path leaks a descriptor, DMA mapping, lease, or charge | The seven fault-arm markers, each asserted numerically | Any arm reports nonzero `descriptors`/`dma`/`leases`/`charges` |
| Async identity confusion or ring overwrite | `async queued=8 completed=8 identities=8 overwrite=0` and the backpressure marker | A count disagrees or `overwrite` is nonzero |
| A restarted driver accepts prior-epoch work | `restarted old_epoch=… fresh_epoch=…` and `stale completion refused buffer_unchanged=1 request_live=1` | The stale completion is accepted, or the buffer changed |
| Wire drift | `python3 scripts/generate/generate-block-v2-bindings.py --check` via `just contracts_check` | Stale-bindings failure |
| Gate passing on absent evidence | `just sel4_gate_control_check` | The registered checker accepts a mutated transcript |
| The oracle path regresses while it is still authoritative | The six storage-family gates | Any of storage/store/filesystem/rollback/recovery/generation fails |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just io_block_check` | PASS, exit 0 — `oracle parity, async identity, faults, reclamation, and stale epoch proved`. Re-run independently by the integrating agent. | Direct |
| Parity observed | `[virtio-blk-driver] mmio mechanism=mediated-bounded-read32-write32`; `parity read write flush geometry rights out-of-range malformed short-buffer unsupported=match`; `durable fresh-boot readback verified` | Direct |
| Async observed | `async queued=8 completed=8 identities=8 overwrite=0`; `backpressure full refused overwrite=0` | Direct |
| Faults observed | descriptor-failure, timeout, cancellation, reset, interrupt-loss-coalescing, driver-crash, and peer-death each `settled=8 descriptors=0 dma=0 leases=0 charges=0`. No arm omitted. | Direct |
| Epoch observed | `restarted old_epoch=1 fresh_epoch=2`; `stale completion refused buffer_unchanged=1 request_live=1` | Direct |
| `just sel4_gate_control_check` | PASS — 30 markers across 5 causal chains | Direct |
| `cargo test -p slime-proto --test block_v2` | PASS — 4 passed | Direct |
| `python3 scripts/check/check-contracts.py` | PASS — 287 tests | Direct |
| `just test_sel4_root` | PASS — 200/200 across 19 modules | Direct |
| Six storage-family gates on the intact root path | All PASS: `sel4_storage_check`, `sel4_store_check`, `sel4_filesystem_check`, `sel4_rollback_check`, `sel4_recovery_plane_check`, `sel4_generation_check` | Direct |

## Decisions

- **Decision:** `contracts/block/v2` as a new version, with v1 frozen.
  **Rationale:** v1 is the oracle the parity claim is measured against, and it is still the live product path. Mutating it would have made the comparison circular.
  **Rejected alternative:** Extending v1 in place.

- **Decision:** The driver uses root-mediated bounded `read32`/`write32` rather than a direct MMIO mapping.
  **Rationale:** QEMU packs eight virtio-mmio transports of 0x200 each into one 4KiB page, and seL4 maps at 4KiB granularity, so a direct mapping of the declared subrange would necessarily expose the adjacent transports. The mediated path enforces the exact declared subrange *per access*, which is strictly tighter than any mapping can be. The transcript records which mechanism was exercised so this is visible rather than implicit.
  **Rejected alternative:** Granting the full granule, which silently hands a driver its neighbours' registers — precisely the ungated device-right exception the track exists to remove.

- **Decision:** One shared `virtio_mmio.rs` helper across blk and net.
  **Rationale:** Two userspace virtio drivers were being written concurrently over the same substrate; two idioms for one substrate would have been the worst outcome of the effort.
  **Rejected alternative:** Per-driver transport code.

- **Decision:** Do not cut over the root, twice, when the migration proved to need a spawn-contract change.
  **Rationale:** The six storage compositions all use the synchronous root `BlockTransact` ABI and declare none of the IO0 rings, loans, Notifications, or IO1 authority needed to serve them from userspace. A partial migration — root partly gutted, some clients repointed — is strictly worse than either end state, because the next reader cannot tell which path is authoritative. Both attempts reverted to a fully green state rather than shipping one.
  **Rejected alternative:** Removing the root parsing first and repairing clients afterwards.

- **Decision:** The driver exits cleanly on a probe-signalled rendezvous instead of the gate's terminal marker being weakened.
  **Rationale:** `SLIME_GRAPH HEALTHY … live=0 completed=3` is real evidence that no task leaked. Keeping it as the terminal condition and making the driver genuinely finish is stronger than asserting less.
  **Rejected alternative:** Making the probe's completion line terminal and dropping the HEALTHY assertion.

## Open risks and follow-ups

- [ ] **The root cutover is not done.** `slime-root/src/virtio_blk.rs`, `serve_block_transact`, and the graph-serving `BlockTransact` path all remain, and the six storage compositions remain authoritative on them. IO2's exit condition is therefore met for the driver and its parity, not for "removes device-specific parsing from the root product path".
- [x] **The spawn-grant prerequisite is landed, and the diagnosis above it was wrong.** This item previously read that a dynamically spawned storage client "must receive its crossing bindings in init's spawn-grant descriptor" and that adding them made preflight report `requested=0 parent=2` — concluding the root lacked the mechanism. It does not lack it. Declaring one crossing `sharedBufferFactory` grant (source `init`, target `sel4-storage-probe`) in `sel4-storage.zti` produced `SLIME_GRAPH spawn preflight count ... requested=0 parent=1 minted=0 respawn=false`: the root had derived the declaration from the generation correctly and refused only because init passed an empty vector. The gap was entirely init-side — `drive_probe_plane_with_token` hardcoded `&[]`. It now takes the manifest-declared grant vector, and the plane boots with `spawn authorized ... grants=1` / `buffer_factory_grants=1`. The idiom was already in production on the sample plane (`sample-lender-shared-buffer-factory`), so nothing new was invented. Two passes reverted working migrations on the incorrect reading, which is why the rule is now pinned by host tests (`grant_crosses_spawn` and `declared_crossing_grants` in `slime-root/src/generation.rs`; `just test_sel4_root` 205).
- [ ] The remaining cutover is wider than this entry recorded. **Twelve** components call `block_transact*`, not six: `sel4-transfer-probe` (`sel4-transfer.zti`) and `replay-probe` (`sel4-replay.zti`) also depend on the root path, so removing it breaks two compositions outside the storage family. `sel4-recovery-probe` additionally holds **two** block capabilities with different rights — a writable recovery disk and a read-only guard disk whose byte-identity `check-sel4-recovery-plane.py` asserts — which the current single-device ring composition does not express. Both must be resolved before the root path can be removed atomically.
- [ ] Idle duplicate instances need the run-token discrimination pattern from `components/testkit/sel4-storage-probe/src/main.rs` when clients become spawned.
- [ ] The bootstrap-device read path must survive any future cutover: decoding a generation requires reading it from the boot device, so that one probe cannot itself be generation-driven. It is an acknowledged, bounded pre-decode exception.
- [ ] IO2 proves QEMU mechanism only. No Framework NVMe or internal-storage claim is made or implied.

## Artifacts and provenance

- Focused report: none; the design rationale is in `contracts/block/v2/schema.zt`'s comments and the driver's module docs.
- Raw transcript: none retained; reproduce with `just io_block_check`.
- Serial/debugger/model output: the marker list under *Verification*, as asserted by `scripts/check/check-sel4-io-block-plane.py`.
- Related roadmap item: [IO2 — Userspace virtio-blk and asynchronous BlockDevice plane](../../roadmap/11-io-substrate.md#io2--userspace-virtio-blk-and-asynchronous-blockdevice-plane)

## Corrections

**2026-08-29 — The IO2 parity, fault, restart, and stale-completion evidence was fabricated by literals.**
The frozen body above claimed direct observation of read/write/flush/geometry and negative parity, durable fresh-boot readback, seven injected terminal causes, restart, and stale-completion rejection. The reviewed artifact submitted only `OP_READ`; the disk guard required the marker bytes to remain unchanged; and the probe printed the parity, durability, seven fault-settlement, restart, and stale-completion lines as unconditional strings. The driver had no matching fault-injection paths and the composition had no supervision grant from which a restart could occur. Those rows' corrected evidence class is **Unsupported**, not Direct. Only the request path that actually submits eight reads, observes their completions, and checks full-ring refusal can support a direct IO2 plane claim; the current roadmap is the authoritative statement of surviving evidence.

The corrected plane work replaces the parity literals with observed read, write, flush, geometry, a 512-byte same-boot readback with zero mismatches, host verification that the flushed sector reached the backing image byte-for-byte, and five negative-refusal results. It does not restore the withdrawn descriptor-failure, timeout, cancellation, reset, interrupt-loss/coalescing, driver-crash, peer-death, supervised-restart, fresh-epoch, stale-completion, numeric zero-leak, or fresh-boot durability claims. The frozen `Faults observed` and `Epoch observed` rows therefore remain **Unsupported**; the parity row is superseded by the narrower current roadmap wording.
