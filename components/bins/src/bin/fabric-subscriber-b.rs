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
    FABRIC_REQUEST_MAGIC, FORMAT_VERSION, OBJECT_KIND_ENDPOINT, REQUEST_LEN,
    WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::fabric_qos::{QOS_EVENT_MAGIC, WireQosEvent};
use slime_proto::fabric_stream::{
    EVENT_SAMPLE_LOST, EVENT_STREAM_END, MAX_INLINE_BYTES, STREAM_ACK_MAGIC, STREAM_EVENT_MAGIC,
    STREAM_SAMPLE_MAGIC, WireStreamAck, WireStreamEvent, WireStreamSample,
};
use slime_proto::interface_schema::{diagnostics_stream, telemetry_stream};
use slime_proto::sample_descriptor::{SAMPLE_DESCRIPTOR_MAGIC, WireSampleDescriptor};
use slime_proto::{
    valid_capability_transfer, valid_sample_descriptor, valid_stream_event, valid_stream_sample,
};
use slime_rt::{ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, WaitSource};

slime_rt::entry!(main);

const CONTROL_SLOT: u32 = 0;

const RIGHT_SEND: u64 = 1;
const RIGHT_RECV: u64 = 2;

const TELEMETRY_ROUTE: &str = "telemetry";
const DIAGNOSTICS_ROUTE: &str = "diagnostics";

const PAGE: u64 = 4096;
const BASE: u64 = 0x0000_000F_0000_0000;

/// Bounds on what a stall may cost this subscriber.
///
/// The telemetry publishers send a fixed, known number of samples between them
/// — `fabric-publisher` its inline set plus its stall-window burst and a
/// terminal sample, `fabric-publisher-b` one large one — so neither the total
/// lost nor the number of reports can exceed that. A fabric that retried
/// instead of reporting, or reported per delivery attempt, blows past both.
const MAX_TOTAL_LOSS: u64 = 16;
const MAX_LOSS_REPORTS: u32 = 16;

/// One route's pair of provisioned capabilities: the receive-only data endpoint
/// and the send-only ack endpoint. Held apart so neither route can borrow the
/// other's authority, and so a missing half is a named failure rather than a
/// silent stall.
#[derive(Default)]
struct RoutePair {
    data: Option<u32>,
    ack: Option<u32>,
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-subscriber-b] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn main() {
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

    if request_roles() != ERR_SUCCESS {
        fail(b"request");
    }
    slime_rt::debug_write(b"[fabric-subscriber-b] roles requested\n");

    // Four capabilities arrive: a receive-only data endpoint and a send-only
    // ack endpoint for each of the two declared routes. Each is matched by the
    // route identity its own descriptor carries and by its direction mask, so
    // arrival order is not authority and no route's pair can be mistaken for
    // the other's.
    let mut telemetry = RoutePair::default();
    let mut diagnostics_pair = RoutePair::default();
    for _ in 0..4 {
        let (descriptor, slot) = receive_role();
        if descriptor.status != 0 {
            fail(b"declared subscriber was denied");
        }
        let pair = if valid_capability_transfer(
            &descriptor,
            &telemetry_route,
            DIRECTION_SUBSCRIBE,
            OBJECT_KIND_ENDPOINT,
        ) {
            &mut telemetry
        } else if valid_capability_transfer(
            &descriptor,
            &diagnostics_route,
            DIRECTION_SUBSCRIBE,
            OBJECT_KIND_ENDPOINT,
        ) {
            &mut diagnostics_pair
        } else {
            fail(b"role names no declared route");
        };
        match descriptor.rights_mask {
            RIGHT_RECV => pair.data = Some(slot),
            RIGHT_SEND => pair.ack = Some(slot),
            _ => fail(b"subscriber role carries more than one direction"),
        }
    }
    let (Some(telemetry_slot), Some(telemetry_ack)) = (telemetry.data, telemetry.ack) else {
        fail(b"a declared telemetry capability never arrived");
    };
    let (Some(diagnostics_slot), Some(diagnostics_ack)) =
        (diagnostics_pair.data, diagnostics_pair.ack)
    else {
        fail(b"a declared diagnostics capability never arrived");
    };
    // Two routes, four distinct capabilities. Any collision would mean the
    // fabric had merged two declared edges into one.
    if telemetry_slot == diagnostics_slot || telemetry_ack == diagnostics_ack {
        fail(b"two declared routes arrived as one capability");
    }
    slime_rt::debug_write(b"[fabric-subscriber-b] both subscribe roles received\n");

    // Leave telemetry unread while the independent diagnostics route moves.
    // The QoS profile drives its reliable/deadline arms with explicit time;
    // the plain stream profile only proves the unrelated route stays live.
    slime_rt::debug_write(b"[fabric-subscriber-b] stalling on telemetry\n");
    // Receive the large shared sample before stalling inline delivery. Returning
    // its loan proves the second independently-accounted downstream loan; the
    // later inline burst still fills KEEP_LAST and produces bounded loss.
    receive_large_sample(telemetry_slot, telemetry_ack);

    if option_env!("SLIME_FABRIC_QOS_CHECK") == Some("1") {
        consume_diagnostics(diagnostics_slot, diagnostics_ack);
    } else {
        consume_diagnostics_stream(diagnostics_slot, diagnostics_ack);
    }

    // Resume telemetry after the independent diagnostics arm. The large sample
    // still proves the shared path if it survived the fixed KEEP_LAST window;
    // loss itself is the required zero-credit outcome.
    consume_telemetry(telemetry_slot, telemetry_ack);
    slime_rt::debug_write(b"[fabric-subscriber-b] done\n");
}

