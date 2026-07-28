# C8.4 — Bounded many-to-many streams

| Field | Value |
|---|---|
| Date | 2026-07-28 |
| Kind | Change |
| Status | Verified |
| Scope | Zutai fabric-stream contract, `boot-contracts` KEEP_LAST ring, fabric service brokering, two new participant components, a second declared route, generation manifest and bootstrap wiring, C8.4 checks |
| Roadmap | C8.4 |
| Gates | `just fabric_stream_check` |
| Trigger | C8.4 opened after C8.3 left a fabric that provisioned route authority but carried no data over it |
| Baseline | C8.3 minted both halves of one route and handed each participant a narrowed, non-delegable role, then exited after a single provisioning round. Nothing brokered a sample, so KEEP_LAST, BEST_EFFORT loss, and the one-copy fan-out existed only as declared QoS |

## Summary

C8.4 makes the provisioned routes carry data. The fabric service becomes a
long-lived broker: it provisions every declared stream edge, then moves samples
from every publisher to every matched subscriber, bounded by each subscriber's
declared KEEP_LAST depth. A sample within the control-message bound rides inline
in a fixed `StreamSample`; a payload larger than `MAX_MSG` arrives as a C7.6
descriptor over a receiver-bound loan, which the fabric maps read-only, copies
**once** into a fabric-owned sealed buffer, and re-loans per subscriber. One
publisher sample is therefore one copy and N independently accounted loans. A
subscriber that stops acking stops receiving: its ring evicts the oldest
sequence at the declared depth, counts the loss, and is told what it missed —
a report, never a retry.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/fabric-stream/v1/` | Versioned Zutai contract for three fixed 64-byte records: `StreamSample` (inline payload), `StreamAck` (delivery-slot release), `StreamEvent` (`SAMPLE_LOST`/`STREAM_END`) | The wire form of a stream is schema-owned; magics, widths, and offsets all derive from `schema.zt` |
| `boot-contracts/src/stream_history.rs` | Bounded KEEP_LAST ring: fixed capacity, exact-oldest eviction, saturating loss counter plus oldest-lost sequence, `entry_at` for the first unsent sample | Eviction order is a contract with host unit tests, not an emergent property of the broker |
| `components/bins/src/bin/fabric-service.rs` | Provisioning round extended to every declared edge, then an unbounded brokering loop: route-indexed dispatch, one fabric copy per large sample, per-subscriber downstream loans, KEEP_LAST admission, loss and terminal events, reclamation on ack, eviction, peer death, and route teardown | Route policy stays in userspace, and every queue, buffer, mapping, and loan the fabric holds is bounded by generation-declared numbers |
| `components/bins/src/bin/fabric-{publisher,subscriber}.rs` | Updated to the C8.4 framing: inline publication, ack-driven consumption, and the denials each role must still observe | A role is one direction on the route it names, and acking never becomes publish authority |
| `components/bins/src/bin/fabric-{publisher-b,subscriber-b}.rs` | Two new participants: `-b` publishes the `>MAX_MSG` sample and spans two routes; `-b` subscribes BEST_EFFORT, stalls deliberately, and verifies its loss is bounded and its other route undisturbed | Many-to-many is a real fan-in and fan-out, and a stalled participant cannot disturb an unrelated stream |
| `contracts/interface-schema/v1/interfaces/diagnostics-stream.zti`, `contracts/generation/v1/fixtures/valid.zti` | A second stream interface and route, four telemetry participants, per-holder budgets for the fabric and its buffer-owning participants | Two routes sharing a participant remain distinct authority and matching domains |
| `kernel/src/runtime/bootstrap.rs`, `init.rs` | Two components, their control channels, the fabric's own shared-buffer factory grant, capability slots 45–60 (transfer moved to 61/62), and the `SLIME_FABRIC_STREAM_CHECK` scenario | The control-endpoint ↔ component binding and every quota stay generation facts established at spawn |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A large sample is copied per subscriber instead of per sample | `check_one_copy_per_large_sample` in `scripts/check/check-fabric-stream.py` | `[fabric] large sample copied once` appears more than once |
| The fan-out reaches only one subscriber | Same check, counting `[fabric] downstream loan created` | Fewer than one loan per matched subscriber |
| KEEP_LAST evicts the wrong sample, or more than one | `stream_history::keep_last_evicts_the_exact_oldest_sequence`; `fabric_stream::the_declared_depth_evicts_the_oldest_sequence` at the depth the booted graph declares | An eviction returns a sequence other than the oldest |
| A stall grows unboundedly instead of reporting | `stream_history::a_stalled_subscriber_is_bounded_and_reports_its_loss`; `fabric-subscriber-b`'s own report/total ceilings | Retained entries exceed the depth, or reports exceed what the publishers sent |
| A keeping-up subscriber is told it lost a sample | `just fabric_stream_check` → `[fabric-subscriber] fail: keeping-up subscriber was told it lost a sample` | The reliable reader observes `SAMPLE_LOST` |
| One participant's stall disturbs an unrelated route | `just fabric_stream_check` → `[fabric-subscriber-b] diagnostics unaffected by stall` | The marker is absent or out of order in its chain |
| A subscriber gains publish authority by acking | `[fabric-subscriber] route publish denied`, `ack channel is send-only` | Either denial is absent |
| Authority crosses between two stream routes | `fabric_stream::stream_authority_does_not_cross_routes` | A grant identity resolves for an edge the manifest does not declare |
| A malformed record reaches a subscriber | `FORBIDDEN` list in the check, including the whole `[fabric] reject:` surface | Any rejection marker appears on a clean run |
| The frame table is smaller than the rings it backs | `const _: () = assert!` in `fabric-service.rs` | Build failure naming the deadlock it prevents |
| The fabric busy-waits | `check_no_busy_wait_shape` over all six fabric sources | A fabric source contains `yield_now`, or never parks |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just fabric_stream_check` | Passed: the live boot's nine transcript chains observed in order, one copy and two delivered downstream loans counted, plus 8 kernel tests | Direct |
| `just fabric_authority_check` | Passed after updating its markers for the renamed provisioning lines (7 kernel algebra tests plus the live boot) | Direct |
| `just fabric_manifest_check` | Passed after updating the C8.2 shape assertions from 2 routes/4 participants to 3/8 and from one stream route to two (4 QEMU tests) | Direct |
| `just contracts_check` | Passed, including the new fabric-stream schema/renderer and its `--check` binding comparison; 47 `boot-contracts` lib tests | Direct |
| `just generation_check` | Passed: two byte-identical builds carrying the two new components and the second route | Direct |
| `just test`, `just sample_plane_live_check` | Passed; the C7 live plane is unaffected by the slot renumbering | Direct |
| `just fmt_check`, `just lint`, `just fmt_check_components`, `just lint_components`, `just framework_safety_check`, `just devlog_check` | Clean | Direct |
| Defects found and fixed during bring-up: init exhausted the 64-slot capability table; `deliver` re-sent the ring head instead of advancing; an evicted in-flight sample left `in_flight` ratcheted; publisher-b's termination settled its loan before the fabric copied it; the frame table was smaller than the rings it backed and deadlocked | Each reproduced on a live boot and re-verified by the gate | Direct |
| Independent review round (`history://StreamReview`), 13 findings, all applied. Correctness: the copy path mapped the upstream loan at a hard-coded offset while the descriptor admits any page-aligned one; a descriptor arriving with more than one attached capability leaked every slot past the first; a permanently stalled subscriber deadlocked the whole fabric because loss reporting was gated on an empty in-flight window that could never drain. Bounds: `park_on_streams` silently truncated its wait set, and the frame-table assertion compared against a literal instead of the declared depths. Hygiene: a vacuous `expected_loan` binding, endpoint slots leaked on subscriber retirement, and a per-sample credit overloaded onto `STREAM_END` | Each fixed and re-verified; the offset and padding fixes additionally fault-injected (reverting either makes the gate fail) | Direct |
| Gate strength, same review: the loan marker was emitted before `send`, so a transient `WOULDBLOCK` would inflate the fan-out count; the loss check counted a print-once flag and so could not detect unbounded reporting; and the malformed-descriptor arm existed only as a forbidden marker with no positive test | Marker moved to the delivering path, loss counted per event against a declared ceiling, and two negative-corpus kernel tests added (`malformed_descriptors_are_refused_before_mapping`, `malformed_stream_records_are_refused`) | Direct |

