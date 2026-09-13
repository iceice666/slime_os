#![no_std]
#![no_main]

//! IO4's exact-destination authority, and behind it, IO11's bounded IPv4
//! stack. The authority decides which holder may reach which exact tuple;
//! the stack moves bytes for the capabilities the authority minted, over the
//! `LinkDevice` the generation bound this service to, and nowhere else.

use boot_contracts::network_destination::{
    Address, Destination, ENTRY_BYTES, FORMAT_VERSION, HEADER_BYTES, MAGIC, MAX_DESTINATIONS,
    NetworkDestinations, OFF_HEADER_DESTINATION_COUNT, OFF_HEADER_FORMAT_VERSION,
    OFF_HEADER_HEADER_SIZE, OFF_HEADER_MAGIC, OFF_HEADER_REQUIRED_FLAGS, OFF_HEADER_TOTAL_LEN,
    RIGHT_CONNECT, RIGHT_LISTEN, RIGHT_RECV, RIGHT_SEND, Right, Transport,
};
use boot_contracts::network_interface::{self, Interface as DeclaredInterface, NetworkInterfaces};
use slime_components::tick_clock::TickClock;
use slime_proto::io_queue::{
    self, DIRECTION_DEVICE_READ, DIRECTION_DEVICE_WRITE, REQUEST_PAYLOAD_BYTES, WireBufferSlice,
};
use slime_proto::io_queue_ring::{Queue, QueueError};
use slime_proto::network_service::{
    self, DATA_QUEUE_SLOTS, SHUTDOWN_CAPABILITY, STATUS_DENIED, STATUS_MALFORMED,
    STATUS_RESET_BY_PEER, STATUS_UNREACHABLE, STATUS_UNSUPPORTED, WireLoanDelegation,
    WireNetworkCompletion, WireNetworkRequest,
};
use slime_proto::{valid_loan_delegation, valid_network_request};
use slime_rt::{
    ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, capability_import_from, debug_write,
    exit, monotonic_frequency, monotonic_read, network_destinations_read, network_interface_read,
    resolve_binding, shared_buffer_loan_map, yield_now,
};
use smoltcp::iface::{Config, Interface, PollResult, SocketHandle, SocketSet, SocketStorage};
use smoltcp::socket::tcp;
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpEndpoint, Ipv4Address};

mod link;
use link::Link;

slime_rt::entry!(main);

const MAX_ROWS: usize = MAX_DESTINATIONS;
const PAGE_ROWS: usize = 6;
/// The root copies up to every declared interface in one call, so the page
/// must hold the contract's maximum or a fuller table reads as none.
const INTERFACE_PAGE_ROWS: usize = network_interface::MAX_INTERFACES;
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
const SOCKET_BUFFER_BYTES: usize = 4096;
/// How long a socket may owe an answer before it is aborted: the handshake a
/// connect started, the settling of a close, and a blocking data request the
/// peer has neither served nor advanced. Only the connect uses the stack's own
/// silence bound, which starts with the SYN; the others are deadlines this
/// service keeps, because the stack counts its bound from the peer's last
/// packet whatever is owed, and arming it on a socket that has been idle
/// aborts at the next poll. An idle connection is never aborted for being idle.
const SOCKET_TIMEOUT_MS: i64 = 5000;

fn socket_timeout() -> Option<Duration> {
    Some(Duration::from_millis(SOCKET_TIMEOUT_MS as u64))
}

fn silence_bound() -> Duration {
    Duration::from_millis(SOCKET_TIMEOUT_MS as u64)
}
const FIRST_LOCAL_PORT: u16 = 49152;
const DATA_PAGES: usize = 2;
const PAGE: u64 = 4096;
/// Where a client's lent pages are mapped: above the link's own pages.
const DATA_BASE: u64 = 0x0000_0019_0000_0000 + 16 * PAGE;
/// The clients a generation may bind to this service, by the grant name each
/// resolves and the instance name its holder identity derives from. A grant
/// the generation does not declare simply resolves to no client.
const CLIENTS: [(&[u8], &str); 3] = [
    (b"network-probe-service", "io-network-probe"),
    (b"network-intruder-service", "io-network-intruder"),
    (b"network-tcp-probe-service", "io-tcp-probe"),
];

/// Sockets whose holders closed them, draining in the stack (TIME-WAIT for an
/// active close) until it lets them go. Their capabilities are gone, so they
/// charge no holder's ceiling; only the storage stays occupied.
/// A socket its holder let go of, finishing its close: the deadline is set
/// the first time it is reaped, since the release that lists it has no clock.
#[derive(Clone, Copy)]
struct Drain {
    handle: SocketHandle,
    deadline: Option<Instant>,
}

type Draining = [Option<Drain>; TCP_SOCKETS];

// Socket buffers live in the image's writable data rather than on the stack:
// four sockets' worth is twice the declared stack.
static mut RX_BUFFERS: [[u8; SOCKET_BUFFER_BYTES]; TCP_SOCKETS] =
    [[0; SOCKET_BUFFER_BYTES]; TCP_SOCKETS];
static mut TX_BUFFERS: [[u8; SOCKET_BUFFER_BYTES]; TCP_SOCKETS] =
    [[0; SOCKET_BUFFER_BYTES]; TCP_SOCKETS];
