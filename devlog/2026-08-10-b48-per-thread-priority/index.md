# B48: a busy thread declared below its peer does not starve it

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Change |
| Status | Verified |
| Scope | `boot-contracts/src/generation.rs`, `slime-root/src/{task,main}.rs`, `scripts/build/build-generation.py`, `contracts/generation/v1/schema.zt`, `contracts/generation/v1/fixtures/sel4-sample.zti`, `components/bins/src/bin/sample-worker.rs`, `scripts/check/check-sel4-{sample-plane,gate-controls}.py` |
| Roadmap | B48, B47 |
| Gates | `just sel4_sample_check`, `just sel4_qos_check`, `just sel4_root_boot_check`, `just sel4_gate_control_check`, `just test_sel4_root` |
| Trigger | B48's priority half applied a per-*instance* priority; the exit condition also asks that one busy client not starve an unrelated service, and nothing tested it. |
| Baseline | `instance_priority` resolved through the process's main thread, and B47's worker inherited it. One priority per process. |

## Summary

Priority is now per thread rather than per instance, and the starvation clause
of B48's exit condition is proven under the scheduler the project actually
runs. A component declares `workerPriority` below its own `priority`; its
worker spins 200M iterations without yielding; its main thread still reaches
its completion marker. On one core with a priority-only scheduler that can only
happen if the kernel keeps preempting the low-priority loop.

The MCS-dependent clauses — budget and period as authenticated data, timeout
faults reaching a declared handler — remain deferred, so B48 stays open on
them.

## Changes

- `Generation::thread_priority(instance, thread_index)` resolves a specific
  thread's schedule record. Index 0 is the process's `main_thread`; the rest
  follow in table order, which is the order the builder emits and the root
  constructs.
- `Instance.workerPriority` in the manifest, defaulting to `priority`, bounded
  and refused rather than clamped like its sibling.
- `task::create` takes a per-thread priority array; the worker takes its own
  through `admit_priority` rather than inheriting the main thread's.
- Both the boot and spawn paths resolve it, and the boot path emits
  `SLIME_GRAPH schedule instance=... thread=N priority=P`.

## Regression guards

Three assertions on the sample plane, and the gate-control pin raised 22 → 23:

- the worker's declared priority reached the root (`thread=1 priority=100`),
- the main thread ran,
- **the main thread completed while the worker was spinning** — the
  non-starvation property itself.

## Verification

An earlier attempt is worth recording because it failed for an instructive
reason. The first probe put the spin in `fabric-intruder`, which the QoS plane
already declares at priority 100 against peers at 254. It proved nothing: the
intruder is blocked on IPC for most of the run and only reaches its scenario at
transcript line 948, after the publisher's last marker at 893. A low-priority
thread that is not *runnable* during the window of interest demonstrates
nothing about preemption. It was reverted rather than kept as decoration.

B47's second thread is what made the test possible — the spinner has to be
concurrently runnable with the thread it must not starve, and before B47 no
component could have two threads.

The behavioural control is unusually direct. With `workerPriority = 254`:

```
[sample-worker] worker thread running
[sample-worker] main thread running
[sample-worker] main thread running[sample-worker] main thread done
```

Two threads round-robin through the console and the output interleaves
*mid-line*. With `workerPriority = 100` the worker never runs at all and the
transcript is clean. Same binary, same plane, one manifest field.

All 33 plane gates, `contracts_check`, `generation_check`,
`sel4_boot_layout_check`, `sel4_gate_control_check` (27 gates, 1104 mutations),
`test_sel4_root` 149/149, `test_host` 7 suites, `lint_all`, `fmt_check_all`,
`ruff`, `typos`.

## Decisions

**Per thread, not per instance.** The `ScheduleRecord` has been per-thread
since the v5 cutover; reading one priority per instance flattened a
distinction the format already made. A process that cannot differentiate its
own threads cannot express "background work that must never delay my service
loop", which is the entire point of the clause.

**The starvation test asserts the *high*-priority thread's marker, not the low
one's absence.** Absence proves nothing — a thread can fail to print for many
reasons. Progress by the thread that must not be starved, while the other is
provably spinning, is the positive form of the property.

**No `yield_now` in the spinning loop.** A yield hands over voluntarily, which
would make the test pass on a scheduler that ignores priority entirely.

## Open risks and follow-ups

- Budget and period stay zero. Without MCS the kernel has no notion of either,
  and writing figures it cannot enforce would make the record claim more than
  the system does.
- Timeout faults have no mechanism to reach a declared handler without MCS.
- The proof is single-core. Priority preemption on SMP is a different argument,
  and the platform is `-smp 1`.
- `STARVATION_SPINS` is tuned to be long relative to the plane, not calibrated.
  If the plane ever gets much slower, the worker could finish legitimately and
  the assertion would still pass — it asserts the main thread's progress, not
  the worker's incompleteness.

## Artifacts and provenance

- Both transcripts above were observed in this session against
  `build/slime-sel4-sample.elf`; the interleaved line is verbatim.
- The `fabric-intruder` line numbers are from a QEMU run of
  `build/slime-sel4-qos.elf` in this session.
