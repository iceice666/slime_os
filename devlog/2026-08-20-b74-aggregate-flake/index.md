# B74: one flaky gate was two defects, and the silent one hid behind a deliberate suppression

| Field | Value |
|---|---|
| Date | 2026-08-20 |
| Kind | Defect |
| Status | Fixed |
| Scope | `slime-root/src/main.rs` graph dispatcher, `scripts/check/check-sel4-fabric-aggregate.py`, `components/bins/src/fabric_trace_log.rs`, `components/bins/src/bin/fabric-service.rs` |
| Roadmap | B74, B75, C8.15 |
| Gates | `just sel4_fabric_aggregate_check` |
| Trigger | `just sel4_fabric_aggregate_check` failed twice in one session on 2026-08-19 with two different signatures, neither reproducing afterwards |
| Baseline | B68 closed the same gate as deterministic on 2026-08-17 with 10 consecutive passing runs |

## Summary

B74 recorded one intermittent gate. It is two independent defects that happen to
share a symptom. The **wedge** half is a root defect: the dispatcher certifies the
graph `healthy` while every required task is still merely parked, and that
certification permanently disarms the `MAX_GRAPH_ITERATIONS` exhaustion guard by
design (B55). When the workload later stops draining, the root runs its bound out,
prints its ordinary service summary, and stops — silently, with tasks still live.
QEMU's `-serial mon:stdio` does not exit on guest quiescence, so the gate blocks in
`for line in process.stdout` until its watchdog fires and reports a bare timeout
that names nothing. Both signatures are host-timing coupled: under plain TCG the
guest's architected counter tracks host wall time, CPU oversubscription moves
preemption points, and pinning the guest clock to a fixed instruction budget
removes both at the load measured. This entry fixes the silence and leaves the
drain race itself open; it assigns no cause to either signature.

## Observable symptom

- Command: `just sel4_fabric_aggregate_check`
- Expected: both schedules boot twice and emit 140 byte-identical trace records each.
- Observed, on 2026-08-19: `boot 2 exceeded 240s without init's clean exit`, and
  separately a divergence naming a `stream kind=resource ... event=7 high_water=4` row.
- Exit/fault/serial evidence: no fault, no panic, no failure marker. A wedged boot
  ends at `SLIME_ROOT allocator live_slots=…` with `SLIME_GRAPH served live=7 …
  tasks=7`, ~3445 lines against ~3731 for a complete boot.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | A complete boot takes 1.3-2.5s against a 240s timeout — ~120x headroom | "Exceeds 240s" means wedged, not slow |
| 2 | Early hang captures were byte-identical across *different* schedules | The capture was stale: `boot()` discards its transcript on the timeout path, so a caller variable still held the prior boot's text. Reimplemented the reader to retain it |
| 3 | Five genuine hangs, both planes, both boot positions | No `SLIME_ROOT allocator quiescent` in any: the `live == 0` break at `main.rs:2466`, the loop's only break, was never taken |
| 4 | Every hang carries exactly one lowercase `SLIME_GRAPH healthy … live=20 idle=20` | `healthy_emitted` is set while all 20 are merely parked, disarming the guard at `main.rs:3318` for the rest of the boot |
| 5 | Hangs show 12-13 component exits against 19 in a complete boot, none with non-zero status | Nothing failed; work simply stopped progressing |
| 6 | Graded load: 0/10 idle, 0/10 at 4 spinners, 6/10 at 24 spinners on 18 cores | The onset is CPU oversubscription, and B74's "boot 2 only, traffic only" framing was a two-sample artifact |
| 7 | `now_ns` is a credit-barriered logical tick, and `platform_timer.rs` is used only by the boot proof at `main.rs:460-524` | The graph clock is not host wall time; that hypothesis is dead |
| 8 | Counting `"reliable retry accounted"` in the diverging pair gives 3 against 4, matching `event=7 high_water=3/4` | That divergence is a real behavioural difference, not a reporting artifact |
| 9 | `TraceLog::blank()` stamps `sequence`/`now_ns` from sink-shared state | B68's (worker, kind) grouping quarantines record *order* but not field *values* |
| 10 | QEMU is TCG (`info kvm`: "kvm support: not compiled"). At 24 spinners: plain TCG 6/10 pairs fail; `-icount shift=auto` 3/10; fixed `-icount shift=3` **0/10** | A fixed instruction budget suppresses both signatures, so both are host-timing coupled at this load. `shift=auto` re-derives its budget from host speed, so its partial result assigns no cause |

## Root cause

**The wedge.** `main.rs:3268` certifies when `live_required + completed == required`,
which the `completed == 0` arm satisfies with every required task merely parked, and
sets `healthy_emitted = true`. The guard at `main.rs:3318` was
`iterations == MAX_GRAPH_ITERATIONS && live != 0 && !healthy_emitted`, so from the
first certification onward the dispatcher could exhaust all 32768 iterations with
work outstanding and say nothing. The suppression is deliberate and correct for B55 —
a graph whose declared success is every required task parked forever legitimately
runs the loop out — but it was implemented as *silence* rather than as *a reported
fact*, and silence is indistinguishable from a wedge to anything downstream.

The root genuinely cannot tell the two apart: B55's success and a stalled graph both
park required tasks and complete none. The gate can, because it knows whether init's
completion marker had arrived. So the fact belongs in the root and the verdict in
the gate.

