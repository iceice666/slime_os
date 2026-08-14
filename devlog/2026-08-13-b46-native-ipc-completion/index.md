# B46 — all seven fabric planes run on native seL4 IPC

| Field | Value |
|---|---|
| Date | 2026-08-13 |
| Kind | Change |
| Status | Verified |
| Scope | `components/bins/src/{operation_broker.rs,fabric_operation_scenario.rs}`, `components/bins/src/bin/{fabric-service.rs,fabric-op-worker.rs}`, `components/proto/src/{fabric_operation.rs,lib.rs}`, `contracts/fabric-operation/v1/{schema.zt,gen_rust.zt}`, `slime-root/src/main.rs`, `scripts/check/check-sel4-operation-plane.py`, `roadmap/00-backlog.md` |
| Roadmap | B46 |
| Gates | `just sel4_channel_check`, `just sel4_crossing_check`, `just sel4_stream_check`, `just sel4_qos_check`, `just sel4_call_check`, `just sel4_operation_check`, `just sel4_visibility_check`, `just lint_all`, `just fmt_check_all`, `just devlog_check` |
| Trigger | The operation plane was the last B46 gate not completing after the logical IPC cutover. |
| Baseline | Six native gates passed; `sel4_operation_check` could block when a broker sent a new request while the single-threaded server still owed its explicit idle transition. |

## Summary

B46's last operation-plane blocker is removed. The operation broker now uses the same two native mechanisms established by the call plane: one badged Notification for multi-source wakeup and receiver-confirmed retirement for mandatory endpoint deliveries. It also serializes requests sent to the single-threaded operation server until the server emits a versioned `KIND_SERVER_IDLE` fence. The plane reaches all 53 required markers across 15 causal chains, and all six participants plus init exit cleanly.

The completion closes the cutover as a whole: the deleted logical channel, transit, parked-reply, and wait-set implementations have no compatibility fallback, and all seven B46 gates pass on native Endpoint, Reply, Notification, and shared-ring paths.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Operation contract | Added `KIND_SERVER_IDLE` to the Zutai schema, generated binding, and validator. | Readiness crossing a process boundary is an explicit versioned wire fact, not inferred from an empty poll. |
| Operation broker | Added the multi-source wake, receiver acknowledgement, dead-client reclamation, and one outstanding server request guarded by the idle fence. | A multiplexer never blocks on one peer, never guesses delivery from `seL4_NBSend`, and never sends into a server that is still sending its fence. |
| Operation scenario | Signals before endpoint sends, acknowledges mandatory records, and makes the server fence every handled goal or cancellation. | Wakeup precedes rendezvous, retirement comes from the receiver, and the server's readiness transition is observable. |
| Generation/spawn path | Converted operation control and phase edges to ordinary grants and retained only factories and supervision as minted bindings; root spawn evidence reports the grant kinds separately. | The generation owns declared connectivity and the gate proves the actual authority handed to every task. |
| Gate | Split scheduler-independent causal chains from concurrent branches and assert the native spawn/exit evidence. | The gate proves behavior without imposing an accidental total serial order on independently runnable tasks. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Broker sends while the server still owes a fence | `just sel4_operation_check` | A server-bound request stalls or a required post-fence chain is absent. |
| Mandatory operation outcome is retired without receiver evidence | `just sel4_operation_check` | Backpressure/retry chains do not complete or a terminal acknowledgement is missing. |
| Multi-source wake loses a participant | `just sel4_operation_check` | Declared notification bindings or one of the client/server/time causal chains is absent. |
| Native IPC cutover regresses another plane | The seven B46 plane gates | Any channel, transfer, ring, QoS, call, operation, or visibility assertion fails. |
| Contract source and generated binding drift | `python3 scripts/generate/generate-fabric-operation-bindings.py --check` | Generated protocol binding differs from the Zutai source. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Seven B46 gates in order | Pass; operation gate observed 53 markers across 15 causal chains, six spawned tasks, and clean init exit | Direct |
| `python3 scripts/generate/generate-fabric-operation-bindings.py --check` | Pass | Direct |
| Fresh reviewer pass | Correct; one P3 cleanup finding applied (`drop_dead_client` now clears the client route before reclamation) | Direct |
| `just lint_all` | Pass | Direct |
| `just fmt_check_all` | Pass | Direct |
| `just devlog_check` | Pass; 145 entries indexed | Direct |
| `just sel4_operation_check` after reviewer cleanup | Pass; 53 markers across 15 causal chains, six spawned tasks and init exited cleanly | Direct |

## Decisions

- Decision: server readiness is `KIND_SERVER_IDLE`, emitted after every handled goal or cancellation and consumed before the next server-bound request.
- Rationale: an empty non-blocking receive says only that no sender was already blocked at that instant. It cannot prove a single-threaded server has completed the prior request or is ready to receive another blocking send.
- Rejected alternative: infer readiness from `server_request` becoming logically terminal. Logical operation lifetime and the server thread's IPC state are different obligations; conflating them reproduced the deadlock.

- Decision: retain concurrent logical operations while serializing only the server-bound request stream.
- Rationale: the server is single-threaded, but timeouts, cancellation, feedback, retained results, and client delivery remain independently active. Serializing the whole broker would delete the concurrency the plane exists to prove.
- Rejected alternative: make every operation globally sequential, which would hide rather than fix the peer-state distinction.

- Decision: evaluate the operation gate as causal chains plus lifecycle evidence rather than one total marker sequence.
- Rationale: the participants are independent seL4 tasks. Ordering within a protocol chain is required; ordering between unrelated chains is scheduler-dependent and not a correctness property.
- Rejected alternative: preserve a single total regex order, which failed valid runs when independent tasks interleaved differently.

## Open risks and follow-ups

- [ ] B50 still owns the broader removal of logical capability and universal syscall compatibility residue; B46 proves the IPC mechanisms and named behavior gates only.
- [ ] A native Endpoint does not report peer death from `send`; every remaining liveness obligation must continue to use supervision rather than an `ERR_PEER_DEAD` send arm.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: the ordered seven-gate run was captured by the OMP command artifact for this session; no frozen raw log was added.
- Reviewer: `FocusedOperationReview`; verdict `correct`, one P3 consistency finding applied before final checks.
- Related roadmap item: `roadmap/00-backlog.md` B46.
