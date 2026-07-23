# Slime OS Roadmap

This document is the canonical milestone plan for Slime OS. It tracks architectural progress from the QEMU kernel foundation to the Framework daily-driver target.

Completion requires observable behavior, not only compiled code or framebuffer output. QEMU is the deterministic architecture target; physical-machine claims additionally require an observed removable-media Framework boot under the repository's storage-safety rules.

## Status summary

| Milestone | Status | Exit condition |
| --- | --- | --- |
| 1. Kernel foundation | QEMU tests passing | Invalid mappings and faults are reported deterministically rather than silently hanging. |
| 2. Isolation and IPC | Core QEMU exit passing | Two userspace components communicate, and one may fault without corrupting the other or the kernel. |
| 3. Bootstrap component graph | QEMU vertical slice passing | The first isolated userspace vertical slice works under QEMU. |
| 4. Framework safe bring-up | Verified | The same isolated userspace slice runs from removable media without modifying internal storage. |
| 5. Storage and generations | In progress — M5.1 through M5.6c, M5.8, and M5.9 complete; M5.7 physical verification pending | A failed pending generation automatically leaves or restores a bootable known-good generation. |
| 6. Native interactive environment | Done — M6.1 through M6.7 complete | Native components inspect, stage, select, roll back, and transfer generations through explicit capabilities. |
| 7. Daily-driver hardware | Planned — M7.1 through M7.14 defined; physical keyboard unavailable | The Framework target supports the hardware and lifecycle needed for daily use. |
| 8. Foreign-workload authority foundations | Not yet implemented | Revocation, scheduling class, and secrets exist as non-ambient, auditable, rollbackable authority so foreign and agent workloads can be confined. |
| 9. Compatibility route | Not yet implemented | A Linux binary declared as a container runs under the personality, confined to its declared grants and denied everything else with a normal errno. |
| 10. Accelerator compute authority | Not yet implemented | Inference/compute submission is a rights-gated, budgeted, IOMMU-contained capability visible in the manifest. |
| 11. Physical trust and attestation | Not yet implemented | Reflashing an older generation fails stage-0 verification against TPM-held counters; the running generation identity is remotely attestable. |
| 12. Distributed capabilities | Not yet implemented | A grant proxied to a service on another machine stays unforgeable, non-ambient, and revocable across the wire. |

## Architectural constraints

Every milestone must preserve these project invariants:

- The kernel owns only privileged mechanisms: scheduling, address spaces, memory objects, capability enforcement, IPC, interrupts, timers, and minimal platform control.
- Device, filesystem, generation, health, activation, and rollback policy belongs in userspace services.
- Authority is carried by explicit capabilities. There are no ambient executable paths, storage handles, working directories, streams, or environment state.
- The kernel object-by-rights surface, the rules for extending it, and the planned authority horizon are fixed in `docs/capability-matrix.md`; a new object or right updates the matrix in the same change.
- Generation and storage formats are deterministic, versioned, bounded, integrity checked, and explicitly rejected when malformed or unsupported.
- Activation never overwrites the running generation in place.
- No physical-machine support claim is complete without observed hardware behavior.
- Internal Framework NVMe writes remain disabled until the required bounds, DMA, timeout/reset, flush-ordering, interrupted-write, and malformed-metadata checks pass.

## Milestone 1: Kernel foundation

**Status:** QEMU tests passing.

Scope:

- exception and crash reporting;
- physical and virtual memory management;
- kernel allocation;
- APIC/timer support;
- architecture boundaries suitable for QEMU and Framework bring-up.

Exit condition: invalid mappings and faults are reported deterministically rather than silently hanging.

## Milestone 2: Isolation and IPC

**Status:** Core QEMU exit passing.

Scope:

- userspace mode and independent address spaces;
- preemptible tasks;
- kernel object and capability tables;
- channels, shared-memory transfer, timeouts, cancellation, and peer-death notification.

Exit condition: two userspace components communicate, and one may fault without corrupting the other or the kernel.

## Milestone 3: Bootstrap component graph

**Status:** QEMU vertical slice passing.

Scope:

- boot object loading;
- versioned manifest decoding;
- init/service management;
- console, `sysinfo`, and `echo-agent` stub components.

Exit condition: the first vertical slice works under QEMU.

## Milestone 4: Framework safe bring-up

**Status:** Verified.

Scope:

- UEFI/GOP console;
- ACPI discovery;
- keyboard input;
- timer and shutdown/reboot paths;
- removable-media boot with internal NVMe access disabled.

Exit condition: the same isolated userspace slice runs on the Framework without modifying internal storage.

## Milestone 5: Storage and generations

**Status:** In progress. M5.1 through M5.6c and M5.8 are complete; Framework NVMe safety promotion (M5.7) and bounded recovery (M5.9) remain.

Top-level scope:

- virtio block, followed by the Framework NVMe transport;
- GPT and an integrity-checked object store;
- immutable generations;
- pending and known-good boot state;
- rollback and garbage-collection roots;
- explicit persistent-state policy;
- checked conformance between boot-state models and implementation traces;
- signed generation release trust and bounded recovery.

Exit condition: a failed pending generation automatically leaves or restores a bootable known-good generation, while selection, state roots, garbage collection, release authorization, and recovery remain verifiable across interruptions.

### Current baseline

The repository already provides:

- a deterministic generation source contract in `contracts/generation/v1/`;
- a host builder and checker for one `generation.bin`;
- kernel decoding with whole-manifest and per-object SHA-256 validation;
- one Limine-loaded generation module;
- isolated userspace components, IPC endpoints, capability transfer, and structured termination;
- QEMU test execution through `kernel/scripts/run-kernel.sh`;
- Framework removable-media image and write-safety tooling;
- ACPI MCFG parsing and bounded PCI enumeration with capability-chain and BAR validation;
- rights-checked capabilities for PCI functions, DMA memory, interrupts, and shared memory, with DMA pinning guarded against reclamation while a request is outstanding;
- a bounded block request/reply IPC protocol with payloads in shared memory;
- an allowlist-based `scripts/check-no-storage-authority.py` proving no component receives ambient disk-write authority;
- a bounded live-task table, spawn grants that enforce the same transfer-right condition as IPC sends, and capability tables that reject rights meaningless for the object kind (`kernel/tests/spawn_authority.rs`);
- the `storage_cap_check` QEMU target (`kernel/tests/storage_capability.rs`).

The remaining gaps include:

- no virtio transport or device backend behind the block protocol;
- no GPT or persistent object store beyond the QEMU object-store slice (M5.4): the store is not yet wired into generation loading, staging, or boot-state selection;
- no persistent boot-state record;
- no pending/known-good selection, health promotion, rollback, or GC implementation;
- generation source fields such as `parent`, state policy, health policy, and component dependencies are not represented in the current boot-time binary;
- the current boot path always loads one fixed `generation.bin`.

### Storage authority model

Do not add global block-device syscalls such as `SYS_BLOCK_READ` or expose a guessed device name. The intended data path is:

```text
client component
  -> block-service endpoint capability
  -> bounded shared-memory capability
  -> trusted virtio-blk or NVMe driver component
  -> PCI function, DMA-memory, and interrupt capabilities
  -> device
```

The kernel enforces capability rights, mappings, DMA-buffer lifetime, and interrupt delivery. The userspace block service owns request policy, partition selection, retries, and access control.

Before IOMMU enforcement exists, DMA-capable driver components remain part of the trusted computing base. This interim path is acceptable only for deterministic QEMU images and dedicated test devices; it does not authorize writes to the Framework's internal NVMe.

### M5.1: Storage capability foundation

**Status:** Complete. The exit condition is observed by the `storage_cap_check` QEMU target (`kernel/tests/storage_capability.rs`): an unprivileged component cannot acquire device rights.

Deliverables:

- parse ACPI MCFG and enumerate bounded PCI segment/bus/device/function ranges;
- validate PCI capabilities and BAR sizes before mapping MMIO;
- introduce generic, rights-checked capabilities for PCI functions, DMA memory, interrupts, and shared memory;
- pin DMA pages for the complete device operation and reclaim them only after completion or reset;
- define a bounded block request/reply protocol over IPC;
- keep payload data in shared memory rather than increasing IPC messages into an unbounded data plane;
- evolve `scripts/check-no-storage-authority.py` from “no storage mechanisms exist” to an allowlist proving that no component receives ambient disk-write authority.

Required checks:

- a component without the required capability cannot map device registers, DMA memory, or shared buffers;
- rights cannot be widened during capability transfer;
- duplicate, stale, out-of-range, and wrong-object handles are rejected;
- DMA buffers cannot be reclaimed while a request is outstanding;
- malformed PCI capability chains and BAR declarations are rejected without hanging.

Exit condition: an isolated driver service can receive only explicitly granted generic device resources, while an unprivileged component cannot access them.

Follow-up (not an M5.1 exit requirement): capability transfers should eventually record a provenance link (granting component, transferred rights, originating grant) so that authority chains can be reconstructed for auditing. The capability table introduced here is the natural place to attach it.

### M5.2a: Typed IPC schemas

**Status:** Complete. The versioned Zutai block schema generates both kernel Rust and component GNU assembler bindings; `contracts_check` rejects stale or invalid bindings, and QEMU tests cover byte-identical round trips, bounds, and unknown versions.

This slice precedes or runs in parallel with M5.2. It is deliberately early: every later protocol, interposition tool, and agent tool-call surface gets cheaper once message contracts are schema-first.

Deliverables:

- declare the block request/reply protocol (M5.1) as Zutai types in `contracts/`;
- generate kernel-side and component-side bindings from the schema, or validate hand-written bindings against it deterministically;
- version the schema; unknown versions and out-of-bounds fields are rejected structurally;
- document that new IPC protocols must be schema-first from this point on.

Required checks:

