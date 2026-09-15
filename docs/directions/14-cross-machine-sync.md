# 14. Cross-machine generation sync

| | |
| --- | --- |
| Route | sync |
| Depends on | general network transport additionally depends on Hardware H6 |
| Enables | [entry 29](29-schema-state-merge.md), [distributed capabilities](../plans/authority-and-trust.md#distributed-capabilities) |

## Motivation

A generation is a manifest plus content-addressed objects; moving a
system to a new machine is object transfer plus activation, including
capability grants and state policy — not dotfile reconstruction. The
delta versus conventional migration is that authority and state
semantics travel with the bytes: the receiving machine gets the same
component graph, the same grant closure, and the same five state
policies, verified by the same immutable selector/root admission path,
rather than a best-effort copy of files.

## Design sketch

Transfer unit: the generation manifest plus the closure of objects it
references, including state bindings per policy — `preserve` and
`snapshotBeforeUpgrade` state travels, `ephemeral` does not, and
`immutable` travels read-only. The manifest itself already declares
this, so the transfer tool computes the closure mechanically.

Target binding is the open trust question M5.8 leaves behind: a release
signature authorizes a generation for a target; sync must define whether
"this machine" is a new target requiring fresh authorization, or whether
an existing authorization covers a declared machine set. Machine
identity (what the target is bound to — TPM key once
[physical trust and attestation](../plans/authority-and-trust.md#physical-trust-and-attestation) exists, or a rollout key
before then) is the design input.

Activation on the receiver is deliberately unoriginal: stage as pending,
consume attempts, health-confirm. The interesting failures are earlier —
incomplete closure (a referenced state object did not arrive) and
authorization mismatch — and both must fail closed before any boot
attempt is consumed.

## Open questions

- What identifies a machine for target binding before TPM support
  exists?
- Partial transfer: may a receiver pull only the objects it lacks
  (set-difference against its store), and is the closure proof still
  checkable?
- Do transferred generations keep their parent chain intact, or is the
  receiver's chain rebased (with what rollback implications)?
- Which failures consume a boot attempt — closure and authorization checks
  must finish before pending selection mutates durable BootState.