**The divergence.** Two mechanisms. `TraceLog::blank()`
(`fabric_trace_log.rs:233-234`) stamps `sequence` and `now_ns` from state shared
across every kind on one sink — `sequence` is a per-instant ordinal reset by
`advance_time`, so it encodes cross-kind arrival order, exactly what B68's grouping
was meant to quarantine. Separately, `retry_count` (`fabric-service.rs:2296-2311`)
increments once per tick where `in_flight != 0`, so a drain that lands on the other
side of a tick reports a different high-water. The first is a reporting artifact;
the second is a real difference.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/src/main.rs` | Split the exhaustion guard: `!healthy_emitted` still fails fatally; the certified case now emits `SLIME_GRAPH exhausted live=… iterations=… certified=1` instead of nothing | A root that stops serving with work outstanding says so |
| `scripts/check/check-sel4-fabric-aggregate.py` | Stop reading on that marker and fail naming the wedge, ahead of the generic timeout arm | A gate reports the cause it observed, not the watchdog that fired |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The root stops serving silently again with tasks live | `just sel4_fabric_aggregate_check` | `boot N wedged: the root exhausted its dispatcher bound …` |
| The wedge returns and is misreported as slowness | `just sel4_fabric_aggregate_check` | Observed: the wedge arm fired on real `slime-sel4-fault.elf` boots at 16.8s and, after a rebuild, 17.1s against the 240s watchdog, naming 32768 iterations and 7 live tasks. Seven healthy boots of the same image passed in 14.1-15.1s, so the arm does not fire on them |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| 10 boot-pairs, idle host | 0 failures | Direct |
| 10 boot-pairs, 4 spinners on 18 cores | 0 failures | Direct |
| 10 boot-pairs, 24 spinners on 18 cores | 6 failures: 4 timeout, 2 divergence | Direct |
| 10 boot-pairs, 24 spinners, `-icount shift=auto,sleep=off` | 3 failures: 3 timeout, 0 divergence | Direct |
| 10 boot-pairs, 24 spinners, `-icount shift=3,sleep=off` | 0 failures; all 20 boots 25.6-31.2s vs a 1.5-2.5s idle baseline, confirming load held throughout | Direct |
| `just sel4_fabric_aggregate_check --no-build` on the rebuilt images | traffic passed both boots (140 byte-identical records); `slime-sel4-fault.elf` boot 1 failed through the **new wedge arm**, naming 32768 iterations and 7 live tasks instead of a bare timeout | Direct |
| Wedge signature across 5 captured hangs | 5/5 identical: no quiescence marker, one lowercase certification, 7-8 tasks live | Direct |

## Decisions

- Decision: the root reports exhaustion as a fact; the gate decides whether it is a failure.
- Rationale: the root cannot distinguish B55's parked-forever success from a stalled
  graph, and inventing a discriminator there would either break B55 or be a guess. The
  gate already knows whether the workload finished.
- Rejected alternative: making the certified case fatal in the root. That would fail
  B55's full-graph boot, whose declared success state is exactly this condition.
- Rejected alternative: pinning `-icount` in `sel4/pins.toml` to buy determinism.
  A fixed `shift=3` did pass 10/10 under the load that fails 6/10 plain, so this is
  a live option rather than a dead one — but it is a separate change on its own
  evidence, not a substitute for the marker. It was measured at one load level on
  one host, `shift=0` and `shift=7` do not boot at all, and it changes what every
  seL4 gate executes. Pinning it would also hide the wedge rather than report it.

## Open risks and follow-ups

- [ ] The drain race itself is unfixed. The marker reports the wedge; it does not
      prevent it. Why the graph stops draining under oversubscription is open.
- [ ] The divergence half is unfixed. `sequence`/`now_ns` sink-shared stamping is the
      same class B68 retired and likely wants either per-kind counters or an explicit,
      argued narrowing of what C8.15 compares — not a silently widened exclusion list.
- [ ] `retry_count`'s tick-coupled high-water is a real behavioural difference, so
      narrowing the comparison there would hide a genuine nondeterminism rather than a
      reporting artifact.
- [ ] B74's exit-condition clause 1 ("10 consecutive passes under comparable host
      load") is unattainable above CPU oversubscription and holds below it. Closing B74
      should go through clause 2.

## Artifacts and provenance

- Focused report: [`observations.md`](observations.md)
- Raw transcript: five wedged boots — [`h0-fault-plane-a6.txt`](h0-fault-plane-a6.txt),
  [`h1-traffic-plane-a15.txt`](h1-traffic-plane-a15.txt),
  [`h2-fault-plane-a18.txt`](h2-fault-plane-a18.txt),
  [`h3-traffic-plane-a19.txt`](h3-traffic-plane-a19.txt), and the first instrumented
  capture [`hang-inst-i1.txt`](hang-inst-i1.txt)
- Serial/debugger/model output: the diverging pair
  [`repro-run8-fault-boot1.txt`](repro-run8-fault-boot1.txt) and
  [`repro-run8-fault-boot2.txt`](repro-run8-fault-boot2.txt); the falsified
  peak-sampling comparison [`idle-boots-resource-records.txt`](idle-boots-resource-records.txt)
  and [`loaded-boots-resource-records.txt`](loaded-boots-resource-records.txt)
- Related roadmap item: [B74](../../roadmap/00-backlog.md), [C8.15](../../roadmap/02-core-runtime.md)
