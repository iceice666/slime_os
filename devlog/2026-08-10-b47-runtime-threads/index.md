# B47 runtime threads: a process runs two of them

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Change |
| Status | Verified |
| Scope | `components/runtime/src/{runtime,lib}.rs`, `components/runtime/src/syscall/sel4_transport.rs`, `components/bins/src/bin/sample-worker.rs`, `slime-root/src/{child_vspace,task,transfer_window,main}.rs`, `boot-contracts/src/generation.rs`, `contracts/generation/v1/fixtures/sel4-sample.zti`, `scripts/check/check-sel4-{sample-plane,component-graph,gate-controls}.py` |
| Roadmap | B47 |
| Gates | `just test_sel4_root`, `just sel4_spawn_check`, `just sel4_supervision_check`, `just sel4_reclamation_check`, `just sel4_boot_check`, `just sel4_sample_check`, `just sel4_component_graph_check`, `just sel4_gate_control_check` |
| Trigger | B47's format half (`f93a55b`, `8e49b5e`) left the runtime half open: a generation could declare a second thread that nothing constructed. |
| Baseline | One `Task` meant one TCB. `slime_rt::entry!` declared one stack and one entry point, and `runtime::start` claimed the crate's single ambient IPC-buffer slot. |

## Summary

A component process now runs two threads. They share a CSpace and a VSpace —
which is what makes them threads rather than tasks — while each owns a TCB, a
stack, an IPC buffer, a transfer window, and a schedule. The obstacle was never
the TCB: it was that `sel4`'s IPC-buffer slot is one process-wide static on
`aarch64-sel4-minimal`, which declares no `has-thread-local`. The answer is the
one B41 reached in the root — a capability carries its own invocation context —
applied at the five transport call sites that reach the ambient buffer, with
the thread's identity in `TPIDR_EL0` because the kernel context-switches it.

`sample-worker` on the sample plane prints from both threads, and the gate
refuses a transcript missing either line.

## Changes

**Runtime.** `entry!` gains a `worker = ...` form declaring a second stack
(`WorkerStack`) and entry point (`__slime_rt_worker_entrypoint`).
`runtime::start_thread` is the worker's path and deliberately never calls
`sel4::set_ipc_buffer`: that static belongs to the main thread, and overwriting
it would repoint the main thread's syscalls at the worker's buffer.
`thread_index()` reads `TPIDR_EL0`; `thread_ipc_buffer_addr` and
`transfer_window_addr` derive from it.

**Transport.** Five sites reached the ambient buffer — `call_on`, the console
`call`, and three console sends. Each now branches: the main thread uses the
ambient path, every other thread supplies `thread_context()` through
`Cap::with`. `WINDOW_BASE`/`WINDOW_LEN` became per-thread arrays; sharing one
entry would let a `recv` on one thread overwrite a `send` staging on the other.

**Root.** `create_child_vspace` takes a thread count and maps one buffer/window
pair per thread, in thread order, at the arithmetic the runtime performs from
its own `_end` — neither image holds a table the other could disagree with. It
refuses a count above `MAX_CHILD_THREADS` rather than truncating. `task::create`
builds one TCB per thread and resolves the worker's entry and stack from the
image's symbol table, because ELF has exactly one entry point. `WindowTable`
keys on `(task, base)`, and `release` drops every window a task declared.

**Contract.** `instance_threads` counts a plan's thread records, and both the
boot and spawn paths read it. The root emits `SLIME_GRAPH threads
instance=... count=N`.

## Regression guards

- `sel4_sample_check` requires three new markers: the declared thread count,
  the worker's own console line, and the main thread's beside it. The worker's
  line can only be written by a thread that reached its entry point, on its own
  stack, and completed a syscall through its own buffer.
- `sel4_component_graph_check` now asserts every task bound a *distinct* window
  base, replacing two hardcoded addresses. The old pins broke on this change
  for an uninteresting reason — component code grew ~700 bytes and crossed a
  page boundary — while the property they meant to protect is distinctness.
- `sel4_gate_control_check`'s marker pin for the sample plane rose 19 → 22.
- `test_sel4_root` is 146, with a new test covering per-thread windows.

## Verification

Two mutations, each observed to fail:

| Mutation | Result |
|---|---|
| Never resume the worker TCB | `missing marker: sample-worker's second thread ran and made its own syscall` |
| Give the worker the main thread's index (`tpidr_el0 = 0`) | `SLIME_GRAPH FAIL required instance sample-worker fault` |

The second is the interesting one: it is the corruption the design prevents,
and it faults rather than silently sharing a buffer.

Both threads observed directly, since the gate truncates at its terminal
marker:

```
SLIME_GRAPH threads instance=sample-worker count=2
[sample-worker] worker thread running
[sample-worker] main thread running
```

Named gates: `test_sel4_root` 146/146, `sel4_spawn_check`,
`sel4_supervision_check`, `sel4_reclamation_check`, `sel4_boot_check` — all
pass. Full sweep of 31 plane gates, `contracts_check`, `generation_check`,
`sel4_boot_layout_check`, `sel4_gate_control_check` (27 gates, 1103 mutations),
`lint_all`, `fmt_check_all`, `ruff`, `typos`, `test_host` — all green.

## Decisions

**`TPIDR_EL0`, not a static.** Three routes were tried. A `CURRENT_THREAD`
static is unsound: two runnable threads race it and the failure is silent
cross-thread window corruption. Deriving the index from the stack pointer works
but couples every syscall to a layout `sel4-runtime-common` owns. The thread
pointer is per-thread in hardware and the kernel context-switches it, so no two
threads can observe each other's value.

**Set `tpidr_el0` in the register context, not through `seL4_TCB_SetTLSBase`.**
seL4 counts that register in the general-purpose set, so a later
`WriteRegisters` overwrites a separately invoked TLS base with the context's
zero. This cost a full diagnostic cycle: the symptom was a thread starting at
IP=1, which is the index itself landing where the PC should be.

**A worker that returns parks.** `exit` terminates the whole task, taking the
main thread with it, and a child holds no TCB capability for its own threads.
So it spins at its own priority and the root reclaims the TCB at teardown.

**The window pins became a distinctness assertion.** Hardcoded addresses were
protecting the wrong thing: they broke because unrelated code changed size,
while a genuine defect — two tasks sharing one base — would have passed them.

## Open risks and follow-ups

- `MAX_THREADS` is 2, matched in `runtime.rs` and `child_vspace.rs`. Raising it
  is a change in both; the root refuses a plan that exceeds it rather than
  truncating.
- The worker's schedule is its process's priority. Per-thread priorities are
  representable in the `ScheduleRecord` but nothing declares them yet.
- Fault attribution is per-task, not per-thread: both threads report through
  one badged fault endpoint, so the transcript says which *instance* faulted,
  not which thread. B46's deletion work is where the fault plane gets revisited.

## Artifacts and provenance

- The two mutations above were run against the sample plane and their failures
  observed; neither is inherited.
- The `~700 byte` growth figure is measured: `spawn-service.elf` ends at
  `0x235e68` before this change and `0x236138` after, from the ELF program
  headers.
- `[INFERENCE]` The claim that a shared window would corrupt a concurrent
  `recv` is reasoning about the code path, not an observed failure; the
  observed failure for a shared *buffer* is the fault in the table above.