static mut SOCKET_STORAGE: [SocketStorage<'static>; TCP_SOCKETS] =
    [SocketStorage::EMPTY; TCP_SOCKETS];

/// Which socket buffer pair each live handle occupies; a handle's own index
/// is smoltcp's private business.
type SocketSlots = [Option<SocketHandle>; TCP_SOCKETS];

fn free_socket(slots: &mut SocketSlots, sockets: &mut SocketSet<'static>, handle: SocketHandle) {
    if let Some(slot) = slots.iter_mut().find(|slot| **slot == Some(handle)) {
        *slot = None;
    }
    sockets.remove(handle);
}

#[derive(Clone, Copy)]
struct DataPage {
    buffer: u64,
    lease: u64,
    base: u64,
}

impl DataPage {
    fn bytes(self) -> &'static mut [u8] {
        // Mapped at `base` for the life of this task from a loan the client
        // delegated; the pending table decides which bytes a request may use.
        unsafe { core::slice::from_raw_parts_mut(self.base as *mut u8, PAGE as usize) }
    }
}

/// One data request the socket could not finish at once.
#[derive(Clone, Copy)]
struct Pending {
    request_id: u64,
    op: u8,
    capability: u64,
    page: usize,
    offset: usize,
    length: usize,
    progress: usize,
    nonblocking: bool,
    /// When a blocking request the socket has not advanced aborts it.
    deadline: Instant,
}

/// A client's IO0 data queue and the pages it lent, once delegated.
struct DataQueue {
    queue: Queue<'static>,
    pages: [Option<DataPage>; DATA_PAGES],
    pending: [Option<Pending>; DATA_QUEUE_SLOTS],
}

struct Client {
    slot: u32,
    holder: [u8; 32],
    closed: bool,
    /// The next mapping base for a page this client lends.
    next_base: u64,
    queue_slot: Option<u32>,
    data: Option<DataQueue>,
    /// A connect whose reply waits for the handshake.
    pending_connect: Option<u64>,
}

#[derive(Clone, Copy)]
struct Capability {
    id: u64,
    holder: [u8; 32],
    destination: usize,
    rights: u16,
    kind: u8,
    epoch: u64,
    socket: Option<SocketHandle>,
}

#[derive(Default)]
struct Observed {
    requests: u32,
    packets: u32,
    socket_refusals: u32,
    listener_refusals: u32,
    dns_refusals: u32,
    cross_holder_refusals: u32,
    tcp_opened: u32,
    tcp_established: u32,
    tcp_reset: u32,
    bytes_sent: u64,
    bytes_received: u64,
}

/// Everything that exists only when the generation binds this service to a
/// link and declares its interface.
struct Stack {
    link: Link,
    iface: Interface,
    clock: TickClock,
    next_port: u16,
}

impl Stack {
    fn now(&self) -> Instant {
        let ticks = monotonic_read().unwrap_or_else(|_| fail(b"monotonic read"));
        Instant::from_millis(self.clock.millis(ticks))
    }
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

    let mut clients: [Option<Client>; CLIENTS.len()] = [const { None }; CLIENTS.len()];
    for (index, (entry, (grant, name))) in clients.iter_mut().zip(CLIENTS).enumerate() {
        if let Ok(slot) = resolve_binding(grant) {
            *entry = Some(Client {
                slot,
                holder: boot_contracts::network_destination::holder_identity(name),
                closed: false,
                next_base: DATA_BASE + (index as u64) * (1 + DATA_PAGES as u64) * PAGE,
                queue_slot: None,
                data: None,
                pending_connect: None,
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
    // The storage is static, as the buffers are, so the set may live for the
    // whole task without borrowing a local.
    let storage = unsafe { &mut *core::ptr::addr_of_mut!(SOCKET_STORAGE) };
    let mut sockets = SocketSet::new(&mut storage[..]);
    let mut socket_slots: SocketSlots = [None; TCP_SOCKETS];
    let mut draining: Draining = [None; TCP_SOCKETS];

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
                    let length = result as usize;
                    // A delegated loan arrives as its own record, told from a
                    // request by its magic; everything else is a request.
                    if let Some(delegation) = WireLoanDelegation::decode(&bytes[..length])
                        .filter(|delegation| delegation.magic == network_service::DELEGATION_MAGIC)
                    {
                        if let Err(reason) = accept_delegation(client, &delegation) {
                            refuse_client(
                                client,
                                reason,
                                &mut capabilities,
                                &mut sockets,
                                &mut draining,
                            );
                        }
                        continue;
                    }
                    observed.requests += 1;
                    let request = WireNetworkRequest::decode(&bytes[..length]);
                    let op = request.map_or(0, |value| value.op);
                    let outcome = match request {
                        Some(request)
                            if valid_network_request(&request)
                                && request.op == network_service::OP_CLOSE
                                && request.capability == SHUTDOWN_CAPABILITY =>
                        {
                            // The client exits once it has this reply, and the
                            // root then reclaims every page it lent; nothing
                            // below may touch that queue again.
                            close_client(client, &mut capabilities, &mut sockets, &mut draining);
                            Some((0, network_service::CAPABILITY_NONE, 0))
                        }
                        Some(request) if valid_network_request(&request) => dispatch(
                            &destinations,
                            client,
                            request,
                            &mut capabilities,
                            &mut next_capability,
                            epoch,
                            &mut observed,
                            stack.as_mut(),
                            &mut sockets,
                            &mut socket_slots,
                            &mut draining,
                        ),
                        _ => Some((STATUS_MALFORMED, network_service::CAPABILITY_NONE, 0)),
                    };
                    // `None` is a connect whose answer waits for the handshake.
                    if let Some((status, kind, capability)) = outcome {
                        send(
                            client.slot,
                            &completion(op, kind, status, 0, capability).encode(),
                        );
                    }
                }
            }
        }
        if let Some(stack) = stack.as_mut() {
            progress |= stack.link.drain();
            let now = stack.now();
            progress |= stack.iface.poll(now, &mut stack.link, &mut sockets)
                == PollResult::SocketStateChanged;
            progress |= stack.link.replenish();
            for client in clients.iter_mut().flatten().filter(|client| !client.closed) {
                progress |= settle_connect(
                    client,
                    &mut capabilities,
                    &mut sockets,
                    &mut socket_slots,
                    &mut observed,
                );
                match serve_data(
                    client,
                    &mut capabilities,
                    &destinations,
                    &mut sockets,
                    &mut observed,
                    now,
                ) {
                    Ok(served) => progress |= served,
                    Err(reason) => {
                        refuse_client(
                            client,
                            reason,
                            &mut capabilities,
                            &mut sockets,
                            &mut draining,
                        );
                        progress = true;
                    }
                }
            }
            progress |= reap_drained(&mut draining, &mut sockets, &mut socket_slots, now);
        }
        if !progress {
            yield_now();
        }
    }
    if let Some(mut stack) = stack {
        quiesce_sockets(&mut stack, &mut sockets, &mut socket_slots, &mut draining);
        release_stack(&mut stack, &observed);
    }
    report_observed(&observed);
    exit(0)
}

