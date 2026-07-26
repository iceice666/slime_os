# C7 milestone audit — boot wedge and unproven exit conditions

| Field | Value |
|---|---|
| Date | 2026-07-26 |
| Kind | Audit |
| Status | Verified |
| Scope | C7.1–C7.7 gates, full-graph boot paths (`transfer_check`, `spawn_service_check`, `dango_check`), generation budget wiring, shared-buffer syscall coverage |
| Roadmap | C7, B3, B4, B5, B6, B7, B8 |
| Gates | `just transfer_check`, `just spawn_service_check`, `just dango_check` |
| Trigger | Requested audit of the complete C7 milestone at `2384bea` |
| Baseline | `roadmap/02-core-runtime.md` records C7 and every sub-slice as complete; `roadmap/README.md` names C8 as the next open slice |

## Summary

Every C7 sub-slice gate passes at HEAD, but the milestone gate is not closed.
Three full-graph boot checks — `just transfer_check`, `just spawn_service_check`,
and `just dango_check` — wedge: the guest never reaches `on_idle`, so `exit_qemu`
never runs and each check dies on its outer timeout. Bisect attributes this to
C7.5 (`ca15764`); C7.4 (`928389e`) is clean and the C7.5..HEAD range touches no
kernel source, exonerating C7.6/C7.7. Raising the inner QEMU timeout tenfold
(60 s → 600 s) does not help, so this is a wedge and not slowness. Separately,
several C7 exit conditions are worded about components, generations, and boots
but are only proven against an in-kernel harness: no generation declares a
shared-buffer budget, no `SharedBufferFactory` is ever minted, none of the nine
`SYS_SHARED_BUFFER_*` syscalls is reachable from any test, and C7.7's "two
isolated components" are two `u64` constants. The mechanism code itself is sound
— no logic defect was found in the shared-buffer state machine. Findings are
recorded as backlog items B3–B8; B3 and B4 gate the "C7 complete" claim and C8.

## Observable symptom

- Command: `just transfer_check`, `just spawn_service_check`, `just dango_check`
- Expected: each reaches its success line (`generation transfer check: ...
  passed` / `[generation] vertical slice healthy`) and QEMU exits `Success`.
- Observed: serial output stops at an init progress marker; no
  `idle-blocked` or `vertical slice healthy` line is ever printed; QEMU is
  killed by the harness timeout (exit 124).
- Exit/fault/serial evidence: `transfer_check` stalls after `[init] generation
  transfer installed`; `spawn_service_check` and `dango_check` stall after
  `[init] spawn graph launched`. Full serial in
  [`transcript.txt`](transcript.txt) §3. No fault, no panic, no `kernel exit:`
  line — the guest simply never finishes draining its ready queue.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | All six C7 sub-slice gates pass at HEAD (8+7+8+7+4+5 cases), as do `just test`, `generation_check`, `fmt_check`, `lint`, `framework_safety_check`, `generation_cmd_check` | The defect is not in the shared-buffer unit surface; look at gates the C7 devlogs did not run |
| 2 | `transfer_check` wedges after `[init] generation transfer installed`, three runs | Real, reproducible; not a one-off flake |
| 3 | Raised only the inner QEMU timeout 60 s → 600 s in a scratch copy of `check-transfer.py`; still wedged after 1804 s wall | Wedge, not slowness — rules out timeout tuning as the fix |
| 4 | The `bad-closure` and `bad-release` negative scenarios also hang *after* printing their expected rejection markers | The wedge is not specific to the success path; the harness only "passes" them because it catches `SystemExit` |
| 5 | `spawn_service_check` and `dango_check` wedge the same way, `spawn_service_check` three times including alone on an idle machine (load avg 2.45) | Not machine-load contention; the whole full-graph boot class is affected |
| 6 | Bisect on `transfer_check`: pass at C7.2 `991dcbb`, C7.3 `ed49fb5`, C7.4 `928389e`; fail at C7.5 `ca15764` and HEAD | C7.5 is the first bad commit |
| 7 | Bisect on `spawn_service_check`: pass at `928389e`, fail at `ca15764` and HEAD | Second independent gate confirms the same boundary |
| 8 | `git diff --stat ca15764 HEAD -- kernel/src components boot-contracts/src` is empty | C7.6 and C7.7 introduced no kernel code; the defect is inside the C7.5 diff |
| 9 | Parsed the built `generation-1.bin`: 21 objects, zero of `KIND_RESOURCE`; the one `SLIMESB` match lies inside the kernel object's byte range | No live generation declares a budget → every holder is `HolderQuota::DENY` (B4) |
| 10 | Because the tables are empty at boot (step 9), `reclaim_owner` iterates nothing — so the obvious "reclaim under `SCHEDULER`" hypothesis cannot by itself explain the wedge | Root cause remains narrowed to the C7.5 diff, not isolated; see below |
| 11 | No test reaches the syscall layer (`grep dispatch\|UserFrame\|sys_` over `kernel/tests/` is empty) and no test touches the global `SHARED_BUFFER_TABLE` | Explains how a kernel-lifecycle regression passed review on the unit gates (B5) |

