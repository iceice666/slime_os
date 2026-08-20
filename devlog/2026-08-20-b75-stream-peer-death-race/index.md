# B75: three planes asserted a peer-death property that only a race produced

| Field | Value |
|---|---|
| Date | 2026-08-20 |
| Kind | Defect |
| Status | Monitoring |
| Scope | `components/bins/src/bin/fabric-service.rs` publisher supervision sweep, `components/bins/src/bin/fabric-publisher.rs`, `components/bins/build.rs`, `scripts/build/build-{sel4,generation}.py`, `scripts/check/check-sel4-{fabric-aggregate,fault-plane,qos-plane,stream-plane}.py` |
| Roadmap | B75, B74, C8.5, C8.14, C8.15 |
| Gates | `just sel4_fabric_aggregate_check`, `just sel4_fault_check`, `just sel4_stream_check`, `just sel4_qos_check` |
| Trigger | B74's second signature: a byte-identical trace comparison diverging on the fault plane's stream route under host load |
| Baseline | `1bc21a6` closed B75's call-broker wedge; the divergence half was left open by [`devlog/2026-08-20-b74-aggregate-flake/`](../2026-08-20-b74-aggregate-flake/index.md) |

## Summary

The fabric broker concluded a publisher's death from supervision alone, which
races that publisher's own final write: a peer that publishes `FLAG_LAST` and
returns is *already terminated* while its terminal sample is still queued in the
ring. Whichever observation the sweep reached first decided whether the route
ended orderly or was reported dead — so one composition, booted twice, could
produce different traces. The sweep now latches the termination and concludes
death only from a drain that ran after the latch, which makes the outcome
independent of the race. Three planes turned out to depend on the losing side of
it: the fault plane's stream `EXPECTED_FAULTS` entry, and the stream and QoS
planes' `[fabric] QoS peer dead` markers, were satisfied *only* by the bug. Each
now scripts a real publisher death instead. Two prose claims in gate and build
sources were wrong and are corrected here, one of them attributing the marker to
a subscriber path that has never existed.

## Observable symptom

- Command: `just sel4_fabric_aggregate_check`
- Expected: two boots of one declared composition emit byte-identical semantic traces.
- Observed, under 24 spinners on 18 cores: the fault schedule's stream route
  differed between boots — one boot carried a `kind=fault order=peer-death` record
  for the telemetry route, the other did not.
- Exit/fault/serial evidence: no fault, no panic. The differing record is
  `kind=fault order=peer-death now=50 route=fade05446b2c7013 … event=8`; the
  boot without it ended the same route orderly instead.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | The sweep at `fabric-service.rs:1133` concluded death from `supervision_status` alone, with no reference to ring state | Termination and drain are independent observations, and only one was consulted |
| 2 | `fabric-publisher` publishes its terminal `FLAG_LAST` sample and returns immediately | The peer is terminated while its last sample is still queued: the two observations are simultaneous by construction, not merely adjacent |
| 3 | `call_broker.rs` and `operation_broker.rs` already answer this exact race — `retire_server` and `receive_time` classify from an *empty input*, not from termination | The pattern was established in two sibling brokers; the stream sweep alone had not adopted it |
| 4 | A plain `drained` flag is unsound: a ring can read `Empty` *before* the peer's final write | Emptiness must be invalidated by a later termination observation, so the flag has to be ordered against the latch rather than merely conjoined with it |
| 5 | `subscriber.ended` is assigned at exactly one site, `announce_end` (`:1969`), driven by `route_finished` (`:1988`) over `finished` — which the death path still sets | Deferring the death conclusion cannot wedge the exit predicate. Checked before editing, since the fix delays a terminal transition |
| 6 | After the fix the aggregate failed: traffic emitted 139 records against the shared `EXPECTED_TRACE_RECORDS = 140` | The old constant was satisfied on the traffic plane *only* by the race. Measured independently: traffic 139, fault 140 — the planes genuinely differ by one record |
| 7 | The fault plane's stream record moved from `now=50` to `now=100` | It is now emitted after the drain rather than at the race, which is the intended ordering |
| 8 | `just sel4_stream_check` and `just sel4_qos_check` then failed on `missing marker: [fabric] QoS peer dead` | Two more planes depended on the same race, and neither is covered by the aggregate — the passing aggregate had not exercised them |
| 9 | The stream plane's own transcript shows it completing cleanly, both subscribers `done`, with no participant death at all | Its "peer death is a distinct structured event" chain was asserting a scheduling accident |
| 10 | `grep -n "QoS peer dead" components/bins/src/bin/fabric-service.rs` returns exactly one line, `:1175`, inside the **publisher** sweep | The QoS gate's chain label, "a departed subscriber is retired through the peer-dead path", is misattributed: no subscriber path has ever emitted this marker |

