# IO1: hardware-resource authority, DMA accounts, and reclamation on death

| Field | Value |
|---|---|
| Date | 2026-08-28 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/io_resource.rs`, `slime-root/src/graph_runtime/services/io_resource.rs`, `slime-root/src/graph_runtime/services.rs`, `slime-root/src/graph_runtime/services/spawn.rs`, `slime-root/src/{shared_buffer,object_allocator,device,buffer_adapter,graph,generation,ipc}.rs`, `contracts/io-resource/v1/`, `boot-contracts/src/io_resource.rs`, `contracts/generation/v5/`, `contracts/syscall-abi/v1/`, `components/runtime/src/syscall.rs`, `components/testkit/io-driver-{probe,intruder}/`, `contracts/generation-manifest/v1/compositions/sel4-io-driver-authority.zti`, `scripts/check/check-sel4-io-driver-authority-plane.py`, `docs/capability-matrix.md` |
| Roadmap | IO1, B82 |
| Gates | `just io_driver_authority_check`, `just sel4_gate_control_check`, `just test_sel4_root` |
| Trigger | IO2 and IO3 needed explicit, bounded hardware authority before a userspace driver could exist at all; the root held device untypeds only because no such model existed. |
| Baseline | P5.4.2's root-owned device path: `DeviceRegion` mapped only into the root's own VSpace with the frame kept private, `DeviceIrq` bound to root-held notifications, `DmaPage` allocated ad hoc, and no per-driver accounting, epoch, or reclamation anywhere. |

## Summary

A manifest-declared userspace driver now receives exactly one device instance plus its bounded MMIO, DMA account, interrupt, and supervision handle; an ungranted component receives none of them; and crash/restart returns every charge with a fresh epoch. `just io_driver_authority_check` boots the plane and proves each of those, including a numeric reclamation line showing 4096 MMIO bytes, one MMIO mapping, one IRQ source, two DMA pages, and one DMA mapping returning to exact zero before the driver respawns at epoch 2 and refuses predecessor epoch 1.

Two platform facts shaped the design more than anything in the roadmap text: QEMU's virtio-mmio transports share a 4KiB page, and the root consumes the block device's frames before the graph launches. Both are recorded below as decisions rather than worked around silently.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/src/io_resource.rs` | `ResourceTable` as a pure bounded state machine: typed identities, `DriverQuota`/`DriverOccupancy`, opaque `Iova` only the table constructs, `MmioAccess`, `DmaDirection`, `IoResourceAdapter` + `AdapterAction` teardown list, `reclaim_driver`, `revoke_lease`, `declare_quota_at_epoch` | Hardware authority is bounded, accounted, and reclaimable, with policy testable off-target |
| Capability classes | Kinds `device`=10, `mmioRegion`=11, `interruptSource`=12, `dmaAccount`=13 and root service `SERVICE_IO_RESOURCE`=11, emitted from `contracts/generation/v5/gen_rust.zt` and mapped in the builder | Four separately grantable authorities, so "may map MMIO but must not touch DMA" stays sayable |
| `contracts/io-resource/v1/` + `boot-contracts/src/io_resource.rs` | Per-driver budget as a generation resource object with a bounds-validating decoder | Limits are generation data, authenticated by the existing object digest table |
| Dual MMIO mechanism | Direct child mapping admitted only for page-exclusive regions; root-mediated bounded `read32`/`write32` validating `offset + 4 <= declared_length` per access | The exact declared subrange is enforced even where the MMU cannot isolate it |
| `shared_buffer.rs` | `loan_frames(receiver, handle) -> LoanFrames{len,is_empty,get,writable}` — authenticated live-loan anchor view returning no physical address | A DMA mapping can be built from a lease without exposing the lease's memory identity |
| `object_allocator.rs` | `physical_address_of(slot)` retaining allocation-time provenance per root CSlot; `allocate_contiguous_granules(count)` | An already-allocated frame's physical address is recoverable, which `last_physical_address()` alone could not do |
| `device.rs` | `DmaPage::{allocate_child, map_child_slot, release}` and a child-VSpace device-frame seam | Device memory reaches an authenticated child bounded and non-cacheable, and can be taken back |
| Real teardown | `UnmapMmio` frame-unmaps the exact device frame from the child VSpace; `UnbindIrq` clears the handler and releases handler/notification/signal caps; `DestroyDma` releases every contiguous queue page. All idempotent. | Reclamation is an effect, not a bookkeeping update |
| Task death wiring | Both fault and orderly-exit paths call `io_resource::reclaim_driver` *before* task-object reclamation | Frames and IRQ authority are released while the VSpace and CSpace still exist to release them from |
| Restart rebinding | Per-instance `next_epoch` table; a respawn reconstructs declared cross-spawn capabilities after checking the supervisor holds each declared kind | A replacement gets fresh authority at the epoch reclamation issued, and cannot acquire what it was never granted |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A grant widens beyond its declaration | `cargo test -p slime-root --lib io_resource` (18 tests) | A wrong-device/offset/length/access/duplicate case is admitted |
| Death leaks a device frame or a live IRQ | The `SLIME_IO reclaim` numeric marker asserted by the plane checker | Any `post_*` field is nonzero, or `reclaimed_*` disagrees with `pre_*` |
| A replacement inherits predecessor authority | `[io-driver-probe] fresh epoch=2` and `predecessor epoch refused=1` | A prior-epoch handle succeeds |
| A shared-granule region gets widened to its page | `[io-driver-probe] shared-granule direct map refused not widened` | The direct map succeeds |
| An ungranted component reaches hardware | `[io-driver-intruder] device mmio dma interrupt denials proven` | Any denial arm is absent |
| An ordinary client obtains an IOVA or physical address | `[io-driver-probe] opaque dma path exposes no physical address proven` plus the host tests | A test constructs an `Iova` outside the table |
| The gate passes on absent evidence | `just sel4_gate_control_check` — 45 gates / 1757 mutations | The registered checker accepts a mutated transcript |
| A retried reclamation double-performs an effect | Idempotence host tests | A second application errors or repeats an effect |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just io_driver_authority_check` | PASS — `exact mediated MMIO, bounded IRQ authority, and ungranted denial proved`. Re-run independently by the integrating agent. | Direct |
| `just test_sel4_root` | PASS — 203/203 across 19 modules (independently re-run) | Direct |
| `SEL4_PREFIX=… cargo test -p slime-root --lib io_resource` | PASS — 18 passed, up from 15 | Direct |
| `just sel4_gate_control_check` | PASS — 45 gates reject 1757 mutated transcripts, authority plane pinned at 15 markers | Direct |
| `python3 scripts/check/check-contracts.py` | PASS — bindings current, 287 contract tests | Direct |
| Reclamation observed | `SLIME_IO reclaim task=3 pre_mmio_bytes=4096 pre_mmio_mappings=1 pre_irq_sources=1 pre_dma_pages=2 pre_dma_mappings=1 pre_requests=0 reclaimed_mmio_bytes=4096 reclaimed_mmio_mappings=1 reclaimed_irq_sources=1 reclaimed_dma_pages=2 reclaimed_dma_mappings=1 settled_requests=0 post_*=0 actions=3 fresh_epoch=2` | Direct |
| Restart observed | `faulting with live authority`, `predecessor fault collected`, `fresh epoch=2`, `predecessor epoch refused=1`, `replacement completed` | Direct |
| Clean-exit reclamation observed | `SLIME_IO reclaim task=4` with all `pre_*`/`reclaimed_*`/`post_*` zero, `actions=0`, `fresh_epoch=3` | Direct |
| Terminal | `SLIME_GRAPH HEALTHY generation=50 required=3 live=0 completed=3 failed=0` | Direct |

## Decisions

- **Decision:** Four capability kinds, not one `device` with flags.
  **Rationale:** They are four separately grantable authorities; collapsing them makes "may map MMIO but must not touch DMA" unsayable.
  **Rejected alternative:** One kind with a rights bitmask.

- **Decision:** No `DmaDirection::Bidirectional`. Driver-owned queue memory gets its own operation (`io_queue_map`) that takes no lease and no direction.
  **Rationale:** A legacy virtqueue page genuinely is bidirectional — the device reads the descriptor table and available ring and writes the used ring — but adding a `Bidirectional` value would have made it the lazy default for payload mappings too, and a direction enum containing "both" scopes nothing. A virtqueue is not a buffer lease at all: it is the driver's own control structure, from its own DMA account, never covering client data. Putting that in the operation's identity rather than in a caller-chosen value makes the payload guarantee unforgeable.
  **Rejected alternative:** A third direction variant.

- **Decision:** Provide both a direct-map path (page-exclusive only) and root-mediated bounded accessors, rather than granting the full granule.
  **Rationale:** QEMU packs eight virtio-mmio transports of 0x200 each into one 4KiB page and seL4 maps at 4KiB granularity, so directly mapping a declared 0x200 subrange necessarily exposes the neighbours. Granting the granule would hand a driver its neighbours' registers — exactly the ungated device-right exception the track exists to remove. The mediated path enforces the exact subrange *per access*, which is strictly tighter than any mapping can be, so this is a tightening rather than a weakening. The plane records which mechanism was exercised.
  **Rejected alternative:** Granting the full granule, or requiring a bespoke page-separated device layout we do not control.

- **Decision:** Generation-driven selection between root block bring-up and the userspace authority-inventory path, mutually exclusive per device instance.
  **Rationale:** The root retypes the attached virtio MMIO frames and DMA pages into its own `VirtioBlock` before the graph launches, so nothing remains to grant. Driving the choice from the *admitted generation* — the presence of declared device grants or an `ioResourceBudget` — keeps the reasoning correct for future planes and real hardware, unlike a build-variant flag.
  **Rejected alternative:** Refactoring `probe_devices` for everyone (changes ten green planes' bring-up right before IO2's migration), or a variant-keyed branch (a lie the moment a second composition wants device authority).

- **Decision:** The boot selector keeps a pre-admission probe for its bootstrap device, as an acknowledged bounded exception.
  **Rationale:** Decoding a generation requires reading it from the boot device, so that one device cannot be selected by the decoded generation. Ordering, not oversight.
  **Rejected alternative:** Pretending the selection is universal.

- **Decision:** Record physical provenance at allocation time rather than reverse-mapping later.
  **Rationale:** An allocator that remembers is simpler and more auditable than one that queries. `last_physical_address()` was only valid immediately after an allocation, which is why `DmaPage::allocate` worked and an arbitrary loan frame's address was unrecoverable.
  **Rejected alternative:** A reverse lookup over untyped ranges.

- **Decision:** Size the provenance table from other modules' declared ceilings, not from the CSpace.
  **Rationale:** The first cut was `[usize; MAX_ROOT_CSLOTS]` with `MAX_ROOT_CSLOTS = 262_144` — a 2 MB `.bss` array. In this root that is not merely wasteful, it is a *capacity* limit: the seL4 loader creates one root CSlot per `.bss` page before the root runs, so it cost 512 root CSlots and made the boot plane's 2313-slot plan inadmissible (`PlanExceedsRootSlots { required: 2313, available: 2185 }`) — the same failure mode already documented above `SELECTOR_GENERATION_BYTES` in `boot_selector.rs`. Only DMA-participating frames need provenance, so the replacement sums `shared_buffer::MAX_FRAME_ANCHORS`, `io_resource::MAX_DMA_MAPPINGS`/`MAX_MMIO_REGIONS`/`MAX_MMIO_MAPPINGS`, and `device::MAX_BLOCK_DEVICES` — 452 live entries in 1024 open-addressed positions, 16 KiB. Deriving each term from its owning module's own bound means a plane that raises a limit raises this with it.
  **Rejected alternative:** Indexing by every conceivable root CSlot.

- **Decision:** `MaybeUninit` + write-once for the launch tables rather than `const`-initialized statics.
  **Rationale:** `Option<Task>`'s niche makes `None` the byte `0x2`, so a `const`-initialized `[Option<Task>; 48]` is not all-zero and the linker places it in `.data` — 163 KiB of image and ~40 root CSlots to store 48 tag bytes. The `MaybeUninit` pattern `main.rs` already uses for `OBJECT_ALLOCATOR` puts them back in `.bss`.
  **Rejected alternative:** `const fn new()` statics, which silently move cost from `.bss` to `.data`.

## Open risks and follow-ups

- [ ] The DMA arms of the authority plane prove the opaque path and the reclamation tally; the *live-loan payload* mapping is exercised by IO2's and IO3's planes rather than IO1's own probe. Extending the authority plane to also drive a payload lease end to end would consolidate that evidence in one place.
- [ ] Only the mediated MMIO mechanism is exercised on QEMU, because its layout forces it. The page-exclusive direct-map path is proved only by its refusal case; a target with separated transports would exercise the positive path.
- [ ] **B82, found during this work, is the finding most worth carrying forward.** Shrinking the provenance table moved `STACK` down 512 KiB and turned a *pre-existing* unconditional ~513 KiB stack overflow in `launch_instance_graph` from silent corruption of mapped `.bss` slack into an honest guard-page fault. It had been overflowing on every boot with every gate green. Bisecting the array length alone — 131072 entries pass, 65536 fail, identical logic — is what proved it was a layout threshold rather than a logic defect. Two lessons: a green gate over a memory-corrupting root is not evidence, and `ScratchPage`'s guard page only reports an overflow when preceding statics happen to place the stack against it. A deliberate stack-usage check would make this class detectable rather than incidental.
- [ ] `LIFECYCLE_SERVICE` still costs 11,904 bytes of `.data` for the same niche reason, if those root CSlots are ever wanted.
- [ ] Trusted-DMA only. There is no IOMMU here and no containment claim: H4 owns AMD-IOMMU proof and a future Arm milestone owns any SMMU proof.
- [ ] The QEMU transcript still contains seL4 CNode source/destination diagnostics during component staging. All three tasks activate and every authority syscall completes, so this is noise rather than a fault, but it is unexplained and worth a look.
- [ ] `just test_sel4_root`'s asserted count moved 184 → 200 → 203 across this work. The assertion is doing its job, but it also means any concurrent test addition collides with it.

## Artifacts and provenance

- Focused report: none; rationale lives in `slime-root/src/io_resource.rs`'s module docs, `contracts/io-resource/v1/schema.zt`, and the new rows in `docs/capability-matrix.md`.
- Raw transcript: none retained; reproduce with `just io_driver_authority_check`.
- Serial/debugger/model output: the `SLIME_IO reclaim` line and marker list under *Verification*, as asserted by `scripts/check/check-sel4-io-driver-authority-plane.py`.
- Related roadmap item: [IO1 — Hardware resource authority and DMA accounts](../../roadmap/11-io-substrate.md#io1--hardware-resource-authority-and-dma-accounts)
