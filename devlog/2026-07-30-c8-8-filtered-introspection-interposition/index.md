# C8.8 — Filtered introspection and declared interposition

| Field | Value |
|---|---|
| Date | 2026-07-30 |
| Kind | Change |
| Status | Verified |
| Scope | Fabric visibility contract, generated profile, userspace broker and participants, generation graph, bootstrap health policy, and QEMU gate |
| Roadmap | C8.8 |
| Gates | `just fabric_visibility_check`, `just test`, `just fmt_check`, `just fmt_check_components`, `just lint`, `just lint_components` |
| Trigger | C8.8 implementation |
| Baseline | C8.7 provided bounded streams, calls, and operations, while graph visibility and interposition existed only as admitted manifest metadata and did not affect the live userspace path. |

## Summary

C8.8 turns the authenticated fabric graph's visibility and interposition declarations into live userspace behavior. Introspection is a cursor-paged, read-only service over authenticated control endpoints: graph, private, and absent grants produce distinct bounded views without moving a capability, and an absent grant receives a graph-independent terminal record. The declared telemetry interposer receives only non-transferable upstream receive/ack and downstream send/ack roles; the fabric moves away its only direct downstream endpoint, turns proxy loss before or after relay into persisted route-scoped event metadata, and continues an unrelated diagnostics route. The focused gate runs two identical boots plus an injected early-proxy-death boot, compares deterministic records, and keeps the generation healthy in all three.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Contract | Added a versioned Zutai fabric-visibility contract for 64-byte requests, route pages, complete QoS pages, and interposition traces, with generated Rust bindings and validators. | Every cross-component metadata and trace record is bounded, deterministic, and schema-owned. |
| Generation profile | Derived participant visibility and non-empty interposition chains from the same manifest fixture that builds the authenticated graph resource. | Userspace policy cannot drift to an ambient registry or a second hand-authored graph. |
| Introspection | Added deterministic cursor paging filtered by the caller identity bound to its control endpoint. | Names, schema identities, match state, QoS, and event metadata reveal only the caller's exact declared view and never carry authority. |
| Interposition | Compiled `fabric-intruder` as the sole telemetry hop to `fabric-subscriber`, with exact upstream/downstream data and acknowledgement roles and no transfer bit. | Publisher and subscriber have no direct bypass, and the proxy cannot act outside its declared chain. |
| Failure isolation | Converts proxy loss before send, while awaiting acknowledgement/trace, or after relay into a route-scoped loss trace and persisted view event, then completes a direct diagnostics exchange. | Proxy failure terminates only the affected route path, not the fabric or an unrelated route. |
| Gate | Added three QEMU boots, generated-binding checks, protocol tests, source authority lint, deterministic normal-run comparison, and an injected early-proxy-death scenario. | Authority, filtering, failure isolation, early fault handling, and byte determinism fail with focused signals. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Protected graph metadata leaks through counts, names, types, match state, QoS, events, or error detail | `just fabric_visibility_check` | Wrong graph/private view count, a non-empty absent-grant response, malformed record, or missing QEMU marker. |
| A direct edge bypasses the declared proxy or the proxy widens/retransfers a role | `just fabric_visibility_check` | Consumed source slot remains usable, a wrong-direction syscall succeeds, a retransfer succeeds, or the relay trace is absent. |
| Proxy loss before or after relay terminates unrelated graph state | `just fabric_visibility_check` | Missing route-loss event/view metadata, diagnostics delivery/ack failure, fabric failure marker, or unhealthy kernel exit. |
| Generated bindings or trace order drift | `just fabric_visibility_check` | Stale binding or byte inequality between the two identical boots. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just fabric_visibility_check` | Passed: contracts and generated bindings were current; three protocol tests passed; two generation builds were byte-identical; two identical QEMU boots produced byte-identical 12-record views and two-record traces; an injected early-proxy-death boot produced one loss trace; every boot left diagnostics and the generation healthy. | Direct |
| `just test` | Passed the full kernel and QEMU test suite after the final implementation change. | Direct |
| `just fmt_check` and `just fmt_check_components` | Passed after formatting the kernel and component workspaces. | Direct |
| `just lint` and `just lint_components` | Passed with warnings denied after introducing a typed visibility-client error. | Direct |
| `cargo check --target x86_64-unknown-none -p slime-components` with `SLIME_FABRIC_VISIBILITY_CHECK=1` | Passed after the live profile was wired. | Direct |

## Decisions

- Decision: page one 64-byte record per request and separate route identity from complete QoS metadata.
- Rationale: the graph can reach its admitted route ceiling without depending on channel queue depth, allocating a graph-sized response, truncating a full schema identity, or dropping deadline/lifespan/lease fields.
- Rejected alternative: serialize the whole graph into a shared buffer; it would add mapping and loan authority to a read-only metadata service and make an absent caller's allocation behavior another side channel.
- Decision: authenticate visibility and trace senders by their generation-provisioned control endpoints.
- Rationale: caller-supplied names and component identities are metadata, while endpoint possession is the existing unforgeable participant binding.
- Rejected alternative: add discover-by-name or a generic graph registry; either would make observed metadata a path to authority.
- Decision: move the downstream send endpoint to the declared proxy and consume the fabric's source slot.
- Rationale: the absence of a bypass is then a kernel-enforced capability fact rather than a broker branch that promises not to use a retained endpoint.
- Rejected alternative: retain a dormant direct edge for failover; undeclared failover is a bypass and weakens the interposition contract.

## Open risks and follow-ups

- [ ] C8.9 must compose filtered introspection and an interposed route with the full stream, call, and operation graph and its broader fault corpus.
- [ ] The current live profile exercises one-hop stream interposition. The authenticated decoder already admits bounded acyclic chains; a future declared multi-hop or call gateway must receive its own end-to-end gate before being claimed live.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none.
- Serial/debugger/model output: observed through `just fabric_visibility_check`; no frozen sibling capture retained.
- Related roadmap item: [C8.8](../../../roadmap/02-core-runtime.md#c88--filtered-introspection-and-declared-interposition).
