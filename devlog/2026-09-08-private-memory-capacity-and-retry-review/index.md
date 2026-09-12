# Private-memory capacity qualification and zero-page retry review

| Field | Value |
|---|---|
| Date | 2026-09-08 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root` graph launch and allocator capacity accounting, private growth retry state, embedded child fixture, seL4 private-memory plane checker, and generated image identities |
| Work items | none |
| Gates | `just private_memory_check`, `just test_sel4_root`, `just sel4_root_boot_check`, `just sel4_gate_control_check` |
| Trigger | Review of private-memory changes at `5312ab8ee2f33219ccab9d4e30bbfaaabb577f54` identified three correctness defects |
| Baseline | Normal product graphs admitted their declared quotas without a fatal four-holder hypothetical workload, failed initial small-page growth retried through its mapped leaf, and capacity reports included staged graph and static task costs |

## Summary

Normal private-memory images treated a hypothetical four-by-256 MiB workload as a boot prerequisite, a failed initial small-page transaction retained a mapped leaf in the allocator but lost that fact in the region, and the capacity report omitted each holder's static arena allocations and reserved bytes. Qualification now runs only in the existing large-map test image after graph materialization, reports a non-fatal fit computed from live free resources plus an exemplar task's real static backing, and the retry path records the mapped leaf immediately so a full-window retry selects base frames instead of attempting a conflicting large mapping.

## Observable symptom

- Command: `just private_memory_check` and review of the normal graph launch path.
- Expected: normal ARM/RV64 planes contain no hypothetical capacity prerequisite; the dedicated large-map image reports the staged graph plus four probe clones honestly; a zero-page failed small growth retries 512 pages through the retained leaf.
- Observed: normal launch could fatal before staging on the four-holder plan; capacity counted private backing and admission CSlots but not materialized graph/static arena costs; after an initial second-frame failure, `Region::leaf_tables` remained zero although the allocator retained the mapped table.
- Exit/fault/serial evidence: the completed gate records 23 normal markers, a restricted 4096-allocation/144-extent qualification with `fit=0`, task 1's two-page refusal followed by 512 base frames and one leaf table, aggregate 513 pages/two grants, teardown conservation, and `SLIME_ROOT READY`.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `launch_instance_graph` ran `plan_task_backing(65536)` before constructing any graph whenever a private-memory budget existed | Small legitimate products were required to satisfy an unrelated hypothetical workload |
| 2 | The report added `admission.required_root_slots` to a pre-staging free snapshot | It modeled neither the allocator's actual post-staging state nor aliases and static extents created during task construction |
| 3 | Allocation records already distinguish arena ownership, private records, and aliases; extents retain static kind and reserved size | Static exemplar cost can be derived without duplicating the ELF/VSpace planner |
| 4 | `unwind_private_transaction` deliberately retains a failed transaction's leaf table mapped | The region must record that kernel mapping before later bookkeeping can fail |
| 5 | Failure injection required a previously committed granule | The same injection could not exercise a zero-page first transaction even though in-flight position already identifies the second frame |

## Root cause

Qualification mixed a test workload claim into admission for every private-memory generation and computed it from a synthetic pre-staging snapshot. The capacity type had no representation for a task's already-materialized static backing, so it could not include TCBs, VSpace/CNode objects, image/runtime frames, page tables, or transfer-window aliases. Separately, leaf mapping state was committed only with logical page accounting; rollback intentionally preserved the kernel mapping but the region still claimed no leaf existed, so a whole-window retry selected large-frame backing against an occupied translation span.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Graph launch | Removed qualification from normal budget handling and placed it under `slime_private_fail_large_map` after all graph materialization | Ordinary products are admitted by declared quotas and real allocator limits only |
| Test image sizing | Added `slime_private_small_tables` only to the existing large-map root role, selecting 4096 allocation and 144 extent descriptors without enabling Duo platform behavior | The automated image exercises the constrained descriptor envelope without changing production tables |
| Allocator accounting | Added bounded `task_static_backing` derivation over non-private allocation records and exactly one live static extent; capacity requirements use checked arithmetic for private plus static costs | Four-holder requirements include aliases, static descriptors, CSlots, and reserved arena bytes without double-counting the plan's static extent |
| Capacity report | Emits one staged-graph qualification line with full required/available fields and a computed non-fatal fit | `fit=0` is an honest result rather than a boot failure or optimistic constant |
| Growth transaction | Records `region.leaf_tables = 1` immediately after successful leaf mapping and derives retry backing from that live state | Failed logical growth may retain translation structure while later retry uses the mapping that actually exists |
| Failure fixture | Injection depends only on the second in-flight granule; deliberate-fault task receives a 512-page quota and proves zero-page rollback, full base-frame retry, page zeroing/readback, fault supervision, and reclamation | Both committed-page and zero-page retry boundaries run in the same existing image |
| Semantic checks | Normal transcripts reject qualification markers; large-map parsing recomputes static-inclusive requirements and fit; rollback parsing enforces the complete causal sequence; gate controls mutate both reports | Fixed optimistic strings, missing static fields, wrong order, wrong backing, explicit failures, and missing terminals fail closed |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Normal products regain the hypothetical boot prerequisite | `just private_memory_check` | ARM or RV64 normal transcript contains `SLIME_MEM capacity` or `SLIME_MEM qualification` |
| Capacity omits staged/static costs or lies about fit | `just private_memory_check` | Large-map qualification fields fail recomputation or report anything except the resource conjunction |
| Initial rollback loses mapped-leaf state | `just private_memory_check` | Task 1 fails to grow 512 base frames with `leaf_tables=1`, or the child zero/readback proof is absent |
| Arithmetic overflow or a resource shortfall is accepted | `just test_sel4_root` | Checked requirement, static-record, pool-boundary, or per-resource shortfall cases fail |
| Semantic checker accepts weakened evidence | `just sel4_gate_control_check` | Capacity or rollback mutation controls are accepted |
| Fixture-only probe leaks into the normal root | `just sel4_root_boot_check` | The original quota-4/two-growth/reclamation marker chain changes |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test_sel4_root` | 222/222 host tests passed across 19 modules | Direct |
| `just sel4_gate_control_check` | 48 gates rejected 1884 mutated transcripts/layouts; private capacity and rollback semantic mutations rejected | Direct |
| `just private_memory_check` | ARM normal, large-map, and second-allocation images passed; RV64 normal image passed with 23 markers and no qualification marker | Direct |
| `just sel4_root_boot_check` | Pinned AArch64 root boot completed its original task, private-memory, fault, reclamation, and ready path | Direct |
| `just fmt_check_all` | Rust formatting checks passed | Direct |
| `just lint_all` | boot-contracts, host components, root, child, and product component clippy checks passed with warnings denied | Direct |
| `ruff check scripts/check/check-sel4-private-memory-plane.py scripts/check/check-sel4-gate-controls.py scripts/build/build-sel4.py` | All checks passed | Direct |
| Closure and test-run generation | 52 closure records current; 46 system-test-run records checked after blessing | Direct |

