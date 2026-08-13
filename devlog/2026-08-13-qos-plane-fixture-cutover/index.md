# The QoS plane's fixture cutover, and three dead counters behind it

| Field | Value |
|---|---|
| Date | 2026-08-13 |
| Kind | Change |
| Status | Monitoring |
| Scope | `contracts/generation/v1/fixtures/sel4-qos.zti`, `components/bins/src/bin/fabric-service.rs`, `components/bins/src/bin/fabric-subscriber-b.rs`, `components/bins/src/bin/fabric-publisher-b.rs` |
| Roadmap | B46, B50 |
| Gates | `just sel4_qos_check`, `just sel4_stream_check`, `just sel4_visibility_check`, `just sel4_channel_check`, `just sel4_crossing_check`, `just sel4_root_boot_check` |
| Trigger | `sel4_qos_check` failing at `spawn refused … ungranted`, which B50/R2 showed was a fixture shape rather than a slot-allocation problem |
| Baseline | The QoS plane stalling at 40 component markers, with no participant finishing |

## Summary

`sel4-qos.zti` still declared control capabilities in the pre-cutover
`mintedBindings` shape that B46 replaced everywhere else. Converting it to
ordinary grants let the plane admit, boot, and run its whole graph — 40 component
markers to 79, with every participant but one finishing. The conversion then
exposed three genuine defects in `fabric-service`, none of them QoS-specific:
a counter that was never incremented, a QoS condition that reported nothing when
it was reached the hard way, and required events sent by a primitive that
discards. All three are fixed. One assertion still fails, recorded below rather
than worked around.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `sel4-qos.zti` probe | Three `fabric-publisher-probe-*` minted carriers became one ordinary `fabric-publisher-probe` grant, matching `sel4-stream.zti` | The root materializes both halves of a declared endpoint; init holds no route capability and mints nothing |
| `sel4-qos.zti` clock | `fabric-publisher-b-clock` became a grant between the publisher and the broker | It is an endpoint pair between two declared instances, which is exactly what a grant is |
| `sel4-qos.zti` supervision | Deleted `fabric-publisher-b-fabric-supervision`; added the four ring-holder handles the plane was missing | `fabric-publisher-b` names the fabric by its control endpoint for the upstream loan and holds no supervision handle — its own source says so. B46 requires a handle per *ring holder*, and this plane declared only the two subscribers |
| `sel4-qos.zti` quotas | `fabric-publisher` had no shared-buffer entry at all; `fabric-publisher-b`'s was too narrow | A v2 ring is a mapping the participant must be allowed to hold; without a quota its own ring is refused |
| `sel4-qos.zti` priority | Removed `priority = 100` on `fabric-intruder` | Under blocking IPC a low-priority task that must speak before the broker can proceed simply starves, and the broker waits on it forever |
| `fabric-service.rs::deliver` / `drain_acks` | Count a RELIABLE delivery out; clear the balance on the subscriber's credit signal | `in_flight` means what its doc says. Only decrements existed, so it could not leave zero, and every rule reading it was unreachable code rather than an unmet condition |
| `fabric-service.rs` retry exhaustion | Report it whether or not a frame survived the retries | An earlier lifespan expiry drains the queue, and gating on a surviving frame made the condition invisible exactly when it was reached the hard way |
| `fabric-service.rs::send_qos_event` | Retain undelivered records and re-offer each broker pass; hold the terminal event back while any is outstanding | These are not advisory: the plane's contract is that the subscriber observes each declared condition. Both race into one endpoint, so an unheld end retires the route with the condition never delivered |
| `fabric-service.rs::TIME_SLOT` | Derived from `FIRST_CONTROL_SLOT + clients + supervision` instead of a hardcoded 9 | Supervision slots are themselves derived, so a constant beside them is a number racing a computed range — adding a ring participant moved supervision onto it (B50/R2 clause 3) |
| `fabric-publisher-b.rs` | Block on the credit endpoint after publishing rather than polling | The fabric is blocked *sending* that credit; two non-blocking peers never rendezvous |
| `fabric-subscriber-b.rs` | The per-route mailbox became a small FIFO that ignores an identical record already waiting | A route's records are a sequence its owner must see in full, but the fabric re-offers each until taken — so queueing every copy overflows on repetition rather than on traffic |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| `in_flight` accounting wedges a plane that never acks | `just sel4_stream_check` | `graph iterations exhausted` — observed and fixed during this entry |
| The mailbox overflows on re-offered records | `just sel4_stream_check` | `a route produced more records than its mailbox admits` — observed and fixed during this entry |
| A derived slot collides with a hardcoded one | `just sel4_qos_check` | `generation rejected: BadBinding` at admission, before boot |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_qos_check` | **Still red**, but from 40 component markers to 79. The graph admits, all seven tasks spawn, every edge provisions, simulated time advances, and `fabric-publisher`, `fabric-publisher-b`, `fabric-subscriber` all finish | Direct |
| `just sel4_stream_check` | PASS — regressed twice during this entry (`in_flight` gating, mailbox overflow) and green after both fixes | Direct |
| `just sel4_visibility_check`, `just sel4_channel_check`, `just sel4_crossing_check`, `just sel4_root_boot_check` | PASS | Direct |
| `just test_sel4_root` | 118/118 across 13 modules | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff` | Clean | Direct |