- the generated/validated bindings round-trip every message type byte-identically;
- a message violating declared bounds is rejected on both ends;
- `just contracts_check` covers the IPC schemas.

Exit condition: the block protocol used by M5.2 is defined by a versioned schema in `contracts/`, and no hand-written message layout disagrees with it.

### M5.2: Read-only virtio block vertical slice

**Status:** Complete. `storage_read_check` boots a disposable read-only virtio fixture; the capability-gated userspace probe verifies the known sector SHA-256 and structured write, short-buffer, and out-of-range rejection while the generation remains healthy.

Deliverables:

- implement the modern virtio PCI transport needed by `virtio-blk-pci`;
- negotiate only a small, explicit feature set and reject unsupported required features;
- implement a bounded virtqueue with deterministic descriptor ownership;
- support read-only sector requests with explicit LBA and buffer bounds;
- add a fixed QEMU block fixture containing known bytes and hashes;
- add a minimal userspace storage probe that requests a sector and verifies its SHA-256 digest;
- keep write operations disabled in this slice.

Required checks:

- the known sector is read and verified through the complete component/capability path;
- write requests against the read-only service are rejected structurally;
- out-of-range LBAs, short buffers, invalid descriptors, unsupported features, and request timeouts return structured errors;
- driver failure does not terminate unrelated components or the kernel;
- the existing generation vertical slice remains healthy.

Verification target:

```sh
just storage_read_check
```

The target should create a disposable fixture, attach it with `readonly=on`, boot QEMU, exercise the userspace request path, and require a successful guest exit.

Exit condition: a userspace component reads and verifies data from a read-only QEMU virtio block device without gaining ambient storage authority.

### M5.3: Durable virtio writes and fault handling

**Status:** Complete. `storage_write_check` verifies a flushed write after a fresh boot, and `storage_fault_check` covers deterministic request failure, timeout, reset, flush failure, interrupted write, bounded rejection, and flight-recorder replay.

Deliverables:

- add explicitly granted write authority separate from read authority;
- implement bounded writes, flush, completion, timeout, and device reset;
- ensure descriptor and DMA-buffer ownership is recovered after every success or failure path;
- persist a write to a disposable QEMU image and verify it after a fresh boot;
- add deterministic fault injection for request failure, timeout, reset, flush failure, and interrupted updates;
- record the IPC messages of the driver component during fault-injection runs, so a failing run can be re-executed deterministically from its recorded inputs (foundation for a general IPC flight recorder; replay of arbitrary components is out of scope here).

Required checks:

- write then read-back succeeds in one boot;
- the written bytes remain after a fresh QEMU boot;
- out-of-bounds writes leave the image unchanged;
- a failed or timed-out write reports an error and does not leak descriptors or pinned pages;
- a flush failure cannot be reported as durable success;
- a device reset cannot expose a stale completion as a new request's completion.

Planned verification targets:

```sh
just storage_write_check
just storage_fault_check
```

Exit condition: disposable QEMU block images support durable, bounded, explicitly authorized writes with deterministic recovery from injected device failures.

Follow-up (not an M5.3 exit requirement): the fault-injection recording added here is the hand-written instance of a schema-generated interposition membrane and IPC flight recorder (directions register entries 7 and 11); generalizing it consumes only the M5.2a contract tooling.

### M5.4: GPT and integrity-checked object store

**Status:** Complete. `storage_store_check` boots disposable GPT fixtures through the capability-gated `store-probe`: it resolves the object-store partition, retrieves the seeded content-addressed object with full-payload SHA-256 verification, appends and seals a new object durably across a fresh boot, falls back to the older superblock root when the newest is damaged, and rejects conflicting GPT copies and a no-valid-superblock store. GPT copy-recovery, overlap/overflow/CRC rejection, duplicate-identity conflicts, and interruption at every append/commit boundary are pinned by `kernel/tests/object_store.rs` (UEFI firmware auto-repairs a damaged primary GPT before the kernel runs, so damaged-header recovery is unit-tested rather than in QEMU).

Deliverables:

- validate protective MBR, primary and backup GPT headers, table bounds, and CRCs;
- select partitions only through explicit block-service capabilities;
- define a versioned, bounded object-record format containing content hash, type, length, and payload;
- store immutable objects by content identity;
- append and seal new objects without modifying existing object bytes;
- use redundant, checksummed metadata or superblocks so one interrupted metadata update does not destroy the previous valid root;
- reject overlapping partitions, integer overflow, truncated records, bad hashes, duplicate identities with different contents, and unsupported versions.

Required checks:

- valid primary and backup GPT copies resolve to the expected object-store partition;
- one damaged GPT copy can be recovered from the other, while conflicting valid copies are rejected or resolved by a documented rule;
- malformed metadata never causes an out-of-bounds device request;
- an object is not executable or mountable until its complete payload hash verifies;
- interruption at every object append/commit boundary preserves the previous committed root.

Exit condition: QEMU can open a bounded GPT partition and retrieve immutable, content-addressed objects while rejecting malformed or partially committed metadata.

### M5.5: Generation format and boot-state records

**Status:** Complete. `generation_check` proves canonical byte-identical generation and redundant BootState artifacts; host and QEMU checks reject malformed metadata; immutable stage-0 selects and hash-verifies the complete kernel-bearing generation before transfer; production and read-only-storage boots reach a healthy isolated userspace slice.

Deliverables:

- introduce a new boot-time generation binary version rather than changing required format-1 field meanings;
- encode target identity, parent generation, component dependencies, state bindings, health policy, and a real kernel object hash;
- define explicit upper bounds for object, component, grant, state, string, and payload counts and lengths;
- define one canonical serialization order so equivalent input produces identical bytes;
- introduce an independent, versioned `BootState` record containing at least:
  - monotonic sequence number;
  - known-good generation identity;
  - optional pending generation identity;
  - remaining pending attempts;
  - generation and state roots;
  - integrity checksum;
- store two fixed-size `BootState` slots and update the older slot first, committing validity only after required data and flushes complete;
- add a minimal immutable stage-0 boot selector capable of choosing and verifying the selected kernel and generation before control reaches that generation's kernel.

The stage-0 selector is required because userspace cannot roll back a kernel that has already been selected and loaded. A fixed kernel with userspace-only rollback is not sufficient for the complete generation contract.

Required checks:

- two builds from identical normalized input produce byte-identical generation and boot-state artifacts;
- unknown versions, unknown required flags, excessive counts, oversized strings, broken parent references, and bad checksums are rejected;
- the selector never executes a kernel or component object before its hash verifies;
- if one `BootState` slot is invalid or interrupted, the other valid slot remains selectable.

Exit condition: the boot path can deterministically select and verify one complete generation, including its kernel, from redundant persistent boot metadata.

Follow-ups enabled by this milestone (not exit requirements): an authority diff between two generations as a build-pipeline gate, and manifest grant-graph queries such as "which components can reach block-write" (directions register entries 1 and 9). Both are host-side tooling over the machine-readable grants introduced here.

### M5.6a: Checked BootState transition model

**Status:** Complete. `contracts/bootstate/model/bootstate.zt` is exhaustively checked by `zutai model-check` through `just bootstate_model_check`: `SelectableBootRootExists` preserves a bootable root, `PendingAttemptConsumedBeforeTransfer` enforces durable decrement-before-transfer, all nine concrete cut witnesses pass (before pending metadata, slot write A, slot write B, after pending commit, after attempt commit, health promotion, rollback update, state snapshot, and garbage collection), and the skip-attempt mutation is rejected.

Deliverables:

- encode the six M5.6 boot-state transition rules and the eight-point power-cut matrix as an ordinary pure, typed `.zt` model under `contracts/`;
- model both fixed-size `BootState` slots, the older-slot-first update rule, and interruption at every commit boundary;
- check that no modeled interleaving leaves zero bootable roots, and that a pending boot attempt is always persistently consumed before control transfer;
- run the model check from a repository target (planned: `just bootstate_model_check`) so the spec is a maintained contract artifact rather than a one-off proof.

Required checks:

- the checker explores the full interleaving set implied by the power-cut matrix over both slots;
- deliberately breaking a transition rule in the model (for example, skipping the attempt decrement) makes the check fail.

Exit condition: a checked model of the M5.6 transition rules lives in `contracts/` and is validated by CI; M5.6's implementation must not change a transition rule without updating the model in the same change.

### M5.6b: Checked generation, state, and GC transaction model

**Status:** Complete. The typed `.zt` model now pairs generations with complete graph-level state snapshot epochs, encodes all five state policies, protects known-good/pending/running/rollback/staged/persistent GC roots, explores interruption and repeated recovery transitions, and rejects omitted-root and mixed-epoch mutations through `just bootstate_model_check`.

Deliverables:

- extend the M5.6a model with graph-level snapshot epochs that pair one generation root with one complete set of state roots;
- encode `immutable`, `ephemeral`, `preserve`, `snapshotBeforeUpgrade`, and `discardOnRollback` as explicit transition semantics rather than implementation conventions;
- model known-good, pending, running, rollback, staged-transaction, and persistent-state roots as the complete GC root set;
- model interruption during snapshot creation, health promotion, rollback, and object or generation collection;
- require rollback, restart after interrupted transactions, and repeated GC transitions to be idempotent.

Required checks:

- every bootable generation references a complete, schema-consistent state set from one snapshot epoch;
- no modeled interruption can pair a generation with partially upgraded or mixed-epoch state;
- GC cannot collect a sealed object reachable from any retained root, including a staged transaction;
- deliberately omitting a root or allowing a mixed snapshot epoch makes the checker fail.

Exit condition: the checked model proves that every bounded upgrade, snapshot, promotion, rollback, and GC interleaving retains a bootable generation with a consistent state set and never collects a reachable object.

### M5.6: Pending, known-good, rollback, state policy, and GC

