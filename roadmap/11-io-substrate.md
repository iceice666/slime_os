# Native I/O substrate

**Purpose:** Define and prove the architecture-neutral mechanisms that let supervised userspace drivers consume explicit hardware authority and expose typed semantic services. The substrate is shared by block, link/network, USB, audio, display, and future accelerator work without collapsing those protocols into a generic device interface.

**Status:** IO0, IO1, IO3, and IO4 complete 2026-08-28. IO2 complete except the root
cutover's last two planes. Each slice is observed under QEMU by its own gate — `just
io_queue_check`, `just io_driver_authority_check`, `just io_block_check`,
`just io_link_check`, `just io_network_check` — and every gate is registered in
`just sel4_gate_control_check`, which proves each fails on missing, reordered, or
failure evidence.

What is **not** done, stated plainly so no consumer assumes otherwise:

- **IO2's root cutover, for two planes.** Six of the eight block-holding
  compositions — `sel4-storage`, `sel4-store`, `sel4-rollback`, `sel4-replay`,
  `sel4-generation`, and `sel4-filesystem` — now reach their devices through the
  userspace driver over IO0 rings, each gated on generation-declared per-ring rights
  (`contracts/block-authority/v1`). The authority prerequisite this depended on is
  landed: a ring's rights are declared rather than inferred, and the driver produces
  `STATUS_BAD_RIGHTS` for a submission outside them.

  `slime-root`'s virtio-blk command/descriptor implementation and its
  `BlockTransact` path remain the product path for `sel4-recovery` and
  `sel4-transfer`. Both declare two attached devices and IO1 grants exactly one
  device per driver instance, so neither can be composed against the userspace
  driver at all. Tracked as backlog B84, which owns the mechanism gap.
- **IO4's declared-but-refused subset:** IPv6/NDP, DHCP, SLAAC, and the TCP
  listener/accept data path answer `STATUS_UNSUPPORTED`.
- **Physical containment.** Every slice is trusted-DMA on QEMU. No IOMMU exists here,
  so no containment claim is made; H4 owns AMD-IOMMU proof and a future Arm milestone
  owns any SMMU proof.
- **Physical link and storage qualification** remain H6/H12/RP and Framework work.

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

**Status:** Complete 2026-08-28. The contract, the `no_std` queue/lease library, and
the two-component QEMU proof all landed; `just io_queue_check` boots the plane and
asserts round trip, backpressure, late-completion rejection, reset epoch cutover,
and malformed-slice refusal, with 54 host tests behind the structural refusals.
**Evidence:** [`devlog/2026-08-28-io0-queue-substrate/`](../devlog/2026-08-28-io0-queue-substrate/index.md)

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

**Status:** Complete. Observed under QEMU by `just io_driver_authority_check`:
generation-declared device binding, exact-subrange mediated MMIO with out-of-range
refusal, refusal to widen a shared-granule region to its page rather than mapping it,
bounded interrupt authority with spoof refusal, opaque DMA paths exposing no physical
address, and an ungranted component denied device, MMIO, DMA, and interrupt authority.
The owner-spawned driver now faults with live MMIO, IRQ, and driver-owned queue-DMA
authority; task death performs real unmap, IRQ unbind, DMA destruction, charge return,
and request settlement before task-object reclamation. The boot transcript reports
4096 MMIO bytes, one MMIO mapping, one IRQ source, two DMA pages, one DMA mapping,
and zero outstanding requests reclaimed to exact zero, then respawns the driver at
epoch 2 and refuses predecessor epoch 1. IO1's restart tally includes DMA charges now
that live-loan payload DMA and driver-owned contiguous queue DMA have landed.
**Evidence:** [`devlog/2026-08-28-io1-hardware-resource-authority/`](../devlog/2026-08-28-io1-hardware-resource-authority/index.md)

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
- a driver maps only its granted region and requested bounded subrange; direct mapping additionally requires page exclusivity, while shared-granule regions use mediated `read32`/`write32` whose per-access bounds are stricter than page-granular mapping; wrong device, offset, length, cache/access mode, or duplicate mapping fails before touching the VSpace;
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

The boot-selector's disk is a bounded ordering exception: it must be probed before the generation stored on it can be decoded, so decoded generation policy cannot select its own prerequisite. No other device inherits that exception; after decode, hardware ownership is generation-driven and mutually exclusive with the legacy root driver.

## IO2 — Userspace virtio-blk and asynchronous BlockDevice plane

**Status:** Complete except the root cutover's last two planes; the authenticated per-ring rights prerequisite is landed and six of eight block-holding compositions are migrated. QEMU generation 51 boots the userspace virtio-blk plane through root-mediated bounded MMIO, proves read/write/flush/geometry and negative parity, durable fresh-boot readback, eight-request identity-safe queuing and full-ring backpressure, numeric zero-leak settlement for descriptor failure, timeout, cancellation, reset, interrupt loss/coalescing, driver crash, and peer death, plus fresh-epoch restart with stale-completion rejection. `just io_block_check` returns exit 0 after an explicit probe-to-driver shutdown rendezvous.

**Spawn-grant prerequisite: landed, and the previous diagnosis corrected.** Two earlier passes reported that a dynamically spawned storage client could not receive its crossing bindings because init's spawn supplied zero grant records, and each reverted a working migration believing the root lacked the mechanism. That conclusion was wrong. The root already derives crossing grants from the generation: adding a declared `sharedBufferFactory` grant (source `init`, target `sel4-storage-probe`) to `sel4-storage.zti` made preflight report `requested=0 parent=1 minted=0 respawn=false` — it had counted the declaration correctly and refused only because init passed nothing. The gap was entirely init-side. `drive_probe_plane_with_token` now takes the exact grant vector its manifest declares, and the storage plane boots with `SLIME_GRAPH spawn authorized task=0 slot=1 component=sel4-storage-probe grants=1` and `buffer_factory_grants=1`. The same idiom was already in production on the sample plane (`sample-lender-shared-buffer-factory`), so this is a use of the existing mechanism rather than a second one.

The rule is now pinned where a host gate can reach it. `grant_crosses_spawn` and the new `declared_crossing_grants` moved from the binary-only dispatcher into `slime-root/src/generation.rs`, so the dispatcher and the tests share one implementation; `just test_sel4_root` is 205. Widening stays unrepresentable because the count comes from declarations, leaving no index at which an owner can place an undeclared grant.

**Per-ring rights prerequisite: landed 2026-08-28.** The root's
`serve_block_transact` identified the caller from the endpoint badge and checked that
caller's `BlockDevice` for `blockRead` or `blockWrite` on every request. A userspace
driver sees only submissions in shared ring memory, and neither the IO0 envelope nor a
submission binds a ring to authenticated client rights — so `STATUS_BAD_RIGHTS` was
defined by `io-queue/v1` and produced by nothing.

`contracts/block-authority/v1` closes that on IO4's precedent. Each entry binds one
holder, on one device, to one ring, with independent read and write bits and a sector
ceiling; the driver reads its own table through the root's identity-gated cursor-paged
path (label 69) and refuses a submission outside it. Two properties are structural
rather than checked: `(device, ring)` is ordered strictly ascending *without* the
holder, so one ring can never carry two holders' rows — which is what lets the driver
say whose rights a submission carries — and no field can express a wildcard holder,
a device range, or an "all rights" value. The root reads no block right at any point;
it authenticates who may read the table and bounds the bytes.

**Six of eight planes are migrated.** `sel4-storage`, `sel4-store`, `sel4-rollback`,
`sel4-replay`, `sel4-generation`, and `sel4-filesystem` reach their devices only
through the userspace driver, over one shared `components/lib/src/block_io.rs`
adapter, and each gate now asserts the driver's authority read, its device bring-up,
its clean release, and the root's numeric DMA reclamation in place of the retired
`SLIME_GRAPH block served` corroboration. `just sel4_gate_control_check` rose from
1761 to 1779 rejected mutations, so coverage grew rather than moved.

**Two planes remain on the root path, and the reason is a mechanism gap.**
`sel4-recovery` holds a writable recovery disk and a read-only guard disk whose
byte-identity its gate asserts; `sel4-transfer` holds a source and a receiver. IO1
grants exactly one device per driver instance (`DeviceId(1)` is hardcoded in
`slime-root/src/graph_runtime/services/io_resource.rs`), and within one instance every
non-`block` typed grant shares a single positional index, so two devices cannot be
expressed even by declaring the driver twice. Tracked as backlog B84. An attempt to
give `device` grants their own counter was reverted: the shared index meant
incrementing on `Device` shifted each driver's `mmioRegion` index and broke every
plane's virtio handshake at once.

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

**Status:** Complete. `just io_link_check` builds, boots generation 52 under QEMU, and
passes: the supervised userspace `virtio-net-driver` negotiates the legacy transport
with no optional feature, programs two 16-slot virtqueues, and serves one bounded
`LinkDevice` over the same IO0 queues and IO1 authority as virtio-blk. Observed in the
transcript: link state up; one 60-byte frame transmitted and its address-swapped echo
received and byte-verified from the deterministic UDP backend; eight transmits accepted
then a ninth refused `Full` with the ring's submitted count unchanged; receive
replenishment paused rather than reusing a device-owned buffer; eight completions
drained from a single wake (`max-per-wake=8`); undersized and oversized frames refused
with `device-programmed=0`; a frame longer than the slice it names refused
`STATUS_BAD_SLICE` with `device-programmed=0`; reset settling one transmit and one
receive request (`tx=1 rx=1 leases=2`); restart reclaiming every charge numerically
(`dma=0 requests=0 leases=0`, corroborated by the root's own
`SLIME_IO reclaim … post_dma_pages=0 post_dma_mappings=0 post_requests=0`); a fresh
epoch `old=1 new=2` with one stale transmit and one stale receive completion refused;
and the intruder denied all four raw-link operations with no packet emitted. The MMIO
mechanism exercised is **mediated** bounded `read32`/`write32` — QEMU packs eight
0x200 transports into one 4KiB granule, so the region is not page-exclusive and the
direct-map path is not admitted. Interrupt *authority* is granted and reclaimed
(`reclaimed_irq_sources`), but this device completes faster than the line is
dispatched, so completions are serviced by draining the used ring; no
interrupt-sequence marker is claimed.
**Evidence:** [`devlog/2026-08-28-io3-userspace-virtio-net/`](../devlog/2026-08-28-io3-userspace-virtio-net/index.md)

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

**Status:** Complete 2026-08-28 for the declared subset; `just io_network_check` boots
the plane and reports exact destinations, denials, reset, restart, and backend
independence proved. A granted client reaches exactly its declared TCP/UDP
destination and DNS name; eleven separate denial arms — alternate address, alternate
port, alternate DNS name, wrong transport, each of the four missing rights, raw
packet, resolver-wide lookup, and listen without LISTEN — each observe zero packets.
Reset and restart reclaim every queue, buffer, and lease with a fresh epoch and
refuse stale completions.

**Declared but structurally refused as `STATUS_UNSUPPORTED`,** and therefore *not*
claimed: IPv6 and NDP, DHCP, SLAAC, and the TCP listener/accept data path beyond a
structured refusal. Those remain declared in `contracts/network-service/v1` so
adding them is an implementation change rather than a contract change. Anything
consuming IO4 that needs one of them — an RPi5 or ROS path that requires DHCP, or a
listening service — must treat it as unfinished.

**Backend:** proved against a deterministic in-plane `io-link-loopback` `LinkDevice`
provider, which is legitimate evidence precisely because backend independence is an
IO4 deliverable. The virtio-net reference backend is IO3's and is qualified there;
physical link qualification remains H6/H12/RP.
**Evidence:** [`devlog/2026-08-28-io4-network-service/`](../devlog/2026-08-28-io4-network-service/index.md)

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
