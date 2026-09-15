# Exploratory directions register

## Entry index

| # | Direction | Route |
| --- | --- | --- |
| 1 | [Authority diff as a build-pipeline gate](01-authority-diff-gate.md) | authority |
| 7 | [Schema-driven interposition toolchain](07-schema-interposition.md) | determinism; interposition |
| 8 | [Declarative supervision and restart policy](08-declarative-supervision.md) | lifecycle |
| 9 | [Manifest static analysis and grant-graph introspection](09-grant-graph-introspection.md) | authority |
| 11 | [IPC flight recorder and deterministic replay](11-flight-recorder-replay.md) | determinism |
| 12 | [Generation bisect](12-generation-bisect.md) | updates |
| 13 | [Shadow boot](13-shadow-boot.md) | updates |
| 14 | [Cross-machine generation sync](14-cross-machine-sync.md) | sync |
| 15 | [Zutai-defined state migrations](15-zutai-state-migrations.md) | sync |
| 16 | [Powerbox UI](16-powerbox.md) | lifecycle |
| 17 | [Per-component energy accounting](17-energy-accounting.md) | hardware |
| 18 | [Per-destination network authority](18-network-authority.md) | hardware |
| 19 | [MPK/PKU lightweight compartments](19-mpk-compartments.md) | hardware |
| 25 | [Resource accounts as capabilities](25-resource-accounts.md) | lifecycle |
| 26 | [Hermetic generation testing](26-hermetic-testing.md) | determinism |
| 27 | [Policy-carrying generations](27-policy-carrying-generations.md) | authority |
| 29 | [Schema-declared state merge](29-schema-state-merge.md) | sync |
| 32 | [Scheduling class and QoS authority](32-scheduling-authority.md) | lifecycle |
| 34 | [Root capacity ceilings: threads and cores](34-capacity-ceilings.md) | capacity |

## Routes

Routes are a reading aid, not a status axis; entries may name a secondary
route when they span clusters.

- **authority** (1, 9, 27): static authority analysis. Entry 9 builds the grant-graph
  query engine over machine-readable manifests; entry 1 is two graph
  snapshots plus a CI sign-off gate; entry 27 turns a graph predicate into
  a generation-carried, boot-verified invariant. Later
  hardware-route entries (18) consume the same engine as audit queries.
- **determinism** (7, 11, 26): Entry 7's generated membranes supply the recording machinery 11
  generalizes.
- **sync** (14, 15, 29): a dependency chain — transfer and activation
  (14) plus schema migration (15) compose into deterministic three-way
  state merge (29).
- **lifecycle** (8, 16, 25, 32): supervision
  (8), resource accounts (25), powerbox interaction (16), and scheduling
  policy (32). The [runtime reference](../architecture/runtime-authority.md)
  owns current restart and class-ordering mechanisms; the
  [MCS proposal](../decisions/mcs-cpu-budgets.md) owns the conserved-CPU-budget
  boundary. Neither replaces the remaining exploratory composition questions.
- **updates** (12, 13): machinery around the generation parent
  chain — bisect (12) and shadow boot (13) consume M5.6 rollback.
- **hardware** (17, 18, 19): daily-driver and Framework-target work,
  all bound to the Hardware H track at the kernel level; each has a capability-matrix amendment
  or design-note half that is legal today.
- **capacity** (34): additional worker threads and SMP remain exploratory.
  Current private-memory bounds belong to the
  [private-memory reference](../architecture/private-memory.md), and further
  target-qualified memory workloads belong to the
  [capacity plan](../plans/memory-capacity.md). Memory qualification does not
  establish support for additional threads, cores, or a particular guest,
  inference engine, or multi-queue driver workload.

## Research references

The resulting contracts remain
Slime-specific rather than adopting any external system wholesale.

- [Genode Foundations](https://genode.org/documentation/genode-foundations/)
  informs entry 25's account-derived resource delegation; the Slime delta is
  carrying the account distribution as rollbackable generation data.
- [TLA+ implementation trace validation](https://arxiv.org/html/2404.16075v2)
  motivates M5.6c's finite-trace conformance check and documents its limits.
- [The Update Framework specification](https://theupdateframework.github.io/specification/latest/)
  informs M5.8's threshold trust, versioned root rotation, and replay checks.
- [Android A/B updates](https://source.android.com/docs/core/ota/ab) and
  [Verified Boot flow](https://source.android.com/docs/security/features/verifiedboot/boot-flow)
  motivate retaining a bootable fallback and advancing rollback protection
  only after the pending system is confirmed successful.
- [OSTree atomic upgrades](https://ostreedev.github.io/ostree/atomic-upgrades/),
  [OSTree deployments](https://ostreedev.github.io/ostree/deployment/), and
  [Nix GC roots](https://nix.dev/manual/nix/2.34/package-management/garbage-collector-roots)
  inform M5.6b's deployment/state pairing and explicit reachability roots.
- [seL4 capDL](https://docs.sel4.systems/projects/capdl/) informs the static
  authority questions in entries 1 and 9.
