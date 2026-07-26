# B3 — C7.5 full-graph boot wedge: shared-buffer table overflowed the kernel stack

| Field | Value |
|---|---|
| Date | 2026-07-26 |
| Status | Verified |
| Scope | `kernel/src/memory/shared_buffer.rs` static initialization; full-graph boot gates (`transfer_check`, `spawn_service_check`, `dango_check`) |
| Trigger | Backlog B3, opened by the 2026-07-26 C7 audit (`devlog/2026-07-26-c7-audit/`) |
| Baseline | C7.4 `928389e` boots the full component graph to `vertical slice healthy`; C7.5 `ca15764` and every later commit wedge |

## Summary

Every full-graph boot wedged from C7.5 onward: the guest printed its last init
marker and then stopped, never reaching `on_idle`, so `exit_qemu` never ran and
`transfer_check`, `spawn_service_check`, and `dango_check` all died on their
harness timeouts. The cause is a kernel-stack overflow. C7.5 grew
`SharedBufferTable` to 10520 bytes of fixed arrays, and the table was published
through a `LazyLock`, whose initializer constructs the value on whichever stack
first touches the static. Because no `SharedBufferFactory` is minted on the live
boot path (backlog B4), the first touch is `SHARED_BUFFER_TABLE.lock()` inside
`task::terminate` — running on a 32 KiB task kernel stack with no guard page. The
10 KiB temporary overflowed it and corrupted whatever preceded the stack, and
since the overflow silently scribbles rather than faulting, the boot wedged
instead of panicking. Replacing the `LazyLock` with a plain `const`-initialized
static places the table in `.bss` and removes the stack temporary entirely. All
three gates now pass at the stock 32 KiB stack, with the full C7 and kernel
suites clean.

## Observable symptom

- Command: `just spawn_service_check` (also `just transfer_check`, `just dango_check`)
- Expected: `[generation] vertical slice healthy`, QEMU exits `Success`.
- Observed: serial stops at `[init] spawn graph launched` (or `[init] generation
  transfer installed`); no `idle-blocked` line, no `vertical slice healthy`, no
  panic, no `kernel exit:` line. QEMU killed by timeout, exit 124.
- Exit/fault/serial evidence: recorded in `devlog/2026-07-26-c7-audit/transcript.txt`
  §3; reproduced three times per gate, including alone on an idle machine and
  with the inner QEMU timeout raised 60 s → 600 s.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Audit bisect established C7.4 `928389e` clean, C7.5 `ca15764` wedged, and `ca15764..HEAD` touching no kernel source | Defect is inside the C7.5 diff |
| 2 | The audit's initial suspicion — `reclaim_owner` iterating under `SCHEDULER` — was already contradicted by B4: with no factory minted, every table is empty and `reclaim_owner` iterates nothing | Ruled out the loop body; the cost had to be in *reaching* the table, not walking it |
| 3 | `init` prints `[init] spawn graph launched` then immediately calls `slime_rt::exit(0)` (`components/bins/src/bin/init.rs:183-184`) | The wedge is on the **exit** path, i.e. inside `task::terminate` |
| 4 | `grep SHARED_BUFFER_TABLE kernel/src` shows exactly one non-syscall touch point: `task/mod.rs:832`, inside `terminate` | With no syscall ever invoked (backlog B5), `terminate` performs the `LazyLock` **first** initialization |
| 5 | `size_of::<SharedBufferTable>()` measured by a deliberately-failing const probe: **10520 bytes** | A 10 KiB temporary on a 32 KiB stack, mid-call, is a plausible overflow |
| 6 | `KERNEL_STACK_SIZE = 32 * 1024` (`task/mod.rs:19`), allocated as a plain `vec![0u8; …]` boxed slice with **no guard page** (`grep guard` finds nothing in `task/` or `memory/`) | An overflow corrupts adjacent heap silently instead of faulting — matches "wedge, no panic" |
| 7 | Decisive experiment: raised `KERNEL_STACK_SIZE` 32 KiB → 128 KiB, changing nothing else. `just spawn_service_check` → `vertical slice healthy`, exit 0 | Confirms stack exhaustion as the mechanism |
| 8 | Reverted the stack bump; replaced the `LazyLock` with a `const` static. `just spawn_service_check` passes at the **stock 32 KiB** stack | The table, not the stack size, was the defect |
| 9 | Audited sibling lazy statics: `DmaTable` 256 B, `LAST_PAYLOAD` 512 B, `STAGING` heap-allocated via `vec![]`, `Scheduler` empty collections | `SharedBufferTable` was the only instance of this hazard |

