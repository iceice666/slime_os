# B2 — scheduler Blocked task state (busy-poll pathology)

| Field | Value |
|---|---|
| Date | 2026-07-24 |
| Status | Verified |
| Scope | `kernel/src/task`, `kernel/src/ipc`, `kernel/src/drivers/input.rs`, `kernel/src/syscall`, `kernel/src/runtime/bootstrap.rs`, `components/runtime`, `components/bins`; checks `dango_check`/`powerbox_check`/`generation_cmd_check`/`test` |
| Trigger | Backlog B2, promoted to a declared gate on the C8 native typed data fabric |
| Baseline | Waiting components poll-and-yield staying `Ready`; a default Escape input script masked the resulting idle-exit wedge |

## Summary

`TaskState` had only `Ready`/`Running`/`Terminated`, so a component waiting on
input or IPC poll-and-yielded — retrying a non-blocking syscall on
`ERR_WOULDBLOCK` via `yield_now` — and stayed `Ready`. That kept the ready
queue non-empty, so `on_idle` (the only path to `exit_qemu`) never fired, and
any long-lived component (the interactive dango REPL first) wedged every
non-scripted boot at `dango>`. The prior fix injected a default Escape
keystroke to drive dango to termination; it un-wedged the checks but left the
pathology intact. This milestone adds `TaskState::Blocked(BlockReason)` and a
multi-source `SYS_WAIT` syscall: userspace sweeps its sources with the existing
non-blocking ABI, then calls `wait` instead of `yield_now`. A blocked task
leaves the ready queue, so the queue drains to `on_idle` while it is parked, and
four wake sources (endpoint send, keyboard IRQ, scripted input, child
termination) re-ready it with no lost wakeup. The default-Escape hack is
removed; `on_idle` now treats an alive, cleanly-blocked persistent service as
healthy while still requiring one-shot probes to reach `Exit(0)`.

## Observable symptom

- Command: `SLIME_GENERATION_NUMBER=1 cargo run --release -- -display none` (non-scripted full-graph boot)
- Expected: graph reaches idle, `on_idle` reports healthy, QEMU exits Success.
- Observed (before): boot hung at `dango>`; dango busy-polled its input capability, stayed `Ready`, ready queue never drained, `on_idle` never ran.
- Exit/serial evidence (after): serial shows `console idle-blocked (persistent=true)`, `dango idle-blocked (persistent=true)`, `spawn-service idle-blocked (persistent=true)`, `vertical slice healthy`; QEMU exits `0x10` (Success), shell exit `0`.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `just test` runs `#[cfg(test)]` (`test_main`), never `bootstrap::start()` | The busy-poll/`on_idle` path is exercised only by QEMU check scripts (dango/powerbox/generation_cmd/storage), not by `just test`. |
| 2 | At `on_idle`, the ready queue has drained, so any non-terminated task is necessarily parked in `SYS_WAIT` (`Blocked`) | `on_idle` needs no live-state polling beyond "present and not terminated"; alive == cleanly blocked. |
| 3 | `ipc::send` runs under `SCHEDULER` (via `with_current_mut`); keyboard IRQ and `Endpoint::Drop` do not | Wakes must never take `SCHEDULER`; a deferred `PENDING_WAKES` queue drained inside `schedule_next` gives one lock order. |
| 4 | Non-scripted dango boot failed with `[init] spawn failed slot=3 error=-4`, red on clean master | A pre-existing regression blocked dango from spawning at all, masking B2's exit condition (see Root cause). |

## Root cause

Two distinct issues:

1. **Busy-poll pathology (B2 proper).** `TaskState` lacked a waiting state.
   Poll-and-yield kept waiters `Ready`, so `schedule_next` always found a
   runnable task and never reached the `on_idle` fallback. `exit_qemu` was
   reachable only through the termination cascade, which a persistent or
   interactive component never completes on its own.

