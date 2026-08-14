# P5.4.2a — a device resource substrate for `slime-root`

| Field | Value |
|---|---|
| Date | 2026-08-08 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{object_allocator,device,lib,main}.rs`, `scripts/check/check-sel4-{device-plane,root-boot,gate-controls}.py`, `Justfile`, `roadmap/07-architecture-portability.md` |
| Roadmap | P5.4.2, P5.4, M5.1 |
| Gates | `just sel4_device_check`, `just sel4_root_boot_check`, `just sel4_gate_control_check`, `just test_sel4_root`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` |
| Trigger | P5.4.2 recorded the M5 blocker as "`slime-root` has no block device"; the C-series is now closed and M5 is what remains of P5.4 |
| Baseline | The allocator discarded every device untyped, so the root held no MMIO region and no physical address for any frame |

## Summary

`slime-root` can now reach a device. The allocator keeps BootInfo's device
untypeds in their own table keyed by physical address, retypes the granule
containing a requested MMIO page, and records the physical base of every
ordinary allocation so a DMA buffer can be named to a device. A new `device`
module maps such a granule non-cacheably into the root's own VSpace and reads
registers out of it.

The proof is a pair of gates over one code path: `just sel4_root_boot_check`
requires the probe to find **nothing** on a machine with no disk, and
`just sel4_device_check` boots the same image with a virtio-blk device attached
and requires it to name exactly that disk. Either alone is satisfiable by a
probe returning a constant.

This is P5.4.2's first of three slices. There is no driver here and no storage
policy — the transport is P5.4.2b and the M5 gates are P5.4.2c.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `object_allocator.rs` | `DeviceRegion` table populated from `bootinfo.device_untyped_range()`, keyed by `paddr` | The root can name a physical device region; before, `is_device()` discarded them |
| `object_allocator.rs` | `allocate_device_frame(paddr)` with a per-region `retyped` watermark | The granule containing an address can be reached, repeatedly and correctly |
| `object_allocator.rs` | `UntypedRegion.paddr` plus `last_physical_address()` | A DMA buffer's guest-physical address is derivable; nothing else in the root knows it |
| `device.rs` | `DeviceRegion::{map,unmap,read32,write32}` and `VirtioMmio::probe` | MMIO is mapped non-cacheably and accessed volatilely, with bounds-checked offsets |
| `main.rs` | `DEVICE_PAGE`, a second claimed root-image page; `probe_devices` | A standing device mapping does not contend with the transfer window's transient one |
| `device.rs` | `DeviceIrq::{acquire,poll,acknowledge}`, badged like the timer's | The root can bind a device interrupt line, distinguishable from the timer's by badge |
| `check-sel4-device-plane.py`, `Justfile` | `just sel4_device_check` | The attached-disk half is observed |
| `check-sel4-root-boot.py` | Two device markers, `found=0` asserted; reclaimed CSlot ranges repinned | The absent-disk half is observed and is not merely tolerated |

### Two tables, not one

Ordinary untypeds are allocated *from* in watermark order. A device untyped
names a fixed physical range and is only ever asked for the page containing a
given address. Keeping them apart is what makes it impossible for an ordinary
allocation to land in MMIO — which the old `is_device()` skip achieved by
discarding device untypeds entirely, at the cost of making the root unable to
reach a device at all.

### The watermark, and the bug it fixed

`seL4_Untyped_Retype` has no offset argument: each call places its objects at
the untyped's own internal watermark, which only advances. So the granule at
index `n` is reached by retyping enough pages to arrive there and keeping the
last — the trimming the upstream `serial-device` example performs.

The first implementation computed that count from the region base every call.
That is correct exactly once. The second call started where the first stopped
and landed past its target, and the fault was not an error code: the mapping
succeeded and the root took a VM fault reading it.

```text
Caught cap fault in send phase at address 0
vm fault on data at address 0x2ae000 with status 0x92000010
```

`DeviceRegion.retyped` now mirrors the kernel's watermark, so the count asked
for is the distance from where it actually is. Going backwards is impossible and
is refused with `DeviceFramePassed` rather than retyped past.

### Non-cacheable, deliberately