**Status:** Complete. `just rollback_check` boots a deliberately failing pending generation, durably consumes each attempt before transfer (2 → 1 → 0), and automatically returns to the verified known-good generation with unchanged known-good and pending identities. Health-confirmation authority is a `GenerationControl` capability minted only in the kernel bootstrap and transferred once to the declared generation-management service; unprivileged components cannot reach `SYS_HEALTH_CONFIRM`. State policies (`immutable`, `ephemeral`, `preserve`, `snapshotBeforeUpgrade`, `discardOnRollback`) and GC reachability over the known-good, pending, running, rollback, staged, and persistent-state roots are exercised under QEMU by `kernel/tests/generation_manager.rs`; `collect_unreachable` tests every retained root directly so no root is dropped and no reachable object is collected.

Prerequisites: M5.6a and M5.6b must land before implementation. Their checked transition, snapshot, state-policy, and GC semantics are the contract for this slice and must change in the same commit as any implementation-semantic change.

Boot-state transition rules:

1. With no pending generation, boot the known-good generation.
2. With a pending generation and attempts remaining, persistently decrement the attempt count before transferring control to it.
3. A privileged userspace health service may confirm only the currently running pending generation.
4. Confirmation atomically promotes pending to known-good and retains the previous known-good generation as a rollback root until policy permits collection.
5. Failure, reboot, or exhaustion without confirmation selects the previous known-good generation.
6. No transition may overwrite the only valid boot-state record.

Deliverables:

- stage an immutable generation without changing the running or known-good roots;
- grant health-confirmation authority only to the declared generation-management service;
- distinguish component exit, fault, timeout, peer loss, and explicit unhealthy status;
- implement state policies for `immutable`, `ephemeral`, `preserve`, `snapshotBeforeUpgrade`, and `discardOnRollback`;
- derive GC reachability from known-good, pending, currently running, rollback, staged transaction, and persistent-state roots;
- collect only sealed objects that are unreachable from every retained root;
- make rollback idempotent across repeated resets.

Required power-cut matrix:

- before pending metadata write;
- during each `BootState` slot write;
- after pending commit but before first boot;
- after attempt decrement but before kernel transfer;
- during health promotion;
- during rollback metadata update;
- during state snapshot creation;
- during object and generation GC.

Every injected interruption must reboot into either the pending generation with a correctly decremented attempt count or a verified known-good generation. It must never leave zero bootable roots.

Planned verification target:

```sh
just rollback_check
```

Exit condition: a deliberately failing pending generation automatically returns to a verified known-good generation, with persistent state and GC roots matching their declared policies.

Follow-ups enabled by this milestone (not exit requirements): generation bisect (automated boot-and-health-check search over the parent chain) and shadow boot (health-checking a pending generation in a constrained environment before consuming a real boot attempt), tracked as directions register entries 12 and 13. Both consume only mechanisms this milestone already requires.

### M5.6c: BootState model-implementation conformance

**Status:** Complete. `just bootstate_trace_check` boots the failing-pending rollback scenario, captures bounded version-1 transition records at stage-0's durable attempt commits and exhausted-known-good selection, and validates every finite trace against the checked M5.6a/M5.6b `.zt` state machine through `zutai model-check`. It rejects non-decremented transfers, mismatched action/commit or sequence boundaries, wrong-root promotions, and collection of observable retained roots; the fixed 640-byte line bound is schema-pinned and worst-case tested.

Deliverables:

- define a versioned, bounded transition-trace contract containing the selected slot, durable sequence, known-good and pending identities, attempts before and after, generation and state roots, action identity, and commit boundary;
- instrument stage-0 and the generation-management service only at durable state changes that correspond to model actions;
- emit traces from the rollback power-cut scenarios and validate each finite trace against the M5.6a/M5.6b state machines;
- keep trace validation in CI so model and implementation changes cannot drift independently.

Required checks:

- every `rollback_check` scenario produces a trace accepted by the checked models;
- a trace that transfers control before the attempt decrement is durable is rejected;
- traces that promote or collect against the wrong state root are rejected;
- trace instrumentation remains bounded and cannot become a new unbounded boot dependency.

Planned verification target:

```sh
just bootstate_trace_check
```

Exit condition: all durable BootState transitions observed in QEMU fault scenarios conform to M5.6a/M5.6b, and deliberately invalid implementation traces fail validation.

### M5.7: Framework NVMe transport and safety promotion

**Status:** Implementation complete; physical verification pending. `just storage_nvme_read_check` exercises bounded controller/namespace discovery and read-only I/O through the common block protocol under QEMU, while `just framework_safety_check` proves the removable image has no internal-NVMe write path. Completion still requires an observed removable-media Framework boot of the storage-aware slice without internal NVMe modification.

Deliverables:

- enumerate the target Framework NVMe controller through the same bounded PCI resource model;
- implement controller identify, namespace discovery, queue setup, timeout, reset, and read-only I/O first;
- reuse the block-service protocol rather than exposing NVMe-specific authority to clients;
- run destructive write and interruption tests only on a dedicated, replaceable external test device;
- preserve removable-media boot and the existing no-internal-write safety path;
- record an observed Framework boot of the storage-enabled userspace slice.

Promotion gates before any internal NVMe write can be enabled:

- deterministic bounds and malformed-command tests;
- DMA isolation appropriate for the physical target;
- timeout and controller-reset recovery;
- flush-ordering and durable-write tests;
- interrupted metadata and generation-transition tests;
- malformed GPT, object-store, generation, and boot-state tests;
- explicit write capability granted only to the intended storage service;
- an operator-visible distinction between the removable test device and internal NVMe.

Milestone 5 may establish the Framework NVMe transport and read-only path, but production-grade IOMMU-enforced DMA and internal-disk promotion remain part of the Milestone 7 reliability gate.

Exit condition: the Framework can run the storage-aware isolated userspace slice through the common block protocol, while internal NVMe writes remain disabled unless every physical safety promotion gate has been observed.

### M5.8: Signed generation release metadata

**Status:** Complete. `just release_trust_check` verifies bounded deterministic detached metadata, pinned 2-of-3 Ed25519 authorization, dual-threshold consecutive root rotation, malformed and stale release rejection, staging without sequence advancement, failed-pending local rollback, and health-confirmed promotion while retaining the prior known-good root. This does not claim trusted-time freeze protection, UEFI Secure Boot, TPM sealing, or resistance to rollback of the entire physical disk image.

Deliverables:

- define a deterministic, bounded, versioned detached release object naming the generation identity, target identity, parent, release sequence, kernel identity, and authority-manifest identity;
- pin an initial repository trust root independently from generation content and require threshold signatures for release metadata;
- pin one mandatory signature algorithm and canonical key/signature encoding for stage-0 verification;
- support bounded root-key rotation in which each new trust-root version is authorized by the required thresholds of both the previous and replacement trust sets;
- reject internally hash-consistent generations that lack valid release authorization;
- distinguish remote release replay protection from local safety rollback: staging does not advance the accepted release sequence, and promotion advances it only after userspace health confirmation while the retained known-good generation remains locally bootable.

Required checks:

- fewer than the configured threshold of compromised keys cannot authorize a release;
- missing, duplicate-key, malformed, excessive, wrong-target, and stale release metadata are rejected before staging;
- key rotation cannot skip a version or remove continuity with the previously trusted root;
- a failed pending generation returns to the retained known-good generation without advancing the accepted release sequence;
- advancing the sequence after promotion does not invalidate the explicitly retained local rollback root;
- this milestone does not claim trusted-time freeze protection, UEFI Secure Boot, TPM sealing, or resistance to rollback of the entire physical disk image.

Planned verification target:

```sh
just release_trust_check
```

Exit condition: stage-0 and generation-management code accept only correctly authorized releases while preserving automatic local rollback to the retained known-good generation.

### M5.9: Recovery, scrub, and BootState reconstruction

**Status:** Complete. `just recovery_check` boots a signed removable recovery generation, scrubs a capability-selected disposable target, reconstructs both redundant `BootState` slots from verified generation, state, and release roots, and proves a second ungranted disk remains byte-identical. Recovery images grant no internal NVMe write authority by default.

Deliverables:

- fail closed without executing generation objects when neither `BootState` slot is valid;
- boot a signed recovery generation from removable media without granting ambient access to internal storage;
- scrub object records, superblocks, generation closure, state-root closure, and release authorization before offering repair;
- reconstruct redundant `BootState` slots only from complete, verified generation and state roots;
- give the recovery component explicit `GenerationControl` and block-device capabilities for the selected repair target, with internal NVMe write authority absent by default;
- make interrupted reconstruction idempotent and preserve the last valid repair result.

Required checks:

- corrupting both `BootState` slots never causes execution of an unverified object;
- QEMU recovery media can reconstruct a bootable `BootState` from verified roots on one disposable disk while a second attached disk remains byte-identical;
- missing state objects, broken generation closure, unauthorized release metadata, and interrupted reconstruction fail without manufacturing a bootable root;
- the Framework removable-media safety checker proves that recovery images cannot write internal NVMe by default.

Planned verification target:

```sh
just recovery_check
```

Exit condition: a machine with unusable boot metadata can fail closed, boot signed removable recovery, and reconstruct a verified bootable root without modifying any storage device not named by an explicit capability.

### Milestone 5 verification stack

Each permanent change should run the narrowest QEMU scenario that exercises its new behavior. Before a Milestone 5 slice is accepted, the existing repository gates must remain clean:

```sh
just contracts_check
just generation_check
just test
just fmt_check
just lint
```

Storage, model-conformance, release-trust, and recovery slices additionally require their scenario targets. Existing targets remain mandatory where applicable; the later targets are planned:

```sh
just storage_cap_check
just bootstate_model_check
just storage_read_check
just storage_write_check
just storage_fault_check
just rollback_check
just bootstate_trace_check
just release_trust_check
just recovery_check
```

