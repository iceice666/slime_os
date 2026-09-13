# MEM-64M: a 64 MiB private working set, reclaimed and re-served twenty times

| Field | Value |
|---|---|
| Date | 2026-09-13 |
| Kind | Change |
| Status | Verified |
| Scope | `components/testkit/private-cycle-probe`, `components/system/init/src/{main,dispatch}.rs`, `boot-contracts/src/generation.rs`, `contracts/{component-spec,system-spec,generation-manifest,composition-inventory}`, `scripts/check/check-sel4-private-memory-plane.py`, `scripts/check/check-sel4-gate-controls.py`, `just/planes-runtime.just` |
| Work items | 01a07a2d-0060-7d52-8e8d-1c7f45246f8d |
| Gates | `just private_memory_cycles_check`, `just private_memory_check`, `just sel4_gate_control_check`, `just contracts_check`, `just system_spec_check`, `just system_image_closure_aggregate_check`, `just test_host`, `just test_sel4_root` |
| Trigger | MEM-64M was reopened by review: its recorded capacity results established the ceiling but left the reuse clause unobserved |
| Baseline | `just private_memory_check` passing on both QEMU architectures at the declared 16384-page ceiling (24 markers, 7 causal chains), from [the target-qualified capacity entry](../2026-09-13-mem-64m-target-qualified-capacity/index.md) |

## Summary

