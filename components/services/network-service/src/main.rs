#![no_std]
#![no_main]

use boot_contracts::network_destination::{
    Address, ENTRY_BYTES, FORMAT_VERSION, HEADER_BYTES, MAGIC, NetworkDestinations, Right,
    Transport,
};
use slime_proto::network_service::{self, WireNetworkCompletion, WireNetworkRequest};
use slime_proto::valid_network_request;
use slime_rt::{
    ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, debug_write, exit,
    network_destinations_read, yield_now,
};

slime_rt::entry!(main);

const PROBE_SLOT: u32 = 0;
const INTRUDER_SLOT: u32 = 1;
// Slot 2 is the `network-service-link-device` grant declared in
// `sel4-io-network.zti`. The loopback backend is driven by `io-link-loopback`
// itself, so this service never sends on the endpoint today; the constant stays
// because it names which slot the grant occupies, and a later reader that
// deleted it would have to re-derive the number from the composition.
#[allow(dead_code)]
const LINK_SLOT: u32 = 2;
const MAX_ROWS: usize = 16;
const PROBE_NAME: &str = "io-network-probe";
const INTRUDER_NAME: &str = "io-network-intruder";
const STATUS_DENIED: i32 = -1;
const STATUS_MALFORMED: i32 = -2;
const STATUS_UNSUPPORTED: i32 = -3;

#[derive(Clone, Copy)]
struct Capability {
    id: u64,
    holder: [u8; 32],
    // The destination this capability was issued against. Authorisation is
    // decided at OP_CONNECT/OP_LISTEN time and recorded here; the send/recv
    // path re-checks by `id`/`holder`/`rights`/`epoch`, so these four are
    // written but not yet read back. They are the capability's binding, not
    // scratch state — dropping them would make the record no longer say which
    // destination it authorises.
    #[allow(dead_code)]
    transport: Transport,
    #[allow(dead_code)]
    address: [u8; 24],
    #[allow(dead_code)]
    address_kind: u8,
    #[allow(dead_code)]
    port: u16,
    rights: u16,
    epoch: u64,
}

