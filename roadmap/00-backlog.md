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

B10 and B11 are open. They come from one survey, recorded in
`devlog/2026-07-31-boot-layout-positional-coupling/`, and they are ordered: B11
depends on B10's named-grant resolution, so B10 lands first.

### B10 — init's capability layout is a positional convention, so boot paths are selected at kernel compile time

**Problem:** `launch_init` builds init's capability vector by writing fixed
indices (`caps[46] = ...`) rather than resolving named grants the generation
declares. `MAX_CAPS = 64`, and the vector was 61 occupied before C8.10, so a new
participant set cannot be appended — it must squat on another profile's slots or
fork a whole `launch_*_init`. Both happened. The gates that read those slots read
them positionally, which is why the layout cannot simply be renumbered.

The escape hatch chosen instead was compile-time selection: `option_env!` reads a
`SLIME_*_CHECK` flag and compares `generation.number` against a literal. Because
`option_env!` is evaluated at compile time and Cargo tracks these as build inputs
(the kernel's dep-info records `env-dep:SLIME_DANGO_CHECK`,
`env-dep:SLIME_GENERATION_CMD_CHECK`, `env-dep:SLIME_POWERBOX_CHECK` and
siblings), each gate builds a *different kernel binary*. There is no single
kernel artifact that passes the gate suite.

This blocks P1. That milestone requires that "architecture-neutral code can be
type-checked for AArch64 without importing x86-only modules", which cannot hold
while the boot path is selected by x86-gate build flags and hardcoded generation
numbers.

**Evidence:** `kernel/src/runtime/bootstrap.rs:176-182` states the constraint
outright — the vector is "61 of `MAX_CAPS = 64` before this milestone adds
anything", the three new C8.10 roles "need nine slots against three free", and
the vector "is also the layout six passing QEMU gates read positionally — the
`caps[46] = ...` blocks below rewrite it per generation number — so renumbering
it to fit would rewrite C8.3-C8.8's evidence rather than extend it".

Counted at the commit that opened this item:

- 26 positional writes over 13 distinct slots (46-59) in `bootstrap.rs`;
- 3 `launch_*_init` forks: `launch_init` (168), `launch_fabric_boot_init` (964),
  `launch_recovery_init` (1087);
- 9 `generation.number ==` branches in `launch_init`, including
  `generation.number == 14` reassigning slots 46/47/49 under the comment that
  "the call gate reuses the executable/control slots occupied by three stream
  participants in every other generation profile", and the mutually exclusive
  call/operation profiles at lines 793 and 828 sharing one slot range;
- 21 distinct `option_env!("SLIME_*")` flags over 70 sites (18 in `kernel/src`,
  52 in `components/`);
- 11 distinct generation numbers driven by check scripts (6, 7, 8, 9, 10, 11,
  12, 13, 14, 16, 99), e.g. `check-fabric-stream.py` sets
  `SLIME_FABRIC_STREAM_CHECK=1` with number 12, `check-fabric-qos.py` sets
  `SLIME_FABRIC_QOS_CHECK=1` with 13, and `check-data-fabric-boot.py` sets
  `SLIME_FABRIC_BOOT_CHECK=1` against the kernel's `generation.number == 17`.

**Proposed fix:** Resolve init's grants by name from the generation instead of by
index in kernel source, so a profile's participant set is generation data. The
hard constraint is that every profile in use today must resolve to **the same
slot numbers it occupies now** — this is a naming layer over the existing
layout, not a renumbering, because renumbering rewrites six gates' evidence
rather than extending it. With grants named, the `option_env!` and
`generation.number` branches in `launch_init` lose their purpose and the
`launch_*_init` forks collapse.

Storage identity selection at `bootstrap.rs:571` and `bootstrap.rs:595`
(generation numbers 2, 3, 4 selecting different capabilities and a different
storage component) is the same pattern on a different axis. Decide explicitly
whether it is in scope before starting; do not leave it undecided.

Component-side flags are not assumed to fall out of this: 52 `option_env!` sites
in `components/` (9 reading `SLIME_FABRIC_VISIBILITY_CHECK` alone) make their own
build-time decisions independent of the kernel layout, and may need their own
pass.

**Exit condition:** Init's capability layout is resolved from generation-declared
names; an equivalence check demonstrates that every profile in use resolves to
the slot assignments it holds today; the `option_env!` and `generation.number`
branches in `launch_init` are gone; and the existing gates — at minimum `just
dango_check`, `just sample_plane_live_check`, `just fabric_stream_check`, `just
fabric_call_check`, `just fabric_operation_check`, `just fabric_visibility_check`,
and `just data_fabric_boot_check` — observe the results they observe today. P0/P1
name `just architecture_contract_check` and `just x86_portability_check` as
planned targets; neither exists yet, so this item must name a gate that exists
when it is claimed.

