# 14. Cross-machine generation sync

| | |
| --- | --- |
| Status | parked |
| Route | sync |
| Depends on | M5.8 release authorization and M6.7 generation transfer (both complete); general network transport additionally depends on Hardware H6 |
| Enables | [entry 29](29-schema-state-merge.md), [entry 10](10-distributed-capabilities.md) |
| Now | M6.7 delivers deterministic authorized transfer and activation through a second QEMU disk; general cross-machine transport and machine-identity binding remain open. |

## Motivation

A generation is a manifest plus content-addressed objects; moving a
system to a new machine is object transfer plus activation, including
capability grants and state policy — not dotfile reconstruction. The
delta versus conventional migration is that authority and state
semantics travel with the bytes: the receiving machine gets the same
component graph, the same grant closure, and the same five state
policies, verified by the same stage-0, rather than a best-effort copy
of files.

## What exists today

- Generations are deterministic, content-addressed, and self-describing
  (M5.5, complete); the object store deduplicates by identity (M5.4,
  complete), so transfer is set-difference over object identities.
- Rollback semantics are defined (M5.6, complete): a transferred
  generation boots as pending and must be health-confirmed, giving
  first-boot-on-new-hardware the same safety story as an update.
- M5.8 (complete) defines who may authorize a generation for staging
  or boot; an unsigned generation from another machine cannot activate.
- M6.7 (complete) delivers deterministic transfer manifests, closure
  validation, set-difference transfer, and receiver-side activation via
  a second QEMU disk; this entry tracks the general networked capability.

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
[entry 5](05-tpm-bound-boot-state.md) exists, or a rollout key
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
- Which failures consume a boot attempt — closure and authorization
  checks must all be pre-transfer-of-control.

## Exit-condition sketch

An authorized QEMU-built generation transfers to a second machine and
activates there with grants and state policy intact.

## Probe guidance

The M6.7 QEMU fixture proves closure construction, authorization, and
activation. A further probe should retain that format while adding
machine-identity binding and a Hardware H6 network transport.
