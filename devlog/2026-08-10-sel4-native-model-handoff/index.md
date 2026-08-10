# seL4 native-capability-model handoff

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Decision |
| Status | Proposed |
| Scope | `slime-root` capability/channel/task mechanism, `components/runtime` syscall ABI, `contracts/generation/v1`, `boot-contracts/src/generation.rs`, the seL4 boot/spawn/IPC path |
| Roadmap | P5, P5.4, P5.5, C8, B34, B35, B36, B37, B38 |
| Gates | `just test_sel4_root`, `just contracts_check`, `just generation_check` |
| Trigger | A post-B34–B38 review asked which unnatural seL4 mechanisms come from Slime's retained capability/component/generation/IPC/task models, with breaking changes explicitly in scope |
| Baseline | B34–B38 closed the executable/instance conflation, compile-time generation selection, non-unique gate terminal, implicit slot ABI, and monotonic resource watermark; the resulting v4 architecture is internally consistent and green, but is still a userspace re-implementation of a microkernel's capability and IPC mechanism on top of seL4 rather than a mapping onto seL4's own primitives |

## Summary

This entry hands off a breaking-change program: replace Slime's retained
capability-table/channel-queue/task-ID compatibility model with seL4-native
mechanism — real CSpace capabilities, Endpoint/Notification IPC, MCS
scheduling contexts, and a generation format that plans exact seL4 objects
instead of naming logical grants a userspace root re-interprets. The current
v4 architecture (post B34–B38) is correct and gated green, so this is not a
defect fix: it is a proposed, sequenced architecture cutover with no
compatibility shim at any phase, matching this repository's established
cutover discipline (see Decisions). The next engineer should start at
[Changes](#changes) below, phase 0.

## Investigation log

Current-state survey, each row independently re-derivable from the cited
source.

| Step | Observation | Consequence |
|---|---|---|
| 1 | A Slime capability is a logical `(Resource, u64 rights)` pair kept in `slime-root`'s per-task `CapabilityTable`, never a seL4 capability in the child's CSpace (`slime-root/src/graph.rs:1-18`, `:469-552`). | The seL4 kernel is not the authority enforcer for component-to-component operations; `slime-root`'s table lookup is. |
| 2 | A Slime channel is a root-owned bounded queue (`CHANNEL_CAPACITY = 16` messages, `MAX_MESSAGE_BYTES = 64`, `MAX_MESSAGE_CAPS = 4`), not a seL4 Endpoint (`slime-root/src/channel.rs:1-8`, `slime-root/src/ipc.rs:10-19`). | Every message crosses the root twice (sender `Call` + root enqueue, receiver `Call` + root dequeue) instead of once through a direct Endpoint. |
| 3 | Reply capabilities, multi-source waits, and in-flight capability transit are re-implemented in userspace (`slime-root/src/parked.rs:33-53`, `slime-root/src/channel.rs:739-863`, `slime-root/src/transit.rs:1-54`). | `slime-root` re-proves atomicity and lifetime properties seL4's Endpoint, Reply object, and Notification already provide. |
| 4 | Almost the entire legacy syscall surface — 33 `Operation` labels including `Send`, `Recv`, `Wait`, `Spawn`, `CapTransfer`, `EndpointCreate`, block/store/directory/generation transacts — is multiplexed onto one badged root Endpoint per task (`slime-root/src/ipc.rs:89-141`). | One highest-priority root service loop is both the sole IPC broker and the sole driver dispatcher; a low-value client can amplify root execution and one bug widens to a whole-system failure domain. |
| 5 | Every child CSpace has exactly four slots — null, service endpoint, own TCB, fault endpoint — regardless of declared authority (`slime-root/src/task.rs:51-60`), and children run at a single fixed priority strictly below root's, with MCS disabled (`slime-root/src/task.rs:62-64`, `sel4/config/qemu-arm-virt.cmake:4`). | No native seL4 isolation or CPU-time authority differentiates components; every distinction is root-mediated software policy. |
| 6 | Generation v4 (post-B34–B38) cleanly separates `Executable` and `Instance` records with owner, autostart, dependencies, health, quota, and bindings (`contracts/generation/v1/schema.zt:15-46`), but the schema still has no `Process`, `Thread`, `KernelObject`, `Mapping`, `CapBinding`, or `Schedule` records — it names logical grants, not seL4 objects. | Declared graph topology is not provably the same as the seL4 object/authority topology the root actually builds; `sel4-call.md:30-56` explicitly documents that some grants only *name* an edge while a userspace broker mints the real Endpoint. |
| 7 | `init`'s boot graph selection is still driven by `option_env!("SLIME_GENERATION_NUMBER")` and a dozen `SLIME_*_CHECK` compile-time flags (`components/bins/src/bin/init.rs:153-283`). | The same component image behaves differently per build profile instead of being fully determined by the authenticated runtime generation. |
| 8 | Spawn's public ABI returns `Spawned { task_id: u64, supervision_slot: u32 }` (`components/runtime/src/syscall.rs:85-89`), mixing an ambient numeric identity with a capability-based one, while every component is hard-assumed single-threaded (`components/runtime/src/runtime.rs:21-24`) with one TCB per task (`slime-root/src/task.rs:329-365`). | No natural home exists for multi-threaded services, per-thread MCS scheduling contexts, or lifecycle control that isn't ambient-name-shaped. |
| 9 | Table ceilings (`MAX_CHANNELS`, `MAX_TRANSIT`, `MAX_TASK_CAPS`, `MAX_SCOPES`, `MAX_GRAPH_ITERATIONS`) are documented as having been raised each time a new compatibility-plane test graph exhausted the previous number (`slime-root/src/channel.rs:94-140`). | Ceilings track "what the largest existing test graph needed" rather than an authenticated per-generation resource plan, so a plausible future graph can still exhaust a table the manifest never proved insufficient. |

Full architectural detail, each finding's evidence, and the per-finding
proposed fix were produced in-session before this hand-off and are condensed
into [Changes](#changes) below; the finding numbering (F1–F6)
is preserved so this document and the earlier review response reference the
same items.

## Root cause

Slime's seL4 port kept the retired custom kernel's *mechanism* — a global
logical capability table, a root-mediated channel queue, a single
multiplexed syscall Endpoint, one-TCB-per-task identity — and re-implemented
it as a userspace service on top of seL4, instead of re-expressing the same
policy (component isolation, bounded IPC, supervised lifecycle) directly in
seL4 primitives (CSpace capabilities, Endpoints, Notifications, MCS
scheduling contexts, Reply objects). B34–B38 fixed the resulting graph and
lifecycle *defects* inside that compatibility layer; they did not remove the
compatibility layer itself. The violated invariant this entry addresses is
architectural, not behavioral: **authority and scheduling should be
kernel-enforced where seL4 already enforces them**, and the generation format
should plan the exact seL4 objects a graph needs rather than naming grants a
userspace root reinterprets.

## Changes

Proposed, sequenced, breaking. No phase retains a v(n-1) compatibility path;
each phase's exit criterion is the removal of the mechanism it replaces, not
merely the addition of the new one.

| Area | Change | Target invariant |
|---|---|---|
| Phase 0 — Generation v5 contract | Add `Process`, `Thread`, `KernelObject`, `Mapping`, `CapBinding`, `ServiceBinding`, `Schedule`, `FaultPolicy`, `SpawnTemplate`, `ResourceQuota` to `contracts/generation/v1/schema.zt` (or a new `v2` directory per this repo's Zutai versioning convention); require every `CapabilityGrant` to materialize a real capability or be marked policy-only; remove name-only grants and `init`'s `option_env!` graph-selection flags. | Declared graph topology and authenticated seL4 object/authority topology are the same data; no plane's boot graph is chosen by build profile. |
| Phase 1 — Native child CSpace | Size each child's CNode from the admitted resource plan; install real service Endpoints, Notifications, Frames, and lifecycle capabilities into child CSpace instead of the fixed four-slot layout in `slime-root/src/task.rs:51-60`; runtime API slots become real CPtrs. | seL4's own capability derivation tree, not a root-side table, is the authority boundary for kernel-object-backed resources (F1). |
| Phase 2 — Direct service IPC, sliced | Cut root's `Operation` dispatcher one vertical slice at a time — console/debug, spawn/lifecycle, block driver, generation manager, filesystem/store — replacing each slice's root-mediated calls with a direct client→service Endpoint `Call`/`ReplyRecv`. Remove the corresponding `Operation` labels from `slime-root/src/ipc.rs:89-141` in the same change as each slice, not after. | Root stops being sole broker for every service class (F3); each service's fault domain is isolated from the others'. |
| Phase 3 — Replace logical channels | Split the one `Channel` primitive into three: direct synchronous RPC (Endpoint `Call`/`ReplyRecv`), rendezvous messaging (`Send`/`Recv`/`NBSend`/`NBRecv`), and buffered async streams (Zutai-defined shared ring + Notification badge bits for credit/availability). Cap transfer moves to real seL4 capability transfer, at most one capability per IPC message; bundle provisioning becomes an explicit transaction, not an atomic four-cap message. Delete `slime-root/src/channel.rs`, `slime-root/src/transit.rs`, `slime-root/src/parked.rs`, and `WaitSet`. | seL4 Endpoint/Notification semantics carry queueing, blocking, and cap-transfer atomicity; `slime-root` no longer re-proves them (F2). |
| Phase 4 — Process/thread/lifecycle split | Separate package/image, service template, process (CSpace+VSpace ownership), thread (TCB+IPC buffer+scheduling context), service instance (exposed Endpoints), and lifecycle handle (wait/kill/derive authority). `spawn` returns a lifecycle capability plus initial service Endpoint capabilities; it stops returning a public `task_id`. Multi-TCB processes become representable. | Lifecycle control is capability-based end to end (F5); services can be multi-threaded; no ambient numeric task identity crosses a process boundary. |
| Phase 5 — Scheduling cutover | Add a `Schedule` record (budget, period, priority) to generation v5 per thread; enable `KernelIsMCS` (currently `OFF` in `sel4/config/qemu-arm-virt.cmake:4`); resource servers become passive servers receiving scheduling-context donation on `Call`; add per-thread `SetTimeoutEndpoint` fault handling. If assurance policy blocks MCS initially, land distinct non-root-priority service priorities first as an interim step — never all-services-at-max-priority. | CPU-time authority is seL4-enforced per generation policy, not a single fixed `CHILD_PRIORITY = 254` for every child (F3). |
| Phase 6 — Resource-plan admission | Compute exact static requirements (TCB/CNode/CSlot/Endpoint/Notification/Frame counts, untyped bytes by size class, mapping count, IRQ bindings, dynamic quota reserve) from generation v5 at build time; admission fails closed before any task activates if the plan is unsatisfiable; dynamic factories consume/release delegated quota capabilities instead of a fixed table watermark. | Table ceilings are proven sufficient by the admitted plan, not raised reactively when a test graph exhausts them (F6). |
| Phase 7 — Deletion | Once phases 0–6 are each independently gated, delete: global `GraphTables`-as-authority-database, the logical `ChannelTable`, `Transit`, `ParkedReplies`, the universal `Operation` dispatcher, public task IDs, the generic cross-kind `u64` rights field, name-only generation grants, and compile-time plane-selection flags. | No dual-model residue remains; a fresh reader of `slime-root/src` sees only seL4-native mechanism. |

## Regression guards

Each phase must land its own gate before the next phase starts; none of these
exist yet, and adding them is part of the phase's exit criterion, not
follow-up work.

| Risk | Guard (to add) | Failure signal |
|---|---|---|
| Generation v5 declares a grant with no materializing capability, or a plane still selects its boot graph by build flag | `just contracts_check` extended with a v5 "every grant materializes" assertion; remove `SLIME_GENERATION_NUMBER`/`SLIME_*_CHECK` from `init.rs` and re-run `just sel4_boot_check` | A generation admits with an unbacked grant, or two builds of the same image disagree on boot graph |
| Child CSpace regresses to the fixed four-slot layout, or a resource-kind capability stops being a real seL4 cap | A new `sel4_capability_layout_check` dumping each child's actual CSpace contents against the admitted plan | Declared authority and installed CSpace capabilities diverge |
| A service slice keeps a root-mediated fallback after its direct-IPC cutover | `grep` gate (or extend `just sel4_gate_control_check`) asserting the migrated `Operation` labels are absent from `slime-root/src/ipc.rs` once a slice lands | A removed label reappears, or a client still reaches the service through root |
| Buffered-stream cutover loses backpressure or peer-death delivery `ChannelTable`/`Transit` used to provide | Reuse and extend `just sel4_stream_check` and `just sel4_crossing_check` against the new shared-ring/Notification implementation | Producer/consumer wedge, dropped credit, or a stranded capability under peer death |
| Lifecycle cutover reintroduces an ambient numeric task identity across a process boundary | A protocol/schema lint rejecting a bare task-id-shaped field in any `contracts/*/v1/schema.zt` wire record | A wire record smuggles a numeric task id where a capability belongs |
| MCS enablement regresses an existing timing-sensitive plane (QoS, timers) | Re-run `just sel4_qos_check` and the platform-timer boot proof under `KernelIsMCS ON` before flipping the config for the full product | A QoS or timer causal chain that passed under non-MCS fails under MCS |
| Resource-plan admission under- or over-estimates a real graph's needs | A stress plane analogous to `just sel4_reclamation_check` that boots a graph at exactly its admitted ceiling and one over it | Admission accepts a plan that later exhausts a table, or rejects a plan that fits |

## Verification

Baseline evidence gathered for this hand-off, establishing that the current
v4 architecture (the starting point for the phases above) is green. None of
these exercise the proposed phases, which are not yet implemented.

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test_sel4_root` | Passed: 130/130 tests across 13 modules. | Direct |
| `just contracts_check` | Passed: BootState model-check (4 scenarios), all Zutai protocol/component/generation bindings current, RP0 contract corpus, 179 boot-contract tests, boot-layout resource check (19 fixtures, 16 seL4 fixtures). | Direct |
| `just generation_check` | Passed: seL4 pin check, native component-graph QEMU build, and two isolated builds producing byte-identical `generation.bin`/`boot-store.bin`. | Direct |
| Source citations in [Investigation log](#investigation-log) | Each finding is grounded in a specific file:line read directly from the current tree during this review; no claim is inferred from memory or from the earlier B34–B38 entries alone. | Direct |
| seL4 IPC/Notification/MCS semantics cited in [Changes](#changes) | Cross-checked against the official seL4 Reference Manual and Tutorials (IPC, Notifications, MCS) at review time. | Direct, external source |

## Decisions

- Decision: propose a clean, phased, no-shim cutover from the logical
  compatibility model to seL4-native mechanism, rather than an incremental
  dual-model period.
  Rationale: this repository's own precedent (B34–B38, and the AGENTS.md
  development rule against ad-hoc wire formats) is that a clean cutover with
  an authenticated single source of truth is preferred over a compatibility
  shim; a dual model would recreate exactly the kind of hidden-ABI ambiguity
  B37 already removed once.
  Rejected alternative: keep the current `Operation`/`ChannelTable` mechanism
  and only harden its resource ceilings. Rejected because it leaves F1–F3
  (userspace-reimplemented authority and IPC) unaddressed and does not use
  seL4's own isolation and scheduling guarantees.
- Decision: sequence the seven phases so that Generation v5 (phase 0) lands
  first and every later phase consumes it, rather than migrating IPC or
  CSpace mechanism ahead of the schema that must describe their exact object
  requirements.
  Rationale: F4 shows the generation format is the shared contract every
  other model depends on; migrating CSpace or IPC first would repeat the
  "grants name, minted endpoints authorize" split documented in
  `contracts/generation/v1/fixtures/sel4-call.md:30-56`.
  Rejected alternative: migrate IPC first because it is the most visible
  defect (F2). Rejected because a direct-Endpoint cutover still needs an
  admitted per-object resource plan to size CSpace and Notification objects
  correctly.
- Decision: gate MCS enablement (phase 5) behind an explicit interim
  non-uniform-priority step rather than requiring it from phase 3 onward.
  Rationale: `KernelIsMCS` is currently `OFF`
  (`sel4/config/qemu-arm-virt.cmake:4`) and MCS is documented upstream as
  "currently undergoing verification"; decoupling the scheduling cutover from
  the IPC/CSpace cutover lets phases 1–4 land and gate independently of an
  assurance-policy decision on MCS.
  Rejected alternative: require MCS as a prerequisite for direct-Endpoint IPC.
  Rejected because seL4's synchronous IPC and cap-transfer primitives do not
  require MCS; only scheduling-context-donation-based passive servers do.

## Open risks and follow-ups

- [ ] Phase 0 needs a decision on whether Generation v5 is a new schema
      version under `contracts/generation/v1/` (bumping `formatVersion`) or a
      new `contracts/generation/v2/` directory, following this repo's
      existing Zutai versioning convention; not decided in this entry.
- [ ] Phase 3's buffered-stream design (shared ring + Notification badge
      bits) needs a concrete Zutai schema before implementation; this entry
      names the primitive, not the wire layout.
- [ ] Phase 5's MCS enablement is gated on an assurance-policy decision this
      entry does not make; `KernelVerificationBuild` is also `OFF`
      (`sel4/config/qemu-arm-virt.cmake:6`) and interacts with that same
      policy question.
- [ ] No phase in this plan has been implemented yet; every "Regression
      guards" row is a guard to add, not one that exists today. The next
      engineer's first concrete action is phase 0's schema change plus its
      `contracts_check` extension.
- [ ] Physical Framework laptop bring-up and internal-NVMe safety remain
      explicitly out of scope for this program, per AGENTS.md, and are not
      affected by any phase here.
- [ ] This entry does not re-litigate B34–B38's already-closed findings;
      see [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../2026-08-09-b34-b38-sel4-model-audit/index.md)
      and [`devlog/2026-08-10-b34-b38-model-cutover/`](../2026-08-10-b34-b38-model-cutover/index.md)
      for that closed work.

## Artifacts and provenance

- Focused report: this entry is the focused hand-off record; no separate
  report file exists.
- Raw transcript: none retained; every cited file:line was read directly from
  the current tree during this review and is independently re-derivable.
- Serial/debugger/model output: none — this is a static architecture review,
  not a runtime observation. The three gate runs in
  [Verification](#verification) are the only executed evidence and establish
  the pre-migration baseline, not any proposed phase.
- Related roadmap item: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md)
  B34–B38, [`roadmap/07-architecture-portability.md#p5-sel4-microkernel-substitution`](../../roadmap/07-architecture-portability.md)
  (P5, P5.4, P5.5), and
  [`roadmap/02-core-runtime.md#c8-native-typed-data-fabric`](../../roadmap/02-core-runtime.md)
  (C8).
