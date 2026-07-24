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

### B1 — `generation_cmd_check` negative scenario aborts init

**Status:** Open. Pre-existing; not caused by the dango-hang fixes.

**Problem:** `just generation_cmd_check` fails on its `bad-closure` and
`bad-release` scenarios. In the negative path, init calls
`spawn_and_wait(23, …)` for `generation-stage`, and `spawn_and_wait` treats any
non-`Exit(0)` termination as fatal (`slime_rt::exit(1)`). But generation-stage
*correctly rejects* the malformed closure and exits `1`, so init aborts before
printing `[init] negative generation scenario complete`, and the boot exits
`Failed`.

**Evidence:** Reproduced on the current tree and confirmed identical on the
baseline with all dango-hang fixes stashed (same `generation-stage terminated:
Some(Exit(1))` → `kernel exit: Failed`). Unrelated to dango: gen-8 does not
spawn the interactive dango REPL.

**Proposed fix:** In `components/bins/src/bin/init.rs`, the negative
generation-command scenario should expect a rejecting `Exit(1)` from
generation-stage rather than aborting — e.g. a `spawn_and_wait`-style helper
that accepts a declared nonzero status for the staged rejection, then proceed to
the `[init] negative generation scenario complete` / `exit(0)` path.

**Exit condition:** `just generation_cmd_check` passes for `success`,
`bad-closure`, and `bad-release`, with rejected staging still leaving BootState
unchanged.

### B2 — scheduler has no `Blocked` task state (busy-poll pathology)

**Status:** Open, deferred. Latent debt; nothing is currently gated on it.

**Problem:** `TaskState` has only `Ready`, `Running`, and `Terminated`. A task
waiting on input or IPC poll-and-yields, staying `Ready`, so it keeps the ready
queue non-empty. The scheduler reaches `exit_qemu` only via `on_idle`, which
fires when the ready queue drains, so any long-lived poll-and-yield component
(the interactive dango REPL being the first) prevents idle exit. The
dango-hang fix (a default Escape input script for non-interactive boots) only
un-wedges the checks; it does not remove the underlying pathology, which will
recur for future long-lived or interactive components.

**Evidence:** `devlog/2026-07-24-boot-check-hangs/` — every non-scripted
full-graph boot (gen-1 storage, gen-6 directory, gen-99 bootstate/rollback)
hung at `dango>` until fixed by scripting an Escape keystroke.

**Proposed fix:** Add a `Blocked` task state so a task waiting on input/IPC
leaves the ready queue and is re-queued on wake, letting `on_idle` fire while a
persistent-but-idle component is parked. This changes scheduler semantics and
touches every poll-and-yield callsite, so it is deferred until a milestone
needs a long-lived component that cannot be driven to termination by a script.

**Exit condition:** A persistent, idle component (e.g. interactive dango with
no input) no longer prevents `on_idle`/`exit_qemu`, and the non-interactive
boot checks pass without relying on a scripted Escape keystroke.

## Resolved

_None yet. Move closed items here with the observed exit condition and date._