## Root cause

The publisher supervision sweep classified a route's end from one observation
(`supervision_status`) when the condition it was testing requires two. A
publisher that ends its stream correctly and one that dies mid-stream are
*both* terminated; what distinguishes them is whether a `FLAG_LAST` sample is
still in the ring. Reading only termination made the classification a function
of scheduling — under host load the sweep reached the terminated peer before
the pump drained its ring, and the same route that ended orderly on one boot
was reported dead on the next.

The invariant violated is C8.15's: one declared composition produces one
semantic trace. The secondary damage is that three planes had been *passing* on
the wrong side of the race, so the property each named — "peer death is a
distinct structured event" — was never actually exercised by any of them.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `fabric-service.rs` `Publisher` | Added `terminated` and `drained`. Termination is latched, not acted on; the latch clears `drained`, discarding any emptiness seen before it | Death is concluded only from a drain that ran after the termination observation |
| `fabric-service.rs` `pump_publisher` | Records whether the pass consumed the ring to `Empty`. Frame exhaustion breaks with samples queued and is deliberately not a drain | A deferred conclusion, never a lost or a false one |
| `fabric-publisher.rs` | `STREAM_EARLY_EXIT`, an `option_env!` compile-time flag, skips the terminal publish alone — the occupancy report stays reachable so `PARTICIPANT_MAPPINGS["publisher"] = 1` still holds | A scripted death, on the interposition hop's own precedent, with no ambient switch and no product image carrying it |
| `build.rs`, `build-generation.py`, `build-sel4.py` | Propagate the flag; `STREAM_DEATH_VARIANTS` sets it for the `stream`, `qos`, and `fault` variants | Every plane asserting peer death scripts one |
| `check-sel4-fabric-aggregate.py` | `EXPECTED_TRACE_RECORDS` split per plane (traffic 139, fault 140), with an import-time guard that the keys are `PLANES` labels | A plane rename fails at import with its reason, not at the assertion looking like a broker regression |
| `check-sel4-fault-plane.py` | `EXPECTED_FAULTS` docstring rewritten; it claimed the call and operation servers are scripted and the stream broker observes its clock peer leave — **both false** | The gate's stated rationale matches what the plane does |
| `check-sel4-qos-plane.py` | Chain relabelled from "a departed subscriber" to "a departed publisher" | The chain names the participant that actually departs |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The sweep concludes death from termination alone again | `just sel4_fabric_aggregate_check` | A traffic-plane boot emits 140 records against the expected 139, or the two boots diverge on a `peer-death` row |
| A plane asserts peer death without scripting one | `just sel4_stream_check`, `just sel4_qos_check`, `just sel4_fault_check` | `missing marker: [fabric] QoS peer dead` — which is exactly how this defect's remaining reach was found |
| A plane rename silently drops its record count | `just sel4_fabric_aggregate_check` | Import-time `SystemExit` naming the mismatched key set |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_fabric_aggregate_check` | Pass: "2 schedules over one declared composition each passed their own plane gate on two independent boots and produced 279 byte-identical semantic-trace records in total" (139 + 140 across four boots) | Direct |
| `just sel4_fault_check` | Pass: "3 peer-death faults observed, with 8 isolation markers intact" | Direct |
| `just sel4_stream_check` | Pass: 57 markers across 14 causal chains, with the death now scripted | Direct |
| `just sel4_qos_check` | Pass: 14 markers across 9 causal chains, all six participants exited cleanly | Direct |
| `just sel4_traffic_check` | Pass | Direct |
| `just sel4_saturation_check` | Pass: 19 participants, ceilings met, no route worker deadlocked | Direct |
| `just sel4_matrix_check`, `just sel4_visibility_check` | Pass | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | Clean | Direct |

## Decisions

- Decision: latch the termination, then conclude death only from a drain that follows it.
- Rationale: it makes the outcome independent of which observation wins. A queued
  `FLAG_LAST` is always consumed before the ring reads `Empty`, so an orderly exit
  always sets `finished` first and is skipped, while a mid-stream death always drains
  to `Empty` without ever setting it and is always reported.
- Rejected alternative: a plain `drained` flag conjoined with termination. Unsound —
  a ring can read `Empty` before the peer's final write, so an emptiness observed
  earlier would authorise a death that never happened.
- Decision: script a genuine publisher death on the stream, QoS, and fault planes.
- Rationale: an orderly `FLAG_LAST` and an observed mid-stream death are mutually
  exclusive, so a plane asserting peer death as a distinct structured event has to
  script one. Leaving the marker to a race meant the assertion passed without the
  property ever holding.
- Rejected alternative: relaxing the stream and QoS chains to tolerate the marker's
  absence. That would retire a C8.5 property rather than verify it, and would leave
  both planes asserting something nothing on them exercises.
- Decision: split `EXPECTED_TRACE_RECORDS` per plane rather than re-blessing one number.
- Rationale: the planes emit different record counts, and measurement shows the shared
  constant held on traffic only because of the defect.

## Open risks and follow-ups

- [ ] No mutation-backed regression guard for the sweep itself. The broker is not
      host-testable: `components/bins/tests/` does not exist and the module is not in
      `components/bins/src/lib.rs`. The gates above catch the behaviour end-to-end,
      but nothing fails fast on the ordering in isolation.
- [ ] B74's fourth signature is untouched by this fix: `G-gate-24/run1.log`'s
      call-plane `kind=call order=data now=0 correlation=4 sequence=5` against
      `sequence=4`. It is a different route family and a different mechanism.
- [ ] `supervision_slot_for` memoizes per component (`fabric-service.rs:2469-2475`)
      while `Publisher` is per route (`:889`), so a component publishing on two routes
      shares one supervision slot. Not reached by any current fixture; latent.
- [ ] `.died` remains vestigial beyond the `:1102` skip guard.
- [ ] `pump_publisher` cannot set `drained` while zero frames are free, so a death
      conclusion is deferred through frame exhaustion. Benign — `release_frame` runs as
      subscribers drain and KEEP_LAST evicts a stalled one, and `just
      sel4_saturation_check` exercises exactly that at tightened ceilings — but it is a
      liveness dependency the sweep now has and did not have before.

## Artifacts and provenance

- Focused report: this entry; the measured record counts and the moved `now=` stamps
  are quoted inline above.
- Raw transcript: the load campaign's per-run logs under `$HOME/.b75/M-b75-fix/`
  are outside the repository and are not reproduced here. Its per-run verdicts and
  the three divergence signatures it produced are quoted under `## Corrections`.
