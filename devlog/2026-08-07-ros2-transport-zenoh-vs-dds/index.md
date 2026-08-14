# ROS 2 transport: self-built DDSI-RTPS/XCDR vs Zenoh

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Decision |
| Status | Proposed |
| Scope | ROS 2 compatibility roadmap (R0/R1 transport choice), core runtime roadmap (C9 cross-reference), rpi5-ros2-roadmap-pivot open-risk cross-link |
| Roadmap | R0, R1, R2, C9, X1, P5 |
| Gates | none |
| Trigger | Architecture review comparing a self-built rmw/DDS transport against Eclipse Zenoh, prompted by externally sourced material describing Zenoh/zenoh-pico paired with seL4 in robotics contexts |
| Baseline | `roadmap/03-ros2-compatibility.md` already specified R0-R3 as a self-built DDSI-RTPS/XCDR profile without recording why Zenoh was not selected; `devlog/2026-07-31-rpi5-ros2-roadmap-pivot/index.md` left "whether the node code is Slime-native, source-compatible, or Linux-personality-backed" open; `roadmap/02-core-runtime.md` C9 was cited by R2 as a dependency without stating what R2 would consume it for |

## Summary

Three transport options for the RPi5 two-node ROS 2 demo's native middleware layer were compared: (1) the roadmap's existing self-built DDSI-RTPS/XCDR profile (R0/R1), (2) a self-built bounded Zenoh wire-protocol subset implemented as a native `no_std` component, and (3) running the official Zenoh runtime or `rmw_zenoh` under the X1 Linux personality route. Self-built DDSI-RTPS/XCDR remains the R0/R1 path: it is the only option that simultaneously satisfies the capability-based authority boundary, the Zutai-only wire-schema rule, R1's stated interoperability target (pinned Fast DDS/Cyclone DDS peers), and the deterministic bounded-resource verification R0 already requires. Slime's userspace components are `no_std` and capability-gated rather than POSIX (confirmed: every `components/bins/src/bin/*.rs` and `components/bins/src/lib.rs` carries `#![no_std]`), so framing the question as "Zenoh in userspace" does not change this: the official `zenoh` crate assumes `std`/`tokio` and cannot be hosted as a native component regardless of kernel vs. userspace placement. The only route that runs genuine upstream Zenoh code is X1, which is architecturally sound but belongs to R3's existing-workload scope, not R0/R1's native-component demo. Separately, this entry records that C9 ("Robot runtime authority") is the intended substrate for ROS 2 Lifecycle-Node and parameter-service compatibility once R2 is built, rather than a bespoke ROS-specific reimplementation.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `roadmap/03-ros2-compatibility.md` R0 | Added a cross-reference recording that DDSI-RTPS/XCDR was chosen over a self-built Zenoh subset and over an X1-hosted official Zenoh/`rmw_zenoh` route | The transport choice is traceable to a rationale, not just stated as a fact |
| `roadmap/03-ros2-compatibility.md` sequencing (R2) | Expanded the one-line C9 dependency note to state that ROS 2 managed-node and parameter services are expected to map onto C8 `Call<Request, Reply>` routes backed by C9 schemas | R2's C9 dependency states what it is for, not just that it exists |
| `roadmap/02-core-runtime.md` C9 | Added a cross-reference stating C9's lifecycle-transition and parameter-state schemas are the expected substrate for R2's ROS 2 compatibility profile | C9's generic scope and its ROS 2 consumer stay linked without duplicating either description |
| `devlog/2026-07-31-rpi5-ros2-roadmap-pivot/index.md` open risks | Narrowed the "Slime-native, source-compatible, or Linux-personality-backed" sub-question of the open DDS/RMW boundary risk and linked it to this entry, without checking off the remaining wire-parameter items | Open risk list reflects what is actually still unresolved |

## Decisions

