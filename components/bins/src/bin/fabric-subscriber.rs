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
    CAPABILITY_TRANSFER_MAGIC, FABRIC_REQUEST_MAGIC, FORMAT_VERSION, OBJECT_KIND_ENDPOINT,
    OBJECT_KIND_SHARED_BUFFER_LOAN, REQUEST_LEN, WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::fabric_qos::{QOS_EVENT_MAGIC, WireQosEvent};
use slime_proto::fabric_stream::{
    EVENT_SAMPLE_LOST, EVENT_STREAM_END, MAX_INLINE_BYTES as STREAM_MAX_INLINE_BYTES,
    STREAM_ACK_MAGIC, STREAM_EVENT_MAGIC, WireStreamAck, WireStreamEvent, WireStreamSample,
};
use slime_proto::fabric_visibility::{EVENT_PROXY_LOST, TRACE_PROXY_LOST, WireInterpositionTrace};
use slime_proto::interface_schema::telemetry_stream;
use slime_proto::ring::Ring;
use slime_proto::sample_descriptor::{SAMPLE_DESCRIPTOR_MAGIC, WireSampleDescriptor};
use slime_proto::{
    valid_capability_transfer, valid_interposition_trace, valid_sample_descriptor,
    valid_stream_sample,
};
use slime_rt::{ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};
slime_rt::entry!(main);

include!(concat!(env!("OUT_DIR"), "/fabric_profile.rs"));

/// Control endpoint to the fabric — this component's only starting authority.
const CONTROL_SLOT: u32 = 0;

const ROUTE_NAME: &str = "telemetry";

/// The visibility plane's declared proxy-chain edges: telemetry data in, ack
/// out, and route events in. Generation facts, installed before this component
/// runs; the broker's role reply names them rather than carrying them.
const PROXY_DATA_SLOT: u32 = 1;
const PROXY_ACK_SLOT: u32 = 2;
const PROXY_EVENT_SLOT: u32 = 3;

const RIGHT_SEND: u64 = 1;
const RIGHT_RECV: u64 = 2;

const PAGE: u64 = 4096;
const BASE: u64 = 0x0000_000E_0000_0000;
const RING_BYTES: usize = 4096;

/// This component's name, as the generation's participant table spells it.
const COMPONENT: &[u8] = b"fabric-subscriber";

