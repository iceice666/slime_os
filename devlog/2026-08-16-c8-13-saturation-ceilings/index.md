# C8.13 — a saturation fixture, and which declared ceilings a manifest field can actually prove

| Field | Value |
|---|---|
| Date | 2026-08-16 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/generation/v1/fixtures/sel4-saturation.zti`, `scripts/check/check-sel4-saturation-plane.py`, `scripts/build/{build-sel4.py,build-generation.py}`, `scripts/check/check-sel4-gate-controls.py`, `Justfile`, `roadmap/02-core-runtime.md` |
| Roadmap | C8.13 |
| Gates | `just sel4_saturation_check`, `just data_fabric_saturation_check`, `just sel4_gate_control_check` |
| Trigger | C8.13's open follow-up: "a saturation scenario that deliberately drives every declared ceiling to its manifest bound at once, and asserts neither an exceeded bound nor a deadlocked route worker" |
| Baseline | `sel4-traffic.zti` carries real concurrent traffic with comfortable headroom against every declared resource ceiling; nothing before this entry proves any ceiling was ever actually reached rather than merely bounded |

## Summary

Investigating "saturate every declared ceiling" against the actual admission
and runtime code (`boot-contracts/src/fabric_graph.rs`, the C7.3 shared-buffer
quota machinery, the three broker binaries) found that the 19 fields
`fabricGraph.limits` declares split into two classes: eleven are upper bounds
a real runtime table is sized from and can genuinely be driven to (in-flight
calls, in-flight operations, retained results, event-depth pending
deliveries, shared-buffer mapping/loan/buffer counts, retries), and the rest
(`queueDepth`, `historyDepth` graph-wide, `capabilitySlots`) are declared but,
per direct code reading, never checked against real usage at all — a
structural fact, not a scope choice, since decode only compares them against
a global `LIMIT_*` ceiling, never against what the graph actually consumes.

Of the reachable eleven, three had a concrete, already-instrumented signal
(from the 2026-08-16 resource-evidence pass earlier the same day) cheap
enough to verify by direct measurement: in-flight calls, in-flight
operations, retained operation results. Two of the three were already
exactly at their declared bound in the unmodified traffic fixture (4
declared, 4 observed for both) — a fact this pass turns from a coincidence
into an assertion. The third, in-flight operations, had slack (4 declared, 2
observed) and was tightened to 2. `sel4-saturation.zti` is otherwise
byte-identical to `sel4-traffic.zti`: same `bootAction`, same components, same
schedule, only the declared ceiling and the generation number differ. The
remaining eight reachable classes (event-depth, retries, and the six
shared-buffer quota fields across 8 holders) are not attempted: the first two
were separately proven this same day to be either a structural zero under
this schedule or already covered adequately (retries), and the six
buffer-quota fields have no live per-holder occupancy observable
component-side to safely tighten without extensive empirical binary search —
see *Open risks*.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/generation/v1/fixtures/sel4-saturation.zti` (new) | Exact copy of `sel4-traffic.zti` with `inFlightOperations` 4→2 and `generation` 36→39; nothing else differs | A saturation fixture is provably the traffic fixture's own scenario at a tighter declared bound, not a differently-shaped test that happens to also pass |
| `scripts/check/check-sel4-saturation-plane.py` (new) | Adapted from `check-sel4-traffic-plane.py` (identical `CHAINS`/`FAILURE_MARKERS`/lifecycle/concurrency/resource checks, verified line-for-line against the original); adds `check_saturation`, which asserts `RESOURCE_CALLS`/`RESOURCE_OPERATIONS`/`RESOURCE_RETAINED` peaks equal — not merely bounded by — the ceiling `declared_limits()` reads back out of the fixture itself | "Saturated" is a falsifiable claim: a future edit that loosens the fixture's ceilings back toward the traffic fixture's headroom fails this gate instead of silently continuing to pass a weaker test |
| `scripts/build/build-sel4.py` | `SATURATION_VARIANT`, `--saturation-plane` flag, and matching entries in `VARIANT_MANIFESTS`/`VARIANT_TARGET_DIRS`/`VARIANT_IMAGES`, mirroring the `TRAFFIC_VARIANT` wiring exactly | The saturation image is reachable by its own build flag the same way every other plane is |
| `scripts/build/build-generation.py` | `"sel4-saturation"` entry in `SEL4_MANIFESTS` | `SLIME_SEL4_MANIFEST=sel4-saturation` resolves to the new fixture |
| `Justfile` | `sel4_saturation_check`, `data_fabric_saturation_check` | The gate is reachable by a roadmap-named target, matching the traffic plane's own two-target convention |
| `scripts/check/check-sel4-gate-controls.py` | `("sel4_saturation_plane", "check/check-sel4-saturation-plane.py", 10)` | The new gate is itself proven to reject a deleted/transposed/appended-failure marker, pinned at the traffic plane's own count since the `CHAINS` shape is identical |
| `roadmap/02-core-runtime.md` | C8.13 status paragraph records the saturation fixture's scope and the eight ceilings it does not attempt, by name | The roadmap states exactly what "saturating every declared ceiling" now covers rather than leaving it implied by a passing gate |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A future traffic-scenario change drops the real peak for in-flight calls or retained results below 4, silently un-saturating a ceiling this entry asserts is already tight | `just sel4_saturation_check` | `the {family} worker's {name} peak was N, expected exactly 4` |
| A future fixture edit loosens `sel4-saturation.zti`'s `inFlightOperations` back toward 4 without noticing it stops being adversarial | `just sel4_saturation_check` | `declared_limits()` reads the loosened value back and the peak (still 2) no longer equals it |
| The saturation and traffic variants drift apart in `build-sel4.py`'s variant tables | `just sel4_saturation_check` (build step) | `KeyError`/`unknown seL4 manifest` at build time |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_saturation_check` (×2) | Pass; raw serial confirmed `event=3` (calls) peak=4, `event=2` (operations) peak=2, `event=8` (retained) peak=4, each exactly matching the fixture's declared ceiling, nothing dropped/rejected | Direct |
| `just sel4_gate_control_check` | 31 gates (was 30) reject 1188 mutated transcripts (was 1158) | Direct |
| `just sel4_traffic_check`, `just sel4_boot_check`, `just sel4_matrix_check`, `just sel4_operation_check`, `just sel4_call_check` | All pass unchanged | Direct — confirms the build-plumbing additions do not regress any existing plane |
| `just contracts_check` | Pass; 30 seL4 manifests now (was 29) | Direct |
| `just generation_check` | Pass | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | Pass | Direct |
| Direct code reading: `boot-contracts/src/fabric_graph.rs` lines 352-358, 653-667 | `queueDepth`/`historyDepth`/`capabilitySlots` are only ever compared against a fixed global `LIMIT_*` ceiling, never against the graph's own real usage | Direct — grounds the scope decision below, not merely asserted |

