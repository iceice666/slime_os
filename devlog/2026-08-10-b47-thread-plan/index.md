# B47 — three assumptions kept the process/thread split notional

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/generation/v1/schema.zt`, `scripts/build/build-generation.py`, `boot-contracts/src/generation.rs` |
| Roadmap | B47 |
| Gates | `just sel4_channel_check`, `just sel4_boot_check`, `just sel4_spawn_check`, `just sel4_supervision_check`, `just sel4_reclamation_check` |
| Trigger | B47: one `Task` means image instance, CSpace/VSpace owner, single TCB, service identity, and lifecycle identity at once. |
| Baseline | v5 separated process and thread records; nothing could declare more than one thread. |

## Summary

A generation can declare extra threads in a process and the decoder admits
them. v5 has had separate process and thread records since the cutover, but
three independent assumptions — one in the builder, two in the decoder — kept
the split notional. All three are fixed. The root still constructs only the
main thread, which is the item's larger remaining half.

## Observable symptom

Declaring a second thread produced `SLIME_ROOT FATAL generation rejected:
BadBounds` before any component started.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Builder sets `thread = process` | Holds only while the tables grow in lockstep |
| 2 | Generation with 2 processes, 3 threads decodes to `BadBounds` | Not a section-layout problem: every section stayed adjacent |
| 3 | Guessing header offsets repeatedly | Wrong five times; switched to tagging each `BadBounds` site with a distinct `Probe(n)` |
| 4 | `Probe(8)` → `validate_plan` | `process_count != thread_count` refused it outright |
| 5 | Then `BadKernel` | The per-thread check required `main_thread == index` for *every* thread |

## Root cause

Three assumptions, each individually reasonable while a process had one thread:

- **`thread = process`** in the builder, indexing the thread, schedule, and
  fault tables by the process index.
- **`process_count == thread_count`** in `validate_plan`, alongside the
  schedule, fault-policy, and quota counts.
- **`process(thread.process).main_thread == index`** in the per-thread check,
  which requires every thread to be its process's main one.

## Changes

- `Instance.extraThreads?` in the manifest schema.
- Table indices counted from the records rather than assumed equal to the
  process index.
- `validate_plan` requires `process_count <= thread_count`, keeps the quota per
  process, and ties schedules and fault policies to the thread count.
- A thread is checked for belonging to a real process and owning objects of the
  right kind; `main_thread` is validated separately against the thread table,
  so a process cannot name a thread belonging to someone else.

## Regression guards

- `main_thread` validation moved rather than removed: the new loop catches a
  process naming another process's thread, which the old check could not see
  because it walked threads.
- Every extra thread's TCB must be owned by the process that claims it, which
  the old check did not verify at all.

## Verification

| Check | Result |
|---|---|
| `extraThreads = 1` on the channel plane's console | 2 processes, 3 threads; decodes clean |
| `just sel4_channel_check` with it declared | pass |
| `just sel4_boot_check`, `sel4_spawn_check`, `sel4_supervision_check`, `sel4_reclamation_check` | pass |
| All 30 seL4 gates | pass |
| `cargo test -p slime-root --lib` | 146 passed |
| `cargo test -p boot-contracts --lib` | 180 passed |
| `just contracts_check`, `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos` | clean |

## Decisions

**Probes instead of more guessing.** I misread the header offsets five times in
a row — object count, object offset, process offset, record lengths — each time
producing a confident wrong conclusion about which check was failing. Tagging
every `BadBounds` site with a distinct `Probe(n)` variant found it in one run.
The lesson is cheap: when a layout is generated, read the generated constants
or instrument, do not infer from a hex dump.

**The fixture is reverted.** Leaving `extraThreads = 1` declared would mean a
plan the root does not execute — a thread allocated in the object plan, counted
in the quota, and never started. That is worse than not declaring it, because
the transcript would show a two-thread process that behaves as one.

**Quota stayed per process, schedule and fault per thread.** The counts were
all equal before, so nothing distinguished them. A quota bounds CSpace, VSpace,
and objects, which a process owns; a schedule and a fault policy describe a
thread. Getting this wrong in the other direction would have made a
two-threaded process consume two quotas.

## Open risks and follow-ups

- Running a declared second thread needs `slime-rt` restructured:
  `slime_rt::entry!` declares one stack, one entry point, and one IPC buffer,
  and `runtime::start` calls `sel4::set_ipc_buffer`, which claims the crate's
  single ambient slot per address space. That is exactly B41's obstacle in the
  root, and `Cap::with` is the same answer — applied to every component's entry
  path.
- Nothing yet asserts that a declared extra thread is *not* silently dropped.
  Until the root constructs one, a fixture declaring one would pass every gate
  while running single-threaded.

## Artifacts and provenance

- Commits: `f93a55b`, `7884b24`, `f5df17b`.