Physical-machine evidence is separate from QEMU evidence. QEMU can prove deterministic logic and fault handling; it cannot prove actual Framework firmware behavior, DMA containment, device identity, power-loss behavior, or absence of writes to internal hardware.

### Milestone 5 definition of done

Milestone 5 is complete only when all of the following are observed:

- every executable object is content verified before execution;
- staging cannot modify the running or known-good generation;
- a pending boot attempt is persistently consumed before control transfers to it;
- userspace health confirmation atomically promotes only the running pending generation;
- interruption at every metadata commit boundary leaves at least one valid `BootState` slot;
- implementation traces from every rollback fault scenario conform to the checked BootState and state/GC models;
- exhausted or failed pending generations automatically boot the known-good generation;
- GC never removes known-good, pending, running, rollback, staged, or persistent-state roots;
- every persistent-state policy has an upgrade and rollback test;
- storage read and write authority is granted only through explicit generation capabilities;
- malformed storage and generation metadata is rejected before out-of-bounds I/O or execution;
- staged and selected generations carry valid release authorization without preventing local known-good rollback;
- invalid boot metadata fails closed and removable recovery cannot write storage absent an explicit capability;
- the existing isolated component graph remains healthy under QEMU;
- the Framework storage-aware slice is observed without unauthorized internal NVMe writes.

## Milestone 6: Native interactive environment

**Status:** Done. Slices M6.1 through M6.7 are complete.

Scope:

- minimal Dango implementation and core runtime;
- command profile/resolver and spawn service;
- kernel prerequisites the spawn service consumes, none of which exist yet: userspace endpoint minting, a non-consuming derive-copy grant path, per-spawner resource accounting, and supervision handles (tracked in `docs/capability-matrix.md`);
- filesystem service and directory capabilities;
- generation inspection and update commands;
- a powerbox-style file dialog service where the user's selection gesture mints a single-object capability;
- generation sync/transfer between machines using authorized release metadata, object transfer, and staged activation.

This milestone consumes the storage, object-store, state, rollback, release-trust, and recovery mechanisms from Milestone 5. Dango commands must resolve executable and directory authority through capabilities rather than global paths or an implicit working directory.

Exit condition: the system can inspect, build or stage, select, and roll back generations through native components.

Sequencing: M6.1 gates every other slice; M6.2 and M6.3 are independent once M6.1 lands; M6.4 consumes both; M6.5 and M6.6 are independent; M6.7 consumes M6.5 and the completed M5.8. M5.7 physical verification and M5.9 recovery proceed independently of this milestone. M6 development and acceptance run under QEMU and removable media, since internal NVMe writes remain disabled until the Milestone 7 reliability gate.

### M6.1: Kernel spawn prerequisites and generation format v2

**Status:** Done.

This slice lands the four kernel mechanisms the spawn service consumes — none exist today (`docs/capability-matrix.md` records them so the milestone does not discover them mid-flight) — and makes grant and bootstrap wiring data-driven.

Deliverables:

- userspace endpoint minting through a named factory capability, with creation authority recorded in the capability matrix; no unprivileged or unbounded mint;
- a non-consuming derive-copy grant path so a spawner retains its own capabilities while gifting narrowed copies (narrow-only, never widening, per the matrix grammar);
- per-spawner resource accounting replacing reliance on the single global `MAX_TASKS` bound; the heap coupling noted in the matrix (`MAX_TASKS * 64 KiB` kernel stacks against the 2 MiB heap) is re-budgeted in the same change;
- supervision handles letting a spawner distinguish child exit, fault, timeout, and peer loss;
- generation format v2 as a new version rather than changed v1 field meanings: manifest grant rights strings map 1:1 to rights bits, `transferable` maps to `RIGHT_TRANSFER`, and bootstrap wiring derives from manifest data; remove the hardcoded `component_name_from_id` debt-register entry and land the deferred `RIGHT_SPAWN` gate in the same change.

Required checks:

- a spawner cannot gift rights it does not hold, and derive-copy never widens;
- a spawner exhausting its per-spawner budget receives a structured error while other spawners continue;
- endpoint minting is bounded and cannot exhaust kernel object tables;
- supervision handles distinguish exit, fault, timeout, and peer loss;
- two builds from identical normalized input produce byte-identical v2 artifacts; unknown versions are rejected;
- bootstrap grants in tests come only from manifest data, not from hardcoded component identity.

Exit condition: a userspace spawner holding a factory capability can mint bounded endpoints, gift narrowed non-consuming copies of its grants, and supervise children within a per-spawner budget, with all wiring derived from a deterministic v2 manifest.

### M6.2: Spawn service and command profile

**Status:** Done.

Deliverables:

- declare the spawn request/reply protocol as a versioned schema in `contracts/spawn/v1` (executable capability, arguments, explicit environment, optional working-directory capability, stream endpoints, grant list) following the dango host boundary in `deps/dango/README.md`;
- implement a userspace spawn service holding the generation-declared executable capabilities and the M6.1 factory capability, spawning on behalf of clients within their declared budgets;
- implement a command profile/resolver mapping command names to executable capabilities from manifest data — no global paths, no implicit working directory;
- enforce per-client accounting through the M6.1 mechanisms.

Required checks:

- generated/validated bindings round-trip every message type byte-identically; out-of-bounds fields and unknown versions are rejected on both ends (M5.2a rule);
- a client cannot launch an executable its profile does not name, cannot exceed its budget, and cannot inject code (spawn composes known hash-verified components only);
- resolver output is deterministic for a fixed manifest;
- spawn-service failure does not terminate unrelated components or the kernel.

Planned verification target:

```sh
just spawn_service_check
```

Exit condition: a client component resolves a command name through its profile and launches the component through the spawn service with exactly the declared grants.

### M6.3: Filesystem service and directory capabilities

**Status:** Done. Verified by `just directory_check`.

Deliverables:

- introduce a Directory object kind with explicit rights in the same change as its `docs/capability-matrix.md` entry; the READ/WRITE/LIST granularity question tracked in the matrix horizon is decided there, and powerbox minting must need no more than `derive`;
- implement a userspace filesystem service over the M5.4 object store presenting a bounded, versioned directory namespace with immutable directory snapshots and explicit root transitions;
- declare directory operations as a versioned schema in `contracts/fs/v1`; malformed requests are rejected structurally before any object-store I/O;
- directory capabilities derive and transfer per the matrix grammar.

Required checks:

- a component without a directory capability cannot resolve or mutate names under it;
- derive narrows only (subdirectory scope, fewer rights) and never widens;
- an interrupted namespace transition preserves the previous committed root;
- bounds on path length, entry counts, and depth are enforced before object-store requests.

Planned verification target:

```sh
just directory_check
```

Exit condition: components browse and mutate a namespace only through explicit directory capabilities, with all metadata integrity-checked by the store.

### M6.4: Minimal Dango implementation and core runtime

**Status:** Complete. Native Dango provides the bounded interactive command subset, capability-mediated launch contexts, structured termination, and deterministic scripted QEMU verification.

Scope boundary: this slice delivers the interactive command subset — REPL, `$(...)` launch, explicit command context, and structured termination. The full Hindley-Milner, row-polymorphism, and effect-inference machinery from `deps/dango/docs/semantics.md` is not M6 scope.

Deliverables:

- implement a minimal Dango interpreter as a native component: parser for the command subset plus the arity and shape checks needed for launch;
- launch `$(...)` external commands through the M6.2 spawn service, resolving names through the active command profile;
- implement explicit command context (`with-env`, `with-cwd`, `with-stdin`) with no ambient inheritance of environment, working directory, or streams;
- map structured component termination (exit, fault, timeout, peer loss, revocation) to command results and the `IO.Exit` behavior;
- provide an interactive REPL over the console with keyboard input, plus deterministic scripted-input fixtures so sessions reproduce under QEMU.

Required checks:

- every command launch traces to a profile resolution and a spawn-service request; the interpreter holds no direct spawn authority beyond its own grant;
- child components receive only the constructed context; nothing ambient leaks;
- termination reasons remain distinguishable at the language boundary;
- scripted REPL sessions reproduce deterministically under QEMU.

Planned verification target:

```sh
just dango_check
```

Exit condition: a user at the console runs native commands through Dango with capability-resolved authority and structured failure behavior.

### M6.5: Generation inspection and update commands

**Status:** Complete. Native list, inspect, stage, select, and rollback components use the versioned generation-management service; `BOOT_UPDATE` is manifest-scoped to that service; closure and release failures preserve BootState; and `just generation_cmd_check` validates deterministic inspection plus model-checked transition traces.

Deliverables:

- implement native generation commands — list, inspect, stage, select, roll back — as components talking to the declared generation-management service;
- land the matrix horizon's BootState update authority beyond confirmation (candidate `BOOT_UPDATE` right) in the same change as the staging service and its capability-matrix entry, granted only to the declared generation-management service;
- stage generations from object closure plus a v2 manifest; staged generations require valid M5.8 release authorization, staging never advances the accepted release sequence, and activation never overwrites the running generation;
- extend the generation-management protocol under `contracts/` as a versioned schema.

Required checks:

- inspect output matches the deterministic manifest and store contents;
- staging with missing objects or invalid release authorization fails closed before any BootState change;
- select and rollback transitions conform to the checked M5.6a/M5.6b models, with implementation traces validated as in M5.6c;
- unprivileged components cannot reach BootState update operations.

Planned verification target:

```sh
just generation_cmd_check
```

Exit condition: generations are inspected, staged, selected, and rolled back entirely through native components, with all authority manifest-declared.

### M6.6: Powerbox file dialog service

**Status:** Complete. The versioned powerbox protocol, console chooser, user-gesture mint, narrow-only single-object transfer, cancellation path, and provenance event are exercised by `just powerbox_check`.

