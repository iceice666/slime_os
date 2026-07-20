# 29. Schema-declared state merge

| | |
| --- | --- |
| Status | parked |
| Route | sync |
| Depends on | entries [14](14-cross-machine-sync.md) and [15](15-zutai-state-migrations.md); Zutai evaluation in the sync path (host-side acceptable initially) |
| Enables | multi-machine state without silent winner-picking |
| Now | The formal core — deterministic three-way merge semantics — is a self-contained host-side exercise with fixtures, legal before 14/15 land. |

## Motivation

Entry [14](14-cross-machine-sync.md) moves objects between machines and
entry [15](15-zutai-state-migrations.md) migrates state schemas, but
neither answers what happens when the same state binding evolves
independently on two machines. Attach a pure Zutai merge function to a
state schema; sync performs a deterministic three-way merge whose result
is byte-identical on both machines, and a non-mergeable conflict is a
structured error that retains both roots — never a silent winner.

## What exists today

- State bindings are content-addressed objects in epochs (M5.6b,
  complete), so "same binding, two evolutions" is well-defined: two
  descendant objects of a common ancestor identity.
- Zutai purity for transformations is established by entry 15's
  migration design; merge adds a second input of the same schema.
- Nothing in the sync path exists yet (14 is parked, M6 is a stub), so
  the merge point's integration is open.

## Design sketch

The merge function is schema-attached and pure: (base, left, right) →
merged | conflict, in Zutai, with the same determinism discipline as
migrations. The protocol runs identically on both machines: each sends
its tip identity, both compute base as the common ancestor in the object
graph, both evaluate merge(base, left, right) on the same bytes, and
both seal the identical result — byte-identical by construction, not by
coordination.

This determinism requirement is deliberately stronger than eventual
consistency: CRDTs guarantee convergence of state, not byte-identity of
representation, and the generation/store stack here identifies objects
by content hash, so two machines holding "equal but differently encoded"
state would diverge in identity. Merge functions must therefore produce
canonical output — which the schema's canonical encoding (M5.2a-style)
already defines for wire form.

Conflict is first-class: schemas without a merge function, or merges
that hit the conflict case, reject divergent sync with a structured
error and retain both roots, leaving resolution to explicit user or
generation action.

## Open questions

- Which schema types admit total merge functions (registers, sets,
  maps with commutative merges), and what is the conflict surface for
  those that do not?
- Is base computed from object-graph ancestry (requires history
  retention) or from a recorded last-sync identity in the manifest?
- Where does merge authority live — a sync service holding read/write
  on both bindings, declared in whose manifest?
- Does a merged result belong to both machines' epochs, and how does GC
  treat the two pre-merge tips (retained until when)?

## Exit-condition sketch

A fixture state diverged on two machines merges to byte-identical bytes
on both; a schema without a merge function rejects divergent sync rather
than silently picking a winner.

## Probe guidance

Host-side today, independent of 14/15: pick one fixture schema, write
its merge function in Zutai, and demonstrate byte-identical convergence
plus the conflict path on synthetic diverged fixtures. The probe's
output is the merge-function contract and a classification of which
state types in the system admit total merges — the evidence that decides
whether this entry promotes as a general mechanism or a per-schema
opt-in.
