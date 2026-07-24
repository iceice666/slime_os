# Stage-0 boot-check hangs: stack overflow and dango REPL

| Field | Value |
|---|---|
| Date | 2026-07-24 |
| Status | Verified |
| Scope | `stage0`, `kernel/memory` (vmm), `kernel/bootstrap` (scheduler idle exit), `components/dango`, `components/init`, `contracts/bootstate` model, generation build tooling, QEMU check scripts |
| Trigger | `boot-contracts` growth (`660f703`, `8a73ff1`); dango interactive runtime (`f7d63e8`); model bump (`e5e9531`); gen-1 build change (`2fcaea5`) |
| Baseline | Stage-0 checks previously reached `[generation] vertical slice healthy` and exited QEMU |

## Summary

A cluster of stage-0 boot checks (`bootstate_trace_check`, `rollback_check`,
`directory_check`, and the user-reported `recovery_check`/`transfer_check`
`leaf_flags_in` faults) hung or failed. Two primary regressions plus two latent
bugs stacked on top of each other:

1. **Kernel stack overflow into the PML4** — a debug-build boot path overflowed
   the unguarded 64 KiB stage-0 stack into the page-table root one page below
   it, triple-faulting; a shallow trample surfaced as a `leaf_flags_in`
   page-fault hang.
2. **dango became a persistent busy-poll REPL** — after the crash was fixed the
   guests booted healthy but never exited QEMU, because the scheduler only
   reaches `exit_qemu` via `on_idle` (all tasks terminated) and dango
   busy-polls the keyboard forever.
3. **gen-99 init death** — no-disk boots gave init an `ObjectStore` fallback
   cap, so the storage-probe `BLOCK_READ` derive was rejected and init aborted
   before generation-manager.
4. **Two latent model/build bugs** the hang had masked — a `maxSequence` bound
   too small for `maxAttempts=3`, and generation-1 components baked with the
   pending generation's number.

All four are fixed and the affected checks now exit cleanly. One unrelated
pre-existing defect (`generation_cmd_check`) was found and filed as backlog B1.

## Observable symptom

- Command: `just bootstate_trace_check` / `just rollback_check` / `just directory_check`
- Expected: guest boots, exits QEMU, check reports `ok` / `vertical slice healthy`.
- Observed (pre-fix, phase 1): infinite triple-fault reboot loop; check never
  returns (scripts had no subprocess timeout). Some runs surfaced as
  `[kernel fault] vec=14 rip∈leaf_flags_in` + `hlt_loop`.
- Observed (pre-fix, phase 2, after stack fix): healthy boot to idle `dango>`,
  then `EXIT=124` (subprocess timeout). gen-99 additionally logged
  `[spawn] rejected BadCapability` / `[init] spawn failed slot=8`.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `qemu -d int`: instruction-fetch `#PF` with `CR2==RIP`, escalating `#PF→#DF→triple fault` | Fault is on the kernel's own code; handlers also unfetchable |
| 2 | `CR3=0x0e01b000` lies between `RSP=0x0e018bb0` and `RBP=0x0e0204c8` | Call stack spans the PML4 frame |
| 3 | Stage-0 allocates 64 KiB stack then `PageTables::new()`; OVMF allocates top-down | PML4 lands one page below the unguarded stack base |
| 4 | lldb write-watch on `0x0e01bff8` fires from `memmove (crt.rs:56)`, `rsp≈13.5 KB` below base | Kernel's own deep boot-contracts frames overflow into the PML4 |
| 5 | Shallow trample writes garbage into PML4[384] (ECAM scratch); `leaf_flags_in` derefs it unchecked | Explains the `leaf_flags_in` "page-fault hang" as the first reader, not the corruptor |
| 6 | After stack fix, all three checks reach idle `dango>` and time out | Second, independent problem: guest never self-terminates |
| 7 | `on_idle` is the sole `exit_qemu` path; `TaskState` has no `Blocked`; dango loops `input_read→Ok(None)→yield_now` | Persistent dango keeps the ready queue non-empty; `on_idle` never fires |
| 8 | gen-7 dango_check exits (EXIT=0) — it installs an Escape input script; gen-6/gen-99 install none | Fix direction: drive dango to Escape in headless boots |
| 9 | gen-99 logs `BadCapability` on storage-probe slot 8 with no disk | Second regression: `ObjectStore` fallback cap can't derive `BLOCK_READ` |
| 10 | After dango+init fixes, `bootstate_trace_check` runs to completion but oracle rejects `consume-attempt (1→0)` | Latent model bug: `maxSequence=5` too small for `maxAttempts=3` |
| 11 | After model fix, `rollback_check` known-good boot reports `Unhealthy`, exits `Failed` | Latent build bug: gen-1 baked with `policy_number` (99) |

Decisive chain only; see `transcript.txt` (sibling) for exploratory detail.

## Root cause

- **Stack overflow:** stage-0 mapped the boot stack with no guard page directly
  above the PML4; the identity/direct maps made the whole region silently
  writable, so stack growth past the base overwrote the page-table root. The
  fault RIP (`leaf_flags_in`) named the first reader of the corrupted table,
  not the corruptor. Root invariant violated: a stack overflow must fault, not
  silently corrupt state.
- **dango hang:** `f7d63e8` turned dango into a persistent keyboard REPL that
  busy-polls (`Ok(None) → yield_now`), staying perpetually `Ready`. The
  scheduler reaches `exit_qemu` only through `on_idle`, which fires only when
  the ready queue drains (every task `Terminated`). No `Blocked` state exists,
  so a never-terminating dango wedges every headless full-graph boot.
