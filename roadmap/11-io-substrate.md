# Native I/O substrate

**Purpose:** Define and prove the architecture-neutral mechanisms that let supervised userspace drivers consume explicit hardware authority and expose typed semantic services. The substrate is shared by block, link/network, USB, audio, display, and future accelerator work without collapsing those protocols into a generic device interface.

**Status:** Not started. IO0 is the first slice. Existing C7 shared-buffer/loan accounting, C9.2 bounded WaitSets, C9.4 supervised restart, and the root-owned virtio-blk path are prerequisites and regression evidence, not completion of this track.

**Dependencies:** [Foundations](01-foundations.md), especially M5 block semantics and M6 capability/supervision machinery; [Core runtime](02-core-runtime.md), especially C7 shared buffers and loans, C9.2 Notification-backed WaitSets, and C9.4 restart/reclamation; [Architecture portability](07-architecture-portability.md), especially P1's source boundary and P5's current seL4 product path. Platform tracks supply concrete resource descriptions and physical containment evidence: Framework PCI/ACPI/APIC/AMD-IOMMU data remains in [H](04-platform-hardware.md), while Raspberry Pi 5 device-tree/controller qualification remains in P4/RP milestones.

The load-bearing rule is:

> **Share I/O mechanisms, not device semantics.**

The common shape is:

```text
capability
    │
typed control IPC
    │
bounded shared data
    │
request / lease
    │
completion
    │
Notification
    │
WaitSet
```

`slime-root` owns only the mechanism needed to construct, grant, account, revoke, and reclaim hardware resources. It does not parse NVMe commands, USB descriptors, packets, TCP state, audio formats, display damage, or GPU command streams. Those meanings stay in schema-first userspace protocols and services.

## Boundaries

- Control operations such as configure, query, bind, open, start, stop, cancel, reset, and close use small typed Endpoint IPC. Bulk payloads never enlarge the root control-message bound.
- Bulk data uses C7 shared buffers and bounded slices/leases. A wire descriptor names a buffer identity, offset, and length; ordinary clients never receive a physical address or unconstrained IOVA.
- CPU sharing and DMA authority are distinct. Holding or mapping a `SharedBuffer` does not authorize a device to DMA into it.
- Request/completion queues are a reusable userspace protocol/library pattern over shared memory plus Notifications. They are not a new seL4 object, a root-owned event multiplexer, or a universal device protocol.
- Notification wakeups mean readiness, not event counts. Badge bits may coalesce; a consumer drains every indicated source through the existing C9.2 WaitSet discipline.
- Hardware-facing authority and service-facing authority remain separate. Drivers may hold device, MMIO, interrupt, and DMA capabilities; ordinary clients hold semantic capabilities such as `BlockDevice`, `LinkDevice`, `NetworkDestination`, `TcpConnection`, `InputSeat`, `AudioStream`, or `DisplaySurface`.
- Device-specific requests remain separate Zutai protocols. `BlockRequest`, `PacketTx`, `UsbTransfer`, `AudioPeriod`, and a future GPU submission must never become variants of one `IoOpcode`.
- This track does not introduce file descriptors, generic `read`/`write`/`ioctl`, global PCI/USB enumeration, ambient sockets, or a universal `Device` object.
- Physical containment is target-qualified. IO1 defines the portable DMA authority and lifetime contract; H4 owns AMD-IOMMU proof, and a future Arm platform milestone owns any SMMU proof. A trusted-DMA profile must be explicit and cannot satisfy a physical containment claim.

## Existing evidence and migration boundary

The current seL4 product already proves one semantic storage path:

```text
component → Block capability → root-served BlockTransact → virtio-blk
```

P5.4.2 established device-untyped/MMIO/IRQ/DMA construction in `slime-root`, a bounded single-outstanding virtio-blk transport, and capability-checked read/write/flush behavior. That implementation is the behavioral oracle for IO2, but not the destination architecture: the root owns the driver only because no userspace device-resource model exists yet. IO2 moves device-specific queue handling into a supervised component, preserves the `BlockDevice` behavior, and removes the root driver after parity is observed.

## Sequencing