- Serial/debugger/model output: the three post-fix fault-plane peer-death rows are
  quoted in the investigation log.
- Related roadmap item: [B75](../../roadmap/00-backlog.md), and
  [`devlog/2026-08-20-b74-aggregate-flake/`](../2026-08-20-b74-aggregate-flake/index.md),
  which left this half open.

## Corrections

*Appended 2026-08-20, after the load campaign this entry's Verification section had
not yet observed when its body was written.*

This entry was first written with `Status | Fixed`. That was premature: B75's exit
condition names `just sel4_fabric_aggregate_check` passing **ten consecutive runs**
at 24 spinners on 18 cores without an `-icount` pin, and the campaign had not
finished. It has now run, and the result is **6/10**. `Status` is corrected to
`Monitoring` and B75 stays open.

Observed, `$HOME/.b75/M-b75-fix/`, 24 spinners live throughout, 5-minute load average
27.98 rising to 30.44:

| Run | Result | Diverging record |
|---|---|---|
| 1 | Fail | stream `kind=resource` seq 4, `high_water=2` vs `1` |
| 2 | Pass | — |
| 3 | Fail | stream `kind=fault order=peer-death` seq 0, `now=100` vs `now=50` |
| 4 | Fail | stream `kind=resource` seq 4, `high_water=2` vs `1` |
| 5–9 | Pass | — |
| 10 | Fail | call `kind=call order=ack` seq 9, `correlation=22` vs `4` |

