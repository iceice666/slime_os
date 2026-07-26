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

_Opened 2026-07-26 by a full C7 audit (C7.1–C7.7) at `2384bea`. Every C7
sub-slice gate passes; the audit found a boot regression and several exit
conditions that were provable only in-harness. Evidence and bisect:
`devlog/2026-07-26-c7-audit/`. B3 (the C7.5 full-graph boot wedge) and B4 (the
dormant shared-buffer plane) are resolved, so C7's blocking items are cleared
and C8 may open; B5–B8 remain as evidence and hygiene debt on the C7 surface._

### B5 — no C7 gate exercises the syscall layer or real components

**Problem:** No *test* reaches any `SYS_SHARED_BUFFER_*` syscall. The gates call
`SharedBufferTable` methods directly on locally constructed tables and never
touch the global `SHARED_BUFFER_TABLE`, so the rights gates, the
`available_slots()` pre-checks, the create-insert-failure rollback
(`kernel/src/syscall/mod.rs:604-611`), and the loan-insert-failure revoke
(`:820-825`) have no test coverage. C7.7's "two isolated components" are two
`u64` constants, and its "peer death" is a direct `reclaim_owner` call, so the
real reclamation wiring in `task::terminate` is never executed by the gate that
claims it. This is what let B3 through.

**Partly addressed 2026-07-26 (B4).** `slime_rt` now wraps
`create`/`map`/`unmap`/`seal`/`release`, and the dango and spawn-service
startup probes exercise all five against the real kernel on every boot, so
those paths are no longer unreachable from userspace. What remains: the four
loan syscalls (`LOAN`/`LOAN_MAP`/`RETURN`/`REVOKE`) still have no wrapper and
no caller; no *test* drives any syscall; and the C7.7 gate still composes owner
ids rather than tasks.

**Evidence:** `grep 'dispatch|UserFrame|sys_'` over `kernel/tests/` returns no
matches; `grep SHARED_BUFFER_TABLE` over `kernel/tests/` returns no matches,
while `SharedBufferTable::new()` appears 33 times. `kernel/tests/
sample_plane.rs:57-58` defines `LENDER`/`RECEIVER` as bare `u64` constants and
the file never mentions `spawn` or `task::`; `:462` calls
`buffers.reclaim_owner(RECEIVER)` in place of a termination. Compare
`kernel/tests/isolation.rs:174-189`, which spawns real tasks.

**Proposed fix:** Add the four missing loan wrappers to `slime_rt`, then promote
the C7.7 gate to the milestone it claims: spawn two real components holding
granted factory/loan capabilities (the factory grant landed with B4), move the
descriptor and payload through the actual syscalls, and drive reclamation by
terminating a task rather than calling the table. Add negative syscall cases for
a missing right and a full capability table so the denial and rollback arms are
covered.

**Exit condition:** `just sample_plane_check` spawns two real tasks, moves a
payload larger than `MAX_MSG` through `SYS_SHARED_BUFFER_*` and a real
channel, and reclaims every charge via task termination; each of the nine
shared-buffer syscalls has at least one authorized and one denied case.

### B6 — the retained-v2 "still boots" claim is proven only as decode

**Problem:** C7.1's exit condition states that a retained v2 known-good
artifact "still decodes **and boots**". Only decode is proven. No v2
generation is ever booted, so the rollback window's boot path is unverified.

**Evidence:** `scripts/lib/boot_contracts.py:7-8` pins
`GENERATION_MAGIC = b"SLIMEG3\0"` / version 3, so the builder emits v3 only.
The sole v2 artifacts are hand-built in memory: `boot-contracts/src/
generation.rs:786` (`retained_v2_generation_still_decodes`) and
`kernel/tests/sample_plane.rs:564` (`build_v2_known_good`). C7.7's
`retained_v2_known_good_decode_is_unaffected` is honestly named as a decode
probe; `roadmap/02-core-runtime.md:38,63` upgrades it to "boots".

**Proposed fix:** Either build a v2 known-good generation into a bootstore
fixture and boot it under QEMU during the rollback window, or amend C7.1's
exit condition and status line to claim decode-and-verify only, and record the
boot arm as an explicit deferral with its reason.

**Exit condition:** Either a v2 known-good generation boots to a healthy slice
under a named `just` target, or `roadmap/02-core-runtime.md` states the
decode-only scope and the deferral is recorded here.

