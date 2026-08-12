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
    FABRIC_REQUEST_MAGIC, FORMAT_VERSION, OBJECT_KIND_SHARED_BUFFER_LOAN, REQUEST_LEN,
    WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::fabric_qos::{QOS_EVENT_MAGIC, WireQosEvent};
use slime_proto::fabric_stream::{
    EVENT_SAMPLE_LOST, EVENT_STREAM_END, MAX_INLINE_BYTES, STREAM_EVENT_MAGIC, WireStreamEvent,
};
use slime_proto::interface_schema::{diagnostics_stream, telemetry_stream};
use slime_proto::ring::{Ring, RingError};
use slime_proto::sample_descriptor::{SAMPLE_DESCRIPTOR_MAGIC, WireSampleDescriptor};
use slime_proto::{valid_capability_transfer, valid_sample_descriptor, valid_stream_event};
use slime_rt::{ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

include!(concat!(env!("OUT_DIR"), "/fabric_profile.rs"));

const CONTROL_SLOT: u32 = 0;

const TELEMETRY_ROUTE: &str = "telemetry";
const DIAGNOSTICS_ROUTE: &str = "diagnostics";

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

/// This component's name, as the generation's participant table spells it.
const COMPONENT: &[u8] = b"fabric-subscriber-b";

/// This participant's declared ring depth for `route`, as the generation
/// resolved it.
///
/// The fabric formats each ring at exactly this depth, and `Ring::attach`
/// checks the header's slot count against what the caller expects — so a
/// hardcoded constant here is a disagreement waiting to happen. Floored at
/// `MIN_RING_SLOTS` exactly as the fabric floors it.
fn ring_slots(route: &str) -> usize {
    FABRIC_HISTORY_DEPTHS
        .iter()
        .find(|(name, entry, _)| *name == COMPONENT && *entry == route)
        .map(|(_, _, depth)| *depth as usize)
        .unwrap_or_else(|| fail(b"route declares no history depth"))
        .max(slime_proto::fabric_ring::MIN_RING_SLOTS)
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-subscriber-b] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn main(_startup_arg: u32) {
    if option_env!("SLIME_FABRIC_VISIBILITY_CHECK") == Some("1") {
        visibility_main();
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
    receive_large_sample(CONTROL_SLOT);
    if option_env!("SLIME_FABRIC_QOS_CHECK") == Some("1") {
        consume_diagnostics(&mut diagnostics_ring);
    } else {
        consume_diagnostics_stream(&mut diagnostics_ring);
    }
    consume_telemetry(&mut telemetry_ring);
    slime_rt::debug_write(b"[fabric-subscriber-b] done\n");
}
fn visibility_main() {
    let route = route_identity(
        DIAGNOSTICS_ROUTE,
        &diagnostics_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    if request_roles() != ERR_SUCCESS {
        fail(b"visibility request");
    }
    let (descriptor, ring_slot) = receive_role();
    if descriptor.status != 0
        || !valid_capability_transfer(
            &descriptor,
            &route,
            DIRECTION_SUBSCRIBE,
            OBJECT_KIND_SHARED_BUFFER_LOAN,
        )
    {
        fail(b"visibility diagnostics role");
    }
    if slime_rt::shared_buffer_loan_map(ring_slot, DIAGNOSTICS_RING_BASE, 0, RING_BYTES as u64)
        != ERR_SUCCESS
    {
        fail(b"visibility diagnostics ring map");
    }
    let bytes =
        unsafe { core::slice::from_raw_parts_mut(DIAGNOSTICS_RING_BASE as *mut u8, RING_BYTES) };
    let mut ring = Ring::attach(
        bytes,
        diagnostics_stream::TYPE_TAG,
        ring_slots(DIAGNOSTICS_ROUTE),
    )
    .unwrap_or_else(|_| fail(b"visibility diagnostics ring attach"));
    loop {
        let _ = slime_rt::notification_poll(FABRIC_SUBSCRIBER_B_DIAGNOSTICS_READY_SLOT);
        let mut payload = [0u8; MAX_INLINE_BYTES];
        match ring.consume(&mut payload) {
            Ok((length, _)) if length != 0 => break,
            Ok(_) => fail(b"empty visibility sample"),
            Err(RingError::Empty) => slime_rt::yield_now(),
            Err(_) => fail(b"visibility diagnostics ring consume"),
        }
    }
    let _ = slime_rt::notification_signal(FABRIC_SUBSCRIBER_B_DIAGNOSTICS_CREDIT_SLOT);
    slime_rt::debug_write(b"[fabric-subscriber-b] unrelated diagnostics live after proxy death\n");
}

fn consume_diagnostics_stream(ring: &mut Ring<'_>) {
    let mut sample_seen = false;
    loop {
        let _ = slime_rt::notification_poll(FABRIC_SUBSCRIBER_B_DIAGNOSTICS_READY_SLOT);
        let mut payload = [0u8; MAX_INLINE_BYTES];
        loop {
            match ring.consume(&mut payload) {
                Ok((length, _last)) => {
                    if length == 0 {
                        fail(b"empty diagnostics sample");
                    }
                    sample_seen = true;
                    let _ =
                        slime_rt::notification_signal(FABRIC_SUBSCRIBER_B_DIAGNOSTICS_CREDIT_SLOT);
                }
                Err(RingError::Empty) => break,
                Err(_) => fail(b"diagnostics ring consume"),
            }
        }
        let mut message = [0u8; MAX_MSG];
        let mut received = [0u64; MAX_CAPS_PER_MSG];
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => {
                slime_rt::yield_now();
                continue;
            }
            n if n < 0 => fail(b"diagnostics control recv"),
            _ => {}
        }
        let magic = u32::from_le_bytes(message[..4].try_into().expect("record prefix"));
        match magic {
            QOS_EVENT_MAGIC => {}
            STREAM_EVENT_MAGIC => {
                let event =
                    WireStreamEvent::decode(&message).unwrap_or_else(|| fail(b"decode event"));
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

fn receive_large_sample(control_slot: u32) {
    loop {
        let mut message = [0u8; MAX_MSG];
        let mut received = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(control_slot, &mut message, &mut received) {
            ERR_WOULDBLOCK => {
                slime_rt::yield_now();
                continue;
            }
            n if n < 0 => fail(b"telemetry control recv"),
            n => n as usize,
        };
        if length != MAX_MSG {
            fail(b"descriptor is not one control message");
        }
        let magic = u32::from_le_bytes(message[..4].try_into().expect("record prefix"));
        if magic == QOS_EVENT_MAGIC || magic == STREAM_EVENT_MAGIC {
            continue;
        }
        if magic != SAMPLE_DESCRIPTOR_MAGIC {
            fail(b"ordinary telemetry sample used control endpoint");
        }
        let descriptor = WireSampleDescriptor::decode(&message)
            .unwrap_or_else(|| fail(b"decode sample descriptor"));
        let loan_slot = received[0] as u32;
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
        return;
    }
}

/// Consume telemetry until `stop` is satisfied.
///
/// Loss is admissible throughout — this reader declares BEST_EFFORT — so it is
/// counted and bounded rather than forbidden at any point. Returns the number
/// of samples consumed.
fn consume_telemetry(ring: &mut Ring<'_>) -> u32 {
    let mut consumed = 0;
    let mut observed_loss = false;
    let mut reports = 0u32;
    let mut total_lost = 0u64;
    loop {
        let _ = slime_rt::notification_poll(FABRIC_SUBSCRIBER_B_TELEMETRY_READY_SLOT);
        let mut payload = [0u8; MAX_INLINE_BYTES];
        loop {
            match ring.consume(&mut payload) {
                Ok((length, _last)) => {
                    if length == 0 {
                        fail(b"empty telemetry sample");
                    }
                    consumed += 1;
                    let _ =
                        slime_rt::notification_signal(FABRIC_SUBSCRIBER_B_TELEMETRY_CREDIT_SLOT);
                }
                Err(RingError::Empty) => break,
                Err(_) => fail(b"telemetry ring consume"),
            }
        }
        let mut message = [0u8; MAX_MSG];
        let mut received = [0u64; MAX_CAPS_PER_MSG];
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => {
                slime_rt::yield_now();
                continue;
            }
            n if n < 0 => fail(b"telemetry control recv"),
            _ => {}
        }
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
        let _ = slime_rt::notification_poll(FABRIC_SUBSCRIBER_B_DIAGNOSTICS_READY_SLOT);
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
        let mut message = [0u8; MAX_MSG];
        let mut received = [0u64; MAX_CAPS_PER_MSG];
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => {
                slime_rt::yield_now();
                continue;
            }
            n if n < 0 => fail(b"diagnostics control recv"),
            _ => {}
        }
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