### B11 — test scaffolding is declared in the product boot generation

**Problem:** `contracts/generation/v1/fixtures/valid.zti` declares 42 components,
of which 16 are probes and scenario doubles: `storage-probe`,
`storage-fault-probe`, `storage-store-probe`, `directory-probe`,
`powerbox-probe`, `sample-lender`, `sample-receiver`, `fabric-intruder`,
`fabric-probe`, `fabric-observer`, `fabric-proxy`, `fabric-publisher-b`,
`fabric-subscriber-b`, `fabric-call-client-b`, `fabric-op-client-b`, and
`fabric-op-client-b-restart`. They are not incidental strings: they hold
`-control` endpoints and real capability grants, and `storage-probe` appears in
`requiredComponents`. There is one manifest and one shape, so verification
scaffolding and product services are declared as peers.

**Evidence:** The fixture is 1161 lines. `fabric-intruder` (line 593),
`fabric-probe` (689), and `fabric-observer` (705) each declare an object, a
component entry, and a `-control` grant; `requiredComponents` at line 1159 is
`["init"; "console"; "dango"; "storage-probe";]`.

**Proposed fix:** Extend the profile mechanism that already exists rather than
adding a second selector. `scripts/build/build-generation.py` already resolves a
named profile (`selected_profile_name` at 563, `resolve_fabric_graph` at 616,
`SLIME_FABRIC_PROFILE`), and the fixture already declares `default`,
`visibility`, and `unified` profiles — but that mechanism governs interposition
chains only, not which components a generation declares. Extending it to select
the component set lets scaffolding live in test profiles while the product
profile declares only real services. A separate test-generation file is the wrong
shape: it would duplicate the route, QoS, and budget declarations the fabric
graph already resolves, and would let the two paths drift.

**Depends on:** B10. While grants are positional, moving a component between
profiles changes slot numbers, which is exactly what the gates cannot absorb.

**Exit condition:** The product boot profile declares only components the product
needs; scaffolding participants are declared in test profiles selected by the
existing profile mechanism; and every gate that today depends on a probe still
selects it explicitly and observes its current result.

## Resolved

### B9 — terminated tasks are never reaped, so their frames never return

**Resolved:** 2026-07-28. See `devlog/2026-07-28-b9-task-frame-reclamation/`.

**Problem:** `task::terminate` marked a task `Terminated`, drained its
capabilities, and reclaimed its shared buffers, but never removed the `Task`
from the scheduler. The `Task` — and the `AddressSpace` it owns — therefore
lived for the rest of the boot, so `AddressSpace::drop` never ran. Even when it
did, that `Drop` freed only the PML4 frame and deliberately leaked every
user-half page table; the image and stack frames mapped by
`spawn_with_caps_for` had no release path at all. Every spawn permanently
consumed its image pages plus its stack pages, so a repeated spawn/exit
workload drained the frame allocator monotonically.

**Evidence:** `kernel/src/task/mod.rs` — `terminate` pushed to
`sched.terminated` and left the task in `sched.tasks`; `remove_task` was called
only from the `spawn_from_cap` capability-insert failure path.
`kernel/src/memory/address_space.rs` — `Drop` dealloc'd `self.pml4` alone, with
the comment that intermediate user-half tables "intentionally leak for the
small M2 isolation test". The per-cycle delta is no longer an inference: a boot
probe running four real spawn/release cycles before `launch_init` reported
`spawn/exit leaked: 52 frame(s) over 4 cycles` — 13 frames per cycle.

**Fix:** two gaps on one path, closed together. `vmm::free_user_half` walks
PML4 entries 0..256, freeing leaf pages then the tables that held them, and
`AddressSpace::drop` now calls it before releasing the PML4 — so every frame an
address space owns has a release path, including on the `spawn_with_caps_for`
early-return paths, which hold it as a local. `reap_terminated` gives the
scheduler a reclamation point, removing every terminated task except the one
the CPU is standing on; it runs from `schedule_next` after the switch target is
chosen. Reaping is deferred rather than immediate because `terminate` executes
on the terminating task's own kernel stack and address space. `sched.terminated`
stays a separate log, so `supervision_status` and `SYS_WAIT` still answer for a
reaped child. The kernel half (entries 256..512, shared aliases of the one
kernel hierarchy) is never touched.

**Exit condition (observed):** the boot probe reports `spawn/exit conserves
frames: 14 per cycle, 0 drift`, asserted by `just dango_check`. `just test`
passes 185 assertions including five new `task_reclamation` cases — eight-cycle
conservation, release scaling with image size, a task holding capabilities, a
rejected spawn, and the shared-buffer double-free ordering. Supervision results
stay observable after reaping, proven by `just spawn_service_check` and `just
dango_check`, whose components spawn and exit through `terminate` and the
reaper and still report a healthy slice; `just sample_plane_live_check` and
`just fabric_stream_check` are unaffected. Fault injection confirms the guards
bite: removing the `free_user_half` call makes both the harness tests and the
live probe fail, and inverting the reclaim/release order fails the double-free
test.

