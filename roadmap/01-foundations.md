# Foundations and implemented history (Milestones 1–6)

**Purpose:** Preserve the implemented kernel, isolation, bootstrap, storage/generation, and native-environment contracts that later roadmap tracks build on.

**Status:** Milestones 1–4 and 6 are complete. Milestone 5 is open only because M5.7 still lacks the required physical Framework observation; M5.1–M5.6c, M5.8, and M5.9 are complete, and the M5.7 implementation and QEMU checks are complete.

**Dependencies:** M1 has no roadmap prerequisite. Each later milestone consumes the mechanisms and invariants established before it. M6 consumes M5 storage, generation, state, rollback, release-trust, and recovery mechanisms, but does not depend on M5.7 physical verification. Future tracks must preserve capability-only authority, component isolation, immutable generation semantics, deterministic bounds, and the distinction between QEMU evidence and physical evidence.

## Invariants carried forward

- The kernel supplies privileged mechanism, not service policy: address spaces, memory objects, capability enforcement, IPC, scheduling, interrupts, timers, and minimal platform control.
- Components receive only manifest-declared capabilities. Transfers and derivations can narrow authority but never widen it.
- Protocols and persistent formats are versioned, bounded, deterministically encoded, and reject unknown required versions or flags.
- Executable objects are immutable and content-verified before execution. Boot state, generation roots, and state roots remain separate concepts.
- QEMU establishes deterministic logic, bounded error handling, and fault recovery. It does **not** establish Framework firmware behavior, physical DMA containment, device identity, power-loss behavior, or absence of writes to internal hardware.
- **Internal Framework NVMe writes remain disabled.** Neither recovery media nor a storage-aware generation receives internal-NVMe write authority by default. No such write path may be enabled until all M5.7 promotion gates and the later hardware reliability gate have physical evidence.

## M1 — Kernel foundation

**Status:** Complete; the QEMU tests pass.

**Depends on:** None.

**Delivered:** Deterministic exception/crash reporting, physical and virtual memory management, kernel allocation, APIC/timer support, and architecture boundaries usable for QEMU and Framework bring-up.

**Required checks:** Exercise invalid mappings and faults and require a bounded diagnostic rather than a silent hang.

**Exit condition:** Invalid mappings and faults are reported deterministically.

## M2 — Isolation and IPC

**Status:** Complete; the core QEMU exit passes.

**Depends on:** M1.

**Delivered:** Userspace mode, independent address spaces, preemptible tasks, kernel-object capability tables, channels, shared-memory transfer, timeouts, cancellation, and peer-death notification.

**Required checks:** Two userspace components communicate; faulting one does not corrupt the other component or the kernel.

**Exit condition:** Isolated userspace communication and fault containment are observable under QEMU.

## M3 — Bootstrap component graph

**Status:** Complete; the QEMU vertical slice passes.

**Depends on:** M1–M2.

**Delivered:** Boot-object loading, versioned manifest decoding, init/service management, console, `sysinfo`, and the `echo-agent` stub component.

**Required checks:** Boot the manifest-defined graph and exercise the console-to-component vertical slice.

**Exit condition:** The first isolated component vertical slice runs under QEMU.

## M4 — Framework safe bring-up

**Status:** Complete and verified.

**Depends on:** M1–M3.

**Delivered:** UEFI/GOP console, ACPI discovery, keyboard input, timer and shutdown/reboot paths, and removable-media boot with internal NVMe access disabled.

**Required checks:** Run the isolated userspace slice from removable media without modifying internal storage.

**Exit condition:** The same isolated slice used under QEMU runs on Framework while preserving the no-internal-storage-write boundary. This is historical M4 evidence only; it does not claim that later storage-aware slices, NVMe writes, or later hardware peripherals have been physically verified.

## M5 — Storage and generations

**Status:** Open only on M5.7 physical verification. M5.1–M5.6c, M5.8, and M5.9 are complete. M5.7's implementation and QEMU evidence are complete.

**Depends on:** M1–M4.

