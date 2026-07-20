# 18. Per-destination network authority

| | |
| --- | --- |
| Status | parked |
| Route | hardware |
| Depends on | M7 networking (not implemented); the capability-matrix horizon tracks the NetworkDestination object shape |
| Enables | manifest-auditable exfiltration surface — particularly for agent components |
| Now | Paper: the NetworkDestination object shape and rights strings are a matrix amendment exercise legal today. |

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
- M7 (not implemented) owns networking; no stack exists to gate yet.

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
escape hatch before M7 makes it concrete.