Three distinct signatures, and the honest reading is that **none of them is the
peer-death race this entry fixed**:

- Runs 1 and 4 diverge on a *resource* record's `high_water`, which samples the peak
  frames held live at once. That is a scheduling-dependent occupancy sample, not a
  termination ordering.
- Run 3 diverges on the fault plane's peer-death record's **timestamp**, not on its
  presence. The scripted death is now emitted on both boots — which is what this
  entry's change was for, and it holds — but *when* it lands still moves between
  boots.
- Run 10 is B74's fourth signature, already recorded as untouched in the open-risks
  list above: a call-plane correlation id differing at equal sequence.

So the fix stands on what it claimed — the marker's presence no longer depends on
which observation wins, and the fault plane's stream record count is stable at 140 —
but it was not sufficient for B75's exit condition, and this entry should not have
implied it was before the evidence existed. The remaining divergences are a separate
class: values sampled from scheduling state (`high_water`, `now`, `correlation`)
rather than from control flow. They need their own root-cause pass.

The `## Verification` table above stays as written; it records gates that did pass,
each observed directly. Nothing in it is withdrawn.

### The `pump_time` audit, concluded

The open-risks list left a `pump_time` audit unconcluded. Concluding it here, since
its finding is adjacent to this entry's defect and was found the same way.

`IpcError::PeerDead` is declared at `slime-root/src/ipc.rs:91` and mapped to
`ERR_PEER_DEAD` at `:129`, but a repo-wide grep for `IpcError::PeerDead` returns
**only those two lines** — nothing constructs it. A native seL4 Endpoint carries no
closed-peer signal, so `slime_rt::recv` cannot return `ERR_PEER_DEAD` on any path,
and every receive-side arm branching on it is unreachable. That includes
`call_broker.rs:1415-1417` and `operation_broker.rs:1236-1238`, both of which set
`time_closed = true` there, and `fabric-service.rs:2287-2289`.

The clocks do still close: `call_broker` reaches `time_closed` through
`observe_server_death` (`:1496-1512`), and `fabric-service`'s `receive_time`
consults the supervision handle from its `ERR_WOULDBLOCK` arm — with a comment
already stating the rule outright, *"A native Endpoint has no `ERR_PEER_DEAD`: an
exited clock is indistinguishable from a silent one"*. So the planes pass, and this
is latent, not active. `operation_broker`'s `pump_time` has no supervision
consultation at all, which leaves its clock retirement resting on one unaudited
path.

Recorded as [B76](../../roadmap/00-backlog.md). Worth stating plainly why it belongs
next to this entry: an unreachable arm that *looks* like working redundancy is the
same misreading that produced the defect above — a source comment describing
intended behaviour, taken for an observation of actual behaviour.

### The `pump_time` audit, corrected

*Appended 2026-08-20. The subsection above stays as written; it is wrong in two
places and this records how, rather than editing the record.*

Re-reading the three clock receivers against the generation fixtures — instead of
against their own doc comments — inverts two of that audit's three conclusions.

**`call_broker` is worse than recorded, not "latent".** The audit above says its
clock "does still close" through `observe_server_death`. That call reads
`supervision[2]`, which `fabric-call-worker.rs:43` fixes as
`SERVER_SUPERVISION_SLOT` — the **server's** handle. `sel4-call.zti` declares
`fabric-call-time` as a component in its own right, with its own executable
(`:410-412`), and declares exactly three supervision bindings — client, client-b,
server (`:451,462,473`) — **none for the clock.** So `time_closed`, which the exit
predicate at `:307` waits on and which gates the sole trace flush, is set by the
wrong component's death. A clock exiting while the server lives is unobserved and
the worker never flushes. Two comments in the file assert otherwise and disagree
with each other besides: `:711-718` calls server and clock "separate declared
instances" (true) while naming a clock-endpoint `ERR_PEER_DEAD` as a closing
mechanism (impossible), and `:1504-1505` claims "the server's task hosts this
plane's clock" (false).

