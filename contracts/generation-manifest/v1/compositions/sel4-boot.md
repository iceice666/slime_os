# `sel4-boot.zti` — full-graph boot composition

The [`sel4-boot.zti`](sel4-boot.zti) manifest owns `bootAction = "boot"`, twenty
component instances, five routes, and four interface schemas. Stream, call, and
operation workers coexist with a separate unauthorized probe, declared proxy,
and filtered-introspection observer. Shared semantics belong to the
[fabric architecture](../../../../docs/architecture/typed-data-fabric.md).

## Controls and worker authority

The `unified` profile explicitly declares its seven ordered `streamControls`.
The [build resolver](../../../../scripts/build/generation_fabric.py) reads that
list and the grants' holders; profile-name spelling is not the switch selecting
a hidden control table. Each worker has its own capability table, so equal slot
numbers across workers do not alias authority.

The root installs declared native control Endpoints into their bound instances.
Init does not mint or place controls. The
[current launcher](../../../../components/system/init/src/fabric_planes.rs)
spawns all nineteen children, including the call and operation workers; init has
`spawnBudget = 19`, while the service and workers have zero. The worker executable
grants are sourced from init and target their respective worker identities.

Handle subjects must exist before the broker receiving their supervision copies.
Init therefore starts stream participants and proxy before the service, call
participants and clock before the call worker, and operation participants before
the operation worker. The service receives its factory plus six supervision
copies at slots 9–14; the call worker receives its factory plus four at slots
6–9; the operation worker receives four at slots 8–11. The operation clock's
handle remains with init. All sixteen participant executable grants are
transferable; service and worker executables are not. Spawn-time narrowed copies
are distinct from later capability transfer.

## Budgets and verification boundary

The manifest owns every holder's shared-buffer quota, including the service's
28 pages and 14 buffers/mappings/loans and the call worker's four pages, two
buffers/loans, and four mappings. The service also has 16 private-memory pages.
The aggregate graph declares 48 capability slots and nine ingress sources.
The builder separately checks bounded worker wait shapes: stream eight, call
seven, operation nine. These are declared bounds, not measured runtime peaks.

This is a provisioning-to-healthy-idle composition, not concurrent traffic
qualification. The [boot checker](../../../../scripts/check/check-sel4-boot-plane.py)
requires worker and participant readiness, probe denial, and continued quiet
without component exit after the healthy record. That record alone only counts
live required instances; it does not certify completed userspace provisioning.
The [traffic manifest](sel4-traffic.zti) selects a distinct traffic scenario.
No fresh boot result or hardware qualification is claimed here.
