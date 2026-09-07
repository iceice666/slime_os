# RPi5 ROS 2 two-node roadmap pivot

| Field | Value |
|---|---|
| Date | 2026-07-31 |
| Kind | Decision |
| Status | Proposed |
| Scope | roadmap index, RPi5 demo track, ROS 2 compatibility, architecture portability, Framework hardware deferral |
| Work items | 01a0724c-5400-701b-b2c7-d0a5b9533503, 01a0724c-5400-7715-bad2-72a76852749b, 01a01ac0-3800-727f-b113-f7ce726f08c6, 01a0724c-5400-7cb4-ab51-0bcb23a882b5, 01a0724c-5400-7019-a587-694c834f91cc, 01a0724c-5400-7b6b-ad3a-f388c30d5ed1, 01a0724c-5400-72f4-998b-6d7b21b59e0e, 01a0724c-5400-75e6-803a-7a56bd28c492, 01a0724c-5400-75db-8908-dffc253181e4, 01a00b4d-2400-7987-93d6-19201c397dff, 019fbe0d-c000-7ace-97cd-aae461a1248a, 019fbe0d-c000-7b72-a9c2-f9b8a43bd455, 019fe21a-4400-7fe0-881a-f308c0314bd1, 01a02f59-a800-78b6-9398-6b22bfbc6fdd |
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

- [ ] RP0 must pin the exact minimal DDS/RMW boundary: DDSI-RTPS version, XCDR representation, discovery mode, participants, locators, QoS subset. The transport-family sub-question (Slime-native DDSI-RTPS vs. Zenoh vs. Linux-personality-backed) is resolved by [`devlog/2026-08-07-ros2-transport-zenoh-vs-dds/`](../2026-08-07-ros2-transport-zenoh-vs-dds/index.md): self-built DDSI-RTPS/XCDR as a native component. The remaining wire-level parameters stay open.
- [ ] New `just rpi5_*` and `just rpi5_ros2_dds_*` target names are roadmap placeholders; they need implementation before any RP milestone can be claimed complete.
- [ ] The current codebase still appears x86-64-first; P0/P1/P2/RP1/RP2 must establish target-qualified artifacts and AArch64 boot before the board demo is credible.
- [ ] The roadmap now defines R0, but no DDSI-RTPS/XCDR runtime, ROS 2 runtime, or node API has been implemented by this decision.

## Artifacts and provenance

- Focused report: none.
- Raw transcript: none.
- Serial/debugger/model output: none.
- Related roadmap item: `roadmap/09-rpi5-ros2-demo.md`, `roadmap/03-ros2-compatibility.md`, `roadmap/07-architecture-portability.md`, `roadmap/README.md`.
