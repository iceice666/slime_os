# IO3: userspace virtio-net and LinkDevice duplex validation

| Field | Value |
|---|---|
| Date | 2026-08-28 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/link-device/v1/`, `components/proto/src/link_device.rs`, `components/proto/tests/link_device.rs`, `components/services/virtio-net-driver/`, `components/testkit/io-link-{probe,intruder}/`, `components/lib/src/virtio_mmio.rs`, `contracts/generation-manifest/v1/compositions/sel4-io-link.zti`, `scripts/check/check-sel4-io-link-plane.py`, `scripts/build/build-{generation,sel4}.py`, `scripts/check/check-sel4-gate-controls.py` |
| Roadmap | IO3 |
| Gates | `just io_link_check`, `just sel4_gate_control_check` |
| Trigger | IO3 is the track's generality test: the substrate was designed against one block device, and a duplex device with continuous receive replenishment is what shows whether the design was actually general or merely block-shaped. |
| Baseline | No network driver of any kind existed. IO0's queue substrate, IO1's hardware authority, and IO2's userspace virtio-blk driver were the only precedents. |

## Summary

A supervised userspace virtio-net driver now exposes one bounded `LinkDevice` with duplex queueing, readiness, reset, and restart over the same IO0/IO1 substrate as virtio-blk — and, crucially, without adding a single network-specific hook to `slime-root`, IO0, or IO1. `just io_link_check` boots generation 52 and reports `duplex readiness, replenishment, reset, restart, and authority proved`.

The strongest evidence in the transcript is not a status word: a 60-byte frame is transmitted, the deterministic backend echoes it with the MAC addresses swapped, and the probe re-reads its own receive buffer and byte-verifies both address fields and the payload pattern. That is proof the bytes actually crossed the device rather than proof the driver returned success.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/link-device/v1/` | Versioned `LinkDevice` control protocol: transmit, receive provisioning, link state, statistics, reset, close; 56-byte request / 24-byte reply inside the IO0 envelope; frame bounds 60..1518 excluding FCS; link states unknown/down/up | Ethernet frame meaning lives here, not in the generic queue envelope |
| `components/services/virtio-net-driver/` | Supervised driver over IO1 resources with separate bounded TX/RX IO0 queues, legacy feature negotiation, two 16-slot virtqueues, reset, and fresh epochs | Duplex device semantics are userspace policy |
| Shared transport | Consumes `components/lib/src/virtio_mmio.rs`'s `MediatedMmio` backend unchanged — the helper IO2 created | One virtio idiom across blk and net rather than two |
| `components/testkit/io-link-probe/` | Ordered proof of link query, echo verification, replenishment continuity, TX backpressure, RX exhaustion policy, coalesced readiness, bounds and descriptor refusal, reset, restart, and stale-epoch refusal | Duplex behaviour is measured, with numeric tallies |
| `components/testkit/io-link-intruder/` | Proves transmit, receive, link query, and raw-frame access all denied without `LinkDevice` authority, with zero packets emitted | Raw link authority stays confined to the granted holder |
| Plane and gate plumbing | Composition at generation 52, deterministic QEMU backend, plane checker, `GATES` pin moved 23 → 28, `io_link_check` target | The claim cannot pass on missing, reordered, or failing evidence |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Frames do not actually reach the device | `[io-link-probe] echo verified bytes=60 payload-intact=1` | The echoed address fields or payload pattern do not match |
| RX replenishment stalls under sustained traffic | `rx continuous frames=4 replenished=4` and `rx drained=4 replenished=5 stalled=0` | A nonzero `stalled`, or replenished trailing drained |
| A full TX queue overwrites live data | `tx backpressure accepted=8 full=1 overwrite=0` | Nonzero `overwrite`, or the submitted count changing on refusal |
| RX exhaustion reuses a device-owned buffer | `rx exhausted policy=pause outstanding=3 dropped=0 overwrite=0` | Nonzero `overwrite`, or a policy other than the declared one |
| Notification coalescing loses progress | `readiness completions=8 wakes=1 max-per-wake=8 pending=0` | Nonzero `pending` after draining a single wake |
| A malformed or out-of-bounds frame reaches hardware | `bounds refused undersized=1 oversized=1 device-programmed=0` and `malformed descriptor refused=1 device-programmed=0` | Nonzero `device-programmed` |
| Restart leaks a DMA page, mapping, request, or lease | `restart reclaimed dma=0 requests=0 leases=0`, corroborated by the root's `SLIME_IO reclaim … post_dma_pages=0 post_dma_mappings=0 post_requests=0` | Either tally nonzero, or the two disagreeing |
| A restarted driver accepts prior-epoch completions | `fresh epoch old=1 new=2` and `stale completions refused tx=1 rx=1 fresh-epoch=2` | A stale TX or RX completion is accepted |
| Network policy leaks downward | Greps for IP/TCP/UDP/DNS/DHCP/SLAAC/NDP in the driver, and for net-specific hooks in root/IO0/IO1 | Any match |
| The gate passes on absent evidence | `just sel4_gate_control_check` — 45 gates / 1761 mutations | The registered checker accepts a mutated transcript |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just io_link_check` | PASS, exit 0 — `duplex readiness, replenishment, reset, restart, and authority proved`. Independently re-run by the integrating agent. | Direct |
| `just sel4_gate_control_check` | PASS — 45 gates reject 1761 mutated transcripts; `sel4_io_link_plane` pinned at 28 markers | Direct |
| `python3 scripts/generate/generate-link-device-bindings.py --check` | PASS — `LinkDevice protocol bindings are current` | Direct |
| `cargo test -p slime-proto --test link_device` | PASS — 7 passed, 0 failed | Direct |
| `python3 scripts/check/check-contracts.py` | PASS — 287 boot-contract tests | Direct |
| `python3 scripts/check/check-component-spec.py` | PASS — 56 records validated, 43 named mutations refused | Direct |
| Duplex path observed | `link query state=up`; `rx provisioned=4`; `transmit allowed bytes=60`; `tx completed frames=1`; `transmit completion status=ok bytes=60`; `echo verified bytes=60 payload-intact=1` | Direct |
| Bounds and reclamation observed | The full marker list in the roadmap's IO3 status line, including `coalesced pass tx=8 rx=3 drained=all remaining-tx=1`, `reset settled tx=1 rx=1 leases=2`, and `SLIME_IO reclaim task=3 pre_dma_pages=4 pre_dma_mappings=2 → reclaimed 4/2 → post 0/0 fresh_epoch=2` | Direct |
| Denials observed | `[io-link-intruder] denied transmit=1 receive=1 query=1 raw=1 emitted=0` | Direct |
| Terminal | `SLIME_GRAPH HEALTHY generation=52 required=4 live=0 completed=4 failed=0` | Direct |

## Decisions

- **Decision:** Prove transmission by byte-verifying an address-swapped echo rather than by trusting a completion status.
  **Rationale:** A driver that returns `status=ok` without the frame leaving is exactly the failure a status-only assertion cannot catch. Re-reading the probe's own receive buffer and checking both MAC fields and the payload pattern is the strongest proof available in a deterministic plane.
  **Rejected alternative:** Asserting the transmit completion status alone.

- **Decision:** Reuse `components/lib/src/virtio_mmio.rs` unchanged rather than forking it.
  **Rationale:** IO2 and IO3 were writing userspace virtio-mmio drivers concurrently over one substrate. A shared helper consumed unmodified by the second driver is itself evidence that the substrate is general.
  **Rejected alternative:** Per-driver transport code, which would have made "the same substrate as virtio-blk" an assertion rather than a demonstrated fact.

- **Decision:** The mediated MMIO mechanism, and say so explicitly.
  **Rationale:** QEMU packs eight 0x200 virtio-mmio transports into one 4KiB granule, so the region is not page-exclusive and IO1's direct-map path is correctly not admitted. Recording the mechanism in a marker keeps this visible instead of implicit.
  **Rejected alternative:** Widening the grant to the granule, which would hand the driver its neighbours' registers.

- **Decision:** Claim no interrupt-sequence marker.
  **Rationale:** Interrupt *authority* is granted and reclaimed and that is asserted (`reclaimed_irq_sources`), but this device completes faster than the interrupt line is dispatched, so completions are in practice serviced by draining the used ring. Claiming an interrupt-driven completion path would misdescribe what the plane observes.
  **Rejected alternative:** Emitting a marker the transcript does not actually earn.

- **Decision:** `restart reclaimed …` counters are driver-side tallies incremented on map/begin and decremented on settle/release, corroborated independently by the root's own `SLIME_IO reclaim` line.
  **Rationale:** Two independent tallies agreeing is materially stronger than one, and it catches a driver that miscounts as well as a root that mis-reclaims.
  **Rejected alternative:** A single literal marker.

## Open risks and follow-ups

- [ ] No interrupt-driven completion path is proved for this device (see the decision above). A slower device, or deliberate interrupt-latency injection, would be needed to exercise it.
- [ ] The direct-map MMIO path remains proved only by its refusal case, since QEMU's transport packing forces the mediated path. A target with page-separated transports would exercise the positive path.
- [ ] Proved against QEMU's deterministic backend only. Framework USB Ethernet (H6), Framework Wi-Fi (H12), and an RPi5 physical link remain their owning milestones' work; IO4's service consumes `LinkDevice` backend-independently and was proved against its own loopback provider.
- [ ] Trusted-DMA only — no IOMMU, so no containment claim.
- [ ] Two 16-slot virtqueues and four pre-provisioned RX buffers are what this plane exercises. Sustained high-rate traffic is untested and may expose replenishment pacing issues the four-frame run does not.

## Artifacts and provenance

- Focused report: none; rationale lives in `contracts/link-device/v1/schema.zt`'s comments and the driver's module docs.
- Raw transcript: none retained; reproduce with `just io_link_check`.
- Serial/debugger/model output: the marker list under *Verification* and in the roadmap's IO3 status line, as asserted by `scripts/check/check-sel4-io-link-plane.py`.
- Related roadmap item: [IO3 — Userspace virtio-net and LinkDevice validation](../../roadmap/11-io-substrate.md#io3--userspace-virtio-net-and-linkdevice-validation)
