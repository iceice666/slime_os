# C8.13.3 — the one declared ceiling with no signal, and the two slot spaces it turned out to have

| Field | Value |
|---|---|
| Date | 2026-08-17 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{cspace.rs,task.rs,graph.rs,main.rs,generation.rs,peer_endpoint.rs,lib.rs}`, `components/runtime/src/{lib.rs,syscall.rs,syscall/sel4_transport.rs}`, `components/bins/src/bin/fabric-service.rs`, `components/proto/tests/fabric_trace.rs`, `contracts/fabric-trace/v1/{schema.zt,gen_rust.zt}`, `scripts/lib/fabric_graph_limits.py`, `scripts/check/check-sel4-{traffic,saturation}-plane.py`, `docs/{syscall-abi.md,capability-matrix.md}`, `Justfile`, `AGENTS.md` |
| Roadmap | C8.13.3, C8.13 |
| Gates | `just sel4_traffic_check`, `just sel4_saturation_check`, `just test_sel4_root`, `just lint_all`, `just fmt_check_all` |
| Trigger | Implementing C8.13.3, the last of C8.13's three broken-out resource slices |
| Baseline | `capabilitySlots` was compared only to a fixed `LIMIT_CAPABILITY_SLOTS` at decode time and to `graph::MAX_TASK_CAPS` at admission; nothing anywhere counted a live holder's slots |

## Summary

C8.13.3 gives `capabilitySlots` the signal it never had. The root now answers
`CAPABILITY SLOT OCCUPANCY` (label 31) with the caller's own live slot counts,
the stream broker reports them as C8.11 trace evidence, and both traffic gates
check the observed peak against the ceiling the fixture itself declares. Measured
at declared peak 35, live baseline 29, against a declared 48.

Two premises changed under review, and both were category errors worth recording
rather than quietly fixing.

**A child's slots live in two spaces, not one.** The first implementation
censused the physical child CNode — 128 slots at fixed addresses — and compared
that count to `capabilitySlots`. But `capabilitySlots` does not bound that
space: `build-generation.py` derives its required value as
`FABRIC_FIRST_CONTROL_SLOT + control endpoints + buffers`, all in the
*component's own logical numbering from 0*, and `fabric_graph_is_satisfiable`
validates it against `graph::MAX_TASK_CAPS` (64), not against the CNode. A
logical index of 3 lives at physical slot 36. Comparing the physical count to
that ceiling would have failed a holder whose declared budget was satisfied —
and it passed only because the observed 25 happened to sit under 48. The reply
now carries both counts and each is checked against its own bound.

**The peak is the root's to track, not a component's.** The broker originally
sampled per sweep and kept the maximum. But declared occupancy moves on every
install, drop, transfer, and retirement — all root operations — so a component
sampling twice reports the higher of two snapshots, not the run's high-water
mark. That is not a nuance here: this plane's count genuinely rises and falls,
because the broker drops the supervision handles it no longer waits on. The root
now maintains the mark across every mutation and hands it back with the live
count, and the broker takes exactly one query at drain. That also removed a real
cost: this is the one root operation whose price is O(CNode size), and it was
being paid on every progressing sweep of the single-threaded dispatch loop.

A third simplification followed from the split. Only declared space is
*retained*: the root can account for it because every install into it is a root
operation. Physical occupancy is never stored, because a stored copy could only
be stale — the child fills physical slots the root does not mediate — so every
read takes a fresh census and nothing caches the answer.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/src/cspace.rs` | New module: `CSpaceLedger` (declared installs plus a high-water mark), `census`, `breaches_ceiling`, `capacity_of` | A live count exists at all, and only the space the root can account for is retained |
| `slime-root/src/graph.rs` | `AuthorityTable` tracks its own high-water mark | The table half of declared space peaks where it mutates, not where it is read |
| `slime-root/src/task.rs` | `Task.cspace`; `recount_cspace` (a fresh census, never cached); `declared_slots_occupied` returning live and peak | A physical count is only ever answered from the kernel, since a stored copy could only be stale |
| `slime-root/src/main.rs` | Label 31, its dispatch arm, `pack_slot_occupancy`, and declared-space credits at the boot-time notification install | Self-scoped by construction: the CSpace counted is the badge's |
| `slime-root/src/peer_endpoint.rs` | `materialize` credits each holder's declared installs per instance | A holder's declared count includes the native endpoints its generation named |
| `slime-root/src/generation.rs` | `Admission.fabric_capability_slots`, from the graph's own declared limit | The ceiling is decoded once at admission, not per request |
| `components/runtime/src/*` | `SlotOccupancy` and `capability_slot_occupancy()` | A component learns only its own occupancy, in both spaces |
| `contracts/fabric-trace/v1/*` | `resourceCapabilitySlots = 14`; `resourceComplete`/`maxResourceCounter` renumbered to 15; regenerated via `just fabric_trace_gen` | Zutai stays the single source of truth; `maxResourceCounter == resourceComplete` holds |
| `components/proto/tests/fabric_trace.rs` | New code added to the declared-counter admission loop | The counter is exercised by name, as its two predecessors were |
| `components/bins/src/bin/fabric-service.rs` | One drain-time query supplying both records of the pair | Neither record can be a snapshot the other disagrees with |
| `scripts/lib/fabric_graph_limits.py` | `declared_limits(fixture)`, shared by both gates | One parser for one grammar, rather than two copies to diverge |
| `check-sel4-{traffic,saturation}-plane.py` | Held-and-released pair shape, plus the peak checked against the fixture's own `capabilitySlots` | The exit condition, with the bound read from the fixture so loosening it moves the assertion |
| `docs/syscall-abi.md`, `docs/capability-matrix.md` | Label 31's operands/result, both slot spaces, and why it is gated by neither a rights bit nor a table | Roadmap invariant 4 |
| `Justfile`, `AGENTS.md` | Pinned root test count 114 → 118, 13 → 14 modules | B23's asserted count matches the new module's tests |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A holder's occupancy passes a declared bound and is only reported, not caught | `just sel4_traffic_check`, `just sel4_saturation_check` | `SLIME_GRAPH cspace occupancy over-ceiling` / `over-capacity` / `refused` are `FAILURE_MARKERS` |
| The query regresses to answering zero | `just sel4_traffic_check` | `'capability-slots' peak was 0` |
| Real declared occupancy passes the declared ceiling | `just sel4_traffic_check` | `occupies N declared capability slots, exceeding the 48 its generation declares` |
| The pair becomes incoherent evidence | `just sel4_traffic_check` | `baseline N exceeded its own peak M` |
| The fixture is loosened to hide a breach | either gate | The bound is read from `fabricGraph.limits`, so loosening moves the assertion with it |
| The new records overflow the broker's sink | `just sel4_traffic_check` | `dropped=N` in the `stream complete` line |
| A standalone C8.4–C8.9 fixture receives traffic-only records | `just sel4_stream_check`, `sel4_qos_check`, `sel4_call_check`, `sel4_operation_check` | `dropped=N`, or a plane failing its own marker set |
| The declared peak stops being a high-water mark | `just test_sel4_root` | `the_peak_is_a_high_water_mark_over_credits` |
| A credit wraps instead of saturating | `just test_sel4_root` | `credits_saturate_rather_than_wrap` |
| An absent or zero-declared ceiling is treated as a ceiling of zero | `just test_sel4_root` | `a_zero_ceiling_is_never_treated_as_a_ceiling` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_traffic_check` | Passes | Direct |
| Raw traffic boot, `[trace] stream` resource records | `event=14 high_water=35` then `29` — declared peak and live baseline, against a declared `capabilitySlots = 48`; `stream complete capacity=64 records=26 dropped=0 rejected=0`; no `over-ceiling` or `over-capacity` marker | Direct |
| Intermediate boot, before the two-space split | Physical census read 25; the number that revealed the ceiling was being compared across spaces, since 25 physical slots and 48 declared slots are unrelated quantities | Direct; the measurement that redirected the slice |
| Intermediate boot, per-sweep sampling | `peak 33 / baseline 29` — the measurement that disproved "declared slots are held for the holder's life" and moved the counter to `resourceLoan`'s shape | Direct |
| `just sel4_saturation_check` | Passes | Direct |
| `just test_sel4_root` | 118/118 across 14 modules | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just test_host`, `just contracts_check`, `just generation_check`, `just data_fabric_trace_check`, `just sel4_gate_control_check` (31 gates reject 1194 mutations), `just devlog_check` | All pass | Direct |