- Decision: Keep R0/R1's transport as self-built DDSI-RTPS/XCDR; do not adopt Zenoh in any form for the native RPi5 two-node demo.
- Rationale: It is the only option that simultaneously satisfies (a) the Authority boundary section's requirement that writers/readers map to C8 `Stream<T>` endpoints without ambient sockets or discovery, (b) the "Zutai is the only schema language" rule for wire formats crossing a process boundary, (c) R1's already-stated interoperability target of pinned Fast DDS/Cyclone DDS peers, and (d) the fail-before-allocation deterministic bounds R0 already requires for malformed input.
- Rejected alternative: A self-built bounded Zenoh wire-protocol subset as a native `no_std` component. This carries the same engineering cost and owned-bug-surface class as self-built DDSI-RTPS (still a hand-written wire codec with no upstream leverage), but does not by itself reach R1's Fast DDS/Cyclone DDS interoperability goal — it would need an additional bridge component (functionally a second protocol implementation) and would redefine R1's peer target rather than reach it directly.
- Rejected alternative: Running the official Zenoh runtime or `rmw_zenoh` under the X1 Linux personality, with its syscalls translated through explicit generation grants. Rejected for R0/R1 because X1 depends on the completed M5.4 content-addressed store, M6 spawn, and H6 network service — a materially larger dependency chain than the native demo needs — and because `roadmap/03-ros2-compatibility.md` already states R1/R2 do not depend on X1, X2, Linux, or existing ROS binaries. It also does not demonstrate what R0 exists to demonstrate: that two *native* Slime components cross a real DDS/RMW boundary, since an X1-hosted Zenoh peer is an external Linux workload, not a native C8 component. This route remains a legitimate future candidate for R3 (existing-workload route) and should be evaluated there, not substituted for R0/R1's native path.
- Decision: Record C9 ("Robot runtime authority") as the intended substrate for ROS 2 Lifecycle-Node (`change_state`/`get_state`) and parameter-service compatibility once R2 is built, implemented as C8 `Call<Request, Reply>` routes backed by C9's lifecycle-transition and parameter-state schemas.
- Rationale: C9 already commits to "component lifecycle transitions, health dependencies, bounded restart/backoff policy, and parameter state as versioned userspace schemas" for every Slime component, not ROS-specific ones. Treating ROS 2's managed-node and parameter surface as one profile over that generic mechanism avoids a duplicate state machine and keeps "lifecycle" meaning one thing across native and ROS-compatibility components.
- Rejected alternative: Implement ROS 2 lifecycle/parameter services as a bespoke subsystem scoped only to R2, independent of C9. Rejected because it would duplicate a mechanism C9 already commits to building and would fragment lifecycle semantics between native Slime components and ROS-compatibility components.

## Open risks and follow-ups

- [ ] The exact R0 DDS/RMW wire parameters (DDSI-RTPS version, XCDR representation, discovery mode, participants, locators, QoS subset) remain unpinned; this entry resolves only the transport-family sub-question of that open item from `devlog/2026-07-31-rpi5-ros2-roadmap-pivot/index.md`.
- [ ] If R1 later needs interoperability with `rmw_zenoh`-based peers specifically, rather than Fast DDS/Cyclone DDS, that changes R1's stated peer target and needs its own devlog Decision entry rather than reopening this one.
- [ ] C9 is Not started, and its C8 dependency (C8.10) is still open; no concrete work on the lifecycle/parameter mapping described above can proceed until both land.
- [ ] This entry is documentation-only; no `just` target verifies it. Verification arrives with R0, R2, and C9 implementation and their respective gates.

## Artifacts and provenance

- Focused report: none.
- Raw transcript: none.
- Serial/debugger/model output: none.
- Related roadmap item: `roadmap/03-ros2-compatibility.md` (R0, R1, R2, R3), `roadmap/02-core-runtime.md` (C9), `roadmap/05-foreign-workloads.md` (X1), `roadmap/07-architecture-portability.md` (P5), `devlog/2026-07-31-rpi5-ros2-roadmap-pivot/index.md`.
