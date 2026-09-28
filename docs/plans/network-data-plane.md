# Network data plane over LinkDevice

**Canonical work items:** short-term TCP epic
`01a08ff0-0b99-7315-99b0-7e7f340fa6f9`; demo listener/loopback facilities
`01a0ddaa-825f-7309-8508-ed49ffe33a8d`; long-term smoltcp coverage
`01a0ddaa-8676-7d7e-8faf-47285166731f`; native HTTP/DNS
`01a0e12e-32fe-71e6-bea4-48c1c007f04c`.

## Current gap

The product has a userspace virtio-net `LinkDevice` and a destination-authority
service. The `sel4-io-tcp` composition already connects them: generation-declared
interface data configures smoltcp, the service exchanges Ethernet/ARP/IPv4/ICMP
traffic, and shutdown resets/releases the link. An authority-only composition
may omit the interface.

The external TCP-client slice now connects actual application IO0 payloads to
bounded smoltcp sockets. Its QEMU gate compares a 4096-byte stream with an external
frame peer, including handshake, close, refusal and forbidden-egress checks;
[the current architecture](../architecture/network-service.md)
and [protocol semantics](../../contracts/network-service/README.md) own that
implementation and its limits. The first stream item is
`01a08ff0-0bae-741e-9318-331fdafe0b96`; state remains in the work-item store.

Generation-declared application bindings, separate exact local listener/peer
authority, real loopback connect/listen/accept and notification-backed bounded
waiting are now implemented. Local QEMU has observed bidirectional bytes, EOF,
normal teardown and coalesced readiness. The supervised lifetime path has observed
client death, socket/session-buffer reclamation, a reset at the surviving peer,
and same-boot restart followed by a fresh exchange. The AArch64 QEMU service-fault
profile additionally observes a service VM fault with payload work pending, root
reclamation, client faults on revoked mappings, and fresh-incarnation recovery.
Both restart paths reject predecessor handles. A separate external-driver-reset
QEMU gate now observes pending receive reset, queued IO0 settlement, device epoch
advance and supervised recovery with a fresh 4096-byte stream. Its controlled peer
abandons the first acknowledged, unechoed 1024-byte session on a new SYN; this is
not transparent continuation or ordinary-server interoperability across reset.
Local qualification also observes authority refusals and a typed receive timeout
followed by resumed traffic. Physical-device recovery, general application UDP,
arbitrary external listeners and broader backends remain separate work. The
bounded HTTP/DNS slice below extends this transport without broad DNS coverage.

## Planned scope

Complete bounded application transport inside `network-service`, consuming the
existing LinkDevice attachment and preserving its interface contract:

1. extend the external TCP payload transport with exact listener/accept and local
   TCP for the short-term Zenoh demo; broaden to authority-compatible smoltcp
   facilities afterward, with versioned Zutai contracts and bounded state;
2. exact-name or exact-address destination authority with independent CONNECT,
   SEND, RECV, and LISTEN rights; no wildcard or ambient socket grant;
3. typed service capabilities such as `TcpConnection`, `UdpEndpoint`, and
   listeners, while clients receive no NIC, raw-packet, resolver-wide, or DMA
   authority;
4. bounded queues, fragments, retransmission state, timers, DNS records,
   reconnect attempts, sockets, listeners, and bytes per destination;
5. driver/service reset, peer loss, stale completion, and supervised restart
   behavior over fresh IO0 epochs.

Address configuration not already declared by current contracts requires its own
schema change and evidence. IPv6/NDP, DHCP, SLAAC, multicast discovery, and a
general listener/accept service are not implied by the first TCP slice.

## Short-term ROS 2 Zenoh facilities

The [format-2 demo contract](../../contracts/rpi5-ros2-demo/v2/README.md)
selects a static peer session: one connect endpoint and one listen endpoint at
`127.0.0.1:7447`. The external TCP-client slice alone cannot satisfy it. The
local implementation now supplies the byte-transport and readiness foundation;
the following obligations remain the qualification boundary, not an implication
that the ROS/Zenoh integration itself is complete:

- real TCP listen/accept and local loopback between separately authorized
  components, with no external NIC dependency or loopback traffic escape;
- generation-declared client bindings, exact local listener authority and
  admitted-peer policy; accepting a connection must not authorize an arbitrary
  peer, and localhost grants nothing implicitly;
- application IO0 payload slices/leases connected to bounded socket RX/TX state,
  with actual byte counts, partial read/write, backpressure, EOF and errors;
- socket readiness and stack deadlines integrated with existing clocks, timers
  and wait sets, including coalesced-wake draining and bounded idle waiting;
- bounded listeners, accept queues, sockets, bytes, timers and retries, and
  exactly-once request settlement or invalidation with charge/lease reclamation
  on peer loss, client death, driver reset and service restart.

Contracts must distinguish local bind/listener identity from remote destination
identity where needed; do not reinterpret one address/port tuple ambiguously.
Use real middleware-role bindings rather than borrowing the probe identities.
Qualification must compare bidirectional bytes, including split/coalesced
length-prefixed batches; network-service remains a byte transport, not a Zenoh
parser. A capability result or logical `packets` counter is not wire evidence.

The ROS runtime item `01a0724c-5400-7b6b-ad3a-f388c30d5ed1` depends on these
facilities. Zenoh session/framing, declarations, CDR and ROS semantics remain in
`01a00b4d-2400-7987-93d6-19201c397dff`. UDP, DNS, DHCP, IPv6, multicast scouting,
routers and broad smoltcp coverage are not short-term demo prerequisites. Local
TCP proves no physical NIC; actual board and external-peer evidence remain
separate obligations.

## Native HTTP and bounded DNS

Work item `01a0e12e-32fe-71e6-bea4-48c1c007f04c` carries the native HTTP GET
requirements. Delivery proceeds through ordinary-server fixed-address HTTP,
exact-name service-owned DNS/connect, then an explicitly requested public
`http://example.com/` observation. A parser test or controlled echo does not
satisfy either ordinary-server interoperability or the final public observation.

The HTTP parser lives in `components/lib/src/http.rs`, separate from network
policy. It uses fixed storage, incremental partial input, bounded headers,
trailers, informational responses, chunk metadata and streamed body bytes.
Content-Length, chunked and orderly-close-delimited responses are distinct;
reset/timeout never substitutes for EOF. Conflicting lengths, transfer/content
codings outside the supported profile, upgrades and URL/header injection are
refused. Completed non-2xx responses remain HTTP responses, not transport errors.

