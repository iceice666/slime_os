# The cutover's real defect class: code written against `ERR_WOULDBLOCK`

| Field | Value |
|---|---|
| Date | 2026-08-13 |
| Kind | Defect |
| Status | Fixed |
| Scope | `components/bins/src/bin/fabric-service.rs`, `components/bins/src/call_broker.rs`, `components/bins/src/fabric_call_scenario.rs`, nine call/operation components, `components/runtime/src/syscall/sel4_transport.rs`, `contracts/generation/v1/fixtures/sel4-call.zti`, four `scripts/check/check-sel4-*-plane.py` |
| Roadmap | B46 |
| Gates | `just sel4_qos_check`, `just sel4_stream_check`, `just sel4_visibility_check`, `just sel4_channel_check`, `just sel4_crossing_check` |
| Trigger | `just sel4_qos_check` failing `diagnostics ended before every QoS condition` |
| Baseline | Three of seven B46 gates green (channel, crossing, visibility) at `e02a232` |

## Summary

`just sel4_qos_check` failed one assertion, and the cause was not the
demultiplexer the backlog predicted. `send_qos_event` used `try_send`, whose
`seL4_NBSend` reports nothing either way — so the runtime returns `ERR_SUCCESS`
for "attempted" and the retain-and-re-offer machinery built on its
`ERR_WOULDBLOCK` arm **could never execute**. Every QoS event raised while the
subscriber was busy was silently dropped. Fixing it exposed the same shape five
more times across the call plane: every one of them code written against
`ERR_WOULDBLOCK` semantics that native seL4 IPC does not produce. Five of the
seven named B46 gates are green; the call plane runs most of its scenario and
still stalls at cancellation.

## Observable symptom

- Command: `just sel4_qos_check`
- Expected: `fabric-subscriber-b` observes deadline, liveliness, and retry
  exhaustion on its diagnostics route before the terminal event.
- Observed: only liveliness observed; `[fabric-subscriber-b] fail: diagnostics
  ended before every QoS condition`, init exits 1.
- Exit/fault/serial evidence: the broker printed `[fabric] QoS deadline missed`
  with no corresponding arrival at the subscriber.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Instrumented both sides with the event id, target slot, and route tag | `qosdbg send ev=6 slot=6 tag=2845549244` emitted, no matching `rxdbg` |
| 2 | The record never reached the subscriber at all | Not a demultiplexing or dedupe fault — a delivery fault |
| 3 | Read `sel4_transport::try_send` | `nb_send` returns unconditional `ERR_SUCCESS`; the `ERR_WOULDBLOCK` arm is unreachable |
| 4 | Switched QoS events to a blocking `send` | All four conditions observed; plane then hung with `live=2` |
| 5 | `time_dead` is set only by `ERR_PEER_DEAD` | A native Endpoint cannot produce it, so the broker waited forever for an exited clock |
| 6 | Applied the same fix to the call plane's fixture and drivers | Five further instances of the same class surfaced (see Changes) |
| 7 | Traced the residual call stall stage by stage | Blocks inside `pump_server`'s **non-blocking** receive, which by construction cannot block |

## Root cause

`seL4_NBSend` transfers only to a receiver already blocked on the endpoint,
discards otherwise, and **reports nothing either way**. `slime_rt::try_send`
therefore returns `ERR_SUCCESS` meaning "attempted", not "delivered". Every
`ERR_WOULDBLOCK` arm written against it is unreachable code, and every caller
that treats `ERR_SUCCESS` as delivery retires a record that was discarded.

The mirror-image invariant governs `send`: it blocks until a receiver arrives.
A component that multiplexes several peers can therefore never use it safely on
a peer that might itself be sending — that is a deadlock, not backpressure.

