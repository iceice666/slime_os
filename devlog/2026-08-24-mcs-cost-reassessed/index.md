# Reassessing MCS: the cost is per-target, and the QEMU build already left the verified set

| Field | Value |
|---|---|
| Date | 2026-08-24 |
| Kind | Decision |
| Status | Proposed |
| Scope | `sel4/config/qemu-arm-virt.cmake`, `roadmap/02-core-runtime.md` (C9 architecture decisions), `roadmap/00-backlog.md` (B77), read-only survey of `deps/sel4/`, `deps/rust-sel4/`, `slime-root/src/{task,fault,ipc,main}.rs`, `components/runtime/src/syscall/sel4_transport.rs` |
| Roadmap | C9, C9.3, B48, B77 |
| Gates | none |
| Trigger | Asked to discuss what introducing MCS would mean, one commit after `2806be8` recorded "budgets stay undeclarable" as a wall |
| Baseline | `2806be8`'s C9 decision: MCS off because "the AArch64 functional-correctness proofs do not cover that configuration"; `KernelIsMCS OFF` with the 20-line rationale added by B48 |

## Summary

Investigating MCS turned up that the recorded reason for keeping it off is
imprecise in a way that matters, and that the reason it *would* be expensive is
not the one written down. Three claims were checked against the pinned tree.
The timer story is a non-issue: MCS does not touch the root task's PPI 30, and
does not touch the `KernelArmExportPCNTUser`/`PTMRUser` grants either. The Rust
bindings fully support MCS, gated on installed kernel config rather than Cargo
features. But the assurance framing was wrong in both directions: upstream lists
AArch64 MCS proofs as *in progress* rather than absent, and — the part that
actually changes the decision — the QEMU configuration this repository develops
against is **already outside** the verified set (`KernelVerificationBuild OFF`,
`KernelDebugBuild ON`, `KernelPrinting ON`, and no verified-platform entry for
`qemu-arm-virt`), while `sel4/config/bcm2712-rpi5.cmake` includes upstream's own
`AARCH64_bcm2712_verified.cmake`. So "trade a verified kernel for a scheduling
feature" describes the RPi 5 target and not the QEMU one. The real cost of MCS
here is an IPC-shaped one nobody had priced: `sel4::reply` ceases to exist and
every endpoint receive that may accept a `Call` needs an explicit Reply object.
Both rationale sites are corrected; the decision itself does not change, and the
survey also surfaced B77, a latent authenticated-fiction hole that exists
independent of MCS.

## Changes

| File | Change |
|---|---|
| `sel4/config/qemu-arm-virt.cmake` | Replaced the four-line assurance paragraph with the precise terms: AArch64 MCS proofs in progress and RISC-V MCS verified, MCS foundation-supported and stable, this build already unverified, and the RPi 5 config being where the claim is load-bearing. Records that the decision is per-target and that flipping it in this file alone is not the same decision |
| `roadmap/02-core-runtime.md` | C9's "Scheduling classes rest on priority" decision now states the per-target asymmetry instead of a blanket "the proofs do not cover that configuration". The conclusion is unchanged: a budgeted-CPU slice is blocked on the assurance decision, not on C9 |
| `roadmap/00-backlog.md` | New **B77**: `budget_us`/`period_us` are authenticated, unvalidated, and unread, so a non-repo producer can declare a budget that boots and is silently ignored. Proposed fix is to refuse nonzero values while MCS is off |

No implementation changed, and `KernelIsMCS` stays `OFF`.

## Decisions

- Decision: keep `KernelIsMCS OFF`, but on the corrected reasons.
- Rationale: the two claims that survive scrutiny are not about proofs. First,
  the API break is real and lands on the surface B46 just rebuilt:
  `sel4::reply` is `#[sel4_cfg(not(KERNEL_MCS))]`
  (`deps/rust-sel4/crates/sel4/src/syscalls.rs:212`), so it vanishes under MCS,
  and `ReplyAuthority` stops being the implicit unit and becomes a real Reply
  capability (`reply_authority.rs:20-24`). Every endpoint receive that may
  accept a `Call` must then supply a Reply object and invoke it. Second, MCS
  moves scheduling from one axis to two, and the repository has no declaration
  surface for the second: per-thread reservations, aggregate admission against
  one core, account ownership across spawn and restart, and timeout-fault
  routing are all absent. Neither of those is "the proofs don't cover it", and
  both are better reasons than the one previously recorded.
