# B9 — terminated tasks are never reaped, so their frames never return

| Field | Value |
|---|---|
| Date | 2026-07-28 |
| Kind | Defect |
| Status | Verified |
| Scope | `AddressSpace::drop`, `vmm::free_user_half`, scheduler task reaping, spawn failure path, boot reclamation probe, `just test` and `just dango_check` |
| Roadmap | B9, C10 |
| Gates | `just test`, `just dango_check` |
| Trigger | Opened by the C10 planning pass: private component memory would turn a fixed one-shot leak into one that scales with allocation and uptime |
| Baseline | Every spawn permanently consumed its image and stack pages; the fixed boot graph spawns a bounded number of components, so the cost was bounded and never bit |

## Summary

A task that exited never gave anything back. `terminate` marked it
`Terminated` and left the `Task` in `sched.tasks` for the rest of the boot, so
`AddressSpace::drop` never ran — and even when it did, it freed the PML4 alone
and deliberately leaked every user-half page table. The frames
`spawn_with_caps_for` maps for image segments and the stack had no release path
at all. Measured on a live boot, each spawn/exit cycle permanently consumed 13
frames. The fix gives an address space a real teardown and the scheduler a
deferred reaper: a cycle now returns exactly what it took, observed at 14 frames
per cycle with zero drift.

## Observable symptom

- Command: `SLIME_GENERATION_NUMBER=7 SLIME_DANGO_CHECK=1 cargo run --release`
  with the boot reclamation probe added but `free_user_half` not yet called.
- Expected: a spawn/release cycle returns the free-frame count to its start.
- Observed: `[reclaim] spawn/exit leaked: 52 frame(s) over 4 cycles` — 13 frames
  lost per cycle, monotonically.
- Exit/fault/serial evidence: no fault. The boot completes and reports
  `vertical slice healthy`; the leak is silent, which is why it survived until
  it was measured.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `terminate` pushes to `sched.terminated` but leaves the `Task` in `sched.tasks`; `remove_task` is reachable only from the `spawn_from_cap` capability-insert failure path | The `Task`, and the `AddressSpace` it owns, outlive the component for the whole boot |
| 2 | `AddressSpace::drop` deallocs `self.pml4` alone, commented as intentionally leaking intermediate user-half tables "for the small M2 isolation test" | Even a reaped task would return one frame out of the fourteen it took |
| 3 | `spawn_with_caps_for` allocates a frame per image page and per stack page and maps them through `map_user`; no other code path unmaps or frees them | The leak is per-spawn and proportional to image size, not a fixed overhead |
| 4 | A boot probe running four real spawn/release cycles before `launch_init` reported 52 frames lost | Quantified what the backlog entry could only infer from source: 13 per cycle |
| 5 | `terminate` runs on the terminating task's own kernel stack and in its address space, and the `Task` owns both | Reaping cannot happen at termination; it has to be deferred to a later scheduling event |

## Root cause

Two independent gaps on the same path.

The scheduler had no reclamation point. `TaskState::Terminated` was a label,
not a lifecycle transition: nothing ever removed the task, so its `Drop` never
ran. The one caller of `remove_task` was an error path.

`AddressSpace::drop` was written for the M2 isolation test, when a task's
address space held a handful of pages and the kernel booted once. It freed the
root and left everything below it, which is correct only if address spaces are
never destroyed in a running system — exactly the assumption the scheduler gap
made true, and exactly the assumption C10 breaks.

