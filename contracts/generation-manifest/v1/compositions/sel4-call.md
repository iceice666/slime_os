# `sel4-call.zti` — bounded native call composition

The [`sel4-call.zti`](sel4-call.zti) manifest owns `bootAction = "call"`, the
`parameters` route (`ParameterCall`), two clients, a server, and a separately
named clock. The [fabric architecture](../../../../docs/architecture/typed-data-fabric.md)
owns the shared protocol; this note records the local authority and budget shape.

## Controls and supervision

Four declared native control Endpoints connect the participants to the service
at slots 2–5, in client, client B, server, clock order. The
[build resolver](../../../../scripts/build/generation_fabric.py) derives identities
from those grant names and checks their declared slots. Init neither mints these
controls nor authenticates a caller from an identity supplied in its message.

The [launcher](../../../../components/system/init/src/fabric_planes.rs) starts
both clients, the server, and the clock before the broker. It supplies the broker
with its factory at slot 1 and supervision handles at slots 6–9, including the
clock's own handle. This is a spawn-time narrowed copy into declared minted
bindings, not post-spawn handle transfer over a control channel. Native Endpoints
cannot substitute for the handles when observing peer death.

Init's budget is five live children. The service and three route participant
executable grants carry transfer authority; the clock executable does not.
[Spawn admission](../../../../slime-root/src/graph_runtime/services/spawn.rs)
uses that bit to determine returned-handle transferability. A non-transferable
handle can still supply the declared narrowed supervision copy at broker spawn.

## Budgets and verification boundary

The service has 24 shared-buffer pages and 12 buffers/mappings/loans; client A
and the server each have four pages and two of each. Init grants the allocating
participants their factories. Client B and the clock have no shared-buffer quota.
The service also has 16 private-memory pages. The graph bounds in-flight calls
at four, ingress sources at four, retries at two, and capability slots at 32;
there is no operation route.

The [call checker](../../../../scripts/check/check-sel4-call-plane.py) owns the
executable acceptance criteria. Clock advances in this composition are simulated
protocol inputs, not physical latency measurements. No fresh gate run is claimed.