- **gen-99 init death:** `f7d63e8` replaced init's unconditional block cap with
  an `ObjectStore/RIGHT_STORE_READ` fallback for no-disk boots; storage-probe's
  `BLOCK_READ` derive from it is rejected, and `spawn_or_fail` aborted init.
- **Model bound:** `e5e9531` raised `maxAttempts` 2→3 but left `maxSequence=5`;
  the three-attempt exhaustion walk needs sequence 6, so the model could not
  reach the hardware's final `consume-attempt (1→0)`.
- **gen-1 build:** `2fcaea5` built generation-1 components with `policy_number`,
  baking the pending number into the known-good generation, so the recovery
  boot ran the failing-pending path.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `stage0/src/main.rs` | Map boot stack into a dedicated guarded virtual window; raise to 256 KiB | Stack overflow faults deterministically at an unmapped guard |
| `kernel/src/memory/vmm.rs`, `memory/mod.rs` | Walkers reject PS-bit and out-of-RAM entries | Table corruption yields a typed error, not a fault inside the walker |
| `kernel/src/bootstrap.rs` | Default Escape input script for non-interactive boots (skip `SLIME_INTERACTIVE=1` and self-scripted checks) | dango closes so `on_idle` can drain the ready queue |
| `components/bins/src/bin/init.rs` | `spawn_optional_storage` tolerates no-disk `BadCapability` | init reaches generation-manager on diskless boots |
| `Justfile` | `run`/`run_release`/`run_gui`/`monitor`/`debug_server` set `SLIME_INTERACTIVE=1` | Real interactive dango sessions stay alive |
| `contracts/bootstate/model/bootstate.zt` | `maxSequence` 5→6 | Model reaches the full three-attempt exhaustion trace |
| `scripts/build-generation.py` | Build gen-1 as number 1 (except transfer-receiver) | Known-good generation runs the known-good path |
| `scripts/check-*.py` | Subprocess timeout + release-kernel path | A wedged guest fails loudly against the built binary |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Stack overflow trampling page tables | `just bootstate_trace_check`, `just recovery_check` | Triple-fault reboot loop / `leaf_flags_in` fault |
| dango wedges headless boots | `just directory_check`, `just bootstate_trace_check`, `just rollback_check` | Timeout at idle `dango>` (EXIT=124) |
| No-disk init death | `just bootstate_trace_check`, `just rollback_check` | `[init] spawn failed slot=8` |
| Model/hardware trace divergence | `just bootstate_trace_check`, `just bootstate_model_check` | `consume-attempt post-state is not reachable` |
| Known-good boots the wrong generation | `just rollback_check` | Final recovery boot exits `Failed` / `Unhealthy` |
| Interactive dango killed by the fix | `just dango_check`, `just powerbox_check` | Missing transcript / early exit |

## Verification

All direct evidence, this machine, release kernel under TCG.

| Command/scenario | Result | Evidence class |
|---|---|---|
| `bootstate_trace_check` | exit 0 — 3 durable transitions conform to models | Direct |
| `rollback_check` | exit 0 — failing pending returned to known-good | Direct |
| `directory_check` | exit 0 — capability/namespace check ok | Direct |
| `dango_check` | exit 0 | Direct |
| `powerbox_check` | exit 0 | Direct |
| `transfer_check` | exit 0 | Direct |
| `bootstate_model_check` | exit 0 — all 4 scenarios | Direct |
| kernel `just test` | exit 0 — 21 test binaries | Direct |
| fmt (kernel + components) | clean | Direct |
| clippy (kernel + components), `-D warnings` | clean | Direct |

## Decisions

- Decision: fix the dango hang by installing a default Escape input script for
  non-interactive boots, not by adding a `Blocked` task state.
- Rationale: keeps `on_idle`'s all-tasks-terminated invariant intact, needs no
  scheduler change, and scales to every generation (the hang is universal, so
  per-generation scripting would not have scaled).
- Rejected alternative: a `Blocked` scheduler state that parks tasks waiting on
  input/IPC. Correct long-term design but changes scheduler semantics and every
  poll-and-yield callsite; deferred.

## Open risks and follow-ups

- [ ] Backlog B1 (`roadmap/00-backlog.md`): `generation_cmd_check` negative
      scenario aborts init when generation-stage legitimately rejects a
      malformed closure. Pre-existing (confirmed on baseline with all fixes
      stashed); unrelated to the dango hang. Not fixed here.
- [ ] `[INFERENCE]` A `Blocked` task state remains the correct long-term fix for
      the busy-poll pathology; the Escape-script approach only un-wedges checks.
- [ ] Physical Framework evidence for the affected stage-0 paths remains M5.7
      scope; all evidence here is QEMU/TCG only.

## Artifacts and provenance

- Focused reports (siblings): `stage0-stack-overflow-report.md`,
  `dango-shell-hang-report.md`.
- Raw transcript (sibling): `transcript.txt`.
- Serial captures during investigation: `/tmp/gen6-serial.log`,
  `/tmp/gen99-serial.log`, `/tmp/gen7-serial.log` (ephemeral, not committed).
- Related roadmap items: M5.6/M5.6c (`roadmap/01-foundations.md`), M6.3/M6.4
  (dango/directory), backlog B1 (`roadmap/00-backlog.md`).
- Fix commits: `2edd8b2`, `b58762c`, `12ccd26`, `0e2a002`, `437dcad`.