The violated invariant is conservation: a resource acquired on behalf of a task
must be released when that task is gone. Neither half of the teardown held it.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `kernel/src/memory/vmm.rs` | `free_user_half(root)`: walks PML4 entries 0..256, frees leaf pages then the tables that held them, returns the count. Follows only present, non-huge, `PTE_USER` entries, so a corrupted table leaks rather than double-frees | Every frame an address space owns has a release path, and the shared kernel half (256..512) is never touched |
| `kernel/src/memory/address_space.rs` | `Drop` calls `free_user_half` before freeing the PML4 | Dropping an address space returns everything it mapped, not just its root |
| `kernel/src/task/mod.rs` | `reap_terminated(sched, executing)` removes every terminated task except the one the CPU is standing on; called from `schedule_next` after the switch target is chosen, and from `pop_ready_draining` for the interactive idle loop | A terminated task's frames return at the next scheduling event, while the task the kernel is currently running on stays alive |
| `kernel/src/task/mod.rs` | `release_unscheduled(id)` exposes the never-scheduled removal path; `remove_task` documented as the rejected-spawn undo | A rejected spawn costs nothing, and the release path is testable without running a task to exit |
| `kernel/src/runtime/bootstrap.rs` | `reclamation_probe` runs four real spawn/release cycles against a generation component image before `launch_init` and reports frames-per-cycle plus drift | The conservation claim is a measured number on the live boot path, not an inference |
| `kernel/tests/task_reclamation.rs`, `scripts/check/check-dango.py` | Four in-harness conservation tests and a live gate assertion on the probe's verdict | A per-spawn leak of even one frame fails a gate |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A spawn/release cycle leaks | `task_reclamation::spawn_and_release_conserves_frames` (8 iterations, exact equality after warm-up) | `spawn/release leaked N frame(s) on iteration K` |
| Release frees a fixed prefix rather than what was mapped | `task_reclamation::release_returns_every_mapped_frame` compares a 1-page and a 16-page image and pins the 15-frame difference | `release did not scale with the mapped image` |
| A rejected spawn leaks its half-built address space | `task_reclamation::a_rejected_spawn_leaks_nothing` | Free-frame count drops across a failing spawn |
| A task holding capabilities leaks on release | `task_reclamation::release_conserves_frames_for_a_task_holding_capabilities` | Drift across iterations |
| The live boot path regresses even though the harness passes | `just dango_check` → `check_frames_are_conserved` | `[reclaim] spawn/exit leaked: N frame(s)`, or the marker missing entirely |
| Reaping frees a task the CPU is running on | `just test`, `just spawn_service_check`, `just dango_check`, `just sample_plane_live_check` all boot real components through `terminate` | Triple fault or a wild jump rather than a clean exit |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test` | Passed, including the four new `task_reclamation` cases | Direct |
| `just dango_check` | Passed: a real spawn/exit workload with the conservation assertion, twice, deterministically | Direct |
| `just spawn_service_check` | Passed: components spawn and exit through `terminate` and the reaper | Direct |
| `just sample_plane_live_check`, `just fabric_stream_check` | Passed; termination-driven shared-buffer reclamation and the C8.4 stream plane are unaffected | Direct |
| Live measurement | `[reclaim] spawn/exit conserves frames: 14 per cycle, 0 drift` | Direct |
| Fault injection: removed the `free_user_half` call from `AddressSpace::drop` | `task_reclamation::a_rejected_spawn_leaks_nothing` failed, and the live probe reported `52 frame(s) over 4 cycles` — 13 per cycle, quantifying the pre-fix leak | Direct |
| `just fmt_check`, `just lint`, `just fmt_check_components`, `just lint_components` | Clean | Direct |
| Independent review (`history://ReapReview`): verdict `correct` at 0.84, no blocking findings. It confirmed `free_user_half` frees exactly the user half with no reachable double-free or miss, and that the `executing`-skip plus the deliberate no-reap in `pop_ready_draining` close every use-after-free window it could construct. Three doc claims were overstated and corrected: `free_user_half`'s return value is not what the gates compare, and both the probe and the dango gate described themselves as covering the reaper when they cover the release path | Claims corrected; the residual one-task lag and the probe's scope were already recorded as open risks below | Direct |
| Use-after-free found and fixed during the audit: the first draft also reaped from `pop_ready_draining`. `idle_dispatch` is reached from `on_idle`, which `finish_schedule` calls while still on the last task's kernel stack with its PML4 in CR3 — so reaping there could free the stack the loop was running on. Removed; the interactive path now defers to the next scheduling event like every other path | Verified by booting `SLIME_INTERACTIVE=1`: dango exits, `idle_dispatch` parks awaiting input, no fault | Direct |
| Double-free guard added after tracing the shared-buffer interaction: `reclaim_owner` runs inside `terminate` and clears the holder's leaves through `unmap_user_page_in` before any reap walks the table, so a mapped buffer frame is never freed twice. `releasing_a_task_does_not_double_free_shared_buffer_frames` pins that ordering | Fault injection: inverting the order (release the task before reclaiming) fails the test | Direct |

