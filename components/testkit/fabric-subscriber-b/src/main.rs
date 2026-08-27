#![no_std]
#![no_main]

//! C8.4 second subscriber: the stalled BEST_EFFORT reader, on two routes.
//!
//! This component makes the two properties a keeping-up subscriber cannot show
//! observable:
//!
//! 1. **A stall is bounded and reported.** It deliberately stops acking on
//!    `telemetry` while its publishers keep going. The fabric's KEEP_LAST ring
//!    fills at the declared depth and evicts the oldest sequence for each
//!    newer one, so the stall costs a fixed number of entries however long it
//!    lasts. When this component resumes, the fabric reports exactly one
//!    `SAMPLE_LOST` event naming the count and the oldest sequence lost — a
//!    report, not a retry.
//! 2. **Routes stay separate under fault.** It also subscribes to
//!    `diagnostics`, which has its own publisher. The telemetry stall must not
//!    disturb it: the diagnostics sample arrives and verifies regardless of
//!    what telemetry is doing.
//!
//! Its two roles arrive over one control endpoint and are matched by the route
//! identity each descriptor carries, never by arrival order.

use boot_contracts::fabric_graph::{CONTRACT_KIND_STREAM, DIRECTION_SUBSCRIBE, route_identity};
use slime_proto::capability_transfer::{
    FABRIC_REQUEST_MAGIC, FORMAT_VERSION, OBJECT_KIND_ENDPOINT, OBJECT_KIND_SHARED_BUFFER_LOAN,
    REQUEST_LEN, WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::fabric_qos::{QOS_EVENT_MAGIC, WireQosEvent};
use slime_proto::fabric_stream::{
    EVENT_SAMPLE_LOST, EVENT_STREAM_END, MAX_INLINE_BYTES, STREAM_ACK_MAGIC, STREAM_EVENT_MAGIC,
    WireStreamAck, WireStreamEvent, WireStreamSample,
};
use slime_proto::interface_schema::{diagnostics_stream, telemetry_stream};
use slime_proto::ring::{Ring, RingError};
use slime_proto::sample_descriptor::{SAMPLE_DESCRIPTOR_MAGIC, WireSampleDescriptor};
use slime_proto::{
    valid_capability_transfer, valid_sample_descriptor, valid_stream_event, valid_stream_sample,
};
use slime_rt::{ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};
// B59: the capability-rights vocabulary is generated from
// `contracts/generation/v5/schema.zt`; these were local copies of the same
// bit numbering.
use boot_contracts::generation::{BootAction, RIGHT_RECV, RIGHT_SEND};

// C8.13.2: this participant's own shared-buffer occupancy evidence. Included
// here rather than through `slime_components` because a file may be a module
// only once per crate.
#[path = "../../../lib/src/fabric_trace_log.rs"]
mod trace_log;

#[path = "../../../lib/src/fabric_occupancy_trace.rs"]
mod occupancy_trace;

slime_rt::entry!(main);

const CONTROL_SLOT: u32 = 0;

const TELEMETRY_ROUTE: &str = "telemetry";
const DIAGNOSTICS_ROUTE: &str = "diagnostics";
/// C8.12's alternate name over `TelemetryStream`. A distinct route from
/// `TELEMETRY_ROUTE`, because route authority folds the name into the identity.
const MATRIX_ALT_ROUTE: &str = "telemetry-alt";

/// The visibility plane's declared diagnostics edges: egress in, ack out.
/// Generation facts, installed before this component runs.
const DIAGNOSTICS_EGRESS_SLOT: u32 = 1;
const DIAGNOSTICS_ACK_SLOT: u32 = 2;

const PAGE: u64 = 4096;
const BASE: u64 = 0x0000_000F_0000_0000;
/// One page per ring, and the two bases a page apart would overlap exactly:
/// each ring *is* `RING_BYTES`, so the second mapping would land on the first
/// and the ring attached second would read the other route's header.
const TELEMETRY_RING_BASE: u64 = 0x0000_0013_0000_0000;
const DIAGNOSTICS_RING_BASE: u64 = 0x0000_0014_0000_0000;
const RING_BYTES: usize = 4096;

/// Bounds on what a stall may cost this subscriber.
///
/// The telemetry publishers send a fixed, known number of samples between them
/// — `fabric-publisher` its inline set plus its stall-window burst and a
/// terminal sample, `fabric-publisher-b` one large one — so neither the total
/// lost nor the number of reports can exceed that. A fabric that retried
/// instead of reporting, or reported per delivery attempt, blows past both.
const MAX_TOTAL_LOSS: u64 = 16;
const MAX_LOSS_REPORTS: u32 = 16;

/// One generation-provisioned v2 ring for a participant-to-fabric edge.
#[derive(Default)]
struct RouteRing {
    slot: Option<u32>,
}

/// This participant's declared ring depth for `route`, as the generation
/// resolved it.
///
/// The fabric formats each ring at exactly this depth, and `Ring::attach`
/// checks the header's slot count against what the caller expects — so a
/// hardcoded constant here is a disagreement waiting to happen. Floored at
/// `MIN_RING_SLOTS` exactly as the fabric floors it.
fn ring_slots(route: &str) -> usize {
    // The interface is a property of the route, not of this component: the two
    // routes it carries fold different interface identities into their route
    // identity, so picking the wrong one resolves nothing rather than resolving
    // the other route.
    let identity = if route == DIAGNOSTICS_ROUTE {
        route_identity(
            route,
            &diagnostics_stream::INTERFACE_IDENTITY,
            CONTRACT_KIND_STREAM,
        )
    } else {
        route_identity(
            route,
            &telemetry_stream::INTERFACE_IDENTITY,
            CONTRACT_KIND_STREAM,
        )
    };
    ring_slots_checked(&identity)
}

/// This component's declared KEEP_LAST depth, read from the graph.
///
/// Carried a cross-check against `FABRIC_HISTORY_DEPTHS` while both statements
/// of the depth existed; the table is gone, so agreement is no longer something
/// to assert -- there is one source (B70/CP2).
fn ring_slots_checked(identity: &[u8; 32]) -> usize {
    slime_components::fabric_self_view::ring_slots(identity)
        .unwrap_or_else(|| fail(b"route declares no history depth"))
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-subscriber-b] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn main(_startup_arg: u32) {
    if slime_components::generation_composition::is(BootAction::Visibility) {
        visibility_main();
        return;
    }
    if slime_components::fabric_matrix::active() {
        matrix_main();
        return;
    }
    let telemetry_route = route_identity(
        TELEMETRY_ROUTE,
        &telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    let diagnostics_route = route_identity(
        DIAGNOSTICS_ROUTE,
        &diagnostics_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    if slime_components::fabric_boot::active() {
        slime_components::fabric_boot::provision_multi_and_park(
            b"fabric-subscriber-b",
            TELEMETRY_ROUTE,
            telemetry_stream::TYPE_TAG,
            DIRECTION_SUBSCRIBE,
            &[
                (telemetry_route, DIRECTION_SUBSCRIBE, 1),
                (diagnostics_route, DIRECTION_SUBSCRIBE, 1),
            ],
        );
    }

    if request_roles() != ERR_SUCCESS {
        fail(b"request");
    }
    slime_rt::debug_write(b"[fabric-subscriber-b] roles requested\n");

    let mut telemetry = RouteRing::default();
    let mut diagnostics = RouteRing::default();
    for _ in 0..2 {
        let (descriptor, slot) = receive_role();
        if descriptor.status != 0 {
            fail(b"declared subscriber was denied");
        }
        let ring = if valid_capability_transfer(
            &descriptor,
            &telemetry_route,
            DIRECTION_SUBSCRIBE,
            OBJECT_KIND_SHARED_BUFFER_LOAN,
        ) {
            &mut telemetry
        } else if valid_capability_transfer(
            &descriptor,
            &diagnostics_route,
            DIRECTION_SUBSCRIBE,
            OBJECT_KIND_SHARED_BUFFER_LOAN,
        ) {
            &mut diagnostics
        } else {
            fail(b"role names no declared route");
        };
        if ring.slot.replace(slot).is_some() {
            fail(b"duplicate route ring");
        }
    }
    let Some(telemetry_slot) = telemetry.slot else {
        fail(b"telemetry ring missing");
    };
    let Some(diagnostics_slot) = diagnostics.slot else {
        fail(b"diagnostics ring missing");
    };
    if telemetry_slot == diagnostics_slot {
        fail(b"two routes arrived as one ring");
    }
    if slime_rt::shared_buffer_loan_map(telemetry_slot, TELEMETRY_RING_BASE, 0, RING_BYTES as u64)
        != ERR_SUCCESS
        || slime_rt::shared_buffer_loan_map(
            diagnostics_slot,
            DIAGNOSTICS_RING_BASE,
            0,
            RING_BYTES as u64,
        ) != ERR_SUCCESS
    {
        fail(b"subscriber ring map");
    }
    let telemetry_bytes =
        unsafe { core::slice::from_raw_parts_mut(TELEMETRY_RING_BASE as *mut u8, RING_BYTES) };
    let diagnostics_bytes =
        unsafe { core::slice::from_raw_parts_mut(DIAGNOSTICS_RING_BASE as *mut u8, RING_BYTES) };
    let mut telemetry_ring = Ring::attach(
        telemetry_bytes,
        telemetry_stream::TYPE_TAG,
        ring_slots(TELEMETRY_ROUTE),
    )
    .unwrap_or_else(|_| fail(b"telemetry ring attach"));
    let mut diagnostics_ring = Ring::attach(
        diagnostics_bytes,
        diagnostics_stream::TYPE_TAG,
        ring_slots(DIAGNOSTICS_ROUTE),
    )
    .unwrap_or_else(|_| fail(b"diagnostics ring attach"));
    slime_rt::debug_write(b"[fabric-subscriber-b] both subscribe rings received\n");

    slime_rt::debug_write(b"[fabric-subscriber-b] stalling on telemetry\n");
    let early = receive_large_sample();
    if slime_components::generation_composition::is(BootAction::Qos) {
        consume_diagnostics(&mut diagnostics_ring);
    } else {
        consume_diagnostics_stream(&mut diagnostics_ring);
    }
    consume_telemetry(&mut telemetry_ring, early);
    // C8.13.2: gated to the traffic plane, so standalone compositions do not
    // receive occupancy records they did not request. Two rings remain mapped,
    // one per declared route.
    if slime_components::generation_composition::is(BootAction::Traffic) {
        occupancy_trace::report(b"subscriber-b").unwrap_or_else(|_| fail(b"runtime trace depth"));
    }
    slime_rt::debug_write(b"[fabric-subscriber-b] done\n");
}
fn visibility_main() {
    // Two generation-declared edges: the diagnostics egress this component
    // receives on, and the ack it answers with. The broker's replies name them
    // rather than carrying them, so there is nothing to import.
    let route = route_identity(
        DIAGNOSTICS_ROUTE,
        &diagnostics_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    if request_roles() != ERR_SUCCESS {
        fail(b"visibility request");
    }
    let data_descriptor = receive_declared_role();
    let ack_descriptor = receive_declared_role();
    if data_descriptor.rights_mask != RIGHT_RECV
        || ack_descriptor.rights_mask != RIGHT_SEND
        || !valid_capability_transfer(
            &data_descriptor,
            &route,
            DIRECTION_SUBSCRIBE,
            OBJECT_KIND_ENDPOINT,
        )
        || !valid_capability_transfer(
            &ack_descriptor,
            &route,
            DIRECTION_SUBSCRIBE,
            OBJECT_KIND_ENDPOINT,
        )
    {
        fail(b"visibility diagnostics role");
    }
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    let length = loop {
        match slime_rt::recv(DIAGNOSTICS_EGRESS_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => fail(b"visibility diagnostics receive"),
            n => break n as usize,
        }
    };
    if length != MAX_MSG || received.iter().any(|slot| *slot != 0) {
        fail(b"visibility diagnostics framing");
    }
    let sample = WireStreamSample::decode(&message)
        .filter(|sample| {
            valid_stream_sample(sample, diagnostics_stream::TYPE_TAG, MAX_INLINE_BYTES)
        })
        .filter(|sample| sample.sequence == 1)
        .unwrap_or_else(|| fail(b"visibility diagnostics sample"));
    ack(
        DIAGNOSTICS_ACK_SLOT,
        sample.sequence,
        diagnostics_stream::TYPE_TAG,
    );
    slime_rt::debug_write(b"[fabric-subscriber-b] unrelated diagnostics live after proxy death\n");
}

/// C8.12: the alternate-name route's subscriber.
///
/// It holds edges on `telemetry-alt` and `diagnostics` and none on `telemetry`.
/// Asking under the name it does not hold is refused; asking under its own is
/// matched — which is the same distinction `fabric-publisher-b` proves from the
/// publishing side, and both are needed: a broker keyed on direction alone
/// would pass one and fail the other.
fn matrix_main() {
    use slime_components::fabric_matrix::{Outcome, request_role};

    match request_role(
        TELEMETRY_ROUTE,
        telemetry_stream::TYPE_TAG,
        DIRECTION_SUBSCRIBE,
    ) {
        Ok(Outcome::Denied(_)) => {
            slime_rt::debug_write(b"[fabric-subscriber-b] alternate name denied\n");
        }
        Ok(Outcome::Role(_)) => fail(b"a route this component holds no edge on was granted"),
        Err(_) => fail(b"matrix alternate-name request"),
    }

    let alt = route_identity(
        MATRIX_ALT_ROUTE,
        &telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    match request_role(
        MATRIX_ALT_ROUTE,
        telemetry_stream::TYPE_TAG,
        DIRECTION_SUBSCRIBE,
    ) {
        Ok(Outcome::Role(descriptor)) => {
            if descriptor.rights_mask != RIGHT_RECV
                || !valid_capability_transfer(
                    &descriptor,
                    &alt,
                    DIRECTION_SUBSCRIBE,
                    OBJECT_KIND_ENDPOINT,
                )
            {
                fail(b"matrix alternate-route role");
            }
        }
        Ok(Outcome::Denied(_)) => fail(b"the exact compatible tuple was denied"),
        Err(_) => fail(b"matrix alternate-route request"),
    }
    slime_rt::debug_write(b"[fabric-subscriber-b] matrix alternate route matched\n");
}

/// A role reply that carries no capability.
///
/// The visibility broker answers with the descriptor alone, so unlike
/// [`receive_role`] there is nothing to import.
fn receive_declared_role() -> WireCapabilityTransfer {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => fail(b"visibility role reply"),
            _ => {
                let Some(descriptor) = WireCapabilityTransfer::decode(&message).filter(|record| {
                    record.magic == slime_proto::capability_transfer::CAPABILITY_TRANSFER_MAGIC
                }) else {
                    continue;
                };
                if descriptor.status != 0 {
                    fail(b"visibility diagnostics role");
                }
                return descriptor;
            }
        }
    }
}

/// Acknowledge one sample on a declared ack edge.
fn ack(ack_slot: u32, sequence: u64, type_identity: u64) {
    let ack = WireStreamAck {
        magic: STREAM_ACK_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        reserved0: 0,
        sequence,
        type_identity,
        reserved: [0; 32],
    };
    let encoded = ack.encode();
    loop {
        match slime_rt::send(ack_slot, &encoded, &[]) {
            ERR_SUCCESS => return,
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            _ => fail(b"ack"),
        }
    }
}

/// This component's four notification slots, resolved through the root by the
/// grant names the generation declares (CP2/B70).
///
/// Telemetry takes two names because the compositions disagree: the matrix plane
/// declares `telemetry-alt` where the stream, QoS, visibility, boot, and traffic
/// planes declare `telemetry`. No generation declares both for one holder --
/// verified against every manifest that declares notifications -- so this is a
/// disjoint lookup rather than a precedence rule, the same shape
/// `init.rs::console_send_slot` uses for its edge.
fn telemetry_slot(suffix: &[u8]) -> u32 {
    for stem in [
        b"notification:fabric-subscriber-b-telemetry-".as_slice(),
        b"notification:fabric-subscriber-b-telemetry-alt-".as_slice(),
    ] {
        let mut name = [0u8; 64];
        let len = stem.len() + suffix.len();
        if len > name.len() {
            fail(b"notification name exceeds the query bound");
        }
        name[..stem.len()].copy_from_slice(stem);
        name[stem.len()..len].copy_from_slice(suffix);
        if let Ok(slot) = slime_rt::resolve_binding(&name[..len]) {
            return slot;
        }
    }
    fail(b"no telemetry notification in this generation")
}

fn diagnostics_slot(suffix: &[u8]) -> u32 {
    let stem = b"notification:fabric-subscriber-b-diagnostics-";
    let mut name = [0u8; 64];
    let len = stem.len() + suffix.len();
    if len > name.len() {
        fail(b"notification name exceeds the query bound");
    }
    name[..stem.len()].copy_from_slice(stem);
    name[stem.len()..len].copy_from_slice(suffix);
    slime_rt::resolve_binding(&name[..len])
        .unwrap_or_else(|_| fail(b"no diagnostics notification in this generation"))
}

fn consume_diagnostics_stream(ring: &mut Ring<'_>) {
    let mut sample_seen = false;
    loop {
        let _ = slime_rt::notification_poll(diagnostics_slot(b"ready"));
        let mut payload = [0u8; MAX_INLINE_BYTES];
        loop {
            match ring.consume(&mut payload) {
                Ok((length, _last)) => {
                    if length == 0 {
                        fail(b"empty diagnostics sample");
                    }
                    sample_seen = true;
                    let _ = slime_rt::notification_signal(diagnostics_slot(b"credit"));
                }
                Err(RingError::Empty) => break,
                Err(_) => fail(b"diagnostics ring consume"),
            }
        }
        // Nothing else to do this pass: the ring is drained, so block and
        // become visible to the fabric's terminal `nb_send`.
        let Some(message) = receive_for(diagnostics_stream::TYPE_TAG, false) else {
            continue;
        };
        let magic = u32::from_le_bytes(message[..4].try_into().expect("record prefix"));
        match magic {
            QOS_EVENT_MAGIC => {}
            STREAM_EVENT_MAGIC => {
                let event =
                    WireStreamEvent::decode(&message).unwrap_or_else(|| fail(b"decode event"));
                // Drain once more before judging the event. The ring and the
                // control endpoint are independent channels: the fabric writes
                // a sample into the ring and can emit END in the same pass, so
                // a sample published just before it is still unread here. The
                // ring is the record of what arrived, not the arrival order of
                // these two.
                loop {
                    match ring.consume(&mut payload) {
                        Ok((length, _last)) => {
                            if length == 0 {
                                fail(b"empty diagnostics sample");
                            }
                            sample_seen = true;
                        }
                        Err(RingError::Empty) => break,
                        Err(_) => fail(b"diagnostics ring consume"),
                    }
                }
                if !valid_stream_event(&event, diagnostics_stream::TYPE_TAG)
                    || event.event != EVENT_STREAM_END
                    || !sample_seen
                {
                    fail(b"unexpected diagnostics stream event");
                }
                slime_rt::debug_write(b"[fabric-subscriber-b] diagnostics unaffected by stall\n");
                return;
            }
            _ => fail(b"ordinary diagnostics sample used control endpoint"),
        }
    }
}

/// One control endpoint serves both of this component's routes, and a receive
/// is destructive: whichever loop is running takes the next record regardless
/// of which route it belongs to. Every record on this endpoint — sample
/// descriptor, stream event, QoS event — names its route in `type_identity`,
/// so the endpoint is read in exactly one place and each record is filed under
/// the route that owns it. A loop then waits on its own route's mailbox rather
/// than on the endpoint, and cannot consume another route's terminal event.
///
/// The mailbox is a small FIFO, not a single slot. A route's records are a
/// *sequence* the owning loop must see in full — the QoS plane reports deadline,
/// liveliness, and retry-exhaustion as separate events, and all three can arrive
/// while the other route's loop is running. Keeping only the newest would drop
/// the earlier ones, which reads exactly like the fabric never sent them.
///
/// Eight deep: the QoS plane can report deadline, repeated liveliness loss,
/// lifespan expiry, and retry exhaustion for one route before its owner is
/// scheduled again, and the terminal event follows all of them.
struct Pending {
    type_identity: u64,
    queue: [[u8; MAX_MSG]; 8],
    head: usize,
    len: usize,
}

impl Pending {
    const fn new(type_identity: u64) -> Self {
        Self {
            type_identity,
            queue: [[0; MAX_MSG]; 8],
            head: 0,
            len: 0,
        }
    }

    fn push(&mut self, message: [u8; MAX_MSG]) {
        // The fabric re-offers a record every broker pass until its owner takes
        // it, so the same bytes legitimately arrive many times while that owner
        // is busy on the other route. Queueing each copy would overflow on
        // repetition rather than on real traffic, so an identical record
        // already waiting is the one already waiting.
        if (0..self.len)
            .map(|offset| (self.head + offset) % self.queue.len())
            .any(|index| self.queue[index] == message)
        {
            return;
        }
        if self.len == self.queue.len() {
            fail(b"a route produced more records than its mailbox admits");
        }
        let tail = (self.head + self.len) % self.queue.len();
        self.queue[tail] = message;
        self.len += 1;
    }

    fn pop(&mut self) -> Option<[u8; MAX_MSG]> {
        if self.len == 0 {
            return None;
        }
        let message = self.queue[self.head];
        self.head = (self.head + 1) % self.queue.len();
        self.len -= 1;
        Some(message)
    }
}

/// The routes' mailboxes, in the order this component declares them.
/// SAFETY: this component is single-threaded; every access is on its one
/// execution path and no reference outlives the statement that takes it.
static mut MAILBOXES: [Pending; 2] = [
    Pending::new(telemetry_stream::TYPE_TAG),
    Pending::new(diagnostics_stream::TYPE_TAG),
];

/// Read one record for `type_identity`, filing records for the other route.
///
/// `poll` selects how the endpoint is read when this route's mailbox is empty.
/// A caller that must also make progress elsewhere — draining a ring, signalling
/// credit — passes `true` and gets `None` when nothing has arrived. A caller
/// with nothing left to do passes `false` and blocks.
///
/// Blocking is not merely an optimisation here. The fabric announces terminal
/// events with `seL4_NBSend`, which delivers only to a receiver *already*
/// blocked on the endpoint and discards otherwise. A reader that only ever
/// polls can therefore spin forever beside a sender that is faithfully
/// re-offering the record: two non-blocking peers never rendezvous. Once a
/// loop has drained its ring it has nothing else to wait on, so it blocks and
/// becomes visible to that send.
fn receive_for(type_identity: u64, poll: bool) -> Option<[u8; MAX_MSG]> {
    loop {
        // SAFETY: single-threaded; the borrow ends before the receive below.
        let mailbox = unsafe { &mut *core::ptr::addr_of_mut!(MAILBOXES) }
            .iter_mut()
            .find(|pending| pending.type_identity == type_identity)
            .unwrap_or_else(|| fail(b"record names an undeclared route"));
        if let Some(message) = mailbox.pop() {
            return Some(message);
        }

        let mut message = [0u8; MAX_MSG];
        let mut received = [0u64; MAX_CAPS_PER_MSG];
        let outcome = if poll {
            slime_rt::recv(CONTROL_SLOT, &mut message, &mut received)
        } else {
            slime_rt::recv_blocking(CONTROL_SLOT, &mut message, &mut received)
        };
        let length = match outcome {
            ERR_WOULDBLOCK => return None,
            n if n < 0 => fail(b"control recv"),
            n => n as usize,
        };
        if length != MAX_MSG {
            fail(b"record is not one control message");
        }
        let owner = record_route(&message);
        if owner == type_identity {
            return Some(message);
        }
        // SAFETY: single-threaded, as above.
        unsafe { &mut *core::ptr::addr_of_mut!(MAILBOXES) }
            .iter_mut()
            .find(|pending| pending.type_identity == owner)
            .unwrap_or_else(|| fail(b"record names an undeclared route"))
            .push(message);
    }
}

/// The route a control record belongs to, taken from the record itself.
fn record_route(message: &[u8; MAX_MSG]) -> u64 {
    let magic = u32::from_le_bytes(message[..4].try_into().expect("record prefix"));
    match magic {
        STREAM_EVENT_MAGIC => {
            WireStreamEvent::decode(message)
                .unwrap_or_else(|| fail(b"decode event"))
                .type_identity
        }
        QOS_EVENT_MAGIC => {
            WireQosEvent::decode(message)
                .unwrap_or_else(|| fail(b"decode QoS event"))
                .type_identity
        }
        SAMPLE_DESCRIPTOR_MAGIC => {
            WireSampleDescriptor::decode(message)
                .unwrap_or_else(|| fail(b"decode sample descriptor"))
                .type_identity
        }
        _ => fail(b"ordinary sample used control endpoint"),
    }
}

/// Loss the fabric reported while this component was stalled, which belongs to
/// the telemetry reader that runs after the stall rather than to the stall.
#[derive(Clone, Copy, Default)]
struct EarlyLoss {
    reports: u32,
    total: u64,
}

/// Wait for this component's large telemetry sample, which arrives as a
/// descriptor on the control endpoint naming a delegated loan.
///
/// The stall this creates is the very thing that makes the fabric drop
/// telemetry, so the loss is reported *here*, to the loop that caused it,
/// while the reader that must account for it has not started. Same route, so
/// the mailbox cannot hold it for that reader: it is handed back instead.
fn receive_large_sample() -> EarlyLoss {
    let mut early = EarlyLoss::default();
    loop {
        // Ring drained; block so a terminal `nb_send` can find a receiver.
        let Some(message) = receive_for(telemetry_stream::TYPE_TAG, false) else {
            continue;
        };
        let magic = u32::from_le_bytes(message[..4].try_into().expect("record prefix"));
        // A QoS event is advisory; loss is judged as a stream event below.
        if magic == QOS_EVENT_MAGIC {
            continue;
        }
        if magic == STREAM_EVENT_MAGIC {
            let event = WireStreamEvent::decode(&message).unwrap_or_else(|| fail(b"decode event"));
            if !valid_stream_event(&event, telemetry_stream::TYPE_TAG) {
                fail(b"event failed validation");
            }
            if event.event != EVENT_SAMPLE_LOST {
                fail(b"telemetry ended before its large sample");
            }
            if event.lost == 0 || event.sequence == 0 {
                fail(b"loss event named no loss");
            }
            early.reports += 1;
            early.total = early.total.saturating_add(event.lost);
            if early.reports > MAX_LOSS_REPORTS || early.total > MAX_TOTAL_LOSS {
                fail(b"loss reporting grew past its bound");
            }
            slime_rt::debug_write(b"[fabric-subscriber-b] bounded loss reported\n");
            continue;
        }
        if magic != SAMPLE_DESCRIPTOR_MAGIC {
            fail(b"ordinary telemetry sample used control endpoint");
        }
        let descriptor = WireSampleDescriptor::decode(&message)
            .unwrap_or_else(|| fail(b"decode sample descriptor"));
        // A delegated loan arrives as a root-recorded export, not in the
        // message: only a native Endpoint travels inline.
        let loan_slot = slime_rt::capability_import().unwrap_or(0);
        if loan_slot == 0
            || !valid_sample_descriptor(
                &descriptor,
                descriptor.loan_id,
                telemetry_stream::TYPE_TAG,
                PAGE,
            )
        {
            fail(b"descriptor failed validation");
        }
        if slime_rt::shared_buffer_loan_map(loan_slot, BASE, 0, descriptor.length) != ERR_SUCCESS {
            fail(b"loan map");
        }
        let mismatch = unsafe {
            let bytes = BASE as *const u8;
            (0..descriptor.length as usize)
                .find(|index| bytes.add(*index).read_volatile() != (*index % 251) as u8)
        };
        if mismatch.is_some() {
            fail(b"shared payload mismatch");
        }
        if slime_rt::shared_buffer_unmap(loan_slot, BASE) != ERR_SUCCESS
            || slime_rt::shared_buffer_return(loan_slot) != ERR_SUCCESS
        {
            fail(b"return loan");
        }
        slime_rt::debug_write(b"[fabric-subscriber-b] shared sample verified\n");
        return early;
    }
}

/// Consume telemetry until the route ends.
///
/// Loss is admissible throughout — this reader declares BEST_EFFORT — so it is
/// counted and bounded rather than forbidden at any point. `early` carries the
/// loss already reported during the stall, which this reader must account for
/// because it is the loss its own stall caused. Returns the number of samples
/// consumed.
fn consume_telemetry(ring: &mut Ring<'_>, early: EarlyLoss) -> u32 {
    let mut consumed = 0;
    let mut observed_loss = early.reports != 0;
    let mut reports = early.reports;
    let mut total_lost = early.total;
    loop {
        let _ = slime_rt::notification_poll(telemetry_slot(b"ready"));
        let mut payload = [0u8; MAX_INLINE_BYTES];
        loop {
            match ring.consume(&mut payload) {
                Ok((length, _last)) => {
                    if length == 0 {
                        fail(b"empty telemetry sample");
                    }
                    consumed += 1;
                    let _ = slime_rt::notification_signal(telemetry_slot(b"credit"));
                }
                Err(RingError::Empty) => break,
                Err(_) => fail(b"telemetry ring consume"),
            }
        }
        // Ring drained; block so a terminal `nb_send` can find a receiver.
        let Some(message) = receive_for(telemetry_stream::TYPE_TAG, false) else {
            continue;
        };
        let magic = u32::from_le_bytes(message[..4].try_into().expect("record prefix"));
        match magic {
            QOS_EVENT_MAGIC => {
                let event =
                    WireQosEvent::decode(&message).unwrap_or_else(|| fail(b"decode QoS event"));
                if !slime_proto::valid_qos_event(&event, event.type_identity) {
                    fail(b"QoS event failed validation");
                }
                slime_rt::debug_write(b"[fabric-subscriber-b] QoS event observed\n");
            }
            STREAM_EVENT_MAGIC => {
                let event =
                    WireStreamEvent::decode(&message).unwrap_or_else(|| fail(b"decode event"));
                if !valid_stream_event(&event, telemetry_stream::TYPE_TAG) {
                    fail(b"event failed validation");
                }
                match event.event {
                    EVENT_SAMPLE_LOST => {
                        if event.lost == 0 || event.sequence == 0 {
                            fail(b"loss event named no loss");
                        }
                        reports += 1;
                        total_lost = total_lost.saturating_add(event.lost);
                        if reports > MAX_LOSS_REPORTS || total_lost > MAX_TOTAL_LOSS {
                            fail(b"loss reporting grew past its bound");
                        }
                        observed_loss = true;
                        slime_rt::debug_write(b"[fabric-subscriber-b] bounded loss reported\n");
                    }
                    EVENT_STREAM_END => {
                        if !observed_loss {
                            fail(b"the stall was never reported as loss");
                        }
                        return consumed;
                    }
                    _ => fail(b"unknown event kind"),
                }
            }
            SAMPLE_DESCRIPTOR_MAGIC => fail(b"late large descriptor"),
            _ => fail(b"ordinary telemetry sample used control endpoint"),
        }
    }
}

/// Consume the RELIABLE diagnostics route without acknowledging its sample.
/// The service's explicit time input must terminate it through bounded QoS,
/// while the unrelated telemetry route continues independently.
fn consume_diagnostics(ring: &mut Ring<'_>) {
    let mut sample_seen = false;
    let mut deadline = false;
    let mut liveliness = false;
    let mut exhausted = false;
    loop {
        let _ = slime_rt::notification_poll(diagnostics_slot(b"ready"));
        let mut payload = [0u8; MAX_INLINE_BYTES];
        loop {
            match ring.consume(&mut payload) {
                Ok((length, _last)) => {
                    if length == 0 {
                        fail(b"empty diagnostics sample");
                    }
                    sample_seen = true;
                    slime_rt::debug_write(b"[fabric-subscriber-b] reliable sample withheld\n");
                }
                Err(RingError::Empty) => break,
                Err(_) => fail(b"diagnostics ring consume"),
            }
        }
        // Nothing else to do this pass: the ring is drained, so block and
        // become visible to the fabric's terminal `nb_send`.
        let Some(message) = receive_for(diagnostics_stream::TYPE_TAG, false) else {
            continue;
        };
        let magic = u32::from_le_bytes(message[..4].try_into().expect("record prefix"));
        match magic {
            QOS_EVENT_MAGIC => {
                let event =
                    WireQosEvent::decode(&message).unwrap_or_else(|| fail(b"decode QoS event"));
                if !slime_proto::valid_qos_event(&event, diagnostics_stream::TYPE_TAG) {
                    fail(b"QoS event failed validation");
                }
                match event.event {
                    slime_proto::fabric_qos::EVENT_MATCHED => {}
                    slime_proto::fabric_qos::EVENT_DEADLINE_MISSED => {
                        deadline = true;
                        slime_rt::debug_write(b"[fabric-subscriber-b] QoS deadline observed\n");
                    }
                    slime_proto::fabric_qos::EVENT_LIFESPAN_EXPIRED => {
                        fail(b"volatile diagnostics sample expired")
                    }
                    slime_proto::fabric_qos::EVENT_LIVELINESS_LOST => {
                        liveliness = true;
                        slime_rt::debug_write(b"[fabric-subscriber-b] QoS liveliness observed\n");
                    }
                    slime_proto::fabric_qos::EVENT_RETRY_EXHAUSTED => {
                        exhausted = true;
                        slime_rt::debug_write(
                            b"[fabric-subscriber-b] QoS retry exhausted observed\n",
                        );
                    }
                    _ => fail(b"unexpected diagnostics QoS event"),
                }
            }
            STREAM_EVENT_MAGIC => {
                let event =
                    WireStreamEvent::decode(&message).unwrap_or_else(|| fail(b"decode event"));
                if !valid_stream_event(&event, diagnostics_stream::TYPE_TAG)
                    || event.event != EVENT_STREAM_END
                {
                    fail(b"unexpected diagnostics stream event");
                }
                if !sample_seen || !deadline || !liveliness || !exhausted {
                    fail(b"diagnostics ended before every QoS condition");
                }
                slime_rt::debug_write(b"[fabric-subscriber-b] reliable QoS terminal\n");
                return;
            }
            _ => fail(b"ordinary diagnostics sample used control endpoint"),
        }
    }
}

fn request_roles() -> i64 {
    let mut route_name = [0u8; 32];
    route_name[..TELEMETRY_ROUTE.len()].copy_from_slice(TELEMETRY_ROUTE.as_bytes());
    let request = WireFabricRequest {
        magic: FABRIC_REQUEST_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        direction: DIRECTION_SUBSCRIBE,
        type_identity: telemetry_stream::TYPE_TAG,
        route_name_len: TELEMETRY_ROUTE.len() as u32,
        route_name,
        reserved: [0; 4],
    };
    let encoded = request.encode();
    slime_rt::send(CONTROL_SLOT, &encoded, &[])
}

/// The typed role descriptor and the slot its shared ring landed in.
///
/// Only a record whose magic is `CAPABILITY_TRANSFER_MAGIC` is a role reply.
/// The fabric also sends QoS events on this same control endpoint, and a v2
/// role reply carries no capability in the message -- the ring crosses as a
/// root-side export this component claims -- so `received[0]` no longer tells
/// the two apart. Discriminating on the record's own magic does, and it is the
/// same field every other reader of these bytes already trusts.
fn receive_role() -> (WireCapabilityTransfer, u32) {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => fail(b"role reply"),
            _ => {
                let Some(descriptor) = WireCapabilityTransfer::decode(&message).filter(|record| {
                    record.magic == slime_proto::capability_transfer::CAPABILITY_TRANSFER_MAGIC
                }) else {
                    continue;
                };
                if descriptor.status != 0 {
                    return (descriptor, 0);
                }
                let slot = slime_rt::capability_import().unwrap_or_else(|_| fail(b"import role"));
                return (descriptor, slot);
            }
        }
    }
}

const _: () = assert!(REQUEST_LEN == MAX_MSG);
const _: () = assert!(slime_proto::fabric_stream::EVENT_LEN == MAX_MSG);