No UI stack exists until Milestone 7; the chooser is a console-based selection component. This slice implements the directions register entry 16 exit-condition sketch, not the general pattern beyond it.

Deliverables:

- implement a chooser component holding directory authority the requester lacks;
- declare the request/response schema in `contracts/powerbox/v1`: object kind, requested rights, purpose string;
- mint the user's selection gesture as a single-object capability via narrow-only derive from the chooser's own grant, transferred back over the request channel;
- record the gesture as a provenance event.

Required checks:

- a component with no directory grants receives exactly the selected single object with the declared rights — nothing more;
- minted capabilities cannot exceed the chooser's own grant (derive closure);
- cancelling the dialog mints nothing;
- the requesting component cannot bypass the chooser to reach the same objects.

Planned verification target:

```sh
just powerbox_check
```

Exit condition: a component with no directory grants opens the chooser, the user selects a file, and the component receives a single-object capability it could not have obtained from the manifest or any peer.

### M6.7: Generation sync and transfer

**Status:** Done.

This slice implements the directions register entry 14 exit-condition sketch and consumes M5.8 release authorization. In QEMU the transfer medium is a second attachable virtio-blk disk; virtio networking is Milestone 7 scope.

Deliverables:

- implement the host-side closure algorithm (manifest to required object set per state policy: `preserve` and `snapshotBeforeUpgrade` state travels, `ephemeral` does not, `immutable` travels read-only) and a deterministic, versioned transfer-manifest format;
- transfer objects as set-difference over content identities against the receiver's store;
- verify complete closure and M5.8 release authorization on the receiver before staging; both failure modes fail closed before any boot attempt is consumed;
- activate on the receiver through the ordinary M5.6 path: stage as pending, consume attempts, health-confirm.

Required checks:

- the transfer manifest is byte-identical for a fixed generation;
- incomplete closure or authorization mismatch fails closed before transfer of control and consumes no boot attempt;
- the receiving machine boots the transferred generation as pending and promotes it only after health confirmation;
- storage devices not named by an explicit capability remain byte-identical.

Planned verification target:

```sh
just transfer_check
```

Exit condition: an authorized QEMU-built generation transfers to a second machine and activates there with grants and state policy intact.

### Milestone 6 verification stack

Each permanent change runs the narrowest QEMU scenario exercising its new behavior. New IPC protocols are schema-first under `contracts/` (M5.2a rule); new objects or rights land with their `docs/capability-matrix.md` entries in the same change. The existing repository gates must remain clean:

```sh
just contracts_check
just generation_check
just test
just fmt_check
just lint
```

Slice targets (planned):

```sh
just spawn_service_check
just directory_check
just dango_check
just generation_cmd_check
just powerbox_check
just transfer_check
```

### Milestone 6 definition of done

Milestone 6 is complete only when all of the following are observed:

- spawn, filesystem, powerbox, generation-management, and transfer protocols are versioned schemas under `contracts/` and covered by `contracts_check`;
- the four kernel prerequisites (endpoint minting, derive-copy grants, per-spawner accounting, supervision handles) exist, are bounded, and are gated per the capability matrix;
- executable and directory authority resolves only through capabilities — no global paths, no implicit working directory;
- generation inspect, stage, select, and rollback run through native components under QEMU, conforming to the checked BootState models;
- a powerbox selection gesture mints a single-object capability a requester could not otherwise obtain;
- an authorized generation transfers between machines and activates with its grant set and state policy intact;
- the existing isolated component graph and every Milestone 5 target remain healthy.

## Milestone 7: Daily-driver hardware

**Status:** Planned; implementation has not started. The current Framework removable image boots, but physical keyboard input has been observed unavailable. M7.1 captures that failure without requiring interactive input, and M7.4 is the first slice allowed to claim a working physical keyboard.

M7 promotes hardware in two distinct steps: deterministic mechanism and fault handling under QEMU first, then an observed Framework run with the exact generation-declared device grant. A QEMU pass never substitutes for physical evidence. DMA-capable physical drivers remain trusted and read-only until M7.4 installs IOMMU containment; internal NVMe writes remain disabled until M7.7 completes every promotion gate.

Sequencing:

- M7.1 inventories the actual Framework firmware and device topology; later drivers consume that evidence rather than guessed BDFs, interrupt routes, or protocols.
- M7.2 gates every userspace hardware driver. M7.3 proves xHCI, USB, and HID logic under QEMU; M7.4 adds AMD-IOMMU containment and promotes USB HID on the Framework.
- M7.5 and M7.6 consume M7.4 and may proceed in parallel. M7.7 consumes M7.4 plus M7.5's disposable external-media path.
- M7.8 may proceed after M7.2 because it uses the boot GOP framebuffer; M7.9 consumes the M7.1 ACPI inventory. M7.10 consumes every device service that must quiesce and resume.
- M7.11 and M7.12 are independent once their buses, input/audio/network services, and resume hooks exist. M7.13 consumes M7.4, M7.8, and M7.10. M7.14 is the integrated acceptance slice and consumes all preceding slices.

### M7.1: Framework evidence harness and hardware inventory

**Status:** Not started. First observed blocker: the current removable image reports no usable physical keyboard input.

Deliverables:

- emit one bounded, versioned hardware report over both framebuffer and serial without requiring keyboard input: ACPI table identities and checksums, MCFG functions, validated BARs, interrupt routes, framebuffer geometry, IOMMU-table presence, NVMe identity, and every input-controller initialization stage;
- add an append-only host-side evidence record containing image identity, machine identity, firmware version, report output, and pre/post hashes for every storage device whose immutability is being asserted;
- keep the report free of raw memory, firmware secrets, storage payloads, and unbounded descriptor dumps;
- preserve the removable-image no-internal-write rule and provide a non-interactive timeout or shutdown path so a failed input driver cannot strand the test.

Required checks:

- malformed ACPI, PCI, BAR, interrupt, and descriptor fixtures fail with a typed bounded error rather than a hang;
- two boots on unchanged firmware produce the same normalized topology report, excluding explicitly named volatile fields;
- the Framework report identifies whether the keyboard path is i8042, USB HID, or another firmware-described controller and records the exact initialization failure;
- the internal NVMe comparison region remains byte-identical across the inventory boot.

Planned verification target:

```sh
just framework_inventory_check
```

Exit condition: the repository contains reproducible evidence for the target Framework's actual controller topology and the current keyboard failure is localized to a named initialization stage.

### M7.2: Userspace driver authority ABI and generation format v3

**Status:** Not started.

The current matrix has only eight free `u32` rights bits and six grandfathered, ungated device rights. Daily-driver hardware cannot safely grow on that surface. This slice introduces generation format v3 rather than changing v2 meanings, widens rights to `u64`, and lands the gates every later userspace driver consumes.

Deliverables:

- define generation v3 with `u64` rights and deterministic v2-to-v3 builder migration; stage-0 continues to decode retained v2 known-good generations during the rollback window;
- expose bounded operations for mapping only the granted PCI function's validated BAR ranges, allocating/mapping/releasing charged DMA buffers, waiting for and acknowledging only the granted interrupt, and creating/mapping bounded shared buffers;
- make DMA, IRQ, MMIO, and shared-buffer creation explicit named capabilities with hard per-driver quotas in the generation; remove the six ungated exceptions from the capability matrix as their gates land;
- supervise driver components so timeout, fault, peer loss, and reset are reported without terminating unrelated services; driver restart receives fresh mappings and cannot reuse stale DMA or IRQ authority;
- declare every new driver IPC protocol schema-first under `contracts/`; clients see typed device services, never ambient PCI enumeration or raw global MMIO.

Required checks:

- an ungranted component cannot enumerate PCI functions, map a BAR, allocate DMA memory, receive or acknowledge an interrupt, or map another component's buffer;
- BAR offsets, mapping lengths, DMA page counts, outstanding requests, interrupt queues, and shared-buffer totals are bounded before allocation or mapping;
- a driver cannot program DMA outside buffers in its account at the software boundary, and reclamation remains impossible while a request is outstanding;
- v2 and v3 artifacts are byte-deterministic for fixed normalized input, unknown versions and rights bits are rejected, and rollback to a retained v2 generation still boots;
- driver failure revokes its mappings and returns charged resources before a supervised restart.

Planned verification target:

```sh
just driver_abi_check
```

Exit condition: a manifest-declared userspace driver can access exactly one PCI function, bounded DMA buffers, its interrupts, and typed service endpoints without ambient hardware authority.

### M7.3: xHCI, USB core, and HID under QEMU

**Status:** Not started.

This slice proves the USB implementation before physical DMA promotion. It does not yet claim safe Framework xHCI operation.

Deliverables:

- implement a userspace xHCI driver over the M7.2 ABI with bounded command, event, and transfer rings and explicit controller reset/timeout handling;
- implement root-hub and bounded hub enumeration, descriptor validation, address/configuration selection, endpoint lifecycle, disconnect cancellation, and per-device identities derived from topology plus validated descriptors;
- implement USB HID keyboard and pointer boot protocols, key rollover bounds, press/release events, and hotplug;
- route i8042 and USB HID through one versioned seat/input service so Dango and later compositor clients do not depend on the physical transport;
- keep physical xHCI bus mastering disabled until M7.4 supplies an IOMMU domain.

Required checks:

- malformed, cyclic, oversized, short, or inconsistent USB descriptors are rejected before ring or buffer allocation;
- transfer timeout, controller reset, endpoint stall, surprise removal, and event-ring overflow leave no pinned buffer or permanently wedged input service;
- one client holding seat input authority receives events while an ungranted component cannot read or inject them;
- scripted QEMU keyboard input drives the native Dango REPL through the same seat protocol used by physical HID;
- repeated attach/detach remains within fixed device, endpoint, transfer, and queue bounds.

Planned verification target:

```sh
just usb_hid_check
```

