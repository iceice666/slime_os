# Planning C9: every mechanism exists, none of it reaches a component — and two deliverables the platform cannot hold

| Field | Value |
|---|---|
| Date | 2026-08-24 |
| Kind | Decision |
| Status | Proposed |
| Scope | `roadmap/02-core-runtime.md` (C9 body, architecture decisions, track status, sequencing), `roadmap/README.md` (index row, track map), `roadmap/09-rpi5-ros2-demo.md` (RP5 dependency), `roadmap/06-authority-trust.md` (composite boundary, A3 dependency), `roadmap/08-native-development.md` (track dependencies, sequencing, D3, D4), `roadmap/00-backlog.md` (C10.4's deferred follow-ups), `docs/directions/README.md` (lifecycle route, entry-32 row), `devlog/README.md` |
| Roadmap | C9, C9.1, C9.2, C9.3, C9.4, C9.5, C9.6, C10, C10.4, C8.11, C8.15, RP5, A3, D3, D4, B46, B48 |
| Gates | `just devlog_check` |
| Trigger | C9 became the next uncompleted milestone when C10.4 closed the C10 track on 2026-08-24. The backlog is empty and C9's dependencies (C8, P5) are complete, so nothing else gates it |
| Baseline | C9 was one undecomposed heading: 16 bullets spanning clocks, wait sets, scheduling classes, lifecycle/restart, record-replay, and a sensor→controller→actuator workload, with one planned gate (`just robot_runtime_check`) for all of it |

## Summary

C9 is now C9.1–C9.6, sequenced so each slice's evidence is the next one's
precondition. Two of the original deliverables were rescoped against the pinned
platform rather than carried forward as plans, which is the substance of this
entry: as written, C9's first required check could not be satisfied on this
kernel configuration, and its scheduling deliverable rested on a kernel feature
B48 deliberately declined. Both are now recorded as walls in the track's
architecture decisions, in the same shape as B48's own MCS deferral.

This is documentation only. No runtime tests were run; the only gate that
applies is `just devlog_check`.

## Changes

| Area | Change | Established boundary |
|---|---|---|
| `roadmap/02-core-runtime.md` | C9 body replaced by C9.1–C9.6, each with deliverables, required checks, a named gate, and an exit condition | One slice, one primary state surface, one independently reviewable gate — the C7/C8/C10 convention |
| same | New `### Architecture decisions` fixing four cross-slice rules, two of which record a wall | A rescoping is a recorded decision, not a quietly dropped bullet |
| same | New `**Motivation:**` naming what already exists and what does not reach a component | The track starts from measured state, not from an empty field |
| same | Track status and sequencing updated | C9 is the one open milestone; C10 closed 2026-08-24 |
| `roadmap/README.md` | Index row rewritten; `C9` node added to the track map with `C8 --> C9`, `P5 --> C9`, and a dashed `C9 -.-> RP5` | The index names the next open gate as C9.1, not the whole track |
| `roadmap/09-rpi5-ros2-demo.md` | RP5's dependency narrowed from "the subset of C10/private-memory, clock/timer, and datagram/network service" to C10 plus **C9.1 and C9.2 specifically** | The demo path is blocked on two slices, not on a six-slice track |
| `roadmap/06-authority-trust.md` | Composite boundary drops "composition with resource-account quantity" from C9's ownership; A3's dependency stops inheriting entry-25's conserved account from Core runtime | A track cannot depend on a mechanism the platform does not provide |
| `roadmap/08-native-development.md` | Track dependencies, Sequencing item 2, D3's dependency, D3's charging deliverable, and D4's dependency all stop attributing conserved CPU accounts to C9; D3 now bounds CPU as wall-clock time against a deadline | The rescoping propagates to every site that consumed the disclaimed half, not just the nearest one |
| `roadmap/00-backlog.md` | C10.4's two open follow-ups recorded under Deferred follow-ups | Live follow-up work belongs in `roadmap/`, not only in a frozen devlog entry |
| `docs/directions/README.md` | Lifecycle route and entry-32 row stop routing entry 25's conserved account through C9 | The register's index-level claims are read as current, unlike the retained design prose |

## Decisions

- Decision: decompose before implementing, in a separate documentation-only change.
- Rationale: repository precedent, twice. C8 was decomposed in `d7b7efc` and its
  first slice landed afterwards; C10 was planned in `1039b22` on 2026-07-28 and
  C10.1 closed on 2026-08-23. Both kept the plan reviewable separately from the
  code. C9's 16 bullets span six state surfaces — a clock service, a userspace
  wait set, a priority mapping, a lifecycle state machine, a trace/replay
  corpus, and a composed workload — and no single gate can be the exit condition
  for all of them. One `just robot_runtime_check` covering the whole track would
  be a gate whose failure does not localize.

- Decision: C9.1's clock authority gates the *service*, and the roadmap records
  that raw EL0 counter reads are not capability-gated on this platform.
- Rationale: C9's first required check as written — "a component without clock
  authority cannot exercise that operation through another ambient API" — cannot
  be satisfied here, and I verified this in the pinned kernel source rather than
  inferring it. `armv_init_user_access` in
  `deps/sel4/src/arch/arm/armv/armv8-a/64/user_access.c` writes `CNTKCTL_EL1`
  (and, under hypervisor support, `CNTHCTL_EL2`) **once at kernel boot** from
  compile-time config; the bits are `EL0PCTEN`/`EL0PTEN`, and there is no
  per-TCB path that narrows them. `sel4/config/qemu-arm-virt.cmake:37-38` must
  set both, because `slime-root/src/platform_timer.rs` reads `CNTPCT_EL0` and
  programs `CNTP_CVAL_EL0` from EL0 itself. So the grant is all-or-nothing and
  the root is inside it: any EL0 code can execute `mrs CNTPCT_EL0`. No shipped
  component does — I searched `components/` for both the register names and
  inline `asm!`, and the only match is `tpidr_el0` for thread identity — but
  that is a property of the component code, not of a capability.
- Rejected alternative: revoke `KernelArmExportPCNTUser`/`PTMRUser` to make the
  invariant true. That leaves the root with no timer at all. PPI 30 is the only
  architected-timer PPI seL4 does not claim for itself under
  `KernelArmHypervisorSupport ON` — `CNTHP_*`/PPI 26 is the kernel's own tick
  and `CNTV_*`/PPI 27 is reserved unconditionally for VCPU maintenance — and
  non-MCS seL4 offers no kernel-mediated substitute, so `just
  sel4_root_boot_check`'s entire timer phase would have no mechanism. Closing
  this is a kernel-side question, not a config edit, and pretending otherwise in
  a contract would be worse than naming it.

- Decision: scheduling classes rest on priority; conserved CPU accounts leave
  C9's scope entirely.
- Rationale: C9's own text already required a class contract to "state which of
  the two it rests on," so this is answering its question rather than narrowing
  it. Priority is real: declared as `Instance.priority`/`Instance.workerPriority`,
  admitted, applied to the TCB, and reported as `SLIME_GRAPH schedule` since
  B48. Budgets are not: `KernelIsMCS OFF`, `budget_us` and `period_us` are
  written zero by the builder with the reason in a comment, and the kernel has
  nothing to charge. Keeping "conserved CPU resource accounts" as a C9
  deliverable would put a field in a contract that no mechanism honours.
- Rejected alternative: turn MCS on. This is an assurance decision with its own
  evidence requirements, and a budgeted-CPU slice is blocked on it rather than
  on C9. **Superseded in part:** the terms I cited here from
  `sel4/config/qemu-arm-virt.cmake:21-23` were imprecise, and
  [`devlog/2026-08-24-mcs-cost-reassessed/`](../2026-08-24-mcs-cost-reassessed/index.md)
  corrects them — upstream lists AArch64 MCS proofs as *in progress*, and this
  QEMU build is already outside the verified set, so the real costs are the
  Reply-object IPC migration and the absent declaration surface. The decision
  to keep MCS off, and C9.3's scope, are unchanged.

- Decision: C9.2's wait sets are userspace, and the root gains no wait state.
- Rationale: B46 deleted the root's `WaitSet` along with `ChannelTable`,
  `Transit`, and `ParkedReplies` when it replaced logical channels with native
  seL4 Endpoints and badged Notifications. Reintroducing a root-owned ready
  queue would undo that, and C9's own deliverable already says "built on the
  native seL4 Endpoint/Notification wait mechanism B46 established rather than a
  root-owned wait set." The root's contribution is C9.1's timer source, and the
  wait is one `seL4_Wait` on one badged notification — see the next decision,
  which is where my first draft was wrong.

- Decision: a wait set blocks on **one** notification and demultiplexes the
  badge; every admitted source is a signaller of that notification.
- Rationale: found by review, against an exit condition I had written as "blocks
  once" on "timer, message, and supervision sources". No primitive in this
  repository can do that. `notification_wait(slot)` is one `seL4_Wait` on one
  capability (`components/runtime/src/syscall/sel4_transport.rs:645`), a
  component's notifications occupy distinct CSpace slots, and the architecture
  decision above forbids the root from supplying a multiplexer — so the three
  constraints together were unsatisfiable, and a slice author would have
  discovered that only after building against it. The mechanism that does work
  is the one the repo already uses: seL4 coalesces signals onto a single
  notification and the waiter reads the accumulated badge word.
  `build-generation.py` already validates exactly that topology — one waiter,
  one or more signallers — and `sel4-call.zti` already declares it with four
  signallers, while `slime-root/src/notification.rs` already assigns
  `1 << (slot % 63)`. So the per-wait-set source ceiling is the badge width, not
  a number this slice invents, and C9.1's timer expiry becomes one more badge
  bit rather than a separate wake path.

- Decision: C9.4's restart is a userspace supervisor; the root gains no restart
  policy.
- Rationale: consistent with the standing rule that mechanism is `slime-root`
  and policy is userspace, and the existing split already lands this way — the
  root observes termination, holds single-assignment terminal state, and
  reclaims. Attempt bounds and backoff are exactly the kind of decision that
  belongs to a component holding supervision authority.

- Decision: C9.4 must either give `fault.rs`'s `Timeout`/`PeerLoss`/`Unhealthy`
  terminal states production callers or delete them.
- Rationale: they exist, they are public, they are reachable only from their own
  definitions — and they are not even tested. `fault.rs`'s four tests exercise
  `ipc_completed`, `fault`, and `exit` only; a repo-wide search for `.timeout(`,
  `.peer_lost(`, and `.unhealthy(` finds no call site at all. That is worse than
  the "latent but covered" state I first wrote here, and it is why the choice
  belongs in the slice that would legitimately use them: dead public API on the
  supervision surface reads as implemented capability to anyone auditing it,
  which is the same failure mode as a dead guard.

- Decision: C9.2's wait set allocates from the C10 private region.
- Rationale: C10.4 just demonstrated the cost of the alternative on
  `fabric-service` — 29960 bytes of `.bss` plus `.data` reserved in ten
  generations for a graph none of them declared. A wait set sized for the
  maximum source count would be the same mistake in a new component, and the
  mechanism to avoid it now ships.

- Decision: C9.1's and C9.3's authorities are rights with capability-matrix
  rows, and C9 adds no seL4 object kind.
- Rationale: `roadmap/README.md`'s invariant 4 makes this mandatory rather than
  optional, and C10's plan answered the same question in its own architecture
  decisions — for C10 the answer was "no right, matrix unchanged," because a
  page quota designates no object. C9 is the opposite: each of the four
  authorities gates a root operation on a *named* thing a generation grants,
  which is what a right is. The timer itself stays root-brokered state keyed by
  task, so no component can name, transfer, or derive it.

- Decision: the rescoping propagates to every site that consumed the disclaimed
  half, not just to C9's own body.
- Rationale: this began as a review finding against my own edit, and the finding
  was right. I had narrowed `06-authority-trust.md`'s composite boundary and
  `08-native-development.md`'s track dependencies, which left
  `08-native-development.md:138` — D3's actual charging deliverable, the line an
  implementer works from — still charging conserved CPU first in its list. A
  reader arriving at D3 through its own heading would have gotten the pre-change
  claim. A disclaimer at a track preamble that contradicts a deliverable three
  sections below is worse than no disclaimer, because it looks settled. The same
  applies to `docs/directions/README.md`'s index-level rows, which are read as
  current even though the retained design prose beneath them is frozen.

## Open risks and follow-ups

- [ ] The EL0 counter grant is a real hole in the authority model, not merely a
      documentation nuance: a hostile component could read the counter directly
      and no capability would stop it. What C9.1 can enforce is the *service* —
      simulated time, deadline ordering, timer delivery, recorded determinism —
      and those are what C9.5's replay claim rests on. Closing the register
      question needs either a kernel change or an MCS-era rework of how the root
      gets its own timer, and neither is C9 work.
- [ ] C9.5's determinism claim is weaker than it looks while the counter is
      globally readable: a component declared deterministic could read an
      unrecorded clock without holding clock authority. C9.5's check as written
      ("a component granted an unrecorded nondeterminism source cannot be
      declared deterministic") governs *granted* sources, so the register path
      sits outside it. That limitation belongs in C9.5's own contract text when
      the slice is written.
- [ ] Six slices with six new gates is a large addition to a stack that already
      runs 34 marker gates. C9.6 is the composition, so some of the earlier
      gates may fold into it once it exists rather than all six persisting; that
      is a judgement to make when C9.5 lands, not now.
- [ ] The `robot_runtime_check` name stays on C9.6, so the roadmap's original
      planned target still resolves — but five slices before it now name gates
      that do not exist yet. A reader checking "does the named gate exist" will
      find five misses until each slice lands.
- [ ] C9.3's required check "class survives a supervised restart" is a forward
      reference to C9.4, so C9.3 cannot fully close before C9.4 exists. Stated
      here rather than silently reordered, because the alternative — moving
      class after restart — would make C9.4's "restart preserves declared class"
      the forward reference instead.

## Artifacts and provenance

- Focused report: none. The two rescopings are recorded in *Decisions* with
  their kernel-source evidence, and in the roadmap's own architecture-decisions
  section where a slice author will read them.
- Raw transcript: not retained. The two load-bearing facts are checkable
  directly: `deps/sel4/src/arch/arm/armv/armv8-a/64/user_access.c` for the
  global counter grant, and `sel4/config/qemu-arm-virt.cmake` for both
  `KernelIsMCS OFF` and the two export options and their recorded reasons.
- Verification: documentation only; no runtime tests were run. `just
  devlog_check` passes.
- Related roadmap items: [C9](../../roadmap/02-core-runtime.md),
  [RP5](../../roadmap/09-rpi5-ros2-demo.md)
- Predecessors: [`devlog/2026-08-12-b48-mcs-assurance/`](../2026-08-12-b48-mcs-assurance/index.md)
  (the MCS decision C9.3 inherits),
  [`devlog/2026-08-10-b48-declared-priority/`](../2026-08-10-b48-declared-priority/index.md)
  (the priority mechanism C9.3 rests on),
  [`devlog/2026-08-13-b46-multi-source-wait/`](../2026-08-13-b46-multi-source-wait/index.md)
  (the notification primitive C9.2 builds over),
  [`devlog/2026-08-24-c10-4-adoption-and-leak-evidence/`](../2026-08-24-c10-4-adoption-and-leak-evidence/index.md)
  (whose closure made C9 next, and whose private region C9.2 allocates from)
