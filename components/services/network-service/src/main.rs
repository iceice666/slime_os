#![no_std]
#![no_main]

use boot_contracts::network_application::{self, Backend, NetworkApplications};
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
use smoltcp::iface::{Config, Interface, PollIngressSingleResult, PollResult, SocketStorage};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, Ipv4Address};

mod application;
mod link;
mod loopback;
mod tcp;
use link::Link;

slime_rt::entry!(main);

const MAX_ROWS: usize = MAX_DESTINATIONS;
// These bounded arenas are borrowed once by the sole service thread. Keeping
// their backing storage out of the call frame preserves the runtime stack bound.
static mut DESTINATION_STORAGE: [u8; HEADER_BYTES + MAX_ROWS * ENTRY_BYTES] =
    [0; HEADER_BYTES + MAX_ROWS * ENTRY_BYTES];
static mut EXTERNAL_RX: [[u8; tcp::BUFFER_BYTES]; tcp::SOCKETS] =
    [[0; tcp::BUFFER_BYTES]; tcp::SOCKETS];
static mut EXTERNAL_TX: [[u8; tcp::BUFFER_BYTES]; tcp::SOCKETS] =
    [[0; tcp::BUFFER_BYTES]; tcp::SOCKETS];
static mut LOCAL_RX: [[u8; tcp::BUFFER_BYTES]; tcp::SOCKETS] =
    [[0; tcp::BUFFER_BYTES]; tcp::SOCKETS];
static mut LOCAL_TX: [[u8; tcp::BUFFER_BYTES]; tcp::SOCKETS] =
    [[0; tcp::BUFFER_BYTES]; tcp::SOCKETS];
const PAGE_ROWS: usize = 6;
const INTERFACE_PAGE_ROWS: usize = 4;
/// The link peer endpoint and the buffer factory sit at fixed slots, as the
/// IO3 probe's do: a loan names its receiver by endpoint slot, and the root
/// reads that number as an endpoint only while no shared-buffer capability
/// occupies it, so both must sit below the first buffer this service creates.
const LINK_PEER_SLOT: u32 = 0;
const FACTORY_SLOT: u32 = 1;
const MAX_CAPABILITIES: usize = 8;
/// The clients a generation may bind to this service, by the grant name each
/// resolves and the instance name its holder identity derives from. A grant
/// the generation does not declare simply resolves to no client.
const LEGACY_CLIENTS: [(&[u8], &str); 2] = [
    (b"network-probe-service", "io-network-probe"),
    (b"network-intruder-service", "io-network-intruder"),
];
const MAX_CLIENTS: usize = network_application::MAX_APPLICATIONS + LEGACY_CLIENTS.len();
const SHUTDOWN_CAPABILITY: u64 = u64::MAX;
const STATUS_DENIED: i32 = -1;
const STATUS_MALFORMED: i32 = -2;
const STATUS_UNSUPPORTED: i32 = -3;

#[derive(Clone, Copy)]
struct Client {
    slot: u32,
    holder: [u8; 32],
    provision: Option<u32>,
    completion_signal: Option<u32>,
    supervision: Option<u32>,
    backend: Backend,
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
    active: bool,
}

impl Stack {
    fn now(&self) -> Instant {
        let ticks = monotonic_read().unwrap_or_else(|_| fail(b"monotonic read"));
        Instant::from_millis(self.clock.millis(ticks))
    }
}

struct LocalStack {
    device: loopback::Loopback,
    iface: Interface,
    clock: TickClock,
}

