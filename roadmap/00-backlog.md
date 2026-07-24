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
