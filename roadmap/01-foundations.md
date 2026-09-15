# Foundations and implemented history (Milestones 1–6)

> **H2 routing — mixed retained source.** Current capability/runtime boundaries
> are in [architecture](../docs/architecture/README.md), and Framework promotion
> boundaries are in the [hardware plan](../docs/plans/framework-hardware.md).
> M5.7's detailed storage-aware boot and NVMe promotion requirements remain owned
> here where not extracted; they are not historical or waived. M5 storage,
> BootState/recovery and M6 spawn/filesystem/powerbox/transfer detail remains
> unextracted source material, not reverified current documentation: use its owning
> contracts and implementation for current behavior.
> See the [file classification](README.md).

**Purpose:** Preserve the implemented kernel, isolation, bootstrap, storage/generation, and native-environment contracts that later roadmap tracks build on.

## Invariants carried forward

- The kernel supplies privileged mechanism, not service policy: address spaces, memory objects, capability enforcement, IPC, scheduling, interrupts, timers, and minimal platform control.
- Components receive only manifest-declared capabilities. Transfers and derivations can narrow authority but never widen it.
- Protocols and persistent formats are versioned, bounded, deterministically encoded, and reject unknown required versions or flags.
- Executable objects are immutable and content-verified before execution. Boot state, generation roots, and state roots remain separate concepts.
- QEMU establishes deterministic logic, bounded error handling, and fault recovery. It does **not** establish physical firmware behavior, DMA containment, device identity, power-loss behavior, or absence of writes on any named hardware target. Milk-V Duo evidence is likewise target-specific and cannot establish Framework storage behavior.
- **Internal Framework NVMe writes remain disabled.** Neither recovery media nor a storage-aware generation receives internal-NVMe write authority by default. No such write path may be enabled until all M5.7 promotion gates and the later Framework hardware reliability gate have physical evidence; Duo storage or component evidence cannot substitute.

## M5 — Storage and generations

**Depends on:** M1–M4.

**Authority and safety boundary:** Storage clients use a block-service endpoint plus bounded shared memory. A trusted driver receives explicit PCI-function, DMA-memory, interrupt, and shared-memory capabilities. The kernel enforces rights, mappings, DMA-buffer lifetime, and interrupt delivery; the userspace service owns partition policy, retries, and access control. There are no ambient block syscalls or guessed global device names. Before IOMMU enforcement, DMA-capable drivers remain trusted and writes are restricted to deterministic QEMU fixtures or dedicated replaceable test devices.

### M5.1 — Storage capability foundation

**Depends on:** M2 and M4 platform/capability foundations.

**Required checks:** Reject missing, widened, duplicate, stale, out-of-range, and wrong-kind capabilities; prevent reclaim of in-flight DMA buffers; reject malformed PCI metadata without hanging.

**Exit condition:** An isolated driver receives only explicitly granted generic resources, and an unprivileged component cannot acquire device rights.

### M5.2a — Typed IPC schemas

**Depends on:** M5.1's block protocol.

**Required checks:** Round-trip every message type byte-identically on both ends and reject stale bindings and out-of-bounds fields.

**Exit condition:** The M5.2 block protocol has one schema-first layout under `contracts/`, with no disagreeing hand-written representation.

### M5.2 — Read-only virtio block vertical slice

**Depends on:** M5.1 and M5.2a.

**Required checks:** Verify the known sector through the full component/capability path; reject writes, out-of-range LBAs, short buffers, invalid descriptors, unsupported features, and timeouts with structured errors; contain driver failure; keep the generation slice healthy.

**Exit condition:** A userspace component reads and verifies a read-only QEMU virtio device without ambient storage authority.

### M5.3 — Durable virtio writes and fault handling

**Depends on:** M5.2.

**Required checks:** Verify write/read-back and fresh-boot persistence; leave images unchanged on out-of-bounds writes; reclaim descriptors and pins after errors; never report failed flush as durable; never reuse stale completions after reset.

**Exit condition:** Disposable QEMU images provide bounded, explicitly authorized durable writes and deterministic recovery from injected failures.

### M5.4 — GPT and integrity-checked object store

**Depends on:** M5.3.

**Required checks:** Validate GPT bounds and CRCs; resolve copy damage without accepting conflicting valid copies; prevent malformed metadata from causing out-of-bounds I/O; verify complete payload hashes before use; preserve the previous root at every append/commit interruption; reject overlap, overflow, truncation, bad hashes, conflicting identities, and unsupported versions.

