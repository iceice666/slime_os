#![no_std]
#![no_main]

use boot_contracts::network_destination::{
    Address, Destination, ENTRY_BYTES, FORMAT_VERSION, HEADER_BYTES, MAGIC, MAX_DESTINATIONS,
    NetworkDestinations, OFF_HEADER_DESTINATION_COUNT, OFF_HEADER_FORMAT_VERSION,
    OFF_HEADER_HEADER_SIZE, OFF_HEADER_MAGIC, OFF_HEADER_REQUIRED_FLAGS, OFF_HEADER_TOTAL_LEN,
    RIGHT_CONNECT, RIGHT_LISTEN, RIGHT_RECV, RIGHT_SEND, Right, Transport,
};
use slime_proto::network_service::{self, WireNetworkCompletion, WireNetworkRequest};
use slime_proto::valid_network_request;
use slime_rt::{
    ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, debug_write, exit,
    network_destinations_read, resolve_binding, yield_now,
};

slime_rt::entry!(main);

const MAX_ROWS: usize = MAX_DESTINATIONS;
const PAGE_ROWS: usize = 6;
const MAX_CAPABILITIES: usize = 8;
const SHUTDOWN_CAPABILITY: u64 = u64::MAX;
const STATUS_DENIED: i32 = -1;
const STATUS_MALFORMED: i32 = -2;
const STATUS_UNSUPPORTED: i32 = -3;

#[derive(Clone, Copy)]
struct Client {
    slot: u32,
    holder: [u8; 32],
    closed: bool,
}

#[derive(Clone, Copy)]
struct Capability {
    id: u64,
    holder: [u8; 32],
    destination: usize,
    rights: u16,
    kind: u8,
    epoch: u64,
}

#[derive(Default)]
struct Observed {
    requests: u32,
    packets: u32,
    socket_refusals: u32,
    listener_refusals: u32,
    dns_refusals: u32,
    cross_holder_refusals: u32,
}

fn main(_: u32) {
    let mut object = [0u8; HEADER_BYTES + MAX_ROWS * ENTRY_BYTES];
    object[OFF_HEADER_MAGIC..OFF_HEADER_MAGIC + MAGIC.len()].copy_from_slice(&MAGIC);
    object[OFF_HEADER_FORMAT_VERSION..OFF_HEADER_FORMAT_VERSION + 4]
        .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    object[OFF_HEADER_HEADER_SIZE..OFF_HEADER_HEADER_SIZE + 4]
        .copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
    object[OFF_HEADER_REQUIRED_FLAGS..OFF_HEADER_REQUIRED_FLAGS + 8]
        .copy_from_slice(&0u64.to_le_bytes());

    let mut rows = 0usize;
    loop {
        if rows == MAX_ROWS {
            break;
        }
        let mut page = [0u8; PAGE_ROWS * ENTRY_BYTES];
        let read = network_destinations_read(rows, &mut page)
            .unwrap_or_else(|_| fail(b"destination resource read"));
        if read == 0 {
            break;
        }
        if read > PAGE_ROWS || rows + read > MAX_ROWS {
            fail(b"destination resource page");
        }
        let bytes = read * ENTRY_BYTES;
        let start = HEADER_BYTES + rows * ENTRY_BYTES;
        object[start..start + bytes].copy_from_slice(&page[..bytes]);
        rows += read;
    }
    object[OFF_HEADER_DESTINATION_COUNT..OFF_HEADER_DESTINATION_COUNT + 4]
        .copy_from_slice(&(rows as u32).to_le_bytes());
    let total = HEADER_BYTES + rows * ENTRY_BYTES;
    object[OFF_HEADER_TOTAL_LEN..OFF_HEADER_TOTAL_LEN + 4]
        .copy_from_slice(&(total as u32).to_le_bytes());
    let destinations = NetworkDestinations::decode(&object[..total])
        .unwrap_or_else(|_| fail(b"destination resource decode"));
    report_authority(&destinations);

    let mut clients = [
        Client {
            slot: binding(b"network-probe-service"),
            holder: boot_contracts::network_destination::holder_identity("io-network-probe"),
            closed: false,
        },
        Client {
            slot: binding(b"network-intruder-service"),
            holder: boot_contracts::network_destination::holder_identity("io-network-intruder"),
            closed: false,
        },
    ];
    let mut capabilities: [Option<Capability>; MAX_CAPABILITIES] = [None; MAX_CAPABILITIES];
    let mut next_capability = 1u64;
    // This is the service/link generation. It changes only when that generation
    // restarts, never for an individual receive operation.
    let epoch = 1u64;
    let mut observed = Observed::default();

    while clients.iter().any(|client| !client.closed) {
        let mut progress = false;
        for client in &mut clients {
            if client.closed {
                continue;
            }
            let mut bytes = [0u8; MAX_MSG];
            let mut caps = [0u64; MAX_CAPS_PER_MSG];
            match slime_rt::recv(client.slot, &mut bytes, &mut caps) {
                ERR_WOULDBLOCK => {}
                result if result < 0 => fail(b"client receive"),
                result => {
                    progress = true;
                    observed.requests += 1;
                    let request = WireNetworkRequest::decode(&bytes[..result as usize]);
                    let op = request.map_or(0, |value| value.op);
                    let (status, kind, capability) = match request {
                        Some(request)
                            if valid_network_request(&request)
                                && request.op == network_service::OP_CLOSE
                                && request.capability == SHUTDOWN_CAPABILITY =>
                        {
                            client.closed = true;
                            (0, network_service::CAPABILITY_NONE, 0)
                        }
                        Some(request) if valid_network_request(&request) => dispatch(
                            &destinations,
                            client.holder,
                            request,
                            &mut capabilities,
                            &mut next_capability,
                            epoch,
                            &mut observed,
                        ),
                        _ => (STATUS_MALFORMED, network_service::CAPABILITY_NONE, 0),
                    };
                    let reply = WireNetworkCompletion {
                        magic: network_service::NETWORK_MAGIC,
                        version: network_service::FORMAT_VERSION,
                        op,
                        capability_kind: kind,
                        status_detail: status,
                        flags: 0,
                        capability,
                    };
                    send(client.slot, &reply.encode());
                }
            }
        }
        if !progress {
            yield_now();
        }
    }
    report_observed(&observed);
    exit(0)
}

