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
