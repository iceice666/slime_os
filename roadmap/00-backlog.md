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
sub-slice gate passes, but three boot-path gates are red and several C7 exit
conditions are not observable on the live path. Evidence and bisect:
`devlog/2026-07-26-c7-audit/`. `roadmap/02-core-runtime.md` and
`roadmap/README.md` have been corrected to reopen C7; B3 and B4 must close
before the milestone claim is restored or C8 opens._

### B3 — C7.5 wedges every full-graph boot (no `on_idle`, no QEMU exit)

**Problem:** Since the C7.5 loan/return lifecycle landed, every boot that
launches the full component graph hangs instead of draining its ready queue.
`transfer_check` stalls after `[init] generation transfer installed`;
`spawn_service_check` and `dango_check` stall after `[init] spawn graph
launched`. `on_idle` is the only path to `exit_qemu`, so the guest never exits
and each gate dies on its outer timeout. This is the same observable class as
resolved item B2, and it breaks the green-suite precondition for C8.

**Evidence:** Bisected one gate per worktree on `x86_64-qemu-virtio`. `just
transfer_check` passes at C7.2 `991dcbb`, C7.3 `ed49fb5`, and C7.4 `928389e`;
it wedges at C7.5 `ca15764` and at HEAD `2384bea`. `just spawn_service_check`
passes at `928389e` and wedges at `ca15764` and HEAD, reproduced three times
including on an otherwise idle machine. Not a timeout-tuning artifact: raising
the inner QEMU timeout in `scripts/check/check-transfer.py` from 60 s to 600 s
still wedged. `git diff --stat ca15764 HEAD -- kernel/src components
boot-contracts/src` is empty, so C7.6 and C7.7 are not implicated; the defect
is inside the C7.5 kernel diff.

**Root cause:** Not yet isolated — only narrowed. The reclamation path is the
obvious suspect (`task::terminate` calls `SHARED_BUFFER_TABLE.lock()
.reclaim_owner(id)` at `kernel/src/task/mod.rs:832` while `SCHEDULER` is held,
and C7.5 rewrote `reclaim_owner` to settle loans and tear down mappings under
that lock), but it is unlikely on its own: no `SharedBufferFactory` is minted
and every holder is deny-by-default (B4), so the shared-buffer, mapping, and
loan tables are all empty during these boots and `reclaim_owner` iterates
nothing. That points instead at C7.5's capability-surface changes — the new
`KernelObject::SharedBufferLoan(BufferLoan)` variant widens `KernelObject`,
hence `Capability`, hence the `[Option<Capability>; MAX_CAPS]` table and
`Task` — which is the same shape as the `copy_from_current` scratch-array
overflow that the B2 fix had to repair after the `u64`-rights widening.

**Proposed fix:** Confirm the mechanism before patching: bisect the C7.5 diff
itself (capability/rights surface vs. `shared_buffer.rs` rewrite), and
establish whether the wedge is a task left non-`Ready`, a lock-order stall, or
a fault swallowed into a halt loop — serial output stops mid-graph, after
`[init] spawn graph launched` but before any `idle-blocked` line. Then restore
the drain. Separately, add the full-graph boot gates back to the C7
verification set: C7.1's devlog lists `just transfer_check` as direct
evidence, C7.5's does not, which is exactly how a kernel-lifecycle regression
passed review on the shared-buffer unit gates alone.

**Exit condition:** `just transfer_check`, `just spawn_service_check`, and
`just dango_check` each reach their success line and exit QEMU `Success` at
HEAD, with `just test`, `just generation_check`, `just contracts_check`, `just
fmt_check`, and `just lint` still clean.

### B4 — the C7 shared-buffer plane is dormant on the live boot path