## Decisions

- Decision: delete `fabric-publisher-b-fabric-supervision` rather than have init
  mint one.
- Rationale: the component does not use it. `fabric-publisher-b` names the
  fabric by `FABRIC_SLOT = 0`, its control endpoint, and its own comment
  explains why a supervision handle cannot work there — the fabric loans it a
  ring in the other direction, so neither can precede the other. Supplying a
  capability nothing reads to satisfy a count is how the declaration drifted
  from the code in the first place.

- Decision: report retry exhaustion with sequence zero when the queue is empty,
  rather than skipping the event.
- Rationale: the event is a statement about the retries. Skipping it means the
  one path that reaches exhaustion *and* expiry reports the least.

- Rejected alternative: raising the QoS events with blocking `send`. That is the
  deadlock `try_send` was introduced to avoid — the broker would stop on a
  subscriber that has moved on to its ring. Retain-and-re-offer keeps the
  non-blocking send and adds the delivery guarantee on top.

## Open risks and follow-ups

- [ ] **`fabric-subscriber-b` observes liveliness but not deadline or retry
      exhaustion.** Both are raised while it is still blocked in
      `receive_large_sample` on its *other* route. The demultiplexer files them
      correctly by `type_identity`, but the re-offer loop and the per-route
      mailbox interact badly: identical re-offers dedupe to one entry, so three
      distinct liveliness events and one repeated event are indistinguishable to
      the reader. The dedupe is required (without it the mailbox overflows on
      repetition) and so is the re-offer (without it the record is dropped), so
      the fix is to make a record identifiable — a sequence or generation on the
      wire — rather than to remove either. Component-side, not fixture-side.
- [ ] `sel4-call.zti` and `sel4-operation.zti` are unconverted: between them
      about thirty minted bindings across nine holders, each needing to be
      checked against what the component expects in that slot.
- [ ] The QoS plane's `fabric-intruder` priority was removed rather than made to
      work at a low priority. If declared priorities are meant to be meaningful
      under blocking IPC, that is a separate question this entry does not answer.

## Artifacts and provenance

- Focused report: this entry
- Related roadmap item: `roadmap/00-backlog.md` B46 (open), B50 (open)
- Preceding entries: `devlog/2026-08-13-r2-declared-slot-allocation/` — the
  clause that made the count visible and showed this was a fixture shape;
  `devlog/2026-08-12-b46-arena-slot-occupancy/` — the same `nb_send` rendezvous
  hazard, first found on the stream plane
