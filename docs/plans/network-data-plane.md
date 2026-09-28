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
upstream feature. Begin with a complete inventory of that release's protocols,
sockets, media, configuration and resource options. This page will own the
version-bound support matrix; implementation state and dependency edges stay
in MyQue. Each matrix row must name its upstream feature/source, applicable
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