## Decisions

- Decision: report occupancy in both slot spaces and check each against its own bound.
- Rationale: `capabilitySlots` budgets the component's own logical numbering,
  and the CNode's capacity bounds the physical addresses those indices resolve
  to. One number cannot answer both, and comparing either to the other's
  ceiling would fail a holder whose declared budget is satisfied.
- Rejected alternative: census the physical CNode and compare that to
  `capabilitySlots`, which passed only because the observed count happened to
  sit under a ceiling it was unrelated to.

- Decision: the root maintains the declared high-water mark; the component queries once.
- Rationale: every mutation of declared space is a root operation, so between two
  queries the count moves where only the root can see it. A component's peak is
  the maximum of its own snapshots, which is a different and smaller claim. It
  also removes an O(CNode size) root operation from every progressing sweep.
- Rejected alternative: per-sweep sampling in the broker, which both understated
  the peak and paid 128 kernel calls per sweep for the privilege.

- Decision: physical space is censused on every read and never retained;
  declared space is credited and retained.
- Rationale: declared installs are root-mediated end to end, so a credit is
  complete, and retaining it is what lets the root hold a high-water mark no
  reader could observe. Physical occupancy is not root-mediated — the receiving
  runtime moves a transferred Endpoint out of `CHILD_SLOT_RECEIVE` itself — so an
  accumulated physical count would understate every holder that has accepted a
  transfer, and a stored one could only be stale.
