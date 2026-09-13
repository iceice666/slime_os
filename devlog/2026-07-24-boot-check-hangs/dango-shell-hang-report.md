# Investigation: dango shell hangs `directory_check`, `bootstate_trace_check`, and `rollback_check`

**Date:** 2026-07-24
**Context:** Follow-on to the stage-0 stack-overflow fix
(`stage0-stack-overflow-report.md`, sibling in this folder). After that crash
fixed, the stage-0 checks stopped triple-faulting and instead booted healthy
to an idle `dango>` prompt, then hit the new subprocess timeout. This report
identifies why the guest never self-terminates.

**Verdict:** Commit `f7d63e8` ("feat(components/dango): add native interactive
runtime") turned dango from a one-shot spawn client into a **persistent
keyboard REPL**. The kernel's only path to `exit_qemu` is the scheduler's
`on_idle` callback, which fires only when the ready queue drains — i.e. when
*every* task has `Terminated`. A persistent dango that busy-polls the keyboard
never terminates, so the ready queue never empties and `on_idle` never fires.
Every automated check that boots the full component graph **without driving
dango's keyboard to an `Escape`** now hangs until the harness timeout.

A **second, independent regression** from the same commit blocks gen-99
(bootstate/rollback) even earlier: init's storage-probe spawn is rejected with
`BadCapability`, killing init before it spawns `generation-manager`.

## Reproductions (release kernel, this machine)

| Scenario | Env | Result | Terminal serial |
|---|---|---|---|
| directory_check | `SLIME_GENERATION_NUMBER=6` + virtio-blk | **hang, EXIT=124** | all `[directory-probe]` markers, then idle `dango>` |
| bootstate_trace_check | `SLIME_GENERATION_NUMBER=99 SLIME_PENDING_GENERATION=1 SLIME_PENDING_ATTEMPTS=2` | **hang, EXIT=124** | `[spawn] rejected BadCapability` / `[init] spawn failed slot=8` / idle `dango>` |
| dango_check | `SLIME_GENERATION_NUMBER=7 SLIME_DANGO_CHECK=1` | **EXIT=0** | `[dango] interactive session closed` → all components `Exit(0)` → `[generation] vertical slice healthy` |

The dango_check pass is the control: it is the only full-graph boot that
installs a keyboard script (`bootstrap.rs:50`) ending in `\x1b` (Escape), which
drives dango to `return`.

> Correction to the prior report: it stated `directory_check` "ran to
> `[directory-probe] done`" and passed. That was the *marker* check; the QEMU
> process does **not** exit — directory_check hangs at `dango>` exactly like
> the gen-99 checks. Reproduced directly here (EXIT=124).

## Mechanism

### The only exit path is `on_idle`, gated on all-tasks-terminated

- `kernel/src/bootstrap.rs:73` registers `on_idle` as the scheduler idle hook.
- `kernel/src/task/mod.rs:638-656` `schedule_next`: `on_idle` is invoked only
  when `sched.ready.pop_front()` yields nothing — the ready queue is empty.
- `kernel/src/task/mod.rs:51-55` `TaskState` has **no `Blocked` variant**:
  only `Ready`, `Running`, `Terminated`. A task waiting on IPC or input
  poll-and-yields, staying `Ready`.
- `yield_now` (`kernel/src/task/mod.rs:576-585`) re-pushes the current task
  onto `ready`. So any task that loops on `yield_now` keeps the ready queue
  non-empty **forever**.
- `on_idle` (`bootstrap.rs:691-817`) is the sole non-test caller of
  `exit_qemu` in the boot path; there is no reset/shutdown syscall that exits
  QEMU independently. `SYS_UNHEALTHY` merely terminates the calling task
  (`kernel/src/syscall/mod.rs:60`).

Conclusion: the guest can only exit QEMU once **every** live task terminates.

### dango is now a persistent busy-poll REPL

`components/bins/src/bin/dango.rs:31-75`:

```rust
loop {
    match slime_rt::input_read(INPUT_SLOT) {
        Ok(None) => slime_rt::yield_now(),          // no key ready → spin
        Err(_)   => slime_rt::exit(1),
        ...
        InputKey::Escape => {
            console(b"\n[dango] interactive session closed\n");
            return;                                  // ONLY self-exit
        }
        ...
    }
}
```

`input_read` returns `Ok(None)` when the kernel input queue is empty
(`ERR_WOULDBLOCK`, `syscall.rs:290`; kernel side `sys_input_read`
returns `ERR_WOULDBLOCK` when `pop_event()` is empty,
`kernel/src/syscall/mod.rs:683-685`). With no keyboard and no installed input
script, dango spins `Ok(None) → yield_now()` indefinitely and never returns.
It only self-exits on an `Escape` key.

Before `f7d63e8`, dango's `main()` was a one-shot: resolve `sysinfo`, verify
profile rejections, then fall off the end → `entry!` calls `exit(0)`
(`components/runtime/src/lib.rs:35`) → task `Terminated` → `on_idle` fired.

### Why gen-7 (dango_check) still passes

`bootstrap.rs:49-52` installs a scripted keystroke stream for gen-7 ending in
`\x1b`. `sys_input_read` pumps the script (`input.rs:289`), dango sees
`Escape`, returns, and its `exit(0)` cascades: console/spawn-service/
filesystem-service exit on `ERR_PEER_DEAD` (`console.rs:14`,
`spawn-service.rs:46`, filesystem-service `:57`), directory-probe/gen-manager
finish, the ready queue drains, `on_idle` fires, `exit_qemu(Success)`.
The gen-9 powerbox check does the same with `b"\n\x1b"` (`bootstrap.rs:55`).

gen-6 (directory) and gen-99 (bootstate/rollback) get **no** script, so dango
never terminates → hang. These checks predate `f7d63e8` and were silently
broken by it (they previously exited via the one-shot dango).

## Second regression (gen-99 only): storage-probe `BadCapability`

Independent of the dango hang, gen-99 boots fail even earlier:

```
[init] launching component graph
[spawn] rejected BadCapability
[init] spawn failed slot=8 error=-1
```

- gen-99 boots with no virtio/nvme disk, so
  `optional_block_function()` returns `None`
  (`kernel/src/bootstrap.rs:329-333, 559-562`).
- `f7d63e8` changed init's slot-9 storage cap from an unconditional
  `BlockDevice`/`ObjectStore` capability to a fallback:
  `caps.push(storage_capability.unwrap_or(ObjectStore/RIGHT_STORE_READ))`
  (`bootstrap.rs:385-388`). With no disk, init's slot 9 is an
  `ObjectStore` cap carrying only `RIGHT_STORE_READ`.
- init unconditionally spawns storage-probe (slot 8) for gen != "9",
  deriving `RIGHT_BLOCK_READ` from slot 9
  (`init.rs:151-155, 28, 111`). You cannot derive `BLOCK_READ` from a
  `STORE_READ`-only `ObjectStore` cap → `BadCapability`.
- `spawn_or_fail` calls `exit(1)` on failure (`init.rs:181-189`), so init
  dies **before** spawning generation-manager (slot 10, `init.rs:158`).
  The pending → unhealthy → rollback flow that the check asserts never runs.

Pre-`f7d63e8`, init's storage cap was always a real `BlockDevice` cap
(`default_block_function()`), so this derive succeeded regardless of a disk.

## Impact summary

- `directory_check` (M6.3), `bootstate_trace_check` (M5.6c), `rollback_check`
  (M5.6): **hang** at `dango>`; caught now only because the sibling task added
  subprocess `timeout=` to the check scripts.
- `dango_check` (M6.4), `powerbox_check` (M6.6): unaffected (scripted Escape).
- gen-99 checks carry an additional init-death regression that must be fixed
  for the rollback `2 → 1 → 0` sequence to complete even after dango is fixed.

## Recommended fixes

Ordered by leverage; both regressions trace to `f7d63e8`.

1. **Terminate dango deterministically in non-interactive boots.** The clean,
   capability-consistent option: give gen-6 and gen-99 the same treatment gen-7
   and gen-9 already receive — install an `Escape` input script so dango closes
   and the termination cascade fires. This keeps `on_idle`'s
   all-tasks-terminated invariant intact and needs no kernel change.
   (`bootstrap.rs` `install_script` for `generation.number == 6` and `== 99`,
   gated on the relevant check env vars, mirroring lines 49-56.)

   Alternative (larger, but removes the busy-poll pathology generally): add a
   `Blocked` task state so a task waiting on input/IPC leaves the ready queue,
   letting `on_idle` fire while a persistent-but-idle dango is parked. This is
   the correct long-term design but changes scheduler semantics and every
   poll-and-yield callsite; out of scope for un-wedging the checks.

2. **Fix the gen-99 storage-probe cap derivation.** Either:
   - have init skip storage-probe when no block cap is available (extend the
     gen-based guard at `init.rs:151-155`), matching the design intent that
     storage-probe `Exit(1)`s when the disk is absent
     (`on_idle`'s `optional_storage_absent`, `bootstrap.rs:752-754`); or
   - restore a disk-independent block capability path so the derive succeeds.

   The first is preferred: it keeps the no-disk boot honest (no phantom block
   authority) and lets init proceed to generation-manager.

3. **Verify end-to-end after both fixes:** `just directory_check`,
   `just bootstate_trace_check`, `just rollback_check` must exit cleanly
   (not merely time out), plus `just dango_check` / `just powerbox_check`
   to confirm no regression, and `just test`.

## Artifacts

- `/tmp/gen6-serial.log` — directory_check (gen-6) hang at `dango>` (EXIT=124).
- `/tmp/gen99-serial.log` — bootstate (gen-99): `BadCapability` + `dango>`
  hang (EXIT=124).
- `/tmp/gen7-serial.log` — dango_check (gen-7): clean cascade to
  `[generation] vertical slice healthy` (EXIT=0).

## Fixes applied (2026-07-24)

Four bugs had to be fixed for the three checks to exit cleanly. The first two
are the dango hang and its gen-99 companion; the last two were latent bugs that
the hang had masked (the checks never ran far enough to hit them).

1. **dango terminates in non-interactive boots** (`kernel/src/bootstrap.rs`).
   Rather than script each generation, the kernel now installs a single Escape
   (`\x1b`) input script by default for every boot, skipped only when the boot
   is explicitly interactive (`SLIME_INTERACTIVE=1`) or a check already
   installed its own scripted input (gen-7 dango_check, gen-9 powerbox). dango
   sees the Escape, returns, and its `exit(0)` cascades PEER_DEAD through
   console/spawn-service/filesystem-service, draining the ready queue so
   `on_idle` fires. `just run`/`run_release`/`run_gui`/`monitor`/`debug_server`
   now pass `SLIME_INTERACTIVE=1` so a real interactive session is unaffected.
   Reproduction note: the hang is universal — gen-1 (storage checks), gen-6
   (directory), and gen-99 (bootstate/rollback) all wedged; per-generation
   scripts would not have scaled, hence the default-Escape approach.

2. **gen-99 storage-probe tolerates the no-disk case**
   (`components/bins/src/bin/init.rs`). With no block device attached,
   bootstrap hands init an `ObjectStore` fallback in the storage slot, so the
   storage-probe `BLOCK_READ` derive is rejected with `BadCapability`. init now
   uses `spawn_optional_storage`, which treats that specific rejection as the
   absent-storage case and continues (the kernel's `on_idle` already tolerates
   an absent storage-probe), instead of `exit(1)` before generation-manager.

3. **Model attempt/sequence bounds realigned**
   (`contracts/bootstate/model/bootstate.zt`). Commit `e5e9531` raised
   `maxAttempts` 2→3 (to match the manifest's `bootAttempts=3`) but left
   `maxSequence=5`. The full three-attempt exhaustion walk (stage-pending at
   seq 3, then consume 3→2, 2→1, 1→0 at seqs 4/5/6) needs sequence 6, so the
   model could not reach the hardware's final `consume-attempt (1→0)`
   observation and `bootstate_trace_check` rejected a legitimate trace. Bumped
   `maxSequence` to 6. The exhaustive `bootstate_model_check` still passes
   (all 4 scenarios, including the three deliberate-violation witnesses).

4. **Generation 1 builds as its own number** (`scripts/build-generation.py`).
   Commit `2fcaea5` changed the known-good generation-1 components from
   `build_rust_components(1)` to `build_rust_components(policy_number)`, so a
   single build baked `SLIME_GENERATION_NUMBER=99` into *both* gen-1 and gen-2.
   The known-good recovery boot (runtime generation 1) then ran gen-99's
   failing-pending path and reported `Unhealthy`, so `rollback_check`'s final
   recovery boot exited `Failed`. gen-1 components are now built as number 1
   again, except on the transfer-receiver path where generation 1 legitimately
   *is* the policy-numbered receiver generation.

### Verification (all clean, release kernel under TCG)

| Check | Result |
|---|---|
| `bootstate_trace_check` | exit 0 — 3 durable transitions conform to the models |
| `rollback_check` | exit 0 — failing pending returned to known-good |
| `directory_check` | exit 0 — capability/namespace check ok |
| `dango_check` | exit 0 (regression guard) |
| `powerbox_check` | exit 0 (regression guard) |
| `transfer_check` | exit 0 (touched build path) |
| `bootstate_model_check` | exit 0 — all 4 scenarios |
| kernel `just test` | exit 0 — 21 test binaries |
| fmt (kernel + components) | clean |
| clippy (kernel + components) | clean, `-D warnings` |

### Out of scope: pre-existing `generation_cmd_check` failure

`generation_cmd_check` fails on its negative scenario (bad-closure/bad-release):
init's `spawn_and_wait(23, …)` for generation-stage calls `exit(1)` when
generation-stage *legitimately rejects* the malformed closure, so init aborts
before printing `[init] negative generation scenario complete`, and the boot
exits `Failed`. Confirmed pre-existing by stashing all four fixes and re-running
on the baseline — identical failure. It is unrelated to the dango hang (gen-8
does not spawn interactive dango) and is left untouched here; the fix belongs
in init's negative-scenario handling (expect an Exit(1) from generation-stage
rather than treating it as fatal).