`sel4::VmAttributes::DEFAULT & !PAGE_CACHEABLE`. A cached mapping can return a
stale line from a register read and leave a register write sitting in one. The
accesses are `read_volatile`/`write_volatile` for the same reason at the
compiler's level.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The allocator stops seeing device untypeds | `devices untypeds=[1-9]` in `just sel4_root_boot_check` | marker missing |
| A mapping faults, or the watermark lands past its target | `virtio probed granules=4 slots=32` — no line at all if any map fails | marker missing |
| The probe reads a constant rather than a register | the pair: `found=0` with no disk, `found=1` with one | whichever side stops matching |
| The probe matches something other than a transport | `just sel4_device_check` requires exactly one transport line | "the probe reported N transports, expected 1" |
| An ordinary allocation lands in MMIO | separate tables; the general path can never see a device region | — (structural) |
| The gate loses evidence | `just sel4_gate_control_check`, `sel4_root_boot` pinned at 42 | a mutated transcript is accepted, or the count drifts |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_device_check` | Pass; disk identified as `transport=0xa003e00 device=2 vendor=0x554d4551`, its IRQ bound at 79 | Direct |
| `just sel4_root_boot_check` | Pass; same probe reports `found=0` with no disk attached | Direct |
| `just sel4_gate_control_check` | Pass; 15 gates reject 718 mutated transcripts and layouts | Direct |
| `just test_sel4_root` | Pass; 113/113 across 13 modules | Direct |
| `just sel4_component_graph_check` | Pass | Direct |
| `just sel4_channel_check` | Pass | Direct |
| `just sel4_loan_check` | Pass | Direct |
| `just sel4_spawn_check` | Pass | Direct |
| `just sel4_sample_check` | Pass | Direct |
| `just sel4_supervision_check` | Pass | Direct |
| `just sel4_crossing_check` | Pass | Direct |
| `just sel4_stream_check` | Pass | Direct |
| `just sel4_qos_check` | Pass | Direct |
| `just sel4_call_check` | Pass | Direct |
| `just sel4_operation_check` | Pass | Direct |
| `just sel4_visibility_check` | Pass | Direct |
| `just sel4_boot_check` | Pass | Direct |
| `just sel4_boot_layout_check` | Pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |

Every plane was re-run: the device phase is unconditional, so it runs on every
seL4 boot. `sel4_root_boot_check`'s pinned reclaimed CSlot ranges moved
839..889 → 849..899 because the probe retypes ten granules before the graph
starts; the width-and-adjacency property is unchanged.

## Decisions

- **Decision:** A separate `DEVICE_PAGE` rather than reusing the loader's
  scratch address.
  **Rationale:** `transfer_window` maps and unmaps a child frame at the scratch
  address on every staged transfer. An MMIO frame left there would be replaced
  by the next syscall.
  **Rejected alternative:** mapping the device transiently around each access,
  which is correct but makes every register read a pair of syscalls.

- **Decision:** Probe all thirty-two transports rather than reading one pinned
  slot.
  **Rationale:** QEMU attaches devices downward from the highest free
  transport, so which slot holds a disk is a function of the command line. The
  first version read `0x0a00_0000`, found nothing with a disk attached, and only
  worked once retargeted at `0x0a00_3000` — which would have broken the moment a
  second `-device` appeared.
  **Rejected alternative:** an FDT walk, which is the right answer for a driver
  and is P5.4.2b's; a fixed stride is enough to prove the mechanism.

- **Decision:** Report device-phase failures and return, never `fatal!`.
  **Rationale:** no plane depends on a device yet. A root refusing to boot
  without one would break sixteen gates to prove nothing.

- **Decision:** Assert `found=0` on the diskless gate rather than ignoring the
  line.
  **Rationale:** it is half the evidence. A probe that always reports a
  transport would pass the device gate alone.

## Open risks and follow-ups

- [ ] The bound IRQ is never serviced. `DeviceIrq::acquire` establishes that
      the root can take a device line — observed as `irq bound … irq=79`, the
      DTB's own SPI for that transport — but nothing acknowledges it, because
      acknowledging a level-triggered virtio line before the driver clears
      `InterruptACK` is exactly the ordering that storms. `poll` and
      `acknowledge` exist and are exercised by no gate until P5.4.2b's driver
      has a device condition to clear first.
- [ ] `last_physical_address()` is implemented and unused. It exists because the
      allocator is the only place that can know a frame's physical address, and
      P5.4.2b's virtqueue needs it; it is exercised by no gate yet.
- [ ] The skipped granule capabilities are retained rather than deleted. They
      name real MMIO pages, and deleting them would return the space to a device
      untyped this root has no second use for — but the CSlots are spent for the
      boot, which is what moved the reclaimed-slot base by ten.

## Artifacts and provenance

- Both halves of the proof, with the values read and where they come from:
  [`device-check.txt`](device-check.txt).
- The slice that closed the C-series and left M5 as what remains:
  [`devlog/2026-08-08-p5-4-9-full-graph-boot/`](../2026-08-08-p5-4-9-full-graph-boot/index.md).
- Related roadmap item: P5.4.2 in
  [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md),
  whose decomposition into a/b/c this slice opens.