- Rejected alternative: turn MCS on now, on the grounds that the QEMU build is
  already unverified. That argument is locally valid and still loses, because
  the decision is not per-config in practice — `sel4/config/bcm2712-rpi5.cmake`
  is the same kernel pin with `KernelIsMCS OFF`, and the RP5 track's whole point
  is a demo image on a platform upstream actually verifies. Enabling MCS on QEMU
  only would mean developing and gating every plane against a kernel
  configuration the hardware target does not share, which is worse than not
  having budgets: the 34 marker gates would stop being evidence about the
  shipped configuration.

- Decision: the recorded rationale for a wall must state the terms precisely,
  including the ones that weaken it.
- Rationale: the previous text said the proofs "do not cover that
  configuration". Read literally that is true of MCS on AArch64 and *equally*
  true of the build the sentence appears in, which makes it an argument the
  config file does not survive. A reader checking it would either conclude the
  project is confused or that the wall is decorative. Both are worse outcomes
  than the honest version, where MCS is cheap on QEMU, expensive on `bcm2712`,
  and declined for API and declaration-surface reasons that hold on both.

- Decision: the MCS question does not reopen C9.3.
- Rationale: C9.3 was already scoped to ordering rather than quantity, and the
  survey confirms every one of its deliverables and checks rests on
  `Instance.priority`/`Instance.workerPriority` and TCB priority application
  that B48 already enforces — `slime-root/src/task.rs:861,905` apply it and
  `scripts/check/check-sel4-sample-plane.py:267-276` already demonstrates a
  priority-100 spinner failing to starve its higher-priority main thread. MCS
  would add a second axis beside C9.3, not replace it.

## Open risks and follow-ups

- B77 is open and unblocked: while MCS is off, both validators should refuse a
  nonzero `budget_us`/`period_us` rather than decode and ignore it. It is a
  small predicate in `check-generation.py` and `Generation::validate` plus
  mutation coverage, and it is the difference between a field that is stated
  zero and one that is accidentally zero.
- `slime-root/src/fault.rs`'s `Termination::Timeout`/`LifecycleEventKind::TimedOut`
  are a genuine fit for an MCS budget-exhaustion fault, not a naming
  coincidence — but the decoder has no timeout arm and `FaultKind` cannot carry
  the payload (`fault.rs:41-79,133-203`). C9.4 still has to decide their fate on
  non-MCS terms, and should not be tempted to reserve them for a fault that
  cannot currently be delivered.
- If the RP5 track ever wants real CPU budgets on `bcm2712`, the sequence is
  fixed by this survey: the assurance decision first, then the Reply-object IPC
  migration across roughly seven files, then the declaration surface, then
  gates. It is not a config flip, and it is not small.
- Unverified by execution: no MCS kernel was built and no MCS image was booted.
  Every claim here is read from the pinned `deps/sel4/` and `deps/rust-sel4/`
  sources, so the API inventory is a static reading rather than a compile
  result. A real attempt would likely find further breakage the grep did not
  name.

## Artifacts and provenance

Read-only survey, three parallel scouts plus direct verification of the claims
that changed the outcome.

Timer and IRQ, from the pinned kernel:

- `deps/sel4/tools/hardware.yml:120-130` — `KERNEL_TIMER_IRQ` selects the
  hypervisor timer under `CONFIG_ARM_HYPERVISOR_SUPPORT`; no MCS conditional.
- `deps/sel4/src/arch/arm/kernel/boot.c:120-145` — `init_irqs` claims
  `KERNEL_TIMER_IRQ` and reserves the virtual-timer IRQ; PPI 30 is left
  `IRQInactive` and remains issuable to the root task. Unchanged by MCS.
