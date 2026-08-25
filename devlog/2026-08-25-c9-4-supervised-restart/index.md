# C9.4: the root charges the bound, and userspace decides the restart

| Field | Value |
|---|---|
| Date | 2026-08-25 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/lifecycle-policy/v1/`, `contracts/generation/v1/{schema.zt,fixtures/sel4-lifecycle-restart.zti}`, `contracts/generation/v5/schema.zt`, `contracts/syscall-abi/v1/schema.zt`, `boot-contracts/src/{lifecycle_policy.rs,generation.rs,lib.rs}`, `slime-root/src/{lifecycle,generation,graph,ipc,fault,main,lib}.rs`, `components/runtime/src/{syscall.rs,syscall/sel4_transport.rs,lib.rs}`, `components/bins/lifecycle-restart-probe/`, `components/bins/init/src/main.rs`, `scripts/build/{build-sel4,build-generation}.py`, `scripts/generate/generate-boot-bindings.py`, `scripts/check/{check-sel4-lifecycle-restart-plane,check-sel4-boot-layout,check-sel4-gate-controls}.py`, `docs/{syscall-abi,capability-matrix}.md`, `Justfile` |
| Roadmap | C9.4, C9, C9.1, C9.2, C9.3, B71, B76 |
| Gates | `just lifecycle_restart_check`, `just sel4_gate_control_check`, `just sel4_boot_layout_check`, `just contracts_check`, `just generation_check`, `just test_sel4_root`, `just test_host`, `just component_crate_split_check`, `just lint_all`, `just fmt_check_all` |
| Trigger | C9.4 became the next uncompleted milestone: the backlog is empty, C9.3 closed on 2026-08-25, and RP3/RP4/P4 are deferred on a USB-UART adapter |
| Baseline | Nothing restarted anything. `ComponentSpec.lifecycle` was a `List Text` validated for canonical order on the host and dropped by generation derivation, so no runtime could refuse a transition; `fault.rs` carried `Timeout`, `PeerLoss`, and `Unhealthy` terminal states with no constructor and no test; and there was no parameter state at all, declared or otherwise |

## Summary

A generation now declares a lifecycle transition graph, a per-instance restart
bound with growing backoff, health dependencies that gate a start, and parameter
authority — and a *userspace* supervisor restarts a failing component under all
four while `slime-root` gains no restart policy. The root's whole contribution is
mechanism: it records why a task ended, answers what the generation admits,
charges the declared attempt, computes the declared backoff instant, and refuses
every request the policy does not cover.

The load-bearing property is observed rather than asserted: one transcript carries
a component faulting, exiting cleanly, and declaring itself unhealthy across three
supervised restarts, with each replacement reading back *why its predecessor
ended* and behaving differently because of it — then the attempt bound spending
out, the instance entering the declared terminal state, and its next spawn refused.

C9.4's fourth deliverable asked for `fault.rs`'s three dead terminal states to get
production callers *or be deleted*. Two were deleted. `Unhealthy` became real, as
`lifecycle::Terminal::Unhealthy` recorded by the operation that observes it, but
`Timeout` and `PeerLoss` have no mechanism that can produce them — there is no
timeout in the root, and a native seL4 Endpoint has no closed-peer signal, which
is why `IpcError` carries no `PeerDead` either (B76). A first revision of this
slice gave both of them contract cause ids anyway; review caught that it was
*widening* the declared-but-unreachable surface the deliverable existed to close,
so the ids were dropped and `fault.rs`'s two methods deleted with them.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/lifecycle-policy/v1/` | New Zutai contract: the admitted transition graph with declared entry and exhaustion states, per-instance `(attempts, causes, backoffNs, backoffFactor)`, health dependencies naming a required state per edge, and `(holder, subject, read, write)` parameter edges. A self-transition, a self-dependency, a cause outside the closed vocabulary, a shrinking backoff factor, and a parameter edge carrying neither bit all fail to decode | Every cross-boundary format is a versioned Zutai schema; a policy that could never fire is a generation that does not decode |
| same | `RestartPolicy::backoff_for` computes the declared delay, saturating at the contract ceiling | The root and the supervisor resolve *one* growth rule, so a supervisor cannot wait for a delay the root does not recognize |
| `contracts/generation/v1/schema.zt` | `lifecyclePolicy? : LifecyclePolicy` carrying transitions, restarts, dependencies, and parameters together | None of the four is a policy alone: a bound without a terminal state, or a graph without an entry, states nothing |
| `contracts/generation/v5/schema.zt` | `LIFECYCLE_RESTART` (31), `PARAMETER_READ` (32), `PARAMETER_WRITE` (33) | C9's authorities are rights, so each lands in the vocabulary with the operation it gates (README invariant 4) |
| `boot-contracts/src/lifecycle_policy.rs` | Decoder: ascending duplicate-free tables, both graph endpoints declared and distinct, an edge into the terminal state required whenever any restart bound exists, attempts within the ceiling, and reserved bytes zero | An exhausted instance cannot be placed in a state the graph does not reach |
| `slime-root/src/lifecycle.rs` | `LifecycleService`: per-task state keyed by `TaskId`, and attempt counts, parameter values, and the last terminal cause on a per-*instance* row that outlives every task representing it | `TaskId` is never reused, so a per-task counter would reset on the very death it must bound |
| same | `terminal` (pending, consumed by the admission that charges for it) and `last_cause` (retained) are separate fields | One death charges one attempt, *and* a replacement can still read why its predecessor ended |
| `slime-root/src/generation.rs` | `lifecycle_policy_admission`: every identity is a declared instance, and every restart *and dependency* subject is owner-spawned | Both authorities ride on a spawn-minted supervision handle, so an edge naming a root-autostart subject could never be reached and is refused rather than left to silently never apply |
| `slime-root/src/fault.rs` | `Termination` narrowed to `{Exit, Fault}`; `LifecycleEventKind` lost three arms; `timeout`/`peer_lost`/`unhealthy` deleted | A terminal state no mechanism can reach reads as implemented capability to anyone auditing the supervision surface |
| `slime-root/src/main.rs` | `serve_lifecycle_request` plus three spawn-time refusals — exhaustion, pending backoff, unsatisfied dependency — each routed through `LifecycleError` so its status and its marker class have one source | A supervisor that ignores an admission refusal, skips its own wait, or starts a dependent early is refused by the mechanism rather than trusted |
| `contracts/syscall-abi/v1/schema.zt`, `docs/syscall-abi.md` | `STATE_READ` (52) and `STATE_ADVANCE` (53) self-scoped on `lifecycle`; `RESTART_ADMIT` (54), `PARAMETER_READ` (55), `PARAMETER_WRITE` (56) on `supervision` | A component reads and moves its *own* state; anything naming another component resolves it through a capability, never a wire task id (B42) |
| `components/bins/lifecycle-restart-probe/` | Four roles selected from authenticated authority: the supervisor, the restarted worker, a graph walker, and a denied instance | What a component may do is generation data, so the role is discovered rather than compiled in |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A restart loop outlives its declared bound | `just lifecycle_restart_check` | `check_restart_sequence` requires exactly `attempts` admissions with monotonically decreasing remaining budget; the terminal chain requires both the admission *and* the following spawn refused |
| Backoff degrades into a spin count | `just lifecycle_restart_check` | The declared growth is recomputed from the contract's own arithmetic and each observed `now` compared against the admission's `ready_at`; a `class=backoff-pending` refusal is required before the wait |
| The three terminal causes collapse into "it died" | `just lifecycle_restart_check` | Both the root's records and each replacement's own `cause=` reading must cover fault, exit, and unhealthy |
| A predecessor handle keeps answering | `just lifecycle_restart_check` | One stale-handle refusal is required per observed death, counted exactly |
| A restart silently loses its class or quota | `just lifecycle_restart_check` | `check_class_and_quota_survive` requires, once per incarnation, `SLIME_SCHED class` at the declared band, the builder's own `SLIME_GRAPH schedule` record agreeing with it, and `SLIME_MEM quota` whose *installed* pages equal the declared ceiling — `declared=` alone is a manifest lookup identical by construction |
| A health dependency gates nothing | `just lifecycle_restart_check` | `check_fixture_shape` refuses a fixture whose dependency requires the state its dependency boots in; the chain requires a `class=lifecycle-dependency` refusal *before* the advance that satisfies it |
| A restart row is skipped at admission | `just lifecycle_restart_check` | Startup fatals when the count of subjects admission *proved* owner-spawned disagrees with the resource's own restart count, and `admitted=` carries the former beside `restarts=` in the policy marker |
| A transition graph is carried but not enforced | `just lifecycle_restart_check`, `just test_host` | `DecodeError::SelfTransition`/`UnreachableState`, and a refused `Ready -> Initialize` that leaves the state unmoved |
| A marker is deleted, reordered, or a failure marker appears | `just sel4_gate_control_check` | 55 pinned markers, mutation-tested with the other 38 gates |
| The new plane's capability layout drifts | `just sel4_boot_layout_check` | 30 frozen plane layouts |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just lifecycle_restart_check` | Pass. Generation 44 boots; `SLIME_LIFECYCLE policy transitions=5 restarts=1 admitted=1 dependencies=1 parameters=4 initial=Initialize terminal=Error`; the worker's spawn is refused `class=lifecycle-dependency` until the supervisor advances to `Running`; three restarts are admitted at attempts 0/1/2 with remaining 2/1/0; the first is refused `class=backoff-pending` before its wait; exhaustion prints `SLIME_LIFECYCLE terminal … state=Error attempts=exhausted` and the following spawn is refused `class=lifecycle-exhausted`; `SLIME_GRAPH HEALTHY generation=44 required=4 live=0 completed=4 failed=0` | Direct |
| Cause sequence, from the same transcript | `cause=fault` → worker reads `cause=2`, exits cleanly → `cause=exit` → worker reads `cause=1`, declares unhealthy → `SLIME_LIFECYCLE unhealthy … cause=unhealthy` → worker reads `cause=3`, exits. Each replacement's behaviour is selected by the cause it read, so the three are distinguishable from inside the restarted component and not only from its supervisor | Direct |
| Configuration survival, from the same transcript | One `worker parameter previous=0` write, then `parameter value=4242` from all four incarnations, across three restarts and four never-aliasing task ids | Direct |
| Class and quota survival | `SLIME_SCHED class task=N instance=lifecycle-worker class=normal priority=150` and `SLIME_MEM quota … declared=4 installed=4` once per incarnation | Direct |
| `just sel4_gate_control_check` | Pass: 39 gates reject 1518 mutated transcripts and layouts | Direct |
| `just sel4_boot_layout_check` | Pass: 30 plane layouts match their fixtures | Direct |
| `just contracts_check` | Pass, including `docs/syscall-abi.md` documents all 43 declared operations | Direct |
| `just generation_check` | Pass: two isolated builds byte-identical; four resealed CPU-budget mutations refused | Direct |
| `just test_host` | Pass: 284 boot-contract tests, 19 of them lifecycle-policy | Direct |
| `just test_sel4_root` | Pass: 183/183 across 19 modules (was 170) | Direct |
| `just component_crate_split_check` | Pass: 58 component crates, each one package with one binary | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just deny`, `just machete`, `just miri` | Pass | Direct |
| Builder↔decoder round trip | The builder's 544-byte payload for this fixture decoded through `boot_contracts::lifecycle_policy` with every count and both backoff steps as declared, before the plane was ever booted | Direct |
| Two review rounds over the diff | Round 1 returned `incorrect` with eight findings — one dead-surface P1, one admission P1, two coverage P1s, three dead-declaration P2s, one convention P3 — all applied or explicitly answered. Round 2 confirmed all eight closed and found nine more against the response changes: two leaked-state P2s, two vacuous-assertion P2s, and five documentation or precision P3s. All nine applied. See Decisions | Direct |

## Decisions

- Decision: `Timeout` and `PeerLoss` were **deleted** rather than given contract
  cause ids, and `Unhealthy` was given a real one.
- Rationale: found by review, against my own first revision. C9.4's deliverable
  offered both options, and the first revision took neither cleanly: it declared
  `timeout` and `peerLoss` cause ids in the new contract while leaving
  `fault.rs`'s methods uncalled. That is strictly worse than the state it
  replaced — a manifest could write `causes = ["timeout"]`, pass the builder and
  the decoder, boot, and then refuse every death with `UnadmittedCause`, which is
  a declared policy that silently never fires. There is no timeout mechanism in
  this root and no closed-peer signal on a native Endpoint (B76's finding), so
  neither cause has a producer to wire. `Unhealthy` does: a component declaring
  itself broken is observable, and it must be distinguishable from the plain exit
  that follows, since `unhealthy()` exits immediately afterwards.
- Rejected alternative: keeping the two ids as "reserved for a future mechanism".
  A reserved id in a closed vocabulary the decoder admits is indistinguishable at
  runtime from a supported one.

- Decision: attempt counts, parameter state, and the last terminal cause live on
  a per-*instance* row that outlives every task; only the lifecycle *state* is
  keyed by `TaskId`.
- Rationale: `TaskId` is never reused, so a per-task attempt counter would reset
  on exactly the event it is meant to bound — the death that triggers the
  restart. The same reasoning puts parameter state there: a value a supervisor
  writes must be what the *replacement* is started with, which makes it a
  property of the declaration rather than of a task lifetime.
- Consequence: `RESTART_ADMIT` resolves its subject's instance from the instance
  row (the task row is gone by then, released on the death being answered),
  while the parameter operations resolve a *live* task. One `released` flag
  distinguishes the two, because admitting a released subject to a parameter
  write would be a write nothing reads until a restart that may never come.

- Decision: `PARAMETER_SELF_SLOT` (`u32::MAX`) names the caller's own instance.
- Rationale: no component holds a supervision capability naming itself — the
  root mints one only for a spawner — so without a sentinel a *reflexive*
  parameter edge would decode, admit, and be unreachable: the exact
  declared-but-never-applied shape B71 closed. It confers nothing, because
  `parameter_authority` still requires the declared edge, and an instance the
  policy grants no reflexive edge is refused with it. The plane observes both
  sides: `lifecycle-graph` holds a write-only reflexive edge and is refused its
  read, `lifecycle-denied` holds none and is refused both.

- Decision: the supervisor blocks on a C9.1 timer between status polls.
- Rationale: found during bring-up of the class-preservation arm, and it is a
  real interaction rather than a style choice. Once `lifecycle-worker` is declared
  at `normal`/150 and the supervisor runs at the root's 254 default, a busy
  `STATUS` poll starves the worker completely and the gate times out —
  and `yield_now` does not help, because a yield re-schedules within the same
  band. Blocking hands the CPU down. This is C9.3's observation from the other
  side, and it is why that plane's foreground component blocks rather than spins.
- Rejected alternative: raising the worker's band above the supervisor's. That
  would make the class-preservation assertion pass for the wrong reason, since a
  worker that outranks its supervisor is scheduled regardless of the declared
  policy.

- Decision: `LifecycleError::BackoffPending` was made load-bearing rather than
  deleted.
- Rationale: review found it unconstructed. The alternative to deleting it was
  better: `restart_ready` now returns it and `serve_spawn` maps it through the
  same `lifecycle_error_status`/`lifecycle_error_class` pair every other
  lifecycle refusal uses, so the status and the marker class have one source
  instead of being restated at the call site. It maps to `WouldBlock` rather than
  `InvalidOperation`, on `SUPERVISION STATUS`' rule: the request is well-formed
  and the answer is "not yet".

- Decision: a termination record reports the cause it *holds*, not the cause the
  caller passed.
- Rationale: found by round-two review, and it made one transcript
  self-contradictory. `record_termination` is first-writer-wins, but it returned
  `Some(instance)` either way — so when a component declared itself unhealthy and
  then exited (as `unhealthy()` always does, immediately), the EXIT path's write
  was correctly discarded while the marker still printed `cause=exit`. The
  transcript then carried two root-attributed causes for one death with the wrong
  one last. Returning `(instance, recorded)` makes the print and the record one
  fact, and the host test now pins that a later writer is told the recorded cause.

- Decision: the admission count is a tally of what admission *proved*, not the
  resource's own count.
- Rationale: also round two. The first revision compared
  `admission.lifecycle_restarts` against `policy.restart_count()` and fatalled on
  disagreement — but both sides were the same decode over the same immutable
  bytes, so it was `x != x` and the guard was unreachable. C9.3's analogous check
  works because its two producers are the root and the *builder*. The count is now
  incremented once per restart subject admission proves owner-spawned, which is a
  fact only the ownership forest establishes, so a skipped row moves it.
- Rejected alternative: deleting the field. The check is worth having; it was the
  *source* that was wrong, not the idea.

- Decision: the quota assertion compares `installed=`, and the class assertion
  cross-checks the builder's `ScheduleRecord`.
- Rationale: round two again, and the same vacuity in a different place.
  `declared=` is a manifest lookup keyed by instance name, identical on every
  incarnation by construction, so comparing it to the fixture compared the
  manifest to itself; only the occurrence count carried information. `installed=`
  is what the root actually placed. The class half had the same weakness — the
  root's `priority=` is the policy-resolved number — so the builder's own
  schedule record is now required to agree, which is exactly what C9.3's plane
  does.

## Open risks and follow-ups

- [ ] `slime_rt::Termination` still decodes wire kinds 2/3/4 as
      `Timeout`/`PeerLoss`/`Unhealthy`, and four components match on them, while
      the root's `supervision::Termination::encode` emits only 0 and 1. This
      slice deleted the *root-side* dead states and deliberately did not touch
      the runtime's, because that decode predates C9.4 and narrowing it is a
      behavioural change to four shipped components. It is the same dead-arm
      shape B76 closed for `IpcError::PeerDead`, one layer out, and it should be
      the next audit's starting point.
- [ ] The C9.4 restart mechanism is exercised only by this plane's own probe. No
      shipped component holds `lifecycleRestart`, and the product graph restarts
      nothing. That is ordinary work now the mechanism is proven on a boot, but
      it is a behavioural change to shipped components and does not belong in the
      slice that built the mechanism — the same follow-up C9.3 recorded for
      `CLASS_READ`.
- [ ] `lifecycle-restart-probe` has no `contracts/component-spec/v1/components/`
      record. Review flagged this against `AGENTS.md`'s "Adding a component" row,
      and it is a real divergence — but a pre-existing one: `scheduling-class-probe`,
      `wait-set-probe`, `clock-authority-probe`, and `private-memory-probe` have
      none either (42 records for 58 crates). Adding one for this probe alone
      would deepen the inconsistency rather than remove it, so the guide and the
      probe convention should be reconciled deliberately in their own change.
- [ ] Health dependencies are evaluated only on the `SPAWN` path. The root's own
      autostart path still uses `Instance.dependencies`' one-shot barrier, and
      admission now *refuses* a lifecycle dependency whose subject is
      root-autostart rather than evaluating one. That keeps the mechanism honest,
      but it means a composition wanting readiness-gated autostart has no way to
      declare it.
- [ ] `components/runtime/src/syscall.rs`'s five new wrappers are the part of
      this slice no host gate compiles, for C9.2's recorded `sel4-alloca` reason.
      The plane exercises them on a real boot.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none retained; the gate's own assertions and the marker
  contract in `scripts/check/check-sel4-lifecycle-restart-plane.py` are the
  record.
- Serial/debugger/model output: quoted inline under Verification from a
  `just lifecycle_restart_check` run.
- Related roadmap item:
  [`roadmap/02-core-runtime.md`](../../roadmap/02-core-runtime.md) C9.4, whose
  dependencies are
  [`devlog/2026-08-24-c9-1-clock-authority/`](../2026-08-24-c9-1-clock-authority/index.md)
  and
  [`devlog/2026-08-25-c9-3-declared-scheduling-class/`](../2026-08-25-c9-3-declared-scheduling-class/index.md),
  whose plan is
  [`devlog/2026-08-24-c9-decomposition/`](../2026-08-24-c9-decomposition/index.md),
  and whose predecessor
  [`devlog/2026-08-25-c9-2-bounded-wait-sets/`](../2026-08-25-c9-2-bounded-wait-sets/index.md)
  established the supervision badge this slice's death observation rests on.
  C9.3's deferred restart-survival check is closed here.
