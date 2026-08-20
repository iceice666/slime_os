# B75: what a determinism gate may compare — separating a trace's declared content from its observed sampling

| Field | Value |
|---|---|
| Date | 2026-08-20 |
| Kind | Decision |
| Status | Verified |
| Scope | `scripts/check/check-sel4-fabric-aggregate.py` |
| Roadmap | B75, C8.15 |
| Gates | `just sel4_fabric_aggregate_check` |
| Trigger | B75's progress note of 2026-08-20, which root-caused two residual trace divergences to faithful observations of genuinely varying quantities and called for a decision entry rather than acting |
| Baseline | `338c9e8`; the aggregate gate compared every rendered `[trace]` field verbatim and measured 6/10 under 24 spinners on 18 cores |

## Summary

C8.15's determinism gate compared each `[trace]` record line byte for byte
within its `(worker, kind)` group. Three of the rendered fields are not
properties of the declared composition at all — they are observations of the run
that produced them — so that comparison asserted that the host scheduled two
QEMU boots identically. It does not, and the gate measured 6/10 under load. This
entry decides what a determinism claim may compare, and revises the gate
accordingly: records are parsed into fields, a declared `SEMANTIC_FIELDS` set is
compared and a declared `OBSERVED_FIELDS` set is not, `high_water` is exempt per
*counter* rather than outright, and `now` is exempt only for the one record
whose instant is a deferred conclusion. The relaxation is scoped by measurement
in both axes, and the gate now passes 10/10 under the same load B75's exit
condition names. B75's remaining half closes on the second branch of its own
exit condition; the first branch — root-causing a stall to a race — was closed
by measurement, since the stall did not recur once in twenty loaded runs.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `check-sel4-fabric-aggregate.py` grammar | `TRACE_LINE`'s whole-line match replaced by `RECORD`/`SUMMARY` field grammars, matching the sibling `check-sel4-trace-plane.py`. A line matching neither fails loudly | A field-level comparison can state which field diverged, and an unparsed trace line is an error rather than a silent omission |
| `SEMANTIC_FIELDS` / `OBSERVED_FIELDS` | The nine fields the composition determines are compared; `sequence` and `high_water` are not. An import-time guard asserts the two sets partition `RECORD.groupindex` exactly | A field added to the grammar cannot land in neither set and go silently uncompared |
| `POLL_SAMPLED_COUNTERS` / `COMPARED_COUNTERS` | `high_water` is exempt only for the ten resource counters that are running maxima over a per-sweep sample. The five that are not — `resourceMapping`, `resourceSinkDropped`, `resourceRoles`, `resourceEvent`, `resourceComplete` — stay compared. Codes come from `scripts/lib/fabric_trace_contract.py`, not restated | The counter whose *not moving* is the invariant is still checked. A guard asserts the split covers the contract's declared range |
| `DEFERRED_INSTANT_RECORDS` | `now` is exempt only for `("stream", "peer-death")`, keyed on the pair. The call and operation workers' peer-death records stay compared | An exemption reaches only the emitter whose mechanism justifies it |
| `check_determinism` | Two independent claims per group: the ordered list of declared positions `(order, now)` must match exactly, and the semantic content is compared as a multiset | Order is checked blind to content and content blind to order; neither subsumes the other |
| Failure rendering | Divergences print in record field order with `xN` multiplicity, and an empty side prints `none` | A `Counter` difference that dropped multiplicity, or rendered a submultiset as a blank section, reported less than it observed |
| Gate output | "byte-identical" replaced by "semantically identical … each holding its declared instant and tie class" | The summary line states the property actually asserted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_fabric_aggregate_check`, idle | Pass: 279 semantically identical trace records (139 traffic + 140 fault) across four boots | Direct |
| `just sel4_fabric_aggregate_check` x10, 24 spinners on 18 cores, no `-icount` pin | **10/10**, load average 55.91 sustained; every run emitted the full 279 records, no wedge and no divergence. B75's exit-condition campaign | Direct |
| 15-case mutation harness over `check_determinism` | The three recorded B75 signatures pass; twelve mutations fail, including stream `resourceMapping` 6→5, a moved call/operation peer-death instant, a reversed declared order within one group, a moved sink-loss counter, and a correlation change that is not a swap | Direct |
| Divergence witness: boot the fault plane twice under load and report which fields differed | Four attempts, all `PASS`; all four happened to agree verbatim, so **the relaxation was not exercised by these pairs** — recorded rather than claimed as evidence for it | Direct |
| `just ruff`, `just typos` | Clean | Direct |

The mutation harness and the witness ran against the gate module imported
directly; neither is checked in, and both are reproduced in the campaign notes
below rather than kept as scripts.

## Decisions

- Decision: a determinism gate compares what the composition *declares*, not what a run *observes*. Fields are classified explicitly, and the classification is guarded at import against the record grammar.
- Rationale: the alternative reading — that every rendered byte is semantic — is a claim about host scheduling, not about the graph. B75 measured it false three separate ways, and each was root-caused to a value that is a faithful reading of a genuinely varying quantity. A gate asserting an untrue property fails intermittently and teaches its readers to re-run it.
- Rejected alternative: making the values constant broker-side. Not available for two of the three. `peak_frames` maxes over a count read once per dispatch-loop iteration, and the run's true concurrent peak differs between boots; the peer-death instant is when a deferred drain completed, which B75 deliberately made independent of the race in *presence* but not in *instant*. Finer sampling cannot fix either, because the concurrency being sampled is itself different.
- Rejected alternative: an `-icount` pin. It does make these fields constant, and B75's exit condition forbids it for that reason: it would assert determinism of an instruction-budgeted guest rather than of the composition.
- Decision: exempt `high_water` per counter, not per field.
- Rationale: ten counters are poll samples; five are not. `resourceMapping` is the counter whose *not moving* is the invariant the contract states, and no plane gate pins its value for the `stream` worker — `check-sel4-traffic-plane.py`'s mapping arm asserts only `baseline == peak` and `peak != 0`. A blanket exemption would have made a stream mapping regression invisible to this gate and to its plane gates together. Found in review, not by measurement, which is why the review round mattered.
- Decision: exempt `now` by `(worker, order)`, not by `order`.
- Rationale: only the stream worker's peer death is drain-deferred. `call_broker`'s and `operation_broker`'s `retire_server` are each reached from an `observe_server_death` that acts straight off the supervision read with no drain between, and their stamps are fixed in every captured transcript. Keying on `order` alone would have exempted two records whose mechanism does not justify it.
- Decision: keep the declared-position check alongside the content multiset.
- Rationale: it was argued in review to be subsumed, and that argument was withdrawn on testing: it silently assumed a correctly sorting sink. A boot emitting one group as `[time@0, data@0]` against `[data@0, time@0]` has an identical content multiset and a different position list. Nothing else covers that on these two images — `check_order` lives in `check-sel4-trace-plane.py`, which runs the separate qos/call/operation images, while the traffic and fault gates assert only that the terminal is last.

## Open risks and follow-ups

- [ ] A reordering *within* one `(order, now)` tie class is invisible to both claims: positions are equal and content is equal. That reordering is `sequence`-only, which this entry declares non-semantic, so it is in scope of the decision rather than a hole — but it is the exact boundary of what this gate now asserts, and it is stated here so a later reader does not rediscover it as a gap.
- [ ] The relaxation was not observed being exercised. Twenty loaded aggregate runs and four witness pairs all produced agreeing traces, so the 6/10 → 10/10 improvement is evidence the gate stopped failing, not a direct observation of a tolerated divergence. The mutation harness covers the intent; a captured diverging pair would be better.
- [ ] `just sel4_trace_check` still compares every rendered field verbatim, including `sequence` and `high_water`, on the qos/call/operation images. It has not been observed flaking — those planes are single-worker and less concurrent — but the property it asserts there is the same one this entry found untrue on the aggregate planes. Not changed here, because no measurement justifies it yet.
- [ ] No mutation-backed regression guard for the broker sweep itself, inherited unchanged from B75: `components/bins/tests/` does not exist and the module is not exported from `components/bins/src/lib.rs`.

## Artifacts and provenance

- Focused report: this entry. The campaign's per-run verdicts and the mutation matrix are quoted inline; the raw per-run logs lived under `/tmp/b75-campaign/` and are not reproduced, since each is a full QEMU transcript whose only distinguishing content is the summary line quoted above.
- Raw transcript: none checked in for this pass. The gate's own output line is the observation.
- Serial/debugger/model output: none beyond the gate transcripts.
- Related roadmap item: [B75](../../roadmap/00-backlog.md), [C8.15](../../roadmap/02-core-runtime.md), and the two entries this one closes over — [`devlog/2026-08-20-b75-stream-peer-death-race/`](../2026-08-20-b75-stream-peer-death-race/index.md), whose corrections root-caused two of the three signatures, and [`devlog/2026-08-20-b74-aggregate-flake/`](../2026-08-20-b74-aggregate-flake/index.md), which recorded the third.
