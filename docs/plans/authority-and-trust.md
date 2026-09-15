# Authority and trust plans

The current system has explicit capabilities, narrow-only derivation/transfer,
checked rights vocabulary, declared scheduling classes, and deterministic-
recording admission. The mechanisms below are not implemented and confer no
current authority.

## Revocation and leases

Add manifest-rooted derivation trees so a holder may revoke exactly a subtree it
created while parent and sibling grants survive. Specify use-after-revoke,
in-flight IPC, notification, expiry/renewal, reboot, rollback, and provenance.
The checked capability algebra must show that derive, transfer, lease transition,
and revoke never widen authority and that removing edges creates no reachability.

The A1 acceptance requirements remain in [the authority roadmap](../../roadmap/06-authority-trust.md#a1--revocation-and-leases).
Agent authority should end with its task or a declared lease (for example, write
access for thirty minutes), without killing the agent. Every derivation must
record parentage rooted in a manifest grant; narrow-only rights alone do not
prove an acyclic derivation tree. Keep derivation and lifetime ownership in one
capability-table model rather than a parallel lifecycle structure. The userspace
health service owns expiry policy through explicitly granted authority; this
does not reintroduce the retired `GenerationControl` kernel object. Provenance
must make who revoked what and when auditable. Optional manifest lifetimes expire
with the same structured revoked error, distinct from never-held authority.

- Choose wall-clock expiry evaluated at use time versus a durable health-service
  transition. The former is simple but can resurrect an expired window on
  rollback; the latter makes expiry a durable state transition. Specify whether
  leases survive generation rollback and what an epoch-anchored thirty minutes
  means across reboot, without extending authority beyond declared policy.
- Decide whether subtree revocation notifies holders or discovery at next use
  suffices, and what happens to in-flight IPC carrying the revoked capability.
- Decide whether renewal is allowed and whether the health service may renew by
  policy or only a new generation may issue the renewed authority.

The paper probe must express tree and clock semantics in the matrix grammar
without new ambient authority. Extend the [checked rights algebra](../architecture/ipc-and-capabilities.md#checked-rights-algebra):
per-operation conservation must survive revocation, and edge removal must never
create reachability; the baseline's weaker closure corollary is not a substitute.
The proxy-revokes/client-fails/siblings-survive case remains A1's concrete check.

## Secrets as capabilities

Add a `Secret` object with USE distinct from READ, service-mediated use,
non-recordability, trace redaction/re-injection, rollback-safe rotation, and
revocation timing. A foreign-workload personality that must materialize plaintext
becomes the explicit trust boundary; no recordable channel receives ambient
credential bytes.

The A2 acceptance requirements remain in [the authority roadmap](../../roadmap/06-authority-trust.md#a2--secrets-as-capabilities).
Resolve these design questions before implementation:

- Test whether USE-only service mediation covers real credentials; evaluate an
  agent presenting a bearer token to a declared model-provider destination and a
  Linux program expecting an environment credential. The latter is an explicit
  plaintext-delivery trust boundary, not evidence that revocation erases bytes
  already read by a workload.
- Specify redaction identity and replay: stable handle, commitment hash, or
  fixture substitution. Production replay re-injects from the sealed store,
  never the trace. As in [D3's clock/entropy design](../../roadmap/08-native-development.md#deterministic-component-authority),
  real entropy is not reproduced from IPC alone; a declared seeded fixture is
  an explicit reproducible input.
- Decide whether secrets must wait for A4 TPM binding or what pre-TPM sealing
  authority is acceptable. The generation must not carry the at-rest key in
  plaintext; resolve the original interim generation-carried-key proposal
  against that constraint rather than silently treating it as approved.
- Specify next-USE refusal versus interruption of in-flight uses and the
  revocation-effectiveness argument for each delivery path.
- The roadmap selects `discardOnRollback` by default so rollback cannot
  resurrect rotated credentials; any generation-declared alternative must
  preserve withdrawal. State bindings supply storage and rollback discipline;
  A4 may bind sealed material to known-good boot state.

The matrix amendment must define USE/READ, non-recordability markings, structured
errors, recorder/replay behavior, and revocation interactions before kernel work.
The [archived direction 33](https://git.justaslime.dev/iceice666/slime_os-history/src/commit/45ed1745907b2d0a13fdf70c8b34eb635bed5f23/docs/directions/33-secrets-as-capabilities.md)
preserves the original proposal, not current implementation claims.

## Accelerator compute authority

Add rights-gated queue submission with a generation-declared quantity the
implementation can actually charge — tokens, work items, or queue time — plus
IOMMU-constrained DMA over held shared buffers and visible grant topology. It
must not inherit a fictional CPU budget from scheduling class; current kernels
are non-MCS.

The A3 acceptance requirements remain in [the authority roadmap](../../roadmap/06-authority-trust.md#a3--accelerator-compute-authority).
The matrix amendment must settle the object shape, rights strings, budget model,
and H4 IOMMU-containment requirements before driver work discovers authority
questions. Hardware validation still waits for accelerator bring-up and active
DMA containment; storage's split-rights pattern is a design template, not
accelerator evidence.

- Choose one object per device or per queue class (for example, NPU inference
  versus GPU compute), with SUBMIT separate from queue creation and management.
- Choose tokens, work items, queue time, or an energy proxy as the chargeable
  unit, and a manifest scalar versus a future resource-account quantity. Define
  accounting windows, resets, and structured rejection or throttling; entry 25's
  conserved-account proposal is not an available mechanism.
- Decide whether higher-priority work can evict another queue's work and which
  right authorizes it, composing with C9 scheduling authority rather than
  equating class with budget.
- Assign firmware-loading and mode-control authority and specify how firmware
  identity interacts with generation verification.
- Reuse SharedBuffer handoff and IO0/IO1 leases/mappings; resolve the shared-buffer
  quota question rather than inventing a second buffer mechanism. DMA must stay
  within buffers the submitter holds, or SUBMIT becomes ambient memory access.
- Keep local inference and remote NetworkDestination authority disjoint, so a
  manifest can express local-model-only/no-network or remote-only deployment,
  and grant introspection can enumerate every accelerator holder.

The [archived direction 28](https://git.justaslime.dev/iceice666/slime_os-history/src/commit/45ed1745907b2d0a13fdf70c8b34eb635bed5f23/docs/directions/28-accelerator-objects.md)
preserves the original design and probe guidance.

## Physical trust and attestation

Bind the attempt counter (or epoch) and known-good generation hash to TPM-held
facts: disk-only rollback protection can itself be rolled back, resurrecting a
known-bad generation. The immutable selector must compare disk and TPM before
candidate bytes are read, alongside generation, signed-release, and boot-bundle
verification. Preserve durable attempt consumption before read/decode/launch and
known-good updates only on health confirmation, plus M5.9's sealed-counter
recovery semantics when BootState slots are unusable.

The A4 acceptance requirements remain in [the authority roadmap](../../roadmap/06-authority-trust.md#a4--physical-trust-and-attestation).
Resolve the layout and verification-flow design before implementation:

- Choose a TPM monotonic counter versus an NVRAM-sealed value, explicitly
  comparing counter exhaustion and NVRAM clearing on owner change. The bounded
  driver's counter/sealed-value support does not decide placement by itself.
- Specify the four-cell disk×TPM desynchronization matrix and cleared/unavailable
  TPM behavior. Disk newer than TPM (including reset) routes only through M5.9
  recovery; TPM newer than disk is resurrection and fails closed. A cleared TPM
  must not brick a healthy disk. Check the selector amendment against both
  `SelectableBootRootExists` and `PendingAttemptConsumedBeforeTransfer`.
- Decide quote scope: generation hash only, or also BootState health history?
  A4 requires the bound running-generation identity as the minimum read-direction
  attestation; whether health history belongs in that quote remains unresolved.
  Neither choice claims general measured boot.
- Decide whether a virtual TPM keeps the model/selector probe QEMU-verifiable or
  whether TPM-specific verification stays Framework-only. QEMU/virtual-TPM
  evidence cannot satisfy the physical exit: an older reflashed image must fail
  immutable-selector verification on the Framework target.

## Distributed capabilities

Define a cryptographic wire form that maps remote presentation to local grants,
retains revocation topology, binds session identity, rejects replay, and surfaces
partition/unreachable/revoked states as structured channel errors. Distributed
authority is separate from ROS typed-data interoperability.

The A5 acceptance requirements remain in [the authority roadmap](../../roadmap/06-authority-trust.md#a5--distributed-capabilities).
Unlike [generation sync](../directions/14-cross-machine-sync.md), which moves
objects and activation, this exposes a remote service as an ordinary typed IPC
endpoint. Keep implementation gated on cross-machine sync, A1 revocation, and
the roadmap's IO4/target-qualified network path; deterministic second-disk
transfer is not general cross-machine transport. Local typed channels and
[membrane interposition](../directions/07-schema-interposition.md) are foundations,
not evidence of implemented wire authority.

- Choose a cryptographic bearer or reference and map minting, transfer, and
  presentation to local grants; both kernels independently verify and enforce
  their own capability tables.
- Decide whether derivation trees cross machines as remote sub-derivations or
  terminate at the wire as fresh remote local grants backed by the sender's
  retained capability. Local subtree revocation must invalidate remote
  presentations, including withdrawal when a partition heals or a session ends.
- Specify the session/transport binding that rejects captured presentations in
  another context, plus in-flight partition messages and replay after reconnect.
  Unreachable, partition, replay rejection, and revoked remain distinct ordinary
  channel errors, not distributed-systems special cases in components.
- Resolve exactly-once tool-call semantics across partitions: schema-declared
  idempotency versus sequence-checked channels. This is an unresolved design
  question, not an implemented guarantee or a guarantee supplied by A5's
  presentation-replay rejection.
- A5 selects membrane proxy reuse; determine how the machinery preserves
  recording and dry-run semantics across machines without changing the typed
  endpoint component model.

The paper probe must cover wire form, A1 revocation mapping, and the partition
error vocabulary against two or three concrete cross-machine agent scenarios.

## Cross-cutting requirements

Each mechanism lands with its capability-matrix and syscall-ABI changes, a
versioned Zutai contract for every wire/persisted record, bounded tables, explicit
rollback behavior, and a real denial gate. Work-item UUIDs and dependency edges
remain in `.tasks/items/`; retained exploratory designs remain in `docs/directions/`,
and extracted origins route to immutable history below.

## References

- [`../capability-matrix.md`](../capability-matrix.md)
- [`../architecture/runtime-authority.md`](../architecture/runtime-authority.md)
- [`../decisions/mcs-cpu-budgets.md`](../decisions/mcs-cpu-budgets.md)
- Immutable origins: [direction 02](https://git.justaslime.dev/iceice666/slime_os-history/src/commit/45ed1745907b2d0a13fdf70c8b34eb635bed5f23/docs/directions/02-revocable-leases.md),
  [direction 05](https://git.justaslime.dev/iceice666/slime_os-history/src/commit/45ed1745907b2d0a13fdf70c8b34eb635bed5f23/docs/directions/05-tpm-bound-boot-state.md), and
  [direction 10](https://git.justaslime.dev/iceice666/slime_os-history/src/commit/45ed1745907b2d0a13fdf70c8b34eb635bed5f23/docs/directions/10-distributed-capabilities.md).
- [Current checked rights-algebra boundary](../architecture/ipc-and-capabilities.md#checked-rights-algebra).
