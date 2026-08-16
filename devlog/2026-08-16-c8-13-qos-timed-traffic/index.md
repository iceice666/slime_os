# C8.13 — the QoS-timed clock wiring the last pass reverted, done in one coordinated change

| Field | Value |
|---|---|
| Date | 2026-08-16 |
| Kind | Change |
| Status | Verified |
| Scope | `components/bins/src/bin/{fabric-service.rs,fabric-publisher-b.rs}`, `contracts/generation/v1/fixtures/{sel4-traffic.zti,sel4-saturation.zti}`, `contracts/fabric-trace/v1/schema.zt`, `scripts/check/{check-sel4-traffic-plane.py,check-sel4-saturation-plane.py}`, `roadmap/02-core-runtime.md` |
| Roadmap | C8.13 |
| Gates | `just sel4_traffic_check`, `just sel4_saturation_check`, `just sel4_qos_check`, `just sel4_gate_control_check` |
| Trigger | C8.13's open follow-up: "QoS-timed stream traffic running concurrently with call/operation (dropped after its clock-grant wiring proved to need its own multi-step discovery" |
| Baseline | The stream plane's QoS-timed arm (RELIABLE retry accounting/exhaustion, deadline/lifespan/liveliness expiry) ran only in `sel4-qos.zti`'s standalone plane; `sel4-traffic.zti` never drove it, and a prior attempt to widen `qos_check()` alone was reverted for regressing `sel4_trace_check` |

## Summary

The 2026-08-15 pass tried widening `fabric-service.rs`'s `qos_check()` to
`"qos" || "traffic"` alone and reverted it: the timed arm's `TIME_SLOT`
control endpoint had nothing real bound to it under `"traffic"`, since the
`fabric-publisher-b-clock` grant only ever existed as a hand-authored entry in
`sel4-qos.zti`, never generated or declared for the unified partition. This
pass did the three coordinated changes that attempt needed together: added
the identical grant (plus two bindings, at slots each side's own
generation-derived layout computes rather than guessed) to `sel4-traffic.zti`
and its derivative `sel4-saturation.zti`, widened `qos_check()`, and widened
`fabric-publisher-b`'s own send-loop gate. It booted clean on the first
attempt.

