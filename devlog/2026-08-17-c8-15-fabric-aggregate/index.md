# C8.15 — the C8 parent close, and the C8.9 gate the audit found red

| Field | Value |
|---|---|
| Date | 2026-08-17 |
| Kind | Change |
| Status | Verified |
| Scope | `scripts/check/check-sel4-fabric-aggregate.py`, `scripts/check/check-data-fabric-profile.py`, `Justfile`, `roadmap/02-core-runtime.md`, `roadmap/README.md`, `roadmap/00-backlog.md` |
| Roadmap | C8.15, C8.9, C8 |
| Gates | `just sel4_fabric_aggregate_check`, `just data_fabric_check`, `just data_fabric_profile_check` |
| Trigger | Implementing C8.15, the C8 track's parent close |
| Baseline | C8.1–C8.14 complete under their own gates; no gate compared two runs of anything concurrent, and `just data_fabric_profile_check` was red |

## Summary

C8.15 closes the C8 track with one aggregate gate and one defect the audit
uncovered.

The gate asserts the two things no single-plane gate can. **Determinism**: the
same graph, inputs, and simulated-time sequence run twice must produce
byte-identical semantic traces, which is a claim about the relationship *between*
runs and so is unstatable by a gate holding one transcript. **One aggregate
path**: both required schedules — the normal concurrent one and the fault one —
are exercised over the same declared composition, rather than assembled from
unrelated profile boots. Measured: 4 boots, 140 trace records each, 280
byte-identical in total, with every narrow plane gate satisfied on all four.

