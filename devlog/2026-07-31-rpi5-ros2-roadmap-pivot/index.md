# RPi5 ROS 2 two-node roadmap pivot

| Field | Value |
|---|---|
| Date | 2026-07-31 |
| Kind | Decision |
| Status | Proposed |
| Scope | roadmap index, RPi5 demo track, ROS 2 compatibility, architecture portability, Framework hardware deferral |
| Roadmap | RP0, RP1, RP2, RP3, RP4, RP5, RP6, RP7, RP8, R0, P0, P1, P2, P4 |
| Gates | none |
| Trigger | Project goal changed to running two ROS 2 nodes exchanging data on Raspberry Pi 5 |
| Baseline | Roadmap previously led with x86-64 QEMU as the deterministic reference, Framework hardware qualification, external ROS wire compatibility, and later AArch64 replay |

## Summary

The near-term roadmap now centers on a concrete robotics acceptance test: Slime OS must boot on Raspberry Pi 5 and run two local ROS 2 nodes that exchange bounded topic data through a minimal DDSI-RTPS/XCDR profile. The completed x86-64/QEMU and Framework work remains preserved as regression and historical evidence, but new milestone sequencing prioritizes target-qualified AArch64 artifacts, AArch64 QEMU bring-up, Raspberry Pi 5 physical boot, Arm component data-path replay, the minimal DDS/RTPS topic profile, the ROS 2 node runtime envelope, and a physical DDS-backed two-node data-transfer demo.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Roadmap index | Replaced the previous parallel-lane summary with demo-first sequencing and a new RPi5 ROS 2 release gate. | Milestone order now follows the current product acceptance test rather than historical x86/Framework priorities. |
| RPi5 demo track | Added `roadmap/09-rpi5-ros2-demo.md` with RP0–RP8 milestones from demo contract through repeatability/fault hardening. | The Raspberry Pi 5 ROS 2 claim has explicit dependencies, deliverables, checks, and physical evidence requirements. |
| ROS 2 track | Reframed ROS compatibility around R0 minimal DDS/RTPS topic behavior before broader external/multi-vendor R1/R2 and existing-workload R3. | The first ROS milestone now includes the DDS layer needed to call the result ROS 2 without overclaiming full ROS/DDS support. |
| Architecture portability | Made AArch64/RPi5 the near-term physical path while retaining x86-64 as regression evidence and deferring RV64. | Target identity remains exact and executable artifacts remain profile-qualified. |
| Framework hardware | Marked Framework daily-driver work as deferred relative to the RPi5 demo and clarified that H6 is only needed for later external ROS wire compatibility. | Physical claims stay tied to the named platform that produced the evidence. |

## Decisions

- Decision: The near-term release target is the RPi5 ROS 2 two-node demo, not generic Arm support or Framework daily-driver progress.
- Rationale: A concrete board/workload/data-transfer acceptance test gives the project a narrower path and prevents architecture bring-up from being declared complete at “boots on QEMU”.
- Rejected alternative: Keep the old ordering where ROS wire compatibility and Framework hardware remain parallel first-class lanes and Raspberry Pi appears only as a later heterogeneous peer or physical replay.

- Decision: Introduce R0 as a minimal DDSI-RTPS/XCDR topic profile before R1/R2 broader external and multi-vendor interoperability.
- Rationale: The requested demo is two ROS 2 nodes on Raspberry Pi 5, and ROS 2's normal communication boundary is DDS/RMW. R0 keeps DDS on the critical path while avoiding the larger Fast DDS/Cyclone DDS external-peer matrix until the board-local demo is stable.
- Rejected alternative: Treat a local-only `rmw_slime`/C8 route as enough for the first ROS 2 claim, which would make the demo faster but ambiguous about whether it is actually exercising ROS 2's DDS/RMW model.

- Decision: Preserve completed evidence but explicitly defer Framework, RV64, foreign workloads, broad authority, and native-development tracks unless they de-risk RP0–RP8.
- Rationale: Completed history should remain searchable and regression-useful, while future work should not compete with the stated RPi5 demo goal.
- Rejected alternative: Delete or rewrite old tracks as if their observed results never existed.

## Open risks and follow-ups

- [ ] RP0 must pin the exact minimal DDS/RMW boundary: DDSI-RTPS version, XCDR representation, discovery mode, participants, locators, QoS subset, and whether the node code is Slime-native, source-compatible, or Linux-personality-backed.
- [ ] New `just rpi5_*` and `just rpi5_ros2_dds_*` target names are roadmap placeholders; they need implementation before any RP milestone can be claimed complete.
- [ ] The current codebase still appears x86-64-first; P0/P1/P2/RP1/RP2 must establish target-qualified artifacts and AArch64 boot before the board demo is credible.
- [ ] The roadmap now defines R0, but no DDSI-RTPS/XCDR runtime, ROS 2 runtime, or node API has been implemented by this decision.

## Artifacts and provenance

- Focused report: none.
- Raw transcript: none.
- Serial/debugger/model output: none.
- Related roadmap item: `roadmap/09-rpi5-ros2-demo.md`, `roadmap/03-ros2-compatibility.md`, `roadmap/07-architecture-portability.md`, `roadmap/README.md`.
