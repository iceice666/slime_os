# Authority and trust roadmap

**Purpose:** Define the remaining authority and physical-trust work: revocable and leased grants, non-recordable secrets, rights-gated accelerator compute, TPM-bound boot state and attestation, and capabilities transported between machines. This track owns authority mechanisms, not scheduling policy, foreign-workload translation, or ROS 2 wire interoperability.

**Status:** Planned; A1–A5 are not implemented.

**Dependencies:** Completed generation, state-binding, BootState, recovery, typed-channel, capability, and supervision foundations in [Foundations](01-foundations.md); runtime resource accounting, grant introspection, cross-machine sync, and **C9 scheduling authority** in [Core runtime](02-core-runtime.md); common queue/buffer/DMA/network mechanisms in the [Native I/O substrate](11-io-substrate.md); and physical accelerator, IOMMU, TPM, and target-backend enablement in [Platform hardware](04-platform-hardware.md). Foreign personalities consume these mechanisms through [Foreign workloads](05-foreign-workloads.md). ROS 2 interoperability remains separately owned by [ROS 2 compatibility](03-ros2-compatibility.md).

## Composite authority boundary

The earlier combined plan grouped revocation, scheduling class, and secrets. This track owns revocation and secrets as A1 and A2; Core [C9.3](02-core-runtime.md#c93--declared-scheduling-class) owns manifest-declared scheduling class, generation-controlled assignment, userspace dynamic reclassification, prevention of self-widening, and preservation across supervision restart. Composition with *resource-account quantity* is not C9's: conserved CPU accounts have no mechanism while the pinned kernel is built `KernelIsMCS OFF`, so that half is blocked on the MCS assurance decision recorded in `sel4/config/qemu-arm-virt.cmake` rather than on this track or C9.

The composite release boundary still requires all three mechanisms: a component cannot claim an ungranted scheduling class, every capability-matrix amendment lands with its gate, foreground ordering survives contention, grants can be revoked, and scoped secrets remain usable without becoming readable or recordable.

## A0 — Checked capability rights algebra

**Status:** Complete 2026-08-27. The bounded model, single-sourced vocabulary,
enforcement partition, and must-fail mutations are guarded by
`just capability_rights_model_check` and `just contracts_check`. Source and
probe measurements: [direction 24](../docs/directions/24-rights-algebra-model.md).

### Delivered

- `contracts/capability-rights/model/capability-rights.zt` models `derive`,
  `spawnGrant`, `export`, `finalize`, `import`, and `cancel` over a bounded
  four-class rights abstraction.
- Seven safety properties cover narrow-only delegation, transfer authority,
  declared-edge confinement, kind validity, transfer uniqueness, and the weaker
  authority-closure corollary. Six independent mutations must each produce the
  named counterexample.
- `contracts/generation/v5/vocab/rights.zt` is the one source of `rightBits`.
  Host tests partition all 33 names against `capability_rights_valid` and the
  root's `rights_type!` masks, including the root-only supervision rights and
  eight named-but-ungated legacy rights.
- Any change to `rightBits`, `capability_rights_valid`, or a `rights_type!`
  `VALID` mask must land with the resulting model and capability-matrix updates.

### Exit condition

A bounded model of the six authority operations checks seven named properties,
six must-fail mutations fail closed, and the single-sourced rights vocabulary is
pinned against real manifest and runtime enforcement. The original manifest
authority-closure formulation is not the theorem: the probe showed it cannot be
both non-vacuous and true under honest transfer.

## A1 — Revocation and leases

**Status:** Planned; not implemented.

**Dependencies:** Narrow-only capability derivation and manifest-rooted grants; the M6 spawn/endpoint-minting and health-service foundations in [Foundations](01-foundations.md); capability provenance; the checked rights algebra from [direction 24](../docs/directions/24-rights-algebra-model.md); and Core C9 for the composite scheduling exit. This milestone enables A2 revocable credentials and A5 wire revocation. Source: [direction 02](../docs/directions/02-revocable-leases.md).

### Deliverables

- Kernel-maintained capability derivation trees. Every derivation records parentage, every tree is rooted in a manifest grant, and a holder may revoke exactly the subtree it created without revoking its parent or siblings.
- A structured use-after-revoke error distinct from both never-held authority and transport failure, including specified behavior for in-flight IPC and holder notification versus discovery at next use.
- Optional generation-declared grant lifetimes reclaimed by the health service, with renewal authority and expiry behavior expressed without ambient authority.
- A single lease clock model that resolves wall-clock-at-use versus durable health-service transition. Its reboot and rollback behavior must be explicit: rollback must not resurrect expired authority or extend a lease beyond the declared policy.
- Auditable provenance for grant derivation, revocation, lease creation, renewal, and expiry.
- A capability-matrix amendment for derivation-tree ownership, revocation, leases, and their structured errors, landing in the same change as the mechanism.
- An extension of the capability algebra proving narrow-only derivation and revocation closure: no sequence of derive, transfer, lease transition, or revoke can exceed the initial manifest grants, and **removing edges never creates reachability**.

### Required checks

- A proxy revokes a derived subtree; the original holder's further use fails structurally while sibling grants survive.
- A lease expires and is reclaimed by the health service with rollback-consistent semantics.
- The checked rights-algebra model retains closure under edge removal and admits no authority widening through revoke, expiry, renewal, or rollback.
- The capability-matrix amendment for revocation semantics lands in the same change as the mechanism.

### Exit condition

A derived grant can be withdrawn without killing its holder: the holder receives a structured revoked result, sibling and parent grants survive, lease expiry is health-service-reclaimed with specified reboot and rollback semantics, and the checked capability algebra proves that removal cannot create reachability. Together with A2 and Core C9, this satisfies the composite boundary for non-ambient, auditable, rollbackable authority.

## A2 — Secrets as capabilities

**Status:** Planned; not implemented.

**Dependencies:** A1 revocation; completed M5.6b state bindings for at-rest storage; spawn with no implicit environment; typed IPC; the flight-recorder/replay design; optional A4 TPM sealing for hardware-bound at-rest protection; and [Foreign workloads](05-foreign-workloads.md) for personality or guest delivery that genuinely requires plaintext. Core C9 is required only for the composite authority boundary. Source: [direction 33](../docs/directions/33-secrets-as-capabilities.md).

### Deliverables

- A `Secret` object kind with a USE right, allowing presentation only to a designated service, separated from a narrower or absent READ right. Derivation remains narrow-only.
- Service-mediated secret use so a USE-only holder can authenticate without receiving credential bytes. Network-destination authority and secret authority remain separate grants.
- A non-recordability rule for Secret capabilities and secret-bearing IPC. The flight recorder stores only a handle or commitment; replay re-injects material from the sealed store rather than copying it from the trace.
- Explicit redaction and deterministic-replay semantics: traces remain replayable in structure while containing no credential material.
- At-rest storage using M5.6b state bindings, with `discardOnRollback` as the default so rollback cannot resurrect a rotated credential. Any alternative policy must be generation-declared and preserve revocation effectiveness.
- Defined revocation timing for next USE and in-flight use. After revocation, no trace or peer may retain material that defeats withdrawal.
- A bounded foreign-workload delivery rule: when a program must read plaintext, the personality is the trust boundary and materializes it only inside the confined workload address space, never through a recordable channel.
- A capability-matrix amendment for the Secret object, USE/READ rights, non-recordability, rollback policy, and structured errors, landing in the same change as the mechanism.

### Required checks

- A USE-only Secret authenticates to a declared service without the holder reading the secret bytes.
- A recorded trace contains no secret material while remaining replayable by sealed-store re-injection.
- Revoking the Secret capability denies the next use, and neither an earlier trace nor a peer holds the value.
- Rolling back a generation does not resurrect a rotated credential under the default `discardOnRollback` policy.
- The capability-matrix amendment for the `Secret` object lands in the same change as the mechanism.

### Exit condition

A scoped credential is usable but neither readable nor recordable: a USE-only holder authenticates to a declared service, replay remains structurally deterministic without secret bytes, revocation denies subsequent use, and rollback cannot resurrect discarded material. Together with A1 and Core C9, this satisfies the composite boundary, including capability-matrix changes accompanying each mechanism.

## A3 — Accelerator compute authority

**Status:** Planned; not implemented.

**Dependencies:** IO0 queue/lease/completion semantics and IO1 DMA authority from the [Native I/O substrate](11-io-substrate.md); platform-hardware accelerator bring-up and IOMMU-enforced DMA in [Platform hardware](04-platform-hardware.md); generation manifests; entry-9 grant introspection in [Core runtime](02-core-runtime.md); and [C9.3](02-core-runtime.md#c93--declared-scheduling-class) wherever queue ordering or preemption uses scheduling authority. Entry-25's conserved `ResourceAccount` is *not* available from Core runtime: C9 disclaims conserved CPU accounts while the pinned kernel is built `KernelIsMCS OFF`, so A3's compute budget must pick a unit it can actually charge — tokens, work items, or queue time per window — rather than inheriting a general account model. The existing storage-capability pattern is the rights-gating template, not proof that a GPU queue exists.

### Deliverables

- An `Accelerator` object representing one compute device or queue class, with SUBMIT separated from queue creation, firmware loading, mode control, and any preemption authority.
- Generation-declared, deterministically bounded compute budgets expressed as tokens, work items, queue time per window, or a selected resource-account quantity. The chosen unit, accounting window, reset behavior, and exhaustion response must be manifest-defined and auditable.
- Structured rejection or throttling at budget exhaustion. Accelerator budget controls quantity; Core C9 owns scheduling class, ordering, and any authority to preempt.
- IOMMU-constrained accelerator DMA limited to IO1 mappings of SharedBuffers the submitting component holds, with IO0 leases retaining those buffers through completion and no parallel or ambient buffer-transfer mechanism.
- A capability-matrix row for the object and every right, landing with the driver and mechanism.
- Manifest/grant-graph visibility for every component holding accelerator authority.

### Required checks

- A component without the accelerator capability cannot submit work.
- A component past its declared budget is rejected or throttled with a structured error.
- Accelerator DMA cannot reach memory outside the submitting component's held buffers.
- The manifest lists every component holding accelerator authority (entry-9 queryable).

### Exit condition

Compute submission is a rights-gated, budgeted, IOMMU-contained capability: unprivileged components cannot submit, over-budget components are rejected or throttled, accelerator DMA cannot escape held buffers, and every accelerator grant is visible in the manifest.

## A4 — Physical trust and attestation

**Status:** Planned; not implemented.

**Dependencies:** Completed M5.6 BootState semantics and checked invariants, including `SelectableBootRootExists` and `PendingAttemptConsumedBeforeTransfer`; completed M5.9 recovery and reconstruction; immutable selector generation/release verification; and a bounded Framework-class TPM driver from [Platform hardware](04-platform-hardware.md). Source: [direction 05](../docs/directions/05-tpm-bound-boot-state.md).

### Deliverables

- A bounded TPM driver sufficient for monotonic counters and NVRAM-sealed values; this does not claim general measured boot.
- TPM binding for the attempt counter or epoch and known-good generation hash. On-disk BootState must agree with TPM-held facts at the immutable selector's pre-candidate-read gate.
- A checked disk×TPM desynchronization matrix with explicit policy for every cell: disk newer than TPM routes only through M5.9 recovery; TPM newer than disk is the resurrection case and fails closed; cleared or unavailable TPM cannot silently bypass policy and must never brick a healthy disk.
- Recovery/reconstruction rules for restoring or rebinding TPM state without violating `SelectableBootRootExists` or `PendingAttemptConsumedBeforeTransfer`.
- Attestation limited to the read direction needed here: a TPM quote of the bound running-generation identity, sufficient for a remote verifier to distinguish generations.
- An explicit evidence split. Framework hardware is required for the physical trust exit. QEMU checks may cover the transition model and immutable selector logic; a virtual TPM may be added only if selected explicitly, and QEMU evidence must never be presented as physical TPM evidence.

### Required checks

- Reflashing an older generation image fails immutable-selector verification against TPM-held counters.
- A cleared or unavailable TPM fails open only through the explicit M5.9 recovery path and never bricks a healthy disk.
- Every desync-matrix cell resolves to its declared policy without leaving zero bootable roots.
- The checked flow continues to satisfy `SelectableBootRootExists` and `PendingAttemptConsumedBeforeTransfer`.
- A remote verifier can distinguish two different running generation identities from their attestations.
- The first three physical claims are demonstrated on the Framework target; any virtual-TPM/QEMU result is recorded separately as model or emulator evidence.

### Exit condition

On the Framework target, reflashing an older generation fails immutable-selector verification against TPM-held counters, a cleared TPM cannot brick a healthy disk, and the running generation identity is remotely attestable—all without violating `SelectableBootRootExists`. QEMU or virtual-TPM evidence alone does not satisfy this exit.

## A5 — Distributed capabilities

**Status:** Planned; not implemented.

**Dependencies:** A1 derivation-tree revocation; cross-machine sync; IO4 networking and exact destination authority from the [Native I/O substrate](11-io-substrate.md); a target-qualified physical link backend where required; typed channels and entry-7 membrane/interposition machinery from [Core runtime](02-core-runtime.md). A5 is authority transport and is independent of the ROS 2 R1/R2 typed-data interoperability work in [ROS 2 compatibility](03-ros2-compatibility.md). Source: [direction 10](../docs/directions/10-distributed-capabilities.md).

### Deliverables

- A cryptographic wire form whose minting, transfer, and presentation map to local grants on both sides while each kernel independently enforces its own capability table.
- A specified revocation topology: derivation trees either extend across machines or terminate at the wire with the sender retaining a backing grant. In either design, revoking the local A1 subtree invalidates remote presentations without widening authority.
- Session- or transport-identity binding so a captured presentation replayed in another context fails.
- Explicit partition semantics for in-flight messages and reconnect, including distinguishable structured errors for endpoint unreachable, partition, replay rejection, and capability revoked in the existing channel vocabulary.
- Reuse of membrane proxy endpoints so recording and dry-run semantics extend across machines without changing the component model: a tool call remains typed IPC to an endpoint.
- A capability-algebra extension showing that serialization, remote derivation, reconnect, revocation, and replay handling cannot exceed the granting side's initial authority.
- A strict boundary from ROS: A5 transports unforgeable, non-ambient **authority** and its revocation semantics between Slime kernels. ROS R1/R2 own DDSI-RTPS discovery, transport, and typed-data interoperability. A DDSI-RTPS participant or typed sample is not a wire capability; implementing one does not satisfy the other.

### Required checks

- A grant proxied to a remote service is usable there and remains unforgeable and non-ambient.
- Revoking the local subtree invalidates the remote presentation.
- A captured presentation replayed from a different session or transport fails.
- Partition, unreachable, and revoked conditions are distinguishable structured errors requiring no distributed-systems special case in components.
- The checked capability algebra retains narrow-only closure across serialization and reconnect; revocation cannot create reachability.
- The A5 checks use capability-bearing Slime endpoints and do not substitute ROS 2 DDSI-RTPS typed-data interoperability evidence.

### Exit condition

A capability proxied to a service on another machine is usable, unforgeable, and non-ambient across the wire, is revocable from the granting side, and resists replay from a foreign session—with partition and revocation surfaced as ordinary structured channel errors. ROS R1/R2 interoperability remains a separate exit and is neither required nor implied by A5.
