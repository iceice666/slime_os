# 18. Per-destination network authority

| | |
| --- | --- |
| Status | promoted → [IO4](../../roadmap/11-io-substrate.md#io4--network-service-and-exact-destination-authority) (the authority boundary, 2026-08-28) and [IO11](../../roadmap/11-io-substrate.md#io11--qemu-tcp-client-byte-stream-over-virtio-net) (the first data plane behind it, 2026-09-11) |
| Route | hardware |
| Depends on | [IO3](../../roadmap/11-io-substrate.md#io3--userspace-virtio-net-and-linkdevice-validation) for the QEMU link; a physical link is P6.F's (H1V1 Ethernet) and [Hardware H6](../../roadmap/04-platform-hardware.md)'s |
| Enables | manifest-auditable exfiltration surface — particularly for agent components |
| Now | Landed: `contracts/network-destination/v1` declares per-holder rows with CONNECT/SEND/RECV/LISTEN rights, `network-service` enforces them, and `sel4-io-tcp` carries a TCP client byte stream to one declared destination behind that boundary. |

## Motivation

Network access is a capability to explicit endpoints declared by the
generation, making the exfiltration surface auditable in the manifest.
This is the agent-safety network story: an agent component's reachable
destinations are enumerated in the same document as every other grant,
so "where can this agent send data" is a static question with a
checkable answer — not a property of runtime socket calls.

## What exists today

- The pattern is proven by storage: M5.1/M5.2 gating a BlockDevice
  behind declared rights, verified by `storage_cap_check`, is exactly
  the shape a NetworkDestination row would follow.
- The capability-matrix horizon names the object shape question:
  NetworkDestination with CONNECT / SEND / RECV / LISTEN rights, and
  whether the object is (protocol, address, port) declared in the
  generation.
- [entry 9](09-grant-graph-introspection.md) makes the audit concrete:
  "which components can reach which destinations" becomes a grant-graph
  query over the manifest the day the row lands.
- Hardware H6 owns networking; no stack exists to gate yet.

## Design sketch

A NetworkDestination object identifies a declared remote — the
horizon's candidate shape is (protocol, address, port) — and rights
split CONNECT / SEND / RECV / LISTEN so, e.g., a component may receive
from a destination without initiating toward it. The generation
manifest lists every reachable destination per component; wildcards are
the design pressure point (an update-check destination is a stable
endpoint, but model-provider endpoints change), and each wildcard form
weakens the audit story proportionally.

Enforcement mirrors the block device: the network service is a
userspace component, and the kernel gates creation of channel endpoints
to it — a component without the destination capability cannot address
traffic there. DNS is a destination too (resolution is itself an
information flow), so the amendment must decide whether name resolution
is a separate object kind.

Audit composition: entries [9](09-grant-graph-introspection.md) and
[1](01-authority-diff-gate.md) extend for free — destination changes
across generations appear in the authority diff, and exfiltration
reachability is a query.

## Open questions

- Wildcards: are patterns (domain suffixes, port ranges) expressible,
  and how are they audited — expanded at build time or carried as
  pattern grants?
- Is DNS resolution a distinct object kind with its own rights?
- Inbound authority: does LISTEN on a declared local endpoint follow
  the same object shape, and who declares exposure?
- Per-destination budgets (rate, byte counts) — inside this row, or
  deferred to [entry 25](25-resource-accounts.md)-style accounts?

## Exit-condition sketch

A component holding a capability for one declared destination cannot
connect to any other address or port; the manifest lists every
reachable destination.

## Probe guidance

Paper: the matrix amendment (object shape, rights strings, wildcard
policy, DNS treatment) evaluated against the agent scenarios in
README's agentic direction — does every realistic agent deployment keep
a fully enumerable destination list? The answer sizes the wildcard
escape hatch before Hardware H6 makes it concrete.