1. IO0 fixes the queue, request identity, epoch, cancellation, and buffer-lease invariants over C7 and C9.2.
2. IO1 exposes explicit device/MMIO/interrupt/DMA authority with bounded accounting and C9.4 restart/reclamation. It contains no device-specific opcode handling.
3. IO2 moves virtio-blk into a userspace driver and proves the complete substrate against the existing block behavior.
4. IO3 applies the same substrate to virtio-net and a typed `LinkDevice`, proving that the design supports duplex readiness and replenished receive buffers without block-specific exceptions.
5. IO4 builds the bounded network service and exact destination authority over `LinkDevice`. ROS R0/R1, foreign workloads, and Framework H6 consume this service rather than owning a second network architecture.
6. H2 binds IO1 to the Framework PCI profile. H3–H13 consume IO0–IO4 as applicable and add device-specific protocols, drivers, platform bindings, containment, and physical evidence.

## IO0 — Queue, identity, and buffer-lease contract

**Status:** Not started.

**Depends on:** C7 shared-buffer identities, mappings, loans, quotas, and fault reclamation; C9.2 bounded WaitSets; C9.4 supervised restart terminal states.

### Deliverables

- define a schema-first bounded buffer-slice descriptor carrying only shared-buffer identity, offset, length, access direction, and lease identity; validate integer overflow, bounds, access rights, and zero/overlap rules before queue admission;
- define `RequestId` and `DriverEpoch` as independent bounded identities carried by every asynchronous request and completion; a restart, reset, disconnect/reconnect, or generation transition creates a fresh epoch before new work is admitted;
- define one reusable request/completion queue envelope whose generic fields cover capacity, producer/consumer positions, request identity, epoch, payload extent, and completion status while the request and completion payload schemas remain protocol-specific;
- implement a `no_std` queue/lease library for fixed-capacity rings, full/empty detection, backpressure, wraparound, acquire/release ordering, malformed-entry rejection, and drain-after-Notification behavior;
- define the request lifecycle `Queued → InFlight → Complete | Cancelled | Reset | PeerDead`, with every terminal transition single-assignment and every buffer lease released or invalidated exactly once;
- define cancellation as a request with an observable terminal result, not best-effort disappearance; a late completion after cancellation, reset, or peer death is rejected and cannot resurrect the request or lease;
- account queue entries, outstanding requests, leased buffers, mapped bytes, and completion backlog against generation-declared fixed bounds;
- keep the existing Endpoint, SharedBuffer, Notification, and WaitSet mechanisms unchanged unless a failing proof demonstrates a missing primitive.

### Required checks

- fixed capacities reject zero, oversized, non-power-of-two where required, and arithmetic-overflow layouts before mapping or allocation;
- a full request or completion ring returns defined backpressure and never overwrites an unconsumed entry;
- malformed positions, duplicate request identities within an epoch, wrong epochs, invalid buffer slices, and impossible lifecycle transitions fail closed;
- memory backing an in-flight request cannot be released, remapped for conflicting access, or reused by the client until the lease reaches one terminal state;
- driver death/reset invalidates every outstanding request and lease, returns all queue/loan charges, and permits a fresh epoch to start without accepting an old completion;
- Notification coalescing does not lose progress: one wake followed by draining all indicated queues reaches quiescence without busy polling;
- protocol-specific payloads remain separately generated from their owning Zutai schemas, with no generic opcode discriminator.

### Planned verification target

```sh
just io_queue_check
```

### Exit condition

Two supervised components exchange protocol-specific requests and completions through fixed shared rings, buffer leases, Notifications, and a WaitSet; full queues backpressure, cancellation and peer death settle every request, restart creates a fresh epoch, and stale or malformed completions cannot reclaim or resurrect memory.

## IO1 — Hardware resource authority and DMA accounts

**Status:** Not started.

**Depends on:** IO0, C7 generation-v3 rights and quotas, C9.4 supervision/reclamation, and the P1/P5 architecture boundary.

### Deliverables