/// The loop above ends with its last client, whose close still owes the wire
/// an exchange: the FIN the stack has queued, the peer's answer, and this
/// side's final ACK. Poll until every draining socket has settled, which is
/// closed and reaped or in TIME-WAIT with nothing left to send, before the
/// link is reset from under those frames. A socket that has not settled
/// within the silence bound is aborted, as an unanswered close is in service,
/// and its reset leaves with the last poll.
fn quiesce_sockets(
    stack: &mut Stack,
    sockets: &mut SocketSet<'static>,
    socket_slots: &mut SocketSlots,
    draining: &mut Draining,
) {
    let count = draining.iter().flatten().count();
    let deadline = stack.now() + Duration::from_millis(SOCKET_TIMEOUT_MS as u64);
    let mut aborted = 0u32;
    loop {
        let mut progress = stack.link.drain();
        let now = stack.now();
        progress |=
            stack.iface.poll(now, &mut stack.link, sockets) == PollResult::SocketStateChanged;
        progress |= stack.link.replenish();
        progress |= reap_drained(draining, sockets, socket_slots, now);
        let unsettled = draining
            .iter()
            .flatten()
            .filter(|drain| {
                sockets.get::<tcp::Socket>(drain.handle).state() != tcp::State::TimeWait
            })
            .count();
        if unsettled == 0 {
            break;
        }
        if now >= deadline {
            for drain in draining.iter().flatten() {
                let socket = sockets.get_mut::<tcp::Socket>(drain.handle);
                if socket.state() != tcp::State::TimeWait {
                    socket.abort();
                    aborted += 1;
                }
            }
            let now = stack.now();
            stack.iface.poll(now, &mut stack.link, sockets);
            break;
        }
        if !progress {
            yield_now();
        }
    }
    write_number(b"[network-service] link quiesced sockets=", count as u64);
    write_number(b" aborted=", u64::from(aborted));
    debug_write(b"\n");
}

/// A client lends this service one queue page and then its data pages. The
/// descriptor names the buffer, the loan, and which of the two it is; the
/// authority behind it is claimed through the runtime from this client's
/// endpoint and no other sender, never trusted from the bytes. A descriptor
/// this client may not send, or a loan the root will not map, refuses the
/// client; it is never fatal to the service.
fn accept_delegation(
    client: &mut Client,
    delegation: &WireLoanDelegation,
) -> Result<(), &'static [u8]> {
    if !valid_loan_delegation(delegation) {
        return Err(b"delegation record");
    }
    let WireLoanDelegation {
        kind,
        buffer,
        lease,
        ..
    } = *delegation;
    // Every structural check first: nothing is claimed for a descriptor that
    // could not be installed.
    match kind {
        network_service::DELEGATION_QUEUE if client.data.is_none() => {}
        network_service::DELEGATION_QUEUE => return Err(b"second queue"),
        network_service::DELEGATION_DATA => match client.data.as_ref() {
            None => return Err(b"data page before queue"),
            Some(data) if data.pages.iter().all(|page| page.is_some()) => {
                return Err(b"too many data pages");
            }
            Some(_) => {}
        },
        _ => return Err(b"delegation kind"),
    }
    let Ok(slot) = capability_import_from(client.slot) else {
        return Err(b"no export from this client");
    };
    let base = client.next_base;
    if shared_buffer_loan_map(slot, base, 0, PAGE) != ERR_SUCCESS {
        return Err(b"loan map");
    }
    client.next_base += PAGE;
    match kind {
        network_service::DELEGATION_QUEUE => {
            let bytes = unsafe { core::slice::from_raw_parts_mut(base as *mut u8, PAGE as usize) };
            let Ok(queue) = Queue::attach(bytes, DATA_QUEUE_SLOTS) else {
                return Err(b"queue format");
            };
            client.queue_slot = Some(slot);
            client.data = Some(DataQueue {
                queue,
                pages: [None; DATA_PAGES],
                pending: [None; DATA_QUEUE_SLOTS],
            });
        }
        _ => {
            let data = client.data.as_mut().unwrap_or_else(|| unreachable!());
            let entry = data
                .pages
                .iter_mut()
                .find(|page| page.is_none())
                .unwrap_or_else(|| unreachable!());
            *entry = Some(DataPage {
                buffer,
                lease,
                base,
            });
        }
    }
    Ok(())
}

