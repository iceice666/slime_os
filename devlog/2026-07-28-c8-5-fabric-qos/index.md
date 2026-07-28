# C8.5 — Reliable, retained, and timed QoS

| Field | Value |
|---|---|
| Date | 2026-07-28 |
| Kind | Change |
| Status | Verified |
| Scope | Fabric QoS/time schemas, graph admission, userspace broker, live participants, bootstrap grants, QEMU gate |
| Roadmap | C8.5 |
| Gates | `just fabric_qos_check` |
| Trigger | Implement the next open core-runtime milestone after C8.4 |
| Baseline | C8.4 brokered bounded BEST_EFFORT streams but had no reliable retry, retained replay, or explicit-time QoS transitions |

## Summary

C8.5 adds bounded RELIABLE credit and acknowledgement state, BEST_EFFORT loss without retry state, generation-bounded retained history with a live late-subscriber replay, offered/requested QoS notifications, fixed retry exhaustion, and deadline, lifespan, liveliness, lease, and peer-death events driven by an explicit capability-routed monotonic-time input. The dedicated QEMU gate and the repository validation stack passed.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Contracts | Added versioned Zutai schemas and generated Rust bindings for QoS events and monotonic-time advances. | Every process-boundary record has one versioned schema source of truth. |
| Graph/profile | Validated and generated complete per-participant reliability, durability, liveliness, history, retained, and timing policy. | Runtime policy cannot exceed or drift from generation admission. |
| Broker | Added compatibility matching, bounded delivery credit, elapsed-time retry exhaustion, retained windows, late replay, explicit-time ordering, and distinct terminal/degradation events. | Stalls remain bounded; time and QoS behavior are deterministic and policy-driven. |
| Live scenario | Added a capability-routed simulated-time sequence and subscriber assertions for RELIABLE, BEST_EFFORT, retained replay, expiry, deadline, liveliness, and peer death. | The milestone is proven through real components under QEMU rather than source-shape claims alone. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Reliable delivery retries without elapsed time or exceeds the declared bound | `just fabric_qos_check` | Missing/out-of-order retry markers, retry count outside the fixed bound, or QEMU failure |
| Retained durability is only logged rather than delivered | `just fabric_qos_check` | Late-subscriber offer/replay/expiry chain missing or out of order |
| BEST_EFFORT acquires retry state or an explicit-time transition is lost | `just fabric_qos_check` | Source guard failure, missing bounded-loss marker, time-credit mismatch, or terminal QEMU failure |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just fabric_qos_check` | Passed: contracts and deterministic generation checks were current; the QEMU scenario observed bounded retry exhaustion, late retained replay and expiry, deadline, liveliness, BEST_EFFORT loss, peer death, and healthy vertical-slice completion. | Direct |
| `just test` | Passed: kernel unit and QEMU integration suite. | Direct |
| `just lint` | Passed with warnings denied. | Direct |
| `just lint_components` | Passed with warnings denied. | Direct |
| `just fmt_check` | Passed. | Direct |
| `just fmt_check_components` | Passed. | Direct |

## Decisions

- Decision: drive all timed QoS transitions through one explicit bidirectional time capability with one acknowledged absolute timestamp applied per broker pass.
- Rationale: this preserves a deterministic ingress/ack/time tie order and prevents queued advances from collapsing into a scheduling-dependent final timestamp.
- Rejected alternative: infer time from kernel ticks or drain several time messages into one value; either would introduce ambient authority or lose observable boundaries.
- Decision: prove retained replay with a real endpoint pair and decode/validate the replayed sample before expiring the bounded late-subscriber history.
- Rationale: markers alone do not establish delivery.
- Rejected alternative: pop retained state into a receiverless endpoint and print a replay marker.

## Open risks and follow-ups

- [ ] C8.6 must preserve the same explicit event/time semantics when bounded native calls share the fabric.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none retained; exact commands and observed results are recorded above.
- Serial/debugger/model output: emitted by `just fabric_qos_check` during this session; no separate sibling capture retained.
- Related roadmap item: [C8.5](../../roadmap/02-core-runtime.md#c85--reliable-retained-and-timed-qos).