Exit condition: QEMU xHCI keyboard and pointer input survive malformed devices, hotplug, timeout, and reset through a transport-independent input service.

### M7.4: AMD IOMMU containment and Framework USB HID promotion

**Status:** Not started.

Deliverables:

- parse the target's ACPI IVRS data with strict bounds and identify the AMD IOMMU and device aliases from M7.1 evidence;
- create one IOMMU domain per DMA-capable driver, map only its live M7.2 DMA buffers with the requested direction, invalidate translations before reuse, and enable PCI bus mastering only after the domain is active;
- report IOMMU faults with device identity, access type, and bounded address detail; quarantine the offending driver and complete its supervision handle with a distinct failure;
- boot the Framework xHCI driver inside an enforced domain and bring up the actual built-in or attached USB keyboard identified by M7.1;
- retain an emergency removable image that does not enable bus mastering and can reproduce the inventory report if IOMMU setup fails.

Required checks:

- a synthetic driver DMA outside its mapped region faults and cannot modify the guard region or another component's memory;
- stale translations are unusable after buffer release, driver restart, device reset, and generation transition;
- unsupported or malformed IVRS data fails closed before bus mastering;
- IOMMU fault storms are bounded and cannot livelock the kernel or suppress unrelated device interrupts;
- on the Framework, the keyboard can type `$(sysinfo)` into Dango, receive `result:exit:0`, use Backspace and Shift punctuation, and close with Escape while internal NVMe comparison hashes remain unchanged.

Planned verification targets:

```sh
just iommu_check
just usb_hid_check
```

Exit condition: the Framework has observable keyboard input through the common seat service, and xHCI DMA is confined to generation-declared buffers by the AMD IOMMU.

### M7.5: USB mass storage and removable-device identity

**Status:** Not started.

Deliverables:

- implement one standards-based USB mass-storage transport proven by the target test device, with bounded command/data/status phases, sense decoding, timeout, reset, and surprise-removal handling;
- expose USB media through the existing block protocol so filesystem, object-store, recovery, and generation services receive no transport-specific authority;
- identify media by USB topology plus validated device identity and render an operator-visible distinction between removable test media and the internal NVMe;
- require an exact BlockDevice capability for all reads and writes; default Framework generations grant removable media read-only and grant writes only to an explicitly selected disposable device;
- make removal during an outstanding write fail the request and preserve the last committed filesystem/object-store root.

Required checks:

- malformed capacity, short transfer, failed sense, timeout, reset, and unplug paths release every DMA mapping and return a structured block error;
- a writable grant for one USB disk cannot address a second USB disk or internal NVMe;
- removing media at every tested commit boundary preserves the previous committed root;
- repeated enumerate/read/write/flush/remove cycles stay within fixed resource bounds;
- a Framework run writes and verifies a disposable external device while pre/post internal-NVMe hashes remain identical.

Planned verification target:

```sh
just usb_storage_check
```

Exit condition: Slime OS has a capability-selected disposable physical storage target suitable for destructive reliability work without granting internal NVMe write authority.

### M7.6: Network service, USB Ethernet, and destination authority

**Status:** Not started.

Deliverables:

- declare versioned link, IP, DNS, UDP, TCP, and network-service protocols under `contracts/`, with a deterministic virtio-net QEMU backend and one Framework USB-Ethernet backend selected from M7.1 descriptors;
- implement bounded Ethernet, ARP/NDP, IPv4/IPv6, ICMP, DHCP/SLAAC, UDP, TCP, and exact-name DNS resolution sufficient for native update and diagnostic clients;
- add a `NetworkDestination` object in generation v3 identifying transport, exact IP address or exact DNS name, and port, with distinct CONNECT, SEND, RECV, and LISTEN rights; wildcard destinations are not admitted in M7;
- keep DNS authority inside the network service: resolving an exact declared name does not grant the requesting component arbitrary resolver or raw-packet access;
- account socket count, queued bytes, retransmission state, and per-destination traffic against bounded manifest data.

Required checks:

- a component granted one destination connects only to that exact name/address, transport, and port; alternate address, port, DNS name, raw packet, and listen attempts fail closed;
- malformed frames, DHCP options, DNS messages, fragmented packets, TCP options, retransmission exhaustion, and peer loss do not exceed bounds or wedge unrelated connections;
- the manifest and authority-diff tooling enumerate every reachable destination;
- QEMU transfers deterministic data to an allowed endpoint while a simultaneous denied endpoint receives no packet;
- the Framework obtains a lease/address through USB Ethernet and reaches one declared endpoint after link unplug/replug and after a driver restart.

Planned verification target:

```sh
just network_check
```

Exit condition: native components have useful wired networking while the generation, not ambient socket access, determines every reachable destination.

### M7.7: Native NVMe reliability and internal-storage promotion

**Status:** Not started. Internal NVMe writes remain prohibited until this slice's physical evidence is recorded.

Deliverables:

- move the native NVMe transport behind the M7.2 userspace driver ABI and M7.4 IOMMU domain while preserving the common block protocol;
- add bounded interrupt-driven read, write, flush, timeout, abort/reset, and reinitialization paths for the exact target controller and namespace;
- exercise destructive native-NVMe tests only with a dedicated replaceable NVMe installed for testing; the personal internal device is absent during those tests;
- validate flush ordering, forced reset, abrupt power loss, torn metadata, malformed GPT/object-store/generation/BootState records, and recovery across the M5 commit boundaries;
- introduce a separately named internal-storage promotion profile requiring the exact controller/namespace identity and an explicit operator-visible arming step; ordinary Framework and recovery images remain unable to write it;
- record pre/post full metadata-region hashes and rollback/recovery results before promoting any internal write grant.

Required checks:

- IOMMU containment, queue bounds, LBA bounds, timeout/reset, flush ordering, interrupted writes, and malformed metadata pass under QEMU fault injection and on the sacrificial physical NVMe;
- reset or power loss at every recorded commit boundary leaves a bootable known-good generation or signed removable recovery path;
- a wrong controller, namespace identity, capacity, firmware-reported block size, or missing operator arm fails before the first write command;
- the internal-storage profile grants BLOCK_WRITE and BOOT_UPDATE only to the declared storage/generation service and cannot be selected by an unprivileged component;
- the first write-enabled run on the target Framework is observed separately and preserves the retained known-good and recovery roots.

Planned verification target:

```sh
just storage_reliability_check
```

Exit condition: internal NVMe writes are enabled only after deterministic and physical reliability evidence, IOMMU confinement, exact device identity, and rollbackable recovery are all observed.

### M7.8: Software compositor and desktop shell over GOP

**Status:** Not started.

Deliverables:

- move visible output from the global debug stream to a userspace compositor holding the sole framebuffer capability; retain serial as diagnostics rather than the interactive UI path;
- declare versioned surface, damage, presentation, seat-focus, clipboard, and dialog protocols; clients render only into bounded shared buffers and cannot map the physical framebuffer;
- implement software composition, double buffering, damage tracking, cursor rendering, focus, keyboard/pointer routing, and a terminal surface hosting the native Dango session;
- render the M6 powerbox as a compositor-owned modal dialog while preserving its user-gesture capability mint and cancellation semantics;
- make display dimensions, pixel format, surface count, buffer bytes, damage rectangles, and event queues explicit bounds.

Required checks:

- an ungranted client cannot read input, capture another surface, map the framebuffer, steal focus, or bypass a powerbox dialog;
- malformed dimensions, overflowed strides, out-of-bounds damage, stalled clients, and compositor restart cannot write outside granted buffers;
- deterministic scene fixtures produce byte-identical software-composited frame hashes under QEMU;
- Dango remains usable while another client faults or floods damage/events within its quota;
- on the Framework, terminal text, pointer movement, focus changes, and powerbox selection are visible and survive a compositor restart.

Planned verification target:

```sh
just compositor_check
```

Exit condition: the Framework has an isolated, capability-routed graphical session over GOP without granting applications ambient framebuffer or input access.

### M7.9: Battery, charger, brightness, lid, and thermal service

**Status:** Not started.

Deliverables:

- consume M7.1 ACPI/EC evidence through a bounded target-specific ACPI resource evaluator; unsupported AML constructs fail closed rather than growing an implicit general interpreter;
- expose versioned battery charge/rate/health, AC/charger state, lid state, thermal readings/trips, and panel brightness through a userspace platform service;
- keep mechanism in the kernel and policy in userspace: a generation-declared power-policy component decides brightness and thermal responses through explicit control endpoints;
- distinguish read-only telemetry from control authority in the capability matrix and give ordinary applications neither EC register access nor charger/thermal control;
- emit bounded state-change events and provenance for user brightness changes and thermal emergency actions.

Required checks:

- malformed AML/resource data, EC timeout, implausible sensor values, event storms, and missing methods produce typed degraded states without hangs;
- a telemetry-only client cannot change brightness, charger behavior, thermal policy, reset, shutdown, or sleep state;
- brightness remains within firmware-advertised bounds and a thermal emergency cannot be masked by an unprivileged component;
- QEMU fixtures cover AC attach/detach, charge/discharge, lid transitions, and thermal thresholds;
- Framework readings agree with firmware-visible state and brightness/lid events are observed without storage modification.

Planned verification target:

```sh
just platform_service_check
```

Exit condition: the Framework reports and controls its basic power state through explicit service capabilities while platform policy remains outside the kernel.

### M7.10: Suspend/resume lifecycle and device reinitialization

**Status:** Not started.

The target sleep state is selected from M7.1 firmware evidence; the slice does not claim both S3 and modern standby when the machine exposes only one supported route.

Deliverables:

