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
| 5. Storage and generations | In progress — M5.1 complete | A failed pending generation automatically leaves or restores a bootable known-good generation. |
| 6. Native interactive environment | Minimal stub only | Native components can inspect, build or stage, select, and roll back generations. |
| 7. Daily-driver hardware | Not yet implemented | The Framework target supports the hardware and lifecycle needed for daily use. |

## Architectural constraints

Every milestone must preserve these project invariants:

- The kernel owns only privileged mechanisms: scheduling, address spaces, memory objects, capability enforcement, IPC, interrupts, timers, and minimal platform control.
- Device, filesystem, generation, health, activation, and rollback policy belongs in userspace services.
- Authority is carried by explicit capabilities. There are no ambient executable paths, storage handles, working directories, streams, or environment state.
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

**Status:** In progress. M5.1 (storage capability foundation) is complete; the read-only virtio block slice (M5.2) is next.

Top-level scope:

- virtio block, followed by the Framework NVMe transport;
- GPT and an integrity-checked object store;
- immutable generations;
- pending and known-good boot state;
- rollback and garbage-collection roots;
- explicit persistent-state policy.

Exit condition: a failed pending generation automatically leaves or restores a bootable known-good generation.

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
- the `storage_cap_check` QEMU target (`kernel/tests/storage_capability.rs`).

The remaining gaps include:

- no virtio transport or device backend behind the block protocol;
- no GPT or persistent object store;
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

Planned verification target:

```sh
just storage_read_check
```

The target should create a disposable fixture, attach it with `readonly=on`, boot QEMU, exercise the userspace request path, and require a successful guest exit.

Exit condition: a userspace component reads and verifies data from a read-only QEMU virtio block device without gaining ambient storage authority.

### M5.3: Durable virtio writes and fault handling

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

### M5.4: GPT and integrity-checked object store

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

### M5.6: Pending, known-good, rollback, state policy, and GC

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

Follow-ups enabled by this milestone (not exit requirements): generation bisect (automated boot-and-health-check search over the parent chain) and shadow boot (health-checking a pending generation in a constrained environment before consuming a real boot attempt). Both consume only mechanisms this milestone already requires.

### M5.7: Framework NVMe transport and safety promotion

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

### Milestone 5 verification stack

Each permanent change should run the narrowest QEMU scenario that exercises its new behavior. Before a Milestone 5 slice is accepted, the existing repository gates must remain clean:

```sh
just contracts_check
just generation_check
just test
just fmt_check
just lint
```

Storage slices additionally require their scenario targets. `storage_cap_check` exists today; the others are planned:

```sh
just storage_cap_check
just storage_read_check
just storage_write_check
just storage_fault_check
just rollback_check
```

Physical-machine evidence is separate from QEMU evidence. QEMU can prove deterministic logic and fault handling; it cannot prove actual Framework firmware behavior, DMA containment, device identity, power-loss behavior, or absence of writes to internal hardware.

### Milestone 5 definition of done

Milestone 5 is complete only when all of the following are observed:

- every executable object is content verified before execution;
- staging cannot modify the running or known-good generation;
- a pending boot attempt is persistently consumed before control transfers to it;
- userspace health confirmation atomically promotes only the running pending generation;
- interruption at every metadata commit boundary leaves at least one valid `BootState` slot;
- exhausted or failed pending generations automatically boot the known-good generation;
- GC never removes known-good, pending, running, rollback, staged, or persistent-state roots;
- every persistent-state policy has an upgrade and rollback test;
- storage read and write authority is granted only through explicit generation capabilities;
- malformed storage and generation metadata is rejected before out-of-bounds I/O or execution;
- the existing isolated component graph remains healthy under QEMU;
- the Framework storage-aware slice is observed without unauthorized internal NVMe writes.

## Milestone 6: Native interactive environment

**Status:** Minimal stub only.

Scope:

- minimal Dango implementation and core runtime;
- command profile/resolver and spawn service;
- filesystem service and directory capabilities;
- generation inspection and update commands;
- a powerbox-style file dialog service where the user's selection gesture mints a single-object capability;
- generation sync/transfer between machines (object transfer plus staged activation).

This milestone consumes the storage, object-store, state, and rollback mechanisms from Milestone 5. Dango commands must resolve executable and directory authority through capabilities rather than global paths or an implicit working directory.

Exit condition: the system can inspect, build or stage, select, and roll back generations through native components.

## Milestone 7: Daily-driver hardware

**Status:** Not yet implemented.

Bring hardware up in risk order rather than feature visibility:

1. xHCI, USB HID, mass storage, and USB Ethernet;
2. native storage reliability and IOMMU-enforced DMA;
3. software-rendered display/compositor over GOP;
4. battery, charger, brightness, lid, thermal, and suspend/resume lifecycle;
5. touchpad and audio;
6. MT7925 Wi-Fi and Bluetooth;
7. Radeon display control and hardware acceleration.

GPU acceleration, Wi-Fi, and audio do not block the first native userspace milestone, but they are required before the Framework target can be called a daily-use system.

Daily-driver quality goals for this milestone also include per-component energy accounting and per-destination network authority declared in the generation. MPK/PKU lightweight compartments are an optional optimization and do not block the exit condition.

Exit condition: the Framework target supports the hardware, DMA containment, power lifecycle, input, networking, audio, and display behavior required for daily use without bypassing the capability or generation model.
