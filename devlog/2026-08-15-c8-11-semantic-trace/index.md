# C8.11 — a deterministic trace, and the five ways a silent record hides

| Field | Value |
|---|---|
| Date | 2026-08-15 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/fabric-trace/v1/`, `contracts/generation/v1/schema.zt`, `contracts/data-fabric-profile/v1/schema.zt`, `components/proto/src/{fabric_trace.rs,trace_sink.rs,lib.rs}`, `components/bins/src/{fabric_trace_log.rs,call_broker.rs,operation_broker.rs}`, `components/bins/src/bin/fabric-service.rs`, `scripts/build/build-generation.py`, `scripts/check/{check-sel4-trace-plane.py,check-data-fabric-profile.py,check-sel4-gate-controls.py,check-contracts.py}` |
| Roadmap | C8.11, B55 |
| Gates | `just data_fabric_trace_check`, `just sel4_trace_check`, `just data_fabric_profile_check`, `just sel4_gate_control_check` |
| Trigger | C8.11 was the next uncompleted milestone with a fully specified exit condition |
| Baseline | C8.1–C8.10 complete; no bounded semantic-trace contract existed, and each of the three timed fabric workers drove its own disjoint simulated clock |

## Summary

C8.11 asked for a bounded, versioned, deterministic semantic-trace stream over
the fabric's timed workers, with a declared total tie order across data,
acknowledgement, peer death, and time, and a sink whose capacity and overflow
behaviour the generation fixes. That is now `contracts/fabric-trace/v1/`: one
kind-discriminated 64-byte record covering all ten required families, generating
both the Rust bindings and the Python constants the host gate reads, with a
bounded per-worker sink and `just data_fabric_trace_check` observing 100 records
across three planes, byte-identical across two boots of each.

The instructive part was not the contract. Every emission site discards the
`Result` — a trace defect must not kill a worker mid-traffic — so a record
refused by its own validator produces *no output and no error*. Six real defects
hid in exactly that gap, five of them found only after the gate grew from one
worker to three, and each was invisible in a transcript that otherwise looked
correct. The recurring lesson is that a self-validating evidence stream needs a
reported reject count and a per-worker required-family set, or absence reads as
"nothing happened".

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/fabric-trace/v1/` | New Zutai family: one 64-byte record, ten families, four order classes, a resource-counter vocabulary, and the sink's depth ceiling plus terminal reservation. Renders Rust and Python from one schema | Every cross-boundary format is schema-first, and both readers share one vocabulary |
| `contracts/generation/v1/schema.zt` | `FabricGraph` gains `traceDepth` and `traceOverflow`; seven fixtures declare them | The sink's capacity and overflow are generation facts, not component constants |
| `scripts/build/build-generation.py` | `validate_fabric_trace_sink` rejects a depth above the ceiling, at or below the reservation, non-integer, or an unknown discipline; the resolved profile carries both | An over-declared sink fails the build, not the boot |
| `components/proto/src/trace_sink.rs` | Bounded append sink: stable insertion sort by `(now_ns, order_class, sequence)`, a terminal reservation, saturate-only overflow with a counted saturation record | The artifact's order is the declared order, not the scheduler's |
| `components/proto/src/lib.rs` | `valid_trace_record` per-family field discipline; `trace_records_ordered` | A family's unused fields are zero, so two runs are byte-comparable |
| `components/bins/src/fabric_trace_log.rs` | Per-worker accumulation, one clock, and one-`debug_write` line rendering | A trace line cannot be spliced by another task's serial output |
| The three route workers | Route, QoS, call/operation, denial, fault, and resource emission; `retire_server` owns the peer-death transition; real peak-occupancy counters | Evidence records outcomes, once, on whichever path observes them |
| `scripts/check/check-sel4-trace-plane.py` | Boots all three timed planes; per-worker structure, order, bounds, capacity, and determinism | One worker's coverage cannot stand in for three |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A record its own validator refuses, emitted silently | `just sel4_trace_check` | `rejected=[1-9]` is a failure marker; proven by deleting a `peer_death` call |
| The generation's declared depth never reaching the sink | `just sel4_trace_check` | `the sink reports capacity N, but its generation declares traceDepth M`; proven by mutating the fixture |
| A worker losing its peer-death evidence | `just sel4_trace_check` | `fault` is required per worker, not merely admitted |
| A declared depth above the contract ceiling | `just contracts_check`, and `const _: ()` per worker | `traceDepth exceeds the contract ceiling 64`; a hand-edited profile fails with `E0080` |
| Trace order depending on scheduling | `just sel4_trace_check` | Two boots per plane compared record-for-record |
| The gate's own assertions going toothless | `just sel4_gate_control_check` | 28 gates against 1108 mutated transcripts |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just data_fabric_trace_check` | PASS — 100 records across 3 timed workers, byte-identical across two boots of each plane | Direct |
| `just data_fabric_profile_check` | PASS — including four new negative cases for the declared sink | Direct |
| `just sel4_gate_control_check` | PASS — 28 gates, 1108 mutated transcripts | Direct |
| `just sel4_qos_check`, `sel4_call_check`, `sel4_operation_check`, `sel4_stream_check`, `sel4_visibility_check` | PASS | Direct |
| `just contracts_check` | PASS | Direct |
| `just test_host` (28 new trace/sink assertions), `just test_sel4_root` (112) | PASS | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | PASS | Direct |
| Fixture depth mutated to 20 with a stale image | Gate reports the capacity disagreement | Direct |
| `FABRIC_TRACE_DEPTH` hand-edited to 65 | `E0080` at compile time in two workers | Direct |
| `peer_death` call deleted from the operation broker | Gate reports the missing required family | Direct |
| `validate_fabric_trace_sink` neutralized | Profile gate reports `trace depth above contract ceiling was accepted` | Direct |
| `just sel4_boot_check` | FAIL, and identically on unmodified `master` at `84c75f5` — recorded as backlog B55 | Direct |