**Authority and safety boundary:** Storage clients use a block-service endpoint plus bounded shared memory. A trusted driver receives explicit PCI-function, DMA-memory, interrupt, and shared-memory capabilities. The kernel enforces rights, mappings, DMA-buffer lifetime, and interrupt delivery; the userspace service owns partition policy, retries, and access control. There are no ambient block syscalls or guessed global device names. Before IOMMU enforcement, DMA-capable drivers remain trusted and writes are restricted to deterministic QEMU fixtures or dedicated replaceable test devices.

### M5.1 — Storage capability foundation

**Status:** Complete.

**Depends on:** M2 and M4 platform/capability foundations.

**Delivered:** Bounded ACPI MCFG and PCI enumeration; validated PCI capability chains and BARs; rights-checked PCI, DMA, interrupt, and shared-memory capabilities; DMA pinning through completion/reset; bounded block request/reply IPC with shared-memory payloads; and the allowlist-based `scripts/check-no-storage-authority.py` gate.

**Required checks:** Reject missing, widened, duplicate, stale, out-of-range, and wrong-kind capabilities; prevent reclaim of in-flight DMA buffers; reject malformed PCI metadata without hanging.

**Verification target:** `just storage_cap_check` (`kernel/tests/storage_capability.rs`).

**Exit condition:** An isolated driver receives only explicitly granted generic resources, and an unprivileged component cannot acquire device rights.

### M5.2a — Typed IPC schemas

**Status:** Complete.

**Depends on:** M5.1's block protocol.

**Delivered:** A versioned Zutai block schema and generated kernel Rust/component GNU assembler bindings. Bounds and unknown versions are rejected structurally; bindings are kept deterministic and byte-identical.

**Required checks:** Round-trip every message type byte-identically on both ends and reject stale bindings and out-of-bounds fields.

**Verification target:** `just contracts_check`.

**Exit condition:** The M5.2 block protocol has one schema-first layout under `contracts/`, with no disagreeing hand-written representation.

### M5.2 — Read-only virtio block vertical slice

**Status:** Complete.

**Depends on:** M5.1 and M5.2a.

**Delivered:** Modern virtio PCI negotiation with an explicit feature subset, bounded virtqueue ownership, bounded read-only sector requests, a fixed disposable fixture, and a capability-gated userspace probe that verifies a known SHA-256 digest. Writes remain structurally disabled in this slice.

**Required checks:** Verify the known sector through the full component/capability path; reject writes, out-of-range LBAs, short buffers, invalid descriptors, unsupported features, and timeouts with structured errors; contain driver failure; keep the generation slice healthy.

**Verification target:** `just storage_read_check`.

**Exit condition:** A userspace component reads and verifies a read-only QEMU virtio device without ambient storage authority.

### M5.3 — Durable virtio writes and fault handling

**Status:** Complete.

**Depends on:** M5.2.

**Delivered:** Separate explicit write authority; bounded writes, flush, completion, timeout, and reset; ownership recovery on all paths; fresh-boot durability checks; deterministic injection of request, timeout, reset, flush, and interrupted-write faults; and bounded IPC flight-recorder replay.

**Required checks:** Verify write/read-back and fresh-boot persistence; leave images unchanged on out-of-bounds writes; reclaim descriptors and pins after errors; never report failed flush as durable; never reuse stale completions after reset.

**Verification targets:** `just storage_write_check`; `just storage_fault_check`.

**Exit condition:** Disposable QEMU images provide bounded, explicitly authorized durable writes and deterministic recovery from injected failures.

### M5.4 — GPT and integrity-checked object store

**Status:** Complete.

**Depends on:** M5.3.

**Delivered:** Protective MBR and redundant GPT validation; capability-selected partitions; bounded versioned immutable object records addressed by content; append/seal commits; and redundant checksummed superblocks preserving an older valid root. QEMU evidence covers retrieval, durable append, damaged-newest-root fallback, conflicting GPT copies, and no-valid-superblock rejection. GPT recovery and interruption boundaries are additionally pinned by `kernel/tests/object_store.rs`; firmware may repair a primary GPT before QEMU guest code sees it, so that recovery remains unit-test evidence rather than QEMU evidence.

