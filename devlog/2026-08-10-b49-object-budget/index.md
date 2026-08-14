# B49: the stress graph found the ceiling admission was not checking

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root/src/{generation,object_allocator,main}.rs`, `scripts/build/build-generation.py`, `scripts/build/build-sel4.py`, `scripts/check/check-sel4-stress-plane.py`, `contracts/generation/v1/fixtures/sel4-stress.zti`, `components/bins/src/bin/init.rs`, `boot-contracts/src/generation.rs`, `Justfile` |
| Roadmap | B49 |
| Gates | `just sel4_stress_check`, `just contracts_check`, `just generation_check`, `just sel4_reclamation_check`, `just sel4_boot_check`, `just test_sel4_root` |
| Trigger | B49's earlier half derived per-instance quotas and refused three classes; its exit condition also required a graph at the admitted ceiling to boot, and none existed. |
| Baseline | Admission checked CSlots, TCBs, and CNodes per instance. No check compared the plan's total against the root's own CSpace. |

## Summary

Building the stress graph the exit condition asks for immediately produced the
defect it was meant to guard against: a 48-instance generation **was admitted**
and then died at instance 39 with `SlotsExhausted`, 38 children already
running. Admission was checking each process against a per-class ceiling and
never checking that they all fit together — and the per-process quota
understated its own cost by an order of magnitude, declaring 6 objects for a
process whose construction consumed 81 root CSlots.

Both are fixed. The quota now counts the image's own frames, and admission
compares the plan's total against the allocator's real free-slot count before
any component starts. The 23-instance plane — the largest that fits — boots,
constructs every instance, and reclaims to zero; one instance more is refused
by name before activation.

## Observable symptom

- Command: `qemu-system-aarch64 ... -kernel build/slime-sel4-stress.elf` at 48 instances
- Observed:
  ```
  SLIME_ROOT generation admitted number=1 executables=2 instances=48 grants=0 health=32 bootstrap=1
  SLIME_ROOT graph admitted executables=2 instances=48 slimecm=0 elf=2 unrecognized=0
  SLIME_ROOT FATAL SLIME_GRAPH FAIL instance stress-39 construction failed: VSpace(Alloc(SlotsExhausted { allocated: 3186 }))
  ```
- Expected: refusal before any component activates, naming the budget.

## Investigation log

1. Built a 48-instance fixture at `MAX_ADMITTED_INSTANCES`. It admitted and
   then failed mid-construction, which is the exact failure B49 names.
2. Added the aggregate check. It reported `required=288 available=3177` — the
   plan claimed 6 slots per instance while construction used 81.
3. Traced the gap: the quota deliberately omitted the image's frames and page
   tables as "the root's accounting". They are the root's, and the root's
   CSlots are exactly what runs out, so omitting them made the number fiction.
4. Counted image pages in the builder. First attempt read a segment table that
   is not there: the seL4 profile carries the whole ELF after the qualification
   header rather than a re-based segment list, so the pages come from that
   ELF's own program headers. `required` went 288 → 2153.
5. The residual 1033 is intermediate page tables, window aliases, and arena
   parent untypeds — root-side costs belonging to no child's plan.

## Root cause

Two independent undercounts, either of which alone would have been survivable.

**Admission had no aggregate.** Every check was per-instance: this process's
CSlots against `MAX_TASK_CAPS`, its TCBs against one. Nothing summed them. A
graph of N processes that each individually fit was admitted regardless of N,
so the only thing standing between a large graph and mid-construction death was
that no graph had been large enough to reach it.

**The per-process quota omitted its largest term.** The builder counted the
objects its own loop declared — a CNode, a VSpace, a TCB, an IPC-buffer frame,
two endpoints — and explicitly excluded the image's frames and page tables on
the grounds that the root maps those from its own untyped, making them "the
root's accounting rather than the child's plan". That reasoning is exactly
backwards: root CSlots are the resource that runs out, so an object the root
allocates is precisely the object that must appear in the budget. The
distinction the comment drew was ownership; the distinction that mattered was
which pool pays.

## Changes

- `admit_total_slots` sums every quota's objects and refuses
  `PlanExceedsRootSlots` against `ObjectAllocator::free_slots()`, called from
  `main` before any task is constructed.
- The builder derives each process's `frame` count from the pages its
  executable's ELF actually loads, plus one IPC-buffer/window pair per thread.
  `vspace` is counted rather than left zero.
- `admit_resource_quota` was extracted from a loop so it is reachable from a
  test, and now covers six classes rather than three.
- New `sel4-stress` plane, boot action, and `just sel4_stress_check`.

## Regression guards

- `sel4_stress_check` boots the 23-instance graph, requires all 23 staged, the
  plan's total reported and fitting, and reclamation to `live=0`. It also
  refuses a plane that uses less than half the budget — a "ceiling" graph that
  used a tenth of it would prove nothing.
- `test_sel4_root` gained three quota tests (149 total): the builder's plan is
  admitted, a two-thread process may declare two TCBs and two frames, and one
  object over any of six ceilings is refused naming its class.

## Verification

| Control | Result |
|---|---|
| One instance over (24) | `PlanExceedsRootSlots { required: 3219, available: 3180 }`, refused before activation |
| Revert the TCB ceiling to 1 | two-thread quota test fails |
| 48 instances, before the fix | admitted, then `SlotsExhausted` at instance 39 |

Green run: `budget: the graph plans 3084 root CSlots of 3180 free` /
`construction: all 23 declared instances were staged` / reclaimed every one.

Full sweep: 32 plane gates, `contracts_check`, `generation_check`,
`sel4_boot_layout_check`, `sel4_gate_control_check`, `test_sel4_root` 149/149,
`test_host` 7 suites, `lint_all`, `fmt_check_all`, `ruff`, `typos`.

## Decisions

**A measured constant, not a model.** `ROOT_SLOTS_PER_DECLARED_OBJECT = 3`
covers the root-side costs no child quota names. Modelling each source exactly
would claim a precision the accounting does not have, and being wrong low means
admitting a graph that dies mid-construction. Refusing a graph that would have
fit is recoverable; the other direction is not.

**Count the image's frames in the child's quota.** They are allocated from root
CSlots, so calling them "the root's accounting" and omitting them made the
declared number describe nothing. What matters is not whose objects they are
but whose budget they spend.

**The plane is sized to what fits, not to `MAX_ADMITTED_INSTANCES`.** 48 is the
format's ceiling; 23 is this root's. A plane pinned to the format's number
would fail for a reason unrelated to what it tests.

## Open risks and follow-ups

- IRQs and untyped size classes are still unmodelled, so the exit condition's
  "one IRQ or untyped size class over" clause is not covered. No plane declares
  either yet.
- `MAX_TASKS`, `MAX_CHANNELS`, and `MAX_TRANSIT` remain watermarks rather than
  plan-derived. The graph now refuses to exceed the root's CSpace, which is the
  binding constraint; these tables bound structures the CSpace check does not
  reach.
- `MAX_HEALTH_INSTANCES` is 32 while `MAX_INSTANCES` is 48, so a 48-instance
  graph cannot mark every instance required. Found while building the fixture;
  left as-is because 23 is under both.
- 3084 of 3180 slots leaves 96 free, so the plane is genuinely near the
  ceiling. If root startup cost changes, this plane is the first thing that
  will notice — which is the point.

## Artifacts and provenance

- Every figure above was observed in a QEMU transcript in this session; none is
  inherited.
- The `81 slots per instance` figure is `3186 / 39` from the failing run, and
  `33 declared objects` is the quota's own sum for that fixture.
