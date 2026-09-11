# TCP over the IO substrate: opening the lane

| Field | Value |
|---|---|
| Date | 2026-09-11 |
| Kind | Decision |
| Status | Proposed |
| Scope | `contracts/network-interface/v1`, `contracts/syscall-abi/v1`, `slime-root/src/{clock,ipc,generation}.rs`, `components/services/network-service`, `components/testkit/io-tcp-probe`, `contracts/system-spec/v1/systems/sel4-io-tcp.zti`, `scripts/lib/link_peer.py`, `scripts/check/check-sel4-io-network-plane.py`, `.tasks/items/` |
| Work items | 01a08ff0-0b99-7315-99b0-7e7f340fa6f9, 01a08ff0-0bae-741e-9318-331fdafe0b96, 01a08ff0-0bee-73ea-9596-27d3940695cc |
| Gates | `just io_tcp_check`, `just link_peer_check`, `just sel4_gate_control_check` |
| Trigger | IO3 moves raw frames and IO4 decides exact destinations, and the roadmap names everything between them as unimplemented; the user asked for TCP |
| Baseline | No ARP, IPv4, ICMP, UDP, TCP, or DNS exists in the tree; `network-service` is bound to a loopback that performs no link operation; components see raw clock ticks with no rate; no generation data names a stack's own address |

## Summary

This opens a lane that puts a bounded IPv4 stack behind the exact-destination authority IO4
already enforces: smoltcp inside `network-service`, consuming IO3's `LinkDevice` below and
serving the existing `network-service/v1` protocol above. The first milestone is a TCP client
byte stream to one generation-declared destination under QEMU, with ARP and ICMP echo on the
service's declared interface; the H1V1's Ethernet controller is a later, separate milestone.
The lane was preceded by a readiness survey and by B93, whose fix landed first because the
same client-reclamation window the block driver had exists in every service that drains a
client's loaned pages. The durable facts, session cut, and technical design are in
[`plan.md`](plan.md).

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/syscall-abi/v1` | `CLOCK RATE READ` (70): the counter's rate under the `monotonicRead` bit; `CAPABILITY NETWORK INTERFACE READ` (71): the interface table on the destination read's shape | A component that may read ticks may know what they mean; a service configures its stack from generation data, never a constant |
| `contracts/network-interface/v1` | One static IPv4 interface per holder: address, prefix, gateway, MAC; decoder refuses non-unicast hosts, off-prefix gateways, multicast MACs | What a stack answers to is declared, bounded, and validated like every other authority |
| `components/services/network-service` | smoltcp `Device` over the IO3 client shape (`link.rs`), a polling loop that drains the link, polls the interface, and serves clients; reset handshake on release | Frames reach the stack only through lent pages the driver completes; the service never exits while the driver still writes its rings |
| `components/testkit/io-tcp-probe`, `sel4-io-tcp` (generation 54) | A client holding one declared destination, the driver, the service with its interface, and the intruder, on one composition | The data plane is its own composition; generation 53 stays the authority-only regression |
| `scripts/lib/link_peer.py`, `check-sel4-io-network-plane.py --arm tcp` | A frame-level peer on QEMU's UDP socket backend with a ledger; the gate cross-checks it against the service's counters | Every frame that leaves the guest is observed by the gate, not merely absent |
| `.tasks/items/` | IO10 (epic), IO11 (milestone, active), P6.F (board milestone) with their dependency edges | The lane's identity and state live in the store |

## Decisions

**A new composition, not an evolution of generation 53.** System-spec baselines are never
re-blessed (`scripts/check/check-system-spec.py`), so changing the components or grants of
`sel4-io-network` would diverge from its frozen baseline. `sel4-io-tcp` carries the data
plane; the authority-only plane keeps its loopback and its markers.

**A polling loop, not a blocking wait set.** A `WaitSet` is one Notification
(`io-link-probe` builds two), the driver signals three distinct notifications, and client
requests arrive on endpoints no wait set covers. The service drains, polls, and yields when
idle, as the driver and the existing service already do; smoltcp's timers are driven by
`now` on each poll and `timer_arm` is unused in this lane.

**A frame-level peer, not QEMU's user-mode NAT.** The scope includes ICMP echo reply, which
slirp cannot inject; the responder is the whole network the plane sees, so "the intruder
emitted no frame" and "no undeclared host was addressed" are observed counts; it uses two
ephemeral loopback ports exactly as `io_link_check` does; and the transport is the verified
IO3 one, so a failure attributes to the new layer.

**Client bytes travel in an IO0 queue per client.** The existing 56-byte request and 24-byte
completion are exactly IO0's payload sizes, and the contract already says data travels in
the buffer slice beside them. No schema change; the endpoint keeps the control operations.

**The interface is generation data through a small new contract.** A constant in the
service would be an environment assumption; extending the destination table's header would
force a major bump of a hardened decoder for an unrelated concern.

**The tick rate is the clock authority's answer.** `CLOCK RATE READ` sits behind the bit
that already admits reading the counter; the MAVLink lane planned the same label and consumes
this one.

## Open risks and follow-ups

- [ ] The first milestone's TCP byte stream (S2) is not yet observed; this entry records the
      lane's opening and the link attachment session.
- [ ] Only four receive pages are in flight; a burst larger than that between polls is dropped
      by the device and reported in the driver's `stalled` count. The board lane owns a
      `DATA_LOANS` bump.
- [ ] The service handles a client's death by never touching that client's pages after its
      endpoint answers negatively; the ordering is the one B93 established.
- [ ] Two open lanes need `CLOCK RATE READ`; the second to land drops its copy.
- [ ] The maintainer's PR #25 regenerates every closure; this lane's closures are regenerated
      on rebase, never hand-merged.

## Artifacts and provenance

- Focused report: this entry; the plan of record in [`plan.md`](plan.md).
- Related work items: IO10 `01a08ff0-0b99-7315-99b0-7e7f340fa6f9`, IO11
  `01a08ff0-0bae-741e-9318-331fdafe0b96`, P6.F `01a08ff0-0bee-73ea-9596-27d3940695cc`;
  the precondition, B93, in [`../2026-09-11-b93-rollback-driver-fault/`](../2026-09-11-b93-rollback-driver-fault/index.md).