A launch-supplied URL reaches the native client over its explicit endpoint using
the [network launch contract](../../contracts/network-service/README.md#bounded-http-launch-input),
not a compiled URL or ambient environment. Console body chunks are hex-encoded
separately from diagnostics so an untrusted response cannot impersonate service
or qualification markers. This is bounded body streaming, not terminal rendering
of arbitrary control bytes.

DNS is scoped to the original holder/name/TCP/port grant. The service retains
resolution results and returns only a connection; answers and CNAME targets
never mint numeric-address permissions. Resolver traffic requires its own exact
service destination authority. Controlled host-address answers require a separate
exact numeric destination grant; public profiles must not inherit that exception.
Fresh launch entropy is mandatory for query-ID/source-port unpredictability;
this mitigates guessing, not cryptographic DNS authentication.

Qualification must compare actual guest body bytes with ordinary host TCP/UDP
server observations over QEMU user networking, retain current network regression,
and cover negative framing, DNS, authority and cleanup cases. Public retrieval
is opt-in and separate from the offline controlled suite. Completion still
requires a genuine observation tied to target, full revision, image identity,
queried name, returned/selected address, status, byte count and cleanup. Missing
or unreachable public DNS/HTTP is unavailable/failure, never a local substitution.
These obligations do not qualify TLS, physical NICs or IOMMU containment.

Run controlled cases with `just io_http_check`; select one case or retain serial
and packet evidence through the owning checker:

```sh
python3 scripts/check/check-sel4-io-network-plane.py --arm http --http-case dns-content-length --transcript build/http-controlled.log
```

`just io_http_public_check` explicitly opts into live traffic. The equivalent
checker invocation below keeps the transcript and packet capture; it must report
failure if public DNS/HTTP cannot be reached. It uses the public composition's
exact resolver and hostname authority, not the controlled server.

```sh
python3 scripts/check/check-sel4-io-network-plane.py --arm http-public --allow-public --transcript build/http-public.log
```

`just io_http_qualification_check` combines current network regression, controlled
HTTP and a fresh public smoke for one devloop execution identity. No recorded
body string or fixed status is required for the public response. The maintained
work item must remain active until every required observation is qualified;
adding these entry points is not completion evidence.

## Long-term authority-compatible smoltcp coverage

The goal is the widest applicable surface of a **pinned smoltcp release**, not
all Cargo features enabled together or a promise to implement every future
upstream feature. The [support matrix](#smoltcp-support-matrix) below is the
complete inventory of that release's protocols, sockets, media, configuration
and resource options; implementation state and dependency edges stay in MyQue. Each matrix row must name its upstream feature/source, applicable
backend/target, authority mapping, quantitative bounds, implementation slice and
verification. Distinguish supported, planned, excluded for a concrete authority
conflict, and not applicable/not provided by the pinned release. Unfinished
work is not an authority exclusion. `just smoltcp_matrix_check` enforces the
matrix's format and its agreement with the lockfile, the checksum-verified
crate archive and the network service's enabled features; the rules are in
`scripts/lib/smoltcp_matrix.py`.

The initial areas to assess and deliver are:

| Area | Required boundary |
| --- | --- |
| TCP | Complete bounded stream/listener behavior and applicable pinned options; preserve the demo path |
| UDP | Explicit local bind and per-peer datagrams, message boundaries, source identity, truncation/oversize policy and bounded queues; static DDS endpoints first |
| DNS | Exact-name authority, bounded records/retries/expiry and resolution-to-use policy; replies or rebinding never authorize an unrelated destination |
| IPv6/NDP, ICMP, routing | Explicit service control-plane authority, bounded neighbor/route state and no client raw-packet bypass |
| Address configuration | Assess DHCPv4 and any supported SLAAC; leases/advertisements configure an authorized interface, never mint client destination rights |
| Multicast | Explicit group/interface/port/direction, sender policy and membership lifetime; supported maintenance such as IGMP stays behind those grants |
| Fragmentation/reassembly | Bound bytes, fragments, concurrent assemblies and expiry; reject malformed/overlapping input according to the admitted profile |
| Media/backends | Assess every pinned medium separately; framing belongs to authorized services, not ordinary raw-socket clients; no inherited physical claim |
| Resource/profile options | Record supported configurations and bounds without treating every tuning flag as a new service |

Multicast is not categorically incompatible: admit a bounded explicit grant if
it preserves the model. Discovery advertisements, DNS answers, DHCP leases and
peer locators are untrusted data, not authority. General LAN discovery, wildcard
bind/connect/listen, resolver-wide access and ordinary-client packet injection
or sniffing must not enter through a compatibility shortcut. Where a feature
cannot be expressed with explicit holders, operations, destinations and bounds,
skip it and record the violated invariant, considered bounded alternative and
executable denial or build-time exclusion. Raw-IP framing inside an authorized
stack is not the same thing as raw-packet authority for an application.

The epic is decomposed into small independently trackable children in the
work-item store, each binding the `just-target` gate: the matrix and its
feature-delta check (`01a0e239-5106-7bb7-b6a3-a83ab346c183`, first, because
every other slice records its row there); TCP options, external listener with
half-close, and loss/reorder/window bounds; UDP endpoints, then UDP bounds with
the static DDS endpoint profile; DNS generalization; ICMP probes and error
mapping; IPv4 fragmentation; static IPv6/NDP, then IPv6 transport and SLAAC;
DHCPv4; a multicast-capable peer backend, then multicast grants with IGMP;
media/backend classification; saturation bounds; and last the aggregate
qualification (`01a0e239-eb09-7748-811b-3302fb4b9fa0`), which depends on every
other child and is the epic's closing gate. Dependency edges and state live only
in the store.

UDP and multicast facilities do not implement DDSI-RTPS discovery, reliability,
history, CDR or QoS matching. Those remain middleware work. Likewise, TLS/DDS
Security and physical NIC/IOMMU qualification are separate from smoltcp feature
coverage. Admit independently verifiable implementation children before their
code, after the short-term facilities; an inventory or an echo test alone cannot
close the long-term epic. Completion requires every applicable row qualified or
explicitly excluded on authority grounds, with no unresolved planned entries.
Upgrading smoltcp requires a feature-delta review rather than silently expanding
the support claim.

## smoltcp support matrix

Pinned release: smoltcp `0.13.0`, crate checksum `ac729b0a77bd092a3f06ddaddc59fe0d67f48ba0de45a9abe707c2842c7f8767`.

This matrix covers the pinned release only. `just smoltcp_matrix_check`
compares it with `Cargo.lock`, the checksum-verified crate archive and the
feature closure `components/services/network-service/Cargo.toml` enables; the
rules are in `scripts/lib/smoltcp_matrix.py`. A version bump, a new upstream
feature or an enabled feature without a `supported` row fails that check until
this page is reviewed. `supported` rows are QEMU evidence only and carry no
physical NIC claim. `planned` rows name the owning slice, which records the
selected value, bounds and verification here when it lands. `not-applicable`
rows are alternative values of a selected resource option, or facilities with
nothing to deliver. No row is `excluded` yet: exclusions are decided by their
owning slices, chiefly the media and backend classification.

### Facilities

| Row | Kind | Upstream | Status | Backend | Slice | Authority | Bounds | Verification |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| Ethernet and ARP | medium | `src/iface/interface/ethernet.rs` | supported | QEMU `LinkDevice` | 01a08ff0-0b99-7315-99b0-7e7f340fa6f9 | Frames cross only the service's `LinkDevice` capability | 4 RX and 4 TX frames, 1514 bytes, 4 neighbors | `just io_network_qualification_check` |
| IP medium | medium | `src/phy/mod.rs` `Medium::Ip` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | Outside the admitted Ethernet backend | Declared by the owning slice | Owning slice's recipe |
| IEEE 802.15.4 medium | medium | `src/iface/interface/ieee802154.rs` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | Outside the admitted Ethernet backend | Declared by the owning slice | Owning slice's recipe |
| IPv4 static interface | protocol | `src/iface/interface/ipv4.rs` | supported | QEMU `LinkDevice` | 01a08ff0-0b99-7315-99b0-7e7f340fa6f9 | Address, prefix and gateway come from generation data | 1 address, 1 route | `just io_network_qualification_check` |
| ICMPv4 echo reply | protocol | `src/iface/interface/ipv4.rs` | supported | QEMU `LinkDevice` | 01a08ff0-0b99-7315-99b0-7e7f340fa6f9 | Automatic reply to the service's own address only | Same frame budget | `just io_network_qualification_check` |
| TCP external connect | socket | `src/socket/tcp.rs` | supported | QEMU `LinkDevice` | 01a08ff0-0b99-7315-99b0-7e7f340fa6f9 | Service-held; clients reach it only through exact destination grants | 4 sockets, 2048-byte buffers, 10 s timeout | `just io_network_qualification_check` |
| TCP loopback connect, listen and accept | socket | `src/socket/tcp.rs` | supported | Loopback inside the service | 01a0ddaa-825f-7309-8508-ed49ffe33a8d | Exact local listener grants; no external egress | 4 sockets, 2048-byte buffers | `just io_tcp_check` |
| TCP external listener, half-close and simultaneous close | socket | `src/socket/tcp.rs` | planned | QEMU `LinkDevice` | 01a0e239-ad77-7754-874f-b8703be81c04 | Exact external listener grants with admitted peer policy | Declared by the owning slice | Owning slice's recipe |
| TCP loss, reordering, retransmission and window | resource | `src/socket/tcp.rs`, `src/storage/assembler.rs` | planned | QEMU `LinkDevice` | 01a0e239-b16f-735e-9d04-417a547a42e3 | Service-held; clients reach it only through exact destination grants | Declared by the owning slice | Owning slice's recipe |
| TCP options: keepalive, Nagle, hop limit, ack delay, congestion control | configuration | `src/socket/tcp.rs` | planned | QEMU `LinkDevice` | 01a0e239-a96d-7db6-8b7f-d6efef5f8b19 | Declared per application binding, never client-chosen | Declared by the owning slice | Owning slice's recipe |
| TCP operation timeout and disabled delayed ACK | configuration | `src/socket/tcp.rs` | supported | QEMU `LinkDevice` and loopback | 01a08ff0-0b99-7315-99b0-7e7f340fa6f9 | Fixed service policy | 10 s operation timeout, no ack delay | `just io_network_qualification_check` |
| UDP resolver socket | socket | `src/socket/udp.rs` | supported | QEMU `LinkDevice` | 01a08ff0-0b99-7315-99b0-7e7f340fa6f9 | Service resolver only; answers never authorize destinations | 1 socket, 512-byte packets | `just io_network_qualification_check` |
| UDP application endpoints | socket | `src/socket/udp.rs` | planned | QEMU `LinkDevice` | 01a0e239-b54c-7782-8dd6-b403038fa3dd | Exact local bind and per-peer datagram grants | Declared by the owning slice | Owning slice's recipe |
| UDP bounds, truncation and DDS endpoint profile | resource | `src/socket/udp.rs` | planned | QEMU `LinkDevice` | 01a0e239-b927-7d7d-bb54-16c2bc465faa | Declared queues and datagram sizes | Declared by the owning slice | Owning slice's recipe |
| DNS | protocol | `src/socket/dns.rs`, `src/wire/dns.rs` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Exact-name grants; resolution never authorizes an unrelated destination | Declared by the owning slice | Owning slice's recipe |
| ICMP sockets and error mapping | socket | `src/socket/icmp.rs` | planned | QEMU `LinkDevice` | 01a0e239-c1db-736d-a7a3-7ab4dd179e93 | Service-held probes behind explicit echo grants | Declared by the owning slice | Owning slice's recipe |
| IPv4 fragmentation and reassembly | protocol | `src/iface/fragmentation.rs` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Service control plane only | Declared by the owning slice | Owning slice's recipe |
| IPv6 static interface, NDP and ICMPv6 | protocol | `src/iface/interface/ipv6.rs` | planned | QEMU `LinkDevice` | 01a0e239-ca2d-78c4-b9e1-689e6db39e9f | Service control plane only | Declared by the owning slice | Owning slice's recipe |
| TCP and UDP over IPv6 | protocol | `src/socket/tcp.rs`, `src/socket/udp.rs` | planned | QEMU `LinkDevice` | 01a0e239-ce56-7d42-9e8a-3b4aa5dce585 | Exact IPv6 grants; no cross-family authority | Declared by the owning slice | Owning slice's recipe |
| IPv6 SLAAC and router advertisements | configuration | `src/iface/slaac.rs` | planned | QEMU `LinkDevice` | 01a0e239-d614-767b-95d0-9c7261951e72 | Configures an authorized interface only | Declared by the owning slice | Owning slice's recipe |
| DHCPv4 | configuration | `src/socket/dhcpv4.rs` | planned | QEMU `LinkDevice` | 01a0e239-d236-753f-ad60-f56c378eb2fe | Configures an authorized interface only | Declared by the owning slice | Owning slice's recipe |
| Multicast, IGMP and mDNS | protocol | `src/iface/interface/multicast.rs`, `src/socket/dns.rs` (mDNS) | planned | Multicast-capable peer | 01a0e239-daa7-7fb9-b1c7-8c7e9336d95a, 01a0e239-de7f-7bb7-ae5c-756b8cbfe637 | Explicit group, interface, port and direction grants | Declared by the owning slice | Owning slice's recipe |
| 6LoWPAN and RPL | protocol | `src/iface/interface/sixlowpan.rs`, `src/iface/rpl/` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | Outside the admitted set | Declared by the owning slice | Owning slice's recipe |
| Raw sockets | socket | `src/socket/raw.rs` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | Applications never gain raw-packet authority | Declared by the owning slice | Owning slice's recipe |
| IPsec AH and ESP | protocol | `src/wire/ipsec_ah.rs`, `src/wire/ipsec_esp.rs` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | Outside the admitted set | Declared by the owning slice | Owning slice's recipe |
| Bounds under malformed and saturating traffic | resource | `src/iface/`, `src/socket/` | planned | QEMU `LinkDevice` | 01a0e239-e6af-7561-92fc-334aa2ba28df | Service state stays within every declared bound | Declared by the owning slice | Owning slice's recipe |

### Cargo features

Every feature the pinned release advertises, including the implicit features
of its optional dependencies, has exactly one row.

| Row | Kind | Upstream | Status | Backend | Slice | Authority | Bounds | Verification |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `_netsim` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Crate-private network simulator used by upstream tests; not a public facility | — | — |
| `_proto-fragmentation` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Private fragmentation core implied by every fragmentation feature | Declared by the owning slice | Owning slice's recipe |
| `alloc` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | Heap-growable storage; follows the `std` classification | Declared by the owning slice | Owning slice's recipe |
| `assembler-max-segment-count-1` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-b16f-735e-9d04-417a547a42e3 | TCP out-of-order assembler capacity, selected with the loss and reorder bounds | Declared by the owning slice | Owning slice's recipe |
| `assembler-max-segment-count-2` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-b16f-735e-9d04-417a547a42e3 | TCP out-of-order assembler capacity, selected with the loss and reorder bounds | Declared by the owning slice | Owning slice's recipe |
| `assembler-max-segment-count-3` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-b16f-735e-9d04-417a547a42e3 | TCP out-of-order assembler capacity, selected with the loss and reorder bounds | Declared by the owning slice | Owning slice's recipe |
| `assembler-max-segment-count-4` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-b16f-735e-9d04-417a547a42e3 | TCP out-of-order assembler capacity, selected with the loss and reorder bounds | Declared by the owning slice | Owning slice's recipe |
| `assembler-max-segment-count-8` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-b16f-735e-9d04-417a547a42e3 | TCP out-of-order assembler capacity, selected with the loss and reorder bounds | Declared by the owning slice | Owning slice's recipe |
| `assembler-max-segment-count-16` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-b16f-735e-9d04-417a547a42e3 | TCP out-of-order assembler capacity, selected with the loss and reorder bounds | Declared by the owning slice | Owning slice's recipe |
| `assembler-max-segment-count-32` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-b16f-735e-9d04-417a547a42e3 | TCP out-of-order assembler capacity, selected with the loss and reorder bounds | Declared by the owning slice | Owning slice's recipe |
| `async` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | Waker registration for async executors | Declared by the owning slice | Owning slice's recipe |
| `auto-icmp-echo-reply` | feature | `Cargo.toml` `[features]` | supported | QEMU `LinkDevice` and loopback | 01a08ff0-0b99-7315-99b0-7e7f340fa6f9 | Replies only to echo requests addressed to the service's own address; no client ICMP authority | One reply per request, same frame budget | `just io_network_qualification_check` |
| `default` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Upstream default set; the service sets `default-features = false` and names every feature, and each member has its own row | — | — |
| `defmt` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | `defmt` diagnostics | Declared by the owning slice | Owning slice's recipe |
| `dns-max-name-size-64` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned `socket-dns` bound; used only if that slice adopts the facility | Declared by the owning slice | Owning slice's recipe |
| `dns-max-name-size-128` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned `socket-dns` bound; used only if that slice adopts the facility | Declared by the owning slice | Owning slice's recipe |
| `dns-max-name-size-255` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned `socket-dns` bound; used only if that slice adopts the facility | Declared by the owning slice | Owning slice's recipe |
| `dns-max-result-count-1` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned `socket-dns` bound; used only if that slice adopts the facility | Declared by the owning slice | Owning slice's recipe |
| `dns-max-result-count-2` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned `socket-dns` bound; used only if that slice adopts the facility | Declared by the owning slice | Owning slice's recipe |
| `dns-max-result-count-3` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned `socket-dns` bound; used only if that slice adopts the facility | Declared by the owning slice | Owning slice's recipe |
| `dns-max-result-count-4` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned `socket-dns` bound; used only if that slice adopts the facility | Declared by the owning slice | Owning slice's recipe |
| `dns-max-result-count-8` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned `socket-dns` bound; used only if that slice adopts the facility | Declared by the owning slice | Owning slice's recipe |
| `dns-max-result-count-16` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned `socket-dns` bound; used only if that slice adopts the facility | Declared by the owning slice | Owning slice's recipe |
| `dns-max-result-count-32` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned `socket-dns` bound; used only if that slice adopts the facility | Declared by the owning slice | Owning slice's recipe |
| `dns-max-server-count-1` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned `socket-dns` bound; used only if that slice adopts the facility | Declared by the owning slice | Owning slice's recipe |
| `dns-max-server-count-2` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned `socket-dns` bound; used only if that slice adopts the facility | Declared by the owning slice | Owning slice's recipe |
| `dns-max-server-count-3` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned `socket-dns` bound; used only if that slice adopts the facility | Declared by the owning slice | Owning slice's recipe |
| `dns-max-server-count-4` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned `socket-dns` bound; used only if that slice adopts the facility | Declared by the owning slice | Owning slice's recipe |
| `dns-max-server-count-8` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned `socket-dns` bound; used only if that slice adopts the facility | Declared by the owning slice | Owning slice's recipe |
| `dns-max-server-count-16` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned `socket-dns` bound; used only if that slice adopts the facility | Declared by the owning slice | Owning slice's recipe |
| `dns-max-server-count-32` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned `socket-dns` bound; used only if that slice adopts the facility | Declared by the owning slice | Owning slice's recipe |
| `fragmentation-buffer-size-256` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Outgoing fragmentation buffer, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `fragmentation-buffer-size-512` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Outgoing fragmentation buffer, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `fragmentation-buffer-size-1024` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Outgoing fragmentation buffer, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `fragmentation-buffer-size-1500` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Outgoing fragmentation buffer, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `fragmentation-buffer-size-2048` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Outgoing fragmentation buffer, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `fragmentation-buffer-size-4096` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Outgoing fragmentation buffer, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `fragmentation-buffer-size-8192` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Outgoing fragmentation buffer, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `fragmentation-buffer-size-16384` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Outgoing fragmentation buffer, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `fragmentation-buffer-size-32768` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Outgoing fragmentation buffer, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `fragmentation-buffer-size-65536` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Outgoing fragmentation buffer, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `iface-max-addr-count-1` | feature | `Cargo.toml` `[features]` | supported | QEMU `LinkDevice` and loopback | 01a08ff0-0b99-7315-99b0-7e7f340fa6f9 | Selected resource value | 1 interface address | `just io_network_qualification_check` |
| `iface-max-addr-count-2` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-addr-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-addr-count-3` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-addr-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-addr-count-4` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-addr-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-addr-count-5` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-addr-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-addr-count-6` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-addr-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-addr-count-7` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-addr-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-addr-count-8` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-addr-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-multicast-group-count-1` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-de7f-7bb7-ae5c-756b8cbfe637 | Joined groups, bounded by the multicast grant | Declared by the owning slice | Owning slice's recipe |
| `iface-max-multicast-group-count-2` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-de7f-7bb7-ae5c-756b8cbfe637 | Joined groups, bounded by the multicast grant | Declared by the owning slice | Owning slice's recipe |
| `iface-max-multicast-group-count-3` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-de7f-7bb7-ae5c-756b8cbfe637 | Joined groups, bounded by the multicast grant | Declared by the owning slice | Owning slice's recipe |
| `iface-max-multicast-group-count-4` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-de7f-7bb7-ae5c-756b8cbfe637 | Joined groups, bounded by the multicast grant | Declared by the owning slice | Owning slice's recipe |
| `iface-max-multicast-group-count-5` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-de7f-7bb7-ae5c-756b8cbfe637 | Joined groups, bounded by the multicast grant | Declared by the owning slice | Owning slice's recipe |
| `iface-max-multicast-group-count-6` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-de7f-7bb7-ae5c-756b8cbfe637 | Joined groups, bounded by the multicast grant | Declared by the owning slice | Owning slice's recipe |
| `iface-max-multicast-group-count-7` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-de7f-7bb7-ae5c-756b8cbfe637 | Joined groups, bounded by the multicast grant | Declared by the owning slice | Owning slice's recipe |
| `iface-max-multicast-group-count-8` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-de7f-7bb7-ae5c-756b8cbfe637 | Joined groups, bounded by the multicast grant | Declared by the owning slice | Owning slice's recipe |
| `iface-max-multicast-group-count-16` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-de7f-7bb7-ae5c-756b8cbfe637 | Joined groups, bounded by the multicast grant | Declared by the owning slice | Owning slice's recipe |
| `iface-max-multicast-group-count-32` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-de7f-7bb7-ae5c-756b8cbfe637 | Joined groups, bounded by the multicast grant | Declared by the owning slice | Owning slice's recipe |
| `iface-max-multicast-group-count-64` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-de7f-7bb7-ae5c-756b8cbfe637 | Joined groups, bounded by the multicast grant | Declared by the owning slice | Owning slice's recipe |
| `iface-max-multicast-group-count-128` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-de7f-7bb7-ae5c-756b8cbfe637 | Joined groups, bounded by the multicast grant | Declared by the owning slice | Owning slice's recipe |
| `iface-max-multicast-group-count-256` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-de7f-7bb7-ae5c-756b8cbfe637 | Joined groups, bounded by the multicast grant | Declared by the owning slice | Owning slice's recipe |
| `iface-max-multicast-group-count-512` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-de7f-7bb7-ae5c-756b8cbfe637 | Joined groups, bounded by the multicast grant | Declared by the owning slice | Owning slice's recipe |
| `iface-max-multicast-group-count-1024` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-de7f-7bb7-ae5c-756b8cbfe637 | Joined groups, bounded by the multicast grant | Declared by the owning slice | Owning slice's recipe |
| `iface-max-prefix-count-1` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-d614-767b-95d0-9c7261951e72 | Router-advertised prefixes held by the interface | Declared by the owning slice | Owning slice's recipe |
| `iface-max-prefix-count-2` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-d614-767b-95d0-9c7261951e72 | Router-advertised prefixes held by the interface | Declared by the owning slice | Owning slice's recipe |
| `iface-max-prefix-count-3` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-d614-767b-95d0-9c7261951e72 | Router-advertised prefixes held by the interface | Declared by the owning slice | Owning slice's recipe |
| `iface-max-prefix-count-4` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-d614-767b-95d0-9c7261951e72 | Router-advertised prefixes held by the interface | Declared by the owning slice | Owning slice's recipe |
| `iface-max-prefix-count-5` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-d614-767b-95d0-9c7261951e72 | Router-advertised prefixes held by the interface | Declared by the owning slice | Owning slice's recipe |
| `iface-max-prefix-count-6` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-d614-767b-95d0-9c7261951e72 | Router-advertised prefixes held by the interface | Declared by the owning slice | Owning slice's recipe |
| `iface-max-prefix-count-7` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-d614-767b-95d0-9c7261951e72 | Router-advertised prefixes held by the interface | Declared by the owning slice | Owning slice's recipe |
| `iface-max-prefix-count-8` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-d614-767b-95d0-9c7261951e72 | Router-advertised prefixes held by the interface | Declared by the owning slice | Owning slice's recipe |
| `iface-max-route-count-0` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-route-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-route-count-1` | feature | `Cargo.toml` `[features]` | supported | QEMU `LinkDevice` and loopback | 01a08ff0-0b99-7315-99b0-7e7f340fa6f9 | Selected resource value | 1 route (the declared gateway) | `just io_network_qualification_check` |
| `iface-max-route-count-2` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-route-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-route-count-3` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-route-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-route-count-4` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-route-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-route-count-5` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-route-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-route-count-6` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-route-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-route-count-7` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-route-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-route-count-8` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-route-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-route-count-16` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-route-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-route-count-32` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-route-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-route-count-64` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-route-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-route-count-128` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-route-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-route-count-256` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-route-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-route-count-512` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-route-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-route-count-1024` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-max-route-count-1` is selected and changing it is a matrix change | — | — |
| `iface-max-sixlowpan-address-context-count-1` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | 6LoWPAN context table; belongs to the IEEE 802.15.4 medium classification | Declared by the owning slice | Owning slice's recipe |
| `iface-max-sixlowpan-address-context-count-2` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | 6LoWPAN context table; belongs to the IEEE 802.15.4 medium classification | Declared by the owning slice | Owning slice's recipe |
| `iface-max-sixlowpan-address-context-count-3` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | 6LoWPAN context table; belongs to the IEEE 802.15.4 medium classification | Declared by the owning slice | Owning slice's recipe |
| `iface-max-sixlowpan-address-context-count-4` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | 6LoWPAN context table; belongs to the IEEE 802.15.4 medium classification | Declared by the owning slice | Owning slice's recipe |
| `iface-max-sixlowpan-address-context-count-5` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | 6LoWPAN context table; belongs to the IEEE 802.15.4 medium classification | Declared by the owning slice | Owning slice's recipe |
| `iface-max-sixlowpan-address-context-count-6` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | 6LoWPAN context table; belongs to the IEEE 802.15.4 medium classification | Declared by the owning slice | Owning slice's recipe |
| `iface-max-sixlowpan-address-context-count-7` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | 6LoWPAN context table; belongs to the IEEE 802.15.4 medium classification | Declared by the owning slice | Owning slice's recipe |
| `iface-max-sixlowpan-address-context-count-8` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | 6LoWPAN context table; belongs to the IEEE 802.15.4 medium classification | Declared by the owning slice | Owning slice's recipe |
| `iface-max-sixlowpan-address-context-count-16` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | 6LoWPAN context table; belongs to the IEEE 802.15.4 medium classification | Declared by the owning slice | Owning slice's recipe |
| `iface-max-sixlowpan-address-context-count-32` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | 6LoWPAN context table; belongs to the IEEE 802.15.4 medium classification | Declared by the owning slice | Owning slice's recipe |
| `iface-max-sixlowpan-address-context-count-64` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | 6LoWPAN context table; belongs to the IEEE 802.15.4 medium classification | Declared by the owning slice | Owning slice's recipe |
| `iface-max-sixlowpan-address-context-count-128` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | 6LoWPAN context table; belongs to the IEEE 802.15.4 medium classification | Declared by the owning slice | Owning slice's recipe |
| `iface-max-sixlowpan-address-context-count-256` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | 6LoWPAN context table; belongs to the IEEE 802.15.4 medium classification | Declared by the owning slice | Owning slice's recipe |
| `iface-max-sixlowpan-address-context-count-512` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | 6LoWPAN context table; belongs to the IEEE 802.15.4 medium classification | Declared by the owning slice | Owning slice's recipe |
| `iface-max-sixlowpan-address-context-count-1024` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | 6LoWPAN context table; belongs to the IEEE 802.15.4 medium classification | Declared by the owning slice | Owning slice's recipe |
| `iface-neighbor-cache-count-1` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-neighbor-cache-count-4` is selected and changing it is a matrix change | — | — |
| `iface-neighbor-cache-count-2` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-neighbor-cache-count-4` is selected and changing it is a matrix change | — | — |
| `iface-neighbor-cache-count-3` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-neighbor-cache-count-4` is selected and changing it is a matrix change | — | — |
| `iface-neighbor-cache-count-4` | feature | `Cargo.toml` `[features]` | supported | QEMU `LinkDevice` and loopback | 01a08ff0-0b99-7315-99b0-7e7f340fa6f9 | Selected resource value; cache entries are service state, never client-visible | 4 neighbor entries | `just io_network_qualification_check` |
| `iface-neighbor-cache-count-5` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-neighbor-cache-count-4` is selected and changing it is a matrix change | — | — |
| `iface-neighbor-cache-count-6` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-neighbor-cache-count-4` is selected and changing it is a matrix change | — | — |
| `iface-neighbor-cache-count-7` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-neighbor-cache-count-4` is selected and changing it is a matrix change | — | — |
| `iface-neighbor-cache-count-8` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-neighbor-cache-count-4` is selected and changing it is a matrix change | — | — |
| `iface-neighbor-cache-count-16` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-neighbor-cache-count-4` is selected and changing it is a matrix change | — | — |
| `iface-neighbor-cache-count-32` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-neighbor-cache-count-4` is selected and changing it is a matrix change | — | — |
| `iface-neighbor-cache-count-64` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-neighbor-cache-count-4` is selected and changing it is a matrix change | — | — |
| `iface-neighbor-cache-count-128` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-neighbor-cache-count-4` is selected and changing it is a matrix change | — | — |
| `iface-neighbor-cache-count-256` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-neighbor-cache-count-4` is selected and changing it is a matrix change | — | — |
| `iface-neighbor-cache-count-512` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-neighbor-cache-count-4` is selected and changing it is a matrix change | — | — |
| `iface-neighbor-cache-count-1024` | feature | `Cargo.toml` `[features]` | not-applicable | — | — | Alternative value; `iface-neighbor-cache-count-4` is selected and changing it is a matrix change | — | — |
| `ipv6-hbh-max-options-1` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | IPv6 hop-by-hop option bound; follows `proto-ipv6-hbh` | Declared by the owning slice | Owning slice's recipe |
| `ipv6-hbh-max-options-2` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | IPv6 hop-by-hop option bound; follows `proto-ipv6-hbh` | Declared by the owning slice | Owning slice's recipe |
| `ipv6-hbh-max-options-3` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | IPv6 hop-by-hop option bound; follows `proto-ipv6-hbh` | Declared by the owning slice | Owning slice's recipe |
| `ipv6-hbh-max-options-4` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | IPv6 hop-by-hop option bound; follows `proto-ipv6-hbh` | Declared by the owning slice | Owning slice's recipe |
| `ipv6-hbh-max-options-8` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | IPv6 hop-by-hop option bound; follows `proto-ipv6-hbh` | Declared by the owning slice | Owning slice's recipe |
| `ipv6-hbh-max-options-16` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | IPv6 hop-by-hop option bound; follows `proto-ipv6-hbh` | Declared by the owning slice | Owning slice's recipe |
| `ipv6-hbh-max-options-32` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | IPv6 hop-by-hop option bound; follows `proto-ipv6-hbh` | Declared by the owning slice | Owning slice's recipe |
| `libc` | feature | `Cargo.toml` optional dependency | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | Host C library, required only by the host PHY backends | Declared by the owning slice | Owning slice's recipe |
| `log` | feature | `Cargo.toml` optional dependency | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | `log` facade diagnostics | Declared by the owning slice | Owning slice's recipe |
| `medium-ethernet` | feature | `Cargo.toml` `[features]` | supported | QEMU `LinkDevice` and loopback | 01a08ff0-0b99-7315-99b0-7e7f340fa6f9 | Frames cross only the service's `LinkDevice` capability; no client sees frames | 4 RX and 4 TX frames of at most 1514 bytes | `just io_network_qualification_check` |
| `medium-ieee802154` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | Pinned medium outside the admitted Ethernet backend | Declared by the owning slice | Owning slice's recipe |
| `medium-ip` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | Pinned medium outside the admitted Ethernet backend | Declared by the owning slice | Owning slice's recipe |
| `multicast` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-de7f-7bb7-ae5c-756b8cbfe637 | Explicit group, interface, port and direction grants with IGMP behind them | Declared by the owning slice | Owning slice's recipe |
| `packetmeta-id` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | Per-packet metadata identifiers | Declared by the owning slice | Owning slice's recipe |
| `phy-raw_socket` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | Host raw-socket PHY backend | Declared by the owning slice | Owning slice's recipe |
| `phy-tuntap_interface` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | Host TUN/TAP PHY backend | Declared by the owning slice | Owning slice's recipe |
| `proto-dhcpv4` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-d236-753f-ad60-f56c378eb2fe | Leases configure an authorized interface; never mint destination rights | Declared by the owning slice | Owning slice's recipe |
| `proto-dns` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned DNS wire support; replaces or supplements the service resolver by that slice's decision | Declared by the owning slice | Owning slice's recipe |
| `proto-ipsec` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | IPsec AH and ESP wire support, outside the admitted set | Declared by the owning slice | Owning slice's recipe |
| `proto-ipsec-ah` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | IPsec AH wire support, outside the admitted set | Declared by the owning slice | Owning slice's recipe |
| `proto-ipsec-esp` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | IPsec ESP wire support, outside the admitted set | Declared by the owning slice | Owning slice's recipe |
| `proto-ipv4` | feature | `Cargo.toml` `[features]` | supported | QEMU `LinkDevice` and loopback | 01a08ff0-0b99-7315-99b0-7e7f340fa6f9 | One static interface address from generation data | 1 address, 1 route | `just io_network_qualification_check` |
| `proto-ipv4-fragmentation` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | IPv4 fragmentation and reassembly: bounded support or executable exclusion | Declared by the owning slice | Owning slice's recipe |
| `proto-ipv6` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-ca2d-78c4-b9e1-689e6db39e9f | Static IPv6 interface, NDP and ICMPv6 | Declared by the owning slice | Owning slice's recipe |
| `proto-ipv6-fragmentation` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | IPv6 fragmentation, outside the admitted set | Declared by the owning slice | Owning slice's recipe |
| `proto-ipv6-hbh` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | IPv6 hop-by-hop options, outside the admitted set | Declared by the owning slice | Owning slice's recipe |
| `proto-ipv6-routing` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | IPv6 routing header, outside the admitted set | Declared by the owning slice | Owning slice's recipe |
| `proto-ipv6-slaac` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-d614-767b-95d0-9c7261951e72 | SLAAC configures an authorized interface; never mints destination rights | Declared by the owning slice | Owning slice's recipe |
| `proto-rpl` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | RPL routing for 6LoWPAN meshes | Declared by the owning slice | Owning slice's recipe |
| `proto-sixlowpan` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | 6LoWPAN, following the IEEE 802.15.4 medium | Declared by the owning slice | Owning slice's recipe |
| `proto-sixlowpan-fragmentation` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | 6LoWPAN fragmentation, following the IEEE 802.15.4 medium | Declared by the owning slice | Owning slice's recipe |
| `reassembly-buffer-count-1` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Concurrent reassemblies, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `reassembly-buffer-count-2` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Concurrent reassemblies, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `reassembly-buffer-count-3` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Concurrent reassemblies, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `reassembly-buffer-count-4` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Concurrent reassemblies, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `reassembly-buffer-count-8` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Concurrent reassemblies, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `reassembly-buffer-count-16` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Concurrent reassemblies, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `reassembly-buffer-count-32` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Concurrent reassemblies, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `reassembly-buffer-size-256` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Reassembly bytes, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `reassembly-buffer-size-512` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Reassembly bytes, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `reassembly-buffer-size-1024` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Reassembly bytes, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `reassembly-buffer-size-1500` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Reassembly bytes, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `reassembly-buffer-size-2048` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Reassembly bytes, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `reassembly-buffer-size-4096` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Reassembly bytes, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `reassembly-buffer-size-8192` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Reassembly bytes, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `reassembly-buffer-size-16384` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Reassembly bytes, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `reassembly-buffer-size-32768` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Reassembly bytes, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `reassembly-buffer-size-65536` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c630-71d3-8c0c-85ab4d366bd4 | Reassembly bytes, decided with fragmentation support or exclusion | Declared by the owning slice | Owning slice's recipe |
| `rpl-parents-buffer-count-2` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | RPL parent set; follows `proto-rpl` | Declared by the owning slice | Owning slice's recipe |
| `rpl-parents-buffer-count-4` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | RPL parent set; follows `proto-rpl` | Declared by the owning slice | Owning slice's recipe |
| `rpl-parents-buffer-count-8` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | RPL parent set; follows `proto-rpl` | Declared by the owning slice | Owning slice's recipe |
| `rpl-parents-buffer-count-16` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | RPL parent set; follows `proto-rpl` | Declared by the owning slice | Owning slice's recipe |
| `rpl-parents-buffer-count-32` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | RPL parent set; follows `proto-rpl` | Declared by the owning slice | Owning slice's recipe |
| `rpl-relations-buffer-count-1` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | RPL relation table; follows `proto-rpl` | Declared by the owning slice | Owning slice's recipe |
| `rpl-relations-buffer-count-2` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | RPL relation table; follows `proto-rpl` | Declared by the owning slice | Owning slice's recipe |
| `rpl-relations-buffer-count-4` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | RPL relation table; follows `proto-rpl` | Declared by the owning slice | Owning slice's recipe |
| `rpl-relations-buffer-count-8` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | RPL relation table; follows `proto-rpl` | Declared by the owning slice | Owning slice's recipe |
| `rpl-relations-buffer-count-16` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | RPL relation table; follows `proto-rpl` | Declared by the owning slice | Owning slice's recipe |
| `rpl-relations-buffer-count-32` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | RPL relation table; follows `proto-rpl` | Declared by the owning slice | Owning slice's recipe |
| `rpl-relations-buffer-count-64` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | RPL relation table; follows `proto-rpl` | Declared by the owning slice | Owning slice's recipe |
| `rpl-relations-buffer-count-128` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | RPL relation table; follows `proto-rpl` | Declared by the owning slice | Owning slice's recipe |
| `socket` | feature | `Cargo.toml` `[features]` | supported | QEMU `LinkDevice` and loopback | 01a08ff0-0b99-7315-99b0-7e7f340fa6f9 | Socket set infrastructure implied by every socket and medium feature | Service socket set of 4 TCP and 1 resolver UDP socket | `just io_network_qualification_check` |
| `socket-dhcpv4` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-d236-753f-ad60-f56c378eb2fe | Service-held DHCPv4 client socket | Declared by the owning slice | Owning slice's recipe |
| `socket-dns` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-bd70-75e4-90d5-0645363f6fbd | Pinned DNS socket; replaces or supplements the service resolver by that slice's decision | Declared by the owning slice | Owning slice's recipe |
| `socket-icmp` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-c1db-736d-a7a3-7ab4dd179e93 | Service-held echo probes and ICMP error mapping; no client raw ICMP | Declared by the owning slice | Owning slice's recipe |
| `socket-mdns` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-de7f-7bb7-ae5c-756b8cbfe637 | mDNS classified against the multicast grant boundary | Declared by the owning slice | Owning slice's recipe |
| `socket-raw` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | Raw sockets; applications never gain raw-packet authority | Declared by the owning slice | Owning slice's recipe |
| `socket-tcp` | feature | `Cargo.toml` `[features]` | supported | QEMU `LinkDevice` and loopback | 01a08ff0-0b99-7315-99b0-7e7f340fa6f9, 01a0ddaa-825f-7309-8508-ed49ffe33a8d | TCP sockets exist only inside the service; clients hold typed handles for exact granted destinations and listeners | 4 sockets, 2048-byte RX and TX buffers each, 10 s operation timeout | `just io_network_qualification_check` |
| `socket-tcp-cubic` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-a96d-7db6-8b7f-d6efef5f8b19 | Congestion control; exactly one of CUBIC or Reno is selected | Declared by the owning slice | Owning slice's recipe |
| `socket-tcp-pause-synack` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-ad77-7754-874f-b8703be81c04 | Lets the service refuse a SYN for an unadmitted peer before SYN-ACK | Declared by the owning slice | Owning slice's recipe |
| `socket-tcp-reno` | feature | `Cargo.toml` `[features]` | planned | QEMU `LinkDevice` | 01a0e239-a96d-7db6-8b7f-d6efef5f8b19 | Congestion control; exactly one of CUBIC or Reno is selected | Declared by the owning slice | Owning slice's recipe |
| `socket-udp` | feature | `Cargo.toml` `[features]` | supported | QEMU `LinkDevice` and loopback | 01a08ff0-0b99-7315-99b0-7e7f340fa6f9 | Only the service's resolver holds a UDP socket; no client UDP authority exists yet | 1 socket, 512-byte DNS packet buffers, 1 packet of metadata each way | `just io_network_qualification_check` |
| `std` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | Host standard library; the service is `no_std` | Declared by the owning slice | Owning slice's recipe |
| `verbose` | feature | `Cargo.toml` `[features]` | planned | Unassessed | 01a0e239-e2a7-7844-abbc-833a5820dfe0 | Verbose `log` output | Declared by the owning slice | Owning slice's recipe |

## Required behavior

Under QEMU, a component granted one destination can exchange a TCP byte stream
with that exact peer. An alternate address, port, DNS name, transport, raw frame,
resolver operation, or ungranted listen attempt fails before data leaves the
service. Malformed frames and packets remain bounded; unplug, reset, and restart
settle or invalidate every request and release all buffers, mappings, timers,
and charges.

The first implementation is target-neutral above `LinkDevice` and claims no
physical NIC. Framework, Raspberry Pi 5, and Milk-V Duo link qualification remain
separate target evidence.

## Verification ownership

Extend the existing I/O/network plane checker rather than creating a second
framework. External paths must exercise real bytes through the driver boundary;
local TCP must additionally prove genuine connect/accept and delivery without
external egress. Retain authority-denial, missing/failure-evidence and stale-epoch
controls. Multicast requires a suitable peer/backend rather than assuming a
unicast test setup qualifies group behavior. Contract changes run
`just contracts_check`; permanent Rust changes run repository format and lint
gates and regenerate image closures.

New spec-driven items bind the policy's `just-target` gate. Their implementation
must supply a qualification recipe covering all declared obligations and name
it in execution inputs, not requirement text; run every acceptance under that
same qualification identity. Planning admission validates requirements, not
runtime behavior, and no generic green recipe substitutes for the stated tests.

## Owning references

- [`../architecture/io-substrate.md`](../architecture/io-substrate.md)
- `contracts/{link-device,network-destination,network-service,io-queue}/v1/`
- `components/services/{virtio-net-driver,network-service}/`
- `.tasks/items/01a08ff0-0b99-7315-99b0-7e7f340fa6f9.md`
