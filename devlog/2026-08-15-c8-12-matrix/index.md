# C8.12 — one graph, every mismatch, and the two mutual waits it took to serve it

| Field | Value |
|---|---|
| Date | 2026-08-15 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/generation/v1/fixtures/sel4-matrix{,-unsatisfiable}.zti`, `contracts/fabric-trace/v1/{schema.zt,gen_rust.zt}`, `components/proto/src/{fabric_trace.rs,lib.rs}`, `components/proto/tests/fabric_trace.rs`, `components/bins/src/{matrix_broker.rs,fabric_matrix.rs,lib.rs}`, `components/bins/src/bin/{fabric-service,init,fabric-publisher,fabric-subscriber,fabric-publisher-b,fabric-subscriber-b,fabric-observer,fabric-probe,fabric-proxy}.rs`, `boot-contracts/src/generation.rs`, `scripts/build/{build-generation.py,build-sel4.py,boot_layout.py}`, `scripts/check/{check-sel4-matrix-plane.py,check-sel4-boot-layout.py,check-boot-layout-resource.py,check-sel4-gate-controls.py}`, `contracts/boot-layout/v1/fixtures/sel4-matrix.layout`, `Justfile` |
| Roadmap | C8.12 |
| Gates | `just sel4_matrix_check`, `just data_fabric_matrix_check`, `just sel4_gate_control_check`, `just sel4_boot_layout_check` |
| Trigger | C8.12 was the next uncompleted milestone with a fully specified exit condition; B55 (its C8.10 dependency) resolved earlier the same day |
| Baseline | C8.1–C8.11 complete; four trace families (`schema`, `visibility`, `interposition`, `denial`) had validator arms and generated codes but no emitter, per C8.11's own open-risk note |

## Summary

C8.12 asked for one graph that matches exactly, refuses everything else, and
proves a filtered view is not authority — all at once, with alternate-name and
conflicting-type routes as the cases that only exist when matching and
visibility run together. That is now `sel4-matrix.zti`: three routes (two of
them `TelemetryStream` under different names), seven participants including a
real-endpoint-holding ungranted probe, a declared interposition proxy, and a
read-only observer, brokered by a new `matrix_broker.rs` module. `just
sel4_matrix_check` boots it and asserts the exact matched/denied sets per
refusal class, a filtered view bounded to its grant, the declared chain as the
only telemetry path, and — the milestone's other half — a sibling generation
declaring one incompatible QoS pair, which `slime-root` refuses at admission
before any component launches.

The instructive part was concurrency, not matching. The matrix broker is the
first C8 worker to interleave role provisioning with the declared-chain relay
in one non-blocking dispatch loop rather than running them as two phases, and
that interleaving produced two genuine mutual waits: gating a blocking relay
send on the wrong "has this client been answered" flag, and a livelock from
marking every idle poll as scheduler progress. Both were caught by a five-lens
review panel before commit, not by the gate — the gate's 240-second watchdog
reports a bare timeout for either, which is exactly the failure mode the
review's concurrency lens exists to catch ahead of a flaky CI run.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/generation/v1/fixtures/sel4-matrix.zti` | New C8.12 generation: three routes, seven participants, one declared interposition chain, boot action `matrix` (27), generation 34 | One graph exercises exact matching, alternate names, conflicting types, denial, visibility, and interposition together |
| `contracts/generation/v1/fixtures/sel4-matrix-unsatisfiable.zti` | Sibling generation (35) with one `telemetry-alt` publisher weakened to BEST_EFFORT against its RELIABLE subscriber | Proves the milestone's incompatible-QoS half where it actually lives — at `fabric_graph_is_satisfiable` admission, not at an unreachable runtime event |
| `components/bins/src/matrix_broker.rs` | New broker: exact-tuple matching split into ungranted/name-mismatch/type-mismatch denial classes, filtered visibility, declared-chain relay, one non-blocking dispatch loop over all eight sources | Every C8.12 case answered from the graph and the caller's control endpoint alone; no phase blocks on a source the loop is still serving |
| `components/bins/src/fabric_matrix.rs` | New shared client helper: `request_role`, `Outcome`, and a denial-reply validator that rejects any capability, rights, or route identity on a refusal | One capability-drop and one denial-shape check, not seven copies |
| Seven participant binaries | Each gained a `matrix_main()` arm behind `fabric_matrix::active()` | Every C8.12 case has a real component exercising it, not a synthesized transcript |
| `contracts/fabric-trace/v1/schema.zt`, `gen_rust.zt` | `graphViewAnswered`/`graphHopTraversed`/`maxGraphEvent` for the visibility/interposition event vocabulary; `resourceRoles` alongside the other resource counters | The wire vocabulary a cross-process trace record carries stays schema-owned, not a component constant |
| `components/proto/src/lib.rs` | `KIND_VISIBILITY \| KIND_INTERPOSITION` validator arm bounds `event <= MAX_GRAPH_EVENT` | A record outside the declared graph-event vocabulary is refused, not silently accepted |
| `boot-contracts/src/generation.rs`, `init.rs` | `BootAction::Matrix = 27`, matching `boot_action::MATRIX` const-asserted against it | The numeric ABI a fixture's `bootAction` and `init`'s dispatch agree on is pinned, as every prior action is |
| `scripts/build/build-generation.py` | `MATRIX_FABRIC_PROFILE`, matrix control-grant tuple, `FABRIC_EXTRA_ROUTE_CATALOGUE` for the alternate-name route, `denied_components` supervision grant so the ungranted probe settles | A sibling seL4 manifest can declare a route the x86 canonical source does not, without the typo check going toothless |
| `scripts/check/check-sel4-matrix-plane.py` | New gate: exact matched/denied sets per refusal class, all four previously-silent trace families present and bounded, terminal record last, and the negative admission boot | The milestone's required checks are each independently falsifiable, not implied by a passing boot alone |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A refused caller's second/third request silently discarded between the idle poll and the settle check | `just sel4_matrix_check` | `fabric-publisher-b`'s conflicting-type ask, or the probe's four asks, missing from the matched/denied sets |
| The relay's blocking upstream send racing the proxy's own pending role reply | `just sel4_matrix_check` | Bare 240s boot timeout with no failure marker — proven by reverting the `proxy_replied` gate and observing the hang directly |
| The dispatch loop reporting progress on an idle-but-unsettled poll | `just sel4_matrix_check` | Same bare timeout, from CPU starvation rather than a blocked send — proven by reverting the `progressed` fix and observing the identical symptom |
| A malformed message on a shared control endpoint killing every other caller's pending request | `just sel4_matrix_check` | A P1 review finding, fixed before boot; no transcript evidence exists for the reverted state because it was never built |
| A trace family emitted with an out-of-vocabulary or mislabeled event/counter | `just test_host`, `just sel4_matrix_check` | `a_graph_record_must_name_a_declared_event`/`a_resource_record_must_name_which_count_it_carries` refuse the bound; the gate's `records != capacity`/undeclared-family checks catch a live drift |
| A new gate with no teeth | `just sel4_gate_control_check` | 29 gates, 1128 mutated transcripts and layouts |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_matrix_check` (×4 across the fix cycle) | PASS — exact matched/denied sets, all four trace families present and bounded, terminal last, negative admission refused before launch | Direct |
| `just sel4_gate_control_check` | PASS — 29 gates, 1128 mutations | Direct |
| `just sel4_boot_layout_check` | PASS — 25 plane layouts, including the new `sel4-matrix` row | Direct |
| `just contracts_check`, `just generation_check` | PASS | Direct |
| `just test_host` (19 trace tests incl. 2 new), `just test_sel4_root` (112) | PASS | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | PASS | Direct |
| `just sel4_visibility_check`, `sel4_stream_check`, `sel4_qos_check`, `sel4_call_check`, `sel4_operation_check`, `sel4_boot_check`, `sel4_trace_check` | PASS (regression) | Direct |
| Reverting the `proxy_replied` gate on the relay send | Boot hangs to the 240s watchdog, no failure marker | Direct |
| Reverting the `progressed` fix on the idle-poll arm | Identical hang, confirmed by transcript diff showing no task progressing past the probe's exit | Direct |
| `just data_fabric_profile_check` | FAIL — identical on unmodified baseline commit `e2f4833`; unrelated to this change | Direct, pre-existing |

## Decisions

- **Decision:** Prove the incompatible-QoS half of the matching matrix at
  admission, via a sibling generation, rather than at runtime.
  **Rationale:** `slime-root`'s `fabric_graph_is_satisfiable` refuses any
  generation whose graph fails `all_pairs_qos_compatible` before any component
  launches (C8.2's own exit condition). `fabric-service::EVENT_INCOMPATIBLE_QOS`
  is therefore unreachable on any generation that boots. The stronger property
  — the graph never reaches components at all — is what the milestone's
  "without authority" language already implies, and a sibling fixture proves it
  directly rather than weakening admission to make the runtime path reachable.
  **Rejected alternative:** Relaxing `fabric_graph_is_satisfiable` to a warning
  so the event fires live. Deletes a fail-closed guarantee, contradicts C8.2's
  exit condition, and breaks an existing `slime-root` host test.

- **Decision:** Refuse the probe's call/operate/cancel/retrieve verbs by
  absent capability, asserted from the generation's own tables, rather than by
  a runtime broker denial or a raw invocation of an empty slot.
  **Rationale:** Invoking a capability slot holding nothing faults the task
  under seL4 rather than returning an error — `fabric-proxy` already documents
  this constraint for its own wrong-right case. `declared_capabilities`
  broadens across every class the resolved profile can bind (stream/call/
  operation control endpoints, route edges, notification bindings) so the
  assertion cannot pass by the accident of one class going unread.
  **Rejected alternative:** Adding a runtime `deny()` path to the call and
  operation brokers so the probe could invoke them and be refused gracefully.
  Both brokers currently treat an undeclared client as a boot-time
  impossibility (`verify_graph`'s `fail()`), and adding a parallel denial path
  there is new mechanism the milestone does not require.

- **Decision:** One non-blocking dispatch loop over every source (seven
  control endpoints plus the telemetry ingress), not two phases.
  **Rationale:** The subscriber and proxy do not exit until the relay hands
  them a sample, and a caller has not finished asking until it exits — a sweep
  that waited for every caller to settle before relaying would wait on two
  callers the relay itself has to release. This is also what produced the two
  concurrency defects the review panel caught: `client.answered` means
  "settled, will never ask again" (matching `fabric-service::Client`'s
  existing field), not "was replied to", so gating the relay's blocking send on
  that field waits for the proxy to settle while the proxy waits for the relay
  — closed with a dedicated `proxy_replied` flag tracking the reply itself.
  **Rejected alternative:** Two phases (provision to completion, then relay),
  matching `visibility_broker.rs`. Rejected because it cannot express this
  plane's requirement that filtered introspection and the interposition chain
  run *concurrently* with ordinary matching rather than after it.

## Open risks and follow-ups

- [ ] `declared_capabilities`'s notification-binding count assumes the
  five-tuple `FABRIC_NOTIFICATION_BINDINGS` shape holds; a future binding kind
  added to that table needs a matching arm here or the probe's capability
  count silently undercounts again.
- [ ] The matrix plane's boot-layout fixture (`sel4-matrix.layout`) covers only
  the positive generation; the incompatible-QoS sibling has no layout fixture
  because it never reaches a booted `init` to emit one — intentional, recorded
  in `check-boot-layout-resource.py`'s stem exclusion, not an oversight.
- [ ] `just data_fabric_profile_check` fails identically on unmodified
  baseline `e2f4833`; pre-existing, unrelated to C8.12, not
  investigated further here.

## Artifacts and provenance

- Related roadmap item: [C8.12 in the core-runtime track](../../roadmap/02-core-runtime.md)
- Related devlog entry: [C8.11 — a deterministic trace](../2026-08-15-c8-11-semantic-trace/index.md), whose open-risk note named this entry's four trace families
- Related backlog item: [B55](../../roadmap/00-backlog.md), resolved earlier the same day and a precondition for opening C8.12
- Review: five concurrent lens reviews (canonical, correctness, security,
  concurrency, convention) over the uncommitted diff, run twice — the first
  batch partially invalidated by in-flight edits, the retry against a stable
  snapshot. Findings applied: two P1 concurrency defects (relay deadlock,
  dispatch livelock), one P1 security defect (malformed-input broker crash),
  one P1 convention gap (missing terminal/resource trace records), plus
  P2/P3 evidence-quality and schema-ownership fixes. One finding (a vacuous
  intersection check) was resolved as dead code removal rather than a fix.
