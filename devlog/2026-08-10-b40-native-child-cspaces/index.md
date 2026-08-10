# B40 — child CSpaces sized and populated from the admitted plan

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{task,main,graph}.rs`, `slime-root/build.rs`, `boot-contracts/src/generation.rs`, `scripts/build/build-sel4.py`, `scripts/check/{check-generation,check-sel4-capability-layout,check-sel4-boot-plane}.py`, `Justfile` |
| Roadmap | B40 |
| Gates | `just sel4_capability_layout_check`, `just sel4_boot_check`, `just test_sel4_root`, `just contracts_check`, `just generation_check` |
| Trigger | B40, unblocked by B39's admitted v5 plan landing in `4ab5992`'s parent. |
| Baseline | Every child CNode was `CHILD_CNODE_SIZE_BITS = 2` — four slots — with `CHILD_SLOT_SERVICE`/`CHILD_SLOT_TCB`/`CHILD_SLOT_FAULT` compiled in, while actual authority lived in the root-side `CapabilityTable`. |

## Summary

The generation's v5 plan declared each process's CSpace — a CNode object with a
size, cap bindings naming the child's own TCB and fault endpoint, and a service
binding naming its route to the root — and the root ignored all of it, building
the same four-slot shell for every child. The kernel therefore could not enforce
the declared layout: the plan was documentation. The root now sizes each CNode
from the plan's CNode object and installs at the declared slots, and three
audits verify the result against the kernel before the child is resumed. A new
`just sel4_capability_layout_check` proves those audits catch deviation by
injecting six mutations and requiring each to be refused.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `boot-contracts/src/generation.rs` | `instance_cspace_size_bits` and `instance_child_slots` resolve a declared instance's CNode size and its service/TCB/fault slots; `ChildSlotPlan` carries the latter | The plan's CSpace declaration is readable by the component that must honour it |
| `slime-root/src/task.rs` | `create` takes `cnode_size_bits` and `ChildSlots`; the CNode, the arena reservation, `mint_child_slot`'s resolution depth, and `tcb_configure`'s guard all derive from the same size | A child's CSpace is as large as its declared authority needs, and every install resolves against the CNode actually built |
| `slime-root/src/task.rs` | `audit_child_cspace`, `audit_child_types`, `InstallLedger` | Occupancy, type, identity, and rights of the constructed CSpace are checked against the plan by the kernel, not assumed |
| `slime-root/src/task.rs` | `ChildSlots::validate` | A plan cannot name a layout the child could not use |
| `slime-root/src/main.rs` | Boot-graph and spawn callsites read the plan; the fixture path passes `ChildSlots::SHELL` explicitly | Both product paths use one layout source; the fixture path's use of the shell is declared rather than implied |
| `scripts/check/check-generation.py` | Cap-binding slots must be addressable in the process's own CNode; each process declares exactly one own-TCB and one own-fault binding, at distinct slots; the root-dispatch service binding is pinned to `ROOT_SERVICE_SLOT` | The host twin refuses the plans the root refuses |
| `scripts/check/check-sel4-capability-layout.py`, `slime-root/build.rs`, `scripts/build/build-sel4.py` | New gate plus six `slime_b40_mutate_*` injection points | The audits' refusals are observed, not assumed |

### What the audits can and cannot ask

seL4 offers no "read this slot", so each property needed its own probe, and one
of them could not be answered at the slot at all:

- **Occupancy** — `Move` onto the slot itself. `ensureEmptySlot(destSlot)` runs
  at `deps/seL4/src/object/cnode.c:93`, *before* the source lookup, so an
  occupied slot answers `DeleteFirst` and an empty one falls through to
  `FailedLookup`. Neither path mutates anything, which is what makes a
  self-move usable as a read-only question.
- **Type** — copy the slot into a root scratch slot and invoke `tcb_suspend`,
  which `decodeInvocation` refuses with `InvalidCapability` for every non-TCB.
  The thread has not been resumed, so suspending it is a no-op.
- **Rights** — *not* answerable at the slot. `maskCapRights` silently masks a
  copy's rights down and never reports what a capability carries, so a probe
  cannot distinguish a slot installed with excess rights from one without.
  Rights are therefore checked at `InstallLedger::record`, the single
  chokepoint every child install passes through, which makes the check
  complete by construction rather than by convention.
- **Identity** — likewise not observable; the ledger records each install's
  source path and badge. Two installs of the same path under the same badge are
  indistinguishable to the child, which is what makes them an alias. The
  service and fault slots share one endpoint object under two badges, so the
  identity is the pair, not the object.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The audit stops catching a class of deviation | `just sel4_capability_layout_check` | `the audit accepted a mutated CSpace: <class>` |
| A plan moves the service endpoint away from the runtime's constant | `just generation_check`, `ChildSlots::validate` | `BadServiceBinding`; `declares an unusable child layout` |
| A plan names a slot outside the process's own CNode | `just contracts_check` | `BadCapBinding` |
| Child slots collide or land on the null slot | `just test_sel4_root` | `colliding_child_slots_are_refused`, `a_null_child_slot_is_refused` |
| The install ledger's alias rule regresses | `just test_sel4_root` | `ledger_refuses_an_identical_source_and_badge` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_capability_layout_check` | Pass; unmutated graph accepted, all six mutations refused | Direct |
| `just sel4_boot_check` | Pass; twenty-instance graph reaches the supervisor terminal | Direct |
| `just test_sel4_root` | 140/140 | Direct |
| `just contracts_check`, `just generation_check` | Pass | Direct |
| `just sel4_root_boot_check`, `just sel4_component_graph_check`, `just sel4_reclamation_check` | Pass | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos` | Pass | Direct |
| `just sel4_spawn_check` | Fails identically at `b1209a0` (before this work) and after — an unmigrated `sel4-spawn.zti` | Direct, both sides observed |

Each mutation's refusal was observed individually rather than inferred from the
gate's exit status; the `missing` case reports
`CSpaceMismatch { slot: 3, occupied: false }`.

## Decisions

- **Decision:** rights and identity are verified at the install chokepoint, not
  by probing the constructed slot.
  **Rationale:** seL4 masks rights silently and exposes no object identity, so
  a slot probe cannot answer either question. An audit that appeared to check
  them at the slot would be theatre.
  **Rejected alternative:** an `audit_child_rights` that re-derived the rights
  it was about to install and compared them to themselves. It was written,
  found vacuous, and deleted.

- **Decision:** the service endpoint's slot is read from the plan but pinned to
  `ROOT_SERVICE_SLOT` by both the root and the host checker.
  **Rationale:** making the slot plan-driven newly created the possibility of
  drift — every component resolves the root endpoint from a compiled-in
  constant, so a plan naming another slot would build clean, admit clean, pass
  an audit that validates against that same plan, and produce children whose
  first syscall invokes an empty slot. The pin holds until the runtime reads
  the slot from the boot layout too.
  **Rejected alternative:** trusting the plan, on the grounds that the audit
  checks the CSpace against it. The audit cannot catch this: plan and CSpace
  agree, and the component is the one that disagrees.

- **Decision:** the wrong-type mutation replaces the TCB with a CNode rather
  than deleting and leaving the slot empty.
  **Rationale:** occupancy is unchanged, so it tests the type probe rather than
  re-testing the occupancy audit. A mutation the earlier audit already catches
  proves nothing about the new one.

## Open risks and follow-ups

- [ ] `wrong_slot` is a sixth mutation class B40 does not name. It is only
      meaningful because destinations are now plan-declared, and it is kept.
- [ ] The fixture paths still construct a four-slot shell outside any plan.
      They are the P5.1 `Role::CleanExit`/`Role::DeliberateFault` fixtures, not
      declared instances; `ChildSlots::SHELL` marks this explicitly rather than
      inheriting it.
- [ ] `sel4-spawn.zti`, `sel4-channel.zti`, `sel4-sample.zti`,
      `sel4-supervision.zti`, `sel4-crossing.zti`, and `sel4-qos.zti` remain
      unmigrated from B39; their gates fail with signatures unchanged by this
      work.

## Artifacts and provenance

- Independent reviews: two subagents (`reviewer`, `scout`). The reviewer's P1 —
  service-slot drift against the runtime's `ROOT_SERVICE_SLOT` — is fixed
  above, as is its P2 on ledger identity being a bare CPtr rather than a full
  path. The scout's findings on the missing wrong-type class, the
  self-referential wrong-rights mutation, the `LAST_TRANSCRIPT` stale-read
  hazard, and the unguarded product image are all fixed.
- Kernel source consulted for probe soundness: `deps/seL4/src/object/cnode.c`
  (`ensureEmptySlot`, `decodeCNodeInvocation`), `deps/seL4/src/object/objecttype.c`
  (`deriveCap`).
- Related roadmap item: `roadmap/00-backlog.md` B40.