- `deps/sel4/src/drivers/timer/generic_timer.c:7-33` — same driver both ways;
  MCS initializes an absolute deadline of `UINT64_MAX` where non-MCS installs a
  periodic reload.
- `deps/sel4/src/arch/arm/armv/armv8-a/64/user_access.c:7-64` and
  `deps/sel4/src/drivers/timer/config.cmake:35-67` — the EL0 counter/timer
  export options depend on the generic timer and hypervisor support only, not on
  MCS. The C9.1 register wall is therefore identical under MCS.

Assurance, quoted rather than recalled:

- `deps/sel4/CAVEATS.md:111-121` — "Functional correctness proofs for MCS on
  AArch64 are in progress"; MCS "is supported by the seL4 foundation and should
  generally be stable, with small API changes to be expected".
- `deps/sel4/CAVEATS.md:31-55` — verified AArch64 platform list. It includes
  `bcm2712` and does not include `qemu-arm-virt`; RISC-V MCS on `hifive` is
  listed as verified.
- `deps/sel4/configs/include/AARCH64_verified_include.cmake` — the verified
  baseline sets `KernelVerificationBuild ON`, `KernelPrinting OFF`,
  `KernelFastpath ON`. `sel4/config/qemu-arm-virt.cmake:26-28` sets the first
  two the other way, so this build is outside that set on its own terms.
- `sel4/config/bcm2712-rpi5.cmake:1` includes
  `deps/sel4/configs/AARCH64_bcm2712_verified.cmake`, which is where the
  verified claim is load-bearing.

Rust binding support and repo blast radius:

- MCS is supported and gated on installed kernel config, not Cargo features:
  `deps/rust-sel4/crates/sel4/config/data/build.rs:16-20` reads kernel JSON from
  `SEL4_PREFIX`, exported by `scripts/build/build-sel4.py:562-566`.
- MCS-only surface: `cptr.rs:254-313` (Reply, SchedContext, SchedControl),
  `invocations.rs:169-272` (MCS `tcb_configure`/`tcb_set_sched_params`),
  `:275-283` (`tcb_set_timeout_endpoint`), `:322-350` (SchedControl/SchedContext),
  `object.rs:15-127` (new object types and sizes), `bootinfo.rs:95-98`
  (`sched_control`), `init_thread.rs:154-155` (`seL4_CapInitThreadSC`),
  `arch/arm/fault.rs:18-19,41-42` (timeout fault).
- Non-MCS-only surface that disappears: `syscalls.rs:212-215` (`sel4::reply`),
  `invocations.rs:521-524` (`save_caller`), `lib.rs:142-145`
  (`ImplicitReplyAuthority`).
- Repo counts, by grep: 3 `tcb_configure` and 3 `tcb_set_sched_params` call
  sites (`slime-root/src/main.rs:1864,1882`, `slime-root/src/task.rs:852,861,890,905`);
  2 direct `sel4::reply` calls (`slime-root/src/ipc.rs:413`,
  `components/runtime/src/syscall/sel4_transport.rs:181`); 42 `ipc::reply` calls
  in the root dispatcher, all funnelling through the single `ipc.rs` wrapper, so
  the source-level cost is far smaller than the call count suggests.
- Initial-cap slot numbers do not shift: `seL4_CapInitThreadSC` occupies its
  slot in the bootinfo enum in both configurations, so C9's neighbours and the
  frozen boot-layout fixtures are not disturbed by this axis.

Declaration surface, from the repo:

- `contracts/generation/v1/schema.zt:65-104` — the manifest declares
  `priority`/`workerPriority` only, with the comment that budget and period stay
  undeclared to avoid authenticated fiction.
- `contracts/generation/v5/schema.zt:203` — the wire record nonetheless carries
  both as 64-bit fields.
- `scripts/build/build-generation.py:2947-3002,3137-3165` — both written zero,
  main and worker.
- `scripts/check/check-generation.py:688` unpacks both and tests neither;
  `boot-contracts/src/generation.rs:1558-1559` decodes them and `:2200-2208`
  does not constrain them. Basis for B77.

## Corrections

None.