## Decisions

- **Decision:** Saturate `sel4-saturation.zti` by tightening the fixture's declared ceilings against the *unmodified* traffic scenario, rather than writing new adversarial component behavior or a new boot action.
  **Rationale:** Every ceiling this pass targets already has a real, instrumented peak from the same-day resource-evidence work; reusing that evidence turns "does the graph still work at the bound" into a pure fixture-numeric question with no new Rust code, at far lower risk than either wiring new adversarial traffic (rejected for the QoS-timed-clock follow-up the same day, for the same reason) or discovering a new component behavior gap the way each C8.10/C8.13 pass has repeatedly had to.
  **Rejected alternative:** Binary-searching the six shared-buffer quota fields (`mappings`/`loans`/`bufferPages`/`buffers`, graph-wide and per 8 holders) down to their real minimum via repeated boots. Rejected for this pass: no component-side syscall reports a live mapping/loan/buffer count (established in the 2026-08-16 resource-evidence entry), so "the tightest value that still passes" cannot be verified as *exact* the way the three implemented ceilings can — only as "small enough this run," which is a materially weaker claim the milestone's own exit condition ("saturating every declared ceiling") does not ask for.
- **Decision:** Read the three saturated ceilings back out of the fixture (`declared_limits()`) rather than hardcoding the numbers 4/2/4 in the check script.
  **Rationale:** A fresh reviewer pass found the hardcoded version could pass even after a future edit quietly loosened the fixture, since nothing would re-derive the "exact" bound from the fixture itself. Parsing `fabricGraph.limits` directly closes that gap at negligible cost.

## Open risks and follow-ups

- [ ] Six shared-buffer quota fields (`mappings`, `loans`, `bufferPages`, `buffers`, graph-wide and per-holder across 8 holders) are not saturated. Doing so honestly needs either a new root-side query syscall reporting live per-holder occupancy (the same gap the resource-evidence entry documents for `resourceMapping`/`resourceLoan`), or a bounded, disciplined empirical binary search across 32 numbers this pass did not budget for.
- [ ] `retries` (real ceiling, never approached at all under this fixed schedule — 0 observed) and `eventDepth` (feeds the operation plane's pending-delivery table, proven the same day to be a structural zero under this schedule) remain untested at their bound; reaching either needs adversarial component behavior change, not fixture tuning, and is a scenario-design task, not a numbers-only follow-up.
- [ ] `queueDepth`, `historyDepth` (graph-wide), and `capabilitySlots` are declared fields with no runtime consumer that checks them against real usage at all (confirmed by direct code reading, not inference). Tightening them would test nothing; making them meaningful is a mechanism change (wire them to something, or delete them), out of scope for a saturation-fixture pass.

## Artifacts and provenance

- Focused report: none; the investigation is summarized above and in *Decisions*.
- Raw transcript: none captured separately.
- Serial output: `just sel4_saturation_check`'s own transcript (reproducible by running the gate).
- Related roadmap item: [C8.13](../../roadmap/02-core-runtime.md#c813--concurrent-cross-plane-traffic-and-resource-ceilings).
