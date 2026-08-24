# C9.1: a root-brokered clock service, and the register wall it cannot cross

| Field | Value |
|---|---|
| Date | 2026-08-24 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/clock-authority/v1`, generation v5 rights/resources, syscall ABI, `boot-contracts`, `slime-root/src/{clock,generation,ipc,main,notification,platform_timer}.rs`, component runtime, `clock-authority-probe`, seL4 generation/build/check orchestration |
| Roadmap | C9.1, C9 |
| Gates | `just clock_authority_check`, `just sel4_gate_control_check`, `just contracts_check`, `just generation_check`, `just test_sel4_root`, `just test_host`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` |
| Trigger | C9's first slice required the root's existing timer mechanism to become explicit component authority |
| Baseline | `slime-root` owned and boot-tested the physical timer, but no component-facing operation could read a clock, arm a timer, cancel it, or advance deterministic simulated time |

## Summary

C9.1 exposes the existing root timer through an authenticated, bounded service rather than through ambient component state. A versioned Zutai resource grants monotonic read, timer use, simulated read, and simulated advance independently; generated syscall labels carry those operations over each task's badged root endpoint. The root enforces per-task timer quotas, delivers expiry through a generation-declared Notification and badge, drops live timers and authority when the task terminates, and refuses undeclared or malformed calls distinctly. The QEMU gate observes five root-attributed authority installations, independent holders, cancellation, quota isolation, one-shot expiry, malformed denial, and a live timer reclaimed at exit. This closes only the service semantics: current AArch64 seL4 profiles globally enable EL0 physical counter and timer register access, so hostile native code can still read or reprogram the underlying architectural timer; the contract and implementation state that wall explicitly.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/clock-authority/v1` | Added a bounded resource with domain-separated holder and Notification-grant identities, four independent authority bits, per-holder and aggregate timer ceilings, and the architectural register-integrity limitation | Cross-process clock policy has one versioned Zutai source of truth and does not overclaim register isolation |
| Generation and syscall contracts | Added generation v5 clock rights/resource fields and labels 44–48 for monotonic read, timer arm/cancel, simulated read/advance; regenerated Rust/Python bindings and updated capability/syscall documentation | New rights and operations cannot drift between builder, decoder, root, runtime, and documentation |
| `slime-root/src/clock.rs` | Added live-task-keyed authority storage, duplicate-declare refusal, per-task quota enforcement over the existing `TimerScheduler`, deterministic simulated time, stale-expiry suppression, and termination cleanup | Lifetime task ids cannot exhaust a concurrent-live table; one holder cannot consume another's quota; stale timers cannot signal cleared authority |
| Root dispatch | Validates clock word counts before service authorization, routes physical timer Notifications by their reserved badge without interpreting undefined message-info registers, excludes timer-only wakes from the component-request iteration ceiling, drives expiry through `TimerScheduler::service_timer_source`, delivers already-decided wakes even when deadline programming or IRQ acknowledgement fails, and propagates teardown clock-read failure without poisoning the scheduler's monotonic floor | Malformed requests and absent authority stay distinguishable; timer traffic cannot exhaust the request-progress bound; every popped expiry remains deliverable across a later platform error; failed spawns leave no clock authority; teardown read failure cannot corrupt later scheduling |
| Notification admission | Resolves each timer holder's exact wait Notification and rejects a timer badge colliding with an existing signaller on that object | A peer signaller cannot spoof timer expiry with the same badge |
| Runtime and probe | Added typed wrappers plus a fixture-only malformed-request negative control; the six-instance fixture gives each authority to a separate holder and leaves one timer live across exit | Every required authority, denial, cancellation, quota, one-shot, and teardown path is exercised through the real component ABI |
| QEMU gate | Added `just clock_authority_check`, Zutai-decoded fixture assertions, root/task correlation checks, image identity validation, failure markers, a frozen boot layout, and a gate-control pin of 19 required markers | Component-authored serial text is corroborated by the admitted declaration, root-attributed installs, serves/refusals, exit, teardown, and the resolved capability layout |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Clock contract/bindings drift | `just contracts_check` | generated binding or documented syscall mismatch |
| Nondeterministic or inadmissible generation | `just generation_check` | isolated build mismatch or admission refusal |
| Authority, cancellation, quota, expiry, denial, teardown, or timer/request interleaving regression | `just clock_authority_check` | missing/out-of-order root and component evidence, failure marker, wrong task correlation, QEMU timeout, dropped expiry, or graph-iteration exhaustion |
| Gate weakened silently | `just sel4_gate_control_check` | clock marker-count mismatch or a mutated transcript/layout accepted |
| Runtime lifetime/expiry regression | `just test_sel4_root` | fewer than 158 tests, authority-slot reuse failure, or stale expiry delivered |
| Permanent Rust/Python quality regression | `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | formatter, denied warning, Python lint, or spelling failure |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just clock_authority_check` | Passed on `aarch64-sel4-qemu-virt`: independently granted clocks, bounded timer cancellation/expiry, deny-by-default, malformed distinction, and live-timer teardown observed | Direct |
| `python3 scripts/check/check-sel4-boot-layout.py --no-build` | 27 plane layouts matched frozen fixtures; `sel4-clock-authority` resolved zero init slots because clock calls reuse the root-service endpoint | Direct |
| `just sel4_gate_control_check` | 36 gates rejected 1390 mutated transcripts and layouts; clock gate pinned at 19 markers | Direct |
| `just generation_check` | Two isolated builds produced byte-identical `generation.bin` and `boot-store.bin`; admission and CPU-budget mutation corpus passed | Direct |
| `just contracts_check` | Generated bindings current; 203 contract tests passed; all 31 seL4 manifests encoded generation v5 | Direct |
| `just test_sel4_root` | 158/158 tests passed across 17 modules, including post-mutation timer-error transitions that retain already-decided wakes | Direct |
| `just test_host` | 228 boot-contract tests plus the component protocol suites passed | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | Passed | Direct |

