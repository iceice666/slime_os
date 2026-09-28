# Network destination authority, version 1

`v1/schema.zt` owns the versioned destination resource and generated layout.
`boot-contracts/src/network_destination.rs` validates exact holder, transport,
address or DNS name, port and independent rights; there is no wildcard field.
A declaration is authority, not evidence that a transport is implemented.

The current [network service](../network-service/README.md) implements IPv4 TCP
connections only. Live connections reserve socket buffers, a socket, a queue
slot and a timer against the exact destination row. Retry limit bounds repeated
TCP sequence-bearing segments over the connection lifetime. `reconnect_limit`
bounds automatic recovery attempts; it does not cap explicit application
`OP_CONNECT` requests. The current service performs zero automatic reconnects.

Holder/destination egress enforcement applies only to TCP. Interface-owned ARP
and IPv4 ICMP responses, including automatic echo replies, do not require an
application connection or consume these destination budgets. Fixed link-buffer
storage does not imply per-holder ARP/ICMP rate limiting. Local listener
authority is separately declared by
[network-application/v2](../network-application/README.md).