**Follow-up:** a task that terminates when nothing else is runnable is reaped by
the *next* scheduling event, which on the non-interactive path never comes —
`on_idle` exits QEMU. One task's frames are therefore returned to an allocator
that is about to stop existing, which is harmless today but is the residual
lag C10.4's spawn/exit measurement should quantify. The live probe covers the
release path rather than the reaper; a gate counting frames across a full
spawn/exit/reap cycle needs a userspace loop and belongs with that milestone.

### B8 — budget validation bounded each holder but never the aggregate

**Resolved:** 2026-07-26. See `devlog/2026-07-26-b7-b8-budget-hygiene/`.

**Problem:** `SharedBufferBudget::validate_against` checked each holder's quota
against the fixed kernel ceilings but never summed holders, so a budget could
promise N holders `MAX_TOTAL_PAGES` each. Not exploitable —
`SharedBufferTable::create` still enforced the real global ceiling — but the
roadmap said decode rejects "globally impossible" limits, and an aggregate
over-commit degraded a declared quota into first-come-first-served: a
late-starting component failed with `BytesExhausted` despite holding a quota the
generation promised it.

**Evidence:** `boot-contracts/src/shared_buffer_budget.rs:116-148` looped per
entry with no accumulator; its comment noted `max_buffer_pages` was retained
only "for symmetry". Lib tests covered per-holder impossibility only.

**Fix:** Chose the stricter reading, since `AGENTS.md` requires generation data
to be deterministic, bounded, and explicitly validated: `validate_against` now
sums `byte_pages`, `buffer_count`, `mapping_count`, and `loan_count` with
saturating adds and rejects any total past its kernel ceiling, so a budget that
validates is one the kernel can honour with every holder at its ceiling at once.
Also added the two per-holder bounds the check was missing — `mapping_count` and
`loan_count` against `MAX_MAPPINGS`/`MAX_LOANS`, without which a holder could
declare 200 mappings against a 64-entry table. `validate_against` grew to five
parameters; the kernel caller passes the new ceilings.

**Exit condition (observed):** `cargo test -p boot-contracts --lib` passes 24
tests, including `aggregate_over_commitment_is_rejected`,
`aggregate_buffer_mapping_and_loan_ceilings_are_enforced`, and
`per_holder_mapping_and_loan_ceilings_are_enforced`. Fault injection confirms it
bites on the live path: raising the manifest to 306 aggregate pages (> 256) made
the boot fail closed, and the real budget (18/256 pages, 5/32 buffers, 10/64
mappings, 5/64 loans) passes. `just generation_check` (two byte-identical
builds), `just contracts_check`, `just spawn_service_check`, `just
sample_plane_live_check`, `just test`, and fmt/lint are clean.

**Follow-up:** The host builder does not validate the aggregate; only the kernel
does at decode, so an over-committed manifest builds and fails at boot. That is
fail-closed and keeps one source of truth for the rule.

### B7 — the `RIGHT_MAP` rename never reached the manifest vocabulary

**Resolved:** 2026-07-26. See `devlog/2026-07-26-b7-b8-budget-hygiene/`.

**Problem:** C7.1's deliverable was to replace the grandfathered generic
`RIGHT_MAP` name with an object-specific shared-buffer map right. The kernel
constant became `RIGHT_BUFFER_MAP`, but the manifest key stayed `map`, so
generation authors kept writing a generic name for buffer-specific authority.

**Evidence:** `scripts/build/build-generation.py:112` mapped `"map": 1 << 9`
alongside object-specific siblings `bufferWrite`, `bufferCreate`, `bufferLoan`;
`kernel/src/capability/mod.rs:39` defined the same bit as `RIGHT_BUFFER_MAP`.

**Fix:** Renamed the builder key to `bufferMap`. No wire or identity change —
the bit value is unchanged and no manifest fixture referenced the old key.

**Exit condition (observed):** No `"map"` key remains in the builder rights
table; `just generation_check` produces two byte-identical builds and `just
framework_safety_check` stays clean.

### B6 — the retained-v2 "still boots" claim was proven only as decode

**Resolved:** 2026-07-26 (scope corrected + admission covered). See
`devlog/2026-07-26-b6-retained-v2-rollback-scope/`.

**Problem:** C7.1's exit condition stated that a retained v2 known-good artifact
"still decodes **and boots**". Only decode was proven; no v2 generation was ever
booted.