**Exit condition:** QEMU retrieves immutable content-addressed objects from a bounded GPT partition while malformed and partial commits fail closed.

### M5.5 — Generation format and BootState records

**Depends on:** M5.4 and the earlier generation contract.

**Required checks:** Require byte-identical artifacts from normalized inputs; reject unknown versions/flags, excessive counts, oversized strings, broken parents, and bad checksums; execute nothing before verification; retain selection through one interrupted or invalid slot.

**Exit condition:** The immutable disk-backed seL4 selector deterministically selects and verifies a complete generation from redundant persistent metadata before `slime-root` admits and launches its component graph.

### M5.6a — Checked BootState transition model

**Depends on:** M5.5.

**Required checks:** Exhaust the bounded interleavings and ensure a deliberate skipped-attempt mutation fails.

**Exit condition:** CI maintains a checked transition contract; implementation-semantic changes update the model with the implementation.

### M5.6b — Checked generation, state, and GC transaction model

**Depends on:** M5.6a.

**Required checks:** Never mix snapshot epochs or expose incomplete state; never collect a reachable sealed object; ensure omitted-root and mixed-epoch mutations fail.

**Exit condition:** Every modeled upgrade, snapshot, promotion, rollback, and GC interleaving retains a bootable generation with a consistent state set.

### M5.6 — Pending, known-good, rollback, state policy, and GC

**Depends on:** M5.6a and M5.6b; their semantics are the implementation contract.

**Required checks:** Inject interruption before pending metadata, during either slot write, after pending commit, after attempt commit/before candidate read, during promotion, rollback update, state snapshot, and GC. Every reboot selects either pending with the correct reduced attempt count or verified known-good, never zero roots. Distinguish component exit, fault, timeout, peer loss, and explicit unhealthy status; deny health confirmation to unprivileged components.

**Exit condition:** A failing pending generation automatically returns to verified known-good with persistent state and roots matching declared policy.

### M5.6c — BootState model/implementation conformance

**Depends on:** M5.6a, M5.6b, and M5.6.

**Required checks:** Accept every rollback scenario trace; reject attempt consumption that is not durable before candidate execution, mismatched action/commit or sequence boundaries, wrong-root promotion/collection, and any unbounded instrumentation dependency.

**Exit condition:** Durable transitions observed in QEMU conform to the checked state machine, and deliberately invalid traces fail validation.

### M5.7 — Framework NVMe transport and safety promotion

**Depends on:** M5.1–M5.6c, M4's historical removable-media safety invariant, the [Framework CPU-boot observation and current image-binding limitation](../docs/architecture/targets-and-portability.md#automated-and-physical-evidence), H1's observed inventory, H2's exact PCI binding, and H4's AMD-IOMMU containment.

**Still required:** After P6.6 has proven the no-storage product boot, implement the NVMe transport behind the seL4 userspace-driver boundary using H1's observed controller/namespace identity, H2-bound PCI/MMIO/IRQ resources, H4 DMA containment, and IO2's `BlockDevice` contract; then observe a storage-aware removable-media Framework boot without modifying internal NVMe. Destructive writes and interruption experiments may run only on a dedicated replaceable external test device.

**Promotion gates before any internal NVMe write may be enabled:** deterministic bounds and malformed-command tests; DMA isolation suitable for the physical target; timeout/reset recovery; flush ordering and durable-write tests; interrupted metadata and generation-transition tests; malformed GPT/object-store/generation/BootState tests; an explicit write capability held only by the intended service; and an operator-visible distinction between removable test media and internal NVMe. Production IOMMU enforcement and internal-disk promotion remain a later hardware reliability gate.

QEMU, Milk-V Duo, or another board cannot replace the required Framework observation.

**Exit condition:** Starting from the P6.6/H1-proven no-write boot path, a physical Framework runs the storage-aware isolated slice over the common protocol while internal NVMe writes remain disabled unless every physical promotion gate has been observed.

### M5.8 — Signed generation release metadata

**Depends on:** M5.5–M5.6c.

**Required checks:** Reject insufficient threshold, missing/duplicate/malformed/excessive signatures, wrong target, stale releases, skipped rotation, or broken old/new-root continuity; preserve the accepted sequence after failed pending boots and preserve the explicit rollback root after promotion.

**Exit condition:** The immutable seL4 selector/root admission and generation management accept only authorized releases while retaining automatic local rollback.

