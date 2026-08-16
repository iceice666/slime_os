# C8.13.1 — a self-scoped occupancy query, and the counter that could not move

| Field | Value |
|---|---|
| Date | 2026-08-16 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{main.rs,shared_buffer.rs}`, `components/runtime/src/{lib.rs,syscall.rs,syscall/sel4_transport.rs}`, `components/bins/src/bin/fabric-service.rs`, `contracts/fabric-trace/v1/{schema.zt,gen_rust.zt}`, `scripts/check/check-sel4-{traffic,saturation}-plane.py`, `docs/{syscall-abi.md,capability-matrix.md}` |
| Roadmap | C8.13.1, C8.13 |
| Gates | `just sel4_traffic_check`, `just data_fabric_traffic_check`, `just sel4_saturation_check`, `just test_sel4_root` |
| Trigger | Implementing C8.13.1, whose text describes the slice as "additive only" |
| Baseline | Eight of C8.13's eleven resource classes emitted evidence; no syscall returned a live shared-buffer occupancy to any component |

## Summary

C8.13.1 adds `SHARED BUFFER OCCUPANCY` (label 30), a read-only root operation
returning the caller's own live page/buffer/mapping/loan charges, and wires the
stream broker to report that occupancy as C8.11 trace evidence. Two of the
milestone's premises did not survive measurement. Its third deliverable names
`fabric-call-worker` as a second emitter, but that worker's trace sink was
measured exactly full — 62 ordinary records plus its terminal against a
page-sized `maxTraceDepth = 64` — so emitting there would have dropped records
and failed the gate; the call worker is scoped out with the wall recorded rather
than silently narrowed. And `resourceMapping` alone would have been a constant:
instrumenting a boot showed this holder's charges range pages 8/8, buffers 7/7,
mappings 6/6, loans 0/5. The mapping count is fixed at provisioning, so
`resourceLoan` — a code declared since C8.13 with no emitter — now carries the
traffic-varying half, and the mapping record is kept because a constant is the
invariant there. Ten of eleven resource classes now emit evidence.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/src/shared_buffer.rs` | `HolderOccupancy` type and `SharedBufferTable::holder_occupancy`, one `Charge` lookup instead of four independent scans | The four counts are a single consistent observation, not four instants |
| `slime-root/src/main.rs` | `shared_buffer_labels::OCCUPANCY = 30`, its dispatch arm, and `pack_occupancy` | Self-scoped by construction: the holder comes from the authenticated badge, so the request has no holder argument to forge |
| `components/runtime/src/{syscall.rs,syscall/sel4_transport.rs,lib.rs}` | `BufferOccupancy` and `shared_buffer_occupancy()` client wrapper, unpacking the four 16-bit fields | A component learns only its own charges |
| `contracts/fabric-trace/v1/{schema.zt,gen_rust.zt}` | `resourceMapping = 13`, `resourceComplete`/`maxResourceCounter` renumbered to 14; bindings regenerated via `just fabric_trace_gen` | Zutai stays the single source of truth; the schema's `maxResourceCounter == resourceComplete` invariant holds |
| `components/bins/src/bin/fabric-service.rs` | Peak sampling for mappings and loans under `progressed`, one post-drain read deciding both records of each pair | Each peak+baseline pair is wholly present or wholly absent |
| `scripts/check/check-sel4-{traffic,saturation}-plane.py` | `mapping` asserted constant and nonzero; `loan` asserted nonzero peak with baseline bounded by peak | Neither counter can pass as a degenerate all-zero pair |
| `docs/syscall-abi.md`, `docs/capability-matrix.md` | Label 30's operands/result, and a note that it is the one shared-buffer operation gated by budget declaration rather than a rights bit | Roadmap invariant 4 |
| `Justfile`, `AGENTS.md` | Pinned root test count 112 → 114 | B23's asserted count matches the two added tests |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The occupancy query regresses to answering zeros | `just sel4_traffic_check` | `reported no 'mapping' occupancy at all` / `'loan' peak was 0` |
| A mapping is created or released outside provisioning | `just sel4_traffic_check` | `'mapping' baseline N differs from its peak M` |
| Loan accounting leaks past the run's own high-water mark | `just sel4_traffic_check` | `'loan' baseline N exceeded its own peak M` |
| Tightened ceilings regress the same evidence | `just sel4_saturation_check` | Same messages on the saturation plane |
| A standalone C8.4–C8.9 fixture overflows its fixed `traceDepth` | `just sel4_stream_check`, `sel4_qos_check`, `sel4_call_check`, `sel4_operation_check` | `dropped=N` in a sink summary |
| The query stops being self-scoped or stops denying an undeclared holder | `just test_sel4_root` | `holder_occupancy_reports_only_its_own_charges`, `undeclared_holder_reads_empty_but_is_denied_by_quota` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_traffic_check` | Passes; `loan=[5, 0]`, `mapping=[6, 6]`, `stream complete capacity=64 records=24 dropped=0 rejected=0` | Direct |
| `just sel4_saturation_check` | Passes | Direct |
| Five consecutive traffic boots, pre-fix instrumentation | `loan=[5,0]`, `mapping=[6,6]` on every boot — stable, but scheduling margin rather than an enforced invariant | Direct |
| One-off instrumented boot reading all four charges | pages 8/8, buffers 7/7, mappings 6/6, loans 0/5 — the measurement that redirected the slice from mapping-only to mapping+loan | Direct; probe not retained in the code |
| Baseline call-plane sink occupancy | `call complete capacity=64 records=63` — 62 ordinary + terminal, zero headroom | Direct; confirms the wall inherited from `2026-08-16-c8-13-resource-event-loan-walls` |
| `just sel4_stream_check`, `sel4_qos_check`, `sel4_call_check`, `sel4_operation_check` | All pass — no standalone-fixture regression | Direct |
| `just test_sel4_root` | 114/114 across 13 modules | Direct |
| `just test_host`, `contracts_check`, `generation_check`, `data_fabric_trace_check`, `fmt_check_all`, `lint_all`, `ruff`, `typos`, `devlog_check`, `sel4_gate_control_check`, `sel4_boot_layout_check` | All pass | Direct |

## Decisions

- Decision: scope `fabric-call-worker` out of this slice and record the wall.
- Rationale: its sink was measured exactly full, and both alternatives cost more
  than the counter is worth — trimming two verified ack records trades real
  evidence for new evidence, and raising `maxTraceDepth` breaks a page-aligned
  64 × 64-byte convention the C8.11 suite defends as the tampering case.
- Rejected alternative: emit anyway and let the sink saturate, which would have
  produced `dropped=N` and a red gate.

- Decision: emit `resourceLoan` from the stream broker alongside `resourceMapping`.
- Rationale: measurement showed the mapping count never moves, and a counter
  that cannot move is not evidence unless *not moving* is the invariant. The
  loan count is the traffic-varying signal this milestone's exit condition asks
  for, and `resourceLoan` was already declared with no emitter.
- Rejected alternative: ship `resourceMapping` alone and describe it as
  traffic-varying, which the measurement contradicts.

- Decision: gate the query on budget declaration rather than on a
  `SharedBufferFactory` right.
- Rationale: the answer is a projection of the budget entry itself and reveals
  only the caller's own numbers. Requiring a factory would couple a read-only
  self-query to mint authority and deny a loan receiver that holds mappings but
  was never granted a factory. `docs/capability-matrix.md` records this.
- Rejected alternative: gate on `RIGHT_BUFFER_CREATE` like `SHARED BUFFER CREATE`.

- Decision: assert `loan` baseline ≤ peak rather than = 0.
- Rationale: a ring loan settles only when the root reclaims its *receiver*, and
  the broker's exit condition proves the death of both subscribers and the clock
  peer but never inspects `fabric-publisher` liveness. The observed zero is
  scheduling margin; asserting it would make the gate fail with an occupancy
  message about a task-teardown race.
- Rejected alternative: assert 0, which passed all five measured boots and would
  have encoded a scheduling accident as a contract.

## Open risks and follow-ups

- [ ] C8.13.2 still needs the six uninstrumented holders; this slice covers one
      of the two broker holders, not the eight the fixture declares.
- [ ] `fabric-call-worker` cannot report its own mapping/loan occupancy until
      trace-sink headroom exists there. Unblocking it means freeing ordinary
      slots in the call plane or revisiting `maxTraceDepth`, both declined here.
- [ ] `resourceEvent` remains emitter-less for the reason
      `2026-08-16-c8-13-resource-event-loan-walls` root-caused: the
      `ERR_WOULDBLOCK` it depends on is unreachable through a blocking
      `seL4_Send`.
- [ ] The `loan` baseline is bounded rather than pinned. Pinning it to zero
      would require extending the broker's exit condition to require every
      publisher's supervision handle to have reported a terminal.

## Artifacts and provenance

- Focused report: none; the measurements are tabulated above.
- Raw transcript: none retained. The occupancy figures came from a temporary
  probe in `broker()` that was reverted after measuring; the pair values are
  reproducible from `just sel4_traffic_check`'s own `[trace] stream` records.
- Serial/debugger/model output: `[trace] stream` resource records in the traffic
  gate transcript carry `event=13` (mapping) and `event=12` (loan).
- Related roadmap item: [C8.13.1](../../roadmap/02-core-runtime.md#c8131----self-reported-shared-buffer-occupancy-evidence-narrow).
