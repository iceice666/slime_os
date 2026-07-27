# C8 — Native typed data fabric decomposition

| Field | Value |
|---|---|
| Date | 2026-07-27 |
| Kind | Decision |
| Status | Proposed |
| Scope | `roadmap/02-core-runtime.md`, C8 sequencing, contracts, authority, QoS, and planned verification gates |
| Roadmap | C8 |
| Gates | none |
| Trigger | C8 combined schema identity, capability provisioning, streams, QoS, calls, operations, visibility, and integration behind one planned gate |
| Baseline | C7 and B2 complete; C8 not started with one parent deliverable list and one planned `data_fabric_check` |

## Summary

C8 is decomposed into nine independently reviewable slices: deterministic interface schemas, generation graph admission, attenuated endpoint provisioning, streams, reliable/retained/timed QoS, calls, operations, filtered introspection/interposition, and full-graph integration. The decision fixes the authority and timing boundaries before implementation: graph and QoS policy remain in userspace, the kernel gains only generic bounded narrow-on-transfer capability movement, and timed QoS consumes an explicit capability-routed time input. Status remains Proposed; no C8 implementation or runtime behavior changed.

## Changes

| Area | Change | Established boundary |
|---|---|---|
| C8.1 | Full normalized Zutai schema identity and generated native contracts | Schema equivalence and collision handling precede graph authority |
| C8.2 | Authenticated graph/QoS resource with per-entry and aggregate admission | Every route, topology, visibility, and resource limit is generation data |
| C8.3 | Narrow-on-transfer mechanism and live fabric control plane | Clients receive exact non-transferable route roles; names grant nothing |
| C8.4 | Bounded many-to-many streams over inline IPC and C7 shared samples | Stream matching, eviction, fan-out, and reclamation close independently |
| C8.5 | Credit-based reliability, retained history, timed QoS, and structured events | Backpressure and time transitions stay bounded without busy-polling |
| C8.6 | Bounded request/reply correlation | Calls own exact terminal, duplicate, timeout, cancellation, and peer-loss behavior |
| C8.7 | Native operation transport | Goal, feedback, result, and cancellation routing remain separate from application and ROS policy |
| C8.8 | Filtered introspection and explicit proxy chains | Observation grants no authority and declared interposition has no bypass |
| C8.9 | Full QEMU graph, denial, fault, bound, and determinism corpus | The parent C8 exit condition remains an end-to-end claim |

## Decisions

- Decision: preserve C8 as the parent milestone and number the implementation slices C8.1–C8.9.
- Rationale: each slice owns one primary state or authority surface and a narrow planned verification target; C8.9 alone closes the parent exit condition.
- Decision: use a domain-separated SHA-256 digest of versioned normalized Zutai bytes as the authoritative `InterfaceSchema` identity.
- Rationale: equivalent schemas need one deterministic identity, while conflicting layouts must not rely on a short fingerprint. C7's existing 64-bit descriptor field remains a generation-local, collision-checked tag so C7 wire format stays stable.
- Decision: add bounded masked capability movement as the only new kernel mechanism required by the fabric control plane.
- Rationale: current endpoint creation yields bidirectional transferable capabilities, while C8 requires exact publisher/subscriber roles that clients cannot redelegate. Schema, graph, route, and QoS meanings remain userspace policy.
- Decision: broker large fan-out with one bounded copy into a fabric-owned sealed buffer followed by one receiver-bound C7 loan per subscriber.
- Rationale: C7 loans name one exact receiver; multi-receiver loans or ambient supervision duplication would widen kernel authority solely to avoid one userspace copy.
- Decision: drive C8 timed QoS through an explicit monotonic-time input and deterministic simulated-time corpus.
- Rationale: this makes deadline, lifespan, and liveliness semantics executable without ambient time or a C8/C9 dependency cycle; C9 later supplies the standard component-facing time services.
- Decision: admit no more than the existing eight `SYS_WAIT` ingress sources per initial fabric instance.
- Rationale: a graph that cannot register every live wake source would reintroduce busy polling or lost progress. The bound remains generation-visible and can change only after an observed profile need.
- Decision: implement bounded native operation transport in C8, but leave application goal policy and the ROS action state machine outside the fabric.
- Rejected alternative: keep one broad C8 implementation slice and add an internal checklist. A checklist does not independently prove capability attenuation, schema determinism, QoS bounds, call correlation, or visibility isolation.

## Open risks and follow-ups

- [ ] Add each planned `just` target with its implementation slice; none of the C8 targets exists yet.
- [ ] Define the masked capability-transfer descriptor in Zutai and prove that attenuation preserves existing IPC and C7 loan-transfer behavior.
- [ ] Validate the initial eight-ingress-source limit against the C8.9 graph before claiming the aggregate gate; expand or shard only with bounded evidence.
- [ ] C8 timed QoS has deterministic simulated-time evidence only until C9 supplies and verifies the standard monotonic-time service binding.
- [ ] The fabric's one-copy large-sample path needs aggregate page, mapping, and downstream-loan admission so a valid graph is satisfiable at every declared ceiling.

## Artifacts and provenance

- Related roadmap item: `roadmap/02-core-runtime.md` (C8, C8.1–C8.9)
- Completed sample-plane foundation: `roadmap/02-core-runtime.md` (C7)
- Completed wait-set prerequisite: `devlog/2026-07-24-b2-blocked-task-state/`
