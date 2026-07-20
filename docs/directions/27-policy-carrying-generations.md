# 27. Policy-carrying generations

| | |
| --- | --- |
| Status | parked |
| Route | authority |
| Depends on | M5.5 machine-readable grants (complete); the M5.6 activation path (complete) |
| Enables | turns [entry 1](01-authority-diff-gate.md)'s CI gate into a boot gate |
| Now | Fully unblocked: both dependencies are landed. Invariant-section format, builder computation, and verification-hook placement are all designable and implementable as host-side/builder work today. |

## Motivation

Stage-0 verifies hashes and format versions; entries
[1](01-authority-diff-gate.md) and [9](09-grant-graph-introspection.md)
analyze authority only at build time in CI. Both leave the same gap: a
hand-edited or tool-generated manifest that never passed through CI boots
fine if its hashes are consistent. A policy-carrying generation closes it
by carrying a machine-checkable invariant section — for example, "no
component outside the allowlist reaches `BLOCK_WRITE`" — computed by the
builder from the manifest and re-verified before activation. The CI gate
becomes a boot gate: editing a manifest to widen grants without
recomputing the invariant section makes the generation unbootable.

## What exists today

- M5.5 (complete) provides deterministic, byte-identical generation
  output with machine-readable 1:1 rights strings — the invariant section
  can be computed deterministically and covered by the generation's own
  integrity hashes.
- M5.6 (complete) provides the activation path: immutable stage-0 selects
  and hash-verifies the complete kernel-bearing generation before
  transfer, and the health service confirms or rolls back after boot.
  Both are candidate verification points.
- [entry 9](09-grant-graph-introspection.md)'s grant-graph engine is the
  evaluator: an invariant is a predicate over the same graph.
- [entry 24](24-rights-algebra-model.md) defines the widening order the
  predicates express.

## Design sketch

The invariant section is a bounded, versioned list of predicates in a
deliberately small language — reachability with rights predicates over
the manifest's grant graph, plus set constants (allowlists). The builder
computes the section from the manifest and records the result; the
verifier recomputes the predicates from the manifest bytes and requires
both that every predicate holds and that the carried section matches the
recomputed one bit-for-bit. The second check is what makes tampering
unbootable: weakening a predicate is as detectable as widening a grant.

Verification placement is the main design choice. Stage-0 is immutable
and already hash-verifies the generation; adding graph evaluation there
grows the most trusted code. The health service runs post-boot with the
full userspace available, but then a violating generation has already
taken control once. A split is plausible: stage-0 checks only section
integrity and format (cheap), the health service or bootstrap component
evaluates predicates before declaring the generation healthy (rich), so
a violating generation never becomes known-good.

The predicate language must stay small enough to evaluate in a bounded
time and memory envelope; it is a policy format, and per project
invariants the policy itself is data, not kernel code.

## Open questions

- Stage-0 vs health-service verification, or the split sketched above —
  how much evaluation belongs in immutable code?
- Does the section cover only authority predicates, or also resource
  bounds once [entry 25](25-resource-accounts.md) lands (account
  distributions as predicates)?
- Versioning: the predicate language evolves with the matrix grammar —
  how do old generations with old language versions stay verifiable?
- Interaction with entry 1: does the sign-off artifact become a
  predicate ("widenings listed here are accepted") rather than a CI-only
  file?

## Exit-condition sketch

A generation whose grants violate its carried invariant is rejected
before control transfer; a valid generation with a tampered invariant
section fails verification.

## Probe guidance

Legal today end to end on the host: define the predicate language over
format-v2 manifests, implement builder computation plus recomputation
check, and demonstrate both failure modes against fixture manifests.
The probe's output is the format proposal and a measured evaluation
cost, which decides the stage-0/health-service split before promotion.