The audit half of the milestone found that **C8.9's own gate had been red since
B55**. `just data_fabric_profile_check` failed with `invalid control grant
fabric-call-client-control`, and the failure reproduces at `ea40190` and back to
`e2f4833`, so nothing in this session caused it. The gate swept every declared
profile of the reference manifest through `resolve_fabric_profile`, including
`unified`, whose per-plane control-grant holder that manifest structurally
cannot satisfy. Recorded and closed as backlog **B56**.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/check/check-sel4-fabric-aggregate.py` | New gate: boots each plane in `PLANES` twice, runs that plane's own `check_transcript` against both boots in-process, then compares `[trace]` records verbatim with a pinned record count | Determinism is asserted between runs, and the aggregate cannot drift from what the narrow gates require |
| `scripts/check/check-data-fabric-profile.py` | The profile sweep resolves the single-broker profiles and states why `unified` is structurally excluded; a manifest declaring none at all fails rather than checking nothing | C8.9's gate asserts a property instead of a contradiction |
| `Justfile` | `sel4_fabric_aggregate_check` and the roadmap-named `data_fabric_check` alias | The parent milestone's named target exists |
| `roadmap/02-core-runtime.md`, `roadmap/README.md` | C8.15 and the C8 parent marked complete, with each closing slice's narrowed scope recorded; track map updated | Roadmap states measured scope, not intended scope |
| `roadmap/00-backlog.md` | B56 recorded in the resolved log with its root cause and the measurement that isolated it | A defect found and fixed is recorded rather than silently repaired |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A concurrent plane's trace becomes schedule-dependent | `just data_fabric_check` | `trace record N differs between boots -- the semantic trace depends on scheduling` |
| Every worker stops emitting, so two empty transcripts compare equal | `just data_fabric_check` | `emitted N trace records, expected 140` |
| The aggregate drifts from what a narrow plane gate requires | `just data_fabric_check` | Whatever that plane gate itself fails with, now on either boot |
| A plane stops reaching init's clean exit on a second boot | `just data_fabric_check` | `boot 2 did not reach init's clean exit` |
| The reference manifest loses every single-broker profile | `just data_fabric_profile_check` | `declares no single-broker fabric profile to resolve` |
| The `unified` profile regresses | `just sel4_boot_check`, `sel4_traffic_check`, `sel4_fault_check`, `sel4_saturation_check` | All four boot it; resolution alone was the weaker check |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_fabric_aggregate_check` | Passes: 2 schedules × 2 boots, each satisfying its own plane gate, 280 byte-identical trace records | Direct |
| Manual two-boot comparison, traffic plane | 140 records, byte-identical | Direct |
| Manual two-boot comparison, fault plane | 140 records, byte-identical | Direct |
| Traffic vs fault trace comparison | Identical — the interposition hop holds no trace sink, so the fault plane's distinguishing evidence is in markers, not records. Recorded because it explains why the aggregate pins one record count for both | Direct |
| `just data_fabric_profile_check` | Was red at `ea40190` and at `e2f4833`; passes now | Direct |
| Per-profile resolution of the reference manifest | `default` OK, `visibility` OK, `unified` FAIL — the measurement that isolated B56 | Direct |
| Retargeting the nine control grants to the workers | Moves the failure to `default` and `visibility` — measured, then reverted; this is what proved the gate rather than the fixture was wrong | Direct |
| `sel4_stream_check`, `sel4_qos_check`, `sel4_call_check`, `sel4_operation_check`, `sel4_visibility_check`, `sel4_matrix_check`, `sel4_boot_check`, `sel4_traffic_check`, `sel4_saturation_check`, `sel4_fault_check` | All pass | Direct |
| `interface_schema_check`, `fabric_manifest_check`, `fabric_authority_check`, `data_fabric_trace_check`, `sel4_boot_layout_check`, `sel4_gate_control_check` | All pass; gate controls at 32 gates / 1227 mutations | Direct |
| `just contracts_check`, `generation_check`, `test_sel4_root`, `test_host`, `lint_all`, `fmt_check_all`, `ruff`, `typos`, `devlog_check` | All pass | Direct |

## Decisions

- Decision: compare determinism over C8.11 trace records alone, not the whole transcript.
- Rationale: those records carry simulated time and the schema forbids task ids
  and addresses in them, so byte equality is a claim about the schedule rather
  than about capture. Several serial markers legitimately vary — a broker's
  per-edge print races a participant's own summary print — which is precisely
  why the plane gates check those as membership rather than as order.
- Rejected alternative: comparing whole transcripts, which would fail on
  variation the plane gates already established as benign.

- Decision: pin the record count as well as comparing the two boots.
- Rationale: a regression that stopped every worker emitting produces two
  identical empty transcripts, which satisfies a comparison perfectly.
- Rejected alternative: comparison alone, whose worst failure mode is passing
  vacuously.

- Decision: invoke each plane's own gate in-process against both boots.
- Rationale: the aggregate then cannot drift from what the narrow gate requires,
  and no boot is spent twice. Re-implementing the expectations would create a
  second copy to keep in step.
- Rejected alternative: re-invoking each gate as a subprocess, which would
  double the boots and make the aggregate's cost four boots per plane.

- Decision: fix B56 in the gate, not the fixture.
- Rationale: measured. A manifest carries one grant list, and B55 made the
  control-grant holder per-profile, so a manifest declaring both single-broker
  and worker-holder profiles cannot satisfy both rules. Retargeting the grants
  moved the failure rather than removing it. The real full-graph fixtures declare
  `unified` alone and target the workers, so they resolve it correctly — and four
  gates boot it, which is stronger evidence than resolving it in a manifest that
  cannot.
- Rejected alternative: retarget `valid.zti`'s nine control grants, which was
  measured and reverted; and splitting the reference manifest in two, which would
  duplicate every unrelated declaration to fix one sweep's premise.

## Open risks and follow-ups

- [ ] Normalized *schema* artifacts are compared for determinism on the host by
      `just generation_check` and `just data_fabric_profile_check`, not across
      these boots, so the byte comparison here covers semantic traces alone.
- [ ] The aggregate boots two schedules. The denial, stall, and malformed-input
      schedules C8.15's first deliverable names are carried inside those two
      rather than run as separate arms — each is driven and asserted by
      `sel4_fault_check`, which this gate runs against both of its boots.
- [ ] The fault plane's trace records are identical to the traffic plane's,
      because the interposition hop holds no trace sink. Its distinguishing
      evidence is entirely in serial markers, which this gate does not compare
      between planes (only between boots of the same plane).
- [ ] `unified` is no longer resolution-checked in the reference manifest. Four
      QEMU gates boot it, but a future manifest declaring `unified` *alone* would
      benefit from a resolution check the current sweep would skip.

## Artifacts and provenance

- Focused report: none; measurements are tabulated above.
- Raw transcript: not retained. Every figure is reproducible from
  `just sel4_fabric_aggregate_check`'s own output.
- Serial/debugger/model output: `both boots satisfied
  check/check-sel4-{traffic,fault}-plane.py and emitted 140 byte-identical trace
  records`, twice.
- Related roadmap items: [C8.15](../../roadmap/02-core-runtime.md#c815--full-graph-determinism-and-parent-close),
  [B56](../../roadmap/00-backlog.md).