The violated invariant is the same in both directions: **the cutover replaced a
root-mediated queue, which could report "would block", with kernel rendezvous,
which cannot.** Code inherited from the queue model kept asking a question the
new transport does not answer.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `fabric-service.rs` | QoS events take a blocking `send` guarded by the peer's supervision handle; retain machinery deleted | A declared QoS condition is delivered or the peer is gone |
| `fabric-service.rs` | Clock liveness derived from `fabric-publisher-b`'s supervision handle | The broker terminates when its time source exits |
| Nine call/op components | Park on `fabric_boot::active()`, not `startup_arg == 0` | A participant parks only on the plane that gives it no work |
| `sel4-call.zti` | Control edges and phase barriers are ordinary grants; `mintedBindings` carries only what init creates | Init holds no route capability and mints nothing |
| `call_broker.rs` | `server_idle` gates forward and cancellation; deferred work waits in `Phase::Forwarding` | The broker never blocks on a peer that may be blocked on it |
| `call_broker.rs`, `fabric_call_scenario.rs` | A delegated loan is claimed with `capability_import`, never read from `caps[0]` | Only a native Endpoint travels inline |
| `fabric_call_scenario.rs` | Reply and server receives block instead of polling | Two peers with nothing else to do rendezvous |
| `sel4_transport.rs` | `receive_native`'s capability path clears `RECEIVE_SLOT_LIVE` before returning | The receive guard is released on every exit |
| Four check scripts | `SPAWN_PATTERN` matches `endpoints=`/`notifications=` | A gate that cannot match cannot silently pass |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A QoS condition is raised but not delivered | `just sel4_qos_check` | `diagnostics ended before every QoS condition` |
| The broker outlives its clock | `just sel4_qos_check` | `graph iterations exhausted` |
| A participant parks instead of taking its role | `just sel4_call_check` | `boot idle without a role` |
| A gate's spawn pattern rots against the marker | `just sel4_gate_control_check` | negative control fails to fail |
| Ring, loan, and proxy paths regress | `just sel4_stream_check`, `just sel4_visibility_check` | plane-specific failure marker |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_qos_check` | PASS — `14 markers observed across 9 causal chains` | Direct |
| `just sel4_stream_check` | PASS | Direct |
| `just sel4_visibility_check` | PASS | Direct |
| `just sel4_channel_check` | PASS | Direct |
| `just sel4_crossing_check` | PASS | Direct |
| `just sel4_call_check` | FAIL — stalls at cancellation | Direct |
| `just sel4_operation_check` | not run — fixture unconverted | — |
| `just test_sel4_root` | PASS | Direct |
| `just lint_all`, `just fmt_check_all` | PASS | Direct |

## Decisions

- Decision: QoS events, call replies, and terminals take a blocking `send`;
  only genuinely advisory traffic keeps `try_send`.
- Rationale: `try_send` cannot report delivery, so any protocol obligation built
  on it silently degrades to best-effort. Blocking is safe exactly where the
  receiver has nothing else to wait on.
- Rejected alternative: a non-blocking send that reports delivery. `seL4_NBSend`
  leaves the sender's registers untouched whether or not it transfers, so there
  is nothing to observe; a sentinel scheme was written and reverted.

- Decision: peer death is observed through a supervision handle, everywhere.
- Rationale: this is now the third distinct place the cutover has needed it —
  publishers, interposition proxies, and the clock. A native Endpoint carries
  messages; it does not carry death.
- Rejected alternative: synthesising `ERR_PEER_DEAD` in the transport. The
  kernel does not know a silent peer from an exited one.

- Decision: a broker serialises work against a single-threaded peer rather than
  blocking on it.
- Rationale: `Phase::Forwarding` and its retry path already existed for exactly
  this; the deadlock came from bypassing them, not from their absence.
- Rejected alternative: giving the server a second thread. That changes the
  plane's shape to work around a broker bug.

## Open risks and follow-ups

- [ ] `just sel4_call_check` stalls inside `pump_server`'s non-blocking receive,
      which cannot block by construction — the fault is in the receive path, not
      the broker. Next investigation starts there.
- [ ] `sel4-operation.zti` is unconverted: 23 minted bindings across six
      holders, same shape as the call fixture.
- [ ] `try_send` remains a hazard by construction. Every surviving use must be
      paired with a blocked receiver or a supervision-handle fallback; there is
      no gate that proves this.

## Artifacts and provenance

- Focused report: this entry
- Raw transcript: not retained; reproduce with `just sel4_qos_check` and
  `just sel4_call_check`
- Serial/debugger/model output: quoted inline above
- Related roadmap item: `roadmap/00-backlog.md` B46

## Corrections

**The residual call-plane stall was not "inside a non-blocking receive that
cannot block".** That reading of the stage trace was wrong: the broker was not
blocked in `pump_server`, it was *looping past* it. `seL4_NBRecv` takes a
message only from a sender already blocked on the endpoint, so a polling broker
and a blocking server never rendezvous — the trace showed the last loop
iteration before the plane went quiet, not a syscall that failed to return.

The fix follows the same rule this entry states: when nothing has progressed
and a call is outstanding, the server's answer is the only event that can move
the plane, so the broker waits for it in the kernel. Everything else stays
polled. `pump_server` and `block_on_server` share one record handler so the two
disciplines cannot drift. The phase barriers had the same shape.

With that, the call plane runs correlated replies, rejection, duplicate
suppression, shared payloads both ways, cancellation, stale-session refusal, and
malformed-reply detection. It stalls next at the client-B handshake. Three
explanations are refuted by observation rather than argument: both clients reach
the barrier and block; the generation installs the edge symmetrically
(`fabric-call-client` slot 4 / CSpace 37, `fabric-call-client-b` slot 1 / CSpace
34, both `send`+`recv`); and a blocking `send` cannot be self-consumed, since it
returns only once a receiver has taken it. Guard contention in `receive_native`
was tested directly — making a blocking receive refuse instead of reporting
`ERR_WOULDBLOCK` did not move the stall — and reverted as unverified.

**The client-B handshake was not the stall either.** Instrumenting both sides
showed `signal_client_b` *returning* — a blocking `send` completes only once a
receiver takes it, so client B had the message. The plane stalls one step later,
in B's 24-request backpressure burst.

That exposes the limit of the fix above. The broker waits on the server when
nothing else has progressed, and the justification given — "a client's request
would be visible to the preceding non-blocking sweep" — is false. A client that
blocks in `send` *after* the sweep has passed it is invisible until the next
sweep, and a broker already parked on the server never runs one. The burst lands
in exactly that window.

A single Endpoint cannot express "wake me when any of these speak". That is the
reason `graph::Resource` gained a Notification variant during this cutover, and
what `fabric-service`'s stream side already uses. The call and operation brokers
need every peer badged into one Notification they wait on — a design change, not
another blocking-semantics fix, and the honest remaining scope of those two
gates.
