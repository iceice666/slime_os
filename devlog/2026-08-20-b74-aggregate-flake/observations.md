# B74 investigation — observed facts (2026-08-20)

## Measurements
- Normal per-boot wall time: 1.33-2.05s (6 boots, idle). Timeout is 240s => ~120x headroom.
  A boot that "exceeds 240s" is therefore WEDGED, not slow. Prior reading of ~50s/boot was wrong;
  the 258s full-gate run is dominated by build, not by guest execution.
- Idle: 3 traffic boots, all 140 records, 39 resource records byte-identical.
- 24-way CPU saturation on 18 cores: 3 more traffic boots, byte-identical to the idle set.
  => peak_* sweep-sampling hypothesis FALSIFIED. Host CPU load alone does not move the peaks.

## Reproduction (this is what B74 lacked)
Under sustained 24-way load, with BOOT_TIMEOUT_SECONDS lowered to 45s, the gate fails often:
15 attempted runs -> 15 failures across both schedules, both boot positions.
Signatures observed:
  - "<image> boot {1,2} exceeded 45s without init's clean exit"
  - "stream's fault record 0 differs" (peer-death, now=50/sequence=0 vs now=0/sequence=2)
  - "call's call record 3 differs"
  - "call's resource record 1 differs"
=> B74's "boot 2 only, traffic schedule only" framing was an artifact of a 2-sample observation.
=> Divergence is NOT confined to peak counters.

## Retracted
An earlier conclusion that the hung boots had reached init's clean exit (making it a gate-side
parsing defect) rested on transcripts that were STALE: my probe wrote the variable `a`, which on
the timeout path still held the previous successful boot's text. Those files were deleted.
Two of them were byte-identical across different schedules, which is what exposed the bug.
The run8 divergence pair has a distinct hash and is genuine.

## Not yet established
- Whether the wedge is guest-side or gate-side. Needs a transcript captured from inside the
  hung boot itself, not from a caller variable.
- send_qos_event (fabric-service.rs) pre-checks supervision_status then does a BLOCKING
  slime_rt::send: a check-then-act race that could block if the peer dies in the window.
  This is a hypothesis, not an observation.

## ROOT CAUSE (observed 2026-08-20)

The "exceeded 240s" signature is a GUEST-side wedge, not a gate parsing bug.

