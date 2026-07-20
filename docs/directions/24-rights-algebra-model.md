# 24. Checked model of the capability rights algebra

| | |
| --- | --- |
| Status | probing |
| Route | authority |
| Depends on | nothing; host-side contracts work consuming only the capability matrix |
| Enables | [entry 1](01-authority-diff-gate.md) (widening definition), [entry 27](27-policy-carrying-generations.md) (invariant semantics), [entry 2](02-revocable-leases.md) (does revocation preserve the algebra) |
| Now | Active probe, holding the register's single probing slot. |

## Motivation

M5.6a/M5.6b established the checked-contract methodology for BootState,
state, and GC semantics: a model in `../../contracts/bootstate/model/`,
exhaustively checked by `just bootstate_model_check`, with mutations that
must fail. The authority invariants have no equivalent — they are
enforced only by Rust code reviewed against the matrix grammar in
`../capability-matrix.md`. A drift between the grammar and the
implementation would be invisible until a rights bug ships.

The algebra is also load-bearing for the rest of the authority route:
[entry 1](01-authority-diff-gate.md) needs a precise definition of
"widening" for its CI gate, and [entry 27](27-policy-carrying-generations.md)
needs it to state machine-checkable invariants.

## What exists today

- The matrix defines object kinds, a flat `u32` rights space (bits 15–31
  free), and the grammar rules every new object or right must satisfy.
- Format v2 (M5.5, complete) fixes the 1:1 mapping between manifest
  rights strings and matrix rights bits — the model must check the
  mapping, not just the bits.
- The methodology exists and is proven: `SelectableBootRootExists`,
  `PendingAttemptConsumedBeforeTransfer`, nine concrete power-cut
  witnesses, and a rejected skip-attempt mutation are the working
  example of "invariants plus must-fail mutations".

## Design sketch

Model the rights algebra as state transitions over per-component grant
sets: initial grants from the manifest, `derive` (narrow-only), transfer
along channels, and object-kind validity (a right is meaningless on the
wrong object kind). The safety property: no operation sequence lets a
component exceed the closure of its initial grants — every reachable
grant set is a subset-closure of what the manifest declared.

Mutations that must fail, per the register: removal of narrow-only on
derive, and a transfer path that widens rights. A third candidate from
the matrix itself: an object-kind/rights mismatch accepted by the
format-v2 string mapping.

The open methodological risk is stated in the register: if the rights
grammar changes faster than a model can track, the model becomes a
second source of truth that lies. The probe must measure the grammar's
churn rate against the cost of keeping the model in lockstep.

## Open questions

- Does the model cover only kernel-level operations, or also the
  bootstrap component's grant construction from the manifest (the
  userspace half of the authority path)?
- How are free bits 15–31 handled — modeled as opaque, or must the model
  prove unused bits cannot acquire meaning by accident?
- What is the promotion milestone that keeps model and implementation in
  lockstep — the same commit discipline M5.6 requires ("contract changes
  in the same commit as implementation-semantic changes")?

## Exit-condition sketch

A checked model in `contracts/` validated by a repository target passes
the current matrix rules, and fails under a narrow-only-derive-removal
mutation and a transfer-widening mutation.

## Probe guidance

The probe output is the model plus a promotion proposal naming the
milestone that keeps model and implementation in lockstep; if the rights
grammar proves to change faster than a model can track, record that
finding and return to `parked`.