- define a checked suspend state machine covering request, service quiesce, storage flush, device stop, IOMMU teardown, platform entry, resume, IOMMU restore, device reinitialization, and userspace thaw;
- require explicit acknowledgements from storage, compositor, input, network, audio, and generation services before entering sleep; timeout aborts suspend and leaves the system running;
- restore monotonic time, interrupt routing, xHCI, network, NVMe, display, and later audio/wireless devices without reusing stale DMA mappings or capabilities;
- preserve pending-generation attempt semantics across sleep and never treat resume as a fresh successful boot or health confirmation;
- expose lid-close and user-request policy in userspace, with an emergency kernel path only for thermal safety.

Required checks:

- failure or timeout at every quiesce/resume stage either aborts to a working session or enters signed recovery on next boot; it never silently advances BootState;
- no device can DMA while its IOMMU domain is torn down, and all post-resume DMA buffers are freshly mapped;
- network reconnect, USB re-enumeration, input, compositor, storage, and audio recover without duplicating capabilities or leaking bounded resources;
- QEMU fault injection covers every transition edge and repeated cycles;
- the Framework completes at least 50 lid/user suspend-resume cycles, including cycles with active network and audio, without data corruption or loss of Dango control.

Planned verification target:

```sh
just suspend_check
```

Exit condition: the Framework repeatedly suspends and resumes through a checked, rollback-safe lifecycle with all active device services re-confined and usable.

### M7.11: I2C touchpad and audio service

**Status:** Not started. Touchpad and audio are independent implementations and may proceed in parallel after their M7.2/M7.9 bus and resource descriptions are available.

Touchpad deliverables and checks:

- implement the Framework's firmware-described I2C controller and HID-over-I2C touchpad path with bounded reports, interrupt handling, reset, and resume;
- route pointer contacts through the common seat service; gesture recognition, acceleration, palm rejection, and tap policy live in userspace;
- reject malformed descriptors/reports and prove an ungranted component cannot read raw contacts or inject pointer events;
- physically verify pointer movement, click, two-finger scroll, palm rejection, hot reset, and resume.

Audio deliverables and checks:

- select the HDA or ACP route from observed hardware/codec evidence rather than implementing an unused backend by assumption;
- declare a versioned PCM service with bounded shared rings, playback/capture stream capabilities, volume/mute control, device routing, underrun/overrun reporting, and resume;
- give the mixer sole hardware authority; clients cannot map audio DMA or read microphone samples without an explicit capture grant;
- verify deterministic virtual-backend mixing, client fault isolation, bounded latency, underrun recovery, speaker/headphone output, microphone capture, mute indication, and repeated suspend/resume on the Framework.

Planned verification targets:

```sh
just touchpad_check
just audio_check
```

Exit condition: the Framework has daily-usable touchpad input and capability-isolated playback/capture through services that survive reset and suspend.

### M7.12: MT7925 Wi-Fi and Bluetooth

**Status:** Not started.

Deliverables:

- package required device firmware as content-addressed, release-authorized generation objects; drivers never search a global firmware path;
- implement the MT7925/RZ717 Wi-Fi path inside an M7.4 IOMMU domain and attach it as another backend to the M7.6 network service;
- support scan, association, WPA2/WPA3 personal authentication, reconnect, regulatory constraints, and suspend/resume with bounded stations, frames, retries, and key material lifetime;
- accept Wi-Fi credentials through an explicit interactive service and keep them scoped to the network service; M8 later generalizes non-readable and revocable secrets rather than being bypassed here;
- implement the combo device's observed Bluetooth transport, bounded HCI, pairing state, and the HID and audio profiles required for keyboard/pointer and headset use through the existing input/audio services.

Required checks:

- malformed firmware, frames, scan results, association responses, HCI events, pairing messages, and retry storms remain bounded and fail closed;
- Wi-Fi traffic remains subject to the same `NetworkDestination` grants as Ethernet; switching links does not widen reachable destinations;
- ungranted components cannot read credentials, link keys, raw wireless frames, Bluetooth HID events, or microphone audio;
- disconnect, radio reset, driver restart, and suspend/resume clear transient keys and reconnect without stale DMA mappings;
- on the Framework, Wi-Fi reaches one declared destination and Bluetooth keyboard/pointer plus headset audio work after repeated reconnect and resume cycles.

Planned verification targets:

```sh
just wifi_check
just bluetooth_check
```

Exit condition: the Framework has capability-preserving wireless networking and the Bluetooth input/audio paths needed for daily use.

### M7.13: Radeon display control and graphics acceleration

**Status:** Not started.

Deliverables:

- package Radeon firmware as release-authorized generation objects and run the display driver with exact PCI, interrupt, buffer, and IOMMU authority;
- take over the internal panel from GOP, set its validated native mode, page-flip compositor buffers, control the hardware cursor, recover from display/GPU faults, and restore the panel after suspend;
- accelerate compositor-owned copy, fill, scaling, and composition operations while retaining the M7.8 software renderer as the correctness oracle and fallback;
- keep command submission private to the compositor/display service in M7; general application or compute queues and accelerator authority remain Milestone 10 scope;
- integrate panel brightness and thermal limits with M7.9 without moving policy into the driver.

Required checks:

- command buffers, relocations, dimensions, pitches, addresses, and synchronization waits are validated before submission and remain within IOMMU-mapped compositor buffers;
- a faulting or timed-out queue resets to the software compositor without exposing stale frames or wedging input;
- accelerated output matches the software renderer's deterministic frame hashes for the supported operations;
- applications cannot submit GPU commands, map scanout buffers they do not own, or bypass compositor focus/powerbox rules;
- the Framework internal panel runs at its validated native mode, remains stable during interactive use, and recovers through repeated suspend/resume and forced driver reset.

Planned verification target:

```sh
just radeon_display_check
```

Exit condition: the Framework compositor owns a stable, IOMMU-contained Radeon display path with verified software fallback and no general ambient GPU authority.

### M7.14: Energy accounting and integrated daily-driver qualification

**Status:** Not started.

Deliverables:

- attribute scheduler-active time and bounded service work (IPC, storage, network, audio, and graphics bytes/events) to components and supervision subtrees;
- combine those counters with M7.9 battery/platform telemetry into readable per-component energy estimates and declare their schema in the generation;
- keep M7 accounting as telemetry and threshold events, not hidden scheduler authority; revocation and scheduling-class enforcement arrive explicitly in Milestone 8;
- provide an operator-visible hardware status and authority view covering current generation, device drivers, IOMMU domains/faults, storage identity, network destinations, power state, and per-component resource/energy use;
- define and record one reproducible Framework qualification run covering interactive console/compositor use, wired and wireless networking, external removable storage, internal rollbackable storage, audio, touchpad, Bluetooth, display acceleration, suspend/resume, and recovery media.

Required checks:

- accounting totals are monotonic within the declared window, bounded, attributable, and cannot be reset or forged by the measured component;
- shared-service charging rules are fixed and tested so repeated forwarding cannot create or erase usage;
- an eight-hour Framework session with concurrent interactive, network, audio, storage, and background load retains input/display responsiveness and shows no unbounded task, capability, DMA, queue, or buffer growth;
- the qualification run includes at least 50 suspend/resume cycles, repeated USB and network hotplug, forced driver restarts, a failed pending generation, and signed recovery without unauthorized storage writes;
- every physical test records image/generation identity, firmware version, hardware identities, authority manifest, observed output, and storage-integrity evidence.

Planned verification target:

```sh
just daily_driver_check
```

Exit condition: the Framework target sustains the complete native daily-driver workload with explicit hardware and network authority, IOMMU-contained DMA, rollbackable storage, observable power/resource use, and repeatable physical evidence.

### Milestone 7 verification stack

Every permanent change runs the narrowest QEMU or host-side scenario exercising its new behavior. Every physical promotion additionally records a removable-media Framework run; QEMU evidence alone cannot complete a hardware slice. The repository gates remain mandatory:

```sh
just contracts_check
just generation_check
just test
just fmt_check
just lint
just fmt_check_components
just lint_components
just framework_safety_check
```

Planned slice targets:

```sh
just framework_inventory_check
just driver_abi_check
just usb_hid_check
just iommu_check
just usb_storage_check
just network_check
just storage_reliability_check
just compositor_check
just platform_service_check
just suspend_check
just touchpad_check
just audio_check
just wifi_check
just bluetooth_check
just radeon_display_check
just daily_driver_check
```

### Milestone 7 definition of done

Milestone 7 is complete only when all of the following are observed on the target Framework and backed by the deterministic checks above:

- every DMA-capable physical driver runs in an AMD-IOMMU domain that maps only its live generation-declared buffers, with fault isolation and supervised restart;
- the built-in or attached keyboard, touchpad, pointer, display, audio, USB storage, USB Ethernet, Wi-Fi, and Bluetooth paths are usable through typed services rather than ambient hardware access;
- applications receive input, display surfaces, audio streams, files, and network destinations only through explicit capabilities; the manifest can answer which component can reach which device or remote endpoint;
- internal NVMe writes have passed the M5.7/M7.7 bounds, reset, flush, interruption, malformed-metadata, device-identity, rollback, and recovery gates on disposable hardware before promotion on the target device;
- the compositor and Radeon driver survive client faults, driver reset, and suspend/resume with a software-rendered fallback;
- battery, charger, brightness, lid, and thermal state are observable, controls are explicitly authorized, and userspace owns policy;
- suspend/resume repeatedly quiesces and restores storage, IOMMU, USB, input, display, network, wireless, and audio without stale DMA authority or BootState corruption;
- per-component resource and energy accounting is visible and bounded, without silently introducing the Milestone 8 scheduling or revocation model;
- the integrated physical qualification run completes with no unauthorized internal-storage modification, no unbounded resource growth, and a signed removable recovery path that remains bootable.

## Milestone 8: Foreign-workload authority foundations

**Status:** Not yet implemented.