fn consume_diagnostics_stream(route_slot: u32, ack_slot: u32) {
    let mut sample_seen = false;
    loop {
        let mut message = [0u8; MAX_MSG];
        let mut received = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(route_slot, &mut message, &mut received) {
            ERR_WOULDBLOCK => {
                slime_rt::wait(&[WaitSource::Endpoint(route_slot)]);
                continue;
            }
            n if n < 0 => fail(b"diagnostics recv"),
            n => n as usize,
        };
        if length != MAX_MSG {
            fail(b"stream record is not one control message");
        }
        let magic = u32::from_le_bytes(message[..4].try_into().expect("record prefix"));
        match magic {
            STREAM_SAMPLE_MAGIC => {
                let Some(sample) = WireStreamSample::decode(&message) else {
                    fail(b"decode diagnostics sample")
                };
                if !valid_stream_sample(&sample, diagnostics_stream::TYPE_TAG, MAX_INLINE_BYTES) {
                    fail(b"diagnostics sample failed validation");
                }
                sample_seen = true;
                ack(ack_slot, sample.sequence, diagnostics_stream::TYPE_TAG);
            }
            QOS_EVENT_MAGIC => {}
            STREAM_EVENT_MAGIC => {
                let Some(event) = WireStreamEvent::decode(&message) else {
                    fail(b"decode event")
                };
                if !valid_stream_event(&event, diagnostics_stream::TYPE_TAG)
                    || event.event != EVENT_STREAM_END
                    || !sample_seen
                {
                    fail(b"unexpected diagnostics stream event");
                }
                slime_rt::debug_write(b"[fabric-subscriber-b] diagnostics unaffected by stall\n");
                return;
            }
            _ => fail(b"unknown stream record"),
        }
    }
}

fn receive_large_sample(route_slot: u32, ack_slot: u32) {
    loop {
        let mut message = [0u8; MAX_MSG];
        let mut received = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(route_slot, &mut message, &mut received) {
            ERR_WOULDBLOCK => {
                slime_rt::wait(&[WaitSource::Endpoint(route_slot)]);
                continue;
            }
            n if n < 0 => fail(b"telemetry recv"),
            n => n as usize,
        };
        if length != MAX_MSG {
            fail(b"stream record is not one control message");
        }
        let magic = u32::from_le_bytes(message[..4].try_into().expect("record prefix"));
        match magic {
            STREAM_SAMPLE_MAGIC => {
                let Some(sample) = WireStreamSample::decode(&message) else {
                    fail(b"decode inline sample")
                };
                if !valid_stream_sample(&sample, telemetry_stream::TYPE_TAG, MAX_INLINE_BYTES) {
                    fail(b"inline sample failed validation");
                }
                ack(ack_slot, sample.sequence, telemetry_stream::TYPE_TAG);
                continue;
            }
            QOS_EVENT_MAGIC | STREAM_EVENT_MAGIC => continue,
            SAMPLE_DESCRIPTOR_MAGIC => {}
            _ => fail(b"unknown stream record"),
        }
        let Some(descriptor) = WireSampleDescriptor::decode(&message) else {
            fail(b"decode sample descriptor")
        };
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
        ack(ack_slot, descriptor.sequence, telemetry_stream::TYPE_TAG);
        slime_rt::debug_write(b"[fabric-subscriber-b] shared sample verified\n");
        return;
    }
}

