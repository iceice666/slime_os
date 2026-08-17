# C8.14 — the fault envelope was already being driven; nothing asserted it

| Field | Value |
|---|---|
| Date | 2026-08-17 |
| Kind | Change |
| Status | Verified |
| Scope | `scripts/check/check-sel4-fault-plane.py`, `contracts/generation/v1/fixtures/sel4-fault.zti`, `scripts/build/build-sel4.py`, `scripts/build/build-generation.py`, `components/bins/src/bin/{init,fabric-proxy}.rs`, `scripts/check/check-sel4-gate-controls.py`, `Justfile`, `roadmap/02-core-runtime.md` |
| Roadmap | C8.14, C8.13 |
| Gates | `just sel4_fault_check`, `just data_fabric_fault_check`, `just sel4_gate_control_check` |
| Trigger | Implementing C8.14, whose deliverables read as eleven fault paths to exercise |
| Baseline | C8.13's concurrent traffic graph passed its own gate; no gate asserted any denial, degradation, or fault record it emitted |

## Summary

C8.14 turned out to be an assertion milestone, not an implementation one. Its
deliverable list names eleven degradation paths — stalled, malformed, denied,
retry-exhausted, cancelled, rejected, expired, timed-out, participant-death,
server-death, proxy-death — and ten of them were **already being driven** by
C8.13's concurrent graph, through the scripted scenarios `fabric_call_scenario`
and `fabric_operation_scenario` already contained. Measuring a traffic boot
before writing anything found 6 `kind=denial` records, 3 `kind=fault`
peer-death records, QoS records for timeout and retained-result expiry, and
component markers for every remaining condition. All of it unchecked: neither
`check-sel4-traffic-plane.py` nor its saturation sibling mentions
`kind=denial`, `kind=fault`, or peer death anywhere.

So `just sel4_fault_check` requires that vocabulary rather than inventing a new
scenario for it. The one condition that genuinely could not be scripted is a
declared interposition hop dying: a proxy that relays correctly cannot also be
absent, and under the traffic action the hop parks forever
(`fabric_boot::park_only` is `-> !`). That one is injected, which is the only
reason this plane needs its own image rather than only its own assertions.