- add explicit hardware-resource capability classes for one physical device instance, bounded MMIO regions, one interrupt source, a DMA account/domain, and charged DMA mappings/buffers; exact names land with the capability-matrix and generation-schema change rather than as hand-maintained constants;
- let `slime-root` construct and grant only the resources declared by the admitted target profile, map only bounded subranges, bind only the declared interrupt, and reclaim every mapping/object on task termination or supervision-subtree restart;
- separate shared-buffer authority from DMA mapping authority: a driver requests a direction-scoped DMA mapping for a live lease, and the resource layer returns only the driver-visible token/IOVA required to program its granted device;
- enforce per-driver limits for MMIO bytes/mappings, DMA pages/mappings, interrupt sources and pending acknowledgements, outstanding requests, and shared-buffer loans before allocating or mapping;
- issue a fresh driver epoch and fresh resource mappings after restart; stale MMIO handles, interrupt acknowledgements, DMA mappings, and completions from the prior instance fail closed;
- model platform facts as target-profile data: PCI BDF/BAR, virtio-mmio address, device-tree interrupt, ACPI route, APIC vector, AMD-IOMMU alias, or SMMU stream identity never becomes a universal Slime ABI field;
- support an explicitly declared trusted-DMA profile for deterministic QEMU bring-up without representing it as IOMMU containment; physical promotion remains blocked on the owning platform gate;
- keep all device command parsing, queue formats, reset policy, and service semantics in userspace drivers/services.

### Required checks

- a component without the exact device resource cannot enumerate devices, map MMIO, allocate/map DMA memory, receive or acknowledge an interrupt, or map another holder's buffer;
- a driver maps only its granted region and requested bounded subrange; wrong device, offset, length, cache/access mode, or duplicate mapping fails before touching the VSpace;
- DMA allocation and live mappings cannot exceed the generation account, and memory remains charged and unreclaimable while a hardware request owns it;
- one driver receives only its declared interrupt; spoofed, wrong-source, duplicate, and stale acknowledgements are rejected;
- crash/restart revokes mappings and interrupt authority, settles IO0 requests, returns every charge, and starts with a fresh epoch;
- ordinary service clients observe only typed semantic capabilities and buffer descriptors, never physical addresses, IOVAs, BDFs, vectors, or global enumeration results;
- no device-specific opcode, descriptor parser, or retry policy enters `slime-root`.

### Planned verification target

```sh
just io_driver_authority_check
```

### Exit condition

A manifest-declared userspace driver receives exactly one device instance, its bounded MMIO, DMA account, interrupt, shared-data endpoints, and supervision handle; an ungranted component receives none of them, crash/restart returns every charge with a fresh epoch, and the root remains unaware of device semantics.

## IO2 — Userspace virtio-blk and asynchronous BlockDevice plane

**Status:** Not started.

**Depends on:** IO0, IO1, and the existing M5/P5 block behavior and QEMU storage gates.

### Deliverables

- preserve the existing schema-first `BlockDevice` semantics — geometry query, read, write, and flush with explicit rights — while extending the protocol only where asynchronous multi-block requests require bounded slices, request identities, epochs, and completions;
- move virtio-blk feature negotiation, queue setup, descriptors, notification/interrupt handling, timeout, reset, and stale-completion logic from `slime-root` into one supervised userspace driver component holding IO1 resources;
- use IO0 request/completion rings and buffer leases for multi-sector operations instead of growing the transfer-window RPC or accepting caller physical addresses;
- preserve capability-selected device identity and the userspace object-store/generation/recovery layers above `BlockDevice`; clients receive no virtio, MMIO, DMA, or IRQ authority;
- retain the current root-mediated one-sector path only during the migration as a behavioral oracle; after read/write/flush, denial, durability, fault, and restart parity are observed, remove the root's virtio-blk command/descriptor implementation from the product path;
- define reset and cancellation so every submitted request reaches one terminal state, every DMA/lease charge returns, and completions from a prior driver epoch are refused;
- preserve deterministic disposable-image tests and the no-physical-safety-claim boundary: IO2 proves QEMU mechanism, not Framework NVMe or internal-storage promotion.

### Required checks

