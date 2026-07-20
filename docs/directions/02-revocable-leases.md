# 2. Revocable and time-bounded grants

| | |
| --- | --- |
| Status | parked |
| Route | lifecycle |
| Depends on | provenance follow-up to M5.1; touches capability-table design |
| Enables | [entry 10](10-distributed-capabilities.md) (wire revocation), leaseable agent authority |
| Now | Research-heavy; design note before any kernel change. The note — derivation-tree semantics, lease expiry model, interaction with the rights algebra — is legal paper work today. |

## Motivation

The capability matrix has narrow-only `derive` but no revocation story:
once a grant is derived and handed out, the only recourse is component
death. Two additions close the gap. Kernel-maintained derivation trees
let a proxy revoke its own subtree — the natural lifetime boundary for
interposed authority. Generation-declared grant lifetimes let the
manifest express leases, reclaimed by the health service.

Primary motivation: agent authority should be leaseable — "write access
for thirty minutes" — which the current model cannot express. An agent
given a tool for a task should lose it when the task ends, by policy
declared in the generation, not by killing the agent.

## What exists today

- The matrix's `derive` is narrow-only, so a derivation tree is
  well-founded: every derived grant is a subset of its parent, and a
  tree's root is always a manifest grant.
- The health service (M5.6, complete) already exists as the component
  with `GenerationControl` authority and fault classification — the
  natural home for lease expiry policy.
- Provenance (M5.1 follow-up) does not exist; revocation auditing
  ("who revoked what, when") wants it.
- [entry 24](24-rights-algebra-model.md) is modeling the rights algebra
  this extends; revocation must preserve its invariants.

## Design sketch

Derivation trees: every derive records parentage in the capability
table, and a holder may revoke exactly its own subtree — a proxy
revoking the grant it gave a client cannot touch its siblings or its
parent. Use-after-revoke fails with a structured error, so clients
distinguish "revoked" from "never had it". The matrix's debt register
(unreaped task-table entries) shows the table already accumulates
lifecycle state; trees formalize it rather than adding a parallel
structure.

Leases: a grant in the manifest carries an optional lifetime; expiry is
reclaimed by the health service and reported through the same structured
error. The hard question is the clock: lease expiry is wall-clock
semantics in a system whose rollback machinery is epoch-based. Does a
lease survive a generation rollback — and if expiry is epoch-anchored,
what does "thirty minutes" mean across a reboot? Options: wall-clock
expiry evaluated at use time (simple, but rollback can resurrect an
expired grant's window), or health-service-mediated expiry recorded
durably (rollback-safe, but expiry becomes a state-transition).

Both mechanisms interact with [entry 24](24-rights-algebra-model.md):
the algebra's closure property ("no sequence exceeds initial grants")
must hold in the presence of revocation — removing edges must never
create reachability.

## Open questions

- Is lease expiry wall-clock at use time, or a durable health-service
  transition (rollback semantics decide)?
- Does revocation of a subtree notify the holders, or is discovery at
  next use sufficient?
- Can a lease be renewed, and by whom — the health service on policy,
  or only by a new generation?
- How does revocation interact with in-flight IPC carrying the revoked
  capability?

## Exit-condition sketch

A proxy revokes a derived grant; further use by the original holder
fails with a structured error while sibling grants survive.

## Probe guidance

Paper: the design note covering tree semantics in the capability table,
the lease clock model (with the rollback analysis above), and the
extension to entry 24's model — does the checked algebra still hold with
edge removal? The probe succeeds if it produces a semantics the matrix
grammar can express without new ambient authority.
