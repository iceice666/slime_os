# Second review round: refusal scoping, notification scope, and two coverage gaps

| Field | Value |
|---|---|
| Date | 2026-09-10 |
| Kind | Defect |
| Status | Fixed |
| Scope | The private-memory plane's refusal matcher, `check-system-spec.py`'s baseline exemption, the reclamation plane's private-extent evidence, the private-memory plane's test-run records, and `MAX_TASK_ALLOCATIONS`'s documented guarantee |
| Work items | none |
| Gates | `just test_sel4_root`, `just contracts_check`, `just system_image_builder_check`, `just system_test_run_check`, `just sel4_root_boot_check` |
| Trigger | PR #25 review against `52570122` (the first review round's fix commit) reported five further P2 findings |
| Baseline | `2d30362e`'s two P2 fixes landed in `52570122`; this round covers what a fresh full-diff review pass found beyond them |

## Summary

Five independent findings, all in verification tooling or documentation rather
than runtime mechanism: a refusal matcher that took the transcript's first
match instead of the granted holder's own; a baseline exemption that excused
every system's notification sections instead of only the one whose section
postdates its baseline; a reclamation-loop assertion satisfiable by static
extent reuse alone, never exercising a private one; two injected-fault
private-memory images with no frozen test-run record; and a capacity constant
whose comment claimed a guarantee its own neighboring test disproves. All five
are fixed and QEMU/host-verified; the reclamation fix is the only one that
touches runtime composition (a 1-page quota added to the existing
`reclamation-fault` fixture) rather than checker logic alone.

## Observable symptom

- Command: PR #25 review of `52570122`; locally each checker/test named below.
- Expected: a refusal is attributed to the task that produced it; a baseline exemption excuses only the system it is true for; a "reclaimed" assertion is satisfiable only by the kind it claims; every booted image has a frozen execution-input record; a capacity comment states what the constant provides.
- Observed: `check_measured_ceiling` matched the transcript's first `ReservationExceeded`, which could be the denied probe's; `POST_BASELINE_SECTIONS` popped `notificationGrants`/`notificationBindings` for every system; `check-sel4-reclamation-plane.py`'s `reusable_ram > 0` passed on an all-static composition; `sel4-private-memory-fail-large-map`/`-fail-second-allocation` had no `contracts/system-test-run/v1/runs/*.zti`; `MAX_TASK_ALLOCATIONS`'s comment said the pool "reserves enough for four 256 MiB holders" while its own neighboring test (`four_holders_include_static_descriptors_at_default_pool_boundary`) proves it does not, once static descriptors are included.
- Exit/fault/serial evidence: recorded per finding below.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `SLIME_MEM refused task={} delta={delta} cause={} detail={error:?}` carries no instance name, only a task id, and B68 already found this plane's two probes running in unconstrained order | The refusal match had to be scoped by the granted task's own id, resolved through the same `SLIME_MEM quota task=(\d+) instance=private-memory-granted ...` line the base/ceiling check already used |
| 2 | `sel4-matrix.zti`'s frozen baseline already carries real `notificationGrants`/`notificationBindings` content, predating C10.4's private-memory notification | The flat, all-systems exemption was silently excusing a populated section on an unrelated system, not only the new one |
| 3 | `sel4-reclamation.zti`'s system spec declares an empty `privateMemoryBudget`, so every task in the reclamation loop allocates only its static extent | `reusable_extent_bytes()` sums every inactive extent by design; nothing distinguished "a private extent was reclaimed" from "the loop's static churn was reclaimed, as always" |
| 4 | `check-sel4-private-memory-plane.py` boots three AArch64 images (`sel4-private-memory`, `-fail-large-map`, `-fail-second-allocation`) but `EXTRA_RUNS` only ever modeled the legacy RV64 literal-image arm | Two of the plane's three image cases had no frozen execution-input record for `check-system-test-run.py` to validate |
| 5 | `MAX_TASK_ALLOCATIONS = 4 * (MAX_PLANNED_PRIVATE_PAGES + 128) + 1` reserves four holders' *private growth* descriptors only; `four_holders_include_static_descriptors_at_default_pool_boundary` already pins that the same four holders' own static construction overruns the pool by exactly 3 at the one-descriptor floor | The comment's "reserves enough for four 256 MiB holders" was false as a deployment claim; true only as MEM-ARENAS's representability case, which `MAX_REGION_PAGES` (512 pages) keeps out of reach of any real holder today |

## Root cause

