# P5.4.10 (part) — per-pair QoS compatibility at admission

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/generation.rs`, `Justfile`, `AGENTS.md` |
| Roadmap | P5.4.10, P5.4.4, P5.4, C8.2 |
| Gates | `just test_sel4_root`, `just sel4_stream_check` |
| Trigger | P5.4.4, which closed C8.2's aggregate half and named the live-graph assertions as still open |
| Baseline | Nine seL4 gates passing; the root validating aggregate ceilings only |

## Summary

P5.4.4 gave `slime-root` C8.2's *aggregate* admission — `validate_against`
against this root's ceilings — and recorded three live-graph assertions from
`kernel/tests/fabric_manifest.rs` as still uncovered. Two of them turn out to be
already enforced: `FabricGraph::decode` runs `validate_participants` and
`validate_interposition`, so route-membership counts, chain termination, hop
revisits, and self-bypass are refused before any caller sees the graph. The
third was genuinely missing. `all_pairs_qos_compatible` is a *query* rather than
a decode error — C8.5 treats an incompatible pair as a runtime event — and
nothing on the seL4 path called it. Now the admission does, so a graph within
every ceiling that still promises a reader more than its writer offers fails
closed.

## Changes

| Area | Change | Effect |
|---|---|---|
| `generation.rs` | `fabric_graph_is_satisfiable` also requires `all_pairs_qos_compatible` | A graph the root cannot honour is refused before participants launch |
| `generation.rs` tests | `qos_graph` builds a real one-route stream graph with derived identities; two cases | The predicate's wiring is checked, not only its existence |
| `Justfile`, `AGENTS.md` | Asserted count 105 → 107 | B23's count assertion stays exact |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The QoS half stops being called | `just test_sel4_root` | `an_incompatible_qos_pair_is_refused_within_every_ceiling` fails |
| The check starts refusing compatible graphs | `just sel4_stream_check` | The real 2×2 stream graph stops admitting |
| The fixture stops declaring an incompatible pair | `just test_sel4_root` | Its own `assert!(!graph.all_pairs_qos_compatible())` guard fires first |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test_sel4_root` | Pass — 107/107, five fabric-graph cases — [`qos-tests.log`](qos-tests.log) | Direct |
| Fault injection: the QoS branch replaced with `Ok(())` | Fails — [`fault-injection-no-qos-check.log`](fault-injection-no-qos-check.log) | Direct |
| `just sel4_stream_check` | Pass — the stream plane's declared QoS tuples are compatible and the graph still admits | Direct |
| The other eight seL4 gates | All pass | Direct |
| `just contracts_check`, `just generation_check`, `just devlog_check` | Pass | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | Pass | Direct |
| An incompatible graph reaching a boot | **Not observed** — `build-generation.py` validates the same property at encode time. Same situation as P5.4.4's, and covered the same way | Unobserved, with reason |

## Decisions

- Decision: **refuse** an incompatible pair at admission rather than report it.
- Rationale: `boot_contracts` deliberately makes this a query, because C8.5's
  design is that a live fabric surfaces an incompatible pair as a structured
  event. This root has no C8.5 plane to surface it on, so the alternative to
  refusing is launching participants that will never match and saying nothing.
  Recorded in the source as a decision that **becomes wrong when P5.4.5 lands**:
  once the QoS plane exists, reporting is the right answer and this moves.
- Rejected alternative: leaving it uncovered until P5.4.5. The gap is small and
  real now; deferring it would have meant P5.4.10's row stayed open on a
  property already available.

- Decision: a real route-and-participant fixture, not an empty graph.
- Rationale: `all_pairs_qos_compatible` over zero pairs is vacuously true, so
  the existing `graph_with` fixture would have passed either way. The new
  builder folds route and grant identities through `route_identity` and
  `grant_identity` because the decoder recomputes and compares them — which is
  itself worth having exercised, since it is what makes an identity name one
  exact `(route, component, direction)` tuple rather than a label.

## Open risks and follow-ups

- [ ] **This inverts when P5.4.5 lands.** A QoS plane that reports incompatible
      pairs as events wants admission to permit them. The source comment says
      so at the call site; the risk is that P5.4.5 adds the plane without
      revisiting this.
- [ ] **P5.4.10 still has seven open rows** — C8.1 collision rejection, C8.3
      graph provenance, C8.4's structural arm, C7.1's retained-v2 arm, B10's
      seL4 layout fixture, B11's product-vs-test pair, and
      `task_reclamation.rs`'s three properties.
- [ ] The two structural assertions this entry found already covered are
      covered *by decode*, which is stronger than the oracle's test-time check
      — but it means no seL4 gate names them. If `validate_interposition` ever
      moves out of `decode`, nothing on this path notices.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`qos-tests.log`](qos-tests.log).
- Serial/debugger/model output:
  [`fault-injection-no-qos-check.log`](fault-injection-no-qos-check.log).
- Related roadmap item:
  [P5.4.10](../../roadmap/07-architecture-portability.md) (one more row closed),
  [P5.4.4](../../roadmap/07-architecture-portability.md) (which named this gap),
  [C8.2](../../roadmap/02-core-runtime.md) (the oracle milestone).