/// Close a client that stepped outside the protocol. Its queue and pages are
/// forgotten (their loans stay charged to it until the root settles them), its
/// endpoint is no longer received, its capabilities are released as its
/// shutdown would release them, and every other client is unaffected.
fn refuse_client(
    client: &mut Client,
    reason: &[u8],
    capabilities: &mut [Option<Capability>; MAX_CAPABILITIES],
    sockets: &mut SocketSet<'static>,
    draining: &mut Draining,
) {
    debug_write(b"[network-service] client refused reason=");
    debug_write(reason);
    debug_write(b"\n");
    close_client(client, capabilities, sockets, draining);
}

/// End a client, whether it asked to or was refused: it is never received or
/// served again, every request it still holds pending is answered as
/// cancelled and then its queue is forgotten, and everything it still holds
/// goes with it through the same path as `OP_CLOSE`, so an open socket's slot
/// returns once the peer answers or the bound expires rather than staying
/// with a holder nothing will ever settle.
fn close_client(
    client: &mut Client,
    capabilities: &mut [Option<Capability>; MAX_CAPABILITIES],
    sockets: &mut SocketSet<'static>,
    draining: &mut Draining,
) {
    client.closed = true;
    if let Some(data) = client.data.as_mut() {
        for slot in 0..DATA_QUEUE_SLOTS {
            let Some(pending) = data.pending[slot].take() else {
                continue;
            };
            // Admission reserved this request's completion slot, so a client
            // whose cursors are its own always hears the cancellation; a
            // refused client may have wedged its ring, and then it does not.
            let _ = complete_data(
                data,
                pending.request_id,
                pending.op,
                io_queue::STATUS_CANCELLED,
                STATUS_DENIED,
                0,
            );
        }
    }
    client.data = None;
    for index in 0..MAX_CAPABILITIES {
        if capabilities[index].is_some_and(|cap| cap.holder == client.holder) {
            release_capability(client, capabilities, index, sockets, draining);
        }
    }
    client.pending_connect = None;
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
    Some(Stack {
        link,
        iface,
        clock,
        next_port: FIRST_LOCAL_PORT,
    })
}

