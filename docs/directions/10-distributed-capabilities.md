# 10. Distributed capabilities

| | |
| --- | --- |
| Status | parked |
| Route | sync |
| Depends on | [entry 14](14-cross-machine-sync.md); [Authority A1 revocation](../../roadmap/06-authority-trust.md); [Hardware H6 networking](../../roadmap/04-platform-hardware.md) |
| Enables | services addressed across machines under the same authority model |
| Now | Design note only, by register decree: it stays paper until sync exists. |

## Motivation

Cross-machine sync ([entry 14](14-cross-machine-sync.md)) moves objects
and activation. The step beyond: a channel endpoint that proxies to a
service on another machine, with grants serialized as unforgeable
capabilities over the wire (CapTP-style). The component model does not
change — a tool call is still a typed IPC message to an endpoint — but
the endpoint's implementation lives elsewhere, and the authority it
carries crosses machines with the same non-ambient, unforgeable
semantics.

## What exists today

- The local half is complete in principle: channels are typed
  (M5.2a), capabilities are unforgeable and non-ambient, and membranes
  already make a proxied endpoint indistinguishable from a local one
  (README's agentic direction; [entry 7](07-schema-interposition.md)).
- Nothing crosses machines yet: sync (14) is parked, Authority A1 revocation
  is future work, and Hardware H6 networking has not landed.

## Design sketch

The wire capability is the core problem: a local capability is an
unforgeable kernel reference, so its serialized form must be a
cryptographic bearer (or reference) whose minting, transfer, and
presentation map back onto the local grant on each side. Each machine
keeps its own kernel enforcement; the wire form only moves authority
between two kernels that both verify it.

Revocation is a prerequisite, not an option: a proxied grant must be
withdrawable when a partition heals or a session ends, which is exactly
entry 2's derivation-tree revocation — the wire capability is derived
from the local grant, so revoking the local subtree must invalidate the
remote presentations. Whether derivation trees extend across machines
(the remote side holds a sub-derivation) or stop at the wire (the
remote side holds a fresh local grant backed by the sending side's
retained capability) is the entry's central open question.

Partition semantics must be explicit: messages in flight during a
partition, replayed presentations after reconnect, and the difference
between "endpoint unreachable" and "capability revoked" all surface as
structured errors in the same channel vocabulary, so components need no
distributed-systems special cases.

## Open questions

- Do derivation trees extend across machines, or terminate at the wire
  with the sending side retaining the backing grant?
- What binds a wire capability to a session or transport identity
  (replaying a captured presentation from another context must fail)?
- Exactly-once semantics for tool calls across partitions: idempotency
  declared in the schema, or sequence-checked channels?
- Does the membrane machinery (entry 7) implement the proxy endpoint, so
  recording/dry-run semantics extend across machines unchanged?

## Exit-condition sketch

None yet; design note only.

## Probe guidance

Paper: the design note itself, covering the wire form, the revocation
mapping onto Authority A1's revocation trees, and the partition error vocabulary,
evaluated against two or three concrete cross-machine agent scenarios.
Authority A5 depends on entry 14, A1, and Hardware H6.
