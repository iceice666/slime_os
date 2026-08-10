#![no_std]
#![no_main]

//! C8.3/C8.4 fabric subscriber: the receiving half of the attenuated route,
//! consuming both sample forms.
//!
//! Mirrors `fabric-publisher` on the authority arm: it receives a
//! `RIGHT_RECV`-only endpoint through the kernel's narrow-on-transfer move and
//! proves it cannot publish on its own route or re-delegate the role.
//!
//! It then consumes the telemetry stream to its end. Both forms arrive on one
//! endpoint and are told apart by magic: an inline `StreamSample` carries its
//! payload whole, while a C7.6 descriptor names a receiver-bound loan of the
//! fabric's own copy, which this component maps read-only and returns exactly
//! once. Each consumed sample is acked, which is what releases the fabric's
//! delivery slot — this subscriber keeps up, so it must never be told it lost
//! anything.

use boot_contracts::fabric_graph::{CONTRACT_KIND_STREAM, DIRECTION_SUBSCRIBE, route_identity};
use slime_components::fabric_visibility::{ViewPage, request_page};
use slime_proto::capability_transfer::{
    FABRIC_REQUEST_MAGIC, FORMAT_VERSION, OBJECT_KIND_ENDPOINT, REQUEST_LEN,
    WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::fabric_qos::{QOS_EVENT_MAGIC, WireQosEvent};
use slime_proto::fabric_stream::{
    EVENT_SAMPLE_LOST, EVENT_STREAM_END, MAX_INLINE_BYTES, STREAM_ACK_MAGIC, STREAM_EVENT_MAGIC,
    STREAM_SAMPLE_MAGIC, WireStreamAck, WireStreamEvent, WireStreamSample,
};
use slime_proto::fabric_visibility::{EVENT_PROXY_LOST, TRACE_PROXY_LOST, WireInterpositionTrace};
use slime_proto::interface_schema::telemetry_stream;
use slime_proto::sample_descriptor::{SAMPLE_DESCRIPTOR_MAGIC, WireSampleDescriptor};
use slime_proto::{
    valid_capability_transfer, valid_interposition_trace, valid_sample_descriptor,
    valid_stream_event, valid_stream_sample,
};
use slime_rt::{ERR_BAD_CAP, ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, WaitSource};

slime_rt::entry!(main);

/// Control endpoint to the fabric — this component's only starting authority.
const CONTROL_SLOT: u32 = 0;

const RIGHT_SEND: u64 = 1;
const RIGHT_RECV: u64 = 2;

const ROUTE_NAME: &str = "telemetry";

const PAGE: u64 = 4096;
const BASE: u64 = 0x0000_000E_0000_0000;

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-subscriber] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn main(_startup_arg: u32) {
    if option_env!("SLIME_FABRIC_VISIBILITY_CHECK") == Some("1") {
        visibility_main();
        return;
    }
    let route = route_identity(
        ROUTE_NAME,
        &telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    if slime_components::fabric_boot::active() {
        // Two capabilities for one subscriber role: the receive-only data
        // endpoint and the send-only ack channel. The boot gate asserts both
        // arrive narrowed and then parks — no sample, no ack.
        slime_components::fabric_boot::provision_and_park(
            b"fabric-subscriber",
            ROUTE_NAME,
            &route,
            telemetry_stream::TYPE_TAG,
            DIRECTION_SUBSCRIBE,
            2,
        );
    }

    let request = WireFabricRequest {
        magic: FABRIC_REQUEST_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        direction: DIRECTION_SUBSCRIBE,
        type_identity: telemetry_stream::TYPE_TAG,
        route_name_len: ROUTE_NAME.len() as u32,
        route_name: route_name_bytes(),
        reserved: [0; 4],
    };
    if send_request(&request) != ERR_SUCCESS {
        fail(b"request");
    }
    slime_rt::debug_write(b"[fabric-subscriber] role requested\n");

    // Two capabilities arrive for one route: the receive-only data endpoint and
    // a send-only ack endpoint. Keeping them separate is what lets a subscriber
    // release a delivery slot without ever holding publish authority on the
    // route it reads.
    let mut data_slot = None;
    let mut ack_slot = None;
    for _ in 0..2 {
        let (descriptor, slot) = receive_role();
        if descriptor.status != 0 {
            fail(b"declared subscriber was denied");
        }
        if !valid_capability_transfer(
            &descriptor,
            &route,
            DIRECTION_SUBSCRIBE,
            OBJECT_KIND_ENDPOINT,
        ) {
            fail(b"descriptor does not name this role");
        }
        match descriptor.rights_mask {
            RIGHT_RECV => data_slot = Some((descriptor, slot)),
            RIGHT_SEND => ack_slot = Some(slot),
            _ => fail(b"subscriber role carries more than one direction"),
        }
    }
    let (Some((descriptor, route_slot)), Some(ack_slot)) = (data_slot, ack_slot) else {
        fail(b"a declared subscriber capability never arrived");
    };
    slime_rt::debug_write(b"[fabric-subscriber] subscribe role received\n");

    // A subscriber has no publish authority on the route it reads: the data
    // endpoint is receive-only, and the ack channel is a different object.
    if slime_rt::send(route_slot, b"forged", &[]) != ERR_BAD_CAP {
        fail(b"subscriber could publish on its route");
    }
    slime_rt::debug_write(b"[fabric-subscriber] route publish denied\n");

    // Nor can it read the ack channel it writes: the two roles are one
    // direction each, so neither is a back door into the other.
    let mut discard = [0u8; MAX_MSG];
    let mut no_caps = [0u64; MAX_CAPS_PER_MSG];
    if slime_rt::recv(ack_slot, &mut discard, &mut no_caps) != ERR_BAD_CAP {
        fail(b"subscriber could receive on its ack channel");
    }
    slime_rt::debug_write(b"[fabric-subscriber] ack channel is send-only\n");

    // The role is terminal here too: it cannot be handed on.
    let redelegation = WireCapabilityTransfer {
        rights_mask: RIGHT_RECV,
        ..descriptor
    };
    if slime_rt::cap_transfer(CONTROL_SLOT, route_slot, &redelegation.encode()) != ERR_BAD_CAP {
        fail(b"subscriber re-delegated its route");
    }
    slime_rt::debug_write(b"[fabric-subscriber] re-delegation denied\n");

    consume(route_slot, ack_slot);
    slime_rt::debug_write(b"[fabric-subscriber] done\n");
}
fn visibility_main() {
    let mut cursor = 0;
    let mut routes = 0;
    let mut qos = 0;
    loop {
        match request_page(CONTROL_SLOT, cursor).unwrap_or_else(|_| fail(b"private view")) {
            ViewPage::Route(record) => {
                if routes != 0
                    || &record.route_name[..record.route_name_len as usize] != b"telemetry"
                    || record.schema_identity != telemetry_stream::INTERFACE_IDENTITY
                {
                    fail(b"private route metadata");
                }
                routes += 1;
                cursor = record.cursor;
            }
            ViewPage::Qos(record) => {
                if record.route_name[..9] != *b"telemetry" || record.matched != 1 {
                    fail(b"private qos metadata");
                }
                qos += 1;
                cursor = record.cursor;
            }
            ViewPage::End(record) => {
                let _ = record.cursor;
                break;
            }
        }
    }
    if routes != 1 || qos != 1 {
        fail(b"private view bound");
    }
    slime_rt::debug_write(b"[fabric-subscriber] private view routes=1\n");

    let route = route_identity(
        ROUTE_NAME,
        &telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    let request = WireFabricRequest {
        magic: FABRIC_REQUEST_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        direction: DIRECTION_SUBSCRIBE,
        type_identity: telemetry_stream::TYPE_TAG,
        route_name_len: ROUTE_NAME.len() as u32,
        route_name: route_name_bytes(),
        reserved: [0; 4],
    };
    if send_request(&request) != ERR_SUCCESS {
        fail(b"visibility role request");
    }
    let mut slots = [0u32; 3];
    let mut descriptors = [None; 3];
    for index in 0..3 {
        let (descriptor, slot) = receive_role();
        if !valid_capability_transfer(
            &descriptor,
            &route,
            DIRECTION_SUBSCRIBE,
            OBJECT_KIND_ENDPOINT,
        ) {
            fail(b"interposed subscriber role");
        }
        slots[index] = slot;
        descriptors[index] = Some(descriptor);
    }
    if descriptors[0].expect("data descriptor").rights_mask != RIGHT_RECV
        || descriptors[1].expect("ack descriptor").rights_mask != RIGHT_SEND
        || descriptors[2].expect("event descriptor").rights_mask != RIGHT_RECV
    {
        fail(b"interposed subscriber rights");
    }
    if slime_rt::send(slots[0], b"bypass", &[]) != ERR_BAD_CAP {
        fail(b"subscriber bypass publish");
    }
    let mut discard = [0u8; MAX_MSG];
    let mut no_caps = [0u64; MAX_CAPS_PER_MSG];
    if slime_rt::recv(slots[1], &mut discard, &mut no_caps) != ERR_BAD_CAP {
        fail(b"subscriber ack widened");
    }
    if slime_rt::cap_transfer(
        CONTROL_SLOT,
        slots[0],
        &descriptors[0].expect("data descriptor").encode(),
    ) != ERR_BAD_CAP
    {
        fail(b"subscriber retransfer");
    }

    let sample_sequence = match receive_message(slots[0]) {
        Some(sample_bytes) => {
            let sample = WireStreamSample::decode(&sample_bytes)
                .filter(|sample| {
                    valid_stream_sample(sample, telemetry_stream::TYPE_TAG, MAX_INLINE_BYTES)
                })
                .filter(|sample| sample.sequence == 1)
                .unwrap_or_else(|| fail(b"interposed sample"));
            ack(slots[1], sample.sequence);
            slime_rt::debug_write(b"[fabric-subscriber] sample arrived through proxy\n");
            sample.sequence
        }
        None => 1,
    };

    let event_bytes = receive_message(slots[2]).unwrap_or_else(|| fail(b"proxy loss event peer"));
    let event = WireInterpositionTrace::decode(&event_bytes)
        .filter(valid_interposition_trace)
        .filter(|event| {
            event.event == TRACE_PROXY_LOST
                && event.route_identity == route
                && event.sequence == sample_sequence
        })
        .unwrap_or_else(|| fail(b"proxy loss event"));
    let _ = event;
    slime_rt::debug_write(b"[fabric-subscriber] proxy loss route event observed\n");

    let mut cursor = 0;
    let mut event_seen = false;
    loop {
        match request_page(CONTROL_SLOT, cursor).unwrap_or_else(|_| fail(b"event graph view")) {
            ViewPage::Route(record) => cursor = record.cursor,
            ViewPage::Qos(record) => {
                if record.event_mask != EVENT_PROXY_LOST {
                    fail(b"proxy loss absent from graph view");
                }
                event_seen = true;
                cursor = record.cursor;
            }
            ViewPage::End(_) => break,
        }
    }
    if !event_seen {
        fail(b"event graph view empty");
    }
    slime_rt::debug_write(b"[fabric-subscriber] proxy loss visible in graph view\n");
}

fn receive_message(slot: u32) -> Option<[u8; MAX_MSG]> {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(slot, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(slot)]),
            slime_rt::ERR_PEER_DEAD => return None,
            n if n < 0 => fail(b"visibility receive"),
            n if n as usize != MAX_MSG => fail(b"visibility message length"),
            _ if received.iter().any(|slot| *slot != 0) => fail(b"visibility carried capability"),
            _ => return Some(message),
        }
    }
}

