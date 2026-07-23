# 11. IPC flight recorder and deterministic replay

| | |
| --- | --- |
| Status | parked |
| Route | determinism |
| Depends on | M5.3 (complete: driver IPC recording during fault injection, named there as the intended foundation); replay of arbitrary components additionally wants [entry 3](03-nondeterminism-as-capabilities.md) |
| Enables | [entry 26](26-hermetic-testing.md); failure reports as generation hash + trace |
| Now | Recording half exists for drivers; generalizing the trace format and the replay harness for non-driver components is design work legal today, blocked in practice only where components read nondeterminism (entry 3). Named as an M5.3 follow-up in the canonical [roadmap](../../roadmap/README.md). |

## Motivation

All component input crosses channel boundaries, so recording at that
boundary yields deterministic re-execution of a single component. A bug
report becomes a generation hash plus an IPC trace: the reporter's exact
component bytes (content-addressed by the generation) and exact inputs
(the trace) are both in the report, so the failure is reproducible by
anyone, anywhere, without the reporter's machine.

For agent components this is the audit primitive: what the agent did is
what crossed its channels, and a trace is the complete, checkable record.

## What exists today

- M5.3 (complete) records driver IPC during fault injection and replays
  it inside `storage_fault_check` — the proof that record/replay works
  for one carefully constructed component.
- M5.5 (complete) makes "generation hash" a precise reference: the
  generation is deterministic and content-addressed.
- M5.4 (complete) provides the object store a trace artifact would live
  in.
- [entry 7](07-schema-interposition.md) supplies the recording machinery
  as generated membranes instead of per-protocol hand-written recorders.
- Missing: arbitrary components read clocks and entropy off-channel;
  [entry 3](03-nondeterminism-as-capabilities.md) is the amendment that
  brings those under capability control so "all input crosses channels"
  becomes true.

## Design sketch

Two halves with separate blockers. Recording: generalize the M5.3
recorder into the entry-7 membrane so any endpoint's traffic can be
captured in a bounded canonical trace format, sealed as an object. This
half is legal today and its format design should be co-designed with
entry 7.

Replay: a harness that instantiates the component from the named
generation with a virtual channel set, feeds the trace, and compares
output byte-for-byte. For components declared deterministic under
entry 3, replay needs nothing else. For others, the trace must
additionally capture every nondeterminism draw (clock reads, entropy) —
which is precisely what entry 3's seeded-pool option makes recordable.

Replay scope is deliberately per-component, not whole-system: the
component is the determinism boundary, peers are replaced by the trace.
Whole-graph replay is not a goal of this entry.

## Open questions

- Trace format: per-endpoint streams or a single causally ordered log?
  (The former composes with per-component replay; the latter with
  multi-component analysis.)
- How are large payloads represented — inline (bounded, heavy) or as
  content-addressed object references into the M5.4 store?
- For non-deterministic components, is trace-captured nondeterminism
  sufficient, or must replay refuse components lacking entry-3
  declarations?
- Where does replay run — host-side against the same component bytes, or
  under QEMU as a test target like `storage_fault_check`?

## Exit-condition sketch

A recorded trace of a non-driver component re-executes byte-identically;
a failure report consists of a generation hash plus a trace artifact.

## Probe guidance

Design work legal today: define the trace format with entry 7, then
record and replay one non-driver component chosen to avoid
nondeterminism (the way M5.3's fixture does). The probe measures how
much of the current component set is replayable without entry 3, which
sizes the amendment's real value before promotion.