/// Hand the link back: reset the driver, acknowledge its settled completions,
/// and wait for its fresh epoch before this task can exit and have its loans
/// reclaimed from under the driver.
fn release_stack(stack: &mut Stack, observed: &Observed) {
    // A frame handed to the driver and not yet completed (the last ACK of a
    // close the client followed at once with its shutdown) would be counted
    // here and not yet in the driver's ledger.
    stack.link.settle_transmits();
    let tx = stack.link.tx.counts;
    let rx = stack.link.rx.counts;
    write_number(
        b"[network-service] link frames total=",
        u64::from(tx.frames + rx.frames),
    );
    write_number(b" tx=", u64::from(tx.frames));
    write_number(b" rx=", u64::from(rx.frames));
    write_number(b" arp=", u64::from(tx.arp + rx.arp));
    write_number(b" icmp=", u64::from(tx.icmp + rx.icmp));
    write_number(b" tcp=", u64::from(tx.tcp + rx.tcp));
    write_number(b" other=", u64::from(tx.other + rx.other));
    debug_write(b"\n");
    let (tx_frames, rx_frames) = stack.link.statistics();
    write_number(
        b"[network-service] link statistics tx=",
        u64::from(tx_frames),
    );
    write_number(b" rx=", u64::from(rx_frames));
    debug_write(b"\n");
    write_number(
        b"[network-service] tcp sockets opened=",
        u64::from(observed.tcp_opened),
    );
    write_number(b" established=", u64::from(observed.tcp_established));
    write_number(b" reset=", u64::from(observed.tcp_reset));
    write_number(b" bytes-tx=", observed.bytes_sent);
    write_number(b" bytes-rx=", observed.bytes_received);
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

fn completion(op: u8, kind: u8, status: i32, flags: u32, capability: u64) -> WireNetworkCompletion {
    WireNetworkCompletion {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        op,
        capability_kind: kind,
        status_detail: status,
        flags,
        capability,
    }
}

/// The authority decision for one endpoint request, and for a TCP connect on
/// a bound link, the socket behind it. `None` defers the reply.
#[allow(clippy::too_many_arguments)]
fn dispatch(
    destinations: &NetworkDestinations<'_>,
    client: &mut Client,
    request: WireNetworkRequest,
    capabilities: &mut [Option<Capability>; MAX_CAPABILITIES],
    next: &mut u64,
    epoch: u64,
    observed: &mut Observed,
    stack: Option<&mut Stack>,
    sockets: &mut SocketSet<'static>,
    socket_slots: &mut SocketSlots,
    draining: &mut Draining,
) -> Option<(i32, u8, u64)> {
    let holder = client.holder;
    let deny = Some((STATUS_DENIED, network_service::CAPABILITY_NONE, 0));
    if request.address_kind == network_service::ADDRESS_IPV6 {
        return Some((STATUS_UNSUPPORTED, network_service::CAPABILITY_NONE, 0));
    }
    match request.op {
        network_service::OP_RESOLVE => {
            let name = &request.endpoint[..request.name_len as usize];
            let Some((index, destination)) = find_resolve_destination(destinations, &holder, name)
            else {
                return deny;
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
                return deny;
            }
            Some(mint(
                capabilities,
                next,
                holder,
                index,
                destination.rights,
                network_service::CAPABILITY_DNS_RECORD,
                epoch,
                None,
            ))
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
                return deny;
            };
            let sockets_charged = capabilities
                .iter()
                .flatten()
                .filter(|cap| {
                    cap.holder == holder
                        && cap.destination == index
                        && cap.kind != network_service::CAPABILITY_DNS_RECORD
                })
                .count() as u32;
            if sockets_charged >= destination.socket_limit {
                observed.socket_refusals += 1;
                return deny;
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
                    return deny;
                }
                network_service::CAPABILITY_TCP_LISTENER
            } else if request.transport == network_service::TRANSPORT_TCP {
                network_service::CAPABILITY_TCP_CONNECTION
            } else {
                network_service::CAPABILITY_UDP_ENDPOINT
            };
            observed.packets += 1;
            // A TCP connect to an IPv4 destination on a bound link opens a
            // real socket; the authority answer above is what every other
            // combination still gets.
            let real = kind == network_service::CAPABILITY_TCP_CONNECTION
                && stack.is_some()
                && matches!(address, Address::Ipv4(_));
            if !real {
                return Some(mint(
                    capabilities,
                    next,
                    holder,
                    index,
                    destination.rights,
                    kind,
                    epoch,
                    None,
                ));
            }
            let stack = stack.unwrap();
            let Address::Ipv4(remote) = address else {
                unreachable!()
            };
            // One connect settles at a time: the reply for a second one would
            // overwrite the capability the first names, and the first socket
            // would never be settled or reclaimed. The endpoint's send is not
            // a call, so a client can reach here before its reply.
            if client.pending_connect.is_some() {
                return deny;
            }
            let Some(socket_index) = socket_slots.iter().position(|used| used.is_none()) else {
                observed.socket_refusals += 1;
                return deny;
            };
            let (rx, tx) = unsafe {
                (
                    &mut *core::ptr::addr_of_mut!(RX_BUFFERS[socket_index]),
                    &mut *core::ptr::addr_of_mut!(TX_BUFFERS[socket_index]),
                )
            };
            let mut socket = tcp::Socket::new(
                tcp::SocketBuffer::new(&mut rx[..]),
                tcp::SocketBuffer::new(&mut tx[..]),
            );
            socket.set_timeout(socket_timeout());
            let local_port = allocate_port(stack, socket_slots, sockets);
            let remote = IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::from(remote)), request.port);
            if socket
                .connect(stack.iface.context(), remote, local_port)
                .is_err()
            {
                return Some((STATUS_UNREACHABLE, network_service::CAPABILITY_NONE, 0));
            }
            let handle = sockets.add(socket);
            socket_slots[socket_index] = Some(handle);
            observed.tcp_opened += 1;
            let (status, minted_kind, id) = mint(
                capabilities,
                next,
                holder,
                index,
                destination.rights,
                kind,
                epoch,
                Some(handle),
            );
            if status != 0 {
                free_socket(socket_slots, sockets, handle);
                return Some((status, minted_kind, id));
            }
            client.pending_connect = Some(id);
            None
        }
        network_service::OP_SEND | network_service::OP_RECV | network_service::OP_CLOSE => {
            let Some(index) = capabilities
                .iter()
                .position(|entry| entry.is_some_and(|cap| cap.id == request.capability))
            else {
                return deny;
            };
            let cap = capabilities[index].unwrap();
            if cap.holder != holder {
                observed.cross_holder_refusals += 1;
                return deny;
            }
            if cap.epoch != epoch {
                return deny;
            }
            if cap.kind == network_service::CAPABILITY_TCP_LISTENER
                && matches!(
                    request.op,
                    network_service::OP_SEND | network_service::OP_RECV
                )
            {
                return Some((STATUS_UNSUPPORTED, network_service::CAPABILITY_NONE, 0));
            }
            let required = if request.op == network_service::OP_SEND {
                RIGHT_SEND
            } else if request.op == network_service::OP_RECV {
                RIGHT_RECV
            } else {
                0
            };
            if required != 0 && cap.rights & required == 0 {
                return deny;
            }
            if request.op == network_service::OP_CLOSE {
                release_capability(client, capabilities, index, sockets, draining);
            } else if cap.socket.is_some() {
                // The bytes travel in the client's data queue; the endpoint
                // carries no slice.
                return Some((STATUS_UNSUPPORTED, network_service::CAPABILITY_NONE, 0));
            } else {
                observed.packets += 1;
            }
            Some((0, network_service::CAPABILITY_NONE, 0))
        }
        network_service::OP_ACCEPT => {
            Some((STATUS_UNSUPPORTED, network_service::CAPABILITY_NONE, 0))
        }
        _ => Some((STATUS_MALFORMED, network_service::CAPABILITY_NONE, 0)),
    }
}

