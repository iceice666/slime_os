# C8.7 — Native operations

| Field | Value |
|---|---|
| Date | 2026-07-29 |
| Kind | Change |
| Status | Verified |
| Scope | Fabric operation contract, broker, participants, generation graph, bootstrap, and QEMU checks |
| Roadmap | C8.7 |
| Gates | `just fabric_operation_check` |
| Trigger | C8.7 implementation |
| Baseline | C8.6 provided bounded calls and C8.4 provided bounded streams, but no composed native operation transport existed. |

## Summary

C8.7 adds generation-authorized native operations composed from goal, feedback, result, cancellation, and retained-result retrieval records. The userspace broker binds every operation to an authenticated client role, enforces graph-derived active, feedback, retry, event, and retention bounds, preserves mandatory outcomes under endpoint backpressure, supports a declared participant restart without replaying work, and settles peer death while an unrelated operation route remains usable. The focused QEMU gate and the full project verification stack pass.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Contract | Added the versioned Zutai fabric-operation envelope and generated Rust bindings. | Every cross-component operation record has one bounded, validated source of truth. |
| Broker | Added bounded correlation, feedback, cancellation, terminal delivery, retention, expiry, restart, and peer-death handling. | Exact role authority and one terminal result survive concurrency, backpressure, timeout, and restart. |
| Generation/bootstrap | Declared operation routes, participants, controls, ceilings, images, grants, and task accounting. | Runtime authority and resource ceilings come from the admitted generation graph. |
| Live scenario | Added operation clients, server, explicit time source, replacement participant, and deterministic transcript checks. | Correlation, denial, replay, expiry, cancellation, restart, peer fault, and reclamation are observed end to end. |
| Regression repair | Split generated stream, call, and operation control profiles. | Adding request/response participants no longer shifts stream supervision slots or breaks `fabric_stream_check`. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Cross-correlation, authority widening, replay, backpressure loss, or unbounded state | `just fabric_operation_check` | Missing/out-of-order operation transcript marker, participant failure, timeout, or non-zero kernel exit. |
| Generated contract or generation drift | `just contracts_check` | Stale generated binding, rejected manifest, or failed boot-contract test. |
| Existing kernel behavior regression | `just test` | QEMU unit or integration test failure. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just fabric_operation_check` | Passed: focused operation scenario, deterministic generation build, unrelated stream/call profile boots, and healthy reclamation. | Direct |
| `just test` | Passed: kernel unit and QEMU integration suite. | Direct |
| `just lint` | Passed with warnings denied. | Direct |
| `just lint_components` | Passed with warnings denied. | Direct |
| `just fmt_check` | Passed. | Direct |
| `just fmt_check_components` | Passed. | Direct |

## Decisions

- Decision: keep the operation broker in userspace and application goal policy outside the fabric.
- Rationale: the kernel remains policy-free while the fabric supplies only transport correlation, authority, bounds, and outcomes.
- Rejected alternative: implicit kernel time, ambient task identities, or unbounded retry/history queues; each would violate deterministic generation authority or bounded operation state.

## Open risks and follow-ups

- [ ] C8.8 will add filtered introspection and declared interposition; C8.7 exposes no introspection authority.
- [ ] Cross-plane liveness is guarded by successful post-fault stream and call profile boots because the current generated profiles intentionally give one `fabric-service` instance mutually exclusive control-slot layouts.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none.
- Serial/debugger/model output: observed through `just fabric_operation_check` and `just test`; no frozen sibling capture retained.
- Related roadmap item: [C8.7](../../roadmap/02-core-runtime.md#c87--native-operations).
