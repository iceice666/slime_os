#![no_std]
#![no_main]

//! C8.3/C8.4 fabric: attenuated endpoint provisioning and bounded many-to-many
//! stream brokering.
//!
//! A userspace service that owns every route endpoint in the generation's data
//! fabric, hands each participant exactly one non-transferable role capability,
//! and then brokers the samples those roles carry. The kernel supplies two
//! generic mechanisms — `SYS_CAP_TRANSFER`, a bounded narrow-on-transfer move,
//! and the C7 shared-buffer loan lifecycle — and knows nothing of routes,
//! schemas, graph roles, QoS, or matching; all of that policy lives here.
//!
//! **Provisioning (C8.3).** Three properties this service exists to make true:
//!
//! 1. **A role is one direction.** A publisher's endpoint carries `RIGHT_SEND`
//!    and nothing else; a subscriber's carries `RIGHT_RECV`. The two halves of
//!    a route are separate kernel endpoints, so a publisher cannot receive on
//!    its route even by misusing the capability it holds.
//! 2. **A provisioned endpoint is terminal.** Every move omits
//!    `RIGHT_TRANSFER`, so a participant cannot re-delegate its role or mint a
//!    downstream edge. Non-delegability is enforced by the kernel at the moment
//!    of transfer, not by convention afterwards.
//! 3. **Names grant nothing.** A client is authenticated by the
//!    generation-provisioned control endpoint its request arrived on — the
//!    binding init established at spawn — never by the route name, direction,
//!    or type identity the request carries. Those fields are read and ignored:
//!    the answer comes from the graph table, keyed by the caller's identity.
//!
//! **Brokering (C8.4).** Matching is the exact tuple the graph declares: a
//! publisher and a subscriber exchange data only when they name one route, and
//! a route is (name, full interface identity, contract kind). Two participants
//! on different routes never see each other's samples even though one service
//! moves both, because a sample is dispatched by the route index its ingress
//! endpoint belongs to — never by anything the sample itself claims.
//!
//! A sample travels one of two ways, decided by size alone:
//!
//! - **Inline.** A payload within `MAX_INLINE_BYTES` rides in the fixed
//!   `StreamSample` control message, one kernel message per subscriber.
//! - **Shared.** A payload larger than the control-message bound arrives as a
//!   C7.6 descriptor naming a receiver-bound loan. The fabric maps that loan
//!   read-only, copies the bytes **once** into a fabric-owned sealed buffer,
//!   and then creates one independently accounted downstream loan per matched
//!   subscriber. One publisher sample is therefore one copy and N loans, never
//!   N copies, and the upstream loan is returned as soon as the copy lands.
//!
//! Delivery is bounded per subscriber by its declared KEEP_LAST depth. A
//! subscriber releases a delivery slot with a `StreamAck`; until it does, the
//! fabric holds at most `history_depth` samples for it and evicts the oldest to

//! admit a newer one. Eviction is counted, and one stall produces exactly one
//! `SAMPLE_LOST` event when delivery resumes — never a growing queue and never
//! a retry.

#[path = "../call_broker.rs"]
mod call_broker;

#[path = "../operation_broker.rs"]
mod operation_broker;
#[path = "../visibility_broker.rs"]
mod visibility_broker;