/// This participant's declared ring depth for `route`, as the generation
/// resolved it.
///
/// The fabric formats each ring at exactly this depth, and `Ring::attach`
/// checks the header's slot count against what the caller expects — so a
/// hardcoded constant here is a disagreement waiting to happen, and it was
/// one: a ring formatted at the declared depth failed to attach against a
/// local guess. Floored at `MIN_RING_SLOTS` exactly as the fabric floors it.
fn ring_slots(route: &str) -> usize {
    FABRIC_HISTORY_DEPTHS
        .iter()
        .find(|(name, entry, _)| *name == COMPONENT && *entry == route)
        .map(|(_, _, depth)| *depth as usize)
        .unwrap_or_else(|| fail(b"route declares no history depth"))
        .max(slime_proto::fabric_ring::MIN_RING_SLOTS)
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-subscriber] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn main(_startup_arg: u32) {
    if GENERATION_BOOT_ACTION == "visibility" {
        visibility_main();
        return;
    }
    if slime_components::fabric_matrix::active() {
        matrix_main();
        return;
    }
    let route = route_identity(
        ROUTE_NAME,
        &telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    if slime_components::fabric_boot::active() {
        slime_components::fabric_boot::provision_and_park(
            b"fabric-subscriber",
            ROUTE_NAME,
            &route,
            telemetry_stream::TYPE_TAG,
            DIRECTION_SUBSCRIBE,
            1,
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

    let (descriptor, ring_slot) = receive_role();
    if descriptor.status != 0
        || !valid_capability_transfer(
            &descriptor,
            &route,
            DIRECTION_SUBSCRIBE,
            OBJECT_KIND_SHARED_BUFFER_LOAN,
        )
    {
        fail(b"declared subscriber was denied");
    }
    if slime_rt::shared_buffer_loan_map(ring_slot, BASE, 0, RING_BYTES as u64) != ERR_SUCCESS {
        fail(b"subscriber ring map");
    }
    slime_rt::debug_write(b"[fabric-subscriber] subscribe role received\n");
    // A subscribe role is one direction, and the ring it names is a loan, not
    // an endpoint. The denial below is asserted here, before the samples, so a
    // regression that widened the role fails even with the happy path intact --
    // the same rule the publisher asserts from the other side.
    //
    // Direction is not probed by asking again: the fabric answers each client
    // exactly once and reads the request's `direction` only to discard it,
    // keying authority off the control endpoint the request arrived on. A
    // second request would never be read, so there is no request this
    // component can phrase that yields the write side of its route. What the
    // role does carry is checked instead: the graph's direction, and a loan
    // that grants no send.
    if descriptor.direction != DIRECTION_SUBSCRIBE {
        fail(b"subscribe role names another direction");
    }
    if descriptor.rights_mask & RIGHT_SEND != 0 {
        fail(b"subscribe role carries publish authority");
    }
    slime_rt::debug_write(b"[fabric-subscriber] route publish denied\n");
    if slime_rt::capability_delegate(
        CONTROL_SLOT,
        ring_slot,
        slime_rt::CapabilityDisposition::Retain,
        OBJECT_KIND_SHARED_BUFFER_LOAN,
        RIGHT_SEND,
        &[0u8; 64],
    ) == ERR_SUCCESS
    {
        fail(b"subscriber re-delegated its ring loan");
    }
    slime_rt::debug_write(b"[fabric-subscriber] re-delegation denied\n");
    consume(ring_slot);
    slime_rt::debug_write(b"[fabric-subscriber] done\n");
}

/// C8.12: the interposed subscriber, and the plane's proof that a filtered
/// view is not a path to route authority.
///
/// Its telemetry arrives only over the proxy's downstream edge — it holds no
/// edge to the publisher and the broker holds none to it — so a sample landing
/// here is a sample that traversed the declared chain.
fn matrix_main() {
    use slime_components::fabric_matrix::{Outcome, request_role};

    let route = route_identity(
        ROUTE_NAME,
        &telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    match request_role(ROUTE_NAME, telemetry_stream::TYPE_TAG, DIRECTION_SUBSCRIBE) {
        Ok(Outcome::Role(descriptor)) => {
            if descriptor.rights_mask != RIGHT_RECV
                || !valid_capability_transfer(
                    &descriptor,
                    &route,
                    DIRECTION_SUBSCRIBE,
                    OBJECT_KIND_ENDPOINT,
                )
            {
                fail(b"matrix subscribe role");
            }
        }
        Ok(Outcome::Denied(_)) => fail(b"the exact compatible tuple was denied"),
        Err(_) => fail(b"matrix role request"),
    }
    slime_rt::debug_write(b"[fabric-subscriber] matrix exact tuple matched\n");

    let sample_bytes =
        receive_message(PROXY_DATA_SLOT).unwrap_or_else(|| fail(b"matrix interposed sample peer"));
    let sample = WireStreamSample::decode(&sample_bytes)
        .filter(|sample| {
            valid_stream_sample(sample, telemetry_stream::TYPE_TAG, STREAM_MAX_INLINE_BYTES)
        })
        .filter(|sample| sample.sequence == 1)
        .unwrap_or_else(|| fail(b"matrix interposed sample"));
    let ack = WireStreamAck {
        magic: STREAM_ACK_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        reserved0: 0,
        sequence: sample.sequence,
        type_identity: telemetry_stream::TYPE_TAG,
        reserved: [0; 32],
    };
    if slime_rt::send(PROXY_ACK_SLOT, &ack.encode(), &[]) != ERR_SUCCESS {
        fail(b"matrix interposed ack");
    }
    slime_rt::debug_write(b"[fabric-subscriber] matrix sample arrived through proxy\n");
}
fn visibility_main() {
    let mut cursor = 0;
    let mut routes = 0;
    let mut qos = 0;
    loop {
        match request_page(CONTROL_SLOT, cursor).unwrap_or_else(|_| fail(b"private view")) {
            ViewPage::Route(record) => {
                routes += 1;
                cursor = record.cursor;
            }
            ViewPage::Qos(record) => {
                qos += 1;
                cursor = record.cursor;
            }
            ViewPage::End(_) => break,
        }
    }
    if routes != 1 || qos != 1 {
        fail(b"private view bound");
    }
    slime_rt::debug_write(b"[fabric-subscriber] private view routes=1\n");
    // The proxy chain's three declared edges: data in, ack out, route events
    // in. The broker answers each with a descriptor alone — the endpoints
    // themselves are generation facts already installed in this CSpace — but
    // the request is still required: it is what tells the broker this client
    // is provisioned, and its loop serves every client in manifest order
    // before relaying anything.
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
    let mut descriptors = [None; 3];
    for descriptor in &mut descriptors {
        let record = receive_declared_role();
        if !valid_capability_transfer(&record, &route, DIRECTION_SUBSCRIBE, OBJECT_KIND_ENDPOINT) {
            fail(b"interposed subscriber role");
        }
        *descriptor = Some(record);
    }
    if descriptors[0].expect("data descriptor").rights_mask != RIGHT_RECV
        || descriptors[1].expect("ack descriptor").rights_mask != RIGHT_SEND
        || descriptors[2].expect("event descriptor").rights_mask != RIGHT_RECV
    {
        fail(b"interposed subscriber rights");
    }
    // Validate the relayed sample and answer with a real ack: the proxy
    // correlates on sequence, so a zero-filled reply would be refused there
    // rather than proving the chain carried this exact sample.
    if let Some(sample_bytes) = receive_message(PROXY_DATA_SLOT) {
        let sample = WireStreamSample::decode(&sample_bytes)
            .filter(|sample| {
                valid_stream_sample(sample, telemetry_stream::TYPE_TAG, STREAM_MAX_INLINE_BYTES)
            })
            .filter(|sample| sample.sequence == 1)
            .unwrap_or_else(|| fail(b"interposed sample"));
        let ack = WireStreamAck {
            magic: STREAM_ACK_MAGIC,
            version: FORMAT_VERSION,
            flags: 0,
            reserved0: 0,
            sequence: sample.sequence,
            type_identity: telemetry_stream::TYPE_TAG,
            reserved: [0; 32],
        };
        if slime_rt::send(PROXY_ACK_SLOT, &ack.encode(), &[]) != ERR_SUCCESS {
            fail(b"interposed ack");
        }
        slime_rt::debug_write(b"[fabric-subscriber] sample arrived through proxy\n");
    }
    let event_bytes =
        receive_message(PROXY_EVENT_SLOT).unwrap_or_else(|| fail(b"proxy loss event peer"));
    let event = WireInterpositionTrace::decode(&event_bytes)
        .filter(valid_interposition_trace)
        .filter(|event| event.event == TRACE_PROXY_LOST)
        .unwrap_or_else(|| fail(b"proxy loss event"));
    let _ = event;
    slime_rt::debug_write(b"[fabric-subscriber] proxy loss route event observed\n");
    // Page the view again after the loss. The claim is not that an event
    // arrived on a route endpoint but that the *graph* now reports it, which
    // only re-reading it can show — and it is what `serve_event_view` on the
    // broker side exists to answer.
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
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            slime_rt::ERR_PEER_DEAD => return None,
            n if n < 0 => fail(b"visibility receive"),
            _ => return Some(message),
        }
    }
}

fn consume(_ring_slot: u32) {
    let bytes = unsafe { core::slice::from_raw_parts_mut(BASE as *mut u8, RING_BYTES) };
    let mut ring = Ring::attach(bytes, telemetry_stream::TYPE_TAG, ring_slots(ROUTE_NAME))
        .unwrap_or_else(|_| fail(b"subscriber ring attach"));
    let mut inline = 0u32;
    let mut shared = 0u32;
    loop {
        let mut payload = [0u8; slime_proto::fabric_ring::MAX_INLINE_BYTES];
        while let Ok((length, last)) = ring.consume(&mut payload) {
            if length == 0 {
                fail(b"empty sample");
            }
            inline += 1;
            let _ = slime_rt::notification_signal(FABRIC_SUBSCRIBER_TELEMETRY_CREDIT_SLOT);
            if last && shared != 0 {
                slime_rt::debug_write(b"[fabric-subscriber] inline and shared received\n");
                return;
            }
        }
        // The ring is drained, so this loop has nothing left to do but wait on
        // the control endpoint -- and it must wait *there*, blocked. The fabric
        // announces QoS and terminal events with `seL4_NBSend`, which delivers
        // only to a receiver already blocked on the endpoint and discards
        // otherwise. Polling here and sleeping on the ring notification instead
        // would make this component permanently invisible to those sends.
        let mut message = [0u8; MAX_MSG];
        let mut received = [0u64; MAX_CAPS_PER_MSG];
        let n = slime_rt::recv_blocking(CONTROL_SLOT, &mut message, &mut received);
        if n >= 0 && n as usize == MAX_MSG {
            let magic = u32::from_le_bytes(message[..4].try_into().expect("magic"));
            if magic == SAMPLE_DESCRIPTOR_MAGIC {
                let descriptor =
                    WireSampleDescriptor::decode(&message).unwrap_or_else(|| fail(b"descriptor"));
                // A delegated loan arrives as a root-recorded export, not in
                // the message: only a native Endpoint travels inline, so
                // `received[0]` is empty and the authority is claimed here.
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
                if slime_rt::shared_buffer_loan_map(
                    loan_slot,
                    BASE + RING_BYTES as u64,
                    0,
                    descriptor.length,
                ) != ERR_SUCCESS
                {
                    fail(b"loan map");
                }
                let _ = slime_rt::shared_buffer_unmap(loan_slot, BASE + RING_BYTES as u64);
                let _ = slime_rt::shared_buffer_return(loan_slot);
                shared += 1;
                slime_rt::debug_write(b"[fabric-subscriber] shared sample verified\n");
            } else if magic == QOS_EVENT_MAGIC {
                let event = WireQosEvent::decode(&message).unwrap_or_else(|| fail(b"QoS event"));
                let _ = event;
                slime_rt::debug_write(b"[fabric-subscriber] QoS matched\n");
            } else if magic == STREAM_EVENT_MAGIC {
                let event =
                    WireStreamEvent::decode(&message).unwrap_or_else(|| fail(b"stream event"));
                if event.event == EVENT_SAMPLE_LOST {
                    fail(b"keeping-up subscriber was told it lost a sample");
                }
                if event.event == EVENT_STREAM_END && inline != 0 && shared != 0 {
                    slime_rt::debug_write(b"[fabric-subscriber] inline and shared received\n");
                    return;
                }
            }
        } else {
            let _ = slime_rt::notification_wait(FABRIC_SUBSCRIBER_TELEMETRY_READY_SLOT);
        }
    }
}

fn route_name_bytes() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    bytes[..ROUTE_NAME.len()].copy_from_slice(ROUTE_NAME.as_bytes());
    bytes
}

fn send_request(request: &WireFabricRequest) -> i64 {
    slime_rt::send(CONTROL_SLOT, &request.encode(), &[])
}

/// A role reply that carries no capability.
///
/// The visibility broker answers with the descriptor alone: the endpoint it
/// names is a generation-declared edge already installed here, so unlike
/// [`receive_role`] there is nothing to import. QoS events share this control
/// endpoint, so the record's own magic is what tells the two apart.
fn receive_declared_role() -> WireCapabilityTransfer {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => fail(b"visibility role reply"),
            _ => {
                let Some(descriptor) = WireCapabilityTransfer::decode(&message)
                    .filter(|record| record.magic == CAPABILITY_TRANSFER_MAGIC)
                else {
                    continue;
                };
                if descriptor.status != 0 {
                    fail(b"interposed subscriber role");
                }
                return descriptor;
            }
        }
    }
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