**Scope limit:** This evidence does not establish trusted-time freeze protection, UEFI Secure Boot, TPM sealing, or resistance to rollback of an entire physical disk image.

### M5.9 — Recovery, scrub, and BootState reconstruction

**Depends on:** M5.4–M5.8.

**Required checks:** Never execute an unverified object after dual-slot corruption; reconstruct one disposable QEMU disk while a second ungranted disk remains byte-identical; reject missing state, broken closure, unauthorized releases, and interrupted incomplete reconstruction; retain the Framework removable-media no-write gate.

**Exit condition:** Signed removable recovery reconstructs a verified bootable root without modifying any device not named by an explicit capability.

M5 closes only after M5.7's physical observation. At that point all executable content must verify before execution; staging must preserve running/known-good; attempts must commit before candidate bytes are read, decoded, or launched; confirmation must apply only to the running pending generation; interruption must preserve a valid slot; checked traces must match the state/GC models; failure must return to known-good; GC must preserve every retained root; every state policy must have upgrade/rollback evidence; read/write authority must be explicit; malformed metadata must fail before out-of-bounds I/O or execution; releases and recovery must preserve local rollback and capability isolation; and the Framework storage-aware observation must show no unauthorized internal-NVMe write.

## M6 — Native interactive environment

**Depends on:** M1–M5 storage, object-store, generation, state-policy, rollback, release-trust, and recovery mechanisms. M6 acceptance is QEMU/removable-media evidence and remains independent of pending M5.7 physical verification. Internal NVMe writes remain disabled.

### M6.1 — Kernel spawn prerequisites and generation format v2

**Depends on:** M5 generation/capability contracts.

**Required checks:** Deny gifting absent rights and all widening; return structured exhaustion per spawner without harming others; bound endpoint tables; preserve distinct supervision outcomes; produce byte-identical v2 artifacts and reject unknown versions; source test bootstrap grants only from manifests.

**Exit condition:** A factory-authorized spawner mints bounded endpoints, gifts narrowed copies, and supervises children within its budget using deterministic manifest wiring.

### M6.2 — Spawn service and command profile

**Depends on:** M6.1.

**Required checks:** Byte-identical schema round trips and bounded-version rejection; deny undeclared executables, budget excess, and code injection; deterministic resolution; contain spawn-service failure.

**Exit condition:** A client resolves a profile command and launches it with exactly the declared grants.

### M6.3 — Filesystem service and directory capabilities

**Depends on:** M6.1 and M5.4.

**Required checks:** Deny name access without a directory capability; restrict derivation by subdirectory and rights; preserve the previous root across interruption; enforce path, entry-count, and depth bounds before store I/O.

**Exit condition:** Components browse and mutate namespaces only through explicit directory capabilities with store-verified metadata.

### M6.4 — Minimal Dango implementation and core runtime

**Depends on:** M6.2 and M6.3.

Full Hindley–Milner, row-polymorphism, and effect inference are outside this slice.

**Required checks:** Trace each launch to profile resolution and a spawn request; leak no ambient context; preserve termination distinctions at the language boundary; reproduce scripted sessions deterministically.

**Exit condition:** Native console commands run through Dango with capability-resolved authority and structured failures.

### M6.5 — Generation inspection and update commands

**Depends on:** M5.6c, M5.8, and M6.1–M6.2.

**Required checks:** Match deterministic manifest/store inspection; fail before BootState changes on missing objects or invalid release; validate select/rollback traces against M5.6 models; deny update operations to unprivileged components.

**Exit condition:** Native components inspect, stage, select, and roll back generations using only manifest-declared authority.

### M6.6 — Powerbox file dialog service

**Depends on:** M6.3.

A general graphical UI is outside this slice.

**Required checks:** Return exactly the selected object and declared rights; never exceed chooser authority; mint nothing on cancellation; prevent requester bypass.

**Exit condition:** A selection gesture grants one otherwise-unreachable object capability and no broader directory authority.

### M6.7 — Generation sync and transfer

**Depends on:** M5.8 and M6.5.

QEMU uses a second attachable virtio block disk; networking is outside this slice.

**Required checks:** Produce byte-identical manifests; fail incomplete closure or authorization mismatch before staging pending BootState and without consuming an attempt; promote only after health confirmation; leave every ungranted device byte-identical.

**Exit condition:** An authorized QEMU-built generation transfers to a second machine and activates with grants and state policy intact.
