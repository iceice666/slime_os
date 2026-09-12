# IO11: a TCP client byte stream over virtio-net, echoed byte for byte under QEMU

| Field | Value |
|---|---|
| Date | 2026-09-11 |
| Kind | Change |
| Status | Verified |
| Scope | `components/services/network-service/src/main.rs`, `components/testkit/io-tcp-probe`, `contracts/system-spec/v1/systems/sel4-io-tcp.zti`, `contracts/component-spec/v1/components/{io-tcp-probe,network-service}.zti`, `scripts/lib/link_peer.py`, `scripts/check/check-sel4-io-network-plane.py`, `roadmap/11-io-substrate.md` |
| Work items | 01a08ff0-0bae-741e-9318-331fdafe0b96, 01a08ff0-0b99-7315-99b0-7e7f340fa6f9 |
| Gates | `just io_tcp_check`, `just link_peer_check`, `just io_network_check`, `just io_link_check`, `just sel4_gate_control_check`, `just test_host` |
| Trigger | The lane's second session: the service was attached to the link but `OP_CONNECT` still minted an authority-only capability and no byte could move |
| Baseline | [`../2026-09-11-io11-link-attach/`](../2026-09-11-io11-link-attach/index.md): `just io_tcp_check` passing with ARP and ICMP answered, `tcp=0` frames |

## Summary

