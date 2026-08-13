# B46 — the two mechanisms rendezvous IPC actually needs

| Field | Value |
|---|---|
| Date | 2026-08-13 |
| Kind | Change |
| Status | Monitoring |
| Scope | `contracts/fabric-call/v1/{schema.zt,gen_rust.zt}`, `contracts/generation/v1/fixtures/sel4-call.zti`, `scripts/build/build-generation.py`, `boot-contracts/src/generation.rs`, `components/proto/src/{fabric_call.rs,lib.rs}`, `components/bins/src/{call_broker.rs,fabric_call_scenario.rs}`, four call components |
| Roadmap | B46 |
| Gates | `just sel4_call_check`, `just sel4_stream_check`, `just sel4_qos_check`, `just contracts_check` |
| Trigger | `sel4_call_check` stalling in client B's backpressure burst with the broker holding queued terminals |
| Baseline | Call plane reaching 14 component markers |

## Summary

Two mechanisms were missing from the native-IPC cutover, and the call plane
could not work without either. A broker that multiplexes peers cannot wait on an
Endpoint, so it needs a **Notification every peer is badged into**. A broker
offering a message with `seL4_NBSend` cannot observe delivery, so it needs
**receiver-confirmed retirement**. Both landed; the plane went from 14 component
markers to 57, and now fails inside the backpressure burst rather than at
admission.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `sel4-call.zti` | One notification grant, four signallers, one waiter | A broker can wait on its whole peer set |
| `build-generation.py`, `boot-contracts` | One waiter, ≥1 signaller, source among them | A notification grant is not forced to be a pair |
| `build-generation.py` | Wake constants emitted for every profile, absent as `u32::MAX` | A missing constant is not a build failure standing in for a boot-time absence |
| `fabric-call/v1` | `KIND_TERMINAL_ACK`, with validator | Delivery is confirmed by the receiver, on the wire |
| `call_broker.rs` | Retire on ack, matched by request id | An ack cannot drop a terminal the client has not seen |
| `call_broker.rs` | Acks settle before the session guard | An ack for a stale-session terminal cannot feed back |
| `call_broker.rs` | Never park on the wake while a terminal is owed | A held terminal cannot deadlock against the client waiting for it |
| `fabric_call_scenario.rs` | Burst chunked four × six | A client that never reads can never be answered |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A grant regains the one-signaller rule | `just contracts_check` | `requires source signal and target wait bindings` |
| The wake constant is absent on some profile | `just lint_all` | `cannot find value FABRIC_*_READY_SLOT` |
| A terminal is retired undelivered | `just sel4_call_check` | marker count falls |
| Notification changes disturb the ring planes | `just sel4_stream_check`, `just sel4_qos_check` | plane failure marker |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_call_check` | FAIL — 57 markers, was 14 | Direct |
| `sel4_channel/crossing/stream/qos/visibility_check` | PASS | Direct |
| `just contracts_check`, `just test_sel4_root`, `just test_host` | PASS | Direct |
| `just lint_all`, `fmt_check_all`, `ruff`, `typos`, `devlog_check` | PASS | Direct |
| Notification installed | `notification binding task=1..5 slot=64..67`, one `role=Wait` | Direct |

## Decisions

- Decision: relax the one-signaller rule rather than declare one notification
  per peer.
- Rationale: four objects means four waits, which is the problem restated. One
  object with distinct badges is what the kernel offers for "any of these".
- Rejected alternative: a grant per peer — builds, but does not answer the
  question.

- Decision: `KIND_TERMINAL_ACK` in the contract.
- Rationale: it is a wire fact, and every serialized format crossing a process
  boundary is contract-defined here. Echoing the status and matching on the id
  makes a wrong ack unable to retire the wrong record.
- Rejected alternative: inferring retirement. Tried twice, and both *lost*
  ground — 25 markers to 23 keyed on the next request, to 19 keyed on an
  `offered` flag. Guessing at delivery has now failed three times in this
  cutover.

- Decision: do not order terminal offers by request id.
- Rationale: measured, and it gives no gain alone while regressing with a
  `terminal mismatch`. The combination that reached 60 markers needs its own
  evidence before it is worth the complexity.

## Open risks and follow-ups

- [ ] `sel4_call_check` fails inside the backpressure burst. The remaining
      question is whether a client can be answered while it holds requests the
      broker has refused, which the chunking only partly settles.
- [ ] `sel4-operation.zti` is unconverted and needs the same two mechanisms;
      its broker is a separate implementation of the same shape.
- [ ] The wake is call-plane-specific. If the operation broker needs one too,
      the constant naming should generalise rather than gain a second special
      case in `build-generation.py`.

## Artifacts and provenance

- Focused report: this entry
- Raw transcript: reproduce with `just sel4_call_check`
- Serial/debugger/model output: quoted inline above
- Related roadmap item: `roadmap/00-backlog.md` B46

## Corrections

**"Starved, not deadlocked" was wrong.** The plane emitted 138
`terminal delivery queued` markers against the root's shared graph-iteration
budget with only 14 broker passes inside it, which read as starvation. Two
things followed from that reading and only one held up.

The marker traffic was real and is fixed: `pump_terminal` announced every
re-offer, and each `debug_write` is a root round trip, so a diagnostic was
spending the budget on a condition that had not changed. It is one per record
now, on an `offered` flag — 138 down to 52. Worth having regardless: a
diagnostic must not be able to starve the thing it describes.

The diagnosis itself was refuted by raising `MAX_GRAPH_ITERATIONS` from 32,768
to 262,144. Eight times the budget, and the plane stops at exactly the same
point with exactly 57 markers. It is a deadlock. Reverted, since a constant that
changes nothing is not evidence.

Two further candidates were measured and reverted for the same reason: polling
`supervision_status` only while the server owes an answer (a root round trip on
every pass, so it looked like a drain — no change), and ordering terminal offers
by request id, which is a real correctness property and was kept, but moved
nothing by itself.

What did move: `retire_terminal` returned after its first match, so a request id
recorded twice kept one copy forever — and a client never acks an id it has
already passed, so that copy stayed the queue's minimum and blocked everything
behind it. Instrumenting the minimum afterwards showed the mechanism working,
advancing 5, 10, 6, 7, 8, 9 as acks arrived, before stopping with client A
waiting on a terminal the broker still holds. That is where the next
investigation starts, and it is a deadlock rather than a budget.

**The deadlock is isolated, and it points back at `seL4_Call`.**
Instrumenting the offer target shows the machinery working: ids 6, 7, 8, 9
offered to client A in sequence, each advancing as its ack arrives, and the
broker yielding 19 times without ever parking — so it re-offers continuously.
Client A takes 6, 7, 8, then stops.

The state it stops in is **client A blocked sending the ack for 8 while the
broker is mid-sweep offering 9**. Neither is receiving. Both available shapes
were measured:

| Ack shape | Result |
|---|---|
| `try_send` | 57 markers → **19**; the ack is what retires a record, and `seL4_NBSend` reports nothing, so dropped acks leave the broker re-offering terminals already taken |
| blocking `send` | current state; deadlocks against a broker mid-sweep |

This is the same "two peers, opposite directions, one endpoint" shape the whole
cutover kept producing, and it says the ack needs a path that is neither lossy
nor blocking. That is `seL4_Call` — request and reply as one atomic operation,
with a reply capability naming *this* caller — which is what B46's own fix text
asks for and what the runtime still does not have. The primitive was written
earlier this session and reverted as unwired; this is the call site that would
wire it.

A latent instance of the same hazard was fixed on the way:
`expect_terminal_parked` polled with `yield_now` against a broker offering with
`seL4_NBSend`. Client A never reaches that step, so it moved nothing — but it is
the fourth polling receive found in this one scenario file.

**`Call`/`ReplyRecv` now exists and is wired.** `slime_rt::call` over
`seL4_Call`, `slime_rt::reply` over `seL4_Reply`, with the terminal ack as the
call site. It lands now rather than when it was first written and reverted,
because this is the one place both alternatives are provably wrong rather than
merely suspect — and unwired code is not deliverable.

Marker count holds at 57: a deadlocking ack is replaced by a sound one, not a
step forward. Client A still stops after taking terminal 8, which now means it
waits in `Call` for the broker's reply. The broker answers from its client sweep
and reaches that sweep every pass, so the next thing to check is whether the
reply capability survives: both `recv` and `nb_recv` pass `()` as the reply
authority, which under non-MCS is the thread's implicit reply capability. That
assumption has not been verified against a `nb_recv` that took the message.