A hung boot, captured with the gate's own loop conditions instrumented
(`hang-inst-i1.txt`), stops at 3446 lines vs 3734 for a complete run:
  - `saw_init_complete=False` — `[init] traffic plane reclaimed` never emitted
  - last line is `SLIME_ROOT allocator live_slots=...`, part of the root's
    graph-service exit summary (slime-root/src/main.rs:3331-3376)
  - `SLIME_GRAPH served live=8 ... tasks=8` — the dispatcher exited with 8 live tasks
  - no `SLIME_ROOT allocator quiescent` marker => the `live == 0` break at
    main.rs:2466-2473 (the loop's ONLY break) was not taken

Therefore the loop ended by running out `MAX_GRAPH_ITERATIONS` (32768, main.rs:2407).

Why no `SLIME_GRAPH FAIL`: the guard at main.rs:3318 is
    iterations == MAX_GRAPH_ITERATIONS && live != 0 && !healthy_emitted
and the hung boot emitted the LOWERCASE certification
    SLIME_GRAPH healthy generation=36 instances=19dc788956e2d830 required=20 live=20 idle=20 failed=0
from the `completed == 0` arm at main.rs:3279, which sets `healthy_emitted = true`
(main.rs:3308). The comment at main.rs:3312-3317 states this suppression is
deliberate for B55 (a graph whose declared success state is every required task
parked forever legitimately runs the loop out).

So: on the aggregate planes the root certifies `healthy` EARLY (while all 20 are
still merely parked/idle), and that certification then permanently disarms the
iteration-exhaustion guard for the rest of the boot. When the workload later fails
to drain under host load, the root runs out its iterations, prints its exit summary,
and stops WITHOUT emitting the reclaim marker and WITHOUT failing loudly. QEMU with
`-serial mon:stdio` does not exit on guest quiescence, so the gate then blocks in
`for line in process.stdout` until its watchdog fires — reported as a timeout.

## Wedge signature confirmed 5/5 (2026-08-20)

Four further hangs were captured with a reader that RETAINS the transcript on the
timeout path (the gate's own `boot()` discards `lines` when it fails), across both
planes and both boot positions. With `hang-inst-i1.txt` that is five samples, and
the signature is identical in all five:

| transcript | lines | served live/tasks | quiescent | healthy | reclaimed |
| --- | --- | --- | --- | --- | --- |
| `h0-fault-plane-a6.txt` | 3447 | 7 / 7 | 0 | 1 | 0 |
| `h1-traffic-plane-a15.txt` | 3444 | 8 / 8 | 0 | 1 | 0 |
| `h2-fault-plane-a18.txt` | 3446 | 7 / 7 | 0 | 1 | 0 |
| `h3-traffic-plane-a19.txt` | 3444 | 8 / 8 | 0 | 1 | 0 |
| `hang-inst-i1.txt` | 3445 | 8 / 8 | 0 | 1 | 0 |

A complete boot is ~3731 lines. No hang emits `SLIME_ROOT allocator quiescent`, so
the `live == 0` break at `main.rs:2466` — the loop's only break — was never taken;
each ran `MAX_GRAPH_ITERATIONS` out. Each emits exactly one lowercase
`SLIME_GRAPH healthy ... required=20 live=20 idle=20`, which sets
`healthy_emitted = true` and permanently disarms the exhaustion guard at
`main.rs:3318`.

Component exits at the wedge point: 12-13, versus 19 in a complete boot. No exit
carries a non-zero status in any hang, so nothing failed — work simply stopped
progressing. The root then prints its ordinary exit summary and stops silently.

## Graded-load threshold (2026-08-20)

Replaces the earlier binary idle/saturated comparison. Host has 18 cores.

| Host load | Boot-pairs | Failures | Per-pair wall time |
| --- | --- | --- | --- |
| idle | 10 | 0 | 1.5-2.5s |
| 4 spinners (22%) | 10 | 0 | 2.3-3.1s |
| 24 spinners (133%) | 10 | 6 (4 timeout, 2 divergence) | 15.9-29.5s |

The failure onset is CPU oversubscription, not load as such. Consequence for B74's
exit condition: clause 1 ("10 consecutive passes under comparable host load") is
attainable at idle and at mild load but not above oversubscription.

## Divergence has two distinct mechanisms (2026-08-20)

Diffing `repro-run8-fault-boot1.txt` against `boot2.txt` through the gate's own
`records_by_participant()` isolated three diverging groups, from two causes:

1. **Sink-shared stamping (reporting artifact).** `TraceLog::blank()`
   (`components/bins/src/fabric_trace_log.rs:233-234`) stamps `sequence` and
   `now_ns` from state shared across every kind on one sink. B68's (worker, kind)
   grouping quarantines record *order* but not field *values*, so cross-kind
   interleaving still leaks into each record. Observed: `('stream','fault')`
   diverged `sequence=2 now=0` vs `sequence=0 now=50`; `('stream','qos')` diverged
   `sequence=3` vs `sequence=2`.

2. **A genuine drain-vs-tick race (real behavioural difference).**
   `retry_count` increments once per tick where `in_flight != 0`
   (`fabric-service.rs:2296-2311`). Counting `"reliable retry accounted"` in the
   repro pair gives 3 in boot1 and 4 in boot2, exactly matching the observed
   `event=7 high_water=3` vs `high_water=4` divergence (`RESOURCE_RETRIES = 7`,
   `components/proto/src/fabric_trace.rs:30`). This is not a reporting artifact.

## Host-clock hypothesis ruled out (2026-08-20)

The graph clock is not host wall time. `now_ns` is a logical tick: publisher-b's
`advance_time()` sends a `WireTimeAdvance` and `await_time_credit()` then blocks
until `fabric-service` echoes the same `now_ns` back at the end of `apply_time`
(`fabric-service.rs:2381-2394`) — a credit barrier. The EL1 physical timer in
`slime-root/src/platform_timer.rs` (`CNTP_*`, PPI 30) is used only by the boot
proof at `main.rs:460-524` and does not drive the graph service loop.

## The two signatures separate under `-icount` (2026-08-20)

QEMU here runs under TCG, not KVM (`info kvm` reports "kvm support: not
compiled"; `-accel help` offers only `hvf` and `tcg`). Under plain TCG the guest's
architected counter tracks host wall time, so host scheduling reaches the guest.
`-icount` ties it to instructions retired instead.

Boot works at `shift=1`, `shift=3`, and `shift=auto` (0.7-0.9s, faster than the
1.5-2.5s baseline) and does not boot at `shift=0` or `shift=7` within 90-180s.

Re-running the 10-boot-pair loop at 24 spinners on 18 cores (load average 30-35),
`-icount shift=auto,sleep=off`:

| Signature | Plain TCG | `-icount shift=auto` | `-icount shift=3` |
| --- | --- | --- | --- |
| trace divergence | 2 | **0** | **0** |
| boot timeout (wedge) | 4 | 3 | **0** |
| total pairs failed | 6/10 | 3/10 | **0/10** |

The `shift=3` column is the load-valid one: all 20 boots ran 25.6-31.2s against a
1.5-2.5s idle baseline, with the last pair the slowest (30.5s/31.2s), so the 24
spinners held for the whole run. An earlier fixed-shift attempt was discarded
because its spinner budget expired mid-run and left most pairs on an idle host.

What this does and does not establish:

- A **fixed** instruction budget suppresses **both** signatures at this load
  (6/10 — 0/10). Both are reachable only when the guest's clock is free to
  track host wall time, so both are host-timing coupled at the loads measured.
- `shift=auto` is **not** a determinism guarantee — it re-derives its budget
  from observed host speed. Its partial result (4 -> 3 timeouts) is therefore not
  evidence about where the wedge originates, and an earlier draft of this entry
  wrongly read it as proof the wedge was guest-side. A non-deterministic pin
  failing to remove a symptom says nothing about that symptom's cause.
- No cause is assigned to either half here. What is measured is the load
  coupling; the underlying drain race remains open below.

The root-side marker is justified independently of all of this. The root exiting
silently with tasks outstanding is a reporting defect whatever stalls the graph,
and 0/10 under a fixed shift is not a licence to stop reporting: it was observed
at one load level on one host, and the wedge still occurs here under the plain
TCG configuration the gate actually uses — as the real gate run below shows.

## Still open
- Why the workload fails to drain in the first place (the trace-divergence signature
  is likely the same underlying race). send_qos_event's check-then-act blocking send
  remains an unverified hypothesis for that half.