impl LocalStack {
    fn new() -> Self {
        let rate = monotonic_frequency().unwrap_or_else(|_| fail(b"loopback clock rate"));
        let base = monotonic_read().unwrap_or_else(|_| fail(b"loopback clock"));
        let clock = TickClock::new(rate, base).unwrap_or_else(|_| fail(b"loopback clock rate"));
        let mut device = loopback::Loopback::new();
        let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress([
            2, 0, 0, 0, 0, 1,
        ])));
        config.random_seed = base;
        let mut iface = Interface::new(config, &mut device, Instant::from_millis(0));
        iface.update_ip_addrs(|addresses| {
            addresses
                .push(IpCidr::new(
                    IpAddress::Ipv4(Ipv4Address::new(127, 0, 0, 1)),
                    8,
                ))
                .unwrap_or_else(|_| fail(b"loopback address"));
        });
        debug_write(b"[network-service] loopback interface=127.0.0.1 external_nic=none\n");
        Self {
            device,
            iface,
            clock,
        }
    }
    fn now(&self) -> Instant {
        Instant::from_millis(
            self.clock
                .millis(monotonic_read().unwrap_or_else(|_| fail(b"loopback clock"))),
        )
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
    // SAFETY: main runs once and no worker or callback accesses this arena.
    let object = unsafe { &mut *core::ptr::addr_of_mut!(DESTINATION_STORAGE) };
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

    let mut application_object = [0; network_application::MAX_BYTES];
    let application_table = read_applications(&mut application_object);
    let mut clients: [Option<Client>; MAX_CLIENTS] = [None; MAX_CLIENTS];
    for (entry, (grant, name)) in clients.iter_mut().zip(LEGACY_CLIENTS) {
        if let Ok(slot) = resolve_binding(grant) {
            *entry = Some(Client {
                slot,
                holder: boot_contracts::network_destination::holder_identity(name),
                provision: None,
                completion_signal: None,
                supervision: None,
                backend: Backend::External,
                closed: false,
            });
        }
    }
    if let Some(table) = application_table.as_ref() {
        for index in 0..table.application_count() {
            let declaration = table.application(index).unwrap();
            let slot = resolve_binding(declaration.control_binding)
                .unwrap_or_else(|_| fail(b"application control binding"));
            let provision = resolve_binding(declaration.provision_binding)
                .unwrap_or_else(|_| fail(b"application provision binding"));
            if slot == provision
                || clients.iter().flatten().any(|client| {
                    client.slot == slot || client.holder == declaration.holder_identity
                })
            {
                fail(b"application binding alias");
            }
            clients[LEGACY_CLIENTS.len() + index] = Some(Client {
                slot,
                holder: declaration.holder_identity,
                provision: Some(provision),
                completion_signal: (!declaration.completion_notification.is_empty())
                    .then(|| notification_binding(declaration.completion_notification, b"+signal")),
                supervision: (!declaration.supervision_binding.is_empty())
                    .then(|| minted_binding(declaration.supervision_binding)),
                backend: declaration.backend,
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
    let mut socket_storage: [SocketStorage; tcp::SOCKETS] = [SocketStorage::EMPTY; tcp::SOCKETS];
    // SAFETY: each engine exclusively borrows its own arena for this service lifetime.
    let rx_storage = unsafe { &mut *core::ptr::addr_of_mut!(EXTERNAL_RX) };
    let tx_storage = unsafe { &mut *core::ptr::addr_of_mut!(EXTERNAL_TX) };
    let incarnation = if application_table.is_some() {
        allocate_incarnation()
    } else {
        1
    };
    let mut engine = tcp::Engine::new(&mut socket_storage, rx_storage, tx_storage, incarnation, 0)
        .unwrap_or_else(|_| fail(b"external engine incarnation"));
    let mut local_storage: [SocketStorage; tcp::SOCKETS] = [SocketStorage::EMPTY; tcp::SOCKETS];
    // SAFETY: disjoint from the external engine and borrowed only by this main.
    let local_rx = unsafe { &mut *core::ptr::addr_of_mut!(LOCAL_RX) };
    let local_tx = unsafe { &mut *core::ptr::addr_of_mut!(LOCAL_TX) };
    let mut local_engine = tcp::Engine::new(&mut local_storage, local_rx, local_tx, incarnation, 1)
        .unwrap_or_else(|_| fail(b"local engine incarnation"));
    let mut local = application_table
        .as_ref()
        .filter(|table| {
            (0..table.application_count())
                .any(|index| table.application(index).unwrap().backend == Backend::Loopback)
        })
        .map(|_| LocalStack::new());
    let mut applications: [Option<application::Application>; MAX_CLIENTS] =
        core::array::from_fn(|_| None);
    let mut waiting = service_wait_set(application_table.as_ref(), &clients);
    let wait_rate = waiting
        .as_ref()
        .map(|_| monotonic_frequency().unwrap_or_else(|_| fail(b"wait clock rate")));
    let mut coalesced = 0u64;

    while clients.iter().flatten().any(|client| !client.closed) {
        let mut progress = false;
        for (index, client) in clients.iter_mut().enumerate() {
            let Some(client) = client else { continue };
            if client.closed {
                continue;
            }
            if let Some(slot) = client.supervision {
                match slime_rt::supervision_status(slot) {
                    Ok(Some(_)) => {
                        client.supervision = None;
                        let reclaimed = match client.backend {
                            Backend::External => engine.release_holder(client.holder),
                            Backend::Loopback => local_engine.release_holder(client.holder),
                        };
                        let session_released = if let Some(application) = applications[index].take()
                        {
                            if !application.release() {
                                fail(b"dead client buffers");
                            }
                            true
                        } else {
                            false
                        };
                        write_number(
                            b"[network-service] client death handles=",
                            reclaimed.handles as u64,
                        );
                        write_number(b" sockets=", reclaimed.sockets as u64);
                        write_number(b" bytes=", reclaimed.bytes as u64);
                        write_number(b" sessions_released=", u64::from(session_released));
                        debug_write(b"\n");
                        client.closed = true;
                        progress = true;
                        continue;
                    }
                    Ok(None) => {}
                    Err(_) => fail(b"client supervision"),
                }
            }
            if let Some(application) = applications[index].as_mut() {
                progress |= match client.backend {
                    Backend::External => {
                        let stack = stack
                            .as_mut()
                            .unwrap_or_else(|| fail(b"application without stack"));
                        let now = stack.now();
                        application.drive(
                            client.holder,
                            &destinations,
                            None,
                            &mut engine,
                            &mut stack.iface,
                            now,
                        )
                    }
                    Backend::Loopback => {
                        let stack = local
                            .as_mut()
                            .unwrap_or_else(|| fail(b"application without loopback"));
                        let now = stack.now();
                        application.drive(
                            client.holder,
                            &destinations,
                            application_table.as_ref(),
                            &mut local_engine,
                            &mut stack.iface,
                            now,
                        )
                    }
                };
            }
            if applications[index]
                .as_ref()
                .is_some_and(application::Application::failed)
            {
                engine.release_holder(client.holder);
                local_engine.release_holder(client.holder);
                if let Some(application) = applications[index].take() {
                    application.release();
                }
                client.closed = true;
                debug_write(b"[network-service] application session invalidated\n");
                continue;
            }
            let mut bytes = [0u8; MAX_MSG];
            let mut caps = [0u64; MAX_CAPS_PER_MSG];
            match slime_rt::recv(client.slot, &mut bytes, &mut caps) {
                ERR_WOULDBLOCK => {}
                result if result < 0 => fail(b"client receive"),
                result => {
                    progress = true;
                    if result as usize == network_service::REQUEST_BYTES
                        && WireNetworkRequest::decode(&bytes[..result as usize])
                            .is_some_and(|request| request.op == network_service::OP_ATTACH)
                    {
                        let request =
                            WireNetworkRequest::decode(&bytes[..result as usize]).unwrap();
                        let valid = request.magic == network_service::NETWORK_MAGIC
                            && request.version == network_service::FORMAT_VERSION
                            && request.transport == network_service::TRANSPORT_NONE
                            && request.flags == 0
                            && request.port == 0
                            && request.name_len == 0
                            && request.capability == 0
                            && request.address_kind == network_service::ADDRESS_NONE
                            && request.reserved == [0; 7]
                            && request.endpoint == [0; 24];
                        let status = if !valid {
                            network_service::STATUS_MALFORMED
                        } else if client.provision.is_none()
                            || applications[index].is_some()
                            || (client.backend == Backend::External
                                && !stack.as_ref().is_some_and(|stack| stack.active))
                        {
                            network_service::STATUS_DENIED
                        } else {
                            match application::Application::attach(
                                index,
                                FACTORY_SLOT,
                                client.provision.unwrap(),
                                incarnation,
                                client.completion_signal,
                            ) {
                                Ok(application) => {
                                    applications[index] = Some(application);
                                    network_service::STATUS_SUCCESS
                                }
                                Err(()) => network_service::STATUS_EXHAUSTED,
                            }
                        };
                        let reply = WireNetworkCompletion {
                            magic: network_service::NETWORK_MAGIC,
                            version: network_service::FORMAT_VERSION,
                            op: network_service::OP_ATTACH,
                            capability_kind: network_service::CAPABILITY_NONE,
                            status_detail: status,
                            flags: 0,
                            capability: 0,
                        };
                        send(client.slot, &reply.encode());
                        continue;
                    }
                    observed.requests += 1;
                    let request = WireNetworkRequest::decode(&bytes[..result as usize]);
                    let op = request.map_or(0, |value| value.op);
                    let (status, kind, capability) = match request {
                        Some(request)
                            if valid_network_request(&request)
                                && request.op == network_service::OP_CLOSE
                                && request.capability == SHUTDOWN_CAPABILITY
                                && applications[index]
                                    .as_ref()
                                    .is_some_and(|application| !application.quiescent()) =>
                        {
                            (
                                network_service::STATUS_WOULD_BLOCK,
                                network_service::CAPABILITY_NONE,
                                0,
                            )
                        }
                        Some(request)
                            if valid_network_request(&request)
                                && request.op == network_service::OP_CLOSE
                                && request.capability == SHUTDOWN_CAPABILITY =>
                        {
                            if let Some(application) = applications[index].take() {
                                engine.release_holder(client.holder);
                                local_engine.release_holder(client.holder);
                                write_number(
                                    b"[network-service] application bytes sent=",
                                    application.sent,
                                );
                                write_number(b" received=", application.received);
                                debug_write(b"\n");
                                if !application.release() {
                                    fail(b"application buffer release");
                                }
                                debug_write(b"[network-service] application buffers released\n");
                            }
                            client.closed = true;
                            (0, network_service::CAPABILITY_NONE, 0)
                        }
                        Some(_) if client.provision.is_some() => {
                            (STATUS_UNSUPPORTED, network_service::CAPABILITY_NONE, 0)
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
        if let Some(stack) = stack.as_mut().filter(|stack| stack.active) {
            progress |= stack.link.drain();
            let now = stack.now();
            engine.tick(now);
            progress |= stack.iface.poll(now, &mut stack.link, engine.sockets())
                == PollResult::SocketStateChanged;
            progress |= stack.link.flush(&mut engine);
            progress |= stack.link.replenish();
        }
        if let Some(stack) = local.as_mut() {
            let now = stack.now();
            local_engine.tick(now);
            stack.iface.poll_maintenance(now);
            for _ in 0..loopback::FRAME_SLOTS {
                if stack
                    .iface
                    .poll_ingress_single(now, &mut stack.device, local_engine.sockets())
                    == PollIngressSingleResult::None
                {
                    break;
                }
                progress = true;
            }
            progress |= stack
                .iface
                .poll_egress(now, &mut stack.device, local_engine.sockets())
                == PollResult::SocketStateChanged;
            progress |= stack
                .device
                .flush(|frame| local_engine.permit_egress(frame));
            progress |= stack.device.ready_count() != 0 || stack.device.staged_count() != 0;
        }
        if option_env!("SLIME_NETWORK_DRIVER_RESET_IN_FLIGHT") == Some("1")
            && incarnation == 1
            && stack.as_ref().is_some_and(|stack| stack.active)
            && engine.transmit_drained()
            && applications
                .iter()
                .flatten()
                .any(|application| application.in_flight_payload() && application.sent >= 1024)
        {
            let stack = stack.as_mut().unwrap();
            stack.link.release();
            stack.active = false;
            let reclaimed = engine.reset_all(stack.now());
            let settled: usize = applications
                .iter_mut()
                .flatten()
                .map(application::Application::reset_requests)
                .sum();
            write_number(b"[network-service] driver reset requests=", settled as u64);
            write_number(b" handles=", reclaimed.handles as u64);
            write_number(b" sockets=", reclaimed.sockets as u64);
            write_number(b" bytes=", reclaimed.bytes as u64);
            debug_write(b"\n");
        }
        if option_env!("SLIME_NETWORK_FAULT_IN_FLIGHT") == Some("1")
            && incarnation == 1
            && applications
                .iter()
                .flatten()
                .map(|application| application.received)
                .sum::<u64>()
                >= 1024
            && applications
                .iter()
                .flatten()
                .any(application::Application::in_flight_payload)
        {
            debug_write(b"[network-service] injected in-flight fault incarnation=1\n");
            inject_fault();
        }
        if !progress {
            if let Some(wait) = waiting.as_mut() {
                // A short control deadline also covers native attach/finish rendezvous,
                // which are not themselves notification events.
                let mut delay_ms = 10u64;
                if let Some(stack) = stack.as_mut()
                    && let Some(delay) = stack.iface.poll_delay(stack.now(), engine.sockets())
                {
                    delay_ms = delay_ms.min(delay.total_millis().max(1));
                }
                if let Some(stack) = local.as_mut()
                    && let Some(delay) = stack.iface.poll_delay(stack.now(), local_engine.sockets())
                {
                    delay_ms = delay_ms.min(delay.total_millis().max(1));
                }
                let rate = wait_rate.unwrap();
                let ticks = (rate / 1000).saturating_mul(delay_ms).max(1);
                let armed_at = monotonic_read().unwrap_or_else(|_| fail(b"network timer clock"));
                let deadline = armed_at
                    .checked_add(ticks)
                    .unwrap_or_else(|| fail(b"network timer overflow"));
                let timer = slime_rt::timer_arm(ticks).unwrap_or_else(|_| fail(b"network timer"));
                let ready = wait.wait().unwrap_or_else(|_| fail(b"network wait"));
                coalesced += u64::from(ready > 1);
                while wait.next_ready().is_some() {}
                let cancelled = slime_rt::timer_cancel(timer);
                if cancelled != ERR_SUCCESS
                    && !(cancelled == slime_rt::ERR_BAD_CAP
                        && monotonic_read().unwrap_or_else(|_| fail(b"network timer clock"))
                            >= deadline)
                {
                    fail(b"network timer cancellation");
                }
            } else {
                yield_now();
            }
        }
    }
    let cleanup_time = local.as_ref().map_or_else(
        || stack.as_ref().map_or(Instant::from_millis(0), Stack::now),
        LocalStack::now,
    );
    let _ = engine.reset_all(cleanup_time);
    let _ = local_engine.reset_all(cleanup_time);
    if let Some(wait) = waiting {
        write_number(b"[network-service] wait wakes=", wait.wakes() as u64);
        write_number(b" coalesced=", coalesced);
        debug_write(b"\n");
    }
    let external_frames = stack
        .as_ref()
        .map_or(0, |stack| u64::from(stack.link.tx.counts.frames));
    if let Some(stack) = local {
        write_number(
            b"[network-service] loopback frames=",
            stack.device.egress_count(),
        );
        write_number(b" rejected=", stack.device.rejected_count());
        write_number(b" handles=", local_engine.allocated() as u64);
        write_number(b" external_frames=", external_frames);
        write_number(b" resets=", stack.device.reset_count());
        write_number(b" syns=", stack.device.syn_count());
        write_number(b" fins=", stack.device.fin_count());
        debug_write(b"\n");
    }
    if let Some(mut stack) = stack {
        write_number(
            b"[network-service] application connection handles live=",
            engine.allocated() as u64,
        );
        debug_write(b"\n");
        if stack.active {
            release_stack(&mut stack);
        }
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
    Some(Stack {
        link,
        iface,
        clock,
        active: true,
    })
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

fn service_wait_set(
    table: Option<&NetworkApplications<'_>>,
    clients: &[Option<Client>; MAX_CLIENTS],
) -> Option<slime_rt::wait_set::WaitSet> {
    let table = table?;
    let mut name = None;
    for index in 0..table.application_count() {
        let entry = table.application(index).unwrap();
        if entry.request_notification.is_empty() {
            continue;
        }
        if name.is_some_and(|name| name != entry.request_notification) {
            fail(b"network wait fan-in");
        }
        name = Some(entry.request_notification);
    }
    let slot = notification_binding(name?, b"+wait");
    let mut wait = slime_rt::wait_set::WaitSet::declared(slot)
        .unwrap_or_else(|_| fail(b"network wait declaration"));
    for client in clients
        .iter()
        .flatten()
        .filter(|client| client.completion_signal.is_some())
    {
        wait.register_slot(slime_rt::wait_set::Kind::Stream, client.slot)
            .unwrap_or_else(|_| fail(b"network wait source"));
    }
    wait.register_timer()
        .unwrap_or_else(|_| fail(b"network wait timer"));
    Some(wait)
}

fn allocate_incarnation() -> u64 {
    let key = network_application::INCARNATION_PARAMETER_KEY;
    let previous = match slime_rt::lifecycle_parameter_read(slime_rt::PARAMETER_SELF_SLOT, key) {
        Ok(value) => value,
        Err(slime_rt::ERR_INVALID_ARG) => 0,
        Err(_) => fail(b"incarnation authority"),
    };
    let next = previous
        .checked_add(1)
        .filter(|next| *next <= tcp::MAX_INCARNATION)
        .unwrap_or_else(|| fail(b"incarnation exhausted"));
    let observed = slime_rt::lifecycle_parameter_write(slime_rt::PARAMETER_SELF_SLOT, key, next)
        .unwrap_or_else(|_| fail(b"incarnation write"));
    if observed != previous {
        fail(b"incarnation concurrent writer");
    }
    write_number(b"[network-service] incarnation=", next);
    debug_write(b"\n");
    next
}

fn minted_binding(name: &[u8]) -> u32 {
    let mut query = [0; 64];
    let prefix = b"minted:";
    let length = prefix.len() + name.len();
    query[..prefix.len()].copy_from_slice(prefix);
    query[prefix.len()..length].copy_from_slice(name);
    resolve_binding(&query[..length]).unwrap_or_else(|_| fail(b"supervision binding"))
}

fn notification_binding(name: &[u8], role: &[u8]) -> u32 {
    let mut query = [0; 64];
    let prefix = b"notification:";
    let length = prefix.len() + name.len() + role.len();
    if length > query.len() {
        fail(b"notification binding length");
    }
    query[..prefix.len()].copy_from_slice(prefix);
    query[prefix.len()..prefix.len() + name.len()].copy_from_slice(name);
    query[prefix.len() + name.len()..length].copy_from_slice(role);
    resolve_binding(&query[..length]).unwrap_or_else(|_| fail(b"notification binding"))
}

fn read_applications(
    object: &mut [u8; network_application::MAX_BYTES],
) -> Option<NetworkApplications<'_>> {
    use network_application as c;
    object[c::OFF_HEADER_MAGIC..c::OFF_HEADER_MAGIC_END].copy_from_slice(&c::MAGIC);
    object[c::OFF_HEADER_FORMAT_VERSION..c::OFF_HEADER_FORMAT_VERSION_END]
        .copy_from_slice(&c::FORMAT_VERSION.to_le_bytes());
    object[c::OFF_HEADER_HEADER_SIZE..c::OFF_HEADER_HEADER_SIZE_END]
        .copy_from_slice(&(c::HEADER_BYTES as u32).to_le_bytes());
    let mut count = 0;
    loop {
        let mut page = [0; 3 * c::ENTRY_BYTES];
        let read = match slime_rt::network_application_read(count, &mut page) {
            Ok(read) => read,
            Err(_) if count == 0 => return None,
            Err(_) => fail(b"application resource read"),
        };
        if read == 0 {
            break;
        }
        if read > 3 || count + read > c::MAX_APPLICATIONS {
            fail(b"application resource bounds");
        }
        let start = c::HEADER_BYTES + count * c::ENTRY_BYTES;
        object[start..start + read * c::ENTRY_BYTES]
            .copy_from_slice(&page[..read * c::ENTRY_BYTES]);
        count += read;
    }
    let total = c::HEADER_BYTES + count * c::ENTRY_BYTES;
    object[c::OFF_HEADER_APPLICATION_COUNT..c::OFF_HEADER_APPLICATION_COUNT_END]
        .copy_from_slice(&(count as u32).to_le_bytes());
    object[c::OFF_HEADER_TOTAL_LEN..c::OFF_HEADER_TOTAL_LEN_END]
        .copy_from_slice(&(total as u32).to_le_bytes());
    Some(
        NetworkApplications::decode(&object[..total])
            .unwrap_or_else(|_| fail(b"application resource decode")),
    )
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

fn inject_fault() -> ! {
    // This diagnostic path is reachable only in a closure-selected profile.
    // The instruction faults in the component VSpace without a Rust dereference.
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("str xzr, [{address}]", address = in(reg) 0usize, options(nostack));
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("mov qword ptr [{address}], 0", address = in(reg) 0usize, options(nostack));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("sd zero, 0({address})", address = in(reg) 0usize, options(nostack));
    }
    fail(b"fault injection returned")
}

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[network-service] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