use boot_contracts::fabric_graph::{
    CONTRACT_KIND_STREAM, DIRECTION_PUBLISH, DIRECTION_SUBSCRIBE, DURABILITY_RETAINED,
    RELIABILITY_RELIABLE, TransportQos, route_identity,
};
use boot_contracts::stream_history::{HistoryEntry, StreamHistory};
use slime_proto::capability_transfer::{
    CAPABILITY_TRANSFER_MAGIC, FORMAT_VERSION, OBJECT_KIND_ENDPOINT, REQUEST_LEN, TRANSFER_LEN,
    WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::fabric_qos::{
    EVENT_DEADLINE_MISSED, EVENT_INCOMPATIBLE_QOS, EVENT_LIFESPAN_EXPIRED, EVENT_LIVELINESS_LOST,
    EVENT_MATCHED, EVENT_RETRY_EXHAUSTED, EVENT_UNMATCHED, FORMAT_VERSION as QOS_FORMAT_VERSION,
    QOS_EVENT_MAGIC, WireQosEvent,
};
use slime_proto::fabric_stream::{
    EVENT_SAMPLE_LOST, EVENT_SAMPLE_TAKEN, EVENT_STREAM_END, FLAG_LAST, MAX_INLINE_BYTES,
    STREAM_EVENT_MAGIC, STREAM_SAMPLE_MAGIC, WireStreamAck, WireStreamEvent, WireStreamSample,
};
use slime_proto::fabric_time::WireTimeAdvance;
use slime_proto::interface_schema::{diagnostics_stream, telemetry_stream};
use slime_proto::sample_descriptor::{
    CAPABILITY_KIND_LOAN, SAMPLE_DESCRIPTOR_MAGIC, WireSampleDescriptor,
};
use slime_proto::{valid_fabric_request, valid_sample_descriptor, valid_stream_ack};
use slime_rt::{
    ERR_OUT_OF_MEMORY, ERR_PEER_DEAD, ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG,
    Rights, WaitSource,
};

slime_rt::entry!(main);

include!(concat!(env!("OUT_DIR"), "/fabric_profile.rs"));

/// `EndpointFactory`, granted by the generation. The fabric mints both halves
/// of every route through it; no participant holds one.
const FACTORY_SLOT: u32 = 0;
/// `SharedBufferFactory`, granted by the generation. Backs the one fabric-owned
/// copy each large sample makes; the fabric's own `shared-buffer-budget` entry
/// bounds it, so brokering can never outgrow a declared quota.
const BUFFER_FACTORY_SLOT: u32 = 1;
const TIME_SLOT: u32 = 9;
/// Control endpoints, one per client, in the order init granted them. The slot
/// a request arrives on *is* the caller's identity: init bound each to exactly
/// one component at spawn, and no component can forge or re-derive one.
const FIRST_CONTROL_SLOT: u32 = FABRIC_FIRST_CONTROL_SLOT;

const RIGHT_SEND: Rights = 1;
const RIGHT_RECV: Rights = 2;

/// The routes this generation declares. Folded at runtime with the generated
/// C8.1 interface identities so a route identity cannot drift from the admitted
/// schema. Index into this table *is* the route identity for dispatch: a sample
/// is routed by the ingress it arrived on, never by anything it claims.
const ROUTE_NAMES: [&str; 2] = ["telemetry", "diagnostics"];
const ROUTE_COUNT: usize = ROUTE_NAMES.len();

/// Provisioning denial. Distinct from a malformed request so the transcript
/// shows *why* an edge was refused.
const STATUS_NOT_GRANTED: i32 = -1;
const STATUS_BAD_REQUEST: i32 = -2;

/// Fixed brokering capacity. Every table below is sized from the generation's
/// own declared ceilings, so nothing here grows with traffic.
const MAX_PARTICIPANTS: usize = FABRIC_MAX_PUBLISHERS + FABRIC_MAX_SUBSCRIBERS;
/// Fabric-owned sample frames. Each holds one inline payload or names one
/// fabric-owned buffer; a frame is freed when its last reference is delivered
/// or evicted.
///
/// Sized to the graph's `historyDepth` ceiling times the subscriber ceiling it
/// can face at once. A frame is referenced by every subscriber ring holding it,
/// so a table smaller than the summed declared depths would let the rings fill
/// while no frame is free — and with the stalled subscriber holding its ring
/// and the publishers blocked, nothing would ever wake the fabric again. That
/// is a deadlock, not backpressure, so the table is sized to make it
/// unreachable rather than detected.
const MAX_FRAMES: usize = FABRIC_FRAME_CAPACITY;
/// Pages in one fabric-owned copy buffer. Bounds the largest brokered sample at
/// two pages, matching the C7 sample plane's payload and the fabric's declared
/// `bytePages` quota.
const COPY_PAGES: usize = FABRIC_COPY_PAGES;
const PAGE: u64 = 4096;
/// Scratch window where the fabric maps an upstream loan and its own copy
/// buffer. Two disjoint ranges, both unmapped before the next sample.
const UPSTREAM_BASE: u64 = 0x0000_000B_0000_0000;
const COPY_BASE: u64 = 0x0000_000C_0000_0000;

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn qos_check() -> bool {
    option_env!("SLIME_FABRIC_QOS_CHECK") == Some("1")
}

/// One client's control binding: the slot init gave the fabric for it, and the
/// component identity that slot authenticates.
struct Client {
    control_slot: u32,
    component: &'static [u8],
    /// Set once this control endpoint has been answered. A route role is minted
    /// once per declared edge; a further request over the same endpoint is
    /// refused rather than silently issuing a duplicate edge.
    answered: bool,
}

/// One provisioned publisher: the fabric's receiving half of its route, and the
/// route it may publish on. A publisher that finished is retired from the wait
/// set, so no dead source is ever parked on.
struct Publisher {
    /// Fabric-side endpoint. Ingress: the fabric receives here.
    slot: u32,
    /// Fabric-side credit endpoint. Egress: the fabric tells the publisher its
    /// loan has been copied and settled, so the publisher can exit without
    /// reclaiming a region the fabric is still reading.
    credit_slot: u32,
    route: usize,
    finished: bool,
    qos: TransportQos,
    last_assertion_ns: u64,
    /// Per-publisher bounded durable history. Entries hold ordinary frame
    /// references and are replayed only to later compatible subscribers.
    retained: StreamHistory,
}

/// One provisioned subscriber, with its declared delivery bound and the
/// accounting that makes eviction observable.
struct Subscriber {
    /// Fabric-side data endpoint. Egress: the fabric sends samples and events
    /// here, and the subscriber only receives.
    slot: u32,
    /// Fabric-side ack endpoint. Ingress: the subscriber sends slot releases
    /// here. A separate channel so a reader never holds send authority on the
    /// route it reads.
    ack_slot: u32,
    route: usize,
    /// Supervision handle naming the subscriber task. A downstream loan names
    /// its receiver through this capability, never an ambient task id.
    supervision_slot: u32,
    history: StreamHistory,
    /// Delivery slots in flight: samples sent but not yet acked. Bounded by the
    /// declared history depth, which is what makes KEEP_LAST bite.
    in_flight: usize,
    /// Whether a `STREAM_END` event has been emitted for this subscriber.
    ended: bool,
    qos: TransportQos,
    matched_publishers: u32,
    deadline_reported: bool,
    liveliness_reported: bool,
    retry_count: u32,
    terminal: bool,
    retry_interval_ns: u64,
    last_retry_ns: u64,
}

#[derive(Clone, Copy)]
struct LateSubscriber {
    fabric_slot: u32,
    client_slot: u32,
    history: StreamHistory,
    qos: TransportQos,
    received: bool,
    delivered: bool,
}

/// One fabric-owned sample frame. `refs` is the number of subscriber histories
/// still naming it; the frame is released when that reaches zero, so an evicted
/// sample frees its storage without disturbing a subscriber still holding it.
#[derive(Clone, Copy)]
struct Frame {
    refs: usize,
    sequence: u64,
    type_identity: u64,
    flags: u32,
    /// Inline payload bytes, valid for `payload_len` when `buffer_slot` is
    /// `None`.
    payload: [u8; MAX_INLINE_BYTES],
    payload_len: usize,
    /// Fabric-owned sealed buffer holding a large sample's single copy.
    buffer_slot: Option<u32>,
    buffer_len: u64,
    admitted_ns: u64,
}

impl Frame {
    const EMPTY: Self = Self {
        refs: 0,
        sequence: 0,
        type_identity: 0,
        flags: 0,
        payload: [0; MAX_INLINE_BYTES],
        payload_len: 0,
        buffer_slot: None,
        buffer_len: 0,
        admitted_ns: 0,
    };
}

fn main() {
    if option_env!("SLIME_FABRIC_VISIBILITY_CHECK") == Some("1") {
        visibility_broker::run();
        return;
    }
    if option_env!("SLIME_FABRIC_CALL_CHECK") == Some("1") {
        let controls = request_response_controls(FABRIC_CALL_CLIENTS);
        call_broker::Broker::new(
            FACTORY_SLOT,
            BUFFER_FACTORY_SLOT,
            controls.clients,
            controls.server,
            controls.time,
            0,
        )
        .run();
        slime_rt::debug_write(b"[fabric] call plane complete\n");
        return;
    }
    if option_env!("SLIME_FABRIC_OPERATION_CHECK") == Some("1") {
        let controls = request_response_controls(FABRIC_OPERATION_CLIENTS);
        operation_broker::Broker::new(
            FACTORY_SLOT,
            controls.clients,
            controls.server,
            controls.time,
            6,
        )
        .run();
        slime_rt::debug_write(b"[fabric] operation plane complete\n");
        return;
    }
    let routes: [[u8; 32]; ROUTE_COUNT] = [
        route_identity(
            ROUTE_NAMES[0],
            &telemetry_stream::INTERFACE_IDENTITY,
            CONTRACT_KIND_STREAM,
        ),
        route_identity(
            ROUTE_NAMES[1],
            &diagnostics_stream::INTERFACE_IDENTITY,
            CONTRACT_KIND_STREAM,
        ),
    ];
    let type_tags: [u64; ROUTE_COUNT] = [telemetry_stream::TYPE_TAG, diagnostics_stream::TYPE_TAG];

    let mut clients = control_clients();
    let mut publishers: [Option<Publisher>; MAX_PARTICIPANTS] = [const { None }; MAX_PARTICIPANTS];
    let mut subscribers: [Option<Subscriber>; MAX_PARTICIPANTS] =
        [const { None }; MAX_PARTICIPANTS];
    let mut frames = [Frame::EMPTY; MAX_FRAMES];

    provision(&mut clients, &routes, &mut publishers, &mut subscribers);
    slime_rt::debug_write(b"[fabric] every declared stream edge provisioned\n");

    broker(&type_tags, &mut publishers, &mut subscribers, &mut frames);
    slime_rt::debug_write(b"[fabric] stream plane complete\n");
}

struct RequestResponseControls {
    clients: [u32; 2],
    server: u32,
    time: u32,
}

/// Resolve one request/response plane's authenticated control slots from the
/// manifest-derived table. The table is authoritative for ordering; the role
/// assertions make a reordered or misclassified grant a build-time-visible
/// failure instead of a broker reading the wrong capability slot.
fn request_response_controls(table: &[&[u8]]) -> RequestResponseControls {
    assert!(
        table.len() == 4,
        "request/response plane must declare four controls"
    );
    let slot = |component: &[u8]| {
        table
            .iter()
            .position(|entry| *entry == component)
            .map(|index| FABRIC_FIRST_CONTROL_SLOT + index as u32)
            .unwrap_or_else(|| fail(b"request/response control missing"))
    };
    let (client_a, client_b, server, time) = if table[0].starts_with(b"fabric-call-") {
        (
            b"fabric-call-client".as_slice(),
            b"fabric-call-client-b".as_slice(),
            b"fabric-call-server".as_slice(),
            b"fabric-call-time".as_slice(),
        )
    } else {
        (
            b"fabric-op-client".as_slice(),
            b"fabric-op-client-b".as_slice(),
            b"fabric-op-server".as_slice(),
            b"fabric-op-time".as_slice(),
        )
    };
    RequestResponseControls {
        clients: [slot(client_a), slot(client_b)],
        server: slot(server),
        time: slot(time),
    }
}

/// The control-endpoint table, in the fixed order init granted the slots. Built
/// from the same generated participant table the graph resource was encoded
/// from, so a component the manifest does not declare cannot appear here.
fn control_clients() -> [Client; FABRIC_CLIENTS.len()] {
    let mut index = 0;
    [(); FABRIC_CLIENTS.len()].map(|()| {
        let component = FABRIC_CLIENTS[index];
        let client = Client {
            control_slot: FIRST_CONTROL_SLOT + index as u32,
            component,
            answered: false,
        };
        index += 1;
        client
    })
}

/// C8.3 provisioning round: mint both halves of every declared edge and move
/// each participant its exact narrowed role.
///
/// The fabric keeps the opposite half of every edge — that is what lets it
/// broker later — and hands out only the participant's side. Requests are
/// answered until every control endpoint has been answered or died, so the
/// service never proceeds to brokering with an unclaimed declared edge.
fn provision(
    clients: &mut [Client],
    routes: &[[u8; 32]; ROUTE_COUNT],
    publishers: &mut [Option<Publisher>; MAX_PARTICIPANTS],
    subscribers: &mut [Option<Subscriber>; MAX_PARTICIPANTS],
) {
    let mut parked = false;
    while clients.iter().any(|client| !client.answered) {
        // Sweep every unanswered control endpoint through its non-blocking ABI
        // first. Only when all of them would block is parking correct: probing
        // before parking is what closes the lost-wakeup window.
        let mut progressed = false;
        for client in clients.iter_mut().filter(|client| !client.answered) {
            let mut message = [0u8; MAX_MSG];
            let mut received = [0u64; MAX_CAPS_PER_MSG];
            let control_slot = client.control_slot;
            let length = match slime_rt::recv(control_slot, &mut message, &mut received) {
                ERR_WOULDBLOCK => continue,
                ERR_PEER_DEAD => {
                    // A client that died before asking gets no edge, and its
                    // route capability stays with the fabric.
                    slime_rt::debug_write(b"[fabric] control peer died: ");
                    slime_rt::debug_write(client.component);
                    slime_rt::debug_write(b"\n");
                    client.answered = true;
                    progressed = true;
                    continue;
                }
                n if n < 0 => fail(b"control recv"),
                n => n as usize,
            };
            progressed = true;
            client.answered = true;
            // A provisioning request carries no capabilities. One that does is
            // malformed, and its caps are released rather than retained.
            for slot in received.iter().filter(|slot| **slot != 0) {
                let _ = slime_rt::cap_drop(*slot as u32);
            }

            let request = match WireFabricRequest::decode(&message[..length.min(MAX_MSG)]) {
                Some(request) if length == REQUEST_LEN && valid_fabric_request(&request) => request,
                _ => {
                    deny(control_slot, &routes[0], STATUS_BAD_REQUEST);
                    continue;
                }
            };

            // The request's own route name, direction, and type identity are
            // read here only to be discarded. Authority comes from the caller's
            // control endpoint and the generation graph, so a component
            // supplying the exact strings of a route it was never granted gets
            // the same answer as one supplying nothing.
            let _ = (request.direction, request.type_identity, request.route_name);

            if declared_edges(client.component) == 0 {
                slime_rt::debug_write(b"[fabric] ungranted component denied: ");
                slime_rt::debug_write(client.component);
                slime_rt::debug_write(b"\n");
                deny(control_slot, &routes[0], STATUS_NOT_GRANTED);
                continue;
            }

            // One request provisions every edge the graph declares for this
            // component: a participant on two routes receives two roles, each
            // narrowed on its own. The client learns how many to expect from
            // the same graph, so no count crosses as authority.
            for (component, route_name, _, direction) in FABRIC_PARTICIPANTS.iter() {
                if *component != client.component {
                    continue;
                }
                // A route this service does not carry is not this service's to
                // provision. Call and operation routes are declared in the same
                // graph and owned by C8.6/C8.7; skipping them here is why a
                // component on one holds no stream authority by accident.
                let Some(route) = route_index(route_name) else {
                    continue;
                };
                provision_edge(
                    client.component,
                    control_slot,
                    &routes[route],
                    route,
                    *direction,
                    publishers,
                    subscribers,
                );
            }
        }
        if progressed {
            continue;
        }
        // Every source would block: park across the whole set at once. This is
        // the only place provisioning waits, and it burns no CPU doing it.
        if !parked {
            slime_rt::debug_write(b"[fabric] idle: parked on control endpoints\n");
            parked = true;
        }
        park_on_controls(clients);
    }
}

/// Mint one edge's endpoint pair and move the participant its narrowed half.
///
/// Both halves are created here and only one leaves, so the fabric holds the
/// opposite end of every edge it provisioned: a publisher's `RIGHT_SEND` half
/// is matched by a fabric receive half, and a subscriber's `RIGHT_RECV` half by
/// a fabric send half. That ownership is what makes brokering possible without
/// any participant ever holding both directions.
fn provision_edge(
    component: &'static [u8],
    control_slot: u32,
    route: &[u8; 32],
    route_index: usize,
    direction: u32,
    publishers: &mut [Option<Publisher>; MAX_PARTICIPANTS],
    subscribers: &mut [Option<Subscriber>; MAX_PARTICIPANTS],
) {
    let (fabric_side, participant_side) = match slime_rt::endpoint_create(FACTORY_SLOT) {
        Ok(pair) => pair,
        Err(_) => fail(b"route endpoints"),
    };
    let rights = match direction {
        DIRECTION_PUBLISH => RIGHT_SEND,
        DIRECTION_SUBSCRIBE => RIGHT_RECV,
        _ => fail(b"stream route declares a non-stream direction"),
    };

    // The descriptor states exactly what the kernel is about to install.
    // `RIGHT_TRANSFER` is absent from the mask and `FLAG_RETAIN_TRANSFER` is
    // unset, so the destination receives a role it cannot re-delegate.
    let descriptor = WireCapabilityTransfer {
        magic: CAPABILITY_TRANSFER_MAGIC,
        version: FORMAT_VERSION,
        status: 0,
        flags: 0,
        object_kind: OBJECT_KIND_ENDPOINT,
        direction,
        rights_mask: rights,
        route_identity: *route,
    };
    if slime_rt::cap_transfer(control_slot, participant_side, &descriptor.encode()) != ERR_SUCCESS {
        fail(b"provisioning transfer");
    }

    match direction {
        DIRECTION_PUBLISH => {
            // A publisher that loans a large sample cannot exit until the
            // fabric has taken its copy: task termination settles every loan
            // the task lent, so leaving early would reclaim the region out from
            // under the copy in flight (the C7.5 retention rule). The data
            // endpoint is send-only, so the settle signal needs its own
            // opposite-facing channel — receive-only at the publisher, exactly
            // mirroring the subscriber's ack channel.
            let (fabric_credit_side, participant_credit_side) =
                match slime_rt::endpoint_create(FACTORY_SLOT) {
                    Ok(pair) => pair,
                    Err(_) => fail(b"credit endpoints"),
                };
            let credit_descriptor = WireCapabilityTransfer {
                rights_mask: RIGHT_RECV,
                ..descriptor
            };
            if slime_rt::cap_transfer(
                control_slot,
                participant_credit_side,
                &credit_descriptor.encode(),
            ) != ERR_SUCCESS
            {
                fail(b"credit channel transfer");
            }
            let free = publishers
                .iter()
                .position(Option::is_none)
                .unwrap_or_else(|| fail(b"publisher table exhausted"));
            let qos = declared_qos(component, ROUTE_NAMES[route_index]);
            let retained_depth = qos.retained_depth as usize;
            let retained = StreamHistory::new(retained_depth.max(1))
                .unwrap_or_else(|| fail(b"declared retained depth"));
            publishers[free] = Some(Publisher {
                slot: fabric_side,
                credit_slot: fabric_credit_side,
                route: route_index,
                finished: false,
                qos,
                last_assertion_ns: 0,
                retained,
            });
        }
        _ => {
            // A subscriber's data endpoint is receive-only, so it cannot carry
            // the ack that releases a delivery slot. Rather than widening the
            // role — which would let a subscriber publish on the route it reads
            // — mint a second, opposite-facing pair for acks alone. The
            // subscriber gets `RIGHT_SEND` on the ack channel and `RIGHT_RECV`
            // on the data channel: two capabilities, neither of which is the
            // other's direction, and no route on which it holds both.
            let (fabric_ack_side, participant_ack_side) =
                match slime_rt::endpoint_create(FACTORY_SLOT) {
                    Ok(pair) => pair,
                    Err(_) => fail(b"ack endpoints"),
                };
            let ack_descriptor = WireCapabilityTransfer {
                rights_mask: RIGHT_SEND,
                ..descriptor
            };
            if slime_rt::cap_transfer(control_slot, participant_ack_side, &ack_descriptor.encode())
                != ERR_SUCCESS
            {
                fail(b"ack channel transfer");
            }
            let free = subscribers
                .iter()
                .position(Option::is_none)
                .unwrap_or_else(|| fail(b"subscriber table exhausted"));
            let depth = declared_history_depth(component, ROUTE_NAMES[route_index]);
            let history =
                StreamHistory::new(depth).unwrap_or_else(|| fail(b"declared history depth"));
            let qos = declared_qos(component, ROUTE_NAMES[route_index]);
            subscribers[free] = Some(Subscriber {
                slot: fabric_side,
                ack_slot: fabric_ack_side,
                route: route_index,
                // Bound at provisioning time to the control endpoint's peer,
                // which init bound to this exact component at spawn.
                supervision_slot: supervision_slot_for(component),
                history,
                in_flight: 0,
                ended: false,
                retry_interval_ns: qos.deadline_ns.max(1),
                qos,
                matched_publishers: 0,
                deadline_reported: false,
                liveliness_reported: false,
                retry_count: 0,
                terminal: false,
                last_retry_ns: 0,
            });
        }
    }

    refresh_matches(route_index, publishers, subscribers);
    slime_rt::debug_write(b"[fabric] provisioned ");
    slime_rt::debug_write(component);
    slime_rt::debug_write(b" ");
    slime_rt::debug_write(ROUTE_NAMES[route_index].as_bytes());
    slime_rt::debug_write(if direction == DIRECTION_PUBLISH {
        b" publish\n" as &[u8]
    } else {
        b" subscribe\n"
    });
}

/// C8.4 brokering loop: move samples from every live publisher to every matched
/// subscriber, bounded by each subscriber's declared KEEP_LAST depth.
///
/// One pass sweeps every ingress and every ack, then drains what it can into
/// each subscriber; only when nothing moved anywhere does it park across the
/// whole set. The loop retires a source before parking again, so no dead
/// endpoint is ever left in the wait set to spin on.
fn broker(
    type_tags: &[u64; ROUTE_COUNT],
    publishers: &mut [Option<Publisher>; MAX_PARTICIPANTS],
    subscribers: &mut [Option<Subscriber>; MAX_PARTICIPANTS],
    frames: &mut [Frame; MAX_FRAMES],
) {
    let mut parked = false;
    let mut now_ns = 0u64;
    let mut pending_time = None;
    let mut late_subscriber = None;
    let mut late_replay_done = false;
    loop {
        let mut progressed = false;

        for index in 0..publishers.len() {
            if publishers[index]
                .as_ref()
                .is_none_or(|publisher| publisher.finished)
            {
                continue;
            }
            if pump_publisher(index, now_ns, type_tags, publishers, subscribers, frames) {
                progressed = true;
            }
        }

        // Shared samples consume one fabric loan per matched subscriber. Walk
        // this table in reverse so the fixed first subscriber cannot always
        // take the only immediately-available loan before a later peer gets a
        // turn; acknowledgements are still drained for every participant each
        // pass. Inline delivery order is not observable authority.
        for index in (0..subscribers.len()).rev() {
            if subscribers[index].is_none() {
                continue;
            }
            if drain_acks(index, type_tags, subscribers, frames) {
                progressed = true;
            }
            if deliver(index, now_ns, type_tags, subscribers, frames) {
                progressed = true;
            }
        }

        // Deterministic tie order: ingress data first, then acknowledgements and
        // delivery, then exactly one explicit monotonic-time transition.
        if qos_check() {
            receive_time(&mut pending_time);
            if apply_time(
                &mut now_ns,
                &mut pending_time,
                publishers,
                subscribers,
                frames,
            ) {
                progressed = true;
            }
        }
        if qos_check() && !late_replay_done && now_ns >= 200 {
            if late_subscriber.is_none() {
                late_subscriber = Some(create_late_subscriber(publishers, frames));
                progressed = true;
            }
            if pump_late_subscriber(&mut late_subscriber, now_ns, frames) {
                progressed = true;
            }
            late_replay_done = late_subscriber.is_none();
        }

        // A route whose publishers have all finished and whose subscribers hold

        // nothing further is done: emit one terminal event per subscriber so it
        // stops waiting on a route that will produce nothing more.
        for route in 0..ROUTE_COUNT {
            if !route_finished(route, publishers) {
                continue;
            }
            for index in 0..subscribers.len() {
                if announce_end(index, route, type_tags, subscribers) {
                    progressed = true;
                }
            }
        }

        if subscribers
            .iter()
            .flatten()
            .all(|subscriber| subscriber.ended)
        {
            // The QoS check owns one explicit time channel. Its peer closes only
            // after every scheduled boundary has been acknowledged; until then
            // the broker stays alive even when all stream routes have ended.
            if !qos_check() || time_peer_dead() {
                release_retained(publishers, frames);
                return;
            }
        }

        if progressed {
            parked = false;
            continue;
        }
        if !parked {
            slime_rt::debug_write(b"[fabric] idle: parked on stream sources\n");
            parked = true;
        }
        park_on_streams(publishers, subscribers);
    }
}

/// Consume at most one message from one publisher's ingress.
///
/// Returns whether anything moved. A malformed sample is dropped without
/// disturbing the route: an admitted sample of the wrong type, the wrong
/// length, or naming a stale loan never reaches a subscriber, and the publisher
/// stays live so one bad message cannot retire a declared edge.
fn pump_publisher(
    index: usize,
    now_ns: u64,
    type_tags: &[u64; ROUTE_COUNT],
    publishers: &mut [Option<Publisher>; MAX_PARTICIPANTS],
    subscribers: &mut [Option<Subscriber>; MAX_PARTICIPANTS],
    frames: &mut [Frame; MAX_FRAMES],
) -> bool {
    let (slot, credit_slot, route, publisher_qos) = {
        let publisher = publishers[index].as_ref().expect("live publisher");
        (
            publisher.slot,
            publisher.credit_slot,
            publisher.route,
            publisher.qos,
        )
    };
    // Admitting a sample can cost one frame plus one loan per matched
    // subscriber, so refuse to start when no frame is free rather than tearing
    // down a partial fan-out.
    if !frames.iter().any(|frame| frame.refs == 0) {
        return false;
    }

    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    let length = match slime_rt::recv(slot, &mut message, &mut received) {
        ERR_WOULDBLOCK => return false,
        ERR_PEER_DEAD => {
            publishers[index].as_mut().expect("live publisher").finished = true;
            return true;
        }
        n if n < 0 => fail(b"stream recv"),
        n => n as usize,
    };
    let loan_slot = (received[0] != 0).then(|| received[0] as u32);
    // A framed record carries at most one capability: the loan a descriptor
    // names. `admit_shared` consumes that one, so every further slot is a
    // malformed extra and is released here — on every path, including the
    // descriptor path, so a peer cannot strand kernel objects in the fabric by
    // attaching more than the format admits.
    release_received(&received[1..]);
    if length != MAX_MSG {
        // Not a framed record at all, so nothing consumes the first slot
        // either. The extras are already gone.
        release_received(&received[..1]);
        return true;
    }

    let magic = u32::from_le_bytes(message[..4].try_into().expect("message prefix"));
    let admitted = match magic {
        STREAM_SAMPLE_MAGIC => admit_inline(&message, type_tags[route], frames),
        SAMPLE_DESCRIPTOR_MAGIC => {
            // The sequence the descriptor claimed, read before admission so a
            // rejected sample still credits the exact one its publisher is
            // waiting on.
            let sequence = WireSampleDescriptor::decode(&message)
                .map(|descriptor| descriptor.sequence)
                .unwrap_or(0);
            let admitted = admit_shared(&message, type_tags[route], loan_slot, frames);
            // The upstream loan is settled by now, whether the copy succeeded
            // or not, so the publisher may reclaim its buffer and exit. Credit
            // it either way: a publisher left waiting on a rejected sample
            // would hang holding pages the fabric no longer wants.
            credit_publisher(credit_slot, type_tags[route], sequence);
            admitted
        }
        _ => {
            slime_rt::debug_write(b"[fabric] reject: unknown record magic\n");
            None
        }
    };
    // The descriptor's own loan is consumed inside `admit_shared`; for every
    // other record kind, a capability had no business riding along at all.
    if magic != SAMPLE_DESCRIPTOR_MAGIC {
        release_received(&received[..1]);
    }
    let Some(frame) = admitted else {
        slime_rt::debug_write(b"[fabric] malformed sample rejected\n");
        return true;
    };
    publishers[index]
        .as_mut()
        .expect("live publisher")
        .last_assertion_ns = now_ns;
    frames[frame].admitted_ns = now_ns;
    if frames[frame].flags & FLAG_LAST != 0 {
        publishers[index].as_mut().expect("live publisher").finished = true;
    }
    fan_out(frame, route, index, &publisher_qos, subscribers, frames);
    retain_sample(index, frame, publishers, frames);
    true
}

/// Tell a publisher its loaned sample has been taken, so it may reclaim.
///
/// A distinct `SAMPLE_TAKEN` event naming the settled sequence, not a reused
/// terminal notice: a per-sample credit and an end-of-route notice mean
/// different things to their reader, and a publisher of several large samples
/// must be able to tell which one was settled. A publisher that exited first
/// simply never reads it, and its own termination settled the loan anyway.
fn credit_publisher(credit_slot: u32, type_identity: u64, sequence: u64) {
    if sequence == 0 {
        // A credit names the sample it settles, so a descriptor that did not
        // decode has nothing to credit. Its loan was returned regardless.
        return;
    }
    let event = WireStreamEvent {
        magic: STREAM_EVENT_MAGIC,
        version: FORMAT_VERSION,
        event: EVENT_SAMPLE_TAKEN,
        flags: 0,
        lost: 0,
        sequence,
        type_identity,
        reserved: [0; 24],
    };
    match slime_rt::send(credit_slot, &event.encode(), &[]) {
        ERR_SUCCESS | ERR_WOULDBLOCK | ERR_PEER_DEAD => {}
        _ => fail(b"publisher credit"),
    }
}

/// Copy one inline sample into a free fabric frame, or reject it.
fn admit_inline(
    message: &[u8; MAX_MSG],
    expected_type: u64,
    frames: &mut [Frame; MAX_FRAMES],
) -> Option<usize> {
    let Some(sample) = WireStreamSample::decode(message) else {
        slime_rt::debug_write(b"[fabric] reject: inline decode\n");
        return None;
    };
    if !slime_proto::valid_stream_sample(&sample, expected_type, MAX_INLINE_BYTES) {
        slime_rt::debug_write(b"[fabric] reject: inline validation\n");
        return None;
    }
    let Some(index) = frames.iter().position(|frame| frame.refs == 0) else {
        slime_rt::debug_write(b"[fabric] reject: inline no free frame\n");
        return None;
    };
    frames[index] = Frame {
        refs: 0,
        sequence: sample.sequence,
        type_identity: sample.type_identity,
        flags: sample.flags,
        payload: sample.payload,
        payload_len: sample.payload_len as usize,
        buffer_slot: None,
        buffer_len: 0,
        admitted_ns: 0,
    };
    Some(index)
}

/// Take one large sample through the fabric's single copy.
///
/// The publisher's loan is mapped read-only, copied once into a fabric-owned
/// buffer, sealed, and returned immediately. From here on every subscriber gets
/// its own downstream loan of the fabric's copy, so the publisher's buffer is
/// released as soon as the copy lands rather than being retained for the
/// slowest reader.
fn admit_shared(
    message: &[u8; MAX_MSG],
    expected_type: u64,
    loan_slot: Option<u32>,
    frames: &mut [Frame; MAX_FRAMES],
) -> Option<usize> {
    let Some(descriptor) = WireSampleDescriptor::decode(message) else {
        slime_rt::debug_write(b"[fabric] reject: descriptor decode\n");
        return None;
    };
    let Some(loan_slot) = loan_slot else {
        slime_rt::debug_write(b"[fabric] reject: descriptor without loan\n");
        return None;
    };
    // Validate before mapping or allocating anything: an unknown flag, a
    // non-loan capability kind, another route's type, or a length past the
    // fabric's own copy budget never reaches the copy path.
    //
    // `expected_loan` is the descriptor's own `loan_id`, which makes that one
    // arm a non-zero check rather than a binding — and deliberately so. The
    // fabric has no way to ask the kernel for the identity behind a loan
    // capability, so it has nothing independent to compare against. What
    // actually binds the two is the kernel: `shared_buffer_loan_map` resolves
    // the region from the *capability*, never from the claimed id, and admits
    // only the loan's named receiver. A descriptor that lies about its id
    // therefore maps the bytes its capability really names, or nothing at all.
    // The C7 receiver's identical call is a real binding because it holds the
    // id from its own earlier `shared_buffer_loan`; a broker never does.
    if !valid_sample_descriptor(&descriptor, descriptor.loan_id, expected_type, PAGE)
        || descriptor.capability_kind != CAPABILITY_KIND_LOAN
        || descriptor.length > COPY_PAGES as u64 * PAGE
    {
        slime_rt::debug_write(b"[fabric] reject: descriptor validation\n");
        let _ = slime_rt::shared_buffer_return(loan_slot);
        return None;
    }
    let Some(index) = frames.iter().position(|frame| frame.refs == 0) else {
        // No frame to hold the copy. Settle the publisher's loan anyway: it is
        // waiting on the credit to reclaim its buffer, and leaving the loan
        // outstanding would strand its pages for the rest of the boot.
        slime_rt::debug_write(b"[fabric] reject: no free frame\n");
        let _ = slime_rt::shared_buffer_return(loan_slot);
        return None;
    };

    let copy = match slime_rt::shared_buffer_create(BUFFER_FACTORY_SLOT, COPY_PAGES, true) {
        Ok(buffer) => buffer,
        Err(_) => {
            slime_rt::debug_write(b"[fabric] reject: copy buffer create\n");
            let _ = slime_rt::shared_buffer_return(loan_slot);
            return None;
        }
    };
    // Map at the descriptor's own offset, not zero: `valid_sample_descriptor`
    // admits any page-aligned in-bounds offset, and the C7 receiver honours it,
    // so hard-coding zero would silently broker the wrong bytes for a publisher
    // that loaned a subrange.
    let mapped_upstream = slime_rt::shared_buffer_loan_map(
        loan_slot,
        UPSTREAM_BASE,
        descriptor.offset,
        descriptor.length,
    ) == ERR_SUCCESS;
    let mapped_copy = mapped_upstream
        && slime_rt::shared_buffer_map(copy.slot, COPY_BASE, 0, descriptor.length, true)
            == ERR_SUCCESS;
    if mapped_copy {
        // SAFETY: the kernel installed a read-only mapping of exactly
        // `descriptor.length` bytes at `UPSTREAM_BASE` and a writable mapping
        // of the same length at `COPY_BASE`. The two ranges are disjoint by
        // construction and both stay mapped until the unmaps below.
        unsafe {
            let source = UPSTREAM_BASE as *const u8;
            let destination = COPY_BASE as *mut u8;
            for offset in 0..descriptor.length as usize {
                destination
                    .add(offset)
                    .write_volatile(source.add(offset).read_volatile());
            }
        }
    }
    if mapped_upstream {
        let _ = slime_rt::shared_buffer_unmap(loan_slot, UPSTREAM_BASE);
    }
    if mapped_copy {
        let _ = slime_rt::shared_buffer_unmap(copy.slot, COPY_BASE);
    }
    // The copy is made, so the publisher's loan is settled now rather than held
    // for the slowest subscriber. This is the "at most once" copy: every
    // downstream loan below refers to the fabric's own buffer.
    let _ = slime_rt::shared_buffer_return(loan_slot);
    if !mapped_copy {
        slime_rt::debug_write(if mapped_upstream {
            b"[fabric] reject: copy buffer map\n" as &[u8]
        } else {
            b"[fabric] reject: upstream loan map\n"
        });
        let _ = slime_rt::shared_buffer_release(copy.slot);
        return None;
    }
    // Sealing before any downstream loan is what makes the fan-out read-only:
    // a loan requires an irreversibly sealed source, and the fabric drops its
    // own write authority in the same step.
    if slime_rt::shared_buffer_seal(copy.slot) != ERR_SUCCESS {
        slime_rt::debug_write(b"[fabric] reject: copy buffer seal\n");
        let _ = slime_rt::shared_buffer_release(copy.slot);
        return None;
    }

    frames[index] = Frame {
        refs: 0,
        sequence: descriptor.sequence,
        type_identity: descriptor.type_identity,
        flags: descriptor.flags,
        payload: [0; MAX_INLINE_BYTES],
        payload_len: 0,
        buffer_slot: Some(copy.slot),
        buffer_len: descriptor.length,
        admitted_ns: 0,
    };
    slime_rt::debug_write(b"[fabric] large sample copied once\n");
    Some(index)
}

/// Add one admitted sample to the publisher's fixed durable window. The
/// retained ring owns one extra frame reference; eviction releases exactly that
/// reference, so durable history cannot outlive its declared bound.
fn retain_sample(
    publisher_index: usize,
    frame: usize,
    publishers: &mut [Option<Publisher>; MAX_PARTICIPANTS],
    frames: &mut [Frame; MAX_FRAMES],
) {
    let publisher = publishers[publisher_index]
        .as_mut()
        .expect("live publisher");
    if publisher.qos.durability as u32 != DURABILITY_RETAINED || publisher.qos.retained_depth == 0 {
        return;
    }
    frames[frame].refs += 1;
    let entry = HistoryEntry {
        sequence: frames[frame].sequence,
        publisher: publisher_index as u32,
        slot: frame as u32,
        inline: frames[frame].buffer_slot.is_none(),
    };
    if let Some(evicted) = publisher.retained.push(entry) {
        release_frame(evicted.slot as usize, frames);
    }
}

/// Offer one admitted frame to every subscriber matched on its route.
///
/// Matching is the route index plus offered/requested QoS compatibility. A
/// subscriber on another route or with a stronger request is not offered the
/// frame at all.
fn fan_out(
    frame: usize,
    route: usize,
    publisher_index: usize,
    publisher_qos: &TransportQos,
    subscribers: &mut [Option<Subscriber>; MAX_PARTICIPANTS],
    frames: &mut [Frame; MAX_FRAMES],
) {
    // Read the frame's identity once: the loop below borrows `frames` mutably
    // to release an eviction, so it cannot hold a reference to this frame.
    let entry = HistoryEntry {
        sequence: frames[frame].sequence,
        publisher: publisher_index as u32,
        slot: frame as u32,
        inline: frames[frame].buffer_slot.is_none(),
    };
    for subscriber in subscribers.iter_mut().flatten() {
        if subscriber.route != route
            || !TransportQos::offer_satisfies(publisher_qos, &subscriber.qos)
        {
            continue;
        }
        frames[frame].refs += 1;
        // KEEP_LAST: admitting past the declared depth evicts the oldest, and
        // the ring counts the loss so it can be reported once when delivery
        // resumes. The evicted frame's reference is released here.
        if let Some(evicted) = subscriber.history.push(entry) {
            // The evicted sample may have been in flight — sent but not yet
            // acked. Its delivery slot is gone with it, so release the count
            // too; otherwise a stalled subscriber's `in_flight` would ratchet
            // up until it permanently exceeded its depth and never received
            // again.
            subscriber.in_flight = subscriber.in_flight.saturating_sub(1);
            release_frame(evicted.slot as usize, frames);
        }
    }
    if frames[frame].refs == 0 {
        // No subscriber matched this route, so the frame was never referenced.
        // Release its backing storage here: a large sample published to a route
        // with no live subscriber would otherwise retain a fabric buffer for
        // the rest of the boot.
        if let Some(buffer_slot) = frames[frame].buffer_slot {
            let _ = slime_rt::shared_buffer_release(buffer_slot);
        }
        frames[frame] = Frame::EMPTY;
    }
}
fn late_subscriber_qos(publisher: &Publisher) -> TransportQos {
    TransportQos {
        reliability: publisher.qos.reliability,
        durability: DURABILITY_RETAINED as u8,
        liveliness: publisher.qos.liveliness,
        deadline_ns: publisher.qos.deadline_ns,
        lifespan_ns: publisher.qos.lifespan_ns,
        lease_ns: publisher.qos.lease_ns,
        history_depth: publisher.qos.retained_depth,
        retained_depth: publisher.qos.retained_depth,
    }
}

/// Provision a real late subscriber and copy only the retained publisher's
/// declared live window into its bounded delivery history.
fn create_late_subscriber(
    publishers: &mut [Option<Publisher>; MAX_PARTICIPANTS],
    frames: &mut [Frame; MAX_FRAMES],
) -> LateSubscriber {
    let publisher_index = publishers
        .iter()
        .position(|publisher| {
            publisher.as_ref().is_some_and(|publisher| {
                publisher.qos.durability as u32 == DURABILITY_RETAINED
                    && publisher.retained.peek().is_some_and(|entry| entry.inline)
            })
        })
        .unwrap_or_else(|| fail(b"no inline retained publisher"));
    let publisher = publishers[publisher_index]
        .as_ref()
        .expect("retained publisher");
    let qos = late_subscriber_qos(publisher);
    if !TransportQos::offer_satisfies(&publisher.qos, &qos) {
        fail(b"late retained QoS mismatch");
    }
    let (fabric_slot, client_slot) =
        slime_rt::endpoint_create(FACTORY_SLOT).unwrap_or_else(|_| fail(b"late subscriber"));
    let mut history = StreamHistory::new(qos.history_depth as usize)
        .unwrap_or_else(|| fail(b"late subscriber history"));
    let mut retained = publisher.retained;
    while let Some(entry) = retained.pop() {
        if frames[entry.slot as usize].buffer_slot.is_some() {
            continue;
        }
        frames[entry.slot as usize].refs += 1;
        if history.push(entry).is_some() {
            fail(b"retained replay exceeded declared window");
        }
    }
    if history.is_empty() {
        fail(b"late subscriber has no inline retained sample");
    }
    slime_rt::debug_write(b"[fabric] retained history offered to late subscriber\n");
    LateSubscriber {
        fabric_slot,
        client_slot,
        history,
        qos,
        received: false,
        delivered: false,
    }
}

fn pump_late_subscriber(
    late: &mut Option<LateSubscriber>,
    now_ns: u64,
    frames: &mut [Frame; MAX_FRAMES],
) -> bool {
    let Some(subscriber) = late.as_mut() else {
        return false;
    };
    if !subscriber.delivered {
        let Some(entry) = subscriber.history.peek() else {
            fail(b"late subscriber received no retained sample");
        };
        let frame = entry.slot as usize;
        let sample = WireStreamSample {
            magic: STREAM_SAMPLE_MAGIC,
            version: FORMAT_VERSION,
            flags: frames[frame].flags,
            payload_len: frames[frame].payload_len as u32,
            sequence: frames[frame].sequence,
            type_identity: frames[frame].type_identity,
            payload: frames[frame].payload,
        };
        match slime_rt::send(subscriber.fabric_slot, &sample.encode(), &[]) {
            ERR_SUCCESS => {
                subscriber.delivered = true;
                return true;
            }
            ERR_WOULDBLOCK => return false,
            _ => fail(b"late retained delivery"),
        }
    }
    if !subscriber.received {
        let Some(entry) = subscriber.history.peek() else {
            fail(b"late subscriber retained sample disappeared");
        };
        let frame = entry.slot as usize;
        let mut message = [0u8; MAX_MSG];
        let mut caps = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(subscriber.client_slot, &mut message, &mut caps) {
            n if n >= 0 => n as usize,
            ERR_WOULDBLOCK => return false,
            _ => fail(b"late retained receive"),
        };
        release_received(&caps);
        let Some(received) = WireStreamSample::decode(&message[..length]) else {
            fail(b"late retained decode")
        };
        if !slime_proto::valid_stream_sample(
            &received,
            frames[frame].type_identity,
            MAX_INLINE_BYTES,
        ) || received.sequence != entry.sequence
        {
            fail(b"late retained validation");
        }
        subscriber.received = true;
        slime_rt::debug_write(b"[fabric] retained history replayed to late subscriber\n");
        return true;
    }
    if now_ns >= subscriber.qos.lifespan_ns {
        while let Some(entry) = subscriber.history.pop() {
            release_frame(entry.slot as usize, frames);
        }
        slime_rt::debug_write(b"[fabric] QoS lifespan expired\n");
        slime_rt::debug_write(b"[fabric] retained history expired for late subscriber\n");
        let _ = slime_rt::cap_drop(subscriber.fabric_slot);
        let _ = slime_rt::cap_drop(subscriber.client_slot);
        *late = None;
        return true;
    }
    false
}

/// Send at most one queued sample to one subscriber.
/// Bounded by the declared depth: `in_flight` counts samples sent but not yet
/// acked, so a subscriber that stops acking stops receiving, and its publisher
/// keeps running against the KEEP_LAST ring instead of blocking the route.
fn deliver(
    index: usize,
    now_ns: u64,
    type_tags: &[u64; ROUTE_COUNT],
    subscribers: &mut [Option<Subscriber>; MAX_PARTICIPANTS],
    frames: &mut [Frame; MAX_FRAMES],
) -> bool {
    let Some(subscriber) = subscribers[index].as_mut() else {
        return false;
    };
    let route = subscriber.route;
    let type_identity = type_tags[route];
    let slot = subscriber.slot;
    if subscriber.matched_publishers == 0 {
        return false;
    }
    if subscriber.terminal {
        return false;
    }

    // Report loss before the next sample, so a subscriber learns of a gap ahead
    // of the sequence that follows it rather than after.
    //
    // Not gated on an empty in-flight window: a subscriber that stalls
    // permanently keeps its slots occupied forever, so waiting for them to
    // drain would mean its loss is never reported — and since the route cannot
    // end until every subscriber has been told, the whole fabric would hang on
    // one silent peer. The event is independent of the samples in flight, so
    // there is nothing to wait for.
    if let Some((lost, oldest)) = subscriber.history.take_loss() {
        let event = WireStreamEvent {
            magic: STREAM_EVENT_MAGIC,
            version: FORMAT_VERSION,
            event: EVENT_SAMPLE_LOST,
            flags: 0,
            lost,
            sequence: oldest,
            type_identity,
            reserved: [0; 24],
        };
        return match slime_rt::send(slot, &event.encode(), &[]) {
            ERR_SUCCESS => true,
            ERR_WOULDBLOCK | ERR_PEER_DEAD => false,
            _ => fail(b"loss event"),
        };
    }
    if subscriber.in_flight >= subscriber.history.depth() {
        return false;
    }
    // The first sample this subscriber has not been sent yet. A queued sample
    // stays in the ring until its ack settles it, so the head is what the
    // subscriber is still working through — sending it again would deliver one
    // sequence repeatedly and never advance.
    let Some(entry) = subscriber.history.entry_at(subscriber.in_flight) else {
        return false;
    };
    let frame = entry.slot as usize;
    if subscriber.qos.lifespan_ns != 0
        && now_ns.saturating_sub(frames[frame].admitted_ns) >= subscriber.qos.lifespan_ns
    {
        let expired = subscriber.history.pop().expect("queued frame");
        subscriber.in_flight = subscriber.in_flight.saturating_sub(1);
        release_frame(expired.slot as usize, frames);
        if send_qos_event(
            slot,
            EVENT_LIFESPAN_EXPIRED,
            entry.sequence,
            0,
            now_ns,
            type_identity,
        ) {
            slime_rt::debug_write(b"[fabric] QoS lifespan expired\n");
        }
        return true;
    }

    let sent = if let Some(buffer_slot) = frames[frame].buffer_slot {
        // One independently accounted downstream loan per subscriber, bound to
        // that subscriber by its supervision capability: the receiver is named
        // by capability, never by an ambient task id.
        let loan = match slime_rt::shared_buffer_loan(
            buffer_slot,
            subscriber.supervision_slot,
            0,
            frames[frame].buffer_len,
        ) {
            Ok(loan) => loan,
            // Quota or table pressure is backpressure, not a fault: the loans
            // in flight settle as their subscribers return them, and this
            // sample stays queued in the ring until one does. Failing here
            // would let a momentarily saturated fan-out kill the whole fabric.
            Err(ERR_WOULDBLOCK) | Err(ERR_OUT_OF_MEMORY) => return false,
            Err(_) => fail(b"downstream loan"),
        };
        let descriptor = WireSampleDescriptor {
            magic: SAMPLE_DESCRIPTOR_MAGIC,
            version: FORMAT_VERSION,
            flags: frames[frame].flags,
            capability_kind: CAPABILITY_KIND_LOAN,
            loan_id: loan.id,
            offset: 0,
            length: frames[frame].buffer_len,
            type_identity: frames[frame].type_identity,
            sequence: frames[frame].sequence,
            reserved: [0; 8],
        };
        // The loan rides with its descriptor in one message, exactly as the C7
        // sample plane delivers one: the receiver parses the bytes and finds
        // the capability they describe in the same `recv`, so there is no
        // window where it holds one without the other.
        //
        // The attachment moves the loan at the rights the kernel minted it
        // with — `RIGHT_BUFFER_MAP | RIGHT_TRANSFER`. The transfer bit is what
        // let it cross at all, and it grants no authority over the fabric's
        // buffer: a `SharedBufferLoan` is receiver-bound, so a subscriber that
        // passed it on would hand over a capability only itself can map or
        // return. Narrowing further would need a second message, which is the
        // window this shape exists to avoid.
        match slime_rt::send(slot, &descriptor.encode(), &[loan.slot]) {
            ERR_SUCCESS => {
                // One downstream loan per matched subscriber, each charged
                // separately against the fabric's own quota. Marked on the
                // delivering path only: a loan created and then revoked because
                // the send would block never reached a subscriber, so counting
                // it would let a retry inflate the fan-out the gate measures.
                slime_rt::debug_write(b"[fabric] downstream loan created\n");
                true
            }
            ERR_WOULDBLOCK | ERR_PEER_DEAD => {
                // Nothing crossed, so settle the loan we just created rather
                // than leaving an outstanding charge against the fabric.
                let _ = slime_rt::shared_buffer_revoke(buffer_slot, loan.id);
                false
            }
            _ => fail(b"deliver descriptor"),
        }
    } else {
        let mut payload = [0u8; MAX_INLINE_BYTES];
        payload[..frames[frame].payload_len]
            .copy_from_slice(&frames[frame].payload[..frames[frame].payload_len]);
        let sample = WireStreamSample {
            magic: STREAM_SAMPLE_MAGIC,
            version: FORMAT_VERSION,
            flags: frames[frame].flags,
            payload_len: frames[frame].payload_len as u32,
            sequence: frames[frame].sequence,
            type_identity: frames[frame].type_identity,
            payload,
        };
        match slime_rt::send(slot, &sample.encode(), &[]) {
            ERR_SUCCESS => true,
            ERR_WOULDBLOCK | ERR_PEER_DEAD => false,
            _ => fail(b"deliver sample"),
        }
    };
    if !sent {
        return false;
    }
    subscriber.in_flight += 1;
    subscriber.deadline_reported = false;
    if subscriber.in_flight == 1 {
        subscriber.last_retry_ns = now_ns;
    }
    true
}

/// Consume every pending ack from one subscriber, releasing a delivery slot and
/// the frame reference each one settles.
fn drain_acks(
    index: usize,
    type_tags: &[u64; ROUTE_COUNT],
    subscribers: &mut [Option<Subscriber>; MAX_PARTICIPANTS],
    frames: &mut [Frame; MAX_FRAMES],
) -> bool {
    let mut progressed = false;
    loop {
        let Some(subscriber) = subscribers[index].as_mut() else {
            return progressed;
        };
        let slot = subscriber.ack_slot;
        let type_identity = type_tags[subscriber.route];
        let mut message = [0u8; MAX_MSG];
        let mut received = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(slot, &mut message, &mut received) {
            ERR_WOULDBLOCK => return progressed,
            ERR_PEER_DEAD => {
                let _ = send_qos_event(
                    slot,
                    slime_proto::fabric_qos::EVENT_PEER_DEAD,
                    0,
                    0,
                    0,
                    type_identity,
                );
                slime_rt::debug_write(b"[fabric] QoS peer dead\n");
                retire_subscriber(index, subscribers, frames);
                return true;
            }
            n if n < 0 => fail(b"ack recv"),
            n => n as usize,
        };
        progressed = true;
        release_received(&received);
        let Some(ack) = WireStreamAck::decode(&message[..length.min(MAX_MSG)]) else {
            continue;
        };
        if length != MAX_MSG || !valid_stream_ack(&ack, type_identity) {
            slime_rt::debug_write(b"[fabric] malformed ack rejected\n");
            continue;
        }
        // An ack releases the sample at the head of this subscriber's ring and
        // must name it: a subscriber cannot free a slot it never consumed.
        let Some(entry) = subscriber.history.peek() else {
            slime_rt::debug_write(b"[fabric] unmatched ack rejected\n");
            continue;
        };
        if entry.sequence != ack.sequence || subscriber.in_flight == 0 {
            // A sample evicted while it was in flight is already gone from the
            // ring, so its ack arrives naming a sequence the fabric no longer
            // holds. That is the declared BEST_EFFORT outcome, not a protocol
            // error: the subscriber is told about the gap through a
            // `SAMPLE_LOST` event. Only an ack for a sample *newer* than the
            // head — one that was never sent — is a real violation.
            if ack.sequence < entry.sequence {
                continue;
            }
            slime_rt::debug_write(b"[fabric] unmatched ack rejected\n");
            continue;
        }
        subscriber.history.pop();
        subscriber.in_flight -= 1;
        subscriber.retry_count = 0;
        release_frame(entry.slot as usize, frames);
    }
}

/// Emit one terminal event for a finished route, once per subscriber.
fn announce_end(
    index: usize,
    route: usize,
    type_tags: &[u64; ROUTE_COUNT],
    subscribers: &mut [Option<Subscriber>; MAX_PARTICIPANTS],
) -> bool {
    let Some(subscriber) = subscribers[index].as_mut() else {
        return false;
    };
    if subscriber.route != route || subscriber.ended {
        return false;
    }
    // Everything queued must reach the subscriber before it is told the stream
    // ended, except after a declared terminal QoS transition has reclaimed the
    // queue itself. A lifespan-expired publisher is already absent from this
    // subscriber's queue, so retained history cannot keep it waiting.
    if !subscriber.terminal && (!subscriber.history.is_empty() || subscriber.in_flight != 0) {
        return false;
    }
    let event = WireStreamEvent {
        magic: STREAM_EVENT_MAGIC,
        version: FORMAT_VERSION,
        event: EVENT_STREAM_END,
        flags: 0,
        lost: 0,
        sequence: 0,
        type_identity: type_tags[route],
        reserved: [0; 24],
    };
    match slime_rt::send(subscriber.slot, &event.encode(), &[]) {
        ERR_SUCCESS => {
            subscriber.ended = true;
            true
        }
        ERR_WOULDBLOCK => false,
        ERR_PEER_DEAD => {
            subscriber.ended = true;
            true
        }
        _ => fail(b"stream end event"),
    }
}

/// Drop the durable-history references once the broker is finished. Retained
/// samples are live only for this fabric instance; shutdown releases their
/// fixed frame and buffer charges before the component exits.
fn release_retained(
    publishers: &mut [Option<Publisher>; MAX_PARTICIPANTS],
    frames: &mut [Frame; MAX_FRAMES],
) {
    for publisher in publishers.iter_mut().flatten() {
        while let Some(entry) = publisher.retained.pop() {
            release_frame(entry.slot as usize, frames);
        }
    }
}

/// Whether every publisher declared on `route` has finished or died.
fn route_finished(route: usize, publishers: &[Option<Publisher>; MAX_PARTICIPANTS]) -> bool {
    publishers
        .iter()
        .flatten()
        .filter(|publisher| publisher.route == route)
        .all(|publisher| publisher.finished)
}

/// Drop one reference to a fabric frame, releasing its storage at zero.
fn release_frame(frame: usize, frames: &mut [Frame; MAX_FRAMES]) {
    if frames[frame].refs == 0 {
        return;
    }
    frames[frame].refs -= 1;
    if frames[frame].refs != 0 {
        return;
    }
    if let Some(buffer_slot) = frames[frame].buffer_slot {
        // Release the fabric's own copy. Pages stay retained by the kernel
        // while any downstream loan is outstanding, so a subscriber still
        // mapping this sample keeps working and the charge settles when it
        // returns its loan.
        let _ = slime_rt::shared_buffer_release(buffer_slot);
    }
    frames[frame] = Frame::EMPTY;
}

/// Release everything a departing subscriber held, then remove it.
///
/// Frames first, so a dead peer cannot retain fabric storage, then the three
/// capability slots it occupied. Dropping those matters: the fabric's table is
/// the kernel's fixed 64 entries, and a boot that retires and re-provisions
/// subscribers would otherwise exhaust it while every retired slot named a
/// dead endpoint.
fn retire_subscriber(
    index: usize,
    subscribers: &mut [Option<Subscriber>; MAX_PARTICIPANTS],
    frames: &mut [Frame; MAX_FRAMES],
) {
    let Some(subscriber) = subscribers[index].as_mut() else {
        return;
    };
    while let Some(entry) = subscriber.history.pop() {
        release_frame(entry.slot as usize, frames);
    }
    for slot in [
        subscriber.slot,
        subscriber.ack_slot,
        subscriber.supervision_slot,
    ] {
        let _ = slime_rt::cap_drop(slot);
    }
    slime_rt::debug_write(b"[fabric] subscriber retired\n");
    subscribers[index] = None;
}

/// Drop any capability that arrived on a message that had no business carrying
/// one, so a malformed peer cannot strand kernel objects in the fabric.
fn release_received(received: &[u64]) {
    for slot in received.iter().filter(|slot| **slot != 0) {
        let _ = slime_rt::cap_drop(*slot as u32);
    }
}

/// Park across every unanswered control endpoint at once.
fn park_on_controls(clients: &[Client]) {
    let mut sources = [WaitSource::Endpoint(0); slime_rt::MAX_WAIT_SOURCES];
    let mut count = 0;
    for client in clients.iter().filter(|client| !client.answered) {
        if count == sources.len() {
            break;
        }
        sources[count] = WaitSource::Endpoint(client.control_slot);
        count += 1;
    }
    if count != 0 {
        slime_rt::wait(&sources[..count]);
    }
}

/// Park across every live stream source at once.
///
/// Finished publishers and retired subscribers are excluded: a dead source is
/// always ready, so leaving one in the set would turn this park into a spin.
///
/// The set must be complete. C8.2 admission bounds `ingressSources` against
/// `MAX_WAIT_SOURCES`, but that counts publishers only — a graph declaring many
/// subscribers as well can still exceed one park. Silently truncating would
/// drop exactly the ack sources the broker needs to make progress and hang the
/// fabric with work pending, so an over-wide set fails closed instead. Bounded
/// route workers are the C8.5 answer if a real profile needs them.
fn park_on_streams(
    publishers: &[Option<Publisher>; MAX_PARTICIPANTS],
    subscribers: &[Option<Subscriber>; MAX_PARTICIPANTS],
) {
    let mut sources = [WaitSource::Endpoint(0); slime_rt::MAX_WAIT_SOURCES];
    let mut count = 0;
    let mut push = |slot: u32| {
        if count == sources.len() {
            fail(b"live stream sources exceed one SYS_WAIT set");
        }
        sources[count] = WaitSource::Endpoint(slot);
        count += 1;
    };
    for publisher in publishers.iter().flatten() {
        if publisher.finished {
            continue;
        }
        push(publisher.slot);
    }
    for subscriber in subscribers.iter().flatten() {
        // The ack channel, not the data channel: the fabric only ever sends on
        // the data endpoint, so waiting there would never wake. A subscriber
        // becomes interesting when it releases a slot or dies.
        push(subscriber.ack_slot);
    }
    if option_env!("SLIME_FABRIC_QOS_CHECK") == Some("1") {
        push(TIME_SLOT);
    }
    if count == 0 {
        fail(b"no live stream source to park on");
    }
    slime_rt::wait(&sources[..count]);
}

/// Index of `name` in [`ROUTE_NAMES`].
fn route_index(name: &str) -> Option<usize> {
    ROUTE_NAMES.iter().position(|route| *route == name)
}

/// How many edges on a route *this service carries* the generation declares for
/// `component`. Zero is a denial: authority is never ambient, so absence from
/// the table is not a default role — and a component declared only on a call or
/// operation route holds no stream authority either.
fn declared_edges(component: &[u8]) -> usize {
    FABRIC_PARTICIPANTS
        .iter()
        .filter(|(name, route, _, _)| *name == component && route_index(route).is_some())
        .count()
}

/// The KEEP_LAST depth the generation declared for one participant on one
/// route. The graph validated it against the per-graph history ceiling before
/// launch, so a missing entry is a build-time inconsistency rather than a
/// runtime condition.
fn declared_history_depth(component: &[u8], route: &str) -> usize {
    FABRIC_HISTORY_DEPTHS
        .iter()
        .find(|(name, entry_route, _)| *name == component && *entry_route == route)
        .map(|(_, _, depth)| *depth as usize)
        .unwrap_or_else(|| fail(b"participant declares no history depth"))
}

fn declared_qos(component: &[u8], route: &str) -> TransportQos {
    FABRIC_QOS
        .iter()
        .find(|entry| entry.0 == component && entry.1 == route)
        .map(|entry| TransportQos {
            deadline_ns: entry.2,
            lifespan_ns: entry.3,
            lease_ns: entry.4,
            history_depth: entry.5,
            retained_depth: entry.6,
            reliability: entry.7,
            durability: entry.8,
            liveliness: entry.9,
        })
        .unwrap_or_else(|| fail(b"participant declares no QoS"))
}

fn refresh_matches(
    route: usize,
    publishers: &[Option<Publisher>; MAX_PARTICIPANTS],
    subscribers: &mut [Option<Subscriber>; MAX_PARTICIPANTS],
) {
    for subscriber in subscribers
        .iter_mut()
        .flatten()
        .filter(|subscriber| subscriber.route == route)
    {
        let old = subscriber.matched_publishers;
        let matched = publishers
            .iter()
            .flatten()
            .filter(|publisher| publisher.route == route)
            .filter(|publisher| TransportQos::offer_satisfies(&publisher.qos, &subscriber.qos))
            .count() as u32;
        let incompatible = publishers
            .iter()
            .flatten()
            .filter(|publisher| publisher.route == route)
            .count() as u32
            - matched;
        subscriber.matched_publishers = matched;
        if matched != old {
            let event = if matched == 0 {
                EVENT_UNMATCHED
            } else {
                EVENT_MATCHED
            };
            if send_qos_event(
                subscriber.slot,
                event,
                0,
                matched as u64,
                0,
                route_type_tag(route),
            ) {
                slime_rt::debug_write(if event == EVENT_MATCHED {
                    b"[fabric] QoS matched\n" as &[u8]
                } else {
                    b"[fabric] QoS unmatched\n"
                });
            }
        }
        if incompatible != 0
            && send_qos_event(
                subscriber.slot,
                EVENT_INCOMPATIBLE_QOS,
                0,
                incompatible as u64,
                0,
                route_type_tag(route),
            )
        {
            slime_rt::debug_write(b"[fabric] QoS incompatible\n");
        }
    }
}

fn receive_time(pending_time: &mut Option<u64>) {
    if pending_time.is_some() {
        return;
    }
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    let length = match slime_rt::recv(TIME_SLOT, &mut bytes, &mut caps) {
        ERR_WOULDBLOCK | ERR_PEER_DEAD => return,
        n if n < 0 => fail(b"time recv"),
        n => n as usize,
    };
    release_received(&caps);
    let Some(value) = WireTimeAdvance::decode(&bytes[..length]) else {
        fail(b"time decode")
    };
    if !slime_proto::valid_time_advance(&value) {
        fail(b"non-monotonic time")
    }
    *pending_time = Some(value.now_ns);
}

fn time_peer_dead() -> bool {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    match slime_rt::recv(TIME_SLOT, &mut bytes, &mut caps) {
        ERR_PEER_DEAD => true,
        ERR_WOULDBLOCK => false,
        n if n < 0 => fail(b"time peer probe"),
        _ => fail(b"unapplied time advance"),
    }
}

fn apply_time(
    now_ns: &mut u64,
    pending_time: &mut Option<u64>,
    publishers: &mut [Option<Publisher>; MAX_PARTICIPANTS],
    subscribers: &mut [Option<Subscriber>; MAX_PARTICIPANTS],
    frames: &mut [Frame; MAX_FRAMES],
) -> bool {
    let Some(next) = pending_time.take() else {
        return false;
    };
    if next < *now_ns {
        fail(b"non-monotonic time")
    }
    *now_ns = next;

    // Tie order after the broker's data/ack sweep: lifespan, retry exhaustion,
    // deadline, then liveliness/lease.
    for subscriber in subscribers.iter_mut().flatten() {
        while let Some(entry) = subscriber.history.peek() {
            let frame = entry.slot as usize;
            if subscriber.qos.lifespan_ns == 0
                || now_ns.saturating_sub(frames[frame].admitted_ns) < subscriber.qos.lifespan_ns
            {
                break;
            }
            let expired = subscriber.history.pop().expect("queued frame");
            subscriber.in_flight = subscriber.in_flight.saturating_sub(1);
            let publisher_index = expired.publisher as usize;
            if publishers.get(publisher_index).is_none_or(Option::is_none) {
                fail(b"expired sample has no publisher");
            }
            release_frame(expired.slot as usize, frames);
            if send_qos_event(
                subscriber.slot,
                EVENT_LIFESPAN_EXPIRED,
                expired.sequence,
                0,
                *now_ns,
                route_type_tag(subscriber.route),
            ) {
                slime_rt::debug_write(b"[fabric] QoS lifespan expired\n");
            }
        }
    }

    for subscriber in subscribers.iter_mut().flatten() {
        if subscriber.terminal
            || subscriber.qos.reliability as u32 != RELIABILITY_RELIABLE
            || subscriber.in_flight == 0
        {
            continue;
        }
        if now_ns.saturating_sub(subscriber.last_retry_ns) < subscriber.retry_interval_ns {
            continue;
        }
        subscriber.retry_count = subscriber.retry_count.saturating_add(1);
        subscriber.last_retry_ns = *now_ns;
        slime_rt::debug_write(b"[fabric] reliable retry accounted\n");
        if subscriber.retry_count < 4 {
            continue;
        }

        let mut exhausted = None;
        while let Some(entry) = subscriber.history.pop() {
            exhausted.get_or_insert(entry.sequence);
            release_frame(entry.slot as usize, frames);
        }
        subscriber.in_flight = 0;
        subscriber.terminal = true;
        if let Some(sequence) = exhausted
            && send_qos_event(
                subscriber.slot,
                EVENT_RETRY_EXHAUSTED,
                sequence,
                subscriber.retry_count as u64,
                *now_ns,
                route_type_tag(subscriber.route),
            )
        {
            slime_rt::debug_write(b"[fabric] QoS retry exhausted\n");
        }
    }

    for subscriber in subscribers.iter_mut().flatten() {
        if subscriber.qos.deadline_ns != 0
            && !subscriber.deadline_reported
            && *now_ns >= subscriber.qos.deadline_ns
        {
            subscriber.deadline_reported = true;
            if send_qos_event(
                subscriber.slot,
                EVENT_DEADLINE_MISSED,
                0,
                0,
                *now_ns,
                route_type_tag(subscriber.route),
            ) {
                slime_rt::debug_write(b"[fabric] QoS deadline missed\n");
            }
        }
    }

    for publisher in publishers.iter().flatten() {
        if publisher.qos.lease_ns != 0
            && now_ns.saturating_sub(publisher.last_assertion_ns) >= publisher.qos.lease_ns
        {
            for subscriber in subscribers.iter_mut().flatten().filter(|subscriber| {
                subscriber.route == publisher.route && !subscriber.liveliness_reported
            }) {
                subscriber.liveliness_reported = true;
                if send_qos_event(
                    subscriber.slot,
                    EVENT_LIVELINESS_LOST,
                    0,
                    0,
                    *now_ns,
                    route_type_tag(subscriber.route),
                ) {
                    slime_rt::debug_write(b"[fabric] QoS liveliness lost\n");
                }
            }
        }
    }
    let credit = WireTimeAdvance {
        magic: slime_proto::fabric_time::TIME_ADVANCE_MAGIC,
        version: slime_proto::fabric_time::FORMAT_VERSION,
        flags: 0,
        reserved0: 0,
        now_ns: *now_ns,
        reserved: [0; 40],
    };
    match slime_rt::send(TIME_SLOT, &credit.encode(), &[]) {
        ERR_SUCCESS => {}
        ERR_WOULDBLOCK | ERR_PEER_DEAD => fail(b"time credit blocked"),
        _ => fail(b"time credit"),
    }
    true
}

fn route_type_tag(route: usize) -> u64 {
    match route {
        0 => telemetry_stream::TYPE_TAG,
        1 => diagnostics_stream::TYPE_TAG,
        _ => fail(b"route tag"),
    }
}

fn send_qos_event(
    slot: u32,
    event: u32,
    sequence: u64,
    value: u64,
    timestamp_ns: u64,
    type_identity: u64,
) -> bool {
    let record = WireQosEvent {
        magic: QOS_EVENT_MAGIC,
        version: QOS_FORMAT_VERSION,
        event,
        flags: 0,
        sequence,
        value,
        timestamp_ns,
        type_identity,
        reserved: [0; 16],
    };
    match slime_rt::send(slot, &record.encode(), &[]) {
        ERR_SUCCESS => true,
        ERR_WOULDBLOCK | ERR_PEER_DEAD => false,
        _ => fail(b"QoS event"),
    }
}

/// The supervision handle init granted the fabric for one subscriber. Init
/// spawns each client and hands the fabric its supervision capability, so the
/// fabric can name a loan receiver by capability rather than by task id.
fn supervision_slot_for(component: &[u8]) -> u32 {
    FABRIC_SUPERVISION
        .iter()
        .find(|(name, _)| *name == component)
        .map(|(_, slot)| *slot)
        .unwrap_or_else(|| fail(b"subscriber has no supervision handle"))
}

/// Answer a request the graph does not authorize. A denial is the same record
/// with a nonzero status, an empty rights mask, and no capability attached: the
/// caller learns it was refused without learning anything about the route.
fn deny(control_slot: u32, route: &[u8; 32], status: i32) {
    let descriptor = WireCapabilityTransfer {
        magic: CAPABILITY_TRANSFER_MAGIC,
        version: FORMAT_VERSION,
        status,
        flags: 0,
        object_kind: 0,
        direction: 0,
        rights_mask: 0,
        route_identity: *route,
    };
    let encoded = descriptor.encode();
    loop {
        match slime_rt::send(control_slot, &encoded, &[]) {
            ERR_SUCCESS => return,
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(control_slot)]),
            _ => fail(b"deny reply"),
        }
    }
}