**Required checks:** Validate GPT bounds and CRCs; resolve copy damage without accepting conflicting valid copies; prevent malformed metadata from causing out-of-bounds I/O; verify complete payload hashes before use; preserve the previous root at every append/commit interruption; reject overlap, overflow, truncation, bad hashes, conflicting identities, and unsupported versions.

**Verification target:** `just storage_store_check`.

**Exit condition:** QEMU retrieves immutable content-addressed objects from a bounded GPT partition while malformed and partial commits fail closed.

### M5.5 — Generation format and BootState records

**Status:** Complete.

**Depends on:** M5.4 and the earlier generation contract.

**Delivered:** A new deterministic boot-time generation version encoding target, parent, dependencies, state bindings, health policy, and real kernel identity; explicit count/length bounds; canonical serialization; two fixed-size versioned `BootState` slots; older-slot-first updates; and immutable stage-0 selection and full kernel-bearing generation verification.

**Required checks:** Require byte-identical artifacts from normalized inputs; reject unknown versions/flags, excessive counts, oversized strings, broken parents, and bad checksums; execute nothing before verification; retain selection through one interrupted or invalid slot.

**Verification target:** `just generation_check`.

**Exit condition:** Stage-0 deterministically selects and verifies a complete kernel-bearing generation from redundant persistent metadata.

### M5.6a — Checked BootState transition model

**Status:** Complete.

**Depends on:** M5.5.

**Delivered:** `contracts/bootstate/model/bootstate.zt`, modeling both slots, older-slot-first commits, the six transition rules, and interruption witnesses. `SelectableBootRootExists` preserves a bootable root; `PendingAttemptConsumedBeforeTransfer` requires durable decrement before transfer. The nine concrete witnesses cover pending metadata, slot writes A/B, pending commit, attempt commit, health promotion, rollback update, state snapshot, and garbage collection.

**Required checks:** Exhaust the bounded interleavings and ensure a deliberate skipped-attempt mutation fails.

**Verification target:** `just bootstate_model_check`.

**Exit condition:** CI maintains a checked transition contract; implementation-semantic changes update the model with the implementation.

### M5.6b — Checked generation, state, and GC transaction model

**Status:** Complete.

**Depends on:** M5.6a.

**Delivered:** Graph-level snapshot epochs pairing a generation with a complete state set; explicit `immutable`, `ephemeral`, `preserve`, `snapshotBeforeUpgrade`, and `discardOnRollback` semantics; complete known-good, pending, running, rollback, staged-transaction, and persistent-state GC roots; interruption/recovery transitions; and idempotent rollback/restart/GC.

**Required checks:** Never mix snapshot epochs or expose incomplete state; never collect a reachable sealed object; ensure omitted-root and mixed-epoch mutations fail.

**Verification target:** `just bootstate_model_check`.

**Exit condition:** Every modeled upgrade, snapshot, promotion, rollback, and GC interleaving retains a bootable generation with a consistent state set.

### M5.6 — Pending, known-good, rollback, state policy, and GC

**Status:** Complete.

**Depends on:** M5.6a and M5.6b; their semantics are the implementation contract.

**Delivered behavior:** With no pending generation, boot known-good. With pending attempts, durably decrement before transfer. Only the capability-authorized generation-management service may confirm the currently running pending generation. Confirmation promotes it atomically and retains the prior known-good rollback root. Failure, reboot, or exhaustion returns to known-good. No transition overwrites the only valid slot. State policies and GC honor every root modeled by M5.6b, and rollback is idempotent.

**Required checks:** Inject interruption before pending metadata, during either slot write, after pending commit, after attempt commit/before transfer, during promotion, rollback update, state snapshot, and GC. Every reboot selects either pending with the correct reduced attempt count or verified known-good, never zero roots. Distinguish component exit, fault, timeout, peer loss, and explicit unhealthy status; deny health confirmation to unprivileged components.

**Verification target:** `just rollback_check` (including the observed `2 → 1 → 0` failing-pending sequence); state and GC behavior is exercised by `kernel/tests/generation_manager.rs`.

**Exit condition:** A failing pending generation automatically returns to verified known-good with persistent state and roots matching declared policy.

### M5.6c — BootState model/implementation conformance

**Status:** Complete.

**Depends on:** M5.6a, M5.6b, and M5.6.

