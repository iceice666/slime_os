# IO4: the bounded network service and exact destination authority

| Field | Value |
|---|---|
| Date | 2026-08-28 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/network-service/v1/`, `contracts/network-destination/v1/`, `boot-contracts/src/network_destination.rs`, `components/proto/src/network_service.rs`, `components/services/network-service/`, `components/testkit/io-network-{probe,intruder}/`, `components/testkit/io-link-loopback/`, `contracts/generation-manifest/v1/compositions/sel4-io-network.zti`, `scripts/check/check-sel4-io-network-plane.py`, `scripts/build/build-{generation,sel4}.py`, `scripts/check/check-sel4-gate-controls.py`, `just/planes-*.just` |
| Roadmap | IO4 |
| Gates | `just io_network_check`, `just sel4_gate_control_check` |
| Trigger | IO4 is the consumption point for ROS R0/R1, foreign workloads, and Framework H6; each of those was blocked on there being one network architecture rather than several. |
| Baseline | No network stack, no socket concept, and no destination authority existed anywhere in the tree. IO0's queue substrate and the `LinkDevice` protocol from IO3 were the only pieces in place. |

## Summary

IO4's load-bearing claim is not "Slime has TCP" — it is that a component reaches *exactly* the destinations its generation declared, and nothing else. This lands the bounded network service, the `NetworkDestination` generation resource object with separate CONNECT/SEND/RECV/LISTEN rights, and a QEMU plane in which a granted client reaches its one declared endpoint while eleven distinct denial arms each observe zero packets. Wildcards are not merely refused; they are *unrepresentable* — the contract has no netmask, prefix length, port range, or wildcard-name field to write one in.

The slice is complete for the subset it declares and observed under QEMU. What it does not implement is stated explicitly below rather than left to be discovered by the first consumer.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/network-destination/v1/` | Generation resource object: exact transport, exact IPv4/IPv6 address or exact bounded DNS name, exact port, separate CONNECT/SEND/RECV/LISTEN right bits, and per-destination queue/byte/timer/retry/reconnect/socket/listener/DNS-record ceilings | Reachability is generation data, enumerable by tooling, and a wildcard cannot be spelled |
| `boot-contracts/src/network_destination.rs` | Handwritten decoder with full structural and resource-bound validation, plus the `authorizes(...)` predicate that matches on every field and the exact right bit | The authority decision is one testable function rather than scattered checks in a service |
| `contracts/network-service/v1/` + `components/proto/src/network_service.rs` | Client-facing control protocol (connect, send, recv, close, listen, accept, resolve) with 56/24-byte payloads riding inside the IO0 envelope | Clients hold typed `TcpConnection`/`UdpEndpoint` authority; the protocol surface has no NIC handle, raw-frame op, or resolver-wide op to reach for |
| `components/services/network-service/` | Bounded Ethernet/ARP/IPv4/ICMP/UDP and a bounded TCP state machine, plus exact-name DNS; every bound read from generation data rather than a component constant | Policy lives in userspace; the root learns no IP, DNS, UDP, TCP, or destination concept |
| Root resource read | Authenticated generation destination-table read, modelled on the existing cursor-paged `GRAPH_READ` path | The root serves bytes it already authenticates, per-caller, with no new policy |
| `components/testkit/io-link-loopback/` | Deterministic in-plane `LinkDevice` provider speaking `contracts/link-device/v1` over IO0 queues | The service's link boundary is proved to be the declared capability, not a particular NIC |
| Plane and gate plumbing | Composition at generation 53, plane checker, variant/build registration, `GATES` entry, `io_network_check` target | The claim cannot pass on missing, reordered, or failing evidence |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A destination check is relaxed and an alternate address/port/name becomes reachable | `cargo test -p boot-contracts --lib network_destination` plus the eleven plane denial markers | A denial arm reports a nonzero packet count, or an `authorizes` fail-closed test passes a mismatch |
| Wire drift between service and client | `python3 scripts/generate/generate-network-service-bindings.py --check` via `just contracts_check` | Stale-bindings failure |
| The service stops enforcing bounds end to end | `just io_network_check` | A missing or out-of-order causal marker |
| The gate could pass on absent evidence | `just sel4_gate_control_check` | 44 gates / 1731 mutations no longer all reject |
| Reset or restart leaks a queue, buffer, or lease | The numeric reclamation markers in the plane transcript | A settled/queues/buffers/leases count disagrees, or `outstanding` is nonzero |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just io_network_check` | PASS — built `build/slime-sel4-io-network.elf`, booted under QEMU, reported `exact destinations, denials, reset, restart, and backend independence proved`. Re-run independently by the integrating agent with the same result. | Direct |
| `just sel4_gate_control_check` | PASS — 44 gates reject 1731 mutated transcripts and layouts | Direct |
| `python3 scripts/check/check-contracts.py` | PASS — contract bindings plus 287 boot-contract tests | Direct |
| `cargo test -p boot-contracts --lib network_destination` | PASS — 3 passed, 0 failed | Direct |
| `cargo test -p slime-proto --test network_service` | PASS — 3 passed, 0 failed | Direct |
| Granted path observed | `authority destinations=3 rights=connect,send,recv,listen`; `exact tcp destination connected`; `deterministic length-prefixed transfer bytes=12 echoed=12`; `exact dns resolved name=echo.test address=10.0.0.2`; `exact udp endpoint connected` | Direct |
| Denials observed | `simultaneous denied endpoint packets=0`, plus separate `packets=0` arms for alternate address, alternate port, alternate DNS name, wrong transport, missing CONNECT, missing SEND, missing RECV, missing LISTEN, raw-packet attempt, resolver-wide lookup, and listen without LISTEN | Direct |
| Reclamation observed | `link reset settled=2 queues=2 buffers=2 leases=2 outstanding=0`, `fresh epoch=2 stale epoch=1 refused reconnects=1`; `service restart settled=1 queues=2 buffers=2 leases=1 outstanding=0`, `fresh epoch=3 stale completion refused` | Direct |

## Decisions

- **Decision:** Wildcards are unrepresentable rather than validated-against.
  **Rationale:** A schema with a netmask field and a checker that rejects non-`/32` values is one relaxed check away from wildcard reachability. Omitting the field means no code path exists to widen.
  **Rejected alternative:** A prefix-length or port-range field with a strictness check.

- **Decision:** CONNECT, SEND, RECV, and LISTEN are four separate right bits on each destination.
  **Rationale:** "May talk to this endpoint" is four different authorities. A receive-only diagnostic client and a send-only telemetry client are both expressible, and the plane proves each missing bit denies independently.
  **Rejected alternative:** One `use` right per destination.

- **Decision:** Proved against a deterministic in-plane loopback `LinkDevice` rather than waiting for IO3's virtio-net backend.
  **Rationale:** Backend independence is an explicit IO4 deliverable ("keep target backends separate"), so exercising the service against a second backend is *stronger* evidence for the claim IO4 actually makes than exercising it against the reference NIC would be. IO3's virtio-net backend was still blocked on IO1's authority path landing, and IO4's claim is the service and its authority, not the NIC.
  **Rejected alternative:** Serialising IO4 behind IO3, which would have coupled a policy claim to a driver's schedule for no gain in what is proved.

- **Decision:** The destination table reaches the service through the existing cursor-paged authenticated-read idiom (`GRAPH_READ`'s pattern), not a new bespoke root API.
  **Rationale:** The root already authenticates and pages generation resource bytes per caller for the graph and the wait set. Reusing that shape keeps the root serving bytes rather than deciding network policy.
  **Rejected alternative:** A network-specific root call, which would have put destination vocabulary in the root's ABI.

- **Decision:** Declare IPv6/NDP, DHCP, SLAAC, and the TCP listener data path in the contract but structurally refuse them as `STATUS_UNSUPPORTED`, and say so loudly.
  **Rationale:** An honest bounded subset is required; a stub pretending to be complete is not. Keeping them declared means adding them later is an implementation change rather than a contract version bump, and the explicit refusal means a consumer discovers the gap at admission rather than at runtime.
  **Rejected alternative:** Silently omitting them from the contract, which would have made the first DHCP-needing consumer a contract change; or implementing thin stubs, which would have let a caller believe it had DHCP.

## Open risks and follow-ups

- [ ] IPv6/NDP, DHCP, and SLAAC are declared and refused. Any consumer needing address autoconfiguration — a physical RPi5 or Framework path — must treat IO4 as unfinished for that purpose.
- [ ] The TCP listener/accept data path is refused beyond a structured denial. LISTEN authority is expressible and enforced, but nothing accepts yet.
- [ ] The TCP state machine is bounded to what this plane exercises: one connection carrying length-prefixed bytes. R0/RP5's Zenoh Profile 0 path is exactly that shape, but it has not been run against this service.
- [ ] Proved only against the loopback backend. IO3 owns the virtio-net reference path; a physical link remains H6/H12/RP work.
- [ ] Authority-diff tooling enumerates destinations via the composition and the plane's inventory marker. If a future audit surface wants a first-class destination diff, it should read the same resource object rather than re-deriving.

## Artifacts and provenance

- Focused report: none; the contract comments in `contracts/network-destination/v1/schema.zt` and the decoder docs in `boot-contracts/src/network_destination.rs` carry the authority rationale.
- Raw transcript: none retained; reproduce with `just io_network_check`.
- Serial/debugger/model output: the marker chains under *Verification*, as asserted by `scripts/check/check-sel4-io-network-plane.py`.
- Related roadmap item: [IO4 — Network service and exact destination authority](../../roadmap/11-io-substrate.md#io4--network-service-and-exact-destination-authority)
