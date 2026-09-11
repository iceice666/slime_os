#![no_std]
#![no_main]

use boot_contracts::network_destination::{
    Address, Destination, ENTRY_BYTES, FORMAT_VERSION, HEADER_BYTES, MAGIC, MAX_DESTINATIONS,
    NetworkDestinations, OFF_HEADER_DESTINATION_COUNT, OFF_HEADER_FORMAT_VERSION,
    OFF_HEADER_HEADER_SIZE, OFF_HEADER_MAGIC, OFF_HEADER_REQUIRED_FLAGS, OFF_HEADER_TOTAL_LEN,
    RIGHT_CONNECT, RIGHT_LISTEN, RIGHT_RECV, RIGHT_SEND, Right, Transport,
};
use boot_contracts::network_interface::{self, Interface as DeclaredInterface, NetworkInterfaces};
use slime_components::tick_clock::TickClock;
use slime_proto::network_service::{self, WireNetworkCompletion, WireNetworkRequest};
use slime_proto::valid_network_request;
use slime_rt::{
    ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, debug_write, exit, monotonic_frequency,
    monotonic_read, network_destinations_read, network_interface_read, resolve_binding, yield_now,
};
use smoltcp::iface::{Config, Interface, PollResult, SocketSet, SocketStorage};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, Ipv4Address};

mod link;
use link::Link;

slime_rt::entry!(main);

const MAX_ROWS: usize = MAX_DESTINATIONS;
const PAGE_ROWS: usize = 6;
const INTERFACE_PAGE_ROWS: usize = 4;
/// The link peer endpoint and the buffer factory sit at fixed slots, as the
/// IO3 probe's do: a loan names its receiver by endpoint slot, and the root
/// reads that number as an endpoint only while no shared-buffer capability
/// occupies it, so both must sit below the first buffer this service creates.
const LINK_PEER_SLOT: u32 = 0;
const FACTORY_SLOT: u32 = 1;
const MAX_CAPABILITIES: usize = 8;
/// TCP sockets the stack can hold at once. The contract's `maxSockets` is a
/// ceiling on declarations; this is the storage one service instance carries.
const TCP_SOCKETS: usize = 4;
/// The clients a generation may bind to this service, by the grant name each
/// resolves and the instance name its holder identity derives from. A grant
/// the generation does not declare simply resolves to no client.
const CLIENTS: [(&[u8], &str); 3] = [
    (b"network-probe-service", "io-network-probe"),
    (b"network-intruder-service", "io-network-intruder"),
    (b"network-tcp-probe-service", "io-tcp-probe"),
];
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

/// Everything that exists only when the generation binds this service to a
/// link and declares its interface.
struct Stack {
    link: Link,
    iface: Interface,
    clock: TickClock,
}

impl Stack {
    fn now(&self) -> Instant {
        let ticks = monotonic_read().unwrap_or_else(|_| fail(b"monotonic read"));
        Instant::from_millis(self.clock.millis(ticks))
    }
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

    let mut clients: [Option<Client>; CLIENTS.len()] = [None; CLIENTS.len()];
    for (entry, (grant, name)) in clients.iter_mut().zip(CLIENTS) {
        if let Ok(slot) = resolve_binding(grant) {
            *entry = Some(Client {
                slot,
                holder: boot_contracts::network_destination::holder_identity(name),
                closed: false,
            });
        }
    }
    if clients.iter().all(|client| client.is_none()) {
        fail(b"no client binding");
    }
    let mut capabilities: [Option<Capability>; MAX_CAPABILITIES] = [None; MAX_CAPABILITIES];
    let mut next_capability = 1u64;
    // This is the service/link generation. It changes only when that generation
    // restarts, never for an individual receive operation.
    let epoch = 1u64;
    let mut observed = Observed::default();

    let mut stack = attach_stack();
    let mut socket_storage: [SocketStorage; TCP_SOCKETS] = [SocketStorage::EMPTY; TCP_SOCKETS];
    let mut sockets = SocketSet::new(&mut socket_storage[..]);

    while clients.iter().flatten().any(|client| !client.closed) {
        let mut progress = false;
        for client in clients.iter_mut().flatten() {
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
        if let Some(stack) = stack.as_mut() {
            progress |= stack.link.drain();
            let now = stack.now();
            progress |= stack.iface.poll(now, &mut stack.link, &mut sockets)
                == PollResult::SocketStateChanged;
            progress |= stack.link.replenish();
        }
        if !progress {
            yield_now();
        }
    }
    if let Some(mut stack) = stack {
        release_stack(&mut stack);
    }
    report_observed(&observed);
    exit(0)
}

/// Bind to the link and configure the stack when the generation declares an
/// interface for this service; a generation that declares none runs the
/// authority service alone, and binds no link.
fn attach_stack() -> Option<Stack> {
    let declared = read_interface()?;
    let rate = monotonic_frequency().unwrap_or_else(|_| fail(b"clock rate"));
    let base = monotonic_read().unwrap_or_else(|_| fail(b"monotonic read"));
    let clock = TickClock::new(rate, base).unwrap_or_else(|_| fail(b"clock rate too slow"));
    write_number(b"[network-service] clock rate=", rate);
    debug_write(b"\n");
    report_interface(&declared);

    let mut link = Link::attach(FACTORY_SLOT, LINK_PEER_SLOT);
    if !link.link_up() {
        fail(b"link down");
    }
    link.replenish();
    write_number(
        b"[network-service] link query state=up rx provisioned=",
        link.rx_provisioned() as u64,
    );
    debug_write(b"\n");

    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(declared.mac)));
    config.random_seed = base;
    let now = Instant::from_millis(clock.millis(base));
    let mut iface = Interface::new(config, &mut link, now);
    iface.update_ip_addrs(|addrs| {
        addrs
            .push(IpCidr::new(
                IpAddress::Ipv4(Ipv4Address::from(declared.address)),
                declared.prefix_len,
            ))
            .unwrap_or_else(|_| fail(b"interface address"));
    });
    if let Some(gateway) = declared.gateway {
        iface
            .routes_mut()
            .add_default_ipv4_route(Ipv4Address::from(gateway))
            .unwrap_or_else(|_| fail(b"default route"));
    }
    Some(Stack { link, iface, clock })
}