**Problem:** Nothing in a running system can allocate a shared buffer. No
generation declares a `shared-buffer-budget/v1` resource, so every component
launches with `HolderQuota::DENY`; no manifest grants `bufferCreate` or
`bufferLoan`; and the kernel never mints a `SharedBufferFactory`. C7.3's exit
condition ("Two holders receive distinct generation-declared budgets") and
C7.7's ("Two isolated components exchange and return a payload...") are
therefore not observable on the live path — they hold only inside the kernel
test harness. The C7.2, C7.3, and C7.4 devlogs each recorded this wiring as
deferred to C7.7; C7.7 closed with "Open risks: None" without doing it, and
the deferral disappeared from the roadmap rather than being resolved.

**Evidence:** Parsed `/tmp/slime-os-generation-check-a/generation-1.bin` built
by `just generation_check`: 21 objects, zero of kind `KIND_RESOURCE` (4). The
one `SLIMESB` byte match in the file falls at offset 248756, inside the kernel
object's range (72347..639962), not in an object payload.
`scripts/build/build-generation.py` has no budget emitter, and
`contracts/generation/v1/fixtures/valid.zti` declares no budget stanza and no
`bufferCreate`/`bufferLoan` grant (all 26 `rights = [...]` lines checked).
`kernel/src/runtime/bootstrap.rs` mints `EndpointFactory` (:371, :517) and
`Input` (:422) but never `SharedBufferFactory`.

**Proposed fix:** Emit the budget as a real generation object: add a
`KIND_RESOURCE` budget emitter to `build-generation.py` (holder entries sorted
by `holder_identity` and unique, per `SharedBufferBudget::decode`), declare
per-holder quotas plus a factory grant for the two participating components in
the manifest fixture, and mint `SharedBufferFactory` in `bootstrap.rs` for the
granted components so `set_shared_buffer_quota` resolves a real quota instead
of `DENY`. Generation identities change, so rebuild the pinned fixtures and
re-run the determinism check.

**Exit condition:** A built generation contains exactly one `KIND_RESOURCE`
budget object that `crate::generation::decode` validates; two named components
boot with distinct non-`DENY` quotas and one of them allocates a shared buffer
through its factory grant; `just contracts_check` and `just generation_check`
pass with two byte-identical builds.

### B5 — no C7 gate exercises the syscall layer or real components

**Problem:** All nine `SYS_SHARED_BUFFER_*` syscalls are unreachable from
every test. The gates call `SharedBufferTable` methods directly on locally
constructed tables and never touch the global `SHARED_BUFFER_TABLE`, so the
rights gates, the `available_slots()` pre-checks, the create-insert-failure
rollback (`kernel/src/syscall/mod.rs:604-611`), and the loan-insert-failure
revoke (`:820-825`) have zero coverage. C7.7's "two isolated components" are
two `u64` constants, and its "peer death" is a direct `reclaim_owner` call, so
the real reclamation wiring in `task::terminate` is never executed by the gate
that claims it. This is what let B3 through.

**Evidence:** `grep 'dispatch|UserFrame|sys_'` over `kernel/tests/` returns no
matches; `grep SHARED_BUFFER_TABLE` over `kernel/tests/` returns no matches,
while `SharedBufferTable::new()` appears 33 times. `kernel/tests/
sample_plane.rs:57-58` defines `LENDER`/`RECEIVER` as bare `u64` constants and
the file never mentions `spawn` or `task::`; `:462` calls
`buffers.reclaim_owner(RECEIVER)` in place of a termination. Compare
`kernel/tests/isolation.rs:174-189`, which spawns real tasks.

**Proposed fix:** Promote the C7.7 gate to the milestone it claims: spawn two
real components holding granted factory/loan capabilities (available once B4
lands), move the descriptor and payload through the actual syscalls, and drive
reclamation by terminating a task rather than calling the table. Add negative
syscall cases for a missing right and a full capability table so the denial
and rollback arms are covered.

**Exit condition:** `just sample_plane_check` spawns two real tasks, moves a
payload larger than `MAX_MSG` through `SYS_SHARED_BUFFER_*` and a real
channel, and reclaims every charge via task termination; each shared-buffer
syscall has at least one authorized and one denied case.

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