2. **`copy_from_current` length bound (pre-existing regression).**
   `copy_from_current` bounded a byte copy at `MAX_CAPS` (64) using a per-byte
   `[PhysAddr; MAX_CAPS]` scratch array. Commit `8cff20a` widened `Rights` from
   `u32` to `u64`, doubling `SpawnGrant` to 16 bytes, so dango's 5 grants (80 B)
   and generation-manager's 6 grants (96 B) exceeded 64 B and `sys_spawn`
   returned `ERR_INVALID_ARG`. Confirmed red on clean `master`; it made dango
   unspawnable and thus blocked B2's exit condition. Rewritten to capture the
   task pml4 once under `SCHEDULER`, then translate and copy page-by-page with
   no bogus length bound (sound on this uniprocessor: syscalls run with IF=0 so
   the task cannot be preempted mid-copy).

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `task/mod.rs` | `TaskState::Blocked(BlockReason)`; `Task.wake_on_terminate`; `PENDING_WAKES` + `wake`/`drain_pending_wakes`/`is_live`; `wait` (readiness re-check + register + park); `pop_ready`/`schedule_next` refactor; `idle_dispatch` | A waiting task consumes no CPU and leaves the ready queue; wakes are deferred and applied under `SCHEDULER`. |
| `task/mod.rs` | `copy_from_current` no longer bounds a byte copy by `MAX_CAPS` | Multi-grant spawns (dango, gen-manager) with `u64` rights copy in full. |
| `ipc/mod.rs` | `Channel{queue,recv_waiter}` replaces bare `Arc<Mutex<VecDeque>>`; `send`/`Drop` wake the peer recv-waiter; `Endpoint` helpers `has_pending`/`peer_dead`/`register_recv_waiter` | A parked receiver is woken by a peer send or peer death. |
| `drivers/input.rs` | `INPUT_WAITER`; `input_pending`/`register_waiter`; `pump_script` and keyboard `on_interrupt` wake the input waiter | A parked reader is woken by a key event or scripted byte. |
| `syscall/mod.rs` | `SYS_WAIT`=20 + `sys_wait` (validate count≤8 + user range, decode `kind<<32\|slot`, call `task::wait`) | Multi-source blocking wait over the existing non-blocking poll ABI. |
| `runtime/bootstrap.rs` | Remove default-Escape hack; `on_idle` treats alive persistent service as healthy, routes `SLIME_INTERACTIVE` into `idle_dispatch` | Idle-blocked graph exits healthy without a scripted keystroke. |
| `components/runtime` | `wait(&[WaitSource])` + `WaitSource` + `MAX_WAIT_SOURCES`, exported | Userspace blocks instead of busy-polling. |
| `components/bins` | ~13 poll sites converted `yield_now`→`wait` (recv→Endpoint, input→Input, held supervision→Supervision, gen-manager multi-source); send back-pressure and dango RPC-poll kept as `yield_now` | Persistent/interactive components park; transient self-resolving spins unchanged. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Busy-poll idle-exit wedge returns | `just dango_check` | Transcript never reaches `[dango] interactive session closed`; timeout. |
| Non-scripted persistent graph never exits | `SLIME_GENERATION_NUMBER=1 cargo run --release -- -display none` | No `idle-blocked`/`vertical slice healthy`; nonzero exit. |
| Multi-source waiter lost wakeup | `just generation_cmd_check` | Generation-manager stalls; missing `vertical slice healthy`. |
| Input wake lost | `just powerbox_check` | Chooser hangs on gesture; missing selection markers. |
| Multi-grant spawn rejected | `just dango_check` | `[init] spawn failed slot=3 error=-4`. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test` | exit 0 (all kernel + integration tests Passed) | Direct |
| `just dango_check` | `dango native runtime check: ok` | Direct |
| `just powerbox_check` | `powerbox capability check: ok` | Direct |
| `just generation_cmd_check` (via `check-generation-commands.py`) | `generation command check: ok` | Direct |
| `just spawn_prereq_check` | 12 + 8 tests Passed | Direct |
| `just spawn_service_check` | `Spawn protocol bindings are current` + `vertical slice healthy` | Direct |
| `just storage_read_check` | `vertical slice healthy` | Direct |
| `just contracts_check`, `just generation_check` | pass | Direct |
| `just fmt_check`, `just fmt_check_components` | clean | Direct |
| `just lint`, `just lint_components` | clean (`-D warnings`) | Direct |
| Non-scripted gen-1 boot | `console/dango/spawn-service idle-blocked`, `vertical slice healthy`, exit 0 | Direct |

## Decisions

- Decision: multi-source `SYS_WAIT` wait-set, not a blocking `recv`.
  - Rationale: multi-source pollers (generation-manager waits on 5 endpoint slots; dango/powerbox-chooser interleave input and endpoint) would deadlock on a single-slot blocking `recv`, and the wait-set is the primitive C9 needs anyway.
  - Rejected alternative: rip-rewind blocking `recv` at the trap.
- Decision: deferred `PENDING_WAKES` drained inside `schedule_next`.
  - Rationale: gives one strict lock order (`SCHEDULER` → `Channel`/`QUEUE`/`INPUT_WAITER` → `PENDING_WAKES`); wake sources run from IRQ, `Drop`, and under `SCHEDULER`, none of which may take `SCHEDULER`.
  - Rejected alternative: waking directly into the ready queue from the wake source.
- Decision: `on_idle` treats alive == cleanly-blocked persistent service as healthy.
  - Rationale: the ready queue has drained, so a non-terminated task is provably parked; a persistent service parked this way is the intended steady state.
  - Rejected alternative: enumerate expected `Blocked` reasons per component.
- Decision: dango's `wait()` RPC-poll and send back-pressure stay `yield_now`.
  - Rationale: dango holds no supervision cap — it polls spawn-service via a WAIT-flag RPC that replies `ERR_WOULDBLOCK` immediately, so no wake source exists; blocking it would hang. Both spins are transient and bounded.

## Open risks and follow-ups

- [ ] Interactive `idle_dispatch` (`SLIME_INTERACTIVE=1`) is verified by reasoning and non-interactive gates only; a human keystroke wake in a live QEMU window was not exercised in this harness. **[INFERENCE]** the `cli; check; sti; hlt` sequence is atomic against the keyboard IRQ on x86.
- [ ] `recv_waiter`/`INPUT_WAITER` are single-slot (SPSC 1:1), matching current channel topology; a future many-waiter channel needs a waiter list.

## Artifacts and provenance

- Related roadmap item: `roadmap/00-backlog.md` B2 (resolved log).
- Prior entry: `devlog/2026-07-24-boot-check-hangs/` (the Escape-hack workaround this milestone removes).
