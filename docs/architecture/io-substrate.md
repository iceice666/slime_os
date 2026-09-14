# Userspace I/O substrate

## Boundary

Device semantics live in supervised userspace drivers and services. `slime-root`
creates and accounts explicit device, MMIO, interrupt, DMA, shared-buffer, and
supervision authority; it does not parse block, Ethernet, USB, audio, display, or
GPU commands on the product path.

The substrate shares asynchronous lifetime and accounting mechanisms, not one
universal device protocol. Each device class keeps its own Zutai request and
completion schema.

Current owners:

- generic queue/epoch/lease format: `contracts/io-queue/v1/`;
- hardware authority and budgets: `contracts/io-resource/v1/`,
  `contracts/block-authority/v1/`, `boot-contracts/src/io_resource.rs`, and
  `slime-root/src/{device,io_resource}.rs`;
- shared ring adapters: `components/lib/src/`;
- virtio-blk and virtio-net drivers: `components/services/virtio-*-driver/`;
- exact network destinations: `contracts/network-destination/v1/`;
- declared network interface/MAC/address: `contracts/network-interface/v1/`;
- service protocols: `contracts/block/v2/`, `contracts/link-device/v1/`, and
  `contracts/network-service/v1/`.

## Queue, request, and lease semantics

A queue has one client-written cache line and one driver-written cache line,
fixed power-of-two submission and completion rings, an absolute sequence space,
and one driver epoch. A request identity is unique only inside that epoch. Reset,
restart, disconnect, or generation transition advances the epoch so a late
completion from an old driver is rejected.

Requests move through `Queued -> InFlight -> Complete | Cancelled | Reset |
PeerDead`. Terminal state is single-assignment and every terminal answer settles
the request's buffer lease exactly once. Notification badges indicate which ring
must be drained; they do not count events.

A buffer-slice descriptor names shared-buffer identity, lease, offset, length,
and direction. It contains no physical address or IOVA. CPU access through a
`SharedBuffer` and device access through a DMA mapping are independent
authorities. Ordinary service clients never receive raw DMA or MMIO authority.

## Hardware authority and drivers

A driver receives only generation-declared resource capabilities: one device,
exact MMIO subranges, declared interrupt sources, a bounded DMA account, queue
bindings, and supervision. Bounds cover mapping bytes/counts, DMA pages/counts,
interrupt sources, outstanding requests, and buffer loans. Task death and
restart revoke the old resources and charges.

The product's block and link paths are userspace drivers. `virtio-blk-driver`
serves typed block requests over IO0 rings. `virtio-net-driver` exposes one
bounded `LinkDevice` with duplex queueing and replenished receive buffers. The
boot selector retains a narrow pre-admission block reader compiled only into
selector images; it is not a root product driver.

QEMU uses trusted DMA and makes no containment claim. Physical containment is a
target-specific IOMMU/SMMU qualification and cannot be inherited from an emulator
or another board.

## Network boundary

The current network service enforces exact per-holder destination authority and
bounded socket/listener/DNS-record accounting. Its application-facing contract
names TCP/UDP operations and typed capability results without NIC identity or raw
packet authority.

When generation data declares a network interface, the service attaches a
`LinkDevice`, replenishes receive buffers, and polls a smoltcp interface.
The `sel4-io-tcp` composition connects it to `virtio-net-driver`; the current
gate checks Ethernet/ARP/IPv4/ICMP traffic and orderly link reset/release.

It does **not** yet expose a TCP/UDP payload stream. The gate explicitly expects
`tcp=0`; typed connection capabilities alone do not establish socket transport.
DNS, listen/accept, payload-path reset/restart qualification, and backend
independence remain unfinished. An authority-only composition may omit the
interface and attach no link.

The implementation and qualification plan is
[`../plans/network-data-plane.md`](../plans/network-data-plane.md).

## Verification

- `just io_queue_check` — queue, epoch, cancellation, and lease behavior;
- `just io_driver_authority_check` — exact hardware authority and reclamation;
- `just io_block_check` — userspace block driver behavior;
- `just io_link_check` — userspace virtio-net/LinkDevice behavior;
- `just io_network_check` — exact-destination authority plus the linked
  ARP/ICMP arm; `just io_tcp_check` selects the latter, not a TCP-stream claim;
- `just io_queue_model_check`, `just io_resource_model_check`, and the Kani I/O
  proof targets — bounded interleavings and wire/device arithmetic.

Physical checks remain owned by the consuming platform plan; no QEMU or model
result completes a device-specific hardware claim.