Milestones 6 and 7 make the system self-hosting and daily-usable, but the authority model still cannot express three properties a machine running agents and foreign code needs: withdrawing a grant without killing its holder, bounding *when* a component runs, and handing over a credential that cannot be copied or leaked. This milestone adds those three as non-ambient, auditable, rollbackable authority before Milestone 9 runs untrusted foreign code against them. It promotes directions register entries 2, 32, and 33; each has a paper-legal design half today, and all three share the M6 spawn-service prerequisites.

Scope:

- **Revocation and leases (entry 2):** kernel-maintained capability derivation trees so a holder may revoke exactly its own subtree; use-after-revoke returns a structured error distinct from never-held; generation-declared grant lifetimes reclaimed by the health service. The lease clock model must resolve wall-clock-at-use versus durable health-service transition against rollback semantics, and revocation must preserve the entry-24 rights-algebra closure property (removing edges never creates reachability).
- **Scheduling class and QoS authority (entry 32):** a manifest-declared scheduling class per component or supervision subtree (at minimum foreground / normal / best-effort). The kernel owns only the ordering mechanism; class assignment is generation policy and dynamic re-classification is a userspace policy decision. The authority-versus-telemetry question is resolved toward authority: a component cannot widen its own class beyond its grant. Composes with the entry-25 resource account (share quantity) as a separate ordering axis, and preserves class across supervision restart.
- **Secrets as capabilities (entry 33):** a `Secret` object kind with a USE right (present the secret to a designated service) split from a narrower or absent READ, so a component can authenticate with material it can never exfiltrate as bytes. Secret capabilities and secret-bearing IPC are marked non-recordable: the flight recorder (entry 11) redacts to a handle/commitment and replay re-injects from the sealed store. At-rest storage rides M5.6b state bindings; `discardOnRollback` is the default so a rolled-back generation cannot resurrect a rotated credential.

Required checks:

- a proxy revokes a derived subtree; the original holder's further use fails structurally while sibling grants survive;
- a lease expires and is reclaimed by the health service with rollback-consistent semantics;
- a component cannot claim a scheduling class it was not granted;
- a USE-only Secret authenticates to a declared service without the holder reading the secret bytes, and a recorded trace contains no secret material while remaining replayable;
- capability-matrix amendments for the `Secret` object, revocation semantics, and scheduling-class authority land in the same change as the mechanism.

Exit condition: revocation, scheduling class, and secrets are expressible as non-ambient, auditable, rollbackable authority; a derived grant can be withdrawn without killing its holder, a foreground component keeps declared ordering under contention, and a scoped credential is usable but neither readable nor recordable.

## Milestone 9: Compatibility route

**Status:** Not yet implemented.

README names Linux/POSIX compatibility as a future userspace personality or isolated guest VM, but no earlier milestone turns that into a plan. This is the largest gap for the stated daily-driver-plus-containers target: everything before it refines the native model, while this milestone adds the ability to run foreign workloads at all — without smuggling in the ambient filesystem, environment, and network authority a Linux process assumes. It promotes directions register entry 31 and consumes M6 spawn/endpoint-minting/filesystem machinery plus M8's confinement primitives; the guest-VM half additionally requires an unscoped virtualization milestone and M7 IOMMU-enforced DMA.

Scope (personality first):

- a Linux personality component that loads a Linux binary and translates its syscalls into Slime IPC against declared service capabilities: `open`/`read`/`write` into object-store or filesystem-service transactions bounded by directory grants; `socket` traffic gated by entry-18 NetworkDestination grants; `clock_gettime`/`getrandom` as entry-3 clock/entropy capabilities;
- a fixed, audited supported-syscall subset; anything ungranted returns the Linux errno for "not permitted" rather than widening authority;
- the container image as a content-addressed M5.4 object so image identity and integrity reuse the generation verification path;
- `fork`/`exec` mapped onto the spawn service such that a child never holds more than its parent;
- the container plus its grant set as generation data — auditable by entry 9, diffable by entry 1, rollbackable like any component.

Guest VM (later slice, not an M9 exit requirement): a full Linux kernel under AMD-V with virtio devices backed by Slime services, whose only authority is the virtio endpoints it is handed. It presents the same generation-level contract as the personality (foreign workload plus declared grant set) and differs only in fidelity and cost; it is gated on a scoped virtualization milestone and M7 IOMMU enforcement.

Required checks:

- a declared container reads and writes only the files its directory grant covers;
- it reaches only its declared network destinations and is denied everything else with a normal Linux errno;
- an ungranted syscall fails with the mapped errno rather than escalating;
- the container's complete authority is visible in and diffable from the manifest.

Exit condition: a Linux binary declared as a container in the generation runs under the personality, confined to its declared directory, network, and nondeterminism grants, denied everything else with a normal errno, with its complete authority visible in the manifest.

## Milestone 10: Accelerator compute authority

**Status:** Not yet implemented.

The agentic direction makes a language model a userspace service, but the Framework NPU/GPU have no authority story — M7 energy accounting measures power, not compute submission. This milestone introduces compute submission as a first-class capability so an agent's inference authority and budget are manifest data answered statically, not discovered at runtime. It promotes directions register entry 28 and depends on M7 hardware bring-up and IOMMU-enforced DMA; the authority shape reuses the BlockDevice gating template proven by `storage_cap_check`.

Scope:

- an `Accelerator` object kind representing one compute device or queue class, with a SUBMIT right (place work on a queue) split from management rights (queue creation, firmware or mode control);
- generation-declared compute budgets (tokens, work items, or queue-time per window), riding the entry-25 account pattern or declared as manifest scalars, with exhaustion surfaced as a structured error or throttling;
- IOMMU-constrained accelerator memory access limited to buffers the submitting component holds, reusing the SharedBuffer handoff path rather than a parallel mechanism;
- a capability-matrix row for the object and its rights, landing with the driver.

Required checks:

- a component without the accelerator capability cannot submit work;
- a component past its declared budget is rejected or throttled with a structured error;
- accelerator DMA cannot reach memory outside the submitting component's held buffers;
- the manifest lists every component holding accelerator authority (entry-9 queryable).

Exit condition: compute submission is a rights-gated, budgeted, IOMMU-contained capability; unprivileged components cannot submit, over-budget components are rejected or throttled, and every accelerator grant is visible in the manifest.

## Milestone 11: Physical trust and attestation

**Status:** Not yet implemented.

Generations are content-addressed and the Framework has a TPM, but the BootState attempt counter and known-good identity live on disk, so reimaging the disk to an older state also rolls back the rollback protection — a known-bad generation becomes bootable again. This milestone binds the monotonic boot facts to hardware the disk cannot rewrite and exposes remote attestation of the running generation. It promotes directions register entry 5 and consumes M5.6 BootState semantics and M5.9 recovery; it requires a Framework-class TPM driver, which no earlier milestone scopes.

Scope:

- a bounded TPM driver sufficient for monotonic counters and NVRAM-sealed values;
- seal the attempt counter (or its epoch) and the known-good generation hash so the on-disk BootState must agree with TPM-held values at the stage-0 pre-transfer gate;
- a checked disk×TPM desync matrix with per-direction policy: disk newer than TPM routes to M5.9 recovery, TPM newer than disk (the resurrection case) fails closed, and a cleared TPM must never brick a healthy disk — preserving M5.6a's `SelectableBootRootExists`;
- attestation as the read direction: the TPM quotes the bound generation identity so a remote verifier learns what the machine runs (scope limited to boot state, not general measured boot);
- resolve whether QEMU verification gains a virtual TPM or this path is Framework-only, and how sealed counters interact with M5.9 reconstruction.

Required checks:

- reflashing an older generation image fails stage-0 verification against TPM-held counters;
- a cleared or unavailable TPM fails open only through the explicit M5.9 recovery path and never bricks a healthy disk;
- every desync-matrix cell resolves to its declared policy without leaving zero bootable roots;
- a remote verifier can distinguish two different running generation identities from their attestations.

Exit condition: on the Framework target, reflashing an older generation fails stage-0 verification against TPM-held counters, a cleared TPM cannot brick a healthy disk, and the running generation identity is remotely attestable — all without violating `SelectableBootRootExists`.

## Milestone 12: Distributed capabilities

**Status:** Not yet implemented.

The local authority model is complete: channels are typed (M5.2a), capabilities are unforgeable and non-ambient, and membranes make a proxied endpoint indistinguishable from a local one. This milestone takes the step beyond a single machine — a channel endpoint that proxies to a service elsewhere, with grants serialized as unforgeable wire capabilities (CapTP-style) that map back onto local grants on each side. The component model does not change; a tool call is still a typed IPC message. It promotes directions register entry 10 and depends on cross-machine sync (entry 14, M6-era) and M8 revocation (entry 2), plus M7 networking.

Scope:

- a cryptographic wire form for a capability whose minting, transfer, and presentation map back onto the local grant on each side, with each kernel independently enforcing;
- wire capabilities derived from the local grant so revoking the local subtree (M8 entry-2 trees) invalidates remote presentations; resolve whether derivation trees extend across machines or terminate at the wire with the sender retaining the backing grant;
- binding a wire capability to a session/transport identity so a replayed presentation from another context fails;
- explicit partition semantics: in-flight messages, replayed presentations after reconnect, and "endpoint unreachable" versus "capability revoked" all surface as structured errors in the existing channel vocabulary;
- reuse of the entry-7 membrane machinery for the proxy endpoint so recording and dry-run semantics extend across machines unchanged.

Required checks:

- a grant proxied to a remote service is usable there and remains unforgeable and non-ambient;
- revoking the local subtree invalidates the remote presentation;
- a captured presentation replayed from a different session or transport fails;
- partition, unreachable, and revoked conditions are distinguishable structured errors requiring no distributed-systems special case in components.

Exit condition: a capability proxied to a service on another machine is usable, unforgeable, and non-ambient across the wire, is revocable from the granting side, and resists replay from a foreign session — with partition and revocation surfaced as ordinary structured channel errors.