`OP_CONNECT` to a TCP destination on a bound link now opens a smoltcp socket and answers only
when the handshake settles; a client's bytes travel in an IO0 queue it lends the service,
each request admitted in the authority order before a socket sees it. On `sel4-io-tcp`
the probe lent one queue page and two data pages, connected `10.0.0.2:4242`, sent 4096
seeded bytes in one request and read them back in four completions with zero
mismatches, closed; its connect to `10.0.0.2:4243` came back refused on the peer's RST and its
connect to the undeclared `10.0.0.3` was denied before any frame. The service's ledger read
`tcp sockets opened=2 established=1 reset=1 bytes-tx=4096 bytes-rx=4096`, the intruder's
fourteen denials held, the reset handshake and the healthy graph followed, and the peer's
ledger agreed: one echo flow closed by both FINs with 4096 bytes each way, one refusal, no
undeclared host, no undeclared MAC. IO11 is closed on that observation.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `network-service/src/main.rs` | A TCP connect on a bound link opens a socket with a five-second timeout on an ephemeral port from 49152 and defers its reply: `may_send` answers OK, a closed socket answers `STATUS_UNREACHABLE` (−5); `OP_CLOSE` closes the socket and reuses its buffers only once the stack has let it go | A client is told the destination is open only when it is; a refused destination is a status, never a hang |
| `network-service/src/main.rs` | A client's delegated loans become its data queue (one queue page, up to two data pages, mapped above the link's pages); `OP_SEND`/`OP_RECV` are admitted from that queue in the order decode → capability and holder → right → direction, byte budget, page bounds → queue depth, with IO0 statuses `MALFORMED`, `BAD_RIGHTS`, `BAD_SLICE`, `EXHAUSTED`; a send is completed when every byte is accepted, a receive with at least one byte or `FLAG_END_OF_STREAM`; a closed capability settles its pending requests as `CANCELLED` | No byte touches a socket before every authority arm passes; every consumed request gets a terminal status |
| `components/lib/src/link_frames.rs` | Receive slots hand out frames in delivery order, not slot order: a slot freed and lent again holds a newer frame than higher-numbered slots still unread | Frames reach the stack in the order the device delivered them; a stream never reorders on this side of the link |
| `network-service/src/main.rs` | A client's shutdown reply is the rendezvous: the queue and pages it lent are dropped before the reply goes out, and a closed client is never served again | The root reclaims a dead holder's loans; the service never touches a page it no longer holds (the B93 shape, on the other side of the loan) |
| `network-service/src/main.rs` (review follow-up) | A client that misbehaves is refused and closed, never fatal to the service: a descriptor with no export behind it, a loan the root will not map, a queue formatted with the wrong slot count, a second queue, a page before the queue or beyond the two allowed, or a completion ring the client has wedged each close that client alone. Loans are claimed with `capability_import_from` naming the client's endpoint, so one client's descriptor cannot take up another's export. Non-OK completions carry `transferred = 0`; a receive on a reset socket answers `RESET_BY_PEER` where a peer's FIN answers end-of-stream; a closed capability leaves the table at once and its socket drains in TIME-WAIT without charging the holder's ceiling; a data request on one's own closing capability is no longer counted as a cross-holder refusal; a nonblocking send completes with what was accepted | A client's authority over the service ends at its own queue and sockets |
| `slime-root/src/graph_runtime/services/capability.rs`, `slime-root/src/generation.rs`, `components/runtime` (review follow-up) | `CAPABILITY IMPORT` accepts the receiver's endpoint slot in MR1 and then claims only an export from that endpoint's peer instance; `peer_instance_for_slot` is host-tested | An import names its sender when the receiver has more than one |
| `scripts/generate/generate-system-test-runs.py` (review follow-up) | The tcp arm is an extra closure-backed run of the network checker in the generator's own table, and a per-run device override keeps generation 53's record from declaring a device it never attaches; generation 54 has its own record | Every plane gate has exactly one honest run record |
| `scripts/lib/link_peer.py`, `check-sel4-io-network-plane.py`, `check-link-peer.py` (review follow-up) | The peer keeps each flow's bytes and the gate compares them to the probe's pattern; echo replies must carry the request's bytes from the guest's address; ARP requests for an undeclared host fail the gate; the service's transmitted-frame count must equal what the peer received; a serve failure fails the gate; a known-answer TCP checksum | The two ledgers agree on content, not only on counts |
| `scripts/build/generation_resources.py`, `network-service/src/main.rs` (review follow-up) | Class-E addresses are refused at build as the decoder refuses them; the service's interface page holds the contract's maximum so a fuller table never reads as none | Build and admission agree; a declared interface is read |
| `network-service/src/main.rs` | Socket buffers and storage are static: four sockets, 4 KiB each way | Twice the declared stack never lands on it |
| `io-tcp-probe` | Creates and lends the queue and two pages through the transferable service endpoint with a 64-byte descriptor (buffer, loan, kind), then the stream, close, refused, and undeclared arms | The client's memory is the client's; the service sees only what it was lent |
| `sel4-io-tcp.zti`, `io-tcp-probe.zti`, `network-service.zti` | The probe's endpoint transferable, a `tcp-probe-buffer-factory` grant pinned at slot 1 (`componentAbi`), probe budgets 3/3/3/3, service `mappingCount` 16; the probe requires `sharedBufferFactory`; the pass criterion is the verified stream | Budgets declare what the plane observes |
| `scripts/lib/link_peer.py` | A TCP echo server on 4242 and RST on 4243 with a per-flow ledger; `check-link-peer.py` covers handshake, echo, retransmission re-ack, FIN, and refusal on the host | The peer is a tested oracle, not a second implementation trusted blind |
| `check-sel4-io-network-plane.py`, `check-sel4-gate-controls.py` | Seven more markers in the tcp arm (46 pinned); ledger checks for one closed echo flow of 4096 bytes each way and one refusal | The stream's evidence is two independent ledgers agreeing |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A byte is dropped, reordered, or corrupted between the probe and the peer | `just io_tcp_check` | `stream verified bytes=4096 mismatches=0` missing, or `the echo flow carried rx=… echo=…` |
| A refused destination hangs the client or is reported open | `just io_tcp_check` | `connect dst=10.0.0.2:4243 status=refused` missing, or the 240 s timeout |
| The authority boundary lets an undeclared host be addressed | `just io_tcp_check` | `undeclared destination refusals=1` missing, or `the guest addressed IPv4 hosts the composition does not declare` |
| The close handshake does not complete | `just io_tcp_check` | `the echo flow ended in state …, expected both FINs acknowledged` |
| The peer's TCP server drifts | `just link_peer_check` | Its echo, retransmission, FIN, or refusal scenario |
| A marker is deleted or reordered without notice | `just sel4_gate_control_check` | The pinned count 46 |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just io_tcp_check` | Passed, kept as [`io-tcp-plane.log`](io-tcp-plane.log): `connect dst=10.0.0.2:4242 status=ok`, `sent bytes=4096`, `received bytes=4096 completions=4`, `stream verified bytes=4096 mismatches=0`, `close status=ok`, `connect dst=10.0.0.2:4243 status=refused`, `undeclared destination refusals=1`, `held ms=3000`; service `link frames total=47 tx=21 rx=26 arp=2 icmp=10 tcp=35 other=0`, `link statistics tx=21 rx=26`, `tcp sockets opened=2 established=1 reset=1 bytes-tx=4096 bytes-rx=4096`, `observed requests=24 packets=4 socket_refusals=0 listener_refusals=0 dns_refusals=0 cross_holder_refusals=1`; driver `rx drained=26 replenished=30 stalled=0 tx-stalled=0 device-refused=0`, `fresh epoch old=1 new=2`; `SLIME_GRAPH HEALTHY generation=54`; peer `received 21 (arp-reply=1, icmp-echo-reply=5, tcp=15); sent 28`, one flow to 4242 closed with 4096 bytes each way, one refusal on 4243 | Direct |
| First three boots of the stream | (1) every byte echoed at the peer, none received by the probe: receive slots read in slot order reordered the echo and smoltcp dropped the first segment as a stale duplicate ACK; (2) with delivery order fixed, the stream verified and the service faulted after the probe exited, on the reclaimed queue page; (3) with the queue dropped at shutdown, every arm passed and the graph ended healthy, and only the admission marker's grant count and the peer's five pings, which the shortened probe no longer waited for, remained | Direct |
| `just link_peer_check` | Passed, with the TCP echo server's scenarios | Direct |
| Review follow-ups, whole batch rerun on the final tree | Passed: `just io_tcp_check` (the transcript above is this run: same counts, the gate now also comparing the stream's bytes, the echo replies' bytes and source, ARP targets, and `tx=21` against the peer's 21), `just io_network_check`, `just io_link_check`, `just sel4_root_boot_check`, `just test_sel4_root` (217), `just test_host`, `just link_peer_check` (with the known-answer checksum), `just contracts_check`, `just component_spec_check`, `just component_crate_split_check`, `just system_composition_closure_check`, `just system_image_closure_check`, `just system_test_run_check` (one record per arm of the shared checker), `just system_image_closure_aggregate_check`, `just sel4_gate_control_check`, `just deny`, `just machete`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos`, `just devlog_check`, `just tasks_check` | Direct |
| `just io_network_check` (both arms) | Passed: generation 53's 16 markers unchanged with the modified service, and the tcp arm again | Direct |
| `just io_link_check` | Passed: generation 52's 28 markers unchanged | Direct |
| `just sel4_gate_control_check` | Passed: 48 gates, 1917 mutated transcripts rejected, the network plane pinned at 47 markers | Direct |
| `just test_host` | Passed, 535 tests, including `receive_frames_are_taken_in_delivery_order_not_slot_order` | Direct |
| `just contracts_check`, `just component_spec_check`, `just component_crate_split_check`, `just system_spec_check`, `just system_composition_closure_check`, `just system_test_run_check`, `just system_image_closure_check`, `just system_image_closure_aggregate_check`, `just test_sel4_root` (215) | Passed; the refreshed `sel4-io-tcp` baseline reproduces from its spec | Direct |
| `just deny`, `just machete`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos`, `just devlog_check`, `just tasks_check` | Passed | Direct |

## Decisions

- Decision: the receive tracker orders by delivery sequence, found by the first stream boot rather than by design.
- Rationale: the first S2 boot echoed every byte at the peer and verified none at the probe. smoltcp's own debug log, routed to the serial console for one diagnostic build, showed the peer's first echo segment processed after its successor and then dropped as a "duplicate ACK" — smoltcp discards the whole segment when its acknowledgement number is stale — while the peer, which never retransmits, left the hole permanent until the five-second timeout. The link tracker took ready frames by slot index; once ingress paused on transmit pressure with slots 1..3 unread, slot 0 was lent again and its newer frame read first. `receive_frames_are_taken_in_delivery_order_not_slot_order` pins the order on the host.
- Rejected alternative: a peer that retransmits; it would have hidden the reorder.
- Decision: a client's queue is dropped when its shutdown is accepted, before the reply.
- Rationale: the second S2 boot verified the whole stream and then faulted the service as the probe exited: the root reclaimed the probe's three loans, and the service's next iteration polled the reclaimed queue page. The reply is the only rendezvous the endpoint offers, so everything lent must be forgotten before it.
- Rejected alternative: a second "goodbye" round trip; the reply already orders the two exits.
- Decision: the connect reply is deferred until the handshake settles, bounded by the socket's own timeout.
- Rationale: the client's `call()` is a blocking send and receive; an immediate OK would report a destination open that the peer may refuse a round trip later, and an unbounded wait would turn a silent peer into a hung plane.
- Rejected alternative: answering OK at once and reporting the refusal on the first data request.
- Decision: one IO0 queue per data client, with the existing 56-byte request and 24-byte completion as payloads.
- Rationale: the endpoint's 64-byte message cannot carry a slice, IO0 already defines lease-bound slices and terminal statuses, and the network-service schema's comment already places the bytes in the IO0 buffer slice; no contract changed.
- Rejected alternative: a new data-plane protocol contract.
- Decision: `sel4-io-tcp`'s baseline was refreshed with S2's grants and budgets.
- Rationale: the baseline exists to catch a derivation drifting from a hand-authored fixture; `sel4-io-tcp` has no such ancestor, its baseline is the S1 derivation copied verbatim, and the plane had not landed when its grants grew. A landed plane's baseline stays frozen; this one becomes frozen with the lane's first merge.
- Rejected alternative: a second composition for the data path, leaving generation 54 as a link-attach regression.

## Open risks and follow-ups

- [ ] `timerBudget`, `retryLimit`, and `reconnectLimit` remain decoded only.
- [ ] A client that dies without the shutdown rendezvous is invisible to the service, which then faults on the reclaimed page: no mechanism places a supervision capability between root-launched instances. Filed as `01a09446-bbf5-7df6-9b22-ba7f65c0ed76`.
- [ ] End-of-stream (`FLAG_END_OF_STREAM`) is implemented for a peer that closes first, and unobserved: the echo peer closes only after the client.
- [ ] A socket in `TIME_WAIT` holds its buffers for smoltcp's close delay; with four sockets and one client that never bites, and the service exits regardless.

## Artifacts and provenance

- Focused report: this entry; the lane's plan in [`../2026-09-11-io-tcp-lane/plan.md`](../2026-09-11-io-tcp-lane/plan.md).
- Raw transcript: [`io-tcp-plane.log`](io-tcp-plane.log), the gate's serial capture and peer summary from the passing run.
- Related work item: IO11, `.tasks/items/01a08ff0-0bae-741e-9318-331fdafe0b96.md`.