/// Drop one capability at once. Its socket, if any, finishes its own close
/// in the draining list, charging no ceiling: a connect still in flight is
/// aborted rather than closed, since nothing was ever established, and an
/// established socket is closed under the draining list's deadline, because a
/// close owes the peer's answer and an unanswered one must not hold its slot
/// forever.
fn release_capability(
    client: &mut Client,
    capabilities: &mut [Option<Capability>; MAX_CAPABILITIES],
    index: usize,
    sockets: &mut SocketSet<'static>,
    draining: &mut Draining,
) {
    let Some(cap) = capabilities[index].take() else {
        return;
    };
    let Some(handle) = cap.socket else {
        return;
    };
    let socket = sockets.get_mut::<tcp::Socket>(handle);
    if client.pending_connect == Some(cap.id) {
        client.pending_connect = None;
        socket.abort();
    } else {
        socket.close();
    }
    let Some(entry) = draining.iter_mut().find(|entry| entry.is_none()) else {
        // Every socket slot is already draining; the storage is bounded by
        // TCP_SOCKETS, so this cannot happen while the handle also holds a
        // slot.
        fail(b"draining list")
    };
    *entry = Some(Drain {
        handle,
        deadline: None,
    });
}

/// The next ephemeral port no socket in a slot is bound to, draining and
/// TIME-WAIT sockets included: a second socket on a live four-tuple could not
/// be told apart by the stack. The caller holds a free slot, so at most
/// `TCP_SOCKETS - 1` ports are taken and the walk always ends.
fn allocate_port(
    stack: &mut Stack,
    socket_slots: &SocketSlots,
    sockets: &SocketSet<'static>,
) -> u16 {
    for _ in 0..TCP_SOCKETS {
        let port = stack.next_port;
        stack.next_port = stack.next_port.checked_add(1).unwrap_or(FIRST_LOCAL_PORT);
        let taken = socket_slots.iter().flatten().any(|handle| {
            sockets
                .get::<tcp::Socket>(*handle)
                .local_endpoint()
                .is_some_and(|endpoint| endpoint.port == port)
        });
        if !taken {
            return port;
        }
    }
    fail(b"local port")
}

/// Answer a deferred connect once the handshake finished or failed.
fn settle_connect(
    client: &mut Client,
    capabilities: &mut [Option<Capability>; MAX_CAPABILITIES],
    sockets: &mut SocketSet<'static>,
    socket_slots: &mut SocketSlots,
    observed: &mut Observed,
) -> bool {
    let Some(id) = client.pending_connect else {
        return false;
    };
    let Some(index) = capabilities
        .iter()
        .position(|entry| entry.is_some_and(|cap| cap.id == id))
    else {
        client.pending_connect = None;
        return false;
    };
    let cap = capabilities[index].unwrap();
    let handle = cap
        .socket
        .unwrap_or_else(|| fail(b"pending connect socket"));
    let socket = sockets.get_mut::<tcp::Socket>(handle);
    if socket.may_send() {
        client.pending_connect = None;
        // Established and owing nothing: from here what the socket owes is
        // bounded by the deadline of each request `serve_data` holds pending.
        socket.set_timeout(None);
        observed.tcp_established += 1;
        send(
            client.slot,
            &completion(network_service::OP_CONNECT, cap.kind, 0, 0, cap.id).encode(),
        );
        return true;
    }
    if !socket.is_open() {
        client.pending_connect = None;
        observed.tcp_reset += 1;
        free_socket(socket_slots, sockets, handle);
        capabilities[index] = None;
        send(
            client.slot,
            &completion(
                network_service::OP_CONNECT,
                network_service::CAPABILITY_NONE,
                STATUS_UNREACHABLE,
                0,
                0,
            )
            .encode(),
        );
        return true;
    }
    false
}

