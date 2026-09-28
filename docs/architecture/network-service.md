# Network service

The userspace network service: exact per-holder destination authority over a
smoltcp stack, an external backend on the userspace `virtio-net-driver`'s
`LinkDevice`, a loopback backend without a NIC, bounded TCP streams carried
through application IO0 payload slices, and a bounded HTTP/1.x GET client with
service-owned DNS. It is a consumer of the [I/O substrate](io-substrate.md):
queue, epoch, lease, and reset semantics are that page's; this page owns what
the service adds.

Owners:

- service: `components/services/network-service/src/main.rs`; driver:
  `components/services/virtio-net-driver/src/main.rs`;
- client adapters: `components/lib/src/network_io.rs` (typed synchronous TCP
  over one provisioned ring), `components/lib/src/http.rs` (HTTP/1.x framing,
  transport-independent), `components/lib/src/link_frames.rs`;
- contracts: `contracts/network-service/v1/` (operations, bounds, waiting,
  retry, timeout, cleanup — see its [README](../../contracts/network-service/README.md)),
  `contracts/network-application/v1/`, `contracts/network-destination/v1/`,
  `contracts/network-interface/v1/`, `contracts/link-device/v1/`;
- compositions: `sel4-io-network`, `sel4-io-tcp`, `sel4-io-tcp-impairment`,
  `sel4-io-local`, `sel4-io-lifetime`, `sel4-io-service-fault`,
  `sel4-io-driver-reset`, `sel4-http`, `sel4-http-public` under
  `contracts/system-spec/v1/`;
- plan: [`../plans/network-data-plane.md`](../plans/network-data-plane.md).

## Boundary

The current network service enforces exact per-holder destination authority and
bounded socket/listener/DNS-record accounting. Its application-facing contract
names TCP/UDP operations and typed capability results without NIC identity or raw
packet authority.

## External backend and the TCP stream

When generation data declares a network interface, the service attaches a
`LinkDevice`, replenishes receive buffers, and polls a smoltcp interface.
The `sel4-io-tcp` composition connects it to `virtio-net-driver` and supplies a
real bounded TCP client stream through application IO0 payload slices. Its gate
compares 4096 echoed bytes independently at the application and external frame
peer, verifies handshake and FIN closure, refuses an undeclared address/port,
observes a reset from a second declared port, and retains ARP/ICMP and link
reset/release checks.

## Loss, reordering, retransmission and window bounds

Each external socket owns fixed 2048-byte receive and transmit buffers, and
smoltcp's out-of-order assembler holds four disjoint ranges
(`assembler-max-segment-count-4`); a fifth range is dropped and recovered only
by the peer's retransmission. The service states these bounds and its 10 s
socket timeout at attach (`tcp bounds …`) and, at shutdown, the deepest
receive and transmit queues any socket reached (`tcp peaks …`). A peer that
stops acknowledging exhausts the destination's `retryLimit` or that timeout:
the socket is aborted with one reset, the holder's operations and its close
report `timeout`, and the close releases the connection's reservation
(`tcp timeout handles=1 sockets=1 bytes=4096`).

Received frames reach smoltcp in the order the device completed them, however
recycling placed them in receive slots. smoltcp discards a segment whose
acknowledgment another frame has already overtaken, so reordering inside the
service would turn into loss. A notification-enabled client may send with
`FLAG_NONBLOCKING` to observe a full transmit buffer as `would-block` instead
of waiting through it.

