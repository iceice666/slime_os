# `sel4-visibility.zti` — filtered introspection composition

The [`sel4-visibility.zti`](sel4-visibility.zti) manifest owns
`bootAction = "visibility"`, telemetry and diagnostics, and a profile override
placing `fabric-intruder` on the telemetry subscriber's interposition chain.
The [build resolver](../../../../scripts/build/generation_fabric.py) applies the
profile's chain to the authenticated graph; it is not a participant's self-claim.
[Graph decoding](../../../../boot-contracts/src/fabric_graph.rs) rejects cycles
and self-hops; [root admission](../../../../slime-root/src/generation.rs) requires
each hop to name a declared component.

## Static endpoint authority

Controls and route Endpoints are ordinary declared grants with explicit instance
bindings. The [visibility broker](../../../../components/lib/src/visibility_broker.rs)
resolves route roles by declared names such as `telemetry-ingress` and
`telemetry-proxy-upstream`; init does not create or transfer endpoint pairs.
The proxy holds upstream receive/ack-send and downstream send/ack-receive roles.
Telemetry sample delivery traverses the proxy. A separate direct
`telemetry-proxy-event` endpoint reports events to the subscriber; it is not a
bypass for telemetry samples. Diagnostics has its own ingress, egress, and ack
edges and does not depend on the proxy.

All init executable grants are non-transferable. The
[current launcher](../../../../components/system/init/src/fabric_planes.rs)
nevertheless supplies narrowed supervision copies at service spawn: publisher,
subscriber, proxy, publisher B, subscriber B occupy slots 7–11. The proxy and
subscriber handles expose termination; an Endpoint cannot report peer death.
The service also receives its factory, and publisher B has a declared factory
binding. Scenario selection in broker and participants uses authenticated
`BootAction::Visibility`, not an environment flag.

## Budgets and visibility boundary

Init has `spawnBudget = 6`. The service alone has a shared-buffer quota of 28
pages and 14 buffers/mappings/loans, plus 16 private-memory pages. The visibility
protocol uses inline records; the shared-buffer figures are declared admission
budgets, not a measurement. The graph declares seven ingress sources and 32
capability slots; the builder's stream wait shape adds one fixed source, so the
ingress count must not be confused with the worker's complete park set.

The broker reads the graph as its declared fabric holder and filters views for
requesting participants. It does not grant other participants holder-wide graph
access. The [visibility checker](../../../../scripts/check/check-sel4-visibility-plane.py)
owns acceptance for filtered introspection, interposition, and proxy-loss
behavior. No fresh execution result is claimed.