**`operation_broker` is not "resting on one unaudited path"** — it has no path at
all, and needs none. `time_closed` there is absent from `finished()` (`:465-473`):
it is set at `:1237` and read only by `pump_time`'s own early return at `:1229`, an
inert self-gate that termination never consults. The real gap is a different one:
`supervision: [u32; CLIENTS + 1]`, `CLIENTS = 2` (`:63,:192`), covers two clients
and the server, so the separately-declared `fabric-op-time`
(`sel4-operation.zti:49-50,230-237`) has **no supervision handle**. A dead op clock
stops deadline sweeps (`:1259-1285`) and retention sweeps (`:1286-1306`) silently.

**`fabric-service` is confirmed as recorded**, and is the only correct one:
the `fabric-publisher-b-clock` edge is declared only on `sel4-qos.zti:286` and
`sel4-traffic.zti:693`, the two planes whose `qos_check()` gates on it, and both
also carry `fabric-publisher-b-supervision` (`:561`, `:1547`), so its
`ERR_WOULDBLOCK` fallback resolves a real handle wherever the clock exists. All three clock components are `health = "optional"`, so admission
catches none of this.

The scope figure was also understated: roughly 40 unreachable arms across 14 files,
not three. Most are `fail(...)` aborts and harmless while unreachable; the arms that
matter are those setting state an exit predicate waits on.

Both errors have the same cause as the defect this entry is about, one level up: I
read `retire_server`'s and `observe_server_death`'s doc comments as descriptions of
what the code does. A source comment is a claim. The fixtures were the observation,
and they were one grep away. [B76](../../roadmap/00-backlog.md) carries the
corrected findings and exit condition.

### The residual divergences, root-caused

The three signatures left open above were treated as one class — "values sampled
from scheduling state". Reading the code rather than the signatures, **two** of them
are root-caused here, to two distinct mechanisms, and **neither is the trace log's
stamping**. The third is B74's, and this pass does not explain it: routing a
signature to another item's ledger records where it belongs, not why it happens.

One of the two candidate mechanisms this entry recorded was cited wrongly:
`fabric-service.rs:2296` is `fail(b"time decode")`, not a `retry_count` race.
`retry_count` lives at `:2360-2385`. The other citation,
`fabric_trace_log.rs:233`, is correct but is not the cause of any signature here.

**`high_water=2` vs `1` (runs 1, 4) — a poll-rate sample, not a race.**
`peak_frames` is a running maximum over a value read once per dispatch-loop
iteration: `live_frames` at `fabric-service.rs:1190` counts frames with `refs > 0`
*at that instant*, after both `pump_publisher` (`:1071-1079`) and `deliver`
(`:1080-1086`) have run for every peer. The stream plane declares two publishers
(`sel4-stream.zti:21,28`). If both publishers' samples are admitted within one
iteration, the sample sees 2; if the loop turns between them and `release_frame`
(`:2051`) drops the first to zero refs, it sees 1. Both are truthful readings of a
real instant — the run's true concurrent peak genuinely differs between boots. This
is not a defect in the sample; it is a counter whose value is a function of host
scheduling being asserted as a fixed constant. The same shape applies to
`peak_buffers`, `peak_queue`, `peak_history`, and `peak_retries` (`:1194-1225`) —
runs 1 and 4 happened to land on `RESOURCE_FRAMES`.

**`now=100` vs `now=50` (run 3) — the same mechanism, one level up.** A record's
timestamp is `self.now_ns` (`fabric_trace_log.rs:234`), written only by `advance`
(`:135`). So a record's stamp is the clock instant the loop had reached when the
record was emitted, and the peer-death conclusion is deliberately deferred until an
empty ring is observed after termination is latched (`fabric-service.rs:1116-1130`).
That deferral is what makes the record's *presence* deterministic — the fix this
entry made, which holds — but it explicitly does not fix *which instant* the drain
completes in. Whether the empty read lands before or after the next `advance` is
host-scheduling-dependent, so the stamp moves by one clock tick.