/// Admit and service the client's data requests: each is validated against
/// the capability's holder and rights and the lent pages before a byte
/// touches a socket, then held pending until the socket takes or gives it.
/// `Err` is a client that wedged or corrupted its own ring; the caller
/// refuses it.
fn serve_data(
    client: &mut Client,
    capabilities: &mut [Option<Capability>; MAX_CAPABILITIES],
    destinations: &NetworkDestinations<'_>,
    sockets: &mut SocketSet<'static>,
    observed: &mut Observed,
    now: Instant,
) -> Result<bool, &'static [u8]> {
    let holder = client.holder;
    let Some(data) = client.data.as_mut() else {
        return Ok(false);
    };
    let mut progress = false;
    let mut body = [0u8; REQUEST_PAYLOAD_BYTES];
    // One ring's worth per pass: the client owns both of its cursors and may
    // move them while this runs, so the loop's exits below are not a bound on
    // their own. Every other client, and the stack, gets its turn regardless.
    for _ in 0..DATA_QUEUE_SLOTS {
        // Every taken request needs a completion slot, whether it is answered
        // now or held pending and answered later, so a request is taken only
        // while the ring has room for it beyond every answer not yet drained
        // and every request still held: a client cannot fill the ring with
        // refusals and leave a held request nowhere to settle.
        let held = data.pending.iter().flatten().count() as u64;
        if data.queue.completions_pending() + held >= DATA_QUEUE_SLOTS as u64 {
            break;
        }
        let submission = match data.queue.take_request(&mut body, PAGE) {
            Ok(value) => value,
            Err(error) if error.error == QueueError::Empty => break,
            Err(error) if error.error == QueueError::Inconsistent => {
                return Err(b"submission ring");
            }
            Err(error) => {
                if error.request_id != 0 {
                    data.queue
                        .complete(error.request_id, io_queue::STATUS_MALFORMED, 0, &[], false)
                        .map_err(|_| b"completion ring".as_slice())?;
                }
                progress = true;
                continue;
            }
        };
        progress = true;
        let request = WireNetworkRequest::decode(&body[..submission.payload_len]);
        let Some(request) = request.filter(valid_network_request) else {
            complete_data(
                data,
                submission.request_id,
                0,
                io_queue::STATUS_MALFORMED,
                STATUS_MALFORMED,
                0,
            )?;
            continue;
        };
        if !matches!(
            request.op,
            network_service::OP_SEND | network_service::OP_RECV
        ) {
            complete_data(
                data,
                submission.request_id,
                request.op,
                io_queue::STATUS_UNSUPPORTED,
                STATUS_UNSUPPORTED,
                0,
            )?;
            continue;
        }
        let capability = capabilities
            .iter()
            .flatten()
            .find(|cap| cap.id == request.capability)
            .copied();
        let Some(cap) = capability.filter(|cap| cap.holder == holder) else {
            if capability.is_some() {
                observed.cross_holder_refusals += 1;
            }
            complete_data(
                data,
                submission.request_id,
                request.op,
                io_queue::STATUS_BAD_RIGHTS,
                STATUS_DENIED,
                0,
            )?;
            continue;
        };
        let (required, direction) = if request.op == network_service::OP_SEND {
            (RIGHT_SEND, DIRECTION_DEVICE_READ)
        } else {
            (RIGHT_RECV, DIRECTION_DEVICE_WRITE)
        };
        if cap.rights & required == 0 || cap.socket.is_none() {
            complete_data(
                data,
                submission.request_id,
                request.op,
                io_queue::STATUS_BAD_RIGHTS,
                STATUS_DENIED,
                0,
            )?;
            continue;
        }
        let slice: WireBufferSlice = submission.slice;
        let page = data.pages.iter().position(|page| {
            page.is_some_and(|page| page.buffer == slice.buffer && page.lease == slice.lease)
        });
        let budget = destinations
            .destination(cap.destination)
            .map_or(0, |destination| destination.byte_budget as u64);
        let in_bounds = slice
            .offset
            .checked_add(slice.length)
            .is_some_and(|end| end <= PAGE)
            && slice.length > 0
            && slice.length <= budget;
        let Some(page) = page.filter(|_| in_bounds && slice.direction == direction) else {
            complete_data(
                data,
                submission.request_id,
                request.op,
                io_queue::STATUS_BAD_SLICE,
                STATUS_DENIED,
                0,
            )?;
            continue;
        };
        let depth = destinations
            .destination(cap.destination)
            .map_or(0, |destination| destination.queue_depth as usize);
        let held = data
            .pending
            .iter()
            .flatten()
            .filter(|pending| pending.capability == cap.id)
            .count();
        let Some(slot) = data
            .pending
            .iter()
            .position(|pending| pending.is_none())
            .filter(|_| held < depth)
        else {
            complete_data(
                data,
                submission.request_id,
                request.op,
                io_queue::STATUS_EXHAUSTED,
                STATUS_DENIED,
                0,
            )?;
            continue;
        };
        data.pending[slot] = Some(Pending {
            request_id: submission.request_id,
            op: request.op,
            capability: cap.id,
            page,
            offset: slice.offset as usize,
            length: slice.length as usize,
            progress: 0,
            nonblocking: request.flags & network_service::FLAG_NONBLOCKING != 0,
            deadline: now + silence_bound(),
        });
    }
    for slot in 0..DATA_QUEUE_SLOTS {
        let Some(pending) = data.pending[slot] else {
            continue;
        };
        let Some(cap) = capabilities
            .iter()
            .flatten()
            .find(|cap| cap.id == pending.capability)
            .copied()
        else {
            // The capability went away under the request: settled as
            // cancelled, and a refusal reports no transfer.
            complete_data(
                data,
                pending.request_id,
                pending.op,
                io_queue::STATUS_CANCELLED,
                STATUS_DENIED,
                0,
            )?;
            data.pending[slot] = None;
            progress = true;
            continue;
        };
        let handle = cap.socket.unwrap_or_else(|| fail(b"data socket"));
        let socket = sockets.get_mut::<tcp::Socket>(handle);
        if !pending.nonblocking && now >= pending.deadline {
            // The peer has neither served nor advanced this request within
            // the bound: the socket is aborted, and the request settles below
            // as the reset it now is, with every other request it holds.
            socket.abort();
        }
        let page = data.pages[pending.page]
            .unwrap_or_else(|| fail(b"data page"))
            .bytes();
        let region = &mut page[pending.offset..pending.offset + pending.length];
        if pending.op == network_service::OP_SEND {
            if !socket.may_send() {
                complete_data(
                    data,
                    pending.request_id,
                    pending.op,
                    io_queue::STATUS_DEVICE_ERROR,
                    STATUS_RESET_BY_PEER,
                    0,
                )?;
                data.pending[slot] = None;
                progress = true;
                continue;
            }
            let taken = socket.send_slice(&region[pending.progress..]).unwrap_or(0);
            let done = pending.progress + taken;
            if taken > 0 {
                progress = true;
                observed.bytes_sent += taken as u64;
            }
            if done == pending.length || pending.nonblocking {
                // A nonblocking send completes with whatever the socket took.
                complete_data(
                    data,
                    pending.request_id,
                    pending.op,
                    io_queue::STATUS_OK,
                    0,
                    done as u64,
                )?;
                data.pending[slot] = None;
                progress = true;
            } else if taken > 0 {
                // The socket took some: the peer is acknowledging, and the
                // bound starts over from here.
                data.pending[slot] = Some(Pending {
                    progress: done,
                    deadline: now + silence_bound(),
                    ..pending
                });
            }
        } else {
            // The stack tells a peer's FIN (nothing more will ever arrive) from
            // a reset or timed-out socket; the two are different answers.
            match socket.recv_slice(region) {
                Ok(0) if pending.nonblocking => {
                    complete_data(
                        data,
                        pending.request_id,
                        pending.op,
                        io_queue::STATUS_OK,
                        0,
                        0,
                    )?;
                    data.pending[slot] = None;
                    progress = true;
                }
                Ok(0) => {}
                Ok(received) => {
                    observed.bytes_received += received as u64;
                    complete_data(
                        data,
                        pending.request_id,
                        pending.op,
                        io_queue::STATUS_OK,
                        0,
                        received as u64,
                    )?;
                    data.pending[slot] = None;
                    progress = true;
                }
                Err(tcp::RecvError::Finished) => {
                    complete_end_of_stream(data, pending.request_id)?;
                    data.pending[slot] = None;
                    progress = true;
                }
                Err(tcp::RecvError::InvalidState) => {
                    complete_data(
                        data,
                        pending.request_id,
                        pending.op,
                        io_queue::STATUS_DEVICE_ERROR,
                        STATUS_RESET_BY_PEER,
                        0,
                    )?;
                    data.pending[slot] = None;
                    progress = true;
                }
            }
        }
    }
    Ok(progress)
}

