# P5.4.5 (part) — C8.5's arms that already ran, now asserted

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Change |
| Status | Verified |
| Scope | `scripts/check/check-sel4-stream-plane.py` |
| Roadmap | P5.4.5, P5.4, P5.4.1, C8.5 |
| Gates | `just sel4_stream_check` |
| Trigger | P5.4.5 opened after P5.4.2's device half proved blocked |
| Baseline | P5.4.1 recorded C8.5 as having no seL4 coverage |

## Summary

P5.4.1 recorded C8.5 as uncovered on seL4. That was half right: no gate
asserted any QoS property, but three of them were already *running*. The QoS
logic lives in `fabric-service`, which the seL4 stream plane boots unmodified,
so matching, bounded loss under a stall, and peer death were being exercised on
every stream-plane boot and checked by nothing. They are now asserted. This
does not close P5.4.5 — the arms that need simulated time and a dedicated
retry/deadline scenario are untouched — but it stops the milestone from later
being credited for behaviour no gate would notice losing.

## Observable symptom

- Command: `qemu-system-aarch64 … -kernel build/slime-sel4-stream.elf | grep QoS`
- Expected (from P5.4.1's inventory): nothing; C8.5 has no seL4 coverage.
- Observed: `[fabric] QoS matched` ×5, `[fabric-subscriber] QoS matched` ×2,
  `[fabric] QoS peer dead` ×2, plus the already-asserted
  `[fabric-subscriber-b] bounded loss reported` ×3.
- Evidence: [`qos-markers.log`](qos-markers.log).

## Changes

| Area | Change | Effect |
|---|---|---|
| `check-sel4-stream-plane.py` | Chain "QoS is matched before any sample moves" — fabric-side match, subscriber-side match, then the first publish | C8.5's *ordering* property: matching precedes data |
| `check-sel4-stream-plane.py` | Chain "peer death is a distinct structured event" | Peer death stays distinguishable from loss or timeout |
| `check-sel4-stream-plane.py` | Three `SEL4_ONLY` entries with reasons | The oracle-drift comparison stays honest |

`SEL4_ONLY` is required because this gate cross-checks every marker it requires
against `check-fabric-stream.py`'s chain list, in both directions. Two of these
are required by the oracle's *QoS* gate (`check-fabric-qos.py:19,136,140`)
rather than its stream gate. The third is not required by any oracle gate at
all, and its entry says so — see the decision below.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Data moves before QoS is matched | `just sel4_stream_check` | `marker out of order` on the matching chain |
| The subscriber stops learning it matched | same | `missing marker: \[fabric-subscriber\] QoS matched` |
| Peer death degrades into a generic failure | same | `missing marker: \[fabric\] QoS peer dead` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_stream_check` | Pass, 4 declared seL4-only markers | Direct |
| Fault injection: `[fabric-subscriber] QoS matched` → `QoS Xatched` | `missing marker` — [`qos-markers.log`](qos-markers.log) | Direct |
| Fault injection: `[fabric] QoS peer dead` → `QoS Xeer dead` | `missing marker` — same | Direct |
| The other nine seL4 gates | Pass | Direct |
| `just ruff` | Pass | Direct |

**A first fault injection proved nothing and is worth recording.** Renaming the
marker to `QoS matchedX` left the gate green, which looked like the assertion
was not binding. It was not a gate defect: `check_transcript` matches with
`re.search`, so `QoS matched` is still a substring of `QoS matchedX`. Mutating
a character *inside* the marker (`QoS Xatched`) is the injection that tests
anything. A suffix mutation is not a mutation for a substring matcher.

## Decisions

- Decision: assert the arms that already run rather than build a QoS plane.
- Rationale: a ninth image with its own fixture is the right shape for the
  arms that need simulated time — deadline, lifespan, liveliness, lease, retry
  exhaustion — because those need a scenario that deliberately stalls and
  advances a clock. The three arms here need no new scenario; they need someone
  to check them. Doing the cheap half first makes the expensive half's scope
  exact rather than assumed.

- Decision: record `[fabric-subscriber] QoS matched` as required by **no**
  oracle gate.
- Rationale: my first draft claimed it came from the oracle's QoS gate, by
  analogy with its siblings. Grepping showed it is emitted by
  `fabric-subscriber` and asserted nowhere on either side. That makes it a
  genuine seL4-only assertion rather than a ported one, and the exception entry
  now says exactly that — the fabric-side marker shows the *fabric* matched,
  and only the subscriber-side one shows the participant was told.

## Open risks and follow-ups

- [ ] **P5.4.5 stays open.** Untouched: bounded RELIABLE credit/acknowledgement,
      fixed retry exhaustion, deadline/lifespan/liveliness/lease transitions
      driven from the monotonic-time capability, equal-timestamp tie ordering,
      and incompatible-QoS events at runtime. C8.5's own required-checks list is
      the scope; three arms of it are now gated.
- [ ] **Incompatible QoS is refused at admission here, not surfaced at
      runtime.** P5.4.10 made `fabric_graph_is_satisfiable` reject an
      incompatible pair, which is the correct behaviour for a root with no QoS
      plane — but it means the runtime *event* C8.5 requires cannot be reached
      on this path until that plane exists. Recorded at the call site in
      `slime-root/src/generation.rs`; when P5.4.5 completes, that decision
      inverts.
- [ ] **The substring-matcher subtlety applies to every marker gate in
      `scripts/check/`.** A future fault injection that appends to a marker will
      silently pass. Not fixed here — anchoring every pattern is a change to
      eleven gates and would be its own slice — but recorded so the next person
      does not conclude a gate is broken.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`qos-markers.log`](qos-markers.log).
- Related roadmap item:
  [P5.4.5](../../roadmap/07-architecture-portability.md),
  [C8.5](../../roadmap/02-core-runtime.md),
  [P5.4.1](../../roadmap/07-architecture-portability.md) (which recorded C8.5 as
  uncovered).

## Corrections

- **2026-08-07.** The third open risk proposed anchoring every marker pattern
  to end-of-line, describing it as "a change to eleven gates" deferred for
  scope. I tried it: `re.compile(f"{pattern}(?=$|\n)", re.MULTILINE)` applied to
  the ordered-marker matcher in all nine gates that have one. **Seven of the
  nine failed**, and the reason is that the suggestion was simply wrong for
  this codebase.

  Many patterns deliberately match a line *prefix* — for example
  `check-sel4-channel-plane.py`'s `SLIME_ROOT generation admitted number=\d+
  components=2 grants=2 ` ends in a space because the line continues with
  fields that gate does not constrain. Anchoring turns every such pattern into
  a false negative. The change was reverted; all nine gates pass as before.

  The underlying weakness is still real and still unfixed: a marker that gains
  a *suffix* is matched by `re.search`, so a fault injection appending to a
  marker will silently pass. But the fix is not a blanket anchor. It would have
  to be per-pattern — anchoring only those that are intended to be whole lines
  — which is a judgement call across roughly five hundred patterns and belongs
  to its own slice with its own evidence. Recorded so the next attempt starts
  from the measurement rather than repeating it.