/// Consume telemetry until `stop` is satisfied.
///
/// Loss is admissible throughout — this reader declares BEST_EFFORT — so it is
/// counted and bounded rather than forbidden at any point. Returns the number
/// of samples consumed.
fn consume_telemetry(route_slot: u32, ack_slot: u32) -> u32 {
    let mut consumed = 0;
    let mut observed_loss = false;
    let mut reports = 0u32;
    let mut total_lost = 0u64;
    loop {
        let mut message = [0u8; MAX_MSG];
        let mut received = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(route_slot, &mut message, &mut received) {
            ERR_WOULDBLOCK => {
                slime_rt::wait(&[WaitSource::Endpoint(route_slot)]);
                continue;
            }
            n if n < 0 => fail(b"telemetry recv"),
            n => n as usize,
        };
        if length != MAX_MSG {
            fail(b"stream record is not one control message");
        }
        let magic = u32::from_le_bytes(message[..4].try_into().expect("record prefix"));
        match magic {
            STREAM_SAMPLE_MAGIC => {
                let Some(sample) = WireStreamSample::decode(&message) else {
                    fail(b"decode inline sample")
                };
                if !valid_stream_sample(&sample, telemetry_stream::TYPE_TAG, MAX_INLINE_BYTES) {
                    fail(b"inline sample failed validation");
                }
                consumed += 1;
                ack(ack_slot, sample.sequence, telemetry_stream::TYPE_TAG);
            }
            SAMPLE_DESCRIPTOR_MAGIC => {
                let Some(descriptor) = WireSampleDescriptor::decode(&message) else {
                    fail(b"decode sample descriptor")
                };
                let loan_slot = received[0] as u32;
                if loan_slot == 0 {
                    fail(b"descriptor arrived without its loan");
                }
                if !valid_sample_descriptor(
                    &descriptor,
                    descriptor.loan_id,
                    telemetry_stream::TYPE_TAG,
                    PAGE,
                ) {
                    fail(b"descriptor failed validation");
                }
                // This subscriber's loan is its own: mapping and returning it
                // must not depend on, or disturb, the other subscriber's loan
                // of the same fabric copy.
                if slime_rt::shared_buffer_loan_map(loan_slot, BASE, 0, descriptor.length)
                    != ERR_SUCCESS
                {
                    fail(b"loan map");
                }
                // SAFETY: exactly `descriptor.length` bytes are mapped
                // read-only at `BASE` until the return below.
                let mismatch = unsafe {
                    let bytes = BASE as *const u8;
                    (0..descriptor.length as usize)
                        .find(|index| bytes.add(*index).read_volatile() != (*index % 251) as u8)
                };
                if mismatch.is_some() {
                    fail(b"shared payload mismatch");
                }
                if slime_rt::shared_buffer_unmap(loan_slot, BASE) != ERR_SUCCESS {
                    fail(b"unmap");
                }
                if slime_rt::shared_buffer_return(loan_slot) != ERR_SUCCESS {
                    fail(b"return loan");
                }
                slime_rt::debug_write(b"[fabric-subscriber-b] shared sample verified\n");
                consumed += 1;
                ack(ack_slot, descriptor.sequence, telemetry_stream::TYPE_TAG);
            }
            QOS_EVENT_MAGIC => {
                let Some(event) = WireQosEvent::decode(&message) else {
                    fail(b"decode QoS event")
                };
                if !slime_proto::valid_qos_event(
                    &event,
                    if event.type_identity == diagnostics_stream::TYPE_TAG {
                        diagnostics_stream::TYPE_TAG
                    } else {
                        telemetry_stream::TYPE_TAG
                    },
                ) {
                    fail(b"QoS event failed validation");
                }
                slime_rt::debug_write(b"[fabric-subscriber-b] QoS event observed\n");
            }
            STREAM_EVENT_MAGIC => {
                let Some(event) = WireStreamEvent::decode(&message) else {
                    fail(b"decode event")
                };
                if !valid_stream_event(&event, telemetry_stream::TYPE_TAG) {
                    fail(b"event failed validation");
                }
                match event.event {
                    EVENT_SAMPLE_LOST => {
                        // Loss before the deliberate stall is admissible too:
                        // this reader declares BEST_EFFORT at depth 4 while two
                        // publishers feed its route, so the fabric may evict
                        // even while it is keeping up. What must hold either
                        // way is that loss stays bounded and named, which is
                        // what the counters below enforce.
                        if event.lost == 0 || event.sequence == 0 {
                            fail(b"loss event named no loss");
                        }
                        // Bounded is the property, not "exactly one event": a
                        // report covers the drops accrued since the last one,
                        // so a stall spanning several admissions can produce
                        // more than one. What must never happen is unbounded
                        // growth — a fabric retrying instead of reporting would
                        // emit a report per dropped sample forever. Both counts
                        // are bounded by what the publishers actually sent.
                        reports += 1;
                        total_lost = total_lost.saturating_add(event.lost);
                        if reports > MAX_LOSS_REPORTS || total_lost > MAX_TOTAL_LOSS {
                            fail(b"loss reporting grew past its bound");
                        }
                        // One marker per event, not per stall: the gate counts
                        // these, so a fabric that reported per delivery attempt
                        // would show as a growing series rather than hiding
                        // behind a print-once flag.
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
            _ => fail(b"unknown stream record"),
        }
    }
}

/// Consume the RELIABLE diagnostics route without acknowledging its sample.
/// The service's explicit time input must terminate it through bounded QoS,
/// while the unrelated telemetry route continues independently.
fn consume_diagnostics(route_slot: u32, _ack_slot: u32) {
    let mut sample_seen = false;
    let mut deadline = false;
    let mut liveliness = false;
    let mut exhausted = false;
    loop {
        let mut message = [0u8; MAX_MSG];
        let mut received = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(route_slot, &mut message, &mut received) {
            ERR_WOULDBLOCK => {
                slime_rt::wait(&[WaitSource::Endpoint(route_slot)]);
                continue;
            }
            n if n < 0 => fail(b"diagnostics recv"),
            n => n as usize,
        };
        if length != MAX_MSG {
            fail(b"stream record is not one control message");
        }
        let magic = u32::from_le_bytes(message[..4].try_into().expect("record prefix"));
        match magic {
            STREAM_SAMPLE_MAGIC => {
                let Some(sample) = WireStreamSample::decode(&message) else {
                    fail(b"decode diagnostics sample")
                };
                if !valid_stream_sample(&sample, diagnostics_stream::TYPE_TAG, MAX_INLINE_BYTES) {
                    fail(b"diagnostics sample failed validation");
                }
                sample_seen = true;
                slime_rt::debug_write(b"[fabric-subscriber-b] reliable sample withheld\n");
            }
            QOS_EVENT_MAGIC => {
                let Some(event) = WireQosEvent::decode(&message) else {
                    fail(b"decode QoS event")
                };
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
                let Some(event) = WireStreamEvent::decode(&message) else {
                    fail(b"decode event")
                };
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
            _ => fail(b"unknown stream record"),
        }
    }
}

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
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(ack_slot)]),
            _ => fail(b"ack"),
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
    loop {
        match slime_rt::send(CONTROL_SLOT, &encoded, &[]) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(CONTROL_SLOT)]),
            result => return result,
        }
    }
}

fn receive_role() -> (WireCapabilityTransfer, u32) {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(CONTROL_SLOT)]),
            n if n < 0 => fail(b"role reply"),
            _ => {
                let Some(descriptor) = WireCapabilityTransfer::decode(&message) else {
                    fail(b"decode role reply")
                };
                return (descriptor, received[0] as u32);
            }
        }
    }
}

const _: () = assert!(REQUEST_LEN == MAX_MSG);
const _: () = assert!(slime_proto::fabric_stream::EVENT_LEN == MAX_MSG);
