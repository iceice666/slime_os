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

**Two more hypotheses refuted.** Instrumenting the ack shows 34 round trips
completing and the 34th hanging: client A takes all four timeout terminals, acks
three, and blocks in `Call` on the fourth.

| Hypothesis | Test | Result |
|---|---|---|
| The broker parks with nothing owed, so the ack finds no receiver | Signal the wake before the ack; then remove the park entirely, replacing `notification_wait` with `yield_now` | 57 markers both times — the park is not the blocker |
| `seL4_Reply` answers the wrong caller | Kernel headers: it uses "the reply capability stored when the thread was last called", and the broker receives on five endpoints per sweep | The reply is issued with no intervening receive, so the stored capability is still the ack's |

Established: the ack reaches the broker — it retires records and the queue's
minimum advances — the reply path is structurally sound, and the broker is
neither parked nor starved. The one asymmetry not yet tested in isolation is a
`seL4_Reply` issued after a `nb_recv` rather than a blocking `recv`.

That suspect is refuted too: making the broker's client receive blocking —
`recv_blocking` in place of `nb_recv`, so the reply follows a blocking receive —
drops the plane from 57 markers to **3**. A multiplexer that blocks on one
client cannot serve the others, which is the lesson the rest of this cutover
taught. `seL4_Reply` after a non-blocking receive is not the fault.

Every hypothesis raised is now measured and refuted: the iteration budget, offer
ordering, supervision polling, the notification park, reply authority, and the
receive discipline under it. The ack demonstrably reaches the broker and
demonstrably retires records; the 34th `Call` does not return.

What has *not* been instrumented is the broker's side of that exchange —
whether `pump_client` observes the 34th ack at all, as against observing it and
failing to answer. One marker inside the ack branch separates those, and it is
the next thing to place. Recording it rather than guessing again: six
hypotheses have now been paid for with a boot cycle each, and the cheap
observation was never made.

**The observation was made, and it refuted my own framing.** A marker inside
the broker's ack branch shows it seeing all 34 acks; a marker after the client's
`Call` shows all 34 *returning*. The "34th hangs" reading was an artefact of the
last marker being the last line before the root's fatal. The ack path —
delivery, handling, reply, retirement — is sound end to end, and client A moves
past its timeout loop.

One real ordering defect was found there and fixed: the branch retired the
record before replying, so `retire_terminal` and anything it logs sat between
the receive and the answer. `seL4_Reply` uses the capability "stored when the
thread was last called" and `debug_write` is a root round trip, so an
intervening log would consume it. Correct regardless of reachability; not the
fault.

What remains is arithmetic rather than mechanism: 52 terminals queued, 34
acked, **18 never taken**. The budget is not the constraint — 262,144 iterations
stop at the same 57 markers, retested against this state. So a terminal client A
needs is offered to a reader that has moved on, or queued under a client index
the reader does not match. That is answerable directly from the queue-minimum
instrumentation, and no further hypothesis should precede it: seven have now
been paid for with a boot cycle each.

**Three more measurements, and the loop is exonerated.**

| Hypothesis | Test | Result |
|---|---|---|
| The broker is starved of loop passes | Spin counters at 5, 10, 12, 14, 16, 20, 25 | All fire in the **full** transcript; they looked absent only because the gate truncates its tail. The broker loops freely |
| Terminals are owed to an exited client | Instrument the queue's provenance | Every pending record belongs to client 0, still running; a `reclaim_dead_clients` guard never fired and was reverted |
| `inFlightCalls = 4` is too small | Raise to 8 | *Loses* the retry-exhaustion arm — that bound is what makes request 10 exceed the limit. Load-bearing, not incidental |

The queue is now known exactly: client 0 holds terminals 4–9 in `calls` and 10
in the overflow queue; client 1 holds 100–123. Client A takes 4, 5, and 10, then
waits for 6 while the broker offers 6 — same id, same endpoint, matching
sessions (`client_session` is the identical expression on both sides) — and
neither advances.

Worth recording about method: two of these were refuted by *reading the full
transcript instead of the gate's*. The gate truncates its tail, and three
separate conclusions this session were distorted by that — "34th ack hangs",
"broker never reaches 500 spins", "broker never reaches 50 spins". Capturing
the QEMU output directly costs 90 seconds and would have saved several boot
cycles each time.

**The blocker is `server_idle`, and the obvious repair makes it worse.**
Client A's timeout arm sends ids 6–9 with payloads 106–109, which
`handle_inline` deliberately answers with `None` — the server is *supposed*
never to reply. But `server_idle` is cleared by forwarding and set only when the
server sends something back, so a request it never answers leaves the flag false
forever and every later forward is deferred behind it. The transcript confirms
it: four forwards in the whole run, the last being id 10.

Releasing the server when a call times out loses two markers — 57 to 55,
reproducibly across paired runs. So the deferral is load-bearing elsewhere, and
the real fix must separate "the server owes nothing on *this call*" from "the
server is free". One boolean cannot express both, because it conflates a
per-call obligation with a per-peer state.

**A measurement hazard, recorded because it nearly corrupted the above.** A
`git checkout` followed immediately by a gate run reported 19 markers where
three consecutive clean runs report 57 — a stale build, not variance. Combined
with the truncated-tail trap above, that is two distinct ways this gate can lie
about a change. Every comparison here is now taken from at least two consecutive
runs, and every transcript claim from the full QEMU capture rather than the
gate's tail.

**The blocker is fixed, and the metric that hid it was wrong.** Releasing the
server when *its own* call times out advances the plane to
`[fabric-call-server] injected peer death` — the peer-death arm, several stages
past where it had been stopping — with three forwards instead of one.

That change was rejected twice before, because raw marker count *falls* from 57
to 55. Comparing distinct marker **sets** instead of totals shows why: the
missing lines are repeated `terminal delivery queued` and `stale call rejected`
from a broker spinning on work it could not progress. **A count that rewards
spinning is the wrong measure**, and two reverts this session were made on it.

`server_idle` became `server_call: Option<u64>` so the distinction is
expressible: a boolean cannot tell a timeout on the call the server holds, which
releases it, from a timeout on any other, which does not.

Ten distinct stages now, stopping with the server exited and no
`call peer death propagated`. Checking the supervision handle before forwarding
— on the theory that the broker was blocked in `send` to a dead server — changes
nothing, so it is blocked elsewhere; that guard was reverted rather than kept
unearned.

Three measurement traps are now known for this gate, all of which produced a
wrong conclusion this session: the transcript tail is truncated; a run
immediately after `git checkout` can report a stale build; and marker totals
conflate progress with spinning. Comparisons need paired runs, full QEMU
captures, and set-difference rather than counts.