**Delivered:** Bounded version-1 durable-transition records at stage-0 attempt commits and exhausted-known-good selection, validated against the checked models. The schema pins a worst-case-tested 640-byte line bound.

**Required checks:** Accept every rollback scenario trace; reject transfer before durable decrement, mismatched action/commit or sequence boundaries, wrong-root promotion/collection, and any unbounded instrumentation dependency.

**Verification target:** `just bootstate_trace_check`.

**Exit condition:** Durable transitions observed in QEMU conform to the checked state machine, and deliberately invalid traces fail validation.

### M5.7 — Framework NVMe transport and safety promotion

**Status:** Implementation complete; QEMU checks pass; required physical Framework evidence is pending. This is the only open M5 item.

**Depends on:** M5.1–M5.6c and M4 removable-media safety.

**Delivered in implementation/QEMU:** Bounded controller and namespace discovery, queue setup, timeout/reset handling, and read-only I/O over the common block protocol. The removable image has no internal-NVMe write path.

**Still required:** Observe and record a removable-media Framework boot of the storage-aware isolated slice without modifying internal NVMe. Destructive writes and interruption experiments may run only on a dedicated replaceable external test device.

**Promotion gates before any internal NVMe write may be enabled:** deterministic bounds and malformed-command tests; DMA isolation suitable for the physical target; timeout/reset recovery; flush ordering and durable-write tests; interrupted metadata and generation-transition tests; malformed GPT/object-store/generation/BootState tests; an explicit write capability held only by the intended service; and an operator-visible distinction between removable test media and internal NVMe. Production IOMMU enforcement and internal-disk promotion remain a later hardware reliability gate.

**Verification targets:** `just storage_nvme_read_check`; `just framework_safety_check`; plus the pending physical evidence record, which cannot be replaced by QEMU.

**Exit condition:** A physical Framework runs the storage-aware isolated slice over the common protocol while internal NVMe writes remain disabled unless every physical promotion gate has been observed. No such physical success is claimed yet.

### M5.8 — Signed generation release metadata

**Status:** Complete.

**Depends on:** M5.5–M5.6c.

**Delivered:** Bounded deterministic detached metadata; pinned 2-of-3 Ed25519 authorization; dual-threshold consecutive trust-root rotation; target, parent, sequence, kernel, and authority-manifest binding; staging without sequence advance; advance only after health promotion; and retained local known-good rollback.

**Required checks:** Reject insufficient threshold, missing/duplicate/malformed/excessive signatures, wrong target, stale releases, skipped rotation, or broken old/new-root continuity; preserve the accepted sequence after failed pending boots and preserve the explicit rollback root after promotion.

**Verification target:** `just release_trust_check`.

**Exit condition:** Stage-0 and generation management accept only authorized releases while retaining automatic local rollback.

**Scope limit:** This evidence does not establish trusted-time freeze protection, UEFI Secure Boot, TPM sealing, or resistance to rollback of an entire physical disk image.

### M5.9 — Recovery, scrub, and BootState reconstruction

**Status:** Complete.

**Depends on:** M5.4–M5.8.

**Delivered:** Fail-closed behavior when both slots are invalid; a signed removable recovery generation; capability-selected scrub of object records, superblocks, generation/state closure, and release authorization; reconstruction of both slots from verified roots; explicit `GenerationControl` and selected-target block authority; and idempotent interrupted reconstruction. Internal-NVMe write authority is absent by default.

**Required checks:** Never execute an unverified object after dual-slot corruption; reconstruct one disposable QEMU disk while a second ungranted disk remains byte-identical; reject missing state, broken closure, unauthorized releases, and interrupted incomplete reconstruction; retain the Framework removable-media no-write gate.

**Verification target:** `just recovery_check`.

**Exit condition:** Signed removable recovery reconstructs a verified bootable root without modifying any device not named by an explicit capability.

### M5 acceptance and verification stack

Repository gates retained by each accepted slice:

- `just contracts_check`
- `just generation_check`
- `just test`
- `just fmt_check`
- `just lint`

Slice checks, including all existing target names:

