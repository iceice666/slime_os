# `sel4-visibility.zti` — the C8.8 filtered-introspection generation

A twelfth seL4 generation, beside [`sel4-stream.zti`](sel4-stream.md),
[`sel4-call.zti`](sel4-call.md), [`sel4-operation.zti`](sel4-operation.md), and
their siblings. It declares the C8.8 graph — the telemetry and diagnostics
routes with `fabric-intruder` interposed on the telemetry subscriber's chain —
for P5.4.8.

This is the **stream graph plus one declared interposition**. Its routes, QoS,
and participants are `sel4-stream.zti`'s; what differs is the profile, and the
composition `init` runs. Recorded below is only that difference.

## Why generation 21

18, 19, and 20 are the seL4 call, QoS, and operation planes. Generation 16 is
the **x86** visibility plane and keeps the base x86 layout. Numbering this 21
rather than reusing 16 is what stops the seL4 image from walking the x86 table.

## The interposition lives in the profile, not the participant

Every route participant declares `interposition = []`. The chain arrives from
the profile:

```
profiles = [ { name = "sel4"; interpositions = [
  { route = "telemetry"; participant = "fabric-subscriber"; chain = ["fabric-intruder";]; };
]; }; ];
```

`resolve_fabric_graph` (`scripts/build/build-generation.py`) rewrites the named
participant's `interposition` from that override, which is exactly how the
oracle's own `visibility` profile expresses it. Declaring the chain inline on the
participant would work, and would also make the fixture claim something the
oracle's does not: that the chain is a property of the route rather than of the
profile the boot selected.

Admission is fail-closed on both halves. `FabricGraph::decode`
(`boot-contracts/src/fabric_graph.rs`) requires every hop to resolve, the chain
to terminate, and no participant to hop to itself; `slime-root/src/generation.rs`
additionally requires every hop to name a component this generation declares. The
boot marker reports `interpositions=1`, and the gate asserts it — a profile whose
chain silently vanished would admit a graph with a direct edge where the
generation declared a proxy.

## `fabric-intruder` is the proxy here

The same binary that proves undeclared-edge denial on the stream plane is the
*declared proxy* on this one, selected by `SLIME_FABRIC_VISIBILITY_CHECK`. That
is the oracle's arrangement, kept rather than modernized: C8.10's `fabric-proxy`
is a later, distinct identity, and porting it would be porting the unified plane
rather than C8.8.

## Static endpoint authority

Every `init-*` executable grant declares `transferable = false`. Control and
route endpoints are generation-declared objects installed through
`mintedBindings`: the fabric holds slots 2–13, while each participant holds only
the fixed control and route roles its authenticated descriptor describes.

The proxy chain is therefore attenuated by construction. `fabric-intruder`
holds receive/send/send/receive at slots 1–4 for upstream data, upstream ack,
downstream data, and downstream ack; no direct fabric-to-subscriber telemetry
binding exists. Diagnostics uses a separate service/publisher/subscriber triple,
so proxy loss cannot remove that unrelated route. No endpoint factory or runtime
capability handoff participates in visibility provisioning.

## The shared-buffer budget

One holder: `fabric-service`, at 28 pages and 14 buffers/mappings/loans. Those
are the graph's declared `limits`, which `resolve_fabric_profile` checks against
the holder quota. Nothing in this plane allocates — every record is an inline
64-byte `WireVisibility*` or `WireStreamSample` — so the figures are a
satisfiability floor rather than a measurement, and no participant declares a
quota.

## `ingressSources = 7`

Five telemetry participants and two diagnostics participants are seven
publish/subscribe edges; the `stream` route worker's `graphDerived` count adds
its fixed one, giving eight against `MAX_WAIT_SOURCES = 9`. The declared limit
counts the edges rather than the worker's park set, which is why it is 7 and not
8.

## The boot layout

`scripts/build/boot_layout.py`'s `SEL4_VISIBILITY_LAYOUT` records the generation
objects. Native controls and route endpoints are declared in the fixture and
installed into the fixed slots named by `mintedBindings`; component startup does
not construct or transfer endpoint pairs at runtime.
