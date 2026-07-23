# Foreign workloads

| | |
| --- | --- |
| **Purpose** | Run selected Linux workloads without changing Slime's native ABI or importing Linux's ambient authority model. |
| **Status** | Not started. X1 is the first compatibility route; X2 is a later, separately gated route. |
| **Dependencies** | X1 consumes the completed M5.4 content-addressed store and M6 spawn, endpoint-minting, and userspace-filesystem machinery in [Foundations](01-foundations.md), plus the H6 network-service contract in [Platform hardware](04-platform-hardware.md) for networked workloads. X2 additionally requires H4 AMD-IOMMU containment and an observed AMD-V virtualization-enablement gate. [ROS 2 compatibility](03-ros2-compatibility.md) R3 may consume either route. |

Linux compatibility remains a userspace facility. Native Slime components continue to use the capability-based Slime ABI; neither track adds Linux syscalls, FHS paths, process-global environment state, or ambient filesystem and network lookup to that ABI. A foreign workload is still one generation-declared component contract: a content-addressed image, a selected compatibility backend, bounded configuration, and an explicit grant set.

## X1: Linux userspace personality

**Status:** Not started. This is the primary and first compatibility route.

### Deliverables

- Implement a userspace personality component that loads a selected Linux binary from a content-addressed M5.4 object. Image identity and integrity use the existing generation verification path; the personality never searches an executable path or global image registry.
- Define a fixed, versioned, audited syscall-to-service table for a minimal real workload. Each admitted syscall names its Slime IPC service, required grant, argument and resource bounds, result mapping, and Linux errno when the grant or operation is unavailable. Unsupported or ungranted calls fail closed with the appropriate ordinary Linux error; they never widen the table or mint authority dynamically.
- Translate `open`, `read`, and `write` into object-store or filesystem-service transactions bounded by explicit directory or object grants. There is no implicit root, current directory, home directory, descriptor, or path-search authority.
- Translate socket operations through the H6 network service and exact `NetworkDestination` grants. Name resolution remains inside that service and does not imply arbitrary resolver, raw-packet, listen, or destination authority.
- Translate time and randomness requests through explicit clock and entropy grants. Absence of either grant is reported to the guest program rather than substituted with an ambient source.
- Map `fork` and `exec` assumptions onto the M6 spawn service. Every child receives an explicit subset of the personality's held endpoints and can never acquire more authority than its parent. Process counts, descriptors, argument and environment bytes, IPC payloads, queues, and outstanding operations are deterministically bounded.
- Treat environment entries and other startup values as bounded, immutable generation inputs, not inherited host state. Undeclared variables are absent; they cannot convey paths, credentials, endpoints, or other hidden authority.
- Declare the image, personality version, syscall profile, startup data, resource bounds, and complete service-grant set in the generation. The resulting authority is auditable, diffable, and rollbackable like that of every native component.

### Required checks

- A declared container reads and writes only the files covered by its directory or object grants; attempts outside those grants fail with the mapped Linux error and do not issue an unauthorized service operation.
- It reaches only its declared network destinations. Undeclared names, addresses, ports, transports, listen rights, and raw network access are denied with a normal Linux error.
- Every unsupported syscall and every syscall lacking its required filesystem, network, clock, entropy, spawn, or other service grant fails with its specified errno rather than escalating or discovering ambient state.
- A spawned child holds no grant absent from its explicit child grant set and no authority exceeding the parent.
- Changing host filesystem state, process environment, working directory, or network configuration cannot add authority to an unchanged generation.
- The manifest exposes the container's image identity, compatibility profile, bounded startup data, and complete grant set, and the existing authority-diff path reports changes to any of them.

**Exit condition:** A Linux binary declared as a container in the generation runs under the personality, confined to its declared directory, network, time, randomness, process, and other service grants; everything else is denied with a normal Linux errno, and its complete authority is visible and diffable in the manifest.

## X2: Isolated AMD-V guest VM

**Status:** Later. X2 does not block X1 and MUST NOT begin hardware-backed guest execution until H4 IOMMU containment and AMD-V enablement have been observed on the target.

X2 reuses X1's generation-level foreign-workload contract rather than creating another authority model. The backend choice may change fidelity and cost, but never the workload's grants.

### Deliverables

- Add a bounded hypervisor route for a full Linux guest under AMD-V, with explicit guest-memory ownership, nested-page mappings, virtual-CPU and VM-exit bounds, and fail-closed handling of unsupported virtualization features. Do not claim Framework AMD-V support until firmware/CPU availability and the enablement path are physically observed.
- Present only a fixed audited virtio subset backed by Slime userspace services. Each virtual block, network, clock, entropy, console, and other admitted endpoint requires the corresponding generation grant; absent devices are not discoverable as usable authority.
- Bind virtual block devices to named content-addressed image/state objects and virtual network devices to the declared network-service rights and `NetworkDestination` grants. The guest receives no host filesystem, host environment, raw device, or unrestricted network access.
- Keep every physical DMA-capable backend behind H4's active AMD-IOMMU domain before bus mastering. Guest memory and virtio buffers expose only the bounded mappings required for the current operation, and mappings are invalidated before reuse.
- Declare the guest image, VM/backend version, bounded virtual hardware configuration, resource limits, and complete virtio/service grant set in the generation, using the same audit, diff, verification, and rollback paths as X1.

### Required checks

- Deterministic QEMU checks exercise the VM state machine, bounds, malformed descriptors, denied and absent virtio devices, and service-error mapping; a QEMU pass is architecture evidence only and does not satisfy the physical AMD-V or IOMMU gate.
- On the named Framework target, AMD-V enablement is observed before a guest starts, and attempted startup fails closed when virtualization is unavailable or unsupported.
- No DMA-capable backend starts bus mastering before its H4 domain is active; malformed or out-of-range guest descriptors cannot address guest memory outside the declared mapping or any unrelated host memory.
- The guest can access only the content-addressed disks, declared destinations, time, entropy, console, and other virtio-backed services present in its generation grant set. Removing a grant removes the corresponding usable authority without exposing a host fallback.
- Host paths, working directory, process environment, network configuration, and undeclared devices cannot affect the authority of an unchanged guest generation.
- The manifest and authority diff expose the same complete workload grant contract whether its selected backend is X1 or X2.

**Exit condition:** After the physical AMD-V and H4 IOMMU gates pass, a generation-declared Linux guest runs under AMD-V with bounded guest memory and an audited virtio surface; it can use only its declared service-backed devices and destinations, all physical DMA remains IOMMU-contained, and its complete authority is visible and diffable in the manifest. QEMU verification alone cannot complete X2.

## ROS 2 relationship

[R3](03-ros2-compatibility.md) may run an existing ROS workload through X1 when its required Linux syscall profile is supported, or through X2 when it needs a full guest kernel. In either case, its image, filesystem, network, discovery, time, randomness, process, device, and other authority remain generation-declared explicit grants.

R1 and R2 do not depend on X1, X2, Linux, or existing ROS binaries. They establish ROS 2 topic, service, and action interoperability as userspace protocol profiles over native Slime contracts; existing ROS binaries become a requirement only for R3's existing-workload checks.