## Root cause

`SHARED_BUFFER_TABLE` was declared as
`LazyLock<Mutex<SharedBufferTable>>` with `LazyLock::new(|| Mutex::new(SharedBufferTable::new()))`.
A `LazyLock` initializer is an ordinary closure that runs on the caller's stack at
first access: it constructs the 10520-byte `SharedBufferTable` as a local, wraps it
in a `Mutex`, and moves it into the static's storage. In a debug build nothing
elides that temporary.

The violated invariant is that no kernel code path may place a multi-kilobyte
temporary on a task kernel stack. `KERNEL_STACK_SIZE` is 32 KiB and the stack is a
plain heap allocation with no guard page, so exceeding it does not fault — it
overwrites adjacent heap. The overflow happened inside `task::terminate` while
`SCHEDULER` was held, corrupting scheduler-adjacent state, so the ready queue
never drained to `on_idle` and the boot hung with no diagnostic.

`SharedBufferTable::new()` was already a `const fn`, so the laziness bought
nothing: the table has no runtime-dependent initialization. It was pure
incidental cost, and C7.5 grew the arrays (adding `loans: [Option<Loan>; 64]` and
widening `Mapping` with `loan_id: Option<u64>`) until the temporary crossed the
stack budget. C7.4's smaller table fit, which is why the defect appears exactly
at C7.5.

Secondary, not the root cause: the missing guard page is why this manifested as a
silent wedge instead of a page fault, and the absent factory grant (B4) is why
`terminate` — rather than a syscall — was the first toucher. Neither is the
defect; both shaped how it presented.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `kernel/src/memory/shared_buffer.rs` | `SHARED_BUFFER_TABLE` is a plain `const`-initialized `Mutex<SharedBufferTable>` instead of a `LazyLock`; `use spin::Mutex` drops the now-unused `LazyLock` import | The table lives in `.bss`; no kernel path materializes it on a stack |
| `kernel/src/memory/shared_buffer.rs` | Added a `const` assertion that `size_of::<SharedBufferTable>() * 2 < KERNEL_STACK_SIZE`, with a comment explaining the hazard | Growing the fixed bounds past the stack budget fails the build instead of wedging a boot |

