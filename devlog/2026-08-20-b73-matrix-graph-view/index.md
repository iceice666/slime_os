# B73: the matrix plane's graph-wide view is read, not just counted

| Field | Value |
|---|---|
| Date | 2026-08-20 |
| Kind | Defect |
| Status | Verified |
| Scope | `components/bins/src/bin/fabric-publisher.rs`, `scripts/check/check-sel4-matrix-plane.py`, `scripts/check/check-sel4-gate-controls.py` |
| Roadmap | B73 |
| Gates | `just sel4_matrix_check`, `just sel4_gate_control_check` |
| Trigger | B73, recorded when the matrix plane's visibility policy moved onto the graph |
| Baseline | `just sel4_matrix_check` green at `b942d0c`, with the `private` view branch asserted by `fabric-observer` |

## Summary

The matrix plane asserted only half of its visibility contract. `fabric-observer`
proved what a `private` holder is *denied* — it sees `diagnostics` and nothing
else — but no component ever read what a `graph` holder is *shown*. The route
set on the graph-wide view was paged and counted elsewhere and never compared
against anything, so a route silently leaving that view changed no assertion.
Confirmed by mutation: flipping both of `telemetry-alt`'s participants from
`graph` to `private` in `contracts/generation/v1/fixtures/sel4-matrix.zti` took
the graph view from three routes to two and passed `just sel4_matrix_check`
unchanged. Fixed by having `fabric-publisher`, which holds `graph` visibility,
page its own view and assert the three declared route names in order against
source literals. The same mutation now fails, naming `telemetry-alt`.

## Observable symptom

- Command: `just sel4_matrix_check`, with `sel4-matrix.zti` lines 166 and 180 flipped from `visibility = "graph"` to `visibility = "private"`
- Expected: a failure — the graph-wide view lost a route
- Observed: exit 0, the plane's full success sentence printed
- Exit/fault/serial evidence: the gate's ordered marker chains all matched, including `SLIME_ROOT fabric graph=admitted schemas=2 routes=3 participants=7 interpositions=1`; no component reported a failure

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `matrix_broker.rs`'s `answer_view` (line 664) emits route records only — no QoS record and no `write_record` hex dump, unlike `visibility_broker.rs` | B72's approach (gate-side decode of dumped bytes against a frozen fixture) has nothing to consume here and does not transfer |
| 2 | `serve` (line 502) dispatches a visibility request from *any* control endpoint, and all seven matrix components hold one | A `graph` holder can page its own view; no new grant or broker change is required |
| 3 | `sees_graph` (line 352) scans every declared row for the holder's identity, not just this broker's routes | Flipping `telemetry-alt` does not cost `fabric-publisher-b`/`fabric-subscriber-b` their graph status — they still hold `graph` on `diagnostics` — so the mutation changes the view, not the holders |
| 4 | The plane's trace ran 18 records against `traceDepth = 24` with `saturate`, and the gate fails on any drop | A three-page walk (21) fits; the guard cannot redden the gate by overflowing the trace |
| 5 | The mutation was applied and `just sel4_matrix_check` run *before* any guard existed: exit 0 | The defect reproduces, and the mutation is admission-neutral — the gate enforces the `graph=admitted … interpositions=1` marker in order and it still matched |

## Root cause