/// Publish one terminal answer. Only a successful completion reports a
/// transfer; a refusal always reports zero, as the contract validator on the
/// client side requires. `Err` is a completion ring the client has wedged.
fn complete_data(
    data: &mut DataQueue,
    request_id: u64,
    op: u8,
    status: u32,
    detail: i32,
    transferred: u64,
) -> Result<(), &'static [u8]> {
    publish(
        data,
        request_id,
        status,
        transferred,
        completion(op, network_service::CAPABILITY_NONE, detail, 0, 0),
    )
}

/// The peer closed and every byte it sent has been delivered: a successful
/// receive of nothing, flagged as the end of the stream.
fn complete_end_of_stream(data: &mut DataQueue, request_id: u64) -> Result<(), &'static [u8]> {
    publish(
        data,
        request_id,
        io_queue::STATUS_OK,
        0,
        completion(
            network_service::OP_RECV,
            network_service::CAPABILITY_NONE,
            0,
            network_service::FLAG_END_OF_STREAM,
            0,
        ),
    )
}

fn publish(
    data: &mut DataQueue,
    request_id: u64,
    status: u32,
    transferred: u64,
    payload: WireNetworkCompletion,
) -> Result<(), &'static [u8]> {
    data.queue
        .complete(request_id, status, transferred, &payload.encode(), false)
        .map(|_| ())
        .map_err(|_| b"completion ring".as_slice())
}

/// Sockets their holders closed give their storage back once the stack has
/// let them go. One that has not closed within the silence bound of its
/// first reaping is aborted, as an unanswered close is in service, and its
/// storage returns on the next pass.
fn reap_drained(
    draining: &mut Draining,
    sockets: &mut SocketSet<'static>,
    socket_slots: &mut SocketSlots,
    now: Instant,
) -> bool {
    let mut progress = false;
    for entry in draining.iter_mut() {
        let Some(drain) = entry.as_mut() else {
            continue;
        };
        let handle = drain.handle;
        let deadline = *drain.deadline.get_or_insert(now + silence_bound());
        let socket = sockets.get_mut::<tcp::Socket>(handle);
        if socket.state() == tcp::State::Closed {
            free_socket(socket_slots, sockets, handle);
            *entry = None;
            progress = true;
        } else if now >= deadline {
            socket.abort();
            progress = true;
        }
    }
    progress
}

#[allow(clippy::too_many_arguments)]
fn mint(
    capabilities: &mut [Option<Capability>; MAX_CAPABILITIES],
    next: &mut u64,
    holder: [u8; 32],
    destination: usize,
    rights: u16,
    kind: u8,
    epoch: u64,
    socket: Option<SocketHandle>,
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
        socket,
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