The fix matches the existing convention for fixed-size kernel tables:
`FRAME_ALLOCATOR` (`memory/pmm.rs:17`), `QUEUE`/`DECODER` (`drivers/input.rs:23-24`),
and `PENDING_WAKES` (`task/mod.rs:152`) are all `const`-initialized statics.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The table grows past what a kernel stack can hold | `const` assertion in `shared_buffer.rs` (compile-time; verified to fire by temporarily setting `MAX_LOANS = 1024`, which produced `error[E0080]: SharedBufferTable is too large to be safely materialized on a kernel stack`) | Build fails with that message |
| A future change reintroduces lazy initialization of a large kernel table | The `const` static plus its explanatory comment; the assertion still bounds the size | Build failure, or a boot wedge caught by the gates below |
| The full-graph boot regresses again | `just spawn_service_check`, `just transfer_check`, `just dango_check` — all three restored to the C7 verification set | Guest wedges at an init marker; no `vertical slice healthy`; exit 124 |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just spawn_service_check` | pass — `vertical slice healthy`, exit 0 (previously wedged) | Direct |
| `just transfer_check` | pass — install, pending boot, promotion, rollback retention (previously wedged) | Direct |
| `just dango_check` | pass — `dango native runtime check: ok` (previously timed out) | Direct |
| `just test` | pass — 160 `[Passed]` assertions | Direct |
| `just shared_buffer_factory_check` | pass, 8/8 | Direct |
| `just shared_buffer_accounting_check` | pass, 7/7 | Direct |
| `just shared_buffer_mapping_check` | pass, 8/8 | Direct |
| `just shared_buffer_loan_check` | pass, 7/7 | Direct |
| `just sample_descriptor_check` | pass, 4/4 | Direct |
| `just sample_plane_check` | pass, 5/5 | Direct |
| `just generation_cmd_check` | pass | Direct |
| `just contracts_check` | pass | Direct |
| `just generation_check` | pass, byte-identical two builds | Direct |
| `just framework_safety_check` | pass | Direct |
| `just fmt_check` / `just lint` | clean (`-D warnings`) | Direct |
| `just fmt_check_components` / `just lint_components` | clean | Direct |
| Guard fires on a plausible bug (`MAX_LOANS = 1024`) | `error[E0080]` with the intended message; reverted | Direct |

## Decisions

- Decision: fix by making the static `const`-initialized, not by enlarging
  `KERNEL_STACK_SIZE`.
- Rationale: the 128 KiB stack bump was a diagnostic that confirmed the mechanism,
  not a fix — it would have papered over a 10 KiB stack temporary that has no
  reason to exist, and left every other task paying 4× the stack. `SharedBufferTable::new()`
  is already `const`, so the table belongs in `.bss`.
- Rejected alternative: keep the `LazyLock` and raise the stack — treats the
  symptom, and the next table growth reintroduces the same wedge.

- Decision: add a compile-time size assertion rather than a runtime check or a
  QEMU test case.
- Rationale: the hazard is a property of the type's size, knowable at compile
  time; a runtime check would fire after the damage, and a QEMU case would only
  catch it once the bounds already grew. The assertion was verified to fail on a
  plausible bug rather than being assumed correct.
- Rejected alternative: a `debug_assert` on stack headroom in `terminate` — later,
  slower, and it cannot fail the build.

- Decision: leave the missing kernel-stack guard page as a separate concern, not
  folded into this fix.
- Rationale: a guard page would have turned this wedge into an immediate,
  diagnosable fault, and is genuinely worth having — but it is a task/memory
  subsystem change with its own blast radius, not part of restoring the C7 boot.
  Recorded as a follow-up below.
- Rejected alternative: add the guard page here — scope creep into `task::spawn`
  and the frame allocator while three gates are red.

## Open risks and follow-ups

- [ ] Task kernel stacks are plain heap allocations with **no guard page**
  (`task/mod.rs:495`), so any future stack overflow will again corrupt adjacent
  memory silently instead of faulting. Worth a backlog item of its own; this fix
  removes the current trigger, not the class.
- [ ] The audit's other C7 findings remain open: B4 (dormant budget/factory
  wiring), B5 (no syscall or real-component coverage), B6 (retained-v2 decode
  only), B7 (`map` vs `bufferMap`), B8 (budget aggregate). B4 in particular still
  blocks the C7 gate; this entry closes only B3.
- [ ] `just test` was run in release (`--release`, per the Justfile). The overflow
  was observed in the debug-profile boot path used by the checks; no attempt was
  made to determine whether an optimized build elided the temporary, since the
  fix removes it in both.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: the reproduction, bisect, and gate output that opened this item
  are in `devlog/2026-07-26-c7-audit/transcript.txt` (§3 symptom, §4 bisect).
- Serial/debugger/model output: QEMU serial via the `just` targets above; no
  debugger session was needed once the const probe gave the table size.
- Related roadmap items: `roadmap/00-backlog.md` B3 (resolved by this entry);
  `roadmap/02-core-runtime.md` C7.5.
- Related prior entries: `devlog/2026-07-26-c7-audit/` (opened B3);
  `devlog/2026-07-24-b2-blocked-task-state/` (same observable class — ready queue
  never drains to `on_idle` — but an unrelated cause).