## Decisions

- **Decision:** One kind-discriminated record, not ten layouts.
  **Rationale:** The sink is an array of equal-width slots, so ten layouts would
  size every slot to the widest anyway; and the tie order compares `now_ns` and
  `order_class` across families, which needs those fields at one fixed offset.
  **Rejected alternative:** A record type per family, as `fabric-stream/v1` does
  for its three — that contract's records travel separately, whereas these share
  one sink and one ordering.

- **Decision:** The sink *sorts*; it does not refuse out-of-class arrivals.
  **Rationale:** A tie order says how records bearing one instant are arranged.
  It says nothing about the order a broker may observe them in, and a broker
  genuinely sees an acknowledgement before a data record at one instant because
  its sweep drains client endpoints before server replies. The sort is also
  exactly what makes the artifact scheduling-independent.
  **Rejected alternative:** Refusing any record that would land before the last
  one. Measured: 10 records per call-plane run discarded as "emitter defects".

- **Decision:** Emission records outcomes, at the admission or authorization
  decision, never at dispatch entry.
  **Rationale:** Tracing on entry wrote a matched record naming a
  client-supplied correlation before the refusals ran. The call plane fires 24
  requests bearing a foreign session, all refused; the operation plane has one
  client ask for another's result. Both were recorded as accepted traffic, and
  the second republished an identity the broker had just refused. A denial now
  names nothing — no edge, no correlation — enforced by the validator rather
  than by each call site.
  **Rejected alternative:** Tracing attempts and letting a reader infer the
  outcome from a later record. There is no later record for a refusal.

- **Decision:** One `debug_write` per line.
  **Rationale:** Each `debug_write` is a root round trip, so field-by-field
  emission is interleavable — observed directly, a `SLIME_GRAPH supervision
  collected` line spliced into the middle of an ack record, which the gate
  counted as one record fewer than the sink reported. C8.11 requires the trace
  to be comparable independent of serial-log interleaving, and a line that can
  be cut in half is not.
  **Rejected alternative:** Reassembling spliced lines in the gate, which would
  have made the checker tolerate the defect rather than the format prevent it.

- **Decision:** Reuse the three existing timed planes rather than add a fixture.
  **Rationale:** The tie order is only testable where all four classes occur,
  which needs a generation that grants a clock. Exactly three do, one per timed
  worker. A dedicated fixture would assert the same property about the same
  workers while adding a fourth generation to maintain.
  **Rejected alternative:** A single plane. Measured cost: reading one worker
  hid five of the six defects below.

- **Decision:** Depth bounds are `const _: ()` items in each worker.
  **Rationale:** `TraceSink::with_const_capacity` is a `const fn` reached from
  `fn main`, and a `const fn` called at runtime evaluates at runtime — so its
  own assert would be a `no_std` boot panic, which is what the design set out to
  avoid. The crate-level items are evaluated unconditionally.
  **Rejected alternative:** Trusting the builder's validation alone; it does not
  cover a hand-edited `SLIME_DATA_FABRIC_PROFILE`.

## Open risks and follow-ups

- [ ] Four families — schema, visibility, interposition, denial-on-a-held-edge —
      have validator arms and generated codes; only `denial` acquired an emitter
      here. `fabric-service`'s `deny()` and `visibility_broker`'s interposition
      path are the natural producers, and belong with C8.12's visibility and
      denial matrix rather than here. Guard: the gate's `ADMITTED_KINDS` will
      accept them without change; each needs its own required-family entry when
      it lands.
- [ ] B55: `just sel4_boot_check` fails on unmodified `master`, so C8.10's exit
      condition is unobserved on the current tree. Independent of this work and
      recorded in the backlog; C8.12 depends on C8.10 and should not open until
      it is resolved.
- [ ] The stream worker's declared depth is 16 against 12 observed records. The
      call plane needed 64. If a future scenario grows the stream plane's
      traffic, the sink will saturate and report it rather than silently drop —
      but the depth should be revisited with the traffic, not after.

## Artifacts and provenance

- Related roadmap item: [C8.11 in the core-runtime track](../../roadmap/02-core-runtime.md)
- Related backlog item: [B55](../../roadmap/00-backlog.md)
- Contract: `contracts/fabric-trace/v1/schema.zt`
- Gate: `scripts/check/check-sel4-trace-plane.py`
- Review: five concurrent lens reviews (canonical, correctness, security,
  concurrency, convention) over the uncommitted diff; every finding below P3 was
  reproduced against source before being applied, and the two negative results —
  `route_word`'s truncation carrying no authority, and `traceDepth` reaching no
  out-of-bounds index — are recorded as such rather than as fixes.
