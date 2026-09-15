# `sel4-operation.zti` — bounded native operation composition

The [`sel4-operation.zti`](sel4-operation.zti) manifest owns
`bootAction = "operation"`, the `navigation` route, client A's private
`nav-backup` route, two clients, a declared replacement for client B, a server,
and a clock. Shared semantics belong to the
[fabric architecture](../../../../docs/architecture/typed-data-fabric.md).

## Declared controls and restart identity

The root installs native controls at service slots 2–5 for client A, client B,
the server, and the clock; the replacement control is slot 6 and the backup edge
is slot 7. The [build resolver](../../../../scripts/build/generation_fabric.py)
checks the control ordering and that replacement controls terminate at the same
holder. The replacement is a distinct declared instance, not an ambient task id.

The [operation broker](../../../../components/lib/src/operation_broker.rs) admits
the replacement only after client B's slot becomes vacant. It preserves that
client's authenticated index, correlation high-water mark, and retained results;
the replacement uses its own declared control and supervision capability. The
broker releases the replacement through its declared start barrier, not init.

The [launcher](../../../../components/system/init/src/fabric_planes.rs) starts
the clients, server, and replacement before the broker. It supplies the broker's
factory at slot 1 and participant supervision copies at slots 8–11. The clock
starts afterwards; its handle stays with init. Controls are not minted by init,
and supervision is provided at spawn, not introduced over those controls.

Init has `spawnBudget = 6`. Four participant executable grants are transferable;
the fabric and clock executable grants are not. The
[spawn owner](../../../../slime-root/src/graph_runtime/services/spawn.rs) checks
copy rights against the source and declaration independently of whether the
returned handle may later be transferred.

## Budgets and verification boundary

The graph declares four in-flight operations, four retained samples, event depth
eight, two retries, five ingress sources, and 32 capability slots; in-flight
calls are zero. The service alone has a shared-buffer quota: four pages and two
buffers/mappings/loans, plus 16 private-memory pages. The quota satisfies the
8192-byte maximal-sample declaration even though operation envelopes are inline;
it is not a measured allocation count. The
[build resolver](../../../../scripts/build/generation_fabric.py) separately owns
the worker wait-shape accounting, including the operation worker's nine-source
peak; an ingress limit is not the complete park-set size.

The [operation checker](../../../../scripts/check/check-sel4-operation-plane.py)
owns acceptance, including restart and authority isolation. The clock supplies
simulated time; this note makes no fresh execution or real-time qualification
claim.