## Decisions

- Decision: keep four-holder qualification as a non-fatal report in the existing large-map image.
- Rationale: this review fixes accounting and product admission; it does not establish that four 256 MiB holders can run concurrently on every platform.
- Rejected alternative: raise production descriptor tables until `fit=1`. That would encode unverified headroom and increase every root image.
- Decision: derive static cost from one named staged exemplar rather than replay task planning.
- Rationale: allocation and extent records are the authoritative result, including aliases that an arena plan does not count.
- Rejected alternative: duplicate ELF/VSpace planning in the capacity path. A second planner would drift and still risk missing post-plan aliases.
- Decision: preserve mapped translation structure after rollback and expose it through `Region::leaf_tables`.
- Rationale: the kernel mapping exists and the allocator intentionally retains it; backing selection must follow kernel state, while logical page/grant accounting remains transactional.

## Open risks and follow-ups

- The constrained descriptor evidence is QEMU using Duo-sized tables; it is not physical Duo, low-RAM, or 12-bit CNode qualification.
- The qualification report proves arithmetic over a staged graph plus four exemplar clones; it does not allocate or execute a real 1 GiB workload.

## Artifacts and provenance

- Implementation: [`slime-root/src/graph_runtime.rs`](../../slime-root/src/graph_runtime.rs), [`slime-root/src/object_allocator.rs`](../../slime-root/src/object_allocator.rs), [`slime-root/src/private_memory.rs`](../../slime-root/src/private_memory.rs), [`slime-root/src/main.rs`](../../slime-root/src/main.rs), and [`slime-root/child/src/main.rs`](../../slime-root/child/src/main.rs)
- Build selection: [`scripts/build/build-sel4.py`](../../scripts/build/build-sel4.py) and [`slime-root/build.rs`](../../slime-root/build.rs)
- Gates: [`scripts/check/check-sel4-private-memory-plane.py`](../../scripts/check/check-sel4-private-memory-plane.py) and [`scripts/check/check-sel4-gate-controls.py`](../../scripts/check/check-sel4-gate-controls.py)
- Generated identities: [`contracts/system-image-closure/v2/closures/`](../../contracts/system-image-closure/v2/closures/) and [`contracts/system-test-run/v1/runs/`](../../contracts/system-test-run/v1/runs/)
- Preceding investigation: [Private growth rollback and worker IPC](../2026-09-08-private-growth-rollback-and-worker-ipc/index.md)

## Corrections

- **2026-09-12:** This entry's generated-identity link now names
  `contracts/system-image-closure/v2/closures/`. The contract was versioned to
  v2 when `target.sdkRelease` was removed; only the path moved, and every
  conclusion above is unchanged. The closure identities this entry observed
  were recorded under the v1 domain and are not the identities the v2 corpus
  now carries.