- `just storage_cap_check`
- `just bootstate_model_check`
- `just storage_read_check`
- `just storage_write_check`
- `just storage_fault_check`
- `just storage_store_check`
- `just rollback_check`
- `just bootstate_trace_check`
- `just storage_nvme_read_check`
- `just framework_safety_check`
- `just release_trust_check`
- `just recovery_check`

M5 closes only after M5.7's physical observation. At that point all executable content must verify before execution; staging must preserve running/known-good; attempts must commit before transfer; confirmation must apply only to the running pending generation; interruption must preserve a valid slot; checked traces must match the state/GC models; failure must return to known-good; GC must preserve every retained root; every state policy must have upgrade/rollback evidence; read/write authority must be explicit; malformed metadata must fail before out-of-bounds I/O or execution; releases and recovery must preserve local rollback and capability isolation; and the Framework storage-aware observation must show no unauthorized internal-NVMe write.

## M6 — Native interactive environment

**Status:** Complete; M6.1–M6.7 are done.

**Depends on:** M1–M5 storage, object-store, generation, state-policy, rollback, release-trust, and recovery mechanisms. M6 acceptance is QEMU/removable-media evidence and remains independent of pending M5.7 physical verification. Internal NVMe writes remain disabled.

**Sequencing preserved:** M6.1 gated all other slices. M6.2 and M6.3 could proceed independently after it; M6.4 consumed both. M6.5 and M6.6 were independent. M6.7 consumed M6.5 and M5.8.

### M6.1 — Kernel spawn prerequisites and generation format v2

**Status:** Complete.

**Depends on:** M5 generation/capability contracts.

**Delivered:** Bounded factory-authorized userspace endpoint minting; non-consuming narrow-only derive-copy grants; per-spawner resource accounting with kernel-stack/heap rebudgeting; supervision handles distinguishing exit/fault/timeout/peer loss; and deterministic generation format v2. Manifest rights map directly to capability bits, `transferable` maps to `RIGHT_TRANSFER`, `RIGHT_SPAWN` is enforced, and bootstrap wiring is manifest-derived rather than component-name hardcoded.

**Required checks:** Deny gifting absent rights and all widening; return structured exhaustion per spawner without harming others; bound endpoint tables; preserve distinct supervision outcomes; produce byte-identical v2 artifacts and reject unknown versions; source test bootstrap grants only from manifests.

**Check targets:** `just generation_check`; `just contracts_check`; common `just test` coverage (no separate slice target is named in the source roadmap).

**Exit condition:** A factory-authorized spawner mints bounded endpoints, gifts narrowed copies, and supervises children within its budget using deterministic manifest wiring.

### M6.2 — Spawn service and command profile

**Status:** Complete.

**Depends on:** M6.1.

**Delivered:** Versioned `contracts/spawn/v1` request/reply schema; a generation-authorized userspace spawn service; deterministic manifest command profiles mapping names to executable capabilities; explicit arguments, environment, optional directory, streams, and grants; and per-client accounting. There are no global executable paths or implicit working directories.

**Required checks:** Byte-identical schema round trips and bounded-version rejection; deny undeclared executables, budget excess, and code injection; deterministic resolution; contain spawn-service failure.

**Verification target:** `just spawn_service_check`.

**Exit condition:** A client resolves a profile command and launches it with exactly the declared grants.

### M6.3 — Filesystem service and directory capabilities

**Status:** Complete.

**Depends on:** M6.1 and M5.4.

**Delivered:** A capability-matrix-defined Directory object and rights; bounded `contracts/fs/v1` operations; immutable directory snapshots and explicit root transitions over the object store; and narrow-only directory derivation/transfer.

**Required checks:** Deny name access without a directory capability; restrict derivation by subdirectory and rights; preserve the previous root across interruption; enforce path, entry-count, and depth bounds before store I/O.

**Verification target:** `just directory_check`.

**Exit condition:** Components browse and mutate namespaces only through explicit directory capabilities with store-verified metadata.

### M6.4 — Minimal Dango implementation and core runtime

**Status:** Complete.

**Depends on:** M6.2 and M6.3.

