# RP0 — Raspberry Pi 5 ROS 2 demo contract

| Field | Value |
|---|---|
| Date | 2026-08-02 |
| Kind | Change |
| Status | Verified |
| Scope | RPi5 ROS 2 demo contract, Zutai fixtures, semantic admission checks, contract registry, roadmap |
| Roadmap | RP0 |
| Gates | `just rpi5_ros2_demo_contract_check`, `just contracts_check` |
| Trigger | RP0 implementation |
| Baseline | The RPi5 demo track named a physical DDS-backed two-node outcome but had no versioned target-qualified acceptance contract or executable admission gate. |

## Summary

RP0 freezes the first Raspberry Pi 5 ROS 2 two-node demo as a bounded, versioned activation contract. It pins the admitted board revisions, firmware and removable-media path, UART and device-tree hardware boundary, ROS 2 Jazzy route, minimal DDSI-RTPS/XCDR profile, exact two-node workload, capability inventory, semantic trace ordering, resource ceilings, and distinct operator markers. The focused and aggregate contract gates pass; no physical-board, AArch64 runtime, or DDS implementation claim is made by this milestone.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Contract | Added the RP0 Zutai schema plus valid and structurally invalid fixtures. | Every cross-boundary field has one versioned source of truth and closed record shape. |
| Target and workload | Pinned Raspberry Pi 5 revision codes, firmware/media/device paths, ROS 2 Jazzy route, DDSI-RTPS 2.5 plain XCDR1 CDR profile, endpoint ids, locators, QoS, four samples, and subscriber output. | Later milestones implement one exact target rather than selecting a nearby board, profile, or workload. |
| Authority and bounds | Declared exact node, DDS runtime, board-service, storage, datagram, clock, log, participant, and trace capabilities with finite payload, sequence, queue, history, retry, trace, and log ceilings. | Activation has no wildcard authority or unbounded resource dimension. |
| Evidence | Declared an ordered semantic/DDS success trace and distinct success, denial, timeout, wrong-target, malformed-wire, and malformed-generation markers. | Serial and trace evidence can distinguish successful completion from every admitted failure class. |
| Verification | Added semantic checks, one-delta rejection cases, byte-distinct equivalent-source projection comparison, aggregate contract registration, and a focused Just target. | Schema drift, nearby identifiers, inconsistent bounds, trace reordering, and authority widening fail closed. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| RP0 target, profile, workload, bound, authority, or trace drift | `just rpi5_ros2_demo_contract_check` | Structural decode, exact-value, relation, determinism, or rejection-corpus failure. |
| Contract registry or generated-binding regression | `just contracts_check` | Aggregate contract, binding, boot-model, or layout-resource failure. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just rpi5_ros2_demo_contract_check` | Passed; the valid fixture admitted and the structural and semantic rejection corpus completed. | Direct |
| `just contracts_check` | Passed; BootState model checking, RP0 semantic admission, 59 boot-contract tests, binding freshness, and boot-layout resource checks completed. | Direct |
| `just ruff` | Passed. | Direct |
| `just typos` | Passed. | Direct |
| `just fmt_check_all` | Passed. | Direct |
| `just lint_all` | Passed. | Direct |

## Decisions

- Decision: Admit Raspberry Pi 5 Model B revision codes `c04170` and `d04170` under one `bcm2712c1` Profile 0 target.
- Rationale: The accepted set covers the pinned supported revisions while preserving one explicit SoC/profile boundary; unknown revisions still fail closed.
- Rejected alternative: Accept every Raspberry Pi 5 revision or infer a nearby profile from model strings, which would weaken target qualification.
- Decision: Use static generation-declared DDS discovery, plain little-endian XCDR1 CDR, fixed loopback locators, and one keyless writer/reader pair.
- Rationale: This is the minimum DDS-backed route that remains bounded and gives RP1–RP8 an exact interoperability target.
- Rejected alternative: Dynamic discovery, multicast, DDS Security, or a DDS-free local RMW shortcut; each expands scope beyond RP0 or bypasses the roadmap boundary.

## Open risks and follow-ups

- [ ] RP1 must bind every executable and authenticated generation object to the exact `aarch64-rpi5` profile before mapping bytes.
- [ ] RP2–RP7 must implement and observe the declared AArch64, board, DDS, workload, trace, and operator-marker behavior; RP0 proves only host-side contract admission.
- [ ] Physical Raspberry Pi 5 support remains unproven until the removable-media run and serial/DDS evidence required by RP7 are observed.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none retained.
- Serial/debugger/model output: no physical or guest serial evidence is claimed; host-side results were observed through the listed gates.
- Related roadmap item: [`RP0`](../../roadmap/09-rpi5-ros2-demo.md#rp0--demo-contract-and-acceptance-fixture).
