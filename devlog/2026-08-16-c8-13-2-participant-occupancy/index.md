# C8.13.2 — four participants report their own occupancy; three measurably cannot

| Field | Value |
|---|---|
| Date | 2026-08-16 |
| Kind | Change |
| Status | Verified |
| Scope | `components/bins/src/fabric_occupancy_trace.rs`, `components/bins/src/bin/fabric-{publisher,publisher-b,subscriber,subscriber-b}.rs`, `scripts/check/check-sel4-{traffic,saturation}-plane.py`, `contracts/fabric-trace/v1/schema.zt`, `roadmap/02-core-runtime.md` |
| Roadmap | C8.13.2, C8.13 |
| Gates | `just sel4_traffic_check`, `just data_fabric_traffic_check`, `just sel4_saturation_check` |
| Trigger | Implementing C8.13.2, whose text names six new emitters |
| Baseline | C8.13.1 landed the occupancy query and the stream broker's mapping/loan evidence; the other seven declared holders reported nothing |

## Summary

Four of the traffic graph's uninstrumented shared-buffer holders now report their
own mapping occupancy through C8.13.1's self-scoped query, each via a shared
`fabric_occupancy_trace` module rather than four copies of the same three
records. The milestone named six new emitters; measurement found three of the
eight holders cannot produce this evidence at all, so the exit condition is
amended to five reporting holders with the other three recorded as measured
walls. `fabric-call-client` holds nothing at any point it could report — it does
consume its quota, but entirely inside `send_large_request`, which releases
before returning — `fabric-call-server` exits mid-loop and never reaches a
flush, and `fabric-call-worker`'s sink remains full from C8.13.1. A second
measurement changed what the four report: their counts are steady states, not
invariants held throughout — three of the four transiently hold more — so the
gate pins each role's provisioned count and the prose says plainly that the
transient peaks go unreported by design.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `components/bins/src/fabric_occupancy_trace.rs` | New shared participant-side emitter: one query, mapping recorded twice, terminal, flush | A participant needs no sweep loop to report; the shape lives once, not four times |
| `fabric-{publisher,subscriber,subscriber-b,publisher-b}.rs` | `#[path]` includes for the sink and the emitter, plus a traffic-gated `report()` at each one's own completion point | Each declared holder reports its own occupancy |
| `fabric_occupancy_trace.rs` | `const _: () = assert!` pair on `FABRIC_TRACE_DEPTH`, matching every other trace host | An over-declared depth fails the build rather than panicking at boot |
| `check-sel4-{traffic,saturation}-plane.py` | `TRACE_FAMILIES` replaces the hardcoded `stream\|call\|operation` in both regexes and both family-keyed dicts; `PARTICIPANT_MAPPINGS` pins each participant's exact count | New emitters are parsed rather than silently unmatched, and a role that gained or lost a region fails |
| `contracts/fabric-trace/v1/schema.zt` | `resourceMapping` prose now covers both holder kinds and names all three non-reporting holders | The schema describes which of the eight holders the format expects evidence from |
| `roadmap/02-core-runtime.md`, `roadmap/README.md` | C8.13.2 marked complete for four holders with the three walls recorded; C8.13's broker-only attribution corrected | Roadmap states measured scope, not intended scope |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A participant's occupancy query regresses to zero | `just sel4_traffic_check` | `reported no 'mapping' occupancy at all` |
| A role gains or loses a provisioned region | `just sel4_traffic_check` | `the <family> participant reported N mapping(s), expected exactly M` |
| A participant stops emitting entirely | `just sel4_traffic_check` | `the <family> worker emitted no trace records` |
| New records overflow a participant's sink | `just sel4_traffic_check` | `dropped=N` in that family's `complete` line |
| A standalone C8.4–C8.9 fixture receives traffic-only records | `just sel4_stream_check`, `sel4_qos_check`, `sel4_call_check`, `sel4_operation_check` | `dropped=N`, or a plane failing its own marker set |
| Tightened ceilings regress the same evidence | `just sel4_saturation_check` | Same messages on the saturation plane |
| An over-declared `traceDepth` reaches a participant | any build | `E0080` at compile time, proven by temporarily asserting `<= 2` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_traffic_check` | Passes; `publisher high_water=1`, `subscriber 1`, `publisher-b 2`, `subscriber-b 2`, each `complete capacity=64 records=3 dropped=0 rejected=0` | Direct |
| `just sel4_saturation_check` | Passes | Direct |
| Instrumented boot reading all four charges per holder (probes reverted) | `publisher` 0/0/1/0, `subscriber` 0/0/1/0, `subscriber-b` 0/0/2/0, `publisher-b` 3/1/3/0 mid-lend, `call-client` 0/0/0/0 at three separate points, scenario peaks 1/1/1/0 | Direct; the measurement that redefined the scope |
| Depth guard falsification: `assert!(FABRIC_TRACE_DEPTH <= 2)` | `error[E0080]: evaluation panicked` — the guard is a real build-time check, then reverted | Direct |
| `just sel4_stream_check`, `sel4_qos_check`, `sel4_call_check`, `sel4_operation_check`, `sel4_boot_check`, `sel4_matrix_check`, `sel4_visibility_check` | All pass — no standalone-fixture regression | Direct |
| `just test_sel4_root`, `test_host`, `contracts_check`, `fmt_check_all`, `lint_all`, `ruff`, `typos` | All pass | Direct |

## Decisions

- Decision: instrument four holders, not six; record the other three as walls.
- Rationale: `fabric-call-client` holds no charge at any point it could report:
  its consumption lives and dies inside `send_large_request`, which releases
  before returning, so a pair from this component could only be a measured
  `[0, 0]` — the degenerate evidence the schema rules out.
  `fabric-call-server` exits mid-loop on the
  injected peer death, so no flush is reachable without restructuring a path
  `sel4_call_check` and `sel4_boot_check` depend on.
- Rejected alternative: emit for all six, accepting a zero pair and a
  restructured server exit.

- Decision: pin each participant's exact mapping count rather than assert a
  nonzero constant.
- Rationale: a participant reads once and records twice, so the generic branch's
  `baseline == peak` is structural and cannot fail. Without a pinned value only
  `peak != 0` would do any work, and a role that gained or lost a region would
  pass as "still constant, still nonzero".
- Rejected alternative: two reads at different points to make the equality
  measurable, which would put a second root round trip mid-script for a number
  whose steady state is the fact worth recording.

- Decision: report the steady state and leave transient peaks unreported.
- Rationale: three of the four transiently hold more, but a scripted participant
  has no sweep loop, and `fabric_trace_log`'s contract keeps serial writes off
  the traffic path. The only number it can honestly report is the one it holds at
  the end; the prose says so rather than implying a peak was captured.
- Rejected alternative: per-sweep sampling in components that have no sweep.

## Open risks and follow-ups

- [ ] `fabric-call-worker` still reports no occupancy; unblocking it needs
      trace-sink headroom that does not exist under `maxTraceDepth = 64`.
- [ ] `fabric-call-client`'s transient charge (1 page, 1 buffer, 1 mapping
      inside `send_large_request`) is unreported: the only place to sample it is
      the shared `fabric_call_scenario.rs`, which four binaries include, so a
      probe there would need to know which component it is running in.
- [ ] `fabric-call-server`'s occupancy is unreachable without changing its exit
      path; deferred rather than risking the verified call plane.
- [ ] The four participants' transient peaks are unmeasured evidence. Capturing
      them would need a sampling loop these components do not have.

## Artifacts and provenance

- Focused report: none; measurements are tabulated above.
- Raw transcript: none retained. The occupancy figures came from temporary probes
  in the four participants and in `fabric_call_scenario.rs`, reverted after
  measuring; the reported pairs are reproducible from `just sel4_traffic_check`'s
  own `[trace]` records.
- Serial/debugger/model output: `[trace] publisher|subscriber|publisher-b|subscriber-b`
  records with `event=13` in the traffic gate transcript.
- Related roadmap item: [C8.13.2](../../roadmap/02-core-runtime.md#c8132----full-shared-buffer-occupancy-coverage-across-all-declared-holders).
