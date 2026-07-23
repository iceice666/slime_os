# 12. Generation bisect

| | |
| --- | --- |
| Status | parked |
| Route | updates |
| Depends on | M5.6 (complete); the canonical [roadmap](../../roadmap/README.md) names it a follow-up enabled by that milestone |
| Enables | unattended regression localization over the update history |
| Now | Fully unblocked: the parent chain, pending/known-good mechanics, attempt consumption, and QEMU health checks all exist. This is automation over landed machinery. |

## Motivation

Generations form a content-addressed parent chain, so "which update
regressed this" is automatable as safe boot-and-health-check bisection.
Manual regression hunting across updates is the slowest part of
generation-based workflows; because every intermediate state is itself a
bootable, verifiable generation, the search can be delegated to the
machine with rollback as the safety net at every step.

## What exists today

- M5.5 (complete): generations are content-addressed with parent
  metadata — the chain the bisect walks.
- M5.6 (complete): staging a pending generation, consuming attempts
  durably, automatic return to known-good on failure, and the health
  service's confirmation path — every bisect step is an ordinary
  activation with the same safety story.
- QEMU health checks (`just rollback_check` and the other `_check`
  targets) are the pass/fail oracle a bisect step needs.
- Nothing needs inventing in the kernel; the bisect driver is a
  userspace or host-side orchestrator.

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

## Exit-condition sketch

Given a known-good and a known-bad generation identity, an automated run
boots intermediate generations under QEMU health checks and identifies
the first bad parent link unassisted.

## Probe guidance

Buildable today as host-side automation: script the
stage-pending → boot → read-verdict loop over a synthetic chain of
generations with one deliberately broken link, and verify the driver
finds it unassisted. Success promotes directly; no kernel changes are
expected.