## Decisions

- Decision: Give each subscriber a second, opposite-facing endpoint for acks rather than widening its route role.
- Rationale: KEEP_LAST is only meaningful against a bound on undelivered samples, so a subscriber must be able to release a delivery slot — but a `RIGHT_RECV`-only endpoint cannot carry one. Adding `RIGHT_SEND` to the data endpoint would let a subscriber publish on the route it reads, which is exactly the C8.3 denial under test. Two capabilities, one direction each, keep that denial true while making the release expressible.
- Rejected alternative: A bidirectional route role. It would have deleted a proven property to add a new one.
- Decision: Credit publishers over a matching reverse channel after their loan is taken.
- Rationale: A task's termination settles every loan it lent (C7.5), so a publisher that exits between sending its descriptor and the fabric's copy reclaims the region out from under that copy. This was observed, not anticipated: the first live run failed with `ERR_BAD_CAP` on the fabric's `loan_map`. The credit makes the ordering an assertion rather than a race, mirroring the C7.7 lender's settle wait.
- Rejected alternative: Having the fabric copy synchronously inside `recv`. The copy needs two mappings and a buffer allocation; doing it before the publisher can be told anything just moves the window.
- Decision: Move the downstream loan as a plain `send` attachment rather than a narrowed `SYS_CAP_TRANSFER`.
- Rationale: One `cap_transfer` carries exactly one descriptor, so narrowing would split the loan and its sample descriptor across two messages, leaving a window where a subscriber holds a capability it cannot interpret. The attachment therefore crosses at the rights the kernel minted — `RIGHT_BUFFER_MAP | RIGHT_TRANSFER`. Review flagged the retained transfer bit as contradicting the terminal-authority invariant, which is fair as stated: a subscriber *can* pass the loan on. What it cannot do is give anything away by doing so — a `SharedBufferLoan` is receiver-bound, so only the named receiver may map or return it, and a recipient holds a handle that does nothing. The bit is inert rather than absent, and that is weaker than the endpoint roles, which are narrowed outright.
- Rejected alternative: Two messages with a narrowed move. It reintroduces the half-delivered state the C7 descriptor shape exists to avoid. Making the loan genuinely non-delegable needs a kernel-side narrow on `SYS_SEND` attachment, which is a C8.5-or-later change rather than a property this slice can add in userspace.
- Decision: Count loans on the delivering path, not at the receiver.
- Rationale: The stalled BEST_EFFORT subscriber may evict a sample before reading it — that is the declared behaviour, not a fault — so counting verifications would undercount a correct fan-out. Counting at loan *creation* has the opposite flaw, which review caught: a loan created and then revoked because the send would block never reached anyone, so a retry would inflate the count. The marker therefore sits on the success arm of the send: one per loan that actually crossed. The gate separately requires at least one subscriber to verify the payload, or the copy would be unobserved.
- Decision: Put the KEEP_LAST ring in `boot-contracts` rather than in the fabric component.
- Rationale: "Evicts the exact oldest sequence at the declared depth" is a contract the gate asserts, and a transcript cannot show which sample was dropped. In `boot-contracts` it is host-unit-testable without a boot, and `kernel/tests/fabric_stream.rs` then runs it at the depth a real generation declared.

