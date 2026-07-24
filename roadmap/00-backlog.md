# Backlog (defects and unmasked debt)

**Purpose:** Track concrete defects, regressions, and latent bugs found in
implemented code that must be resolved before starting new roadmap-track
milestones. Backlog items are not new capability; they restore an already
claimed exit condition or remove debt that would compound under new work.

**Priority:** Backlog items are handled before roadmap-track milestones. A green
verification suite is a precondition for milestone work, not a milestone itself.
Clear or explicitly defer every open item here before opening a new track gate.

**Entry shape:** Each item states the problem, the evidence (how it was
observed), the proposed fix, and the exit condition that closes it. Close an
item only when its exit condition is observed, then move it to the resolved log
at the bottom rather than deleting it.

## Open

### B2 — scheduler has no `Blocked` task state (busy-poll pathology)

**Status:** Open, active. Promoted from deferred: it is now a declared gate on
the C8 native typed data fabric. C8's fabric service is a long-lived component
that no scripted keystroke can drive to termination, so C8's exit condition
(stalled subscriber, peer death, graph idle) cannot be observed under QEMU
while the busy-poll pathology remains. C7.2–C7.7 do not depend on B2 (shared
buffers complete synchronously), so they proceed first; B2 lands before C8
opens.

**Problem:** `TaskState` has only `Ready`, `Running`, and `Terminated`
(`kernel/src/task/mod.rs`). A task waiting on input or IPC poll-and-yields,
staying `Ready`, so it keeps the ready queue non-empty. The scheduler reaches
`exit_qemu` only via `on_idle` (`bootstrap.rs`), which fires when the ready
queue drains, so any long-lived poll-and-yield component (the interactive
dango REPL being the first) prevents idle exit. The dango-hang fix (a default
Escape input script for non-interactive boots, `bootstrap.rs`) only un-wedges
the checks; it does not remove the underlying pathology, which recurs for
future long-lived or interactive components.

**Evidence:** `devlog/2026-07-24-boot-check-hangs/` — every non-scripted
full-graph boot (gen-1 storage, gen-6 directory, gen-99 bootstate/rollback)
hung at `dango>` until fixed by scripting an Escape keystroke.

**Committed fix (design A — wait-set, not blocking recv):** A single blocking
`recv` is rejected: multi-source pollers (`generation-manager` waits on five
endpoint slots; `dango`/`powerbox-chooser` interleave input and endpoint
polling) would deadlock if `recv` blocked on one slot. Instead:

1. Add `TaskState::Blocked(BlockReason)` where `BlockReason` distinguishes the
   wake class (`Endpoint`, `Input`, `Supervision`) so the C9 wait-set/executor
   can later compose on top of the same discriminant.
2. Add a `SYS_WAIT` syscall taking a bounded slot set. `recv`/`send`/
   `input_read`/`supervision_status` stay non-blocking with their existing
   `ERR_WOULDBLOCK` ABI; userspace polls, then calls `wait` over the same
   source set instead of `yield_now`. This keeps the wait primitive
   multi-source from the start.
3. Add waiter registration to each wake source: a waiter task id (SPSC 1:1 is
   sufficient for current channels) in `EndpointInner` (`kernel/src/ipc`), an
   input-waiter slot for the keyboard queue (`kernel/src/drivers/input.rs`), and
   supervision-waiter link on the parent task.
4. Wire wakes from all four sources: `ipc::send` (peer enqueue), the keyboard
   IRQ handler (`interrupts.rs`), `input::pump_script`, and `task::terminate`
   (child exit wakes a supervision-waiting parent). A wake moves the task
   `Blocked -> Ready` and re-queues it.
5. Close the lost-wakeup race: under `without_interrupts`, re-check readiness
   of the requested set immediately before parking; if any source is already
   ready, return without blocking. Uniprocessor, so only IRQs interleave.
6. Rework `on_idle` (`bootstrap.rs`): once services block instead of
   terminating, most of the graph is `Blocked` (idle), not
   `Terminated(Exit(0))`. `on_idle` must accept an idle-blocked persistent
   service as healthy while still requiring one-shot probe components to reach
   `Exit(0)`.
7. Remove the default-Escape hack (`bootstrap.rs`); its removal is the exit
   condition. Explicit `SLIME_INTERACTIVE` and the scripted dango/powerbox
   checks keep their own installed input.
8. Rewrite the 15+ userspace poll sites (`components/bins`, `components/runtime`)
   to call `wait` over their source set after an `ERR_WOULDBLOCK` sweep rather
   than `yield_now`.

This is milestone-scale (kernel + syscall ABI + ~20 files) and ships with its
own QEMU gate. Rejected alternative: a single blocking `recv` (rip-rewind at
the trap) — provably wrong for the multi-source pollers above, and it would be
rebuilt for the C9 wait-set anyway.

**Exit condition:** A persistent, idle component (e.g. interactive dango with
no input) no longer prevents `on_idle`/`exit_qemu`, and the non-interactive
boot checks pass without relying on a scripted Escape keystroke. A blocked task
consumes no CPU (the ready queue drains to `on_idle` while it is parked), and
every wake source (endpoint send, keyboard IRQ, scripted input, child
termination) re-readies its waiter with no lost wakeup.

## Resolved

### B1 — `generation_cmd_check` negative scenarios corrupted the wrong generation

**Resolved:** 2026-07-24.

**Problem:** `just generation_cmd_check` failed on its `bad-closure` and
`bad-release` scenarios. The original diagnosis (init's `spawn_and_wait`
aborting on a rejecting `Exit(1)`) was wrong: `generation-stage` already
classifies a `-4`/`-3` rejection internally and exits `0`, and init already
exits cleanly after the staged rejection. The real defect was in the fixture
builder `scripts/check-generation-commands.py`. `build_fixture` corrupted
`entries[1]` by fixed directory index, but the bootstore directory is
identity-sorted and staging targets the *candidate* generation (identity ≠
known-good). When component images changed the identity sort order, the
corruption landed on the untouched known-good generation, so staging *succeeded*
(`status=0`), `generation-stage` hit its non-`-4`/`-3` `fail()` path, and the
boot exited `Failed`.

**Evidence:** Instrumented `generation-stage` printed `unexpected status=0` on
`bad-closure`; probing the fixture confirmed the flipped byte fell inside the
known-good generation's blob, which staging never reads.

**Fix:** Select the candidate entry by `identity != known_good` (read from
BootState) instead of a fixed directory index, so the corruption always lands on
the generation staging actually validates.

**Exit condition (observed):** `just generation_cmd_check` passes for `success`
(`staged release=3`), `bad-closure` (`rejected status=-4`), and `bad-release`
(`rejected status=-3`), with rejected staging leaving both BootState slots
unchanged.