Four of the five are the same shape: a check or exemption stated a broader
guarantee than it verified, and nothing forced the narrower, true claim to be
written down or exercised. The fifth (reclamation) is a genuine coverage gap:
the fixture that was supposed to prove segmented private backing survives a
real QEMU task-lifetime churn never allocated any.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Refusal matching | `check_measured_ceiling` resolves `private-memory-granted`'s task id from the same quota line it already used for the base address, then scopes the refusal search to that task | A refusal is attributed to the holder that produced it, not to whichever probe's refusal the transcript happens to name first |
| Baseline exemption | `POST_BASELINE_SECTIONS` keeps only `privateMemoryBudget` (true for every system); `POST_BASELINE_SYSTEM_SECTIONS` scopes `notificationGrants`/`notificationBindings` to `sel4-private-memory` alone, with an independent comparison against the system spec's own declarations | `sel4-matrix` and any other system with real notification content stay baseline-compared |
| Reclamation evidence | `reclamation-fault` (used by no other composition) now declares `privatePageQuota = 1` and grows one page before its deliberate fault; `ObjectAllocator::reusable_private_extent_bytes` and a new `reusable_private_ram=` marker field make the claim kind-specific | "Reclaimed" for this plane now requires a private extent specifically, not only the loop's static churn |
| Test-run coverage | `EXTRA_CLOSURE_RUNS` in the test-run generator adds closure-backed records for `sel4-private-memory-fail-large-map` and `-fail-second-allocation` | Every booted image in this plane has a frozen, checked execution-input record |
| Capacity documentation | `MAX_TASK_ALLOCATIONS`'s comment states the pool covers four holders' private growth descriptors for MEM-ARENAS's representability case, names the static-descriptor gap its neighboring test pins, and defers closing it to the ceiling-raising milestone alongside the matching `plan_task_backing` fallback-extent gap | The comment no longer claims a deployment guarantee the pool does not provide |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The refusal match regresses to an unscoped first-match search | `just private_memory_check` | `check_measured_ceiling` fails or misattributes under either probe ordering |
| A future post-baseline section is exempted for every system again | `just contracts_check` (`check-system-spec.py`) | a system with real content in that section stops being baseline-compared, silently |
| `reclamation-fault` stops growing private memory, or teardown stops reclaiming it | `just sel4_reclamation_check` | `reusable_private_ram=0` fails the ordered-marker match |
| A new fault-injection image is added to the private-memory plane with no run record | `just system_test_run_check` | `check_records_and_planes_correspond` reports a missing record |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just private_memory_check` (AArch64) | 23 markers across 7 causal chains and 3 image cases | Direct |
| `just sel4_reclamation_check` | segmented task extents and root CSlots reclaimed and reused, private-kind reusable RAM observed nonzero | Direct |
| Mutation: `check_measured_ceiling`'s refusal search reverted to unscoped | not applicable -- fixed forward, no separable mutation harness for probe ordering; correctness argued from the shared task-id resolution already used by the base-address check in the same function | Direct (code-level) |
| Mutation: `POST_BASELINE_SECTIONS` reverted to the flat all-systems tuple, `sel4-matrix.zti`'s baseline notification target renamed | passes silently (bug reproduced); with the fix, `manifest.notificationGrants[0].target` mismatch is reported | Direct |
| Mutation: `reusable_private_extent_bytes()`'s emitted value forced to `0` | `just sel4_reclamation_check` fails: no ordered `reusable_private_ram=[1-9]\d*` match; restored source passes | Direct |
| `just test_sel4_root` | 228/228 passed across 19 modules | Direct |
| `just contracts_check` | passed, exit 0 | Direct |
| `just generation_check` | two isolated builds produced byte-identical generation and boot-store; 4 mutations refused | Direct |
| `just system_image_builder_check` | 52 closures current and resolving with distinct identities; 1 closure built twice byte-identically | Direct |
| `just system_test_run_check` | 48 records correspond one-to-one with plane gates, 46 resolve a closure, 2 declared closure-exempt, 5 controls refused | Direct |
| `just sel4_root_boot_check` | ordered generation, timer, task, IPC, fault, ready markers observed | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | passed | Direct |

## Decisions

- Decision: grant `reclamation-fault` its own private quota rather than modify `supervision-child` (shared by five compositions) or add a new dedicated component.
- Rationale: `reclamation-fault` is already single-composition, already a deliberate one-shot fault fixture, and `private_memory_grow`'s `Result` makes the added call a no-op refusal anywhere it might otherwise appear -- it does not, since nothing else references this binary.
- Rejected alternative: reuse `private-memory-probe`. Its granted/denied lifecycles run forever (needed for its own plane's re-query assertions), so it never exits and never exercises task-teardown reclaim.
- Decision: add a kind-specific `reusable_private_ram` marker field rather than only strengthening the composition.
- Rationale: a quota-bearing task alone still leaves the assertion checking the unrestricted sum; without the kind split, a private extent that leaked (never returned to the free list) would be indistinguishable from one that reclaimed, since the static loop's own reuse keeps the unrestricted total positive regardless.
- Decision: leave `MAX_TASK_ALLOCATIONS`'s value unchanged; correct only its comment.
- Rationale: no real holder can reach 256 MiB today (`MAX_REGION_PAGES` clamps to 512 pages), so the pool has orders of magnitude of headroom for real traffic; sizing it for a hypothetical deployment is the ceiling-raising milestone's job, done together with `plan_task_backing`'s matching `>512`-page fallback-extent gap so QEMU capacity evidence is re-taken once, not twice.

## Open risks and follow-ups

- [ ] `plan_task_backing`'s `>512`-page branch and `MAX_TASK_ALLOCATIONS`'s static-descriptor headroom both describe the same not-yet-supported four-256-MiB-holder scenario incompletely; the ceiling-raising milestone should close both together and re-take the private-memory-plane's frozen capacity markers once.
- [ ] `check_measured_ceiling`'s fix has no standalone mutation proof (unlike the other three): B68's own finding was that the two probes' relative order is not pinned, so a synthetic transcript with the denied probe's refusal first would need hand-authoring outside the real QEMU harness; correctness rests on code-level reasoning shared with the already-verified base-address check in the same function.

## Artifacts and provenance

- Focused report: none.
- Raw transcript: none.
- Serial/debugger/model output: command output from this session, quoted in Verification.
- Related work item: none.
