# IO11: the network service attaches to virtio-net, and answers ARP and ICMP under QEMU

| Field | Value |
|---|---|
| Date | 2026-09-11 |
| Kind | Change |
| Status | Verified |
| Scope | `components/services/network-service`, `components/lib/src/{link_frames,tick_clock}.rs`, `components/testkit/io-tcp-probe`, `contracts/system-spec/v1/systems/sel4-io-tcp.zti`, `contracts/network-interface/v1`, `contracts/syscall-abi/v1`, `scripts/lib/link_peer.py`, `scripts/check/check-sel4-io-network-plane.py` |
| Work items | 01a08ff0-0bae-741e-9318-331fdafe0b96 |
| Gates | `just io_tcp_check`, `just io_link_check`, `just io_network_check`, `just link_peer_check`, `just sel4_gate_control_check`, `just test_sel4_root`, `just test_host` |
| Trigger | The lane's first session: the network service had never been wired to the driver IO3 qualified |
| Baseline | `just io_network_check` passing on generation 53 with `io-link-loopback`; `just io_link_check` passing on generation 52 with `io-link-probe` as the driver's only client |

## Summary

`network-service` now attaches to `virtio-net-driver` exactly as the IO3 probe does and runs
smoltcp behind its exact-destination authority. On the new `sel4-io-tcp` plane (generation
54) it read its declared interface and the counter's rate from the root, lent the driver two
queues and eight frame pages, saw the link up with four receive buffers provisioned, and
answered the peer's ARP requests and five ICMP echo requests, its own frame counts agreeing
with the driver's statistics. The client held its one declared destination for
three seconds and closed it; the intruder's fourteen refusals held unchanged; the service
reset the driver, acknowledged its settled frames, saw the fresh epoch, and exited; the graph
ended healthy. The peer's ledger agrees: every echo sequence answered, no frame from an
undeclared MAC, no undeclared host addressed.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/syscall-abi/v1` (70, 71), `slime-root` | `CLOCK RATE READ` under `monotonicRead`; `CAPABILITY NETWORK INTERFACE READ` on the destination read's shape and identity gate | A holder that may read ticks may know what they mean; the root copies authenticated bytes and learns no address |
| `contracts/network-interface/v1`, builder, decoder | One static IPv4 interface per holder; unicast host, on-prefix gateway, unicast MAC enforced at build and decode | What a stack answers to is declared and validated generation data |
| `network-service/src/link.rs` | smoltcp `Device` over the IO3 client shape: receive pages device-owned until completed, transmit pages retained until completed, frames padded to 60 bytes, request ids odd on transmit and even on receive | The driver's one DMA account never sees a duplicate request id; no page is touched while the device owns it |
| `network-service/src/main.rs` | Interface read, clock, optional link attachment with the link peer at slot 0 and the factory at slot 1, a polling loop that drains the link and polls the interface between clients, release by reset handshake | A loan names its receiver by endpoint slot, which the root reads as an endpoint only while no buffer occupies the number; the service never exits while the driver still writes its rings |
| `components/lib/src/link_frames.rs`, `tick_clock.rs` | Pure slot tracking and tick arithmetic, host-tested | Every transition and every conversion is a host test |
| `sel4-io-tcp` (generation 54), `io-tcp-probe` | The data-plane composition and its client | Generation 53 stays the authority-only regression with its frozen baseline |
| `scripts/lib/link_peer.py`, `check-sel4-io-network-plane.py --arm tcp`, `check-sel4-gate-controls.py` | A frame-level peer with a ledger; chains one component each; the gate-control pin from 16 to 39 | Every frame that leaves the guest is observed; no startup interleaving is asserted |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The service stops answering ARP or echo on its declared address | `just io_tcp_check` | `echo replies from the guest were […], expected every sequence 1..5` or `the guest never answered the peer's ARP request` |
| A frame leaves the guest for an undeclared host or from an undeclared MAC | `just io_tcp_check` | `the guest addressed IPv4 hosts the composition does not declare` / `frames arrived from a MAC the composition does not declare` |
| The release handshake regresses and the driver or the service faults at exit | `just io_tcp_check` | `SLIME_ROOT FATAL`, a missing `fresh epoch old=1 new=2`, or the 240 s timeout |
| The authority-only plane changes shape | `just io_network_check` | Its 16 markers |
| The driver's own plane changes | `just io_link_check` | Its 28 markers |
| A marker is deleted or reordered without notice | `just sel4_gate_control_check` | The pinned count 39 |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just io_tcp_check` (twice) | Passed both times. First run: `link frames total=12 tx=6 rx=6 arp=2 icmp=10 tcp=0 other=0`, peer `received 6 (arp-reply=1, icmp-echo-reply=5); sent 8`. Second run, kept as [`io-tcp-plane.log`](io-tcp-plane.log): `link frames total=14 tx=7 rx=7 arp=4 icmp=10 tcp=0 other=0`, `link statistics tx=7 rx=7`, `fresh epoch old=1 new=2`, `SLIME_GRAPH HEALTHY generation=54`; peer `received 7 (arp-reply=2, icmp-echo-reply=5); sent 9`, its first ARP request having gone out before the guest's receive buffers were lent | Direct |
| First two boots of the plane | Refused before any frame: a loan naming the link peer at relative number 2, which the first buffer also took (fixed by pinning the peer at 0 and the factory at 1); then the driver's `request begin` on a duplicate request id across the two queues (fixed by id parity) | Direct |
| `just link_peer_check` | Passed | Direct |
| VERIFY_ROWS | | |

## Decisions

- Decision: chains one component each, in program order, for the tcp arm.
- Rationale: the matcher orders markers only within a chain, and the first passing boot showed the service's authority lines before the driver's negotiation line; whichever runs first is the scheduler's, and B94 is what asserting it costs.
- Rejected alternative: a single ordered chain frozen from one transcript.
- Decision: the link peer and the buffer factory at compiled slots 0 and 1, like the IO3 probe.
- Rationale: the root reads a loan's receiver number as an endpoint only while no shared-buffer capability occupies it, and the service's buffers take the low logical numbers.

## Open risks and follow-ups

- [ ] The TCP byte stream itself: the probe's capability is authority-only until S2 gives `OP_CONNECT` a real socket and the data queue its send and receive semantics.
- [ ] Four receive pages in flight; a burst larger than that between polls is dropped by the device (`stalled=0` on this plane).
- [ ] The intruder still runs its DNS-named refusal arms; the tcp arm adds no new denial yet.

## Artifacts and provenance

- Focused report: this entry; the lane's plan in [`../2026-09-11-io-tcp-lane/plan.md`](../2026-09-11-io-tcp-lane/plan.md).
- Raw transcript: [`io-tcp-plane.log`](io-tcp-plane.log), the gate's serial capture and peer summary from the passing run.
- Related work item: IO11, `.tasks/items/01a08ff0-0bae-741e-9318-331fdafe0b96.md`.