const _: () = assert!(REQUEST_LEN == MAX_MSG);
const _: () = assert!(TRANSFER_LEN == MAX_MSG);
const _: () = assert!(slime_proto::fabric_stream::SAMPLE_LEN == MAX_MSG);
const _: () = assert!(slime_proto::fabric_stream::ACK_LEN == MAX_MSG);
const _: () = assert!(slime_proto::fabric_stream::EVENT_LEN == MAX_MSG);
const _: () = assert!(FABRIC_MAX_SAMPLE_BYTES <= COPY_PAGES * PAGE as usize);
const _: () = assert!(FABRIC_MAX_BUFFER_PAGES <= FABRIC_MAX_BUFFERS * COPY_PAGES);
const _: () = assert!(FABRIC_REQUIRED_CAPABILITY_SLOTS <= FABRIC_MAX_CAPABILITY_SLOTS);
// The peak the generation resolved for the stream worker must fit one `SYS_WAIT`
// set. `park_on_streams` already fails closed if the live set overruns its array,
// but that is a boot-time failure on a graph the build could have refused: the
// resolver rejects an over-wide partition, and this pins that same number here so
// the two cannot disagree.
const _: () = assert!(fabric_worker_wait_sources("stream") <= slime_rt::MAX_WAIT_SOURCES);