## Open risks and follow-ups

- [ ] The fabric brokers streams only. Call and operation routes are declared in the same graph and skipped at runtime, which the participant table and `declared_edges` both scope by route. C8.6/C8.7 own them; until then a component declared only on a call route holds no fabric authority at all, which is correct but untested as a denial.
- [ ] `MAX_FRAMES` is asserted against the summed depths this generation declares, not against the contract's own `LIMIT_HISTORY_DEPTH` ceiling. A future manifest declaring eight subscribers at depth 64 would satisfy C8.2 admission and still outgrow the table — it fails the build rather than a boot, and at runtime the fabric refuses a sample and settles its loan rather than deadlocking, but a declared subscriber would stop receiving. Sizing the table from the resource is C8.5 work, since it also needs the retained-sample budget.
- [ ] Loss reporting is per-drain rather than per-stall: a stall spanning several admissions produces several reports, bounded by what the publishers sent. Both the component and the gate check that bound rather than a count of one. A single coalesced report per stall needs the timed QoS C8.5 introduces.
- [ ] The `>MAX_MSG` path is exercised with one two-page sample at a one-page offset. Larger payloads, multiple concurrent large samples, and a subscriber dying mid-map are unproven; the last is the reclamation arm C8.9 composes.
- [ ] Visibility and interposition remain declared and unread; C8.8 owns filtered introspection and the declared proxy chain.
- [ ] A subscriber that stays alive but never acks again holds its route open: `announce_end` waits for an empty ring, so the broker parks on that subscriber's ack channel indefinitely. A *dead* subscriber is retired on `ERR_PEER_DEAD` and a *slow* one is bounded by KEEP_LAST, so this is narrowly the silent-but-live case. Closing it needs a liveness lease, which is C8.5's `lease_ns` — declared in the graph today and unread.
- [ ] `park_on_streams` now fails closed rather than truncating, but the bound it enforces is one `SYS_WAIT` set across publishers *and* subscribers, while C8.2 admission only bounds `ingressSources`. A graph within its declared limits can therefore still fail at provisioning time. Admitting the full wait demand, or introducing bounded route workers, is the C8.5 decision the architecture notes already flag.

## Artifacts and provenance

- Focused report: none.
- Raw transcript: none.
- Serial/debugger/model output: none captured; the live arm's transcript is printed by `just fabric_stream_check`.
- Related roadmap item: [`C8.4`](../../roadmap/02-core-runtime.md#c84--bounded-many-to-many-streams).