## Root cause

**Not established — narrowed to the C7.5 diff (`928389e..ca15764`).** Recorded
here so the next session does not repeat the elimination work.

The intuitive suspect is reclamation: `task::terminate` calls
`SHARED_BUFFER_TABLE.lock().reclaim_owner(id)` at `kernel/src/task/mod.rs:832`
while `SCHEDULER` is held, and C7.5 rewrote `reclaim_owner` to settle loans and
tear down mappings under that lock. **This is unlikely to be sufficient on its
own:** by finding B4 no `SharedBufferFactory` is minted and every holder is
deny-by-default, so during these boots the region, mapping, and loan tables are
all empty and `reclaim_owner` iterates nothing.

That redirects suspicion to C7.5's capability-surface change. The new
`KernelObject::SharedBufferLoan(BufferLoan)` variant widens `KernelObject`,
hence `Capability`, hence `[Option<Capability>; MAX_CAPS]`, hence `Task`. That
is the same shape as the defect the B2 fix had to repair after the `u64`-rights
widening, where `copy_from_current` bounded a byte copy at `MAX_CAPS` through a
per-byte scratch array and a widened `SpawnGrant` silently overflowed it
(`roadmap/00-backlog.md`, resolved B2). **[INFERENCE]** — this is a lead from
structural similarity, not an observed mechanism. No instrumented boot was run
to confirm it.

What is established: the wedge is a missing ready-queue drain, not a crash. No
panic, fault, or `kernel exit:` line appears; `on_idle` is the only path to
`exit_qemu` (`kernel/src/runtime/bootstrap.rs:856`) and it is never reached.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `roadmap/00-backlog.md` | Opened B3–B8 with problem, evidence, proposed fix, and exit condition each; header note records that the roadmap still claims C7 complete and that B3/B4 gate that claim | Backlog is the canonical home for defects blocking a new track gate |
| `roadmap/02-core-runtime.md` | C7 and C7.3/C7.5/C7.7 status lines corrected from Complete to reflect the red gates and unproven exit conditions; the C7 gate is reopened | Roadmap completion stays authoritative and matches observed evidence |
| `roadmap/README.md` | Core-runtime row and next-step reverted from "C7 complete / begin C8" to the backlog gate | Track index does not advertise an unclosed gate |
| `devlog/2026-07-26-c7-audit/` | This entry plus [`transcript.txt`](transcript.txt) | Evidence chain preserved |

No source, test, or contract file was modified. This was an audit; the fixes
belong to B3–B8.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A kernel lifecycle change passes the shared-buffer unit gates while breaking full-graph boot | `just transfer_check`, `just spawn_service_check`, `just dango_check` — restored to the C7 verification set by B3 | Guest wedges at an init marker; no `vertical slice healthy`; exit 124 |
| Shared-buffer authority silently stays dormant | B4's exit condition: a built generation contains exactly one validated `KIND_RESOURCE` budget object | Zero `KIND_RESOURCE` objects in `generation-1.bin` |
| Syscall gates, rollback arms, and reclamation wiring stay uncovered | B5's exit condition: `sample_plane_check` spawns real tasks and moves the payload through `SYS_SHARED_BUFFER_*` | Gate passes without any `dispatch`/`UserFrame` reference in `kernel/tests/` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just shared_buffer_factory_check` | pass, 8/8 | Direct |
| `just shared_buffer_accounting_check` | pass, 7/7 | Direct |
| `just shared_buffer_mapping_check` | pass, 8/8 | Direct |
| `just shared_buffer_loan_check` | pass, 7/7 | Direct |
| `just sample_descriptor_check` | pass, 4/4 | Direct |
| `just sample_plane_check` | pass, 5/5 | Direct |
| `just test` | pass, full kernel suite | Direct |
| `just generation_check` | pass, byte-identical two builds | Direct |
| `just generation_cmd_check` | pass | Direct |
| `just fmt_check`, `just lint`, `just framework_safety_check` | clean | Direct |
| `just transfer_check` | **FAIL** — wedge, 3 runs (incl. 600 s inner timeout) | Direct |
| `just spawn_service_check` | **FAIL** — wedge, 3 runs (incl. idle machine) | Direct |
| `just dango_check` | **FAIL** — timeout | Direct |
| Bisect `transfer_check` @ `991dcbb`/`ed49fb5`/`928389e` | pass | Direct |
| Bisect `transfer_check` @ `ca15764` | **FAIL** | Direct |
| Bisect `spawn_service_check` @ `928389e` pass, `ca15764` fail | attribution to C7.5 | Direct |
| Generation object-table parse: 0 × `KIND_RESOURCE` | budget absent on live path | Direct |