fn dispatch(
    destinations: &NetworkDestinations<'_>,
    holder: [u8; 32],
    request: WireNetworkRequest,
    capabilities: &mut [Option<Capability>; MAX_CAPABILITIES],
    next: &mut u64,
    epoch: u64,
    observed: &mut Observed,
) -> (i32, u8, u64) {
    if request.address_kind == network_service::ADDRESS_IPV6 {
        return (STATUS_UNSUPPORTED, network_service::CAPABILITY_NONE, 0);
    }
    match request.op {
        network_service::OP_RESOLVE => {
            let name = &request.endpoint[..request.name_len as usize];
            let Some((index, destination)) = find_resolve_destination(destinations, &holder, name)
            else {
                return (STATUS_DENIED, network_service::CAPABILITY_NONE, 0);
            };
            let charged = capabilities
                .iter()
                .flatten()
                .filter(|cap| {
                    cap.holder == holder
                        && cap.destination == index
                        && cap.kind == network_service::CAPABILITY_DNS_RECORD
                })
                .count() as u32;
            if charged >= destination.dns_record_limit {
                observed.dns_refusals += 1;
                return (STATUS_DENIED, network_service::CAPABILITY_NONE, 0);
            }
            mint(
                capabilities,
                next,
                holder,
                index,
                destination.rights,
                network_service::CAPABILITY_DNS_RECORD,
                epoch,
            )
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
            let Some((index, destination)) = find_destination(
                destinations,
                &holder,
                transport,
                address,
                request.port,
                right,
            ) else {
                return (STATUS_DENIED, network_service::CAPABILITY_NONE, 0);
            };
            let sockets = capabilities
                .iter()
                .flatten()
                .filter(|cap| {
                    cap.holder == holder
                        && cap.destination == index
                        && cap.kind != network_service::CAPABILITY_DNS_RECORD
                })
                .count() as u32;
            if sockets >= destination.socket_limit {
                observed.socket_refusals += 1;
                return (STATUS_DENIED, network_service::CAPABILITY_NONE, 0);
            }
            let kind = if request.op == network_service::OP_LISTEN {
                let listeners = capabilities
                    .iter()
                    .flatten()
                    .filter(|cap| {
                        cap.holder == holder
                            && cap.destination == index
                            && cap.kind == network_service::CAPABILITY_TCP_LISTENER
                    })
                    .count() as u32;
                if listeners >= destination.listener_limit {
                    observed.listener_refusals += 1;
                    return (STATUS_DENIED, network_service::CAPABILITY_NONE, 0);
                }
                network_service::CAPABILITY_TCP_LISTENER
            } else if request.transport == network_service::TRANSPORT_TCP {
                network_service::CAPABILITY_TCP_CONNECTION
            } else {
                network_service::CAPABILITY_UDP_ENDPOINT
            };
            observed.packets += 1;
            mint(
                capabilities,
                next,
                holder,
                index,
                destination.rights,
                kind,
                epoch,
            )
        }
        network_service::OP_SEND | network_service::OP_RECV | network_service::OP_CLOSE => {
            let Some(index) = capabilities
                .iter()
                .position(|entry| entry.is_some_and(|cap| cap.id == request.capability))
            else {
                return (STATUS_DENIED, network_service::CAPABILITY_NONE, 0);
            };
            let cap = capabilities[index].unwrap();
            if cap.holder != holder {
                observed.cross_holder_refusals += 1;
                return (STATUS_DENIED, network_service::CAPABILITY_NONE, 0);
            }
            if cap.epoch != epoch {
                return (STATUS_DENIED, network_service::CAPABILITY_NONE, 0);
            }
            if cap.kind == network_service::CAPABILITY_TCP_LISTENER
                && matches!(
                    request.op,
                    network_service::OP_SEND | network_service::OP_RECV
                )
            {
                return (STATUS_UNSUPPORTED, network_service::CAPABILITY_NONE, 0);
            }
            let required = if request.op == network_service::OP_SEND {
                RIGHT_SEND
            } else if request.op == network_service::OP_RECV {
                RIGHT_RECV
            } else {
                0
            };
            if required != 0 && cap.rights & required == 0 {
                return (STATUS_DENIED, network_service::CAPABILITY_NONE, 0);
            }
            if request.op == network_service::OP_CLOSE {
                capabilities[index] = None;
            } else {
                observed.packets += 1;
            }
            (0, network_service::CAPABILITY_NONE, 0)
        }
        network_service::OP_ACCEPT => (STATUS_UNSUPPORTED, network_service::CAPABILITY_NONE, 0),
        _ => (STATUS_MALFORMED, network_service::CAPABILITY_NONE, 0),
    }
}

