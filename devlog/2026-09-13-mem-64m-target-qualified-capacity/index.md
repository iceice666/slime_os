# MEM-64M: private capacity becomes target-qualified, the runtime path segments, and 1 GiB is refused for a named reason

| Field | Value |
|---|---|
| Date | 2026-09-13 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/private-memory-budget/v1`, `boot-contracts/src/private_memory_budget.rs`, `slime-root/src/{private_memory,object_allocator,child_vspace,generation,graph_runtime,main}.rs`, `components/testkit/private-{memory,heap}-probe`, `scripts/build/{generation_resources,build-generation}.py`, `scripts/lib/{component_spec,system_spec}.py`, `scripts/check/{check-sel4-private-memory-plane,check-sel4-gate-controls}.py`, `contracts/system-spec/v1/systems/sel4-private-memory.zti` |
| Work items | 01a07a2d-0060-7d52-8e8d-1c7f45246f8d |
| Gates | `just private_memory_check`, `just contracts_check`, `just component_spec_check`, `just system_spec_check`, `just generation_check`, `just test_sel4_root`, `just test_host`, `just sel4_reclamation_check`, `just lifecycle_restart_check`, `just sel4_gate_control_check` |
| Trigger | MEM-64M's first user-visible capacity target: a real 64 MiB private working set on both QEMU architectures |
| Baseline | `regionPages = 512` (2 MiB) and `totalPages = 2048` (8 MiB) as global scalars; the runtime backing path allocated one data extent sized to the whole quota; no target identity reached quota validation |

## Summary

A private-memory ceiling was a single pair of global constants, so raising it would
have raised it everywhere — including on boards whose 12-bit root CNode cannot hold
the descriptor tables a 64 MiB holder needs. Capacity is now declared per
target-profile name in Zutai, with the previous 512/2048 pair retained as the
conservative default that every physical target still resolves to. The QEMU profiles
declare 16384 pages (64 MiB) per holder and 32768 (128 MiB) across every live region,
and both architectures reach that ceiling under `just private_memory_check`. The
runtime backing path now segments into 2 MiB extents exactly as the planner already
did, so MEM-ARENAS' bounded reclaimable-extent invariant survives the raise. Two
defects were found and fixed by running the plane rather than by reading it: base-page
growth mapped one leaf table for the whole window and faulted crossing the first 2 MiB
span, and the descriptor tables were sized for exactly four maximal holders and zero
other tasks, which refused the fourth holder's spawn with
`DestinationSlotsExhausted`.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/private-memory-budget/v1` | `CapacityProfile` rows keyed by target-profile name; the scalars become `DEFAULT_REGION_PAGES`/`DEFAULT_TOTAL_PAGES`; `capacity_for` is the hand-written lookup beside the generated table | A ceiling is a promise the root must honour in full, so it is declared for the target that has run the workload and defaulted conservatively everywhere else |
| `slime-root/src/generation.rs` | `private_memory_budget_is_satisfiable` resolves the pair from the admitted generation's `target` | The builder and the root select the same envelope from the same identity, so neither can admit a budget the other refuses |
| `slime-root/src/object_allocator.rs` | `PrivateBackingLayout` segments into 2 MiB extents at every quota; `MAX_TASK_ALLOCATIONS`/`MAX_TASK_EXTENTS` derive from the larger of the runtime aggregate and the planner's four-holder report, plus one static envelope per live task | A table sized to exactly N maximal holders leaves nothing for the `init` and coordinator instances such a graph also runs |
| `slime-root/src/private_memory.rs` | One leaf table per 2 MiB span, recognized on retry; the injected large-map failure is scoped to the first large frame of an aligned full-window growth | A one-shot injection lands on the case under test rather than on whichever holder the scheduler ran first |
| `slime-root/src/graph_runtime.rs` | The capacity qualification reports the *admitted* envelope — the target's region ceiling and the holder count its aggregate admits at that size | `fit` answers a question about platform resources rather than about an unqualified claim |
| `components/testkit/private-{memory,heap}-probe` | Probe bounds derive from the generated capacity table; the heap arm reports payload, allocator overhead, and backed bytes separately | A fixed 512-page guard cannot bound a target-qualified region, and a component has no quota query so the ceiling is the root's to report |
| `contracts/system-spec/v1/systems/sel4-private-memory.zti` | `private-memory-granted` at 16384 pages; `private-heap-granted` at 15872 | With `quota == regionPages` the two coincide and `Region::admit` reports every over-ceiling request as `ReservationExceeded`, leaving `QuotaExceeded` unreachable; the two holders now reach both arms |
| `scripts/check/check-sel4-private-memory-plane.py` | Chain literals follow the declared quota; `check_heap_capacity_is_within_the_declared_quota` joins the component's observations to the root's declared ceiling; self-check windows are scoped so the capacity phase is not charged to them | A holder reporting 60 MiB of payload proves nothing about a bound unless the bound is the one the generation declared |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A board silently inherits the QEMU envelope | `just contracts_check` (`capacity_for` tests) | `capacity_for("aarch64-rpi5")` must remain `(512, 2048)` |
| The runtime path stops segmenting and reserves one extent for the whole quota | `just test_sel4_root`, `just sel4_reclamation_check` | Plan/runtime disagreement in the sizing test; `segmented task extents ... reclaimed and reused` regresses |
| The descriptor tables shrink below either demand | `just test_sel4_root` | `the four-holder capacity report would answer fit=0 for want of descriptor table rather than platform resources` |
| Base-page growth stops installing a leaf table per span | `just private_memory_check` | `cause=frames detail=Frames { allocated: 490, error: ... FailedLookup }` at the first 2 MiB boundary |
| The declared ceiling stops binding | `just private_memory_check` | `holds N page(s) against a declared quota of M, so the ceiling did not bind` |
| The 60 MiB case stops exercising the raised ceiling | `just private_memory_check` | `60 MiB of payload occupies N of M declared page(s), so the case does not exercise the raised ceiling` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just private_memory_check` — `qemu-arm-virt` | Passed: `24 markers across 7 causal chains and 3 image case(s)`, `the declared quota (16384 page(s)) is the measured ceiling`. Bulk growth `delta=16384 ... large_frames=32 base_frames=0`; accounting `reservation=67108864 reserved=67239936 payload=67108864 tables=131072 alignment=0 frames_large=32 frames_base=16384 descriptors=16448 extents=65 cslots=16513`; heap `capacity payload=62914560 overhead=16 backed=63008768 pages=15383 touched=1`. Kept as [`private-memory-plane-arm.log`](private-memory-plane-arm.log) | Direct |
| `just private_memory_check` — `qemu-riscv-virt` | Passed: `24 markers across 7 causal chains and 1 image case(s) ... (16384 page(s))` | Direct |
| Large-map injection arm | Passed. The failed span demotes and the rest do not: `delta=1 ... base_frames=1 leaf_tables=1` then `delta=16383 previous=1 pages=16384 large_frames=31 base_frames=512`, probe `retries=1`. Kept as [`large-map-retry-arm.log`](large-map-retry-arm.log) | Direct |
| Incremental-rollback arm | Passed after pinning the fixture's retry to one large-frame span rather than the target's region ceiling | Direct |
| `just contracts_check` | Passed: 317 boot-contract tests | Direct |
| `just test_sel4_root` | Passed: 236/236, up from 235 | Direct |
| `just test_host`, `just component_spec_check`, `just system_spec_check`, `just generation_check` | Passed; 83 component records with 43 mutations refused, 45 systems with 21 mutations refused | Direct |
| `just sel4_gate_control_check` | Passed: 48 gates, 1937 mutated transcripts; the private-memory pin moved 23 → 24 | Direct |
| `just sel4_reclamation_check`, `just lifecycle_restart_check`, `just sel4_root_boot_check`, `just sel4_boot_layout_check` | Passed | Direct |
| `just fmt_check_all`, `just lint_all`, `just deny`, `just machete`, `just typos`, `just ruff` | Passed | Direct |

## Decisions

- Decision: the 64 MiB envelope is 16384 pages per holder and 32768 aggregate; the four-holder 256 MiB envelope is not published.
- Rationale: a published ceiling is a promise the root must honour in full. The planner represents 65536 pages per holder and the tables are sized for it, but no workload of that size has been observed, and the one attempt refused its fourth holder's spawn. The raise belongs to MEM-1G, with its evidence.
- Decision: `private-heap-granted` sits at 15872 pages, below the region reservation.
- Rationale: `Region::admit` tests the reservation before the quota, so a holder whose declared quota equals the reservation can only ever report `ReservationExceeded` — the declared-budget refusal would be unreachable and untested. Split across two holders, both arms are exercised.
- Decision: the capacity qualification reports the admitted envelope rather than the planner's maximum.
- Rationale: `fit=0` against an unqualified claim is not a platform finding. Scoping the report to what the contract admits makes `fit` a statement about resources, and it follows the contract row when MEM-1G raises it.

## Open risks and follow-ups

- [ ] MEM-1G remains open. The ARM arithmetic fits — 1024 MiB payload plus ~2 MiB of tables against the 1,584,454,000 ordinary bytes the root admits — but the observed refusal is `SLIME_GRAPH spawn failed ... error=DestinationSlotsExhausted` for the fourth holder, recorded in [`one-gib-spawn-exhaustion.log`](one-gib-spawn-exhaustion.log). The descriptor-table sizing fixed here is a prerequisite, not the whole of it: the run predates that fix and must be repeated.
- [ ] MEM-64M is reopened. The required 20 spawn/grow/exit-or-fault cycles at the 16384-page ceiling remain unobserved; the existing reclamation and lifecycle results do not substitute for that workload.

## Artifacts and provenance

- Raw transcripts: [`private-memory-plane-arm.log`](private-memory-plane-arm.log), [`large-map-retry-arm.log`](large-map-retry-arm.log), [`one-gib-spawn-exhaustion.log`](one-gib-spawn-exhaustion.log).
- Companion entry: [MEM-PLATFORM's ordinary-memory inventory](../2026-09-13-mem-platform-ordinary-inventory/index.md), whose per-range evidence the 1 GiB feasibility arithmetic rests on.
- Related work item: MEM-64M, `.tasks/items/01a07a2d-0060-7d52-8e8d-1c7f45246f8d.md`.

## Corrections

### 2026-09-13 review follow-up

The earlier verification rows describe the pre-review observations, not a pass
of the final reviewed tree. Review found sparse leaf-table tracking, a mismatch
between simulated and actual extent order, unsupported `--workload 1g` recipe
arguments, stale run identities, a pre-build check of installed outputs, and
insufficient quota-refusal and ordinary-probe validation.

- Private regions now retain a bounded per-span leaf bitmap separately from
  their table count. Host regressions cover `grow(513)` followed by `grow(1)`,
  and reuse of a later-span table retained after a failed growth.
- Capacity simulation replays the actual alternating table/data extent order.
  An 8 MiB untyped with a 4 KiB watermark, 1 MiB static backing, and a 1024-page
  private plan is correctly refused. The prior four-holder positive fixture
  also assumed the optimistic order: it does not fit its 2 GiB region after
  the 128 MiB watermark; the positive fixture now uses 4 GiB. Payload-byte
  subtraction alone is not proof of an ordinary-memory layout fit for MEM-1G.
- The heap probe requests exactly to its target reservation from its measured
  backed-page count, crossing the smaller declared quota without crossing the
  reservation. It verifies refusal leaves the base and backed pages unchanged;
  the checker binds `QuotaExceeded` to the holder, declaration, and request.
- The unsupported 1 GiB recipe arms are removed. Closure inputs were refreshed
  before the run records were regenerated with `--bless`.
- The source-pin precheck no longer reads installed prefixes; explicit
  `--prefix --platform` validation still checks their memory windows. Ordinary
  probe evidence must describe an aligned, fully contained 4096-byte frame.

Direct verification after these corrections:

| Command/scenario | Observed result |
|---|---|
| `just test_sel4_root` | 239/239 passed, including the three new regressions |
| `just private_memory_check` | Passed: 24 markers in 7 chains; three AArch64 image cases and one RV64 case, both measuring the 16384-page ceiling and validating the heap quota refusal |
| `just sel4_root_boot_check` | Passed on AArch64, including the ordinary-memory probe oracle |
| `just sel4_gate_control_check` | Passed: 48 gates, 1945 transcript/layout mutations, 8 identity cases, and 4 runtime cases |
| `just system_test_run_check` | Passed: 50 records, 46 closure-backed and 4 explicitly exempt |
| `python3 scripts/generate/generate-system-image-closures.py --check` | Passed: 59 closure and negative records current |
| `python3 scripts/check/check-sel4-pins.py --skip-host-tools` and explicit `--prefix --platform` checks for both QEMU targets | Passed; stale 1024 MiB control rejected |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just tasks_check` | Passed |

These fixes do not close MEM-64M's remaining raised-capacity reuse campaign and
do not claim completion of MEM-1G or physical-machine qualification.

The matching `python3 scripts/check/check-sel4-root-boot.py --platform
qemu-riscv-virt` run also passed: ordinary probe `paddr=0x13f8fe000 bytes=4096
beyond_legacy=0 verified=1`, followed by the ordered root-boot verdict. Final
`just devlog_check`, `just tasks_check`, and `just typos` passed; the closure and
run identity checks were repeated after the code and formatting settled.