## Decisions

- **Decision:** Clock authority gates the root service, not AArch64 registers.
  **Rationale:** `KernelArmExportPCNTUser` and `KernelArmExportPTMRUser` are global seL4 configuration grants. The root itself needs physical timer access, and the current kernel exposes no per-TCB narrowing. Claiming that an undeclared native component cannot execute `mrs CNTPCT_EL0` or write `CNTP_CVAL_EL0`/`CNTP_CTL_EL0` would be false.
  **Rejected alternative:** presenting the new rights as adversarial counter/timer-register isolation.

- **Decision:** Deliver expiry on an existing generation-declared Notification with a reserved badge bit.
  **Rationale:** C9.2 must wait on time and messages through one native wait mechanism. A second timer-specific wake object would split that path and add authority the generation already describes.
  **Rejected alternative:** a new timer queue or endpoint exposed directly to components.

- **Decision:** Validate malformed clock message shape before checking the caller's service grant.
  **Rationale:** Request shape is public ABI syntax; authorization is a separate decision. This preserves the promised `INVALID_ARG` versus `BAD_CAP` distinction without disclosing any granted state.
  **Rejected alternative:** letting undeclared callers receive `BAD_CAP` for malformed requests.

- **Decision:** Store authority in a bounded live-task table searched by full `TaskId`.
  **Rationale:** `TaskId` is a never-reused lifetime identity while `MAX_TASKS` bounds concurrent tasks. Direct indexing would permanently exhaust authority after enough spawn/exit cycles.
  **Rejected alternative:** indexing the concurrent-live array by lifetime id.

## Open risks and follow-ups

- [ ] The AArch64 register-integrity wall remains: any hostile native component can read the physical counter and, on current profiles, reprogram the same physical timer registers used by the root service. Closing it requires a kernel/platform change or another privileged timer source; C9.1 makes no stronger claim.
- [ ] The service has one authority entry per concurrently live task, including deny-by-default tasks. This matches `TaskTable<MAX_TASKS>` and fails a spawn cleanly if the live table is full; no shipped fixture approaches the bound.
- [ ] C9.2 must consume the declared timer Notification without inventing a second wake mechanism and must preserve the badge-collision admission rule.

## Artifacts and provenance

- Focused report: none; the implementation and gate carry the decisive evidence.
- Raw transcript: not retained as a repository artifact; exact repeatable commands are listed above.
- Serial/debugger/model output: `just clock_authority_check` QEMU serial evidence, observed during this change.
- Related roadmap item: [C9.1](../../roadmap/02-core-runtime.md#c91--explicit-clock-and-timer-service-authority)
