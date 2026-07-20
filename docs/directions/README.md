# Exploratory directions register

Parking lot for every direction that follows from the Slime OS vision but is
not committed work. Each entry keeps a stable number so cross-references
(ROADMAP follow-up notes, the sequencing table) stay valid. Entries 11–19
were originally README's "Differentiating directions" section, moved here so
README carries a single pointer.

Active entries (parked or probing) live in one file each in this directory.
Promoted entries collapse to a pointer row in the index below; ROADMAP.md
owns their content from that point on.

## Rules

- A direction here is not a commitment. It becomes real only when promoted
  into `ROADMAP.md` with an observable exit condition.
- At most one direction may be in `probing` status at a time. A probe is
  time-boxed work (design note or minimal experiment) that ends in either
  promotion to the roadmap or a return to `parked` with the reason recorded.
- A direction that requires violating project invariants (ambient authority,
  kernel-owned policy, non-deterministic formats) is `rejected`, not shelved.
- Dependencies name the roadmap milestone whose mechanisms the direction
  consumes. A direction whose dependency has not landed can only be probed
  as a host-side or paper exercise, never as kernel code.
- New entries take the next free number; numbers are never reused, and
  existing entry files are never renumbered.

## Status values

`parked` — registered, no active work.
`probing` — the single active exploration slot.
`promoted` — moved into ROADMAP.md; this register keeps only a pointer.
`rejected` — decided against, with reason.

## Entry index

| # | Direction | Status | Route |
| --- | --- | --- | --- |
| 1 | [Authority diff as a build-pipeline gate](01-authority-diff-gate.md) | parked | authority |
| 2 | [Revocable and time-bounded grants](02-revocable-leases.md) | parked | lifecycle |
| 3 | [Nondeterminism sources as capabilities](03-nondeterminism-as-capabilities.md) | parked | determinism |
| 4 | Generation-consistent state snapshots | promoted → M5.6b | — |
| 5 | [TPM-bound boot state and attestation](05-tpm-bound-boot-state.md) | parked | hardware; updates |
| 6 | Formal model of BootState transitions | promoted → M5.6a | — |
| 7 | [Schema-driven interposition toolchain](07-schema-interposition.md) | parked | determinism; interposition |
| 8 | [Declarative supervision and restart policy](08-declarative-supervision.md) | parked | lifecycle |
| 9 | [Manifest static analysis and grant-graph introspection](09-grant-graph-introspection.md) | parked | authority |
| 10 | [Distributed capabilities](10-distributed-capabilities.md) | parked | sync |
| 11 | [IPC flight recorder and deterministic replay](11-flight-recorder-replay.md) | parked | determinism |
| 12 | [Generation bisect](12-generation-bisect.md) | parked | updates |
| 13 | [Shadow boot](13-shadow-boot.md) | parked | updates |
| 14 | [Cross-machine generation sync](14-cross-machine-sync.md) | parked | sync |
| 15 | [Zutai-defined state migrations](15-zutai-state-migrations.md) | parked | sync |
| 16 | [Powerbox UI](16-powerbox.md) | parked | lifecycle |
| 17 | [Per-component energy accounting](17-energy-accounting.md) | parked | hardware |
| 18 | [Per-destination network authority](18-network-authority.md) | parked | hardware |
| 19 | [MPK/PKU lightweight compartments](19-mpk-compartments.md) | parked | hardware |
| 20 | BootState model-implementation conformance | promoted → M5.6c | — |
| 21 | Signed generation release metadata | promoted → M5.8 | — |
| 22 | Recovery, scrub, and BootState reconstruction | promoted → M5.9 | — |
| 23 | [Generation build-provenance attestations](23-build-provenance.md) | parked | updates |
| 24 | [Checked model of the capability rights algebra](24-rights-algebra-model.md) | **probing** | authority |
| 25 | [Resource accounts as capabilities](25-resource-accounts.md) | parked | lifecycle |
| 26 | [Hermetic generation testing](26-hermetic-testing.md) | parked | determinism |
| 27 | [Policy-carrying generations](27-policy-carrying-generations.md) | parked | authority |
| 28 | [Accelerator compute objects](28-accelerator-objects.md) | parked | hardware |
| 29 | [Schema-declared state merge](29-schema-state-merge.md) | parked | sync |
| 30 | [Deterministic on-device builds](30-deterministic-on-device-builds.md) | parked | determinism |

## Routes

Routes are a reading aid, not a status axis; entries may name a secondary
route when they span clusters.