// The frame table must cover every reference the declared rings can hold at
// once, or a full set of rings would leave the fabric with no free frame while
// its publishers block. Admission (C8.2) bounds each subscriber's declared
// `history_depth` by the graph's own `historyDepth` limit, which the contract
// caps at `LIMIT_HISTORY_DEPTH`, so the absolute worst case a generation could
// declare is larger than this table.
//
// That ceiling is a real limit rather than an oversight: it is why `admit_*`
// refuses a sample when no frame is free and settles the publisher's loan
// instead of blocking. Refusing is bounded and reported; deadlocking is not.
// This assertion pins the property the table does guarantee — every ring the
// *declared* graph asks for fits at once — so a manifest that outgrows it fails
// the build rather than a boot.
const _: () = assert!(
    MAX_FRAMES >= DECLARED_RING_CAPACITY,
    "frame table smaller than the rings this generation declares"
);

/// Summed KEEP_LAST depth of every subscriber the generation declares. Derived
/// from the same build-time tables the fabric sizes each ring from, so the two
/// cannot disagree.
const DECLARED_RING_CAPACITY: usize = {
    let mut total = 0;
    let mut index = 0;
    while index < FABRIC_PARTICIPANTS.len() {
        // The two tables are positionally paired by `build.rs`, which asserts
        // their lengths match. Direction 2 is `DIRECTION_SUBSCRIBE`.
        if FABRIC_PARTICIPANTS[index].3 == 2 {
            total += FABRIC_HISTORY_DEPTHS[index].2 as usize;
        }
        index += 1;
    }
    total
};
