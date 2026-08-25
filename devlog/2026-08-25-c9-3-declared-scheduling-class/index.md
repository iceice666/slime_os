# C9.3: a class is a priority, and the band that names it

| Field | Value |
|---|---|
| Date | 2026-08-25 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/scheduling-class/v1/`, `contracts/generation/v1/{schema.zt,fixtures/sel4-scheduling-class.zti}`, `contracts/generation/v5/schema.zt`, `contracts/syscall-abi/v1/schema.zt`, `boot-contracts/src/{scheduling_class.rs,generation.rs}`, `slime-root/src/{scheduling,generation,graph,ipc,main}.rs`, `components/runtime/src/{syscall.rs,syscall/sel4_transport.rs,lib.rs}`, `components/bins/scheduling-class-probe/`, `components/bins/init/src/main.rs`, `scripts/build/{build-sel4,build-generation}.py`, `scripts/check/{check-sel4-scheduling-class-plane,check-sel4-boot-layout,check-sel4-gate-controls}.py`, `docs/{syscall-abi,capability-matrix}.md`, `Justfile` |
| Roadmap | C9.3, C9, C9.1, C9.2, C9.4, B48, B71, B77 |
| Gates | `just scheduling_class_check`, `just sel4_gate_control_check`, `just sel4_boot_layout_check`, `just contracts_check`, `just generation_check`, `just test_sel4_root`, `just test_host`, `just component_crate_split_check`, `just lint_all`, `just fmt_check_all` |
| Trigger | C9.3 became the next uncompleted milestone: the backlog is empty, C9.2 closed on 2026-08-25, and RP3/RP4/P4 are deferred on a USB-UART adapter |
| Baseline | B48 made per-thread priority generation-declared data and `sel4-sample.zti` proved a low-priority worker cannot starve its own component's main thread. But priority was a bare number with no vocabulary: `DEFAULT_CHILD_PRIORITY = 254` was compiled into the builder and `task::CHILD_PRIORITY = 254` into the root, nothing named what a priority *meant*, and no component could observe or change one |

## Summary

A generation now declares a scheduling class per instance — `foreground`,
`normal`, `bestEffort` — and declares the mapping from each class to its exact
seL4 TCB priority. The mapping is manifest data, not a constant in the builder
and a matching constant in the root, which is what C9.3's first deliverable asks
for. A component reads its own class over one new self-scoped operation, and a
component the generation grants promotion authority over *another* component can
change that other component's class, bounded by a per-edge declared ceiling.

The load-bearing property is observed rather than asserted: a `foreground`
component makes ordered progress *between two chunks of a `bestEffort` component's
still-running 200M-iteration burn loop*, on one vCPU, which a scheduler ignoring
the declared bands cannot produce.

CPU *quantity* is bounded by nothing here, and the contract says so rather than
leaving it inferable. `KernelIsMCS OFF` gives the kernel no budget to charge and
B77 made both readers refuse a nonzero `budget_us`/`period_us`, so a class orders
access to the CPU; it does not reserve an amount of it.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/scheduling-class/v1/` | New Zutai contract: a band table (`class_id` → exact TCB priority), a per-instance assignment table, and promotion edges of `(holder, subject, ceiling_priority)`. Bands must be distinct priorities; a self-edge and a ceiling naming no declared band do not decode | Every cross-boundary format is a versioned Zutai schema; the class-to-priority mapping is declared rather than compiled |
| `contracts/generation/v1/schema.zt` | `schedulingClass? : SchedulingClassPolicy` on the manifest, carrying `bands`/`instances`/`promotions` together because none is meaningful alone | A composition declares its own band layout, its assignment, and who may repriotitize whom |
| `contracts/generation/v5/schema.zt` | `SCHEDULING_PROMOTE` at bit 30 | C9's authorities are rights, so each lands in the vocabulary with the operation it gates (README invariant 4) |
| `boot-contracts/src/scheduling_class.rs` | Decoder: ascending distinct bands within the child ceiling, ascending non-duplicate assignments naming only banded classes, ascending promotion edges that are never self-edges and whose ceilings name declared bands | A malformed or self-widening policy is refused before a thread runs under it |
| `scripts/build/build-generation.py` | `validated_scheduling_class` reads the band mapping **once** and substitutes the resulting priority into the v5 `ScheduleRecord` the builder already emits. It refuses an instance declaring both a class and a disagreeing priority — including a `workerPriority` disagreeing by inheritance | The class *is* the priority; there is no second number for the two to disagree about |
| `slime-root/src/generation.rs` | `scheduling_class_admission`: every identity the policy names is an instance this generation declares, and every promotion subject is owned by its holder | Builder and root cannot disagree, and an unreachable edge is refused rather than silently never applying |
| `slime-root/src/scheduling.rs` | `SchedulingService`: one row per live task, the class each runs at, and `promote` — which refuses `caller == subject` before any edge lookup, resolves the priority from the *generation's* band table, and enforces the edge's ceiling | Promotion is authority over another component's class and never over the holder's own |
| `slime-root/src/{graph,main}.rs` | `SupervisionRights` admits `RIGHT_SCHEDULING_PROMOTE`, and `serve_spawn` sets that bit on a spawn-minted handle exactly where the policy declares an edge from that spawner to that child | The right on the capability and the edge in the resource are one fact with one source (B71's shape) |
| `contracts/syscall-abi/v1/schema.zt`, `docs/syscall-abi.md` | `SCHEDULING CLASS_READ` (50, self-scoped, gated on `lifecycle`) and `CLASS_PROMOTE` (51, gated on `supervision`) | A component reads its own band; a promotion names its subject through a capability, never a wire task id (B42) |
| `contracts/generation/v1/fixtures/sel4-scheduling-class.zti`, `components/bins/scheduling-class-probe/` | Generation 43 and its probe: a foreground instance, a saturating bestEffort instance, a controller holding one declared edge, and an instance the policy names at all | The plane exercises the ordering claim, the ceiling, the self-widening refusal, and the deny-by-default answer |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A class and a declared priority disagree | `just generation_check`, `just contracts_check` | Builder `fail` naming the instance, both numbers, and the band |
| A generation declares a self-promotion edge, or one its holder cannot reach | `just test_host`, `just scheduling_class_check` | `DecodeError::SelfPromotion`, or root `UnsatisfiableSchedulingClass` |
| The bands stop ordering the CPU | `just scheduling_class_check` | `check_preemption` fails: no foreground step lands between two burner chunks |
| A component widens its own class | `just scheduling_class_check`, `just test_sel4_root` | `[sched] FAIL self-directed promotion was admitted`, or `a_caller_may_not_promote_itself` |
| The root's recorded class drifts from the priority the thread runs at | `just scheduling_class_check` | Every `SLIME_SCHED class` line is cross-checked against the `SLIME_GRAPH schedule` record for the same thread, all four instances |
| A marker is deleted, reordered, or a failure marker appears | `just sel4_gate_control_check` | 25 pinned markers, mutation-tested with the other 37 gates |
| The new plane's capability layout drifts | `just sel4_boot_layout_check` | 29 frozen plane layouts |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just scheduling_class_check` | Pass. Generation 43 boots; `SLIME_SCHED policy bands=3 instances=4 promotions=1 unnamed=undeclared`; each instance's root-attributed class matches the `ScheduleRecord` priority for the same thread; a declared promotion applies at `normal`/150, one band above the ceiling is refused `AboveCeiling`, `undeclared` is refused as a target, a self-directed promotion is refused, the controller's own class is unchanged, the unnamed instance reads `undeclared` at 254 and is refused all 8 swept slots, `SLIME_GRAPH HEALTHY generation=43 required=5 live=0 completed=5 failed=0` | Direct |
| Interleaving, from the same transcript | `[sched-burner] bestEffort spinning` → `progress step=0,1` → `chunk=0` → `step=2` → `chunk=1` → `step=3` → `chunk=2` → `step=4` → `chunk=3` → `step=5` → `foreground complete` → `chunk=4..9` → `bestEffort complete`. Foreground progress lands strictly between burner chunks, and the burner still finishes | Direct |
| `just sel4_gate_control_check` | Pass: 38 gates reject 1450 mutated transcripts and layouts | Direct |
| `just sel4_boot_layout_check` | Pass: 29 plane layouts match their fixtures | Direct |
| `just contracts_check` | Pass, including `docs/syscall-abi.md` documents all 38 declared operations | Direct |
| `just generation_check` | Pass: two isolated builds byte-identical; four resealed CPU-budget mutations refused | Direct |
| `just test_host` | Pass: 265 boot-contract tests, 15 of them scheduling-class | Direct |
| `just test_sel4_root` | Pass: 170/170 across 18 modules (was 160) | Direct |
| `just component_crate_split_check` | Pass: 57 component crates, each one package with one binary | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos` | Pass | Direct |
| Three review rounds over the diff | Round 1 found six issues including one P1; round 2 found the P1 fix's remaining half; round 3 confirmed correct with two sub-threshold nits, both applied. See Decisions | Direct |

## Decisions

- Decision: the class-to-priority band mapping is generation-declared data, and
  the builder substitutes a band's priority into the `ScheduleRecord` it already
  emits rather than the root deriving it a second time.
- Rationale: C9.3's deliverable is that the mapping is declared rather than
  compiled in, and the v5 `ScheduleRecord` has carried a per-thread priority
  since B48. So a class does not need a new enforcement path — it needs to be the
  thing that *chooses* the number already on the wire. Deriving the priority in
  both the builder and the root would be two readers of one mapping, which is
  exactly how the boot-layout resource drifted from the bindings it described
  (B71).
- Rejected alternative: a root-side band table applied to TCBs at launch,
  independent of the plan. It would make the `ScheduleRecord` and the class two
  statements about one thread.

- Decision: an instance the policy does not name reads back as a distinct
  `undeclared` class (id 0) at the root's own child priority, not as `normal`.
- Rationale: found by review, in two rounds. The first revision synthesized a
  `normal` assignment for unnamed instances and mapped it through the band table,
  while the builder left such an instance at `DEFAULT_CHILD_PRIORITY` — so the
  root reported 150 for a thread running at 254. Reporting `normal` at 254 fixes
  the number but not the meaning: `normal` names a band whose declared priority
  the thread is not in, and promoting such a subject *to* `normal` would look like
  a no-op class change while silently moving its priority. `undeclared` is
  therefore a fourth *name* and not a fourth band: it is declared outside
  `classNames`, so no manifest can assign it and no promotion can request it.
- Rejected alternative: reusing `normal`'s id for the unnamed case.

- Decision: promotion authority rides on the supervision capability the root
  mints for a spawner, and the generation's promotion table decides which handles
  carry the bit.
- Rationale: the operation must resolve its subject from a capability rather than
  a wire task id (B42), and a supervision handle already names exactly one
  subject. Setting `RIGHT_SCHEDULING_PROMOTE` where the policy declares the edge
  makes the right and the edge one fact with one source. The alternative — a
  separate `schedulingPromote` grant *plus* an edge — is two statements that can
  disagree, and an early revision required both before review showed the grant
  requirement was redundant with the edge.
- Consequence, now stated in the contract: only spawner-to-owned-child edges are
  admissible. Two root-owned peers cannot hold an edge, because neither would
  ever hold a capability naming the other.

- Decision: the foreground component blocks on a C9.1 timer rather than spinning
  or yielding.
- Rationale: found during bring-up, and it is the difference between evidence and
  a tautology. Under strict priority on one vCPU a spinning `foreground`
  component runs to completion before a `bestEffort` component is scheduled even
  once, so its markers all precede the burner's and prove only launch order — the
  first revision produced exactly that transcript. Blocking hands the CPU to the
  burner, and each expiry then preempts a loop the chunk markers show to be in
  flight. This is also why the burner is chunked: one marker at each end would
  leave "preempted, or merely scheduled after?" unanswerable.
- Rejected alternative: measuring elapsed time. The harness pins no `-icount`, so
  a duration is a host-load measurement rather than a scheduling property (B75).

## Open risks and follow-ups

- [ ] Only this plane's own probe reads or sets a class. No shipped component
      calls `CLASS_READ`, and no supervisor promotes anything in the product
      graph. That is ordinary work now the mechanism is proven on a boot, but it
      is a behavioural change to shipped components and does not belong in the
      slice that built the mechanism.
- [ ] "Per supervision subtree" admission is per *instance* plus the ownership
      check on promotion edges, not a first-class subtree object. The generation
      declares an acyclic owner forest (`Instance.owner`), and dynamic descendants
      are separately tracked by `Task.spawner`; C9.3 hangs off the former. A
      policy that wanted to assign a class to a whole subtree at once would need
      that bridge, which C9.4's restart work is the natural place to build.
- [ ] Class survival across a supervised restart is unobserved, deliberately:
      C9.3's own exit condition assigns it to C9.4's gate, because nothing
      restarts a component until C9.4 exists. `SchedulingService::release` drops a
      dead task's row, so a restarted instance re-derives its class from the
      generation rather than inheriting one — but that is a property of the code,
      not an observation.
- [ ] The band ceiling `MAX_BAND_PRIORITY = 254` is pinned in `boot-contracts`
      against `slime_root::task::CHILD_PRIORITY`'s 254 by comment rather than by a
      shared constant, because the two crates do not depend on each other in that
      direction. Both refuse a higher value independently, so a divergence fails
      closed, but it is two numbers.
- [ ] `components/runtime/src/syscall.rs`'s two new wrappers are the part of this
      slice no host gate compiles, for C9.2's recorded `sel4-alloca` reason. The
      plane exercises them on a real boot.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none retained; the gate's own assertions and the marker
  contract in `scripts/check/check-sel4-scheduling-class-plane.py` are the
  record.
- Serial/debugger/model output: quoted inline under Verification from a
  `just scheduling_class_check` run.
- Related roadmap item:
  [`roadmap/02-core-runtime.md`](../../roadmap/02-core-runtime.md) C9.3, whose
  dependencies are
  [`devlog/2026-08-24-c9-1-clock-authority/`](../2026-08-24-c9-1-clock-authority/index.md)
  and
  [`devlog/2026-08-25-c9-2-bounded-wait-sets/`](../2026-08-25-c9-2-bounded-wait-sets/index.md),
  whose plan is
  [`devlog/2026-08-24-c9-decomposition/`](../2026-08-24-c9-decomposition/index.md),
  and whose successor C9.4 owns the restart-survival check this slice defines but
  does not observe.