Distinctness is asserted as *disjointness of codes within a family*, not as
presence. A broker that collapsed two conditions onto one status would still
emit both records and still pass a presence check, while a reader holding only
the transcript could no longer tell a duplicate from a stale session. That is
what "distinguishable" has to mean here.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/check/check-sel4-fault-plane.py` | New gate: `EXPECTED_DENIALS`, `EXPECTED_QOS_DEGRADATION`, `EXPECTED_FAULTS`, `EXPECTED_ISOLATION`, `EXPECTED_DISTINCT_DEGRADATIONS`, `EXPECTED_INJECTION`, plus `check_injection`/`check_distinct_degradations`/`check_isolation`; inherits `check_task_lifecycle`, `check_concurrency`, and `check_resources` unchanged | Every degradation the graph drives is now required, under its own code |
| `contracts/generation/v1/fixtures/sel4-fault.zti` | `sel4-traffic.zti` with `generation = 40` and nothing else | The plane differs by *build*, not by composition, so a fault cannot be confused with a structural change |
| `scripts/build/build-sel4.py` | `FAULT_VARIANT` across all four registries, `--fault-plane`, and a per-variant generation environment enabling the proxy injection | The injection is scoped to one variant rather than an ambient env var |
| `scripts/build/build-generation.py` | `sel4-fault` manifest entry | The fixture is buildable by name |
| `components/bins/src/bin/fabric-proxy.rs` | Injection moved ahead of the `park_only` arm | The hop can die on the one plane that asks it to; `park_only` never returns, so the old site was unreachable under this action |
| `components/bins/src/bin/init.rs` | `expect_parked` helper; the proxy is waited on rather than checked idle when the injection is compiled in | Init observes the hop leave rather than tolerating a task it stopped tracking |
| `scripts/check/check-sel4-gate-controls.py` | `sel4_fault_plane` pinned at 10 required markers | The new gate is proven to reject its own falsifications |
| `Justfile` | `sel4_fault_check` and the roadmap-named `data_fabric_fault_check` alias | The milestone's named target exists |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Two degradations collapse onto one status code | `just sel4_fault_check` | `denials collapsed onto [...], fewer codes than the conditions it drives` |
| A specific degradation stops being driven | `just sel4_fault_check` | `recorded no denial for <name> (status=N)` / `recorded no <name> (status=N event=M)` |
| A peer dies and its broker does not notice | `just sel4_fault_check` | `recorded no peer-death fault; ... which is what wedges a route worker forever` |
| A fault crosses into an unrelated route class | `just sel4_fault_check` | Any `EXPECTED_ISOLATION` marker missing |
| A fault leaks a loan, mapping, buffer, or correlation | `just sel4_fault_check` | Inherited `check_resources`: a baseline above zero, or above its own peak |
| A fault serializes the schedule | `just sel4_fault_check` | Inherited `check_concurrency`: `showed no marker from another plane between two of its own` |
| The image is built without the injection and passes as a second traffic boot | `just sel4_fault_check` | `this image is indistinguishable from a plain traffic boot, so rebuild with --fault-plane` |
| The injected hop faults instead of exiting cleanly | `just sel4_fault_check` | `exit statuses were [...], expected [0] -- an injected departure is declared, not a failure` |
| The gate stops rejecting its own falsifications | `just sel4_gate_control_check` | 32 gates / 1227 mutations count changes |
| The injection leaks into a sibling plane | `just sel4_traffic_check`, `just sel4_boot_check` | `parked participant left healthy idle` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_fault_check` | Passes: 10 markers across 3 causal chains, 19 participants concurrent while the hop died, **4 distinct denial codes, 3 distinct QoS degradations, 3 peer-death faults, 8 isolation markers** | Direct |
| Raw fault boot | `[fabric-proxy] injected early proxy death` followed by `component exit task=13 status=0`; all seven trace sinks `dropped=0 rejected=0` and byte-identical in record count to the traffic plane (`call` 63, `operation` 32, `stream` 26, four participants 3 each) | Direct |
| Pre-gate traffic-boot measurement | 6 `kind=denial`, 3 `kind=fault`, QoS timeout and expiry records, and every component degradation marker — all present and all unasserted. This is the measurement that redefined the milestone | Direct |
| First fault boot, before `init` was taught | `[init] fabric boot fail: parked participant left healthy idle` — init hardcoded both structural roles as parked, so it refused the injected death itself | Direct |
| `just sel4_traffic_check`, `just sel4_saturation_check`, `just sel4_boot_check` | All pass — the injection is scoped to its own variant | Direct |
| `just sel4_gate_control_check` | 32 gates reject 1227 mutations (was 31 / 1194) | Direct |
| `just contracts_check`, `just generation_check`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos` | All pass | Direct |

## Decisions

- Decision: assert the fault vocabulary the existing graph drives rather than build a new fault scenario.
- Rationale: measurement showed ten of the eleven named paths already running
  under `sel4-traffic.zti`, with their records reaching serial and nothing
  checking them. Writing a second scenario would have duplicated working
  machinery and left the original still unasserted — the actual gap.
- Rejected alternative: a bespoke fault fixture driving each condition in
  isolation, which would also have lost the concurrency property the milestone
  requires (a fault must not disturb a *live* neighbour).

- Decision: inject only the interposition-hop death, and give it its own image.
- Rationale: it is the one condition no participant can script, because a hop
  that relays correctly cannot also be absent. Everything else is a scripted
  request the graph already makes.
- Rejected alternative: injecting several faults into one plane, which would
  have made each fault's isolation claim depend on the others not having fired.

- Decision: assert distinctness as disjointness of codes, not presence.
- Rationale: presence is satisfiable by a broker that reports every condition
  under one status. Disjointness is the property a reader with only the
  transcript actually depends on.
- Rejected alternative: counting records per family, which a collapsed encoding
  passes unchanged.

- Decision: `init` waits on the hop rather than skipping it when the injection is on.
- Rationale: the alternative is to stop tracking the task, which would let a hop
  that faulted look the same as one that departed cleanly. Waiting makes the
  clean exit an assertion.
- Rejected alternative: excluding the proxy from both checks under the
  injection, which is what a gate cannot distinguish from the hop never running.

## Open risks and follow-ups

- [ ] The call worker's sink remains full at 63 of 64 records, so no additional
      call-plane fault evidence can be added without displacing existing
      evidence. Unchanged from C8.13.1; `MAX_TRACE_DEPTH = 64` is a page-sized
      schema ceiling, not a fixture knob.
- [ ] Stream-plane QoS degradations — deadline missed, liveliness lost,
      sample lost, incompatible QoS — are detected by `fabric-service` and
      reported to subscribers, but the stream broker records no `kind=qos`
      degradation of its own on this plane, so the gate asserts the call and
      operation planes' QoS codes only.
- [ ] `resourceEvent` still has no reachable emitter, for the reason
      `2026-08-16-c8-13-resource-event-loan-walls` root-caused. A fault plane
      does not change that: the `ERR_WOULDBLOCK` it needs is unreachable through
      a blocking `seL4_Send`.
- [ ] The injected hop death is the only *injected* fault. A stalled subscriber
      and a faulting (rather than exiting) participant are both still
      unexercised as injections, though the graph's scripted peer deaths cover
      the settlement path either would take.

## Artifacts and provenance

- Focused report: none; measurements are tabulated above.
- Raw transcript: not retained. Every figure is reproducible from
  `just sel4_fault_check`'s own output and its `[trace]` records.
- Serial/debugger/model output: `[fabric-proxy] injected early proxy death`,
  `kind=denial` records at statuses -4/-6/-7, `kind=fault order=peer-death`
  records on all three planes, and the eight `EXPECTED_ISOLATION` markers.
- Related roadmap item: [C8.14](../../roadmap/02-core-runtime.md#c814--degradation-and-fault-isolation).