- **authority** (1, 9, 24, 27): static authority analysis. Entry 24 models
  the rights algebra that defines widening; entry 9 builds the grant-graph
  query engine over machine-readable manifests; entry 1 is two graph
  snapshots plus a CI sign-off gate; entry 27 turns a graph predicate into
  a generation-carried, boot-verified invariant. Later hardware-route
  entries (18, 28) consume the same engine as audit queries.
- **determinism** (3, 7, 11, 26, 30): entry 3's capability-matrix amendment
  for clock/entropy objects is the linchpin; it unlocks general replay
  (11), byte-deterministic CI (26), and on-device builds as pure functions
  (30). Entry 7's generated membranes supply the recording machinery 11
  generalizes.
- **sync** (10, 14, 15, 29): a dependency chain — transfer and activation
  (14) plus schema migration (15) compose into deterministic three-way
  state merge (29); distributed capabilities (10) sit beyond and
  additionally consume revocation (2).
- **lifecycle** (2, 8, 16, 25): component lifetime semantics — revocation
  and leases (2), supervision (8), resource accounts (25), and the
  powerbox pattern (16). Entries 2, 8, and 25 share the M6 spawn-service
  prerequisites.
- **updates** (5, 12, 13, 23): machinery around the generation parent
  chain — bisect (12) and shadow boot (13) consume M5.6 rollback;
  build-provenance attestations (23) accompany the M5.8 release pipeline;
  TPM binding (5) hardens BootState on physical hardware.
- **hardware** (17, 18, 19, 28): daily-driver and Framework-target work,
  all M7-bound at the kernel level; each has a capability-matrix amendment
  or design-note half that is legal today.

## What is unblocked now

Per the dependency rule, the following entries have legal work today
without waiting for any milestone:

| Entry | Legal work today | Why |
| --- | --- | --- |
| 24 | active probe: checked model of the rights algebra | depends on nothing; methodology established by M5.6a/M5.6b |
| 9 | grant-graph query engine (host-side half) | M5.5 machine-readable grants complete |
| 1 | generation authority diff + CI sign-off gate | M5.5 complete; consumes 9's engine |
| 27 | invariant-section format, builder computation, verification-hook design | M5.5 and the M5.6 activation path both complete |
| 12 | automated QEMU boot-and-health-check bisection | M5.6 rollback machinery complete |
| 7 | generated-membrane prototype over `contracts/block/` | M5.2a contract tooling complete; `storage_fault_check` is the replay fixture |
| 3 | capability-matrix amendment proposal for clock/entropy objects | explicitly an amendment proposal first |
| 2, 8, 25, 29, 10 | design notes (paper) | kernel work blocked on M6 prerequisites or later entries |
| 23 | attestation schema design (paper) | promotion awaits an M5.8 builder identity |
| 13 | shadow sub-graph manifest design (paper) | execution plausibly needs M6 spawn machinery |
| 5, 17, 18, 19, 28 | matrix amendments and design notes (paper) | kernel work M7-bound |

## Sequencing

| Wave | Directions | Why then |
| --- | --- | --- |
| 0 — before M5.6 implementation (done) | 6 (M5.6a), 4 (M5.6b) | Both promoted checked contracts landed; transition and state/GC semantics froze before implementation. |
| 1 — with and after M5.6 | 20 (M5.6c), 1, 9, 12, 13, 24 | Trace conformance closes the model/implementation gap; authority analysis, bisect, and shadow boot consume machine-readable manifests or rollback machinery. Entry 24 is dependency-free contracts work in the M5.6a methodology and is the current probe. |
| 2 — late M5 to M6 | 21 (M5.8), 22 (M5.9), 23, 7, 11 (recording), 8, 3, 14, 15, 16, 11 (replay), 25, 26, 27, 29, 30 | Release trust and recovery must precede cross-machine activation; spawn prerequisites then unlock supervision, resource accounts, migration, powerbox, and general replay. Entries 26 and 30 consume entry 3, entry 27 consumes the M5.6 activation path, and entry 29 follows 14 and 15. |
| 3 — M7 and beyond | 5, 2, 10, 17, 18, 19, 28 | Physical TPM, revocation, distributed authority, accelerator control, and daily-driver hardware respectively. |

## Research references

These sources informed entries 4 and 20–25; the resulting contracts remain
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
- [SLSA build provenance](https://slsa.dev/spec/v1.2/build-provenance) informs
  entry 23's separation of build provenance from release authorization.