- read, write, flush, geometry, rights denial, out-of-range LBA, malformed request, short buffer, unsupported feature, and durable fresh-boot behavior match the existing storage-plane oracle;
- multiple queued requests complete without overwrite or identity confusion, and a full ring backpressures the caller;
- injected descriptor failure, timeout, cancellation, reset, interrupt loss/coalescing, driver crash, and peer death settle every request and reclaim every descriptor, DMA mapping, lease, and charge;
- a restarted driver has a fresh epoch, and a deliberately injected old-epoch completion is rejected without modifying the new request or buffer;
- filesystem, object-store, generation, rollback, and recovery clients still reach storage only through their declared `BlockDevice` capability;
- the product root contains no virtio-blk opcode or descriptor parsing after cutover.

### Planned verification target

```sh
just io_block_check
```

### Exit condition

A supervised userspace virtio-blk driver provides the existing capability-gated read/write/flush behavior through asynchronous bounded buffers and completions, survives injected reset/crash/stale-completion cases without leaked authority or memory, and leaves `slime-root` responsible only for IO1 resource construction and reclamation.

## IO3 — Userspace virtio-net and LinkDevice validation

**Status:** Not started.

**Depends on:** IO0, IO1, and IO2 as the first complete substrate proof.

### Deliverables

- define a versioned `LinkDevice` service for bounded frame transmit, receive-buffer provisioning, link state, statistics, reset, and close; Ethernet frame meaning remains here rather than in the common queue envelope;
- implement a supervised userspace virtio-net driver over IO1 resources with separate bounded TX/RX queues, replenished RX leases, interrupt/readiness Notifications, feature negotiation, reset, and fresh epochs;
- keep raw link authority confined to the network service and explicit diagnostics; an application with a future `TcpConnection` or `UdpEndpoint` cannot submit arbitrary frames;
- apply IO0 backpressure and lifetime rules to duplex traffic, including RX buffers owned by the device until completion and TX buffers retained through completion/cancellation;
- prove that the same substrate supports continuous readiness and receive replenishment without adding block-only or network-only operations to `slime-root` or IO0/IO1;
- expose backend-independent `LinkDevice` behavior so Framework USB Ethernet and Wi-Fi can attach later without changing the network-service client ABI.

### Required checks

- a component without `LinkDevice` authority cannot transmit, receive, query link state, or obtain raw frames;
- malformed virtio descriptors, oversized/short frames, exhausted RX replenishment, TX saturation, interrupt coalescing, link reset, and driver crash remain within declared bounds;
- full TX queues backpressure, exhausted RX buffers drop or pause according to the declared policy, and neither path overwrites live data;
- restart returns every DMA/lease charge, creates a fresh epoch, and rejects stale TX/RX completions;
- deterministic QEMU fixtures transmit allowed frames and prove denied/raw access produces no packet;
- no IP, DNS, UDP, TCP, or destination-authority policy enters the driver or root.

### Planned verification target

```sh
just io_link_check
```

### Exit condition

A userspace virtio-net driver exposes one bounded `LinkDevice` with duplex queueing, readiness, reset, and restart behavior over the same IO0/IO1 substrate as virtio-blk, without widening raw-packet authority or introducing device-specific root logic.

## IO4 — Network service and exact destination authority

**Status:** Not started.

**Depends on:** IO3, C9 clocks/WaitSets/restart where timers and reconnect use them, and generation/capability introspection.

### Deliverables

- declare versioned link-facing, IP, DNS, UDP, TCP, and network-service protocols under `contracts/`, consuming `LinkDevice` as the only physical/virtual NIC boundary;
- implement bounded Ethernet, ARP/NDP, IPv4/IPv6, ICMP, DHCP/SLAAC, UDP, TCP, and exact-name DNS resolution sufficient for native update, diagnostics, the RPi5 Zenoh TCP path, and later external ROS interoperability;
- add a `NetworkDestination` generation object identifying transport, exact IP address or exact DNS name, and port, with separate CONNECT, SEND, RECV, and LISTEN rights; wildcard destinations require a separately named future authority and are not implicit;
- issue typed service-facing capabilities such as `TcpConnection` and `UdpEndpoint`; clients receive no NIC, raw-packet, resolver-wide, or ambient socket authority;
- keep DNS authority inside the service: resolving one declared name does not grant arbitrary lookup, alternate addresses, or a raw resolver endpoint;
- bound sockets, listeners, queued bytes, fragments, retransmission state, timers, DNS records, reconnect attempts, and per-destination traffic in generation data;
- keep target backends separate: IO3 supplies the virtio-net QEMU reference, H6 qualifies Framework USB Ethernet, H12 adds Framework Wi-Fi, and an RPi5 milestone qualifies its selected physical link.