### B7 — the `RIGHT_MAP` rename never reached the manifest vocabulary

**Problem:** C7.1's deliverable was to "replace the grandfathered generic
`RIGHT_MAP` name with an object-specific shared-buffer map right". The kernel
constant was renamed to `RIGHT_BUFFER_MAP`, but the host-facing manifest right
is still spelled `map`, so generation authors keep writing the generic name
for a buffer-specific authority.

**Evidence:** `scripts/build/build-generation.py:105` maps `"map": 1 << 9`,
alongside object-specific siblings such as `"bufferWrite"`, `"bufferCreate"`,
and `"bufferLoan"`. `kernel/src/capability/mod.rs:39` defines the same bit as
`RIGHT_BUFFER_MAP`.

**Proposed fix:** Rename the manifest key to `bufferMap` in the builder's
`RIGHT` table, update any manifest fixture that uses it, and re-run
`generation_check`/`contracts_check` for the identity change. No wire-format
change: the bit value is unchanged.

**Exit condition:** No `"map"` key remains in the builder rights table, the
manifest fixtures build byte-identically across two runs, and
`just framework_safety_check` stays clean.

### B8 — budget validation bounds each holder but never the aggregate

**Problem:** `SharedBufferBudget::validate_against` checks each holder's
quota against the fixed kernel ceilings but never sums holders, so a budget
may promise N holders `MAX_TOTAL_PAGES` each. That is over-commitment rather
than a per-holder impossibility, and `SharedBufferTable::create` still
enforces the real global ceiling, so it is not exploitable — but
`roadmap/02-core-runtime.md:104` says decode rejects "globally impossible"
limits, and an aggregate over-commit is exactly the case a reader expects that
phrase to cover. Left as is, the first holder to run wins and later holders
fail with `BytesExhausted` at runtime rather than at decode.

**Evidence:** `boot-contracts/src/shared_buffer_budget.rs:116-148` loops
per-entry with no accumulator; its comment at :141-145 explicitly notes
`max_buffer_pages` is retained only "for symmetry". The lib tests at :298-327
cover per-holder impossibility only.

**Proposed fix:** Decide and record the intent. Either sum `byte_pages`,
`buffer_count`, `mapping_count`, and `loan_count` across holders and reject a
budget whose total exceeds the corresponding kernel ceiling, or keep
over-commitment legal and reword the roadmap deliverable to say per-holder
impossibility. The first is stricter and matches the wording; the second is
the smaller change and keeps deliberate over-subscription available.

**Exit condition:** The chosen rule is implemented with a lib test covering an
aggregate over-commit, and `roadmap/02-core-runtime.md` describes the same
rule the code enforces.

## Resolved

### B4 — the C7 shared-buffer plane was dormant on the live boot path

**Resolved:** 2026-07-26. See `devlog/2026-07-26-b4-live-shared-buffer-budget/`.

**Problem:** Nothing in a running system could allocate a shared buffer. No
generation declared a `shared-buffer-budget/v1` resource, so every component
launched with `HolderQuota::DENY`; no manifest granted `bufferCreate`; the
kernel never minted a `SharedBufferFactory`; and `slime_rt` had no wrapper for
any shared-buffer syscall. C7.3's exit condition ("two holders receive distinct
generation-declared budgets") therefore held only inside the kernel test
harness. C7.2/C7.3/C7.4 each deferred this wiring to C7.7, which closed without
doing it.

**Evidence:** The built `generation-1.bin` held 21 objects and zero of kind
`KIND_RESOURCE`; the one `SLIMESB` match sat inside the kernel object's byte
range, not an object payload. No `bufferCreate` grant in the manifest fixture;
`bootstrap.rs` minted `EndpointFactory` and `Input` but never
`SharedBufferFactory`.

**Fix:** Emit the budget as a digest-authenticated `KIND_RESOURCE` object from
`build-generation.py` (entries sorted by `holder_identity` and duplicate-checked,
as `SharedBufferBudget::decode` requires); declare per-holder quotas and two
`bufferCreate` grants in the manifest; mint one transferable
`SharedBufferFactory` in `bootstrap.rs` at a fixed slot ahead of the optional
transfer block (renumbering the transfer slots to 41/42) and validate both
grants with `require_grant`; add the five missing `slime_rt` wrappers; and run a
bounded create/map/write/seal/unmap/release self-check at dango and
spawn-service startup so a normal boot proves its own quota.