MEM-64M's last open exit condition required twenty spawn/grow/exit-or-fault
cycles at the declared ceiling with no downward drift in reusable memory, slots,
or accounting, and no stale data exposure. Nothing observed it: the existing
plane grows one holder to 64 MiB once and exits, which a root that leaked
everything a dead holder owned would still pass. A new composition,
`sel4-private-memory-cycles` (generation 55), now has `init` relaunch one
declared 16384-page holder twenty times — ten ending by clean exit, ten by a
deliberate fault past the window — with each incarnation reading every one of its
16384 pages as zero before stamping it with a value no other incarnation writes.
The root's allocator watermarks are identical after all twenty reclamations
(`slots=522045 bytes=1515227504 live_objects=145`), `extent_reuses` grows by 65
per cycle to 1235, every growth takes the same 32 large frames, and the plane
ends with `mapped_ram=0`, the full 64 MiB retained as `reusable_private_ram`, and
every allocation descriptor free.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `components/testkit/private-cycle-probe` | One declared holder's whole life: grow to the declared ceiling, read every page as zero, stamp it, read every stamp back, then exit or fault by cycle parity. Three separate passes, not one fused loop | A re-served live page fails the zero pass; an aliased page fails the read-back; fusing the write into the first pass would let an aliased page satisfy its own check |
| `boot-contracts/src/generation.rs` | `BootAction::PrivateMemoryCycles = 39`, in `ALL`, `from_id`, `parse`, and the frozen numbering table | A composition the root can name is one every reader folds back to the same variant (B70) |
| `components/system/init/src/main.rs` | `drive_private_memory_cycles_plane`: twenty spawns, each handed its cycle number over the composition's declared endpoint, each awaited and asserted to end by the path its parity names | An incarnation cannot count its predecessors, and a constant stamp could not distinguish a zeroed page from a re-served written one |
| `contracts/system-spec/v1/systems/sel4-private-memory-cycles.zti` and its derived manifest, baseline, closure, and test-run record | A second composition rather than a fourth instance on the ceiling plane | The private-memory budget's aggregate rule sums *declared* quotas, and the ceiling plane already declares 32280 of the target's 32768 |
| `check-sel4-private-memory-plane.py` | `--arm cycles`: `CYCLE_CHAINS` for the per-cycle causal contract, `check_reuse_cycles` for the aggregate, `check_markers` parameterized by arm, `declared_quotas` by fixture | One checker owns the private-memory invariant; a second top-level checker would be a second copy of its marker discipline |
| `check-sel4-gate-controls.py` | The plane's pinned marker count 24 → 35 | A gate that lost a marker lost coverage |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A dead holder's CSlots, untyped bytes, or kernel objects are not returned | `just private_memory_cycles_check` | `cycle N (task M) left … against … after the first: reclaimed CSlots are not returned`, or the same for untyped bytes or live objects |
| A later incarnation is served a page an earlier one wrote | Same gate | `[private-cycle-probe] FAIL cycle=N a served page was not zero detail=…` |
| Reclaimed backing degrades from 2 MiB frames to base pages | Same gate | `growths took N distinct backing shapes`, or `a reused growth was backed by large_frames=…` |
| Released extent records stop being retained for reuse | Same gate | `extent reuses did not grow across 20 cycles`, or `extent reuse count decreased across cycles` |
| Only one termination path is exercised | Same gate | `cycle N ended by exit, expected fault`, or `[init] private memory cycles exits=10 faults=10` missing |
| A holder dies before committing its quota | Same gate | `N holder(s) reported a committed quota before faulting, against M fault(s)` |
| The ceiling plane's own contract changes | `just private_memory_check` | Its 24 markers across 8 chains, on both architectures |
| A marker is deleted or reordered without notice | `just sel4_gate_control_check` | The pinned count 35 |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just private_memory_cycles_check` | Passed: `11 markers across 4 causal chains and 1 image case on qemu-arm-virt; the declared quota (16384 page(s)) was reclaimed and re-served 20 times over 10 clean exits and 10 deliberate faults, every served page zero, with no drift in reusable slots, untyped bytes, or live objects` | Direct |
| Per-cycle drift, from [`private-memory-cycles-plane.log`](private-memory-cycles-plane.log) | All twenty holder censuses identical: `slots=522045 bytes=1515227504 live_objects=145`; `extent_reuses` 65 → 1235, +65 per cycle | Direct |
| Per-cycle backing shape | Twenty of `SLIME_MEM grown task=N delta=16384 previous=0 pages=16384 base=0x4000000 quota=16384 total=16384 large_frames=32 base_frames=0 leaf_tables=0` — one base for every incarnation, `previous=0` every time | Direct |
| Terminal accounting | `SLIME_GRAPH spawns served=20 drops=0 terminated=21`; `mapped_ram=0 reusable_ram=68026368 reusable_private_ram=67239936 allocation_descriptors_free=288416 extent_descriptors_free=1006 slot_reuses=313748 extent_reuses=1235`; `SLIME_GRAPH HEALTHY generation=55 required=1 live=0 completed=1 failed=0` | Direct |
| First cycles run | Refused: `21 reclamation census record(s), expected 20` — init's own exit is reclaimed too, so the twenty-first census is a task that never held a quota | Direct |
| `just private_memory_check` | Passed on both architectures, unchanged at `24 markers across 8 causal chains`: 3 image cases on `qemu-arm-virt`, 1 on `qemu-riscv-virt` | Direct |
| `just sel4_gate_control_check` | Passed: `48 gates reject 1963 mutated transcripts and layouts` (1937 before), so every new marker is mutation-covered | Direct |
| `just contracts_check`, `just component_spec_check`, `just component_crate_split_check`, `just system_spec_check` | Passed: 42 seL4 manifests, 84 component records with 43 mutations refused, 76 component crates each carrying a release-profile stanza, 45 systems with 21 mutations refused | Direct |
| `just system_image_closure_check`, `just system_test_run_check`, `just system_image_closure_aggregate_check` | Passed: 51 test-run records, all 54 closures exercised by an owning gate | Direct |
| `just test_host`, `just test_sel4_root` | Passed: 342 boot-contracts tests including the extended frozen boot-action table; 240 of 240 root tests | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | Passed | Direct |

## Decisions

- Decision: a separate composition rather than a fourth instance on `sel4-private-memory`.
- Rationale: forced, not chosen. `PrivateMemoryBudget::validate_against` sums
  *declared* quotas and refuses the generation before anything launches; the
  ceiling plane declares 32280 of the target's 32768-page aggregate, leaving 488.
  A 16384-page holder beside them cannot be admitted.
- Rejected alternative: raising the aggregate ceiling — that envelope is MEM-1G's
  to publish with its own evidence.

- Decision: ten exits and ten faults, alternating, rather than twenty of one path.
- Rationale: the clause reads "normal exit **and** deliberate task fault permit
  immediate reuse", and the root reaches `reclaim_dead_task` from its fault arm
  and its exit arm independently. Alternating also orders both transitions ten
  times each, so a defect that appears only when a faulted task's backing is
  re-served to an exiting one is reachable.
- Rejected alternative: twenty clean exits, which would leave half the clause
  unobserved.

- Decision: the fault chain pins `kind=VirtualMemory` and leaves the access field open.
- Rationale: the probe's store demonstrably traps — its "did not trap" report
  never appears — but this configuration reports `access: Execute status: 0`, and
  the pre-existing `reclamation-fault` probe produces exactly the same shape for
  its own deliberate write (`SLIME_GRAPH component fault task=81 kind=VirtualMemory
  { access: Execute, status: 0 } address=Some(67108864)`). Asserting `Write` would
  pin behaviour the platform does not produce.
- Rejected alternative: asserting `access: Write`, which failed against a real
  transcript; and ending the pattern at `kind=`, which accepted the first
  revision's *accidental* prefetch fault and so observed no deliberate fault at all.

- Decision: the cycles arm is closure-backed and AArch64 only.
- Rationale: the reuse clause names no architecture, unlike the milestone's
  grow-to-ceiling condition, which `private_memory_check` already observes on
  both. The RV64 private-memory image is still built through a legacy
  `build-sel4.py` plane flag that CP15 is retiring, and extending that surface to
  reach a second architecture the clause does not ask for would add work to the
  deletion.

## Open risks and follow-ups

- [ ] The root cannot distinguish a data abort from a prefetch fault for a
  component's unmapped write on `qemu-arm-virt`: both `private-cycle-probe` and
  `reclamation-fault` produce `access: Execute status: 0` with `status` zero,
  which `normalize_vm_access` reads as neither of its data-abort exception
  classes. Owner: `slime-root/src/fault.rs`. Until then no gate can assert the
  access class of a deliberate write.
- [ ] Twenty cycles is the milestone's number, not a measured sufficiency bound.
  Drift below one slot per cycle is not detectable at this length.
- [ ] The arm runs one holder at a time. Concurrent 64 MiB holders across
  repeated lives belong to MEM-1G's aggregate qualification.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`private-memory-cycles-plane.log`](private-memory-cycles-plane.log),
  the passing run's full serial capture, 3182 lines through
  `SLIME_GRAPH HEALTHY generation=55`.
- Related work item: MEM-64M, `.tasks/items/01a07a2d-0060-7d52-8e8d-1c7f45246f8d.md`.