fn mint(
    capabilities: &mut [Option<Capability>; MAX_CAPABILITIES],
    next: &mut u64,
    holder: [u8; 32],
    destination: usize,
    rights: u16,
    kind: u8,
    epoch: u64,
) -> (i32, u8, u64) {
    let Some(slot) = capabilities.iter_mut().find(|entry| entry.is_none()) else {
        return (STATUS_DENIED, network_service::CAPABILITY_NONE, 0);
    };
    let id = *next;
    *next += 1;
    *slot = Some(Capability {
        id,
        holder,
        destination,
        rights,
        kind,
        epoch,
    });
    (0, kind, id)
}

fn find_destination<'a>(
    destinations: &'a NetworkDestinations<'a>,
    holder: &[u8; 32],
    transport: Transport,
    address: Address<'_>,
    port: u16,
    right: Right,
) -> Option<(usize, Destination<'a>)> {
    (0..destinations.destination_count()).find_map(|index| {
        let destination = destinations.destination(index)?;
        (destination.holder_identity == *holder
            && destination.transport == transport
            && destination.address == address
            && destination.port == port
            && destination.rights & right_bit(right) != 0)
            .then_some((index, destination))
    })
}

fn right_bit(right: Right) -> u16 {
    match right {
        Right::Connect => RIGHT_CONNECT,
        Right::Send => RIGHT_SEND,
        Right::Recv => RIGHT_RECV,
        Right::Listen => RIGHT_LISTEN,
    }
}

fn find_resolve_destination<'a>(
    destinations: &'a NetworkDestinations<'a>,
    holder: &[u8; 32],
    name: &[u8],
) -> Option<(usize, Destination<'a>)> {
    (0..destinations.destination_count()).find_map(|index| {
        let destination = destinations.destination(index)?;
        (destination.holder_identity == *holder
            && matches!(destination.address, Address::Dns(value) if value == name)
            && destination.rights & RIGHT_CONNECT != 0
            && destination.dns_record_limit > 0)
            .then_some((index, destination))
    })
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

fn report_authority(destinations: &NetworkDestinations<'_>) {
    let mut rights = 0u16;
    let mut sockets = 0u64;
    let mut listeners = 0u64;
    let mut dns = 0u64;
    for index in 0..destinations.destination_count() {
        let destination = destinations.destination(index).unwrap();
        rights |= destination.rights;
        sockets += u64::from(destination.socket_limit);
        listeners += u64::from(destination.listener_limit);
        dns += u64::from(destination.dns_record_limit);
    }
    write_number(
        b"[network-service] authority destinations=",
        destinations.destination_count() as u64,
    );
    debug_write(b" rights=");
    let mut separator = b"".as_slice();
    for (bit, name) in [
        (RIGHT_CONNECT, b"connect".as_slice()),
        (RIGHT_SEND, b"send".as_slice()),
        (RIGHT_RECV, b"recv".as_slice()),
        (RIGHT_LISTEN, b"listen".as_slice()),
    ] {
        if rights & bit != 0 {
            debug_write(separator);
            debug_write(name);
            separator = b",";
        }
    }
    debug_write(b"\n");
    write_number(b"[network-service] declared socket_limit=", sockets);
    write_number(b" listener_limit=", listeners);
    write_number(b" dns_record_limit=", dns);
    debug_write(b"\n");
}

fn report_observed(observed: &Observed) {
    write_number(
        b"[network-service] observed requests=",
        u64::from(observed.requests),
    );
    write_number(b" packets=", u64::from(observed.packets));
    write_number(b" socket_refusals=", u64::from(observed.socket_refusals));
    write_number(
        b" listener_refusals=",
        u64::from(observed.listener_refusals),
    );
    write_number(b" dns_refusals=", u64::from(observed.dns_refusals));
    write_number(
        b" cross_holder_refusals=",
        u64::from(observed.cross_holder_refusals),
    );
    debug_write(b"\n");
}

fn binding(name: &[u8]) -> u32 {
    resolve_binding(name).unwrap_or_else(|_| fail(b"binding"))
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

fn write_number(prefix: &[u8], mut value: u64) {
    let mut digits = [0u8; 20];
    let mut offset = digits.len();
    loop {
        offset -= 1;
        digits[offset] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    debug_write(prefix);
    debug_write(&digits[offset..]);
}

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[network-service] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
