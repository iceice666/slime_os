# Network application authority, version 3

`v3/schema.zt` defines the generation resource for explicit application
bindings, exact TCP listener authority and each binding's declared TCP socket
options. Generate its Python layout and Rust constants with
`python3 scripts/generate/generate-boot-bindings.py`.
`boot-contracts/src/network_application.rs` owns semantic decoding. `v1/` and
`v2/` remain as the retained sources of format versions 1 and 2: version 1
named a listener's admitted peer only as a local holder, and version 2 declared
no socket options. The decoder refuses both.

## Encoding

The 32-byte header contains magic `SLIMENA\0`, format version, header size,
zero required flags, application count, and exact total length. The header
carries format version 3. At most four
320-byte entries follow, sorted strictly by their 32-byte holder identities.
Zero and duplicate holders, trailing bytes, unknown discriminants/flags/rights,
and nonzero reserved bytes are refused. Holder identities use the existing
`network_destination::holder_identity` domain, not a second naming algorithm.

Each entry contains:

- Holder identity; control, provisioning, and optional supervision binding names;
  optional request/completion notification binding names.
- Exact allowed peer holder identity, local IPv4 address/port, role, backend,
  rights, and the admitted-peer kind: none (`0`), holder (`1`) or IPv4 (`2`).
- The admitted remote IPv4 address of an external listener.
- Backlog, accepted-socket limit, byte/timer budgets, queue depth, retry limit,
  and reconnect limit.
- The TCP options: keep-alive interval, idle timeout, hop limit and Nagle;
  explicit reserved padding.

Binding fields are 32-byte, zero-padded names containing lowercase ASCII letters,
digits, hyphens, or underscores. Control and provisioning names must be nonempty
and distinct. Optional supervision may be empty. Notification names must either
both be empty or both be present and distinct. These are grant names, without
resolver prefixes. Their existence, kinds, holders, and rights must be checked
against the admitted generation; decoding a name alone does not mint authority.
A supervision binding name does not prove its runtime task subject: that subject
must be established by the trusted capability issuer and checked at activation.
Service activation can require optional lifecycle
bindings that the wire decoder permits to be absent for a polling-only profile.

## TCP socket options

Every row, client or listener, declares the options the service applies to
each socket it opens or accepts for that holder. Version 3 carves them from
version 2's reserved tail, so an entry stays 320 bytes.

| Field | Wire | Bound | Meaning |
| --- | --- | --- | --- |
| `keepaliveMs` | `keepalive_ms : u32` | 0 (off) or 100–60000 | Interval of zero-payload keep-alive probes on an idle connection |
| `idleTimeoutMs` | `idle_timeout_ms : u32` | 1000–60000 | Abort after this long without a peer packet while data is unacknowledged or keep-alive is on |
| `hopLimit` | `hop_limit : u8` | 1–255 | IPv4 time-to-live of every packet |
| `nagle` | `nagle : u8` | 0 or 1 | Coalesce small writes while a small segment is unacknowledged |

The builder and the decoder refuse any other value. A request never names an
option: the service holds no operation that selects or changes one. Delayed
ACK, the connect and close deadlines and the congestion controller are fixed
service policy, not declarations.

## Service incarnation parameter

The generated `INCARNATION_PARAMETER_KEY = 1` names the network service's own
root-held lifecycle parameter for its incarnation counter. An authorized service
increments it before attaching applications; this key does not add a field to the
resource layout or grant parameter-write authority by itself. The counter can
survive a service restart within one boot while root retains that lifecycle
state. It is not disk persistence, a cross-boot identity, or hardware evidence.

## Roles and authority

A **client** entry selects external or loopback backend and names its bindings.
Its listener endpoint, allowed peer, rights, and every listener resource limit
must be zero. Remote connect/send/receive authority remains exclusively in
`network-destination/v1`; this resource never reinterprets a destination tuple
as listener permission.

A **loopback listener** entry has peer kind holder and the exact address
`127.0.0.1` with a nonzero port. Its peer must be a distinct holder present as a
loopback client in this same table, and its admitted address is zero.

An **external listener** entry has peer kind IPv4, a zero peer holder, a
nonzero port and a local address that must be one the admitted generation
declares for the network service's interface; the builder and the service check
that, since this table cannot see the interface. Its admitted address is one
exact unicast host other than the local address: not unspecified, loopback,
multicast, reserved or limited broadcast. It admits any source port from that
address; there is no prefix, range or wildcard.

For both, two holders cannot declare the same local listener endpoint.
`LISTEN = 8` is mandatory; independent `SEND = 2` and `RECV = 4` may
additionally be granted. No wildcard address, unknown right, or implicit peer is
admitted, and listen authority never comes from a destination row. In system
specs and derived manifests `admittedPeer` names the holder and
`admittedPeerAddress` the IPv4 address; exactly one is nonempty for a listener
and both are empty for a client.

The contract, encoder, decoder and service admit **only backlog one**; all
other values fail closed. That slot
includes a handshake or a connected socket awaiting accept. Accepted-socket
limit is independently 1–4, and a successful accept may rearm the pending slot
while earlier accepted children remain live. Byte budget is at most 16384 and
at least 4096 times the accepted limit. Timer budget is 1–4; queue depth is a
power of two from 2 through 4. The pending slot and accepted children share
these byte, timer and queue ceilings and the backend's four-socket pool.

Retry and reconnect limits are independently 0–16. Reconnect limit bounds only
automatic recovery attempts, not explicit client connects or listener rearming;
the current engine performs no automatic reconnects. These are admission
ceilings, not a promise that runtime resources are currently available.

The format is an authority declaration, not evidence of a running listener,
wait-set integration, automatic cleanup, or a qualified network device. Those
behaviors require their own runtime implementation and observed gates.
