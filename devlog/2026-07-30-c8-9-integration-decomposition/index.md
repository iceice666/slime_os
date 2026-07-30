# C8.9–C8.15 — Full-graph fabric integration decomposition

| Field | Value |
|---|---|
| Date | 2026-07-30 |
| Kind | Decision |
| Status | Proposed |
| Scope | `roadmap/02-core-runtime.md`, C8 full-graph profiles, bootstrap topology, deterministic traces, authority matrix, resource accounting, and aggregate verification |
| Roadmap | C8, C8.9, C8.10, C8.11, C8.12, C8.13, C8.14, C8.15 |
| Gates | none |
| Trigger | C8.8 completion exposed that the original C8.9 required several mutually exclusive runtime planes, unlinked resource declarations, new evidence contracts, and the parent aggregate gate in one review unit |
| Baseline | C8.1–C8.8 complete under independent profiles; stream/QoS, call, operation, and visibility/interposition cannot coexist in one boot because bootstrap slots and broker dispatch are mutually exclusive |

## Summary

The former C8.9 full-graph milestone is split into seven independently reviewable slices. C8.9 first closes the typed generation profile and runtime-bound contract; C8.10 establishes a collision-free simultaneous topology; C8.11 adds one simulated-time order and a versioned semantic trace; C8.12 exercises matching, visibility, interposition, and denial; C8.13 proves concurrent cross-plane traffic and every resource ceiling; C8.14 proves degradation and fault isolation; C8.15 owns repeated-boot determinism and closes the parent C8 exit condition. This is a documentation-only design decision: no runtime behavior or completion claim changed.

## Changes

| Area | Change | Established boundary |
|---|---|---|
| C8.9 | Typed full-profile and resource-bound closure | Host, kernel, and userspace consume one canonical graph/profile and reject unsatisfiable queue, buffer, mapping, loan, and capability limits before launch |
| C8.10 | Collision-free full-graph bootstrap and bounded route workers | All C8 roles coexist without slot aliases, over-sized wait sets, polling, or conflated proxy/probe identities |
| C8.11 | Unified simulated time and deterministic semantic traces | Cross-plane order and evidence become bounded, schema-owned, and independent of scheduler/serial interleaving |
| C8.12 | Integrated matching, visibility, and denial matrix | Exact name/type/kind/QoS matching and complete unauthorized-operation denial close independently from traffic saturation |
| C8.13 | Concurrent cross-plane traffic and resource ceilings | Simultaneous workloads measure every declared high-water and baseline without one route class consuming another's allowance |
| C8.14 | Degradation and fault isolation | Injected stalls and faults stay bounded, distinguishable, and fully reclaimed without disturbing unrelated route classes |
| C8.15 | Full-graph determinism and parent close | One repeated-boot QEMU gate composes every prior slice and alone closes C8 |

## Decisions

- Decision: preserve completed C8.1–C8.8 and replace the unstarted aggregate C8.9 with C8.9–C8.15 rather than renumbering history.
- Rationale: each new slice owns one primary failure surface and one planned gate while retaining a single final parent close.
- Decision: make typed profile/resource closure the first slice.
- Rationale: the current fixture carries load-bearing `fabricGraph.profiles` and `sharedBufferBudget` fields outside the declared generation type, Python and Rust resolve profiles independently, and several graph ceilings are not linked to runtime enforcement. Building a combined topology on that split authority would compound drift.
- Decision: require a collision-free topology before integrating workload semantics.
- Rationale: generations 13–16 overwrite the same init slots and every broker is selected by an early-return branch. Enabling several legacy flags cannot produce one graph and would create precedence-dependent authority.
- Decision: use bounded route workers or an equivalent explicitly scheduled shape, not a larger polling loop.
- Rationale: the admitted graph already reaches the `SYS_WAIT` ingress bound; one monolithic wait set cannot safely add acknowledgements, time, supervision, and proxy sources.
- Decision: define deterministic evidence as versioned semantic records, not whole serial output or prose-marker order.
- Rationale: concurrent scheduling legitimately changes serial interleaving, while route/correlation identities, statuses, events, simulated timestamps, and resource counts must remain byte-stable.
- Decision: separate concurrent resource saturation (C8.13) from degradation and fault injection (C8.14).
- Rationale: a ceiling proof needs a healthy graph at peak load, while a fault proof needs an unhealthy graph plus unaffected-route witnesses. Combining them would restore a review unit spanning eleven resource classes and twelve fault paths at once — the breadth this decomposition exists to remove.
- Rejected alternative: keep one C8.9 milestone with an internal checklist. The required changes span schema authority, capability topology, scheduling, wire evidence, denial semantics, resource accounting, fault injection, and final determinism; one checklist would repeat the broad review unit that this decomposition is intended to remove.

## Open risks and follow-ups

- [ ] C8.9 must formalize the existing profile/budget data without introducing a second manifest or hand-written wire format.
- [ ] C8.10 must demonstrate a simultaneous layout within `MAX_CAPS` and per-worker `MAX_WAIT_SOURCES`; raising kernel ceilings is not a substitute for topology design.
- [ ] C8.11 must bound mandatory trace retention under backpressure and preserve every terminal outcome.
- [ ] C8.12 needs distinct proxy, introspection, and unauthorized-probe identities plus live incompatible and non-aliasing routes.
- [ ] C8.13 must reconcile graph resource limits with the fabric holder's shared-buffer quota rather than assuming the two agree.
- [ ] C8.14 must measure post-fault return to baseline rather than relying on completion markers, and must keep every injected fault terminating within a bound.
- [ ] C8.15 alone may mark C8 complete after `just data_fabric_check` observes the full single-boot exit condition.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none.
- Serial/debugger/model output: none; documentation-only design change based on source inspection of the completed C8.5–C8.8 implementations.
- Related roadmap item: [C8.9–C8.15](../../roadmap/02-core-runtime.md#c89--typed-full-profile-and-resource-bound-closure).
