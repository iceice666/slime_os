# 14. Cross-machine generation sync

| | |
| --- | --- |
| Status | parked |
| Route | sync |
| Depends on | M5.8 release authorization (not started) and the M6 transfer path (M6 is a minimal stub; its scope already lists "generation sync/transfer between machines") |
| Enables | [entry 29](29-schema-state-merge.md), [entry 10](10-distributed-capabilities.md) |
| Now | Paper and host-side format work only: transfer manifests, object-set closure description, target binding design. Partially scoped by M6; still needs its own exit condition. |

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
- M5.8 (not started) defines who may authorize a generation for staging
  or boot; cross-machine transfer consumes that answer — an unsigned
  generation from another machine must not activate.
- M6 (stub only) lists the minimal transfer path in scope; this entry
  tracks the general capability beyond that minimum.

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

Host-side today: write the closure algorithm (manifest → required
object set per state policy) and demonstrate it against the current
QEMU fixture generations, producing the transfer manifest format.
Authorization and machine-identity binding stay paper until M5.8 lands.
