# B16 — a supervision termination record was never reclaimed

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root/src/{supervision,graph,transit,main}.rs`, `components/bins/src/bin/{init,supervision-child}.rs`, `contracts/generation/v1/fixtures/sel4-supervision.{zti,md}`, `scripts/build/{boot_layout,build-sel4,build-generation}.py`, `scripts/check/check-sel4-supervision-plane.py`, `Justfile` |
| Roadmap | B16, B22, B23, P5.4 |
| Gates | `just sel4_supervision_check` |
| Trigger | B16, opened during the P5.3.3 review and deferred three times; P5.4 named as its trigger |
| Baseline | Nine seL4 gates passing; `MAX_RECORDS = 32` never reached by any declared generation |

## Summary

`supervision::Terminations` never removed a record, by design — two parents may
hold handles to one child and each is owed the answer — so `MAX_RECORDS` (32,
from `MAX_TASKS`) bounded the tasks a boot could **ever** create rather than the
outcomes owed at once. `TaskTable::reclaim` frees entries while `next_id` keeps
counting, so a graph that spawns and reaps repeatedly exceeded the bound while
holding only a few tasks; past it, `record` dropped silently and every later
`supervision_status` answered `WouldBlock` forever. Fixed by `supervision::sweep`,
which reclaims records no live holder can name — derived from state that already
exists rather than from a reference count. Verified by a new gate that creates 35
tasks over one boot (`terminated=38`) and still answers for a handle held across
the crossing and one parked in `Transit` across it, with both fault injections
confirmed failing.

## Observable symptom

- Command: none before this change — the defect was latent.
- Expected: a graph creating more than `MAX_RECORDS` tasks over its lifetime
  answers `supervision_status` correctly for every live handle.
- Observed (fault injection 1, sweep removed): the loop dies at the 33rd child
  with `SLIME_GRAPH FAIL termination lost task=33 reason=records-full`.
- Exit/fault/serial evidence: [`fault-injection-1-no-sweep.log`](fault-injection-1-no-sweep.log).

Worth stating plainly: the symptom above is *this change's own* reporting line.
Before this change the same condition produced **no output at all** — the record
was dropped and the parent waited forever. That is what made the defect latent
rather than observable, and converting it into a reported failure is part of the
fix rather than incidental to it.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `MAX_RECORDS` is `MAX_TASKS`, but `reclaim` decrements `len` while `next_id` never rewinds | The bound is on *lifetime* tasks, not live ones — B16's premise confirmed at the source |
| 2 | B16 proposed two fixes: refuse the spawn, or reference-count handles | The refusal makes B16's own exit condition unobservable — a graph that cannot exist cannot be gated. Rejected on the entry's terms |
| 3 | `serve_supervision_status` consumes the handle via `drop_slot`; `graph.release(task)` drops a reclaimed task's whole table | The live-holder set is already represented, so a count is redundant |
| 4 | `valid_rights` grants supervision `RIGHT_TRANSFER`; `serve_cap_transfer` parks the capability in `Transit`, held by no table | A predicate over live tables alone would free a record mid-transfer — B16 reintroduced by its own fix |
| 5 | Swept every module in `slime-root/src` for tables that never free on reclaim | Only `Terminations` and `ChannelTable` qualify; the latter is now B22 |
| 6 | Every channel-free component candidate still sends on a service endpoint | The loop child must be a new binary, or B22's bound binds before B16's |
| 7 | First gate run: `freed=32 live=1`, retained handle lost its outcome | Not the sweep — a driver bug. `wait` was never called, so children 3 and 4 had no records at all, and the transfer freed the slot the retained spawn then reused |
| 8 | Fixed the driver to `wait` without collecting, and to spawn after the transfer | `freed=30 live=3` — the retained record, the in-flight record, and the current one all preserved |

## Root cause

`Terminations::record` filled the first free slot and silently did nothing when
none existed. The module's own doc-comment described this and named B16.

The violated invariant is the module's stated purpose: *a parent that holds a
supervision handle is owed the answer*. A dropped record does not make
`supervision_status` fail — it makes it answer `WouldBlock` indefinitely, which
is indistinguishable from a child that is still running. So the failure mode is
the exact one the module exists to prevent, arriving through its own bookkeeping
rather than through a missed wake.

Not an innocent bystander: `MAX_RECORDS` itself is fine. The defect is that the
table had no reclamation path at all, so the constant was measuring the wrong
quantity.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `supervision.rs` | `sweep(&mut Terminations, &GraphTables, &Transit)` frees every record no live holder can name; `record` returns `bool` and is `#[must_use]` | `MAX_RECORDS` bounds records *observable at once*, not tasks a boot may create |
| `graph.rs` | `GraphTables::holds_supervision(TaskId)` scans live tables | The live half of the predicate |
| `transit.rs` | `Transit::holds_supervision(TaskId)` scans parked entries | The in-flight half — without it the fix reintroduces the defect |
| `main.rs` | `record_termination` sweeps and retries on full, then reports `SLIME_GRAPH FAIL … reason=records-full` | A lost outcome is reported, never silent |
| `main.rs` | `Terminations` splits cumulative `recorded` from live `len`; the `terminated=` marker reads `recorded` | The boot transcript still reports what happened after records are reclaimed |
| `supervision-child.rs` | New channel-free component | The loop crosses B16's bound rather than B22's |
| fixture, `boot_layout.py` | `sel4-supervision.zti` with `transferable = true` on the child's executable, matched by `0x1000c` in the layout | B10's fixture/layout agreement; enables the transit arm |
| `check-sel4-supervision-plane.py`, `Justfile` | The eighth seL4 image and its gate | B16's exit condition is observable |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The sweep stops running | `just sel4_supervision_check` | `SLIME_GRAPH FAIL termination lost task=33 reason=records-full` |
| The sweep stops consulting `Transit` | `just sel4_supervision_check` | `[init] supervision plane fail: a handle parked across the crossing lost its outcome` |
| The sweep becomes too aggressive | `just sel4_supervision_check` | `[init] supervision plane fail: a retained handle lost its outcome` |
| The loop stops crossing the bound | `just sel4_supervision_check` | `terminated=` outside `33..=99` in the terminal marker |
| The loop child grows a channel | `just sel4_supervision_check` | `check_loop_child_is_channel_free` fails against the source |
| The `recorded`/`len` split changes existing gates | `just sel4_spawn_check` | `terminated=` no longer matches |
| The new bin perturbs generation identity | `just generation_check`, `just contracts_check` | identity mismatch or stale layout resource |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_supervision_check` | Pass — 35 tasks, `terminated=38`, `freed=30 live=3` | Direct |
| Fault injection 1: sweep call removed | Fails at `termination lost task=33 reason=records-full` | Direct |
| Fault injection 2: `Transit` half of the predicate removed | Fails at `a handle parked across the crossing lost its outcome`, every earlier marker still passing | Direct |
| `just sel4_spawn_check` | Pass, `terminated=5` — byte-identical to the P5.3.3 baseline | Direct |
| `just sel4_root_boot_check`, `sel4_component_graph_check`, `sel4_channel_check`, `sel4_loan_check`, `sel4_sample_check`, `sel4_stream_check` | All pass | Direct |
| `just generation_check`, `just contracts_check` | Pass — `default_boot_layout.rs` regenerated; the x86 profile prunes the new row | Direct |
| `just fmt_check_all`, `just lint_all` | Pass | Direct |
| `slime-root`'s four new unit tests | **Not run — nothing runs them.** See B23 | Unobserved |

Fault injection 2 is the one worth naming. The `Transit` half is invisible to
any gate whose driver never moves a handle, and a plausible fix that omitted it
would have passed every other marker in this gate. That is precisely how the
defect would have been reintroduced by its own fix.

## Decisions

- Decision: reclaim by a **derived sweep**, not by the reference count or the
  spawn refusal B16 proposed.
- Rationale: the refusal cannot satisfy B16's own exit condition — it makes the
  graph the condition requires impossible to construct, so choosing it would
  mean amending the condition in the change that claims to meet it. The count is
  redundant because the live-holder set is already represented in `GraphTables`
  and `Transit`. The sweep also fails safe: one that does not run leaves a record
  that still answers correctly, whereas a missed decrement loses one forever.
  Same choice, same reason, as `TaskTable::live_children`.
- Rejected alternative: a `serve_spawn` refusal as a backstop — it is the same
  refusal, and it would fire in exactly the graph this gate needs to run.
- Rejected alternative: lowering `MAX_RECORDS` behind a cargo feature for the
  gate. Kept in reserve and not needed: the observed `SLIME_ROOT allocator` line
  showed ample headroom. It would also have booted a root configured unlike
  every other gate's, in tension with the one-image-per-gate rule.

- Decision: sweep **lazily, on full**, rather than on every termination.
- Rationale: one trigger condition is one thing to keep correct. Sweeping
  eagerly would add a scan to every child death for no benefit, since a record
  that stays is a record that still answers.
- Consequence, recorded rather than hidden: because both call sites record
  *before* `GraphTables::release` and `Transit::reclaim`, a sweep fired from a
  dying task's own record still sees that task's holdings as live and collects
  less than the theoretical maximum. That is the safe direction; the next sweep
  collects them.

- Decision: a new `supervision-child` binary rather than reusing `sysinfo`.
- Rationale: `ChannelTable` never reclaims either (B22), so a channel-per-child
  loop exhausts `MAX_CHANNELS` — also 32 — one iteration before reaching the
  record bound. The gate would fail for a reason unrelated to what it tests.
- Cost, stated plainly: this weakens the "no new component binary" property the
  other planes have. It does not touch the frozen oracle, which is `kernel/`,
  but it does mean `generation_check`/`contracts_check` carry a real duty here.

- Decision: init is its own transit peer.
- Rationale: `cap_transfer` needs a peer that *collects* a capability, and every
  unmodified component either ignores the capability array or never receives at
  all. Init holding both ends keeps the in-flight window open across the loop
  without inventing a second binary whose only job is to wait.

## Open risks and follow-ups

- [ ] **B22** — `ChannelTable` never reclaims, same defect shape. Opened, not
      fixed; scheduled under P5.4.1, which audits lifetime-vs-live bounds as a
      class. A sweep of `slime-root/src` during this fix established that
      `Terminations` and `ChannelTable` are the only two per-task tables that
      never free, which is what makes deferring one of them safe rather than
      lucky.
- [ ] **B23** — `slime-root`'s 98 unit tests are run by nothing, including the
      four this change adds. `just sel4_supervision_check` is the sole
      observation point for this fix, which is why it carries two fault
      injections rather than one.
- [ ] `MAX_GRAPH_ITERATIONS` (512) is the tightest *estimated* margin here. The
      terminal marker is asserted last so iteration exhaustion cannot pass as
      success, but the plane's actual cost was not measured.
- [ ] The base boot layout is now 62 of `MAX_BOOT_LAYOUT_ENTRIES` (64). Two more
      appends and a new plane needs an override or replacement instead.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`supervision-plane-boot.log`](supervision-plane-boot.log) —
  the full passing boot.
- Serial/debugger/model output:
  [`fault-injection-1-no-sweep.log`](fault-injection-1-no-sweep.log),
  [`fault-injection-2-no-transit.log`](fault-injection-2-no-transit.log).
- Related roadmap item: [B16](../../roadmap/00-backlog.md) (resolved),
  [B22](../../roadmap/00-backlog.md) and
  [B23](../../roadmap/00-backlog.md) (opened),
  [P5.4](../../roadmap/07-architecture-portability.md) (the trigger).
