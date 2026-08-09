# B32 - park scenario receivers on their endpoints

| Field | Value |
|---|---|
| Date | 2026-08-09 |
| Kind | Defect |
| Status | Verified |
| Scope | `components/bins/src/fabric_call_scenario.rs`, `components/bins/src/fabric_operation_scenario.rs` |
| Roadmap | P5.4.6, P5.4.7, B32 |
| Gates | `just sel4_call_check`, `just sel4_operation_check`, `just fmt_check_all`, `just lint_all`, `just devlog_check` |
| Trigger | P5 closure review found receive loops that yielded instead of registering endpoint waits |
| Baseline | The call plane and operation plane passed, but three blocked receives remained runnable and invisible to root waiter diagnostics |

## Summary

Three scenario receive paths returned `ERR_WOULDBLOCK` to `seL4_Yield` rather than registering the endpoint they awaited. That consumed the root graph's iteration budget and made a wedge indistinguishable from useful work. All three now park on their receive endpoint. Parking exposed an operation teardown race, so client B now records its terminal before client A closes the backup route and permits the broker to exit. The call and operation plane gates observe the timeout, peer-death, and unrelated-route paths completing.

## Observable symptom

- Command: source audit of the call and operation scenario receive loops.
- Expected: a blocked receive registers `WaitSource::Endpoint` so the root can suspend and wake it by dependency.
- Observed: the call terminal helper, operation terminal helper, and operation backup-route probe called `yield_now()` on `ERR_WOULDBLOCK`.
- Exit/fault/serial evidence: `just sel4_call_check` and `just sel4_operation_check` pass after conversion, including the gates' timeout and peer-death marker sequences.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `seL4_Yield` does not report a wait source to `slime-root`. | Each empty receive remained runnable and spent graph iterations. |
| 2 | The call plane already passed after its terminal helper was converted to `wait(Endpoint)`. | Parking does not lose the call peer-death wake. |
| 3 | Operation timeout and peer-death settlement publish terminal records through the same route endpoint; peer death also wakes a registered receiver. | The operation terminal helper can use the same endpoint wait for both statuses. |
| 4 | The backup liveness probe receives only from `backup_route`. | Its exact dependency is `WaitSource::Endpoint(backup_route)`. |
| 5 | Parking exposed an operation teardown race: client A could close the backup route, let the broker exit, and make client B see route peer death before consuming its queued terminal. | Client B now emits its settlement marker and signals client A before A closes the backup route. |

## Root cause

The scenarios treated cooperative yielding as a substitute for blocking. On seL4, `yield_now()` only gives another thread a scheduling opportunity; it does not tell the root task what capability event would make the caller productive again. The violated invariant was: every retry loop around an empty receive must either register the receive endpoint or have a separately proved non-endpoint wake source.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Call terminal receive | Retain the existing endpoint wait conversion. | A pending terminal names its route dependency. |
| Operation terminal receive | Replace yielding with `wait(Endpoint(slot))` and rename the helper to describe parking. | Timeout and peer-death terminals suspend until route readiness. |
| Operation backup-route probe | Replace yielding with `wait(Endpoint(backup_route))`. | Unrelated-route liveness no longer burns graph iterations. |
| Operation peer-death teardown | Order client B's terminal marker before client A closes the backup route. | A queued terminal is consumed before broker exit can turn the route into `ERR_PEER_DEAD`. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Peer-death terminal fails to wake a parked call receiver | `just sel4_call_check` | Missing peer-death settlement or terminal marker. |
| Timeout or peer-death terminal fails to wake a parked operation receiver | `just sel4_operation_check` | Missing timeout, peer-death, concurrent-fault, or clean-exit marker. |
| Backup route no longer carries after primary server death | `just sel4_operation_check` | Missing `[fabric-op-client] unrelated operation route live`. |
| Rust or devlog structure regresses | `just fmt_check_all`, `just lint_all`, `just devlog_check` | Non-zero exit. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_call_check` | Passed; bounded-call plane reached clean terminal completion with the parked terminal receive. | Direct |
| `just sel4_operation_check` | Passed; timeout, peer death, concurrent peer fault, backup-route liveness, and clean exits were observed. | Direct |
| `just fmt_check_all` | Passed. | Direct |
| `just lint_all` | Passed. | Direct |
| `just devlog_check` | Passed. | Direct |

## Decisions

- Decision: park on the exact route endpoint rather than add a timer or supervision wait.
- Rationale: the broker delivers every awaited terminal through that endpoint, and the root's channel peer-death path wakes the endpoint waiter before terminal settlement. The client barrier states the separate teardown ordering: both primary-route terminals are consumed before the backup route closes and lets the broker exit.
- Rejected alternative: retain `yield_now()` to mask the teardown race. Yielding preserved the invisible-spin defect and made success scheduler-dependent; the phase barrier names the actual ordering dependency.

## Open risks and follow-ups

- None for this defect. Future empty-receive loops should use `WaitSource::Endpoint` unless their wake source is explicitly different and gated.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: not retained; the named gates are reproducible.
- Serial/debugger/model output: emitted and asserted by the two plane checkers.
- Related roadmap item: [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md), P5.4.6 and P5.4.7.