**`correlation=22` vs `4` (run 10)** is B74's fourth signature on the call plane
and is unrelated to either of the above; it stays where it is recorded, and stays
un-root-caused there. Nothing in this pass narrowed it.

The consequence for B75's exit condition: two of the three are **not fixable by
ordering** the way the peer-death race was, because the diverging value is a
faithful observation of a genuinely varying quantity. The choice is between asserting
these fields at all and asserting them as constants, and that is a gate-side decision
about what the trace comparison treats as semantic, not a broker-side bug to fix.
Recording it here rather than acting on it: changing what the comparison ignores is
a change to the meaning of every plane's determinism claim, and it needs its own
decision entry.

No runtime tests were run for this pass; it is a reading of the code against
divergence signatures already recorded above.

### The shared-supervision-slot suspicion, refuted

*Appended 2026-08-20. The open-risks checkbox above stays as written; it is wrong in
three places and this records how.*

That item reads: `supervision_slot_for` memoizes per component while `Publisher` is
per route, so a component publishing on two routes shares one supervision slot —
"not reached by any current fixture; latent." Both of its citations are wrong and its
conclusion is wrong.

The citations first. `fabric-service.rs:2469-2475` is a doc comment about blocking
sends; `supervision_slot_for` is at `:2523`. And `:889` is `DIRECTION_PUBLISH => {`,
the match-arm head — the `Publisher` is constructed at `:890-901`, with the slot
assigned on `:901`.

The mechanism is real and, contrary to the checkbox, **is** exercised:
`fabric-publisher-b` appears as a participant on both routes of every stream plane
(`sel4-stream.zti:118,152`; `ROUTE_NAMES` is `["telemetry", "diagnostics"]` at
`fabric-service.rs:165`), so two `Publisher` values already hold the same
`supervision_slot` on every run of `just sel4_stream_check`. It is not latent. It has
been passing.

It passes because sharing is correct. Three observations close it:

- A supervision handle names a **task**, not a route. One component on two routes is
  one task with one termination, so one slot is the accurate model — the alternative,
  a slot per route, would claim two independent deaths for one process.
- The root's read is pure. `slime-root/src/supervision.rs:101` is
  `pub fn get(&self, task: TaskId) -> Option<Termination>`: it takes `&self`, returns
  a copy, and consumes nothing. Two `Publisher`s polling one slot both observe the
  same answer, as many times as they ask.
- The state that must differ per route already does. `finished`, `died`,
  `terminated`, and `drained` are `Publisher` fields, not slot state; the shared
  `supervision_slot` sits beside an already-shared `control_slot` in the same struct.
  `provision_edge` has one call site (`:800`) and no publisher or subscriber entry is
  ever cleared, so no stale reuse arises either.

No code change. The checkbox is closed by disproof.

One real finding came out of the audit, and it is a comment, not a defect.
`SUPERVISION_MEMO`'s doc comment justified its size of 12 as headroom over "the
largest supervision table any seL4 plane declares (7, on the matrix graph)". Counting
`capabilityKind = "supervision"` per fixture, `sel4-boot.zti` and `sel4-traffic.zti`
each declare **13** — so 7 was not the largest, and two planes exceed the stated
bound. The memo still cannot overflow, for a different reason than the one it gave:
it is filled only by `provision_edge` and the `TIME_COMPONENT` clock read (`:2278`),
so its ceiling is `MAX_PARTICIPANTS + 1` = 8 (`:175`, from the generation's declared
publisher and subscriber counts). The bound was safe; its stated derivation counted
the wrong set. The comment is corrected in place.

Same lesson as the two corrections above, a third time: a source comment is a claim,
and so is a line citation. Both were one `sed -n` away.

No runtime tests were run for the reading itself; `just fmt_check_all`,
`just lint_all`, and `just sel4_stream_check` were run for the comment change and
passed, the last reporting "57 markers observed across 14 causal chains".
