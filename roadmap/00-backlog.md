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

_No open items._

## Resolved

### B2 — scheduler has no `Blocked` task state (busy-poll pathology)

**Resolved:** 2026-07-24. See `devlog/2026-07-24-b2-blocked-task-state/`.

**Problem:** `TaskState` had only `Ready`/`Running`/`Terminated`. A task waiting
on input or IPC poll-and-yielded, staying `Ready`, keeping the ready queue
non-empty, so `on_idle` (the only path to `exit_qemu`) never fired and every
non-scripted full-graph boot wedged at `dango>`. A default Escape input script
masked the wedge without removing the pathology.

**Fix (design A — wait-set, not blocking recv):** Added
`TaskState::Blocked(BlockReason{Endpoint,Input,Supervision})` and a multi-source
`SYS_WAIT` syscall (max 8 sources, descriptors pack `kind<<32|slot`). `recv`/
`send`/`input_read`/`supervision_status` stay non-blocking; userspace sweeps its
sources then calls `wait` instead of `yield_now`. Waiter registration lives on
each wake source — `recv_waiter` in a new `ipc::Channel`, a global `INPUT_WAITER`
in `drivers/input.rs`, and `wake_on_terminate` on the child `Task`. Wakes are
deferred through a `PENDING_WAKES` queue drained inside `schedule_next` under
`SCHEDULER` (strict order `SCHEDULER → Channel/QUEUE/INPUT_WAITER →
PENDING_WAKES`), fed by `ipc::send`, the keyboard IRQ, `pump_script`,
`task::terminate`, and `Endpoint::Drop`. `wait` re-checks readiness under
IF-clear before parking to close the lost-wakeup race. The default-Escape hack
is removed; `on_idle` now treats an alive, cleanly-blocked persistent service as
healthy while one-shot probes must still `Exit(0)`, and `SLIME_INTERACTIVE`
routes into a new `task::idle_dispatch` (`sti; hlt`) loop instead of exiting.
A pre-existing regression was also fixed: `copy_from_current` bounded a byte
copy at `MAX_CAPS`=64 via a per-byte scratch array, and the `u64`-rights
`SpawnGrant` widening made dango's 5 grants (80 B) exceed it, so `sys_spawn`
returned `ERR_INVALID_ARG` and dango could not spawn.

**Evidence:** `devlog/2026-07-24-boot-check-hangs/` — every non-scripted
full-graph boot hung at `dango>` until an Escape keystroke was scripted.

**Exit condition (observed):** A non-scripted gen-1 boot parks `console`,
`dango`, and `spawn-service` as `idle-blocked` (consuming no CPU), the ready
queue drains to `on_idle`, and QEMU exits `Success` — no scripted Escape. Every
wake source re-readies its waiter: `just dango_check` (`dango native runtime
check: ok`), `just powerbox_check` (input + endpoint waiters), `just
generation_cmd_check` (multi-source generation-manager), `just
spawn_service_check`/`just storage_read_check` (`vertical slice healthy`), and
`just test` all pass, with `just fmt_check`/`just lint` (and `_components`)
clean.

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