The plane's visibility coverage was asymmetric. `nth_visible_route`
(`matrix_broker.rs:737`) admits a route to a holder's view either because the
holder sees `graph` and some row on that route declares `graph`, or because the
holder has a `private` grant on it. `fabric-observer` exercised the second
branch and asserted its result exactly — one route, named `diagnostics`. The
first branch had no asserting component at all. The broker computed a correct
graph-wide view every boot and every boot discarded it unread, which is the same
shape as B72: a view is paged and counted, but the contents of what it admits go
unasserted. A count is invariant under any permutation or removal that preserves
cardinality, and here even cardinality moved without consequence.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `components/bins/src/bin/fabric-publisher.rs` | `matrix_main` pages its own view and asserts `telemetry`, `telemetry-alt`, `diagnostics` in order, then `routes == 3`; emits `[fabric-publisher] matrix graph view routes=3` | The route set shown to a `graph` holder is observed, in order, by a component that holds `graph` |
| `scripts/check/check-sel4-matrix-plane.py` | New required chain, "a graph-wide view shows every route declared graph" | The marker cannot silently stop being emitted |
| `scripts/check/check-sel4-gate-controls.py` | `sel4_matrix_plane` marker pin 25 → 26 | The plane's marker count stays pinned to its actual coverage |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A route leaves the graph-wide view | `just sel4_matrix_check` | `[fabric-publisher] matrix graph view expected telemetry-alt but was shown diagnostics`, then `fail: matrix graph view route order` |
| The view is reordered without changing its size | `just sel4_matrix_check` | Same, at the first position that moved — the names are positional, not a set |
| The marker stops being emitted | `just sel4_matrix_check` | `a graph-wide view shows every route declared graph: missing marker` |
| The plane loses a marker | `just sel4_gate_control_check` | `declares N required markers, expected 26` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_matrix_check`, fixture unmutated, before the guard | Pass — the defect | Direct |
| `just sel4_matrix_check`, lines 166/180 flipped, before the guard | **Pass** — reproduces B73 and proves the mutation is admission-neutral | Direct |
| `just sel4_matrix_check`, fixture unmutated, with the guard | Pass | Direct |
| `just sel4_matrix_check`, lines 166/180 flipped, with the guard | **Fail**, naming `telemetry-alt`; no `UnsatisfiableFabricGraph`, and the failure follows `matrix sample published`, so components launched | Direct |
| Fixture restored via `git checkout` | Pass | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff` | Pass | Direct |
| `just sel4_gate_control_check` | Fail at pin 25, pass at 26 | Direct |

## Decisions

- Decision: assert component-side, in `fabric-publisher`, rather than adding a gate-side decoder.
- Rationale: the matrix broker emits no hex dump for a gate to decode, and `serve` already answers any control-endpoint holder, so the assertion needs no new authority and no broker change. This is the smaller change.
- Rejected alternative: teach `matrix_broker.rs` to dump records the way `visibility_broker.rs` does, then freeze them gate-side as B72 did. That is a broker change plus a new frozen fixture to serve an assertion the component can already make directly.

- Decision: the expected route names are source literals in the component.
- Rationale: an expectation derived from `sel4-matrix.zti` moves with the fixture a mutation edits and stays green — the vacuity that retired `FABRIC_INTERPOSITIONS` under B70.
- Rejected alternative: generating the expected names from the manifest, as the participant tables do. Rejected for self-confirmation, not for cost.

- Decision: a `graph` holder asserting route *names* is not a privacy inversion.
- Rationale: B72 rejected component-side literals because `fabric-publisher` would have had to carry `fabric-publisher-b`'s manifest-derived QoS. Route names on a graph-wide view are the graph's public shape, which is precisely what `graph` visibility grants; no other component's private data crosses.

## Open risks and follow-ups

- [ ] The guard asserts route names and contract kind, not the schema identity each record carries. A record naming the right route with the wrong `schema_identity` would pass. `answer_view` derives it from the route index, so the two cannot currently disagree, but that is an implementation property rather than an asserted one.
- [ ] Flipping only *one* of `telemetry-alt`'s two participants is invisible through this guard, by construction: `nth_visible_route` admits a route when `rows_on(route).any(visibility == GRAPH)`, so the surviving `graph` row keeps the route in the view with the count unchanged. No stronger component-side assertion rescues it — the route record carries only `route_name`, `contract_kind`, `schema_identity`, and `flags`, nothing per-participant. Catching a single flip would need `answer_view` to emit per-row visibility, the same emission gap that kept B72's approach from transferring. The exit condition is therefore the both-participant flip, and this bound is recorded rather than chased.
- [ ] `ViewPage::Qos` is accepted and skipped for shape parity with the visibility plane's loop. The matrix broker never emits one today; if it starts, the guard will silently ignore it.

## Artifacts and provenance

- Focused report: this entry
- Raw transcript: none retained; every result above is reproducible by the commands named in Verification
- Serial/debugger/model output: quoted inline under Observable symptom and Verification
- Related roadmap item: B73
