# C8.13 — `historyDepth` was wrongly grouped as unconsumed; `queueDepth` and `capabilitySlots` genuinely are

| Field | Value |
|---|---|
| Date | 2026-08-16 |
| Kind | Audit |
| Status | Root-caused |
| Scope | `boot-contracts/src/fabric_graph.rs`, `components/bins/src/bin/fabric-service.rs`, `components/proto/src/stream_history.rs`, `roadmap/02-core-runtime.md`, `devlog/2026-08-16-c8-13-saturation-ceilings/index.md` |
| Roadmap | C8.13 |
| Gates | none |
| Trigger | Investigating what closing C8.13's "queueDepth, historyDepth graph-wide, capabilitySlots are never checked against real usage" gap would take |
| Baseline | `devlog/2026-08-16-c8-13-saturation-ceilings/index.md` and the roadmap (as of this morning's edit) stated all three fields have no runtime consumer at all |

## Summary

Re-tracing the claim that `queueDepth`, graph-wide `historyDepth`, and
`capabilitySlots` are equally unconsumed found it is wrong for one of the
three. `FabricGraph::validate_against` (`boot-contracts/src/fabric_graph.rs:
710-712`) rejects any participant whose declared per-route
`qos.history_depth` exceeds the graph-wide `limits.history_depth` — a real
decode-time cross-check `queueDepth` and `capabilitySlots` genuinely lack
(`TransportQos` has no `queue_depth` field to cross-check at all, and no
per-participant field is ever compared against `limits.capability_slots`
anywhere in the file). The value that check admits then sizes real runtime
state (`StreamHistory::new(qos.history_depth)` in `fabric-service.rs`,
capped by `stream_history.rs`'s `MAX_HISTORY = LIMIT_HISTORY_DEPTH`), and its
live occupancy is exactly what `resourceHistory` — added the same day —
already evidences. `historyDepth` is corrected out of the "declared but
unconsumed" bucket; `queueDepth` and `capabilitySlots` remain in it,
confirmed by the same direct reading. The prior devlog entry's body is
frozen, so the correction is recorded there under `## Corrections` rather
than by editing its original claim, and the roadmap's own copy of the
sentence is corrected directly since it is live project state, not a frozen
record.

## Observable symptom

- Command: none; this is a source-reading correction, not a runtime finding.
- Expected: the roadmap's inherited claim, that decode never compares
  `historyDepth` against anything the graph itself declares.
- Observed: it does, at `fabric_graph.rs:710-712`.
- Evidence: source reading, cited above and in the Investigation log.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `boot-contracts/src/fabric_graph.rs:341-369` (`validate_declared_limits`) compares every `limits` field, including `queue_depth`, `history_depth`, and `capability_slots`, only against a fixed global `LIMIT_*` structural ceiling. | Matches the prior devlog's premise for all three fields at this layer alone. |
| 2 | `boot-contracts/src/fabric_graph.rs:651-722` (`validate_against`) additionally loops every participant and checks `entry.qos.retained_depth > limits.retained_samples \|\| entry.qos.history_depth > limits.history_depth` (line 710-712). | A second, independent cross-check exists for `history_depth` that the first layer alone does not show — the prior devlog's grep evidently stopped at `validate_declared_limits` and did not reach this loop. |
| 3 | `grep` for `.queue_depth`/`.capability_slots` across the repository turns up only: the two structural-ceiling comparisons (`validate_declared_limits`, `validate_against`'s fixed-kernel-constant comparisons at lines 664/669), encode/decode plumbing, and tests. No per-participant or per-route field is ever compared against either. | `queueDepth` and `capabilitySlots` are confirmed to have no cross-check of any kind — the correction is scoped to `historyDepth` alone. |
| 4 | `boot-contracts/src/fabric_graph.rs:106-117` (`TransportQos`) has `history_depth` and `retained_depth` fields but no `queue_depth` field to compare against `limits.queue_depth` in the first place — the absence is structural, not an oversight in `validate_against`. | Rules out a parallel check for `queueDepth` existing under a different field name. |
| 5 | `fabric-service.rs:1436` (`StreamHistory::new(qos.history_depth as usize)`) and `stream_history.rs:25-30` (`MAX_HISTORY = LIMIT_HISTORY_DEPTH`) show the admitted, graph-bounded per-participant `history_depth` sizes the real ring the stream worker holds. | The graph-wide `historyDepth` ceiling is not just checked at decode — the value it bounds drives real runtime state, which `resourceHistory` (this same day's queue/history-evidence pass) already reports live occupancy for. |

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `devlog/2026-08-16-c8-13-saturation-ceilings/index.md` | `## Corrections` section appended (frozen body untouched) naming the `historyDepth` error and its evidence | The record states what was actually established, without rewriting the original frozen claim |
| `roadmap/02-core-runtime.md` | C8.13's gap paragraph corrected to drop `historyDepth` from the "declared, never checked" list, keeping only `queueDepth` and `capabilitySlots` | The roadmap's live text matches what direct code reading actually supports |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Source trace: `fabric_graph.rs:651-722`, `:106-117`, `fabric-service.rs:1436`, `stream_history.rs:25-30` | `history_depth` has a real per-participant cross-check and sizes real runtime state; `queue_depth`/`capability_slots` have neither | Direct (source reading) |
| `just devlog_check` | Pass, 158 entries indexed | Direct |
| `just typos` | Pass | Direct |
| No runtime code changed; no gate exercising fabric-graph admission was run | n/a | n/a |

## Open risks and follow-ups

- [ ] `queueDepth` and `capabilitySlots` remain genuinely unconsumed. Per the roadmap's existing framing, closing this needs an explicit decision to either wire each to a real check (for `queueDepth`, there is no obvious per-participant field to bound the way `history_depth` is bounded, since `resourceQueue`'s occupancy is already governed by ring/history capacity and QoS retry policy per `contracts/fabric-trace/v1/schema.zt:71-73`; for `capabilitySlots`, C8.13.3 would first need to exist to have anything real to check against) or delete both fields from the schema, the builder, every `sel4-*.zti` fixture, and the generated bindings. Neither attempted; a real schema-editing decision, not a fixture change.
- [ ] No other `limits` field was re-audited beyond the three already named by the prior devlog entry; this pass does not claim the remaining sixteen are all correctly characterized, only that these three were re-verified.

## Artifacts and provenance

- Focused report: none; the investigation is summarized above and in the *Investigation log*.
- Raw transcript: none captured separately.
- Serial/debugger/model output: none generated by this pass.
- Related roadmap item: [C8.13](../../roadmap/02-core-runtime.md#c813--concurrent-cross-plane-traffic-and-resource-ceilings).
</content>
