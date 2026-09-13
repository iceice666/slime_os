# Private backing accounting and bounded growth traversal

| Field | Value |
|---|---|
| Date | 2026-09-09 |
| Kind | Defect |
| Status | Fixed |
| Scope | `slime-root` private backing planning, provisioning, allocation records, transaction rollback, and host/QEMU gates |
| Work items | none |
| Gates | `just test_sel4_root`, `just private_memory_check`, `just sel4_root_boot_check` |
| Trigger | PR #25 review against `9578922` identified optimistic runtime backing accounting and root-wide descriptor scans |
| Baseline | Private growth retained failed large-map backing and transaction-local rollback, but capacity planning did not describe the provisioned full-window shape and incremental acquisition scanned the root descriptor pool |

## Summary

The runtime provisioner reserved two 2 MiB data extents at the 512-page quota while `plan_task_backing` reported one, so admission and capacity reports understated RAM, extent descriptors, allocation descriptors, and CSlots. Private acquisition and transaction handling also found task records by scanning the root-wide allocation array. The fix makes one clamped layout authoritative for supported quotas and adds arena-local reusable and in-flight indices with lifetime-bound allocation tokens.

## Observable symptom

- Command: PR #25 review of the private-memory capacity and retry changes.
- Expected: the planner exactly describes runtime provisioning for quotas through 512 pages, and consecutive one-page growth visits only the selected task records.
- Observed: quota 512 planned one data extent while provisioning retained two; acquisition, commit, unwind, mark, and reset searched the root-wide descriptor array.
- Exit/fault/serial evidence: host regressions now assert the exact 512-page provisioning trace and exact 513/513 reusable/mark/commit record visits for 512 one-page growths.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `provision_private_backing` special-cased quota 512 with a second data extent, while `plan_task_backing` used the segmented planning-only formula | Runtime capacity was understated by one data extent, one descriptor, one allocation, and two CSlots |
| 2 | Private reusable acquisition and transaction completion iterated `allocations` directly | Work scaled with `MAX_TASK_ALLOCATIONS`, including unrelated holders |
| 3 | The existing private growth body called seL4 primitives directly | A static kernel seam was required to execute the real body in host tests without duplicating its control flow |

## Root cause

Runtime provisioning and capacity planning encoded the supported-quota layout independently, so the retained fallback data extent at 512 pages existed only in the provisioner. Allocation records carried ownership and transaction bits but no arena-local linkage, forcing reusable selection and transaction completion to rediscover state through global scans.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Backing layout | Added `PrivateBackingLayout::for_quota` and a single request driver used by planning and live provisioning | Every supported runtime quota has one authoritative extent and allocation shape |
| Admission callers | Task construction and generation slot admission read the shared allocation count | Preflight matches live provisioning without a caller-supplied slot formula |
| Allocation records | Narrowed extent indices and added one compact state link while retaining owner and serial | Records remain at most 16 bytes and state linkage cannot cross task lifetimes |
| Arena state | Added per-kind reusable heads, an in-flight head and granule count, and a release guard | Hot private paths walk only one arena's relevant records and cleanup retries remain isolated |
| Growth execution | Added `PrivateMemoryKernel`, native dispatch, direct allocation tokens, and `grow_with_kernel` | Host tests execute the production growth body and token resolution is direct and lifetime-bound |
| Regression coverage | Added exact provisioning, failure-prefix, capacity, one-page traversal, ownership, and real rollback tests; host count is 227 | The two review findings fail deterministically if reintroduced |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Planner/runtime backing drift | `just test_sel4_root` | `runtime_plan_matches_provisioning_trace` or capacity boundary regression fails |
| Root-wide private descriptor scans return | `just test_sel4_root` | single-page growth visit totals exceed the exact selected-record count |
| Stale or foreign state links mutate another arena | `just test_sel4_root` | ownership/release isolation regression fails |
| Native rollback or task teardown changes | `just private_memory_check`, `just sel4_root_boot_check` | private-memory retry or root lifecycle marker contract fails |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test_sel4_root` | 227 passed, 0 failed across 19 modules | Direct |
| `just fmt_check_all` | Passed | Direct |
| `just lint_all` | Passed with warnings denied | Direct |
| `just sel4_root_boot_check` | Ordered generation, timer, task, IPC, fault, and ready markers observed | Direct |
| `just private_memory_check` | ARM: 23 markers across 7 causal chains and 3 image cases; RV64: 23 markers across 7 causal chains and 1 image case | Direct |

## Decisions

- Decision: keep the planning-only segmented formula above 512 pages while deriving supported runtime plans from the clamped live layout.
- Rationale: this fixes current provisioning truth without claiming a future multi-span runtime policy.
- Rejected alternative: delete the second 2 MiB data extent. That loses the retained-large-capability fallback shape the retry contract requires.
- Decision: use compact per-arena singly linked state buckets rather than a permanent ownership chain or root-slot map.
- Rationale: hot paths become quota-bounded while cold release and global allocation placement retain their existing simple scans.
- Rejected alternative: add a root-wide CSlot-to-record index. It increases permanent metadata and duplicates ownership authority.

## Open risks and follow-ups

- [ ] The host kernel records synthetic primitives; real untyped allocation and mapping remain the QEMU gates' responsibility.

## Artifacts and provenance

- Focused report: none.
- Raw transcript: none.
- Serial/debugger/model output: command output from this implementation session.
- Related work item: none.
