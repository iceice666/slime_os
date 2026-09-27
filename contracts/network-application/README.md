# Network application authority, version 1

`v1/schema.zt` defines the generation resource for explicit application bindings
and exact local TCP listener authority. Generate its Python layout and Rust
constants with `python3 scripts/generate/generate-boot-bindings.py`.
`boot-contracts/src/network_application.rs` owns semantic decoding.

## Encoding

The 32-byte header contains magic `SLIMENA\0`, format version 1, header size,
zero required flags, application count, and exact total length. At most four
320-byte entries follow, sorted strictly by their 32-byte holder identities.
Zero and duplicate holders, trailing bytes, unknown discriminants/flags/rights,
and nonzero reserved bytes are refused. Holder identities use the existing
`network_destination::holder_identity` domain, not a second naming algorithm.

Each entry contains:

- Holder identity; control, provisioning, and optional supervision binding names;
  optional request/completion notification binding names.
- Exact allowed peer identity, local IPv4 address/port, role, backend, and rights.
- Backlog, accepted-socket limit, byte/timer budgets, queue depth, retry limit,
  and reconnect limit; explicit reserved padding.

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

A **listener** entry permits only the loopback backend and the exact address
`127.0.0.1` with a nonzero port. Its peer must be a distinct holder present as a
loopback client in this same table. Two holders cannot declare the same local
listener endpoint. `LISTEN = 8` is mandatory; independent
`SEND = 2` and `RECV = 4` may additionally be granted. No wildcard address, arbitrary
external listener, unknown right, or implicit peer is admitted.

The v1 contract, encoder, decoder and service admit **only backlog one**; all
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