/// Hand the link back: reset the driver, acknowledge its settled completions,
/// and wait for its fresh epoch before this task can exit and have its loans
/// reclaimed from under the driver.
fn release_stack(stack: &mut Stack) {
    let counts = link::FrameCounts {
        frames: stack.link.tx.counts.frames + stack.link.rx.counts.frames,
        arp: stack.link.tx.counts.arp + stack.link.rx.counts.arp,
        icmp: stack.link.tx.counts.icmp + stack.link.rx.counts.icmp,
        tcp: stack.link.tx.counts.tcp + stack.link.rx.counts.tcp,
        other: stack.link.tx.counts.other + stack.link.rx.counts.other,
    };
    write_number(
        b"[network-service] link frames total=",
        u64::from(counts.frames),
    );
    write_number(b" tx=", u64::from(stack.link.tx.counts.frames));
    write_number(b" rx=", u64::from(stack.link.rx.counts.frames));
    write_number(b" arp=", u64::from(counts.arp));
    write_number(b" icmp=", u64::from(counts.icmp));
    write_number(b" tcp=", u64::from(counts.tcp));
    write_number(b" other=", u64::from(counts.other));
    debug_write(b"\n");
    let (tx_frames, rx_frames) = stack.link.statistics();
    write_number(
        b"[network-service] link statistics tx=",
        u64::from(tx_frames),
    );
    write_number(b" rx=", u64::from(rx_frames));
    debug_write(b"\n");
    stack.link.release();
    debug_write(b"[network-service] link released\n");
}

fn read_interface() -> Option<DeclaredInterface> {
    let mut object = [0u8; network_interface::MAX_BYTES];
    object[network_interface::OFF_HEADER_MAGIC..network_interface::OFF_HEADER_MAGIC_END]
        .copy_from_slice(&network_interface::MAGIC);
    object[network_interface::OFF_HEADER_FORMAT_VERSION
        ..network_interface::OFF_HEADER_FORMAT_VERSION_END]
        .copy_from_slice(&network_interface::FORMAT_VERSION.to_le_bytes());
    object
        [network_interface::OFF_HEADER_HEADER_SIZE..network_interface::OFF_HEADER_HEADER_SIZE_END]
        .copy_from_slice(&(network_interface::HEADER_BYTES as u32).to_le_bytes());
    let mut rows = 0usize;
    loop {
        if rows == network_interface::MAX_INTERFACES {
            break;
        }
        let mut page = [0u8; INTERFACE_PAGE_ROWS * network_interface::ENTRY_BYTES];
        // No interface object at all is a legitimate generation: the service
        // then serves authority alone.
        let read = network_interface_read(rows, &mut page).ok()?;
        if read == 0 {
            break;
        }
        if read > INTERFACE_PAGE_ROWS || rows + read > network_interface::MAX_INTERFACES {
            fail(b"interface resource page");
        }
        let bytes = read * network_interface::ENTRY_BYTES;
        let start = network_interface::HEADER_BYTES + rows * network_interface::ENTRY_BYTES;
        object[start..start + bytes].copy_from_slice(&page[..bytes]);
        rows += read;
    }
    object[network_interface::OFF_HEADER_INTERFACE_COUNT
        ..network_interface::OFF_HEADER_INTERFACE_COUNT_END]
        .copy_from_slice(&(rows as u32).to_le_bytes());
    let total = network_interface::HEADER_BYTES + rows * network_interface::ENTRY_BYTES;
    object[network_interface::OFF_HEADER_TOTAL_LEN..network_interface::OFF_HEADER_TOTAL_LEN_END]
        .copy_from_slice(&(total as u32).to_le_bytes());
    let interfaces = NetworkInterfaces::decode(&object[..total])
        .unwrap_or_else(|_| fail(b"interface resource decode"));
    interfaces.for_holder(&network_interface::holder_identity("network-service"))
}

fn report_interface(declared: &DeclaredInterface) {
    debug_write(b"[network-service] interface addr=");
    write_ipv4(declared.address);
    debug_write(b"/");
    write_number(b"", u64::from(declared.prefix_len));
    debug_write(b" gateway=");
    match declared.gateway {
        Some(gateway) => write_ipv4(gateway),
        None => {
            debug_write(b"none");
        }
    }
    debug_write(b" mac=");
    write_mac(declared.mac);
    debug_write(b"\n");
}

fn write_ipv4(address: [u8; 4]) {
    for (index, octet) in address.iter().enumerate() {
        write_number(if index == 0 { b"" } else { b"." }, u64::from(*octet));
    }
}

fn write_mac(mac: [u8; 6]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (index, byte) in mac.iter().enumerate() {
        if index != 0 {
            debug_write(b":");
        }
        debug_write(&[HEX[usize::from(byte >> 4)], HEX[usize::from(byte & 0xf)]]);
    }
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
