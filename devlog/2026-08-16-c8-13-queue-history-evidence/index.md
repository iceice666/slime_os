# C8.13 — two more resource classes, and why the other two real signals still can't ship

| Field | Value |
|---|---|
| Date | 2026-08-16 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/fabric-trace/v1/{schema.zt,gen_rust.zt}`, `components/proto/src/fabric_trace.rs`, `components/proto/tests/fabric_trace.rs`, `scripts/lib/fabric_trace_contract.py`, `components/bins/src/bin/fabric-service.rs`, `scripts/check/check-sel4-traffic-plane.py`, `roadmap/02-core-runtime.md` |
| Roadmap | C8.13 |
| Gates | `just sel4_traffic_check`, `just data_fabric_traffic_check`, `just sel4_trace_check`, `just sel4_gate_control_check` |
| Trigger | C8.13's 2026-08-15 partial-exit pass named "queue, history, event, mapping, loan, and capability-slot resource evidence" as its largest open item |
| Baseline | C8.13 emits bounded peak(+baseline) evidence for 6 of 11 declared resource classes: frames, shared buffers, retries, in-flight calls, in-flight operations, retained operation results |

## Summary

This pass picked up the largest of C8.13's three open follow-ups: the six
named-but-unimplemented resource classes. Investigating all six by reading
the actual code that would have to carry each signal — not by inference —
found that only two (`resourceQueue`, `resourceHistory`) have a real,
traffic-varying, honestly-measurable occupancy in the stream plane today. The
other two "real signal" candidates each hit a distinct, concrete blocker
found only by building and booting: the operation plane's pending-delivery
count is a structural zero under the declared traffic schedule (the same
"evidence that cannot change" problem C8.13 already excluded stream retries
for), and the call plane's outstanding-loan count has nowhere to go — its
trace sink is already at 63 of the schema's absolute 64-record ceiling. The
remaining two (shared-buffer mapping occupancy, a live capability-slot count)
have no signal at all: nothing in the current component-side syscall surface
reports a live occupancy for either, only fixed provisioning-time constants
or transient map-then-unmap windows a per-sweep sample would never catch.

C8.13 now emits 8 of 11 declared resource classes and remains **In progress**
against its full exit condition; the roadmap entry states exactly which four
remain and why.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/fabric-trace/v1/{schema.zt,gen_rust.zt}` | Four new resource codes: `resourceQueue`=9, `resourceHistory`=10, `resourceEvent`=11, `resourceLoan`=12; `resourceComplete`/`maxResourceCounter` renumbered 9→13. Doc comment explains what each of the four means, which two are emitted, and precisely why the other two are declared-but-silent | The schema stays the single source of truth for every resource counter's meaning, including the ones with no emitter yet, rather than letting that knowledge live only in a devlog paragraph |
| `components/bins/src/bin/fabric-service.rs` | `peak_queue`/`peak_history` fields on the stream broker's loop state, sampled every sweep as `subscriber.in_flight` and `subscriber.history.len()` summed across every provisioned subscriber (disjoint by construction: `deliver` pops an entry off `history` in the same step it grows `in_flight`), emitted as peak-then-baseline records folded into the existing `GENERATION_BOOT_ACTION == "traffic"` gate that already guards `RESOURCE_BUFFERS` | The stream worker's per-subscriber outstanding-delivery and KEEP_LAST backlog occupancy are observable evidence, gated to the one plane whose fixture budgeted for it, without duplicating the traffic-action guard the buffers counter already establishes |
| `components/proto/tests/fabric_trace.rs` | `a_resource_record_must_name_which_count_it_carries`'s "every declared counter is admitted" loop extended to the four new codes | The widened `MAX_RESOURCE_COUNTER` admission range (9..=12) is exercised by name, not just implied by the boundary checks around it |
| `scripts/check/check-sel4-traffic-plane.py` | `EXPECTED_RESOURCES["stream"]` gains `(RESOURCE_QUEUE, "queue", 2)` and `(RESOURCE_HISTORY, "history", 2)`; the surrounding comment states, for each of the four declared-but-silent classes, the specific reason it has no entry | `check_resources` now falsifies a regression in either new counter's peak/baseline pairing the same way it already does for frames/buffers, and a reader of the comment cannot mistake a declared code for an emitted one |
| `roadmap/02-core-runtime.md` | C8.13 status paragraph updated from "6 of 11" to "8 of 11", with the four remaining classes named individually and each one's specific blocker stated | The roadmap, not just this entry, carries the live count `AGENTS.md`/`devlog/README.md` designate it the authoritative home for |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A future change to `deliver`/`drain_acks` breaks the disjointness `peak_queue`+`peak_history`'s soundness depends on (a sample counted in both tables at once) | `just sel4_traffic_check` | The stream worker's queue+history peak sum would exceed the subscriber table's own bound in a way `check_resources`' baseline-zero assertion would catch on the next drained run |
| A future edit re-widens `RESOURCE_QUEUE`/`RESOURCE_HISTORY` emission off the `"traffic"` gate | `just sel4_stream_check`, `just sel4_qos_check` | `dropped=N` (N>0) in the stream family's trace summary on a standalone fixture whose `traceDepth` predates these counters |
| A new resource code is declared in the schema without a matching admission-test entry | `just test_host` (`fabric_trace` suite) | `a_resource_record_must_name_which_count_it_carries` no longer exercises every declared code, so a validator regression on an untested range would pass silently |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_traffic_check` (×2, plus one raw-serial boot outside the gate) | Pass; raw transcript showed `event=9 high_water=1`→`0` (queue) and `event=10 high_water=8`→`0` (history) — real, non-trivial, non-fake evidence | Direct |
| `just sel4_stream_check`, `just sel4_qos_check`, `just sel4_call_check`, `just sel4_operation_check`, `just sel4_boot_check`, `just sel4_matrix_check`, `just sel4_visibility_check`, `just sel4_trace_check`, `just sel4_gate_control_check` | All pass unchanged | Direct — confirms the shared `fabric-service.rs` changes do not regress any standalone plane |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | Pass | Direct |
| `just test_host` (`fabric_trace` suite, 19 tests including the widened admission loop) | Pass | Direct |
| `just contracts_check`, `just generation_check` | Pass | Direct |
| `just test_sel4_root` | 112/112 | Direct |
| Direct measurement: raw serial boot of the pre-change traffic image | `operation complete capacity=64 records=32`; `call complete capacity=64 records=63`; `stream complete capacity=64 records=8` — the evidence the call-plane-loan and operation-plane-event decisions below are grounded in | Direct |

## Decisions

- **Decision:** Implement `resourceQueue` and `resourceHistory` in the stream plane; declare but do not emit `resourceEvent` and `resourceLoan`; do not declare `resourceMapping` or a capability-slot code at all.
  **Rationale:** Each of the six named classes was investigated by reading the code that would carry it, not assumed implementable from its name. `resourceQueue` (`Subscriber::in_flight`) and `resourceHistory` (`Subscriber::history.len()`) are real, disjoint, per-sweep-sampleable occupancies that visibly move under the traffic scenario (1 and 8 respectively, draining to 0). `resourceEvent`'s backing table (`operation_broker.rs::pending_deliveries`) only fills when a client-bound send would block, and a raw boot showed it never does under the fixed schedule — shipping it would report a peak that cannot move, the exact category C8.13 already excluded stream retries for. `resourceLoan`'s backing state (`call_broker.rs`'s `SharedOutstanding`/`CancellingShared` payload count) is real and distinct from `resourceBuffers`, but the call plane's trace sink is already at 63 of its schema-maximum 64 records — 2 more records would silently drop one, which `check_resources`'s `dropped == 0` assertion exists specifically to catch. `resourceMapping` and a capability-slot count have no signal at all: every mapping any of the three workers holds is either fixed at provisioning time or mapped-and-unmapped within one synchronous call, and no syscall (`shared_buffer_map`/`shared_buffer_loan`/`shared_buffer_return`/...) returns a live occupancy to the caller.
  **Rejected alternative:** Reducing existing call-plane trace volume to make room for `resourceLoan` — would trade real, already-verified evidence for new evidence, which is not a net gain and is outside this pass's scope of *adding* evidence.
- **Decision:** Fold the two new emission sites into the existing `GENERATION_BOOT_ACTION == "traffic"` guard around `RESOURCE_BUFFERS`, rather than a second adjacent guard.
  **Rationale:** A fresh reviewer pass found the duplicated guard added no protection (`GENERATION_BOOT_ACTION` is a compile-time constant) and only restated the adjacent comment's reasoning. One gate per emission phase matches the surrounding style.

## Open risks and follow-ups

- [ ] The operation plane's pending-delivery count (`resourceEvent`) and the call plane's outstanding-loan count (`resourceLoan`) remain undriven. `resourceEvent` needs the traffic schedule itself changed to create real backpressure on a client's delivery endpoint — out of scope for an evidence-only pass, since it would change what the scenario exercises. `resourceLoan` needs either headroom freed in the call plane's trace sink or the schema's `maxTraceDepth` ceiling reconsidered — the latter is a deliberate structural constant, not a fixture knob.
- [ ] Shared-buffer mapping occupancy and a live capability-slot count remain entirely unobservable to a userspace component. Making either real would need new root-side introspection surface (a query syscall reporting live per-holder `mapping_count`/`loan_count` from `slime-root/src/shared_buffer.rs`'s `HolderQuota`, and — harder — a per-child CNode occupancy counter `slime-root/src/object_allocator.rs` does not currently track at all, since it only tracks the root's own global CSlot pool).
- [ ] QoS-timed stream traffic concurrent with call/operation, and a saturation scenario driving every declared ceiling at once, remain untouched from the prior pass — see `devlog/2026-08-15-c8-13-traffic/index.md`'s own open risks.

## Artifacts and provenance

- Focused report: none; the investigation is summarized above and in *Decisions*.
- Raw transcript: none captured separately.
- Serial output: `just sel4_traffic_check`'s own transcript (reproducible by running the gate); the pre-change raw-boot record counts (`operation records=32`, `call records=63`, `stream records=8`) that grounded the `resourceLoan`/`resourceEvent` decisions are reproducible by booting `build/slime-sel4-traffic.elf` directly with serial output captured.
- Related roadmap item: [C8.13](../../roadmap/02-core-runtime.md#c813--concurrent-cross-plane-traffic-and-resource-ceilings).