Documentation-only change; no runtime test was run *for this entry's edits*. All
results above were observed during the audit itself. Raw output:
[`transcript.txt`](transcript.txt).

## Decisions

- Decision: record B3's root cause as narrowed rather than naming
  `reclaim_owner`-under-`SCHEDULER` as the mechanism.
- Rationale: finding B4 proves the shared-buffer tables are empty during these
  boots, so that path iterates nothing and cannot alone explain the wedge.
  Publishing a plausible-but-contradicted cause would send the fix at the wrong
  file.
- Rejected alternative: assert the lock-order hypothesis as root cause — it
  reads decisively but conflicts with the evidence in the same audit.

- Decision: file the missed-gate problem (C7.1 listed `transfer_check` as direct
  evidence; C7.5 did not, and landed the regression underneath) inside B3 rather
  than as its own backlog item.
- Rationale: it is the process cause of B3, not an independent defect; splitting
  it would let B3 close while the gate stays out of the C7 set.
- Rejected alternative: a separate "process" backlog entry — the backlog's stated
  purpose is concrete defects in implemented code.

- Decision: reopen the C7 gate in `roadmap/` rather than annotating it as
  complete-with-caveats.
- Rationale: `AGENTS.md` makes a green verification suite a precondition for
  milestone work, and three gates are red. C8 consumes C7's sample plane, which
  is dormant on the live path (B4).
- Rejected alternative: leave C7 complete and track only via backlog — that
  would let C8 open against an unverified base.

## Open risks and follow-ups

- [ ] **B3** — C7.5 boot wedge. Root cause narrowed to the C7.5 diff, not
  isolated. Next step: bisect within that diff (capability/rights surface vs.
  `shared_buffer.rs` rewrite) and determine whether a task is left non-`Ready`,
  a lock order stalls, or a fault is swallowed into a halt loop.
- [ ] **B4** — no generation declares a shared-buffer budget; no
  `SharedBufferFactory` is minted; the C7 plane is dormant on the live path.
- [ ] **B5** — no gate exercises the nine `SYS_SHARED_BUFFER_*` syscalls or real
  spawned components; C7.7's "components" are `u64` constants.
- [ ] **B6** — retained-v2 "still boots" is proven only as decode.
- [ ] **B7** — the manifest right is still spelled `map`, not `bufferMap`.
- [ ] **B8** — budget validation bounds each holder but never the aggregate;
  needs a decision between summing and rewording.
- [ ] C8 should not open until B3 and B4 close (`roadmap/00-backlog.md`
  priority rule).
- [ ] Not investigated: whether the wedge also affects `storage_*`, `recovery`,
  or `rollback` gates. Only the three named checks were run.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`transcript.txt`](transcript.txt) — all gate output, the
  slow-vs-hung discriminator, the full bisect table, and the generation
  object-table parse.
- Serial/debugger/model output: QEMU serial captured inline in the transcript;
  no debugger session was run.
- Related roadmap items: `roadmap/02-core-runtime.md` C7 and C7.1–C7.7;
  `roadmap/00-backlog.md` B3–B8.
- Related prior entry: `devlog/2026-07-24-b2-blocked-task-state/` — same
  observable class (ready queue never drains to `on_idle`).

## Corrections

**2026-07-26 — B3 root cause isolated; this entry's hypothesis was wrong.**
The body above records B3's root cause as narrowed but not isolated, and offers
a **[INFERENCE]** lead that the `KernelObject::SharedBufferLoan` variant widening
`Capability`/`Task` was the likely mechanism, by analogy with the B2
`copy_from_current` overflow. That lead was incorrect. The actual cause is a
kernel-stack overflow at the *static initializer*, not at any widened struct:
`SHARED_BUFFER_TABLE` was a `LazyLock`, so the 10520-byte `SharedBufferTable`
was constructed as a temporary on whichever stack first touched it — a 32 KiB
unguarded task kernel stack inside `task::terminate`. The step-10 reasoning
above (that empty tables make `reclaim_owner` iterate nothing) was correct and
did rule out the loop body; it simply did not go on to consider the cost of
*reaching* the static rather than walking it. Fixed by const-initializing the
table into `.bss`. Full analysis:
`devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/`. The Status field is
updated from `Root-caused` to `Verified`; the body's observations and evidence
are unchanged and remain accurate.
