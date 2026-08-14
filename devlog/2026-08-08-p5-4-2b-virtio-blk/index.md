# P5.4.2b — a virtio-blk transport for `slime-root`

| Field | Value |
|---|---|
| Date | 2026-08-08 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{virtio_blk,device,lib,main}.rs`, `scripts/check/check-sel4-{device-plane,root-boot}.py` |
| Roadmap | P5.4.2, P5.4, M5.2, M5.3 |
| Gates | `just sel4_device_check`, `just sel4_root_boot_check`, `just sel4_gate_control_check`, `just test_sel4_root`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` |
| Trigger | P5.4.2a gave the root a mapped register bank and a bound IRQ; sectors still could not move |
| Baseline | The root could identify an attached block device and read nothing from it |

## Summary

`slime-root` moves sectors. A bounded virtio-mmio block driver — one queue, one
outstanding request, one fixed buffer — completes reads, writes, and flushes
against a real QEMU disk, and `just sel4_device_check` asserts all three plus a
byte-for-byte read-back.

The evidence is the fixture's own bytes: the gate writes `SLIMEDSK` to sector 0
and requires the read to report `head=534c494d`, so a driver that completed a
request without moving data fails rather than passing on a buffer of zeroes.
The write goes to sector 1 and is confirmed durable on the host image after the
boot.

This is the second of P5.4.2's three slices. The `BlockTransact` and
`StoreTransact` mediation that puts this under the store, and the M5 gates
above it, are P5.4.2c.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `device.rs` | `DmaPage`: retype a granule of ordinary RAM, map it non-cacheably, zero it, expose its physical base | A virtqueue descriptor can name memory the device will read |
| `device.rs` | `DeviceRegion::remap` | A granule the probe already retyped can move to a standing address; seL4's retype is once-per-boot |
| `virtio_blk.rs` | The driver: legacy MMIO handshake, three-descriptor chain, polled completion, `BlockError` | Sectors move, and every failure is a typed value |
| `main.rs` | `bring_up_block`, three more claimed root-image pages | The probe hands the attached transport's frame to a driver instead of releasing it |
| `check-sel4-device-plane.py` | Five more markers; a signed fixture disk | The read reports the fixture's bytes, not zeroes |

### One request in flight, deliberately

The store above reads and writes 512 bytes at a time and waits for each, so
depth buys nothing and costs the correctness argument: with one outstanding
request the used ring has at most one entry to interpret and no completion can
be attributed to the wrong caller.

### Completion is polled, and the IRQ is not serviced

`DeviceIrq` binds the line, and the driver does not wait on it. A
level-triggered virtio line stays asserted until `InterruptACK` is written, and
the root is also the IPC dispatcher — there is nowhere to block. The driver
clears `InterruptStatus` on completion so a bound handler is left sane, and
spins on the used ring's index with a bounded poll count so a wedged device is a
`Timeout` rather than a hung graph.

### The bug worth recording: `QUEUE_ALIGN` is not the page size

The first working handshake reported `sectors=2048` — correct — and then every
request timed out. The device was completing them; the driver was polling a used
ring the device never wrote.

Legacy virtio-mmio places the used ring at the first multiple of `QUEUE_ALIGN`
after the available ring. Writing the granule size there, which reads naturally
beside `GUEST_PAGE_SIZE`, puts the used ring in the *next* page — outside the
one granule the queue occupies. `QUEUE_ALIGN` must be `USED_OFFSET`: the offset
the driver's own layout uses.

`GUEST_PAGE_SIZE` was also missing, and must be written before `QUEUE_PFN` or
the device derives the wrong base from the frame number.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The handshake regresses | `block ready … sectors=2048`, read from config space | marker missing |
| A request completes without moving data | `head=534c494d`, the fixture's own signature | the head bytes change |
| The write direction or FLUSH regresses | `wrote lba=1 flushed=1 verified=1`, with a byte-for-byte read-back | marker missing, or "read-back mismatch" |
| The queue layout drifts | compile-time asserts on ring offsets against the granule | build failure |
| A device wedges | bounded `COMPLETION_POLLS`, then `Timeout` | `block read failed` / `block write failed`, both failure markers |
| DMA addresses stop being physical | `block dma queue=… buffer=…` | marker missing |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_device_check` | Pass; 8 markers; read, write, flush, and verified read-back | Direct |
| Host inspection of the fixture image after the boot | `sector0: SLIMEDSK`, `sector1: SLIMEWR1` | Direct |
| `just sel4_root_boot_check` | Pass; probe still reports `found=0` with no disk | Direct |
| `just sel4_gate_control_check` | Pass; 15 gates reject their mutated transcripts | Direct |
| `just test_sel4_root` | Pass; 113/113 across 13 modules | Direct |
| The other twelve seL4 plane gates | Pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |

`sel4_root_boot_check`'s reclaimed CSlot ranges moved 849..899 → 853..903: the
IRQ binding takes four more slots before the graph starts.

## Decisions

- **Decision:** Poll the used ring rather than wait on the bound interrupt.
  **Rationale:** the root is the IPC dispatcher and cannot block, and
  acknowledging a level-triggered virtio line before the driver clears the
  device condition re-fires it immediately.
  **Rejected alternative:** an interrupt-driven completion path, which needs the
  driver to be a component with its own wait loop — that is the shape P5.4.2c's
  block service takes.

- **Decision:** Hand the probe's already-retyped frame to the driver via
  `remap` rather than retyping the granule again.
  **Rationale:** seL4's retype is monotonic per untyped, so a device page can be
  reached exactly once per boot. The second attempt failed with
  `DeviceFramePassed` — the error P5.4.2a added for exactly this.
  **Rejected alternative:** skipping the attached transport during the scan,
  which would mean the probe could not report what is attached.

- **Decision:** Accept no device features.
  **Rationale:** every optional block feature is a behaviour this driver does
  not implement, and the legacy device works with none of them. Negotiating one
  and then ignoring it is how a driver corrupts data.

- **Decision:** Write the status byte to `0xff` before each request.
  **Rationale:** the previous request's zero would make a device that wrote
  nothing look successful.

## Open risks and follow-ups

- [ ] Non-cacheable DMA mappings, no explicit barriers. Correct on
      qemu-arm-virt, whose virtio transports the device tree declares
      `dma-coherent`. A platform needing explicit cache maintenance would need
      it here, and nothing currently detects that difference.
- [ ] The driver lives in the root. That is right for a bring-up proof and wrong
      as a destination: storage policy belongs in userspace, and P5.4.2c moves
      the request/response surface to a component behind `BlockTransact`. What
      stays in the root is the mediation, not the queue.
- [ ] No fault injection yet. M5.3 requires reset, timeout, and stale-completion
      recovery; this driver has a bounded timeout and nothing else. Those arms
      are P5.4.2c's.
- [ ] `BlockIo` is not yet implemented for `VirtioBlock`. The signatures match
      (`read_sector`/`write_sector`/`flush` over `[u8; 512]`), but the trait
      lives in `boot-contracts` and wiring it up belongs with the store service
      that consumes it.

## Artifacts and provenance

- Gate output, every value's meaning, and the host-side durability check:
  [`block-check.txt`](block-check.txt).
- The substrate this builds on:
  [`devlog/2026-08-08-p5-4-2a-device-substrate/`](../2026-08-08-p5-4-2a-device-substrate/index.md).
- Related roadmap item: P5.4.2 in
  [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md).