/// Consume the route until the fabric reports it ended.
///
/// A keeping-up subscriber acks every sample it consumes, so the fabric's ring
/// never fills and no loss can be reported. Observing a `SAMPLE_LOST` here is
/// therefore a failure, not a tolerated outcome: it would mean the fabric
/// evicted a sample this component had already released the slot for.
fn consume(route_slot: u32, ack_slot: u32) {
    let mut inline = 0u32;
    let mut shared = 0u32;
    loop {
        let mut message = [0u8; MAX_MSG];
        let mut received = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(route_slot, &mut message, &mut received) {
            ERR_WOULDBLOCK => {
                slime_rt::wait(&[WaitSource::Endpoint(route_slot)]);
                continue;
            }
            n if n < 0 => fail(b"stream recv"),
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
                // The payload is a function of the sequence, so this verifies
                // the exact sample rather than a well-formed one.
                let expected = sample.sequence as u8;
                if sample.payload[..sample.payload_len as usize]
                    .iter()
                    .enumerate()
                    .any(|(index, byte)| *byte != expected.wrapping_add(index as u8))
                {
                    fail(b"inline payload mismatch");
                }
                inline += 1;
                ack(ack_slot, sample.sequence);
            }
            SAMPLE_DESCRIPTOR_MAGIC => {
                let Some(descriptor) = WireSampleDescriptor::decode(&message) else {
                    fail(b"decode sample descriptor")
                };
                let loan_slot = received[0] as u32;
                if loan_slot == 0 {
                    fail(b"descriptor arrived without its loan");
                }
                // Validate before mapping or allocating anything.
                if !valid_sample_descriptor(
                    &descriptor,
                    descriptor.loan_id,
                    telemetry_stream::TYPE_TAG,
                    PAGE,
                ) {
                    fail(b"descriptor failed validation");
                }
                if slime_rt::shared_buffer_loan_map(loan_slot, BASE, 0, descriptor.length)
                    != ERR_SUCCESS
                {
                    fail(b"loan map");
                }
                // SAFETY: the kernel mapped exactly `descriptor.length` bytes
                // read-only at `BASE`, and they stay mapped until the return.
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
                // Single-return: settling here releases this subscriber's own
                // downstream loan, independently of any other subscriber's.
                if slime_rt::shared_buffer_return(loan_slot) != ERR_SUCCESS {
                    fail(b"return loan");
                }
                shared += 1;
                slime_rt::debug_write(b"[fabric-subscriber] shared sample verified\n");
                ack(ack_slot, descriptor.sequence);
            }
            QOS_EVENT_MAGIC => {
                let Some(event) = WireQosEvent::decode(&message) else {
                    fail(b"decode QoS event")
                };
                if !slime_proto::valid_qos_event(&event, telemetry_stream::TYPE_TAG) {
                    fail(b"QoS event failed validation");
                }
                match event.event {
                    slime_proto::fabric_qos::EVENT_MATCHED => {
                        slime_rt::debug_write(b"[fabric-subscriber] QoS matched\n");
                    }
                    slime_proto::fabric_qos::EVENT_INCOMPATIBLE_QOS => {
                        slime_rt::debug_write(b"[fabric-subscriber] QoS incompatible\n");
                    }
                    slime_proto::fabric_qos::EVENT_LIFESPAN_EXPIRED => {
                        slime_rt::debug_write(b"[fabric-subscriber] QoS lifespan expired\n");
                    }
                    slime_proto::fabric_qos::EVENT_RETRY_EXHAUSTED => {
                        slime_rt::debug_write(b"[fabric-subscriber] QoS retry exhausted\n");
                    }
                    slime_proto::fabric_qos::EVENT_DEADLINE_MISSED => {
                        slime_rt::debug_write(b"[fabric-subscriber] QoS deadline missed\n");
                    }
                    slime_proto::fabric_qos::EVENT_LIVELINESS_LOST => {
                        slime_rt::debug_write(b"[fabric-subscriber] QoS liveliness lost\n");
                    }
                    slime_proto::fabric_qos::EVENT_UNMATCHED
                    | slime_proto::fabric_qos::EVENT_PEER_DEAD => {
                        slime_rt::debug_write(b"[fabric-subscriber] QoS terminal event\n");
                    }
                    _ => fail(b"unknown QoS event"),
                }
            }
            STREAM_EVENT_MAGIC => {
                let Some(event) = WireStreamEvent::decode(&message) else {
                    fail(b"decode event")
                };
                if !valid_stream_event(&event, telemetry_stream::TYPE_TAG) {
                    fail(b"event failed validation");
                }
                match event.event {
                    EVENT_SAMPLE_LOST => fail(b"keeping-up subscriber was told it lost a sample"),
                    EVENT_STREAM_END => {
                        // Both forms, because every graph that declares this
                        // component also declares the second publisher
                        // (`fabric-publisher-b`) that sends the `>MAX_MSG` one.
                        //
                        // This was briefly a `option_env!` branch: P5.5.1's
                        // seL4 graph declared one publisher and so produced
                        // only the inline form. P5.5.2 removed the branch by
                        // declaring the publisher that produces the other form,
                        // which was the stated way back — the component is the
                        // x86 binary again, with no seL4 branch in it.
                        if inline == 0 || shared == 0 {
                            fail(b"stream ended before both sample forms arrived");
                        }
                        slime_rt::debug_write(b"[fabric-subscriber] inline and shared received\n");
                        return;
                    }
                    _ => fail(b"unknown event kind"),
                }
            }
            _ => fail(b"unknown stream record"),
        }
    }
}

/// Release one delivery slot by naming the sequence it consumed. Sent on the
/// send-only ack channel, never on the route this component reads.
fn ack(ack_slot: u32, sequence: u64) {
    let ack = WireStreamAck {
        magic: STREAM_ACK_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        reserved0: 0,
        sequence,
        type_identity: telemetry_stream::TYPE_TAG,
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

fn route_name_bytes() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[..ROUTE_NAME.len()].copy_from_slice(ROUTE_NAME.as_bytes());
    bytes
}

fn send_request(request: &WireFabricRequest) -> i64 {
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
const _: () = assert!(slime_proto::fabric_stream::ACK_LEN == MAX_MSG);
