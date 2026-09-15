# 12. Generation bisect

| | |
| --- | --- |
| Route | updates |
| Enables | unattended regression localization over the update history |

## Motivation

Generations form a content-addressed parent chain, so "which update
regressed this" is automatable as safe boot-and-health-check bisection.
Manual regression hunting across updates is the slowest part of
generation-based workflows; because every intermediate state is itself a
bootable, verifiable generation, the search can be delegated to the
machine with rollback as the safety net at every step.

## Design sketch

Input: a known-good and a known-bad generation identity. The driver
walks the parent chain between them, selects the midpoint, stages it as
pending, boots it under QEMU, and reads the health verdict: confirmed
healthy marks it good, rollback marks it bad. Each step is safe by
construction — a bad candidate consumes its attempts and the system
returns to known-good automatically.

The oracle question is the real design work: "healthy" as defined by
the health service covers boot success, but a regression bisect wants a
*behavioral* predicate ("does the storage probe still pass"). The
bisect driver should accept a pluggable check target — any `_check`-style
QEMU run — so health confirmation plus a scenario probe together decide
good/bad.

Termination: logarithmic in chain length; every step's staging,
attempts, and verdicts are durable events, so an interrupted bisect
resumes from BootState rather than restarting.

## Open questions

- Oracle composition: is health-confirmation alone sufficient for
  "good", or must each step also run a scenario probe — and how is that
  probe selected per bisect run?
- Does the bisect driver run on the host against QEMU (simplest) or
  on-device as a component with GenerationControl authority?
- Chain gaps: what if the true first-bad link's parent is not bootable
  for unrelated reasons — skip-and-widen strategy?
- How are bisect results recorded — a report object in the store, or
  host-side log only?