### Required checks

- a component granted one destination reaches only that exact name/address, transport, port, and rights set; alternate address, port, DNS name, raw packet, resolver, and listen attempts fail closed;
- malformed frames, DHCP options, DNS messages, fragments, TCP options, sequence/window state, retransmission exhaustion, and peer loss cannot exceed declared bounds or wedge unrelated connections;
- the manifest and authority-diff tooling enumerate every reachable destination and distinguish connection, send, receive, and listen authority;
- QEMU transfers deterministic data to one allowed endpoint while a simultaneous denied endpoint receives no packet;
- link unplug/reset and network-service or driver restart invalidate stale connection/request epochs, reclaim queues and buffers, and reconnect only where the declared policy permits it;
- R0/RP5 can obtain the one bounded TCP byte stream they declare without acquiring discovery, router, wildcard, or raw-link authority.

### Planned verification target

```sh
just io_network_check
```

### Exit condition

Native components obtain bounded TCP/UDP services only for generation-declared exact destinations over a backend-independent `LinkDevice`; the virtio-net reference path survives malformed traffic, denial, link reset, and restart, and no client receives ambient socket or packet authority.

## Consumption by later subsystems

Later milestones reuse the substrate but retain their own semantics:

```text
BlockDevice   = typed block control + IO0 buffers/completions
TcpConnection = typed stream control + bounded stream queues/readiness
UsbEndpoint   = typed USB transfer/completion payloads over IO0
AudioStream   = typed format/control + PCM ring + period/xrun events
DisplaySurface = typed surface/presentation + bounded shared images
GpuQueue      = typed GPU submits + buffer objects + fences
```

This table is a boundary, not a promise that the protocols are interchangeable. USB descriptors remain H3/USB-service work; input/seat semantics remain H3/H11; audio formats and mixer policy remain H11; surface/focus/presentation remain H8; Radeon command validation remains H13; general accelerator compute authority remains A3.

## I/O track verification stack

Every implementation slice runs its narrowest deterministic host/QEMU gate. New rights and operations update the capability matrix and syscall ABI in the same change; new cross-component layouts are Zutai-first. Permanent Rust changes also run the repository-required format and lint gates.

Planned slice targets:

```sh
just io_queue_check
just io_driver_authority_check
just io_block_check
just io_link_check
just io_network_check
```

Physical target checks belong to the consuming platform milestone and cannot complete an IO slice by substitution. Conversely, an IO QEMU pass cannot complete Framework or Raspberry Pi 5 peripheral support.

## I/O track definition of done

The common substrate is complete only when:

- request identity, driver epoch, cancellation, terminal states, queue bounds, Notification draining, and buffer-lease lifetime are specified and observed under malformed input and peer failure;
- hardware resources are explicit generation-declared capabilities with bounded MMIO, DMA, IRQ, shared-buffer, and outstanding-request accounting;
- `SharedBuffer` alone never grants DMA, ordinary clients never see physical addresses/IOVAs, and target-specific controller identifiers remain profile data;
- crash/restart settles or invalidates every request, revokes mappings, returns charges, creates a fresh epoch, and rejects stale completions;
- userspace virtio-blk preserves the existing block behavior and removes device-specific parsing from the root product path;
- userspace virtio-net proves the same substrate under duplex readiness/replenishment without protocol-specific substrate hooks;
- the network service enforces exact destination authority over a backend-independent link service;
- future USB, audio, display, and GPU work can consume queue/buffer/lease/completion mechanisms without adding a universal opcode or moving device semantics into `slime-root`.