- Rejected alternative: report `AuthorityTable::len()` alone, which is one line
  and wrong twice over: a logical entry is not a physical install, and
  `serve_capability_import` populates the table without touching the CNode.

- Decision: omit the graph's declared `capabilitySlots` from the reply.
- Rationale: it is a generation-wide limit, not a property of the caller's
  CSpace, and `SERVICE_CAPABILITY_TRANSFER` is required of any instance holding
  an `Endpoint` or transferable grant — which in the fabric fixtures includes the
  ungranted `fabric-probe` intruder. Shipping it would have handed a graph fact
  to a component the graph grants nothing, and no caller reads it: both gates
  take the ceiling from the fixture.
- Rejected alternative: a fourth packed field carrying the ceiling, which reads
  as self-describing evidence but is a second copy of a manifest value.

- Decision: do not credit the export mirror to declared space.
- Rationale: `sender_ticket_slot` is derived from `source_slot`, so the mirror
  occupies a physical slot standing for a declared capability already counted.
  Crediting it would double-count one capability against the ceiling. The
  physical census sees it, which is the bound it actually consumes.
- Rejected alternative: credit it and widen `cleanup_export_ticket` to `&mut`,
  which inflated the very number the ceiling check reads.

- Decision: report a ceiling breach on serial rather than refusing the query.
- Rationale: the slots are already installed by the time anyone can count them,
  so refusing to answer would hide the single fact the declaration exists to
  surface. Reported on the peak, since a ceiling a run passed through and came
  back under was still passed.
- Rejected alternative: return an error over the ceiling, converting a reporting
  mechanism into an enforcement one the milestone did not ask for.

## Open risks and follow-ups

- [ ] The census costs up to 128 kernel calls per query on the root's
      single-threaded dispatch loop. Now bounded to one query per broker run
      rather than one per sweep, but it remains the only root operation whose
      cost is O(CNode size).
- [ ] Only the stream broker reports. The four instrumented participants have
      sink headroom and could report the same counter; the call worker still
      does not.
- [ ] The observed peak of 35 sits under the declared 48, so the ceiling is
      checked but not saturated. Driving it to its exact bound would need a
      fixture that tightens `capabilitySlots` the way `sel4-saturation.zti`
      tightens `inFlightOperations`.
- [ ] The declared peak sums two independently tracked halves — the root's own
      installs and the task's authority table — so it can name a total neither
      half held simultaneously. That is the conservative direction for a ceiling
      report, and both halves stay bounded, but it is an over-approximation
      rather than an exact simultaneous maximum.
- [ ] `queueDepth` remains the one declared field with no live check at all;
      `2026-08-16-c8-13-declared-fields-audit` records why.

## Artifacts and provenance

- Focused report: none; the measurements are tabulated above.
- Raw transcript: not retained. The reported pair is reproducible from `just
  sel4_traffic_check`'s own `[trace] stream` records.
- Serial/debugger/model output: `[trace] stream` resource records carry
  `event=14` for this counter, alongside `event=13` (mapping) and `event=12`
  (loan) from C8.13.1.
- Related roadmap item: [C8.13.3](../../roadmap/02-core-runtime.md#c8133----live-per-child-capability-slot-occupancy).