**Delivered:** A native bounded command-subset parser/interpreter and console REPL; `$(...)` launches through profile resolution and the spawn service; explicit `with-env`, `with-cwd`, and `with-stdin` contexts; structured exit/fault/timeout/peer-loss/revocation mapping; and deterministic scripted QEMU sessions. Full Hindley–Milner, row-polymorphism, and effect inference are outside this slice.

**Required checks:** Trace each launch to profile resolution and a spawn request; leak no ambient context; preserve termination distinctions at the language boundary; reproduce scripted sessions deterministically.

**Verification target:** `just dango_check`.

**Exit condition:** Native console commands run through Dango with capability-resolved authority and structured failures.

### M6.5 — Generation inspection and update commands

**Status:** Complete.

**Depends on:** M5.6c, M5.8, and M6.1–M6.2.

**Delivered:** Native list, inspect, stage, select, and rollback components using a versioned generation-management service; manifest-scoped `BOOT_UPDATE` granted only to that service; closure/release validation before staging; and activation that never overwrites the running generation or advances accepted sequence during staging.

**Required checks:** Match deterministic manifest/store inspection; fail before BootState changes on missing objects or invalid release; validate select/rollback traces against M5.6 models; deny update operations to unprivileged components.

**Verification target:** `just generation_cmd_check`.

**Exit condition:** Native components inspect, stage, select, and roll back generations using only manifest-declared authority.

### M6.6 — Powerbox file dialog service

**Status:** Complete.

**Depends on:** M6.3.

**Delivered:** A console chooser holding directory authority the requester lacks; versioned `contracts/powerbox/v1`; purpose and requested-rights input; narrow-only, single-object capability mint/transfer on user selection; cancellation; and a provenance event. A general graphical UI is outside this slice.

**Required checks:** Return exactly the selected object and declared rights; never exceed chooser authority; mint nothing on cancellation; prevent requester bypass.

**Verification target:** `just powerbox_check`.

**Exit condition:** A selection gesture grants one otherwise-unreachable object capability and no broader directory authority.

### M6.7 — Generation sync and transfer

**Status:** Complete.

**Depends on:** M5.8 and M6.5.

**Delivered:** A deterministic versioned transfer manifest; closure construction respecting state policy (`preserve` and `snapshotBeforeUpgrade` travel, `ephemeral` does not, `immutable` travels read-only); content-identity set-difference transfer; receiver-side closure/release verification; and ordinary pending-attempt/health-confirm activation. QEMU uses a second attachable virtio block disk; networking is outside this slice.

**Required checks:** Produce byte-identical manifests; fail incomplete closure or authorization mismatch before transfer of control and without consuming an attempt; promote only after health confirmation; leave every ungranted device byte-identical.

**Verification target:** `just transfer_check`.

**Exit condition:** An authorized QEMU-built generation transfers to a second machine and activates with grants and state policy intact.

### M6 acceptance and verification stack

Repository gates:

- `just contracts_check`
- `just generation_check`
- `just test`
- `just fmt_check`
- `just lint`

Slice targets:

- `just spawn_service_check`
- `just directory_check`
- `just dango_check`
- `just generation_cmd_check`
- `just powerbox_check`
- `just transfer_check`

M6 is complete because its spawn, filesystem, powerbox, generation-management, and transfer protocols are versioned and checked; endpoint minting, derive-copy, accounting, and supervision are bounded; executable and directory authority are capability-only; native generation operations conform to checked BootState models; the powerbox grants only the selected object; authorized transfer preserves grants/state policy; and the isolated graph plus M5 QEMU targets remain healthy.

## Handoff to future tracks

Continue with the [roadmap index](README.md) and the independent future tracks:

- [Core runtime](02-core-runtime.md)
- [ROS 2 compatibility](03-ros2-compatibility.md)
- [Platform and hardware](04-platform-hardware.md)
- [Foreign workloads](05-foreign-workloads.md)
- [Authority and trust](06-authority-trust.md)

Later roadmap tracks may extend these mechanisms but must not weaken them. In particular, new drivers and schedulers remain capability-mediated and bounded; new persistent formats remain deterministic and versioned; generation activation continues through checked BootState transitions; physical claims require physical records; and internal Framework NVMe writes remain gated rather than inferred from QEMU success.