The `sel4-io-tcp-impairment` composition qualifies these bounds behind a
scripted frame peer (`scripts/lib/tcp_impairment_peer.py`) that judges each
scenario from the wire: reordering within and one range beyond the assembler
capacity, guest and peer loss with injected duplicate acknowledgments, a
silent peer followed by a fresh exchange on a single-connection destination,
an application stall that closes the receive window, and a peer that opens
with a zero window. Its probe and service use the notification-backed waiting
profile; the polling profile's clock reads would exhaust a finite plane's
root-request watchdog long before the scenarios finish. TIME-WAIT keeps closed
sockets in the fixed four-socket pool for ten seconds, so the probe retries a
connect refused as `exhausted`. The declared values are in the
[plan](../plans/network-data-plane.md#tcp-loss-reordering-retransmission-and-window-bounds).

## Application authority and listeners

`network-application/v1` declares each application's control/provisioning
bindings, backend, optional readiness/supervision bindings, and separate exact
local-listener authority. Listener rows permit only `127.0.0.1`, a nonzero port,
and one declared local client. Local-peer admission precedes SYN publication;
localhost alone grants no authority. An authority-only composition may still
omit the interface; its legacy endpoint results are bookkeeping, not transport.

The service owns distinct application ring and payload buffers. A nontransferable
control endpoint authorizes provisioning and teardown; a separate transferable
endpoint carries receiver-bound loans, not application commands. The typed
`network_io.rs` adapter preserves partial counts, would-block, EOF and explicit
teardown. External and local backends have separate fixed four-socket pools and
static service-owned RX/TX storage. Limits are authority ceilings, not guarantees
that every declared maximum can be allocated simultaneously; TIME-WAIT retains
pool capacity after handle disposal.

## Local loopback backend

Local TCP uses smoltcp loopback without a NIC. The local QEMU path has observed
connect/listen/accept, 4096/2048-byte bidirectional delivery, EOF, teardown and
coalesced readiness draining. Declared notification profiles use wait sets and
stack deadlines, with a maximum 10 ms idle control deadline for native endpoint
rendezvous. The external polling profile remains supported without readiness
bindings. Blocking send/receive/accept retain validated request descriptors,
not borrowed Rust payload references, for at most four seconds; the readiness
client's five-second bound limits waiting for an absent completion. The local gate has
observed seven authority refusals and a typed four-second receive timeout followed
by successful traffic on the same connection; timeout need not poison a session.

## Lifetime, faults, and reset

The supervised lifetime path has observed a client VM fault after payload
exchange, reclamation of its socket and two session buffers, an actual loopback
RST/reset at the surviving client, and same-boot supervised restart followed by
a fresh exchange and EOF. Handle identity packs a checked service incarnation,
backend and serial; incarnation comes from an explicitly authorized, single-writer
root lifecycle parameter, not a timestamp. It persists across service restarts
within one boot, not across boots. The AArch64 QEMU service-fault profile has also
observed an actual service VM fault with payload work pending, root reclamation,
client faults on revoked mappings, and supervised restart at incarnation 2 with
fresh traffic and EOF. Both restart paths reject raw sends using predecessor
handles and admit fresh identities.

The external driver-reset QEMU profile has observed reset with application receive
work pending, typed `RESET`, socket/session reclamation, and supervised driver,
service and client restart followed by a fresh 4096-byte exchange. Link epochs
come from the root device incarnation: the observed device epochs advanced 1 to 2,
then 2 to 3 on subsequent reset. Reset must consume and settle valid queued IO0
submissions as well as already-admitted requests; leaving a published RX request
outside the admitted table cannot make it disappear. The gate exercises that
queued-RX path, while host tests cover the production settlement helper.

Its controlled external peer acknowledges and withholds echo for the first
1024 bytes, then discards that server session when the restarted client sends a
new SYN with a new initial sequence number. That abandonment is not FIN/RST
closure or transparent TCP continuation. Driver/root markers establish actual
reset and reclamation independently of the peer's fresh-session byte comparison.
Neither this reset path nor the local failure paths qualify physical-device
recovery or ordinary-server interoperability across reset.

## Bounded HTTP client and DNS

The bounded HTTP client uses the same application queues, a launch-supplied URL,
and incremental HTTP/1.x framing. Its exact-name connect path resolves A records
inside the service under separate resolver authority, without turning answers
into numeric client grants. Fresh host-launch entropy is required for DNS query
IDs/source ports. Controlled QEMU networking uses ordinary host TCP/UDP sockets,
not the TCP echo frame peer; public retrieval remains a separate opt-in check.
The network contract owns the precise DNS subset, bounds and authority policy.

## Limits

General application UDP, arbitrary external listeners, IPv6, DHCP, multicast,
routers, and broad smoltcp coverage remain unfinished. Neither the loopback
path nor any QEMU profile qualifies a physical NIC; the controlled external
peer's reset behavior is not ordinary-server interoperability across reset.

## Verification

- `just io_network_check` — all six arms: authority, external TCP, local TCP,
  client lifetime, service fault and driver reset; `just io_tcp_check` selects
  the external stream and its host engine tests;
- `just io_local_check` — declared local listen/accept, duplex bytes and readiness;
- `just io_network_lifetime_check` — client death, reclamation and supervised restart;
- `just io_network_service_fault_check` — service VM fault with in-flight local
  application work, mapping revocation and fresh-incarnation recovery;
- `just io_network_driver_reset_check` — external-link reset with pending payload
  work, root-device epoch advance and controlled-peer fresh-session recovery;
- `just io_network_qualification_check` — aggregate network, link, host, contract,
  image-closure and repository quality gates;
- `just io_http_check` — controlled ordinary host-stack HTTP/DNS, independent body
  and packet comparisons, framing/authority refusals and explicit cleanup;
- `just io_http_public_check` — opt-in native public DNS and `example.com` HTTP,
  with no fixed-address substitution or host HTTP client;
- `just io_http_qualification_check` — network regression plus controlled HTTP
  and a fresh public observation; public unavailability is a failure, not a pass;
- `just io_tcp_host_check` — production TCP engine against real host smoltcp peers,
  independent authority/resource limits, partial I/O, retries and close quarantine;
- `just io_tcp_impairment_check` — scripted reordering, loss, a silent peer and
  zero windows in both directions against the declared bounds, judged from the
  wire, after the peer's own host controls;