**Evidence:** `scripts/lib/boot_contracts.py:7-8` pins `GENERATION_MAGIC =
b"SLIMEG3\0"` / version 3, so the builder emits v3 only. The sole v2 artifacts
were hand-built in memory (`boot-contracts/src/generation.rs`,
`kernel/tests/sample_plane.rs:564`).

**Resolution:** The boot arm is not merely unproven, it is unconstructible from
this tree, and investigating why closed a more interesting question.
`stage0::verify_kernel` (`stage0/src/lib.rs:320-325`) resolves
`generation.kernel_object`, so each generation embeds and boots its **own**
kernel. A retained v2 generation therefore runs its v2-era kernel — which is
also why this tree's v3-only rights cannot break the rollback window, despite
`bufferCreate` (bit 24) lying outside v2's 24-bit rights space and
`require_grant` being unconditional. Any "v2 boot" staged today would pair a v2
manifest with a v3-era kernel: a configuration that has never existed.

Covered the provable and load-bearing part instead — the stage-0 admission
chain, which had no coverage. Two `boot-contracts` tests were added:
`retained_v2_generation_passes_stage0_admission` (identity seal, kernel object,
bootstrap component, tamper detection) and
`retained_v2_authority_manifest_is_width_stable`, which pins the 32-bit v2
authority hash. That second one guards a real hazard: `release.rs:163` binds a
signed release to `authority_manifest_identity`, so losing the version branch
would fail every retained v2 release while every gate stayed green. C7.1's
status and exit condition now claim decode + release authorization + admission,
and state why the boot arm cannot be staged.

**Exit condition (observed):** `cargo test -p boot-contracts --lib` passes 21
tests (19 prior + 2 new). Fault injection confirms the guard bites: removing the
v2 branch from `authority_manifest_identity` so it hashes at 64-bit made
`retained_v2_authority_manifest_is_width_stable` fail, and the branch was
restored. `just contracts_check`, `just generation_check`, and `just
transfer_check` all pass.

**Follow-up:** If a real v2 generation is ever recovered from history, booting
it under QEMU would upgrade this from admission to a true rollback boot. The
rollback window also remains unlimited in code — v2 retention is unconditional
decode support, noted since C7.1.

### B5 — no C7 gate exercised the syscall layer or real components

**Resolved:** 2026-07-26. See `devlog/2026-07-26-b5-live-sample-plane/`.

**Problem:** No test or component reached any `SYS_SHARED_BUFFER_*` syscall. The
gates called `SharedBufferTable` methods on locally constructed tables and never
touched the global `SHARED_BUFFER_TABLE`, so the rights gates, the loan receiver
binding, and reclamation through real termination were unproven. C7.7's "two
isolated components" were the `u64` constants `0x71`/`0x72`, and its "peer death"
was a direct `reclaim_owner` call. This is the blind spot B3's boot wedge shipped
through.

**Evidence:** `grep 'dispatch|UserFrame|sys_'` and `grep SHARED_BUFFER_TABLE`
over `kernel/tests/` both returned no matches, while `SharedBufferTable::new()`
appeared 33 times. `kernel/tests/sample_plane.rs:57-58` defined its holders as
bare integers; `:462` stood in for peer death with `reclaim_owner`.

**Fix:** Added the four missing loan wrappers (`loan`/`loan_map`/`return`/
`revoke`) to `slime_rt`, completing the nine-syscall surface begun in B4. Added
two real components, `sample-lender` and `sample-receiver`, that the generation
grants a factory, a channel, and a `supervise` handle; init spawns the receiver
first so the lender names its loan receiver by capability rather than ambient
task id. `just sample_plane_live_check` asserts an ordered transcript covering
the happy path plus six denial arms, and rejects any component `fail:` line.
A first draft exposed a real ordering property: a lender that exits before the
receiver maps has its loan settled by its own termination, so the lender now
waits for a settle message — the C7.5 retention rule, asserted rather than raced.

**Exit condition (observed):** `just sample_plane_live_check` passes: two
separately spawned components move a two-page payload — larger than `MAX_MSG` —
through the real syscalls, with only the 64-byte descriptor crossing the IPC
channel, and every denial arm observed before the operation it guards.
`just sample_plane_check` (5/5), `just test`, all shared-buffer gates
(8/8/8/7/4), `just spawn_service_check`, `just dango_check`, `just
powerbox_check`, `just transfer_check` (exercising the renumbered slots 45/46),
`just generation_cmd_check`, `just generation_check`, `just
framework_safety_check`, and fmt/lint with `_components` are all clean.

**Follow-up:** `SYS_SHARED_BUFFER_REVOKE` has a wrapper and in-harness coverage
but no live caller, since the lender settles by return. The two insert-failure
rollback paths still need a full capability table at the moment of insert, which
neither gate stages.

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
