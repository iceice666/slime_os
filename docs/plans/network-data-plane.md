# Network data plane over LinkDevice

**Canonical work item:** `01a08ff0-0b99-7315-99b0-7e7f340fa6f9`

## Current gap

The product has a userspace virtio-net `LinkDevice` and a destination-authority
service. The `sel4-io-tcp` composition already connects them: generation-declared
interface data configures smoltcp, the service exchanges Ethernet/ARP/IPv4/ICMP
traffic, and shutdown resets/releases the link. An authority-only composition
may omit the interface.

The unfinished boundary is the application payload stream. TCP capability
creation does not yet open a socket transport, and the linked gate expects
`tcp=0`. TCP/UDP payloads, DNS, listen/accept, payload-path reset/restart
qualification, and backend independence remain to be implemented and observed.
The first TCP stream item is `01a08ff0-0bae-741e-9318-331fdafe0b96`.

## Planned scope

Complete bounded application transport inside `network-service`, consuming the
existing LinkDevice attachment and preserving its interface contract:

1. implement TCP payload transport, then the remaining UDP/DNS/service handling,
   with versioned Zutai boundary contracts and bounded protocol state;
2. exact-name or exact-address destination authority with independent CONNECT,
   SEND, RECV, and LISTEN rights; no wildcard or ambient socket grant;
3. typed service capabilities such as `TcpConnection`, `UdpEndpoint`, and
   listeners, while clients receive no NIC, raw-packet, resolver-wide, or DMA
   authority;
4. bounded queues, fragments, retransmission state, timers, DNS records,
   reconnect attempts, sockets, listeners, and bytes per destination;
5. driver/service reset, peer loss, stale completion, and supervised restart
   behavior over fresh IO0 epochs.

Address configuration not already declared by current contracts requires its own
schema change and evidence. IPv6/NDP, DHCP, SLAAC, multicast discovery, and a
general listener/accept service are not implied by the first TCP slice.

## Required behavior

Under QEMU, a component granted one destination can exchange a TCP byte stream
with that exact peer. An alternate address, port, DNS name, transport, raw frame,
resolver operation, or ungranted listen attempt fails before data leaves the
service. Malformed frames and packets remain bounded; unplug, reset, and restart
settle or invalidate every request and release all buffers, mappings, timers,
and charges.

The first implementation is target-neutral above `LinkDevice` and claims no
physical NIC. Framework, Raspberry Pi 5, and Milk-V Duo link qualification remain
separate target evidence.

## Verification ownership

Extend the existing I/O/network plane checker rather than creating a second
framework. The gate must exercise real bytes through the driver boundary and
retain authority-denial and stale-epoch controls. Contract changes run
`just contracts_check`; permanent Rust changes run repository format and lint
gates.

## Owning references

- [`../architecture/io-substrate.md`](../architecture/io-substrate.md)
- `contracts/{link-device,network-destination,network-service,io-queue}/v1/`
- `components/services/{virtio-net-driver,network-service}/`
- `.tasks/items/01a08ff0-0b99-7315-99b0-7e7f340fa6f9.md`