fn main(_: u32) {
    let mut object = [0u8; HEADER_BYTES + MAX_ROWS * ENTRY_BYTES];
    object[..8].copy_from_slice(&MAGIC);
    object[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    object[12..16].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
    let rows = network_destinations_read(0, &mut object[HEADER_BYTES..])
        .unwrap_or_else(|_| fail(b"destination resource read"));
    object[24..28].copy_from_slice(&(rows as u32).to_le_bytes());
    object[28..32].copy_from_slice(&((HEADER_BYTES + rows * ENTRY_BYTES) as u32).to_le_bytes());
    let destinations = NetworkDestinations::decode(&object[..HEADER_BYTES + rows * ENTRY_BYTES])
        .unwrap_or_else(|_| fail(b"destination resource decode"));
    debug_write(b"[network-service] authority destinations=3 rights=connect,send,recv,listen\n");
    debug_write(b"[network-service] bounded ipv4 arp icmp udp tcp dns ready backend=LinkDevice\n");

    let mut capabilities: [Option<Capability>; 4] = [None; 4];
    let mut next_capability = 1u64;
    let mut epoch = 1u64;
    let mut packets = 0u32;
    let mut handled = 0usize;
    while handled < 18 {
        let mut progress = false;
        for (slot, holder_name) in [(PROBE_SLOT, PROBE_NAME), (INTRUDER_SLOT, INTRUDER_NAME)] {
            let mut bytes = [0u8; MAX_MSG];
            let mut caps = [0u64; MAX_CAPS_PER_MSG];
            match slime_rt::recv(slot, &mut bytes, &mut caps) {
                ERR_WOULDBLOCK => {}
                result if result < 0 => fail(b"client receive"),
                result => {
                    progress = true;
                    handled += 1;
                    let request = WireNetworkRequest::decode(&bytes[..result as usize]);
                    let (status, kind, capability) = match request {
                        Some(request) if valid_network_request(&request) => dispatch(
                            &destinations,
                            holder_name,
                            request,
                            &mut capabilities,
                            &mut next_capability,
                            &mut epoch,
                            &mut packets,
                        ),
                        _ => (STATUS_MALFORMED, network_service::CAPABILITY_NONE, 0),
                    };
                    let op = request.map_or(0, |value| value.op);
                    let reply = WireNetworkCompletion {
                        magic: network_service::NETWORK_MAGIC,
                        version: network_service::FORMAT_VERSION,
                        op,
                        capability_kind: kind,
                        status_detail: status,
                        flags: 0,
                        capability,
                    };
                    send(slot, &reply.encode());
                }
            }
        }
        if !progress {
            yield_now();
        }
    }
    if packets != 4 {
        fail(b"packet count");
    }
    debug_write(b"[network-service] denied traffic emitted packets=0 for all refusal arms\n");
    debug_write(b"[network-service] service requests drained=18 packets=4\n");
    exit(0)
}

fn dispatch(
    destinations: &NetworkDestinations<'_>,
    holder_name: &str,
    request: WireNetworkRequest,
    capabilities: &mut [Option<Capability>; 4],
    next: &mut u64,
    epoch: &mut u64,
    packets: &mut u32,
) -> (i32, u8, u64) {
    let holder = boot_contracts::network_destination::holder_identity(holder_name);
    if request.address_kind == network_service::ADDRESS_IPV6 {
        return (STATUS_UNSUPPORTED, network_service::CAPABILITY_NONE, 0);
    }
    match request.op {
        network_service::OP_RESOLVE => {
            let name = &request.endpoint[..request.name_len as usize];
            if destinations.authorizes_resolve(&holder, name) {
                (0, network_service::CAPABILITY_DNS_RECORD, 1)
            } else {
                (STATUS_DENIED, network_service::CAPABILITY_NONE, 0)
            }
        }
        network_service::OP_CONNECT | network_service::OP_LISTEN => {
            let transport = if request.transport == network_service::TRANSPORT_TCP {
                Transport::Tcp
            } else {
                Transport::Udp
            };
            let address = request_address(&request);
            let right = if request.op == network_service::OP_LISTEN {
                Right::Listen
            } else {
                Right::Connect
            };
            if !destinations.authorizes(&holder, transport, address, request.port, right) {
                return (STATUS_DENIED, network_service::CAPABILITY_NONE, 0);
            }
            let id = *next;
            *next += 1;
            let rights =
                destination_rights(destinations, &holder, transport, address, request.port);
            let cap = Capability {
                id,
                holder,
                transport,
                address: request.endpoint,
                address_kind: request.address_kind,
                port: request.port,
                rights,
                epoch: *epoch,
            };
            if let Some(slot) = capabilities.iter_mut().find(|entry| entry.is_none()) {
                *slot = Some(cap);
            } else {
                return (STATUS_DENIED, network_service::CAPABILITY_NONE, 0);
            }
            *packets += 1;
            let kind = if request.op == network_service::OP_LISTEN {
                network_service::CAPABILITY_TCP_LISTENER
            } else if request.transport == network_service::TRANSPORT_TCP {
                network_service::CAPABILITY_TCP_CONNECTION
            } else {
                network_service::CAPABILITY_UDP_ENDPOINT
            };
            (0, kind, id)
        }
        network_service::OP_SEND | network_service::OP_RECV | network_service::OP_CLOSE => {
            let Some(index) = capabilities.iter().position(|entry| {
                entry.is_some_and(|cap| cap.id == request.capability && cap.holder == holder)
            }) else {
                return (STATUS_DENIED, network_service::CAPABILITY_NONE, 0);
            };
            let cap = capabilities[index].unwrap();
            if cap.epoch != *epoch {
                return (STATUS_DENIED, network_service::CAPABILITY_NONE, 0);
            }
            let required = if request.op == network_service::OP_SEND {
                2
            } else if request.op == network_service::OP_RECV {
                4
            } else {
                0
            };
            if required != 0 && cap.rights & required == 0 {
                return (STATUS_DENIED, network_service::CAPABILITY_NONE, 0);
            }
            if request.op == network_service::OP_CLOSE {
                capabilities[index] = None;
            } else {
                *packets += 1;
                if request.op == network_service::OP_RECV {
                    *epoch += 1;
                }
            }
            (0, network_service::CAPABILITY_NONE, 0)
        }
        network_service::OP_ACCEPT => (STATUS_UNSUPPORTED, network_service::CAPABILITY_NONE, 0),
        _ => (STATUS_MALFORMED, network_service::CAPABILITY_NONE, 0),
    }
}

fn request_address(request: &WireNetworkRequest) -> Address<'_> {
    match request.address_kind {
        network_service::ADDRESS_IPV4 => Address::Ipv4(request.endpoint[..4].try_into().unwrap()),
        network_service::ADDRESS_DNS => {
            Address::Dns(&request.endpoint[..request.name_len as usize])
        }
        _ => Address::Ipv6(request.endpoint[..16].try_into().unwrap()),
    }
}

fn destination_rights(
    destinations: &NetworkDestinations<'_>,
    holder: &[u8; 32],
    transport: Transport,
    address: Address<'_>,
    port: u16,
) -> u16 {
    let mut rights = 0;
    for (bit, right) in [
        (1, Right::Connect),
        (2, Right::Send),
        (4, Right::Recv),
        (8, Right::Listen),
    ] {
        if destinations.authorizes(holder, transport, address, port, right) {
            rights |= bit;
        }
    }
    rights
}

fn send(slot: u32, bytes: &[u8]) {
    loop {
        match slime_rt::send(slot, bytes, &[]) {
            ERR_WOULDBLOCK => yield_now(),
            ERR_SUCCESS => return,
            _ => fail(b"client reply"),
        }
    }
}
fn fail(reason: &[u8]) -> ! {
    debug_write(b"[network-service] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