`fabric-service.rs`'s `TIME_SLOT` is `FABRIC_FIRST_CONTROL_SLOT +
FABRIC_CLIENTS.len() + FABRIC_SUPERVISION.len()` -- a formula computed from
generated per-generation tables everywhere, so it already resolved to 15 for
the traffic profile (2 + 7 clients + 6 supervision) with no code change;
verified against the actual build-time-generated `fabric_profile.rs` before
picking that number, not derived from the formula alone. `fabric-publisher-b`'s
own `TIME_SLOT` stays the literal `3` it already hardcoded for the qos
fixture: its own local slot layout (0=control, 1=minted buffer factory,
2=diagnostics-ingress import, 3=next free) turned out to be identical across
both profiles, confirmed by booting rather than assumed.

A side effect the previous session's revert never got far enough to observe:
`Subscriber::retry_count` now genuinely advances under `"traffic"`, driven by
`fabric-subscriber-b`'s existing deliberate telemetry stall (unrelated to
this pass -- the stall itself already existed) taking long enough, while
consuming diagnostics via its `"traffic"`-branch code path, for the RELIABLE
telemetry subscriber to accumulate real retries before being caught up. This
pass adds the stream plane's own `RESOURCE_RETRIES` peak (cumulative,
matching `call_broker.rs`'s existing convention), verified deterministic
(byte-identical peak of 4) across three repeat boots -- a property this
codebase's whole trace-evidence design already required of every counter,
not a claim invented for this one.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/generation/v1/fixtures/{sel4-traffic.zti,sel4-saturation.zti}` | New `fabric-publisher-b-clock` grant (endpoint, send/recv, source=fabric-publisher-b, target=fabric-service, transferable=false -- identical to `sel4-qos.zti`'s own declaration) plus two bindings: fabric-service@15, fabric-publisher-b@3. `sel4-saturation.zti` regenerated fresh from the updated `sel4-traffic.zti` plus its own two field changes, so the two fixtures stay in lockstep | The clock edge exists in the fixture, not just in code that assumes it does -- the exact gap the prior revert's postmortem named |
| `components/bins/src/bin/fabric-service.rs` | `qos_check()` widened to `"qos" \|\| "traffic"`; new `peak_retries` field/sampling/emission (`RESOURCE_RETRIES`, peak-only) | The QoS-timed arm's own gate now matches which action actually provisions its control endpoint; the counter C8.13 already excluded for stream now has a real signal to report |
| `components/bins/src/bin/fabric-publisher-b.rs` | Simulated-time send-loop gate widened to `"qos" \|\| "traffic"` | The clock's only sender drives it under the action that now grants it the endpoint |
| `contracts/fabric-trace/v1/schema.zt` | Doc comment corrected: no longer claims `resourceRetries` is structurally excluded from the stream worker | The schema doc no longer contradicts what `fabric-service.rs` now emits |
| `scripts/check/check-sel4-traffic-plane.py`, `check-sel4-saturation-plane.py` | `grants=44` → `grants=45` (one more declared grant); `EXPECTED_RESOURCES["stream"]` gains a `retries` entry; module docstrings corrected (traffic gate no longer claims the QoS-timed arm is absent; saturation gate's "not attempted" list reclassifies retries as "evidenced but not saturated") | Both gates assert against what the fixtures and code actually do, not what they did before this pass |
| `roadmap/02-core-runtime.md` | C8.13 status paragraph records the clock wiring and the corrected retries accounting | The roadmap states the milestone's actual current shape |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A future change to `fabric-subscriber-b`'s diagnostics-consumption timing shifts the stream retries peak away from 4 | `just sel4_traffic_check` | `check_resources` requires exactly one `RESOURCE_RETRIES` record for the stream family; a structural failure (not merely a different number, since the check does not pin the value) would surface as a missing/duplicate record instead |
| A future fixture edit desyncs `sel4-traffic.zti` and `sel4-saturation.zti`'s grant/binding sets again | `just sel4_saturation_check` | Build or admission failure, since both fixtures must carry the same clock wiring for `qos_check()`'s shared code to find a real endpoint |
| `qos_check()` widens further and accidentally activates under an action that does not provision the clock endpoint | `just sel4_boot_check`, `just sel4_matrix_check`, `just sel4_call_check`, `just sel4_operation_check`, `just sel4_visibility_check` | A refused receive/send against an unbound `TIME_SLOT`, or a boot timeout |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_traffic_check`, `just sel4_saturation_check` (built and booted fresh after every edit) | Pass; raw serial confirmed `kind=qos order=time now=50/100/200/300/400/500/600` records and `[fabric] reliable retry accounted` ×4 plus one `QoS retry exhausted` | Direct |
| Repeat boot of `build/slime-sel4-traffic.elf` ×3 (outside the gate, raw QEMU) | Stream `RESOURCE_RETRIES` peak was exactly 4 every time, byte-identical | Direct |
| `just sel4_qos_check`, `just sel4_stream_check`, `just sel4_boot_check`, `just sel4_matrix_check`, `just sel4_visibility_check`, `just sel4_call_check`, `just sel4_operation_check`, `just sel4_trace_check` | All pass unchanged | Direct — confirms the shared `qos_check()`/`fabric-service.rs` changes affect only the `"qos"` and `"traffic"` actions, traced through every action's dispatch in `main()` |
| `just sel4_gate_control_check` | 31 gates reject 1188 mutated transcripts (unchanged count; the `grants=45` fix keeps the pinned marker count at 10) | Direct |
| `just contracts_check`, `just generation_check`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos`, `just test_host`, `just test_sel4_root` | All pass | Direct |

## Decisions

- **Decision:** Do all three coordinated changes (fixture grant/bindings, `qos_check()` widening, `fabric-publisher-b`'s gate widening) in one pass rather than incrementally.
  **Rationale:** The prior session's revert was caused by doing only the code-side gate widening without the fixture wiring, leaving the timed arm reaching for a control slot nothing was bound to. Verifying `TIME_SLOT`'s formula against the actual generated `fabric_profile.rs` for both the qos and traffic profiles before writing the fixture bindings, rather than guessing a slot number, made the first boot attempt succeed.
- **Decision:** Add `RESOURCE_RETRIES` for the stream plane once its evidence became real, rather than leaving the stale "cannot move" documentation in place.
  **Rationale:** A fresh reviewer pass caught that this counter's peak, sampled once per sweep as a `max` over a monotonically non-decreasing (`saturating_add`) per-subscriber field, cannot miss the true peak the way an occupancy counter's sampling frequency could — the same soundness argument `call_broker.rs`'s existing `peak_retries` already relies on. Verified deterministic by direct repeat-boot measurement rather than asserted from the sampling argument alone.
- **Decision:** Do not claim the retries value's exact magnitude (4) is a designed property of a "retry-inducing scenario." It is a byproduct of `fabric-subscriber-b`'s pre-existing telemetry stall interacting with the newly-active clock, not a scenario purpose-built to exercise RELIABLE retry semantics under traffic. The evidence is real and deterministic in this fixed-generation system, but its provenance is incidental — recorded honestly here rather than overclaimed as intentional.

## Open risks and follow-ups

- [ ] The saturation gate does not drive `retries` (or `eventDepth`, or any shared-buffer quota) to an exact declared bound the way it does in-flight calls/operations/retained results. `retries=4` is declared in both fixtures and the observed peak is also 4 -- possibly already saturated, like `inFlightCalls`/`retainedSamples` were found to be -- but this was not verified with `declared_limits()`-backed equality assertions in this pass.
- [ ] The stream retries peak's provenance (a side effect of `fabric-subscriber-b`'s diagnostics-consumption timing, not a deliberately paced retry scenario) means a future refactor of that consumption path could silently change the peak to a different nonzero value, or in principle to zero, without the refactor's author realizing it touches C8.13 evidence. `check_resources` would still pass (it requires exactly one record, not a pinned value), so this is a soft coupling risk, not a masked one.
- [ ] The operation plane's pending-delivery count, the call plane's outstanding-loan count, shared-buffer mapping/loan/buffer occupancy across 8 holders, and a live capability-slot ceiling remain open, per `devlog/2026-08-16-c8-13-queue-history-evidence/index.md` and `devlog/2026-08-16-c8-13-saturation-ceilings/index.md`.

## Artifacts and provenance

- Focused report: none; the investigation is summarized above and in *Decisions*.
- Raw transcript: none captured separately.
- Serial output: `just sel4_traffic_check`'s own transcript (reproducible by running the gate); the three repeat-boot captures confirming retries determinism are reproducible by booting `build/slime-sel4-traffic.elf` directly.
- Related roadmap item: [C8.13](../../roadmap/02-core-runtime.md#c813--concurrent-cross-plane-traffic-and-resource-ceilings).