## Decisions

- Decision: Reap from `schedule_next` after the switch target is chosen, rather than at termination.
- Rationale: `terminate` executes on the terminating task's kernel stack and in its address space, and the `Task` owns both, so freeing it there would pull the ground out from under the call that is doing the freeing. Deferring to the next scheduling event costs a one-task lag and nothing else. `reap_terminated` takes the executing task explicitly rather than reading `sched.current`, because `pop_ready` has already reassigned it by then.
- Rejected alternative: A self-destruct at the end of `terminate`. It would need the task to survive its own `Drop` until the stack switch, which is the bug in a different shape.
- Decision: Keep `sched.terminated` as a separate log so reaping is eager.
- Rationale: The exit condition requires supervision results to stay observable after the child is reaped. The reason is a `(TaskId, TermReason)` pair, not a `Task`, so `supervision_status` and `SYS_WAIT` keep answering while the frames go back immediately. Waiting for every supervisor to collect would make reclamation depend on userspace politeness.
- Decision: Free the user half in `AddressSpace::drop` rather than in the reaper.
- Rationale: The address space owns those frames; anything else that drops one — the `spawn_with_caps_for` early-return paths, which hold it as a local — gets the same release for free. That is what makes the rejected-spawn path correct without a second code path.
- Decision: Measure on the boot path, not only in the harness.
- Rationale: The kernel test harness has no user tasks, so it can only drive the release path directly. That proves the mechanism but not that a real spawn's frames are the ones being counted. The probe runs against a real generation component image through the real `spawn_with_caps_for`, before `launch_init` so nothing else is allocating and the delta is attributable.

## Open risks and follow-ups

- [ ] The live probe releases without scheduling, so it measures the spawn/release path rather than spawn/`terminate`/reap. The reaper itself is covered only indirectly, by `just spawn_service_check` and `just dango_check` booting real components that exit through it and still reporting a healthy slice. A gate that counts frames across a full spawn/exit/reap cycle needs a userspace loop and a way to read the free count from a component — C10.4's spawn/exit measurement is where that belongs.
- [ ] A task that terminates when nothing else is runnable is reaped by the *next* scheduling event, which on the non-interactive idle path may never come: `on_idle` exits QEMU. The frames are returned to an allocator that is about to stop existing, so this is harmless today, but a system that idles indefinitely without the interactive loop would hold one task's frames.
- [ ] `free_user_half` walks the whole 256-entry user half per teardown. That is 256 loads for a task using one PDPT; fine at present spawn rates, and the alternative — tracking mapped ranges per address space — is C10 work, since private memory needs that bookkeeping anyway.
- [ ] Physical frames are returned but never zeroed on free. A later spawn's `.bss` pages are explicitly zeroed by `spawn_with_caps_for`, and shared buffers are zeroed on create, so no path currently exposes stale bytes — but a future consumer that maps a raw frame without zeroing would inherit a dead task's data. Zeroing on free is an A4-class hardening decision rather than part of this fix.

## Artifacts and provenance

- Focused report: none.
- Raw transcript: none.
- Serial/debugger/model output: none captured; the live measurement is printed by `just dango_check` as `[reclaim] spawn/exit conserves frames`.
- Related roadmap item: [`B9`](../../roadmap/00-backlog.md#b9--terminated-tasks-are-never-reaped-so-their-frames-never-return).