**Exit condition (observed):** A built generation contains exactly one
`KIND_RESOURCE` budget object (128 bytes, digest verified, magic `SLIMESB\0`,
two holders sorted by identity) that `crate::generation::decode` validates.
A normal boot prints `[generation] shared-buffer factory grants valid`,
`[dango] shared-buffer quota live`, and `[spawn-service] shared-buffer quota
live`, then `vertical slice healthy`. The new
`booted_generation_declares_distinct_holder_budgets` case decodes the booted
generation and asserts two distinct non-`DENY` quotas with an absent component
denied. `just generation_check` produces two byte-identical builds; `just
test`, all six C7 sub-slice gates (8/8/8/7/4/5), `just dango_check`, `just
transfer_check`, `just generation_cmd_check`, `just contracts_check`, `just
framework_safety_check`, and fmt/lint (with `_components`) are clean.

**Follow-up:** B5 is partly addressed — five syscalls are now exercised on a
live boot, but the four loan syscalls still have no wrapper and no test drives
any syscall.

### B3 — C7.5 wedged every full-graph boot (kernel-stack overflow)

**Resolved:** 2026-07-26. See
`devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/`.

**Problem:** From C7.5 onward every boot that launched the full component graph
hung instead of draining its ready queue. `transfer_check` stalled after
`[init] generation transfer installed`; `spawn_service_check` and `dango_check`
stalled after `[init] spawn graph launched`. `on_idle` is the only path to
`exit_qemu`, so the guest never exited and each gate died on its timeout — the
same observable class as B2, but an unrelated cause.

**Evidence:** Bisected one gate per worktree: `just transfer_check` passed at
C7.2 `991dcbb`, C7.3 `ed49fb5`, and C7.4 `928389e`, and wedged at C7.5
`ca15764` and HEAD; `just spawn_service_check` passed at `928389e` and wedged
at `ca15764` and HEAD. Not timeout tuning: raising the inner QEMU timeout from
60 s to 600 s still wedged. `git diff --stat ca15764 HEAD -- kernel/src` is
empty, so C7.6/C7.7 were not implicated. Full transcript in
`devlog/2026-07-26-c7-audit/transcript.txt` §3–§4.

**Root cause:** Kernel-stack overflow, not the reclamation logic first
suspected. C7.5 grew `SharedBufferTable` to 10520 bytes of fixed arrays
(`loans: [Option<Loan>; 64]` plus a widened `Mapping`), and the table was
published through a `LazyLock`, whose initializer builds the value on whichever
stack first touches the static. Because no `SharedBufferFactory` is minted on
the live path (B4), the first touch is `SHARED_BUFFER_TABLE.lock()` inside
`task::terminate` (`kernel/src/task/mod.rs:832`) — on a 32 KiB task kernel stack
allocated as a plain boxed slice with no guard page. The 10 KiB temporary
overflowed it while `SCHEDULER` was held, corrupting adjacent memory silently
rather than faulting, so the boot wedged with no panic. Confirmed by raising
`KERNEL_STACK_SIZE` to 128 KiB with no other change, which made the gate pass.

**Fix:** Replaced the `LazyLock` with a plain `const`-initialized
`Mutex<SharedBufferTable>` static, matching `FRAME_ALLOCATOR` and the
`drivers/input.rs` tables. `SharedBufferTable::new()` was already a `const fn`,
so the laziness bought nothing; const-initializing places the table in `.bss`
and removes the stack temporary. The diagnostic stack bump was reverted. Added
a compile-time assertion that `size_of::<SharedBufferTable>() * 2 <
KERNEL_STACK_SIZE`, verified to fire by temporarily setting `MAX_LOANS = 1024`.

**Exit condition (observed):** `just transfer_check` (install, pending boot,
promotion, rollback retention), `just spawn_service_check`, and `just
dango_check` all reach their success lines and exit QEMU `Success` at the stock
32 KiB stack. `just test` (160 assertions), all six C7 sub-slice gates (8/7/8/7/
4/5), `just generation_cmd_check`, `just contracts_check`, `just
generation_check`, `just framework_safety_check`, `just fmt_check`, `just
lint`, `just fmt_check_components`, and `just lint_components` are clean.

**Follow-up:** Task kernel stacks still have no guard page, so a future
overflow will again corrupt memory silently instead of faulting. This fix
removes the trigger, not the class.

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
builder `scripts/check/check-generation-commands.py`. `build_fixture` corrupted
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
