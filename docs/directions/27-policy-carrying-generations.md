# 27. Policy-carrying generations

| | |
| --- | --- |
| Route | authority |
| Enables | turns [entry 1](01-authority-diff-gate.md)'s CI gate into a boot gate |

## Motivation

The immutable selector verifies generation integrity, signed release, and
boot-bundle identity; entries [1](01-authority-diff-gate.md) and
[9](09-grant-graph-introspection.md) analyze authority only at build time in
CI. Both leave the same gap: a hand-edited or tool-generated manifest that
never passed through CI boots fine if its hashes and authorization are valid. A
policy-carrying generation closes it by carrying a machine-checkable invariant section — for example, "no
component outside the allowlist reaches `BLOCK_WRITE`" — computed by the
builder from the manifest and re-verified before activation. The CI gate
becomes a boot gate: editing a manifest to widen grants without
recomputing the invariant section makes the generation unbootable.

## Design sketch

The invariant section is a bounded, versioned list of predicates in a
deliberately small language — reachability with rights predicates over
the manifest's grant graph, plus set constants (allowlists). The builder
computes the section from the manifest and records the result; the
verifier recomputes the predicates from the manifest bytes and requires
both that every predicate holds and that the carried section matches the
recomputed one bit-for-bit. The second check is what makes tampering
unbootable: weakening a predicate is as detectable as widening a grant.

Verification placement is the main design choice. The immutable selector is
small and already verifies the generation/release closure; adding graph
evaluation there grows the most trusted Slime code. A bootstrap or health
component has the full userspace graph available, but then a violating
generation may already have been admitted. A split is plausible: selector/root
admission checks section integrity and a bounded predicate subset, while a
bootstrap or health component evaluates richer predicates before declaring the
generation healthy, so a violating generation never becomes known-good.

The predicate language must stay small enough to evaluate in a bounded
time and memory envelope; it is a policy format, and per project
invariants the policy itself is data, not kernel code.

## Open questions

- Immutable selector/root admission vs bootstrap/health verification, or the
  split sketched above — how much evaluation belongs in immutable code?
- Does the section cover only authority predicates, or also resource
  bounds once [entry 25](25-resource-accounts.md) lands (account
  distributions as predicates)?
- Versioning: the predicate language evolves with the matrix grammar —
  how do old generations with old language versions stay verifiable?
- Interaction with entry 1: does the sign-off artifact become a
  predicate ("widenings listed here are accepted") rather than a CI-only
  file?

## Exit-condition sketch

A generation whose grants violate its carried invariant is rejected before its
component graph launches; a valid generation with a tampered invariant section
fails verification.
