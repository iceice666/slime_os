# Authority and trust roadmap

> **H2 routing — extracted plan with retained acceptance detail.** Read the
> [authority/trust plan](../docs/plans/authority-and-trust.md) for A1–A5 design
> boundaries and the [capability matrix](../docs/capability-matrix.md) for current
> authority. Unextracted per-slice denial checks, lease/revocation cases,
> disk×TPM recovery matrix and distributed-authority acceptance detail remain
> authoritative requirements here, not historical-only text. A0's current model
> boundary lives in [IPC/capability architecture](../docs/architecture/ipc-and-capabilities.md#checked-rights-algebra);
> original delivery evidence routes to immutable history below. No planned object
> or physical trust claim is introduced by this routing. See the [file classification](README.md).

## A1 — Revocation and leases

### Required checks

- A proxy revokes a derived subtree; the original holder's further use fails structurally while sibling grants survive.
- A lease expires and is reclaimed by the health service with rollback-consistent semantics.
- The checked rights-algebra model retains closure under edge removal and admits no authority widening through revoke, expiry, renewal, or rollback.
- The capability-matrix amendment for revocation semantics lands in the same change as the mechanism.

### Exit condition

A derived grant can be withdrawn without killing its holder: the holder receives a structured revoked result, sibling and parent grants survive, lease expiry is health-service-reclaimed with specified reboot and rollback semantics, and the checked capability algebra proves that removal cannot create reachability. Together with A2 and Core C9, this satisfies the composite boundary for non-ambient, auditable, rollbackable authority.

## A2 — Secrets as capabilities

### Required checks

- A USE-only Secret authenticates to a declared service without the holder reading the secret bytes.
- A recorded trace contains no secret material while remaining replayable by sealed-store re-injection.
- Revoking the Secret capability denies the next use, and neither an earlier trace nor a peer holds the value.
- Rolling back a generation does not resurrect a rotated credential under the default `discardOnRollback` policy.
- The capability-matrix amendment for the `Secret` object lands in the same change as the mechanism.

### Exit condition

A scoped credential is usable but neither readable nor recordable: a USE-only holder authenticates to a declared service, replay remains structurally deterministic without secret bytes, revocation denies subsequent use, and rollback cannot resurrect discarded material. Together with A1 and Core C9, this satisfies the composite boundary, including capability-matrix changes accompanying each mechanism.

## A3 — Accelerator compute authority

### Required checks

- A component without the accelerator capability cannot submit work.
- A component past its declared budget is rejected or throttled with a structured error.
- Accelerator DMA cannot reach memory outside the submitting component's held buffers.
- The manifest lists every component holding accelerator authority (entry-9 queryable).

### Exit condition

Compute submission is a rights-gated, budgeted, IOMMU-contained capability: unprivileged components cannot submit, over-budget components are rejected or throttled, accelerator DMA cannot escape held buffers, and every accelerator grant is visible in the manifest.

## A4 — Physical trust and attestation

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

### Required checks

- A grant proxied to a remote service is usable there and remains unforgeable and non-ambient.
- Revoking the local subtree invalidates the remote presentation.
- A captured presentation replayed from a different session or transport fails.
- Partition, unreachable, and revoked conditions are distinguishable structured errors requiring no distributed-systems special case in components.
- The checked capability algebra retains narrow-only closure across serialization and reconnect; revocation cannot create reachability.
- The A5 checks use capability-bearing Slime endpoints and do not substitute ROS 2 DDSI-RTPS typed-data interoperability evidence.

### Exit condition

A capability proxied to a service on another machine is usable, unforgeable, and non-ambient across the wire, is revocable from the granting side, and resists replay from a foreign session—with partition and revocation surfaced as ordinary structured channel errors. ROS R1/R2 interoperability remains a separate exit and is neither required nor implied by A5.
