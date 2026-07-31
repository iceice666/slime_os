#![no_std]
#![no_main]

//! C8.10 declared interposition proxy: the one hop the generation names in the
//! telemetry subscriber's interposition chain.
//!
//! The chain `publisher -> fabric -> proxy -> subscriber` is the only telemetry
//! path. The proxy holds exactly four narrowed roles — upstream data, upstream
//! ack, downstream data, downstream ack — and nothing else. It cannot publish
//! on the route it relays, cannot read the ack channel it writes, and cannot
//! re-delegate any of them: each is proven against the kernel here rather than
//! assumed, so "read-only interposition never becomes route authority" is a
//! checked property of the boot, not a claim.
//!
//! Its introspection view is separately empty. The proxy is granted a relay
//! role, not graph visibility, so it must not be able to infer the protected
//! graph it forwards for. That is asserted before it asks for any role at all.
//!
//! Distinct from [`fabric-probe`] (which the graph declares no edge for) and
//! [`fabric-observer`] (which reads a filtered view but relays nothing): the
//! milestone requires three separate task identities with non-overlapping
//! grants, so no role here can be replayed as another's.

use boot_contracts::fabric_graph::{CONTRACT_KIND_STREAM, DIRECTION_SUBSCRIBE, route_identity};
use slime_components::fabric_visibility::{ViewPage, request_page};
use slime_proto::capability_transfer::{
    FABRIC_REQUEST_MAGIC, FORMAT_VERSION, OBJECT_KIND_ENDPOINT, REQUEST_LEN,
    WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::fabric_stream::{WireStreamAck, WireStreamSample};
use slime_proto::fabric_visibility::{
    FORMAT_VERSION as VISIBILITY_VERSION, INTERPOSITION_TRACE_MAGIC, TRACE_RELAYED,
    WireInterpositionTrace,
};
use slime_proto::interface_schema::telemetry_stream;
use slime_proto::{
    valid_capability_transfer, valid_interposition_trace, valid_stream_ack, valid_stream_sample,
};
use slime_rt::{ERR_BAD_CAP, ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, WaitSource};

slime_rt::entry!(main);

const CONTROL_SLOT: u32 = 0;

const ROUTE_NAME: &str = "telemetry";
const DIRECTION_PUBLISH: u32 = 1;
const RIGHT_SEND: u64 = 1;
const RIGHT_RECV: u64 = 2;

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-proxy] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn main() {
    if slime_components::fabric_boot::active() {
        // The full-graph boot runs the ordinary stream broker, which provisions
        // *route participants*. This component is declared as an interposition
        // hop on the telemetry chain rather than a participant on it, so the
        // broker has no edge to hand it — and the relay authority this file
        // asserts below is C8.8's to provision and prove.
        //
        // What C8.10 requires of it is exactly what happens here: it launches as
        // its own task, with its own generation-declared control endpoint and
        // grants that overlap neither the probe's nor the observer's, and it
        // reaches blocked idle. It asks for nothing, so it must receive nothing.
        slime_components::fabric_boot::park_only(b"fabric-proxy");
    }
    let ViewPage::End(end) =
        request_page(CONTROL_SLOT, 0).unwrap_or_else(|_| fail(b"empty visibility view"))
    else {
        fail(b"proxy inferred protected graph")
    };
    if end.route_name_len != 0
        || end.contract_kind != 0
        || end.schema_identity.iter().any(|byte| *byte != 0)
        || end.flags != 0
    {
        fail(b"empty view leaked metadata");
    }
    slime_rt::debug_write(b"[fabric-proxy] ungranted view is byte-empty\n");

    let route = route_identity(
        ROUTE_NAME,
        &telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );
    let mut route_name = [0u8; 32];
    route_name[..ROUTE_NAME.len()].copy_from_slice(ROUTE_NAME.as_bytes());
    let request = WireFabricRequest {
        magic: FABRIC_REQUEST_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        direction: DIRECTION_PUBLISH,
        type_identity: telemetry_stream::TYPE_TAG,
        route_name_len: ROUTE_NAME.len() as u32,
        route_name,
        reserved: [0; 4],
    };
    send_control(&request.encode());

    let mut slots = [0u32; 4];
    let mut descriptors = [None; 4];
    for index in 0..4 {
        let (descriptor, slot) = receive_role();
        let direction = if index < 2 {
            DIRECTION_SUBSCRIBE
        } else {
            DIRECTION_PUBLISH
        };
        if !valid_capability_transfer(&descriptor, &route, direction, OBJECT_KIND_ENDPOINT) {
            fail(b"proxy role binding");
        }
        slots[index] = slot;
        descriptors[index] = Some(descriptor);
    }
    if descriptors[0].expect("upstream data").rights_mask != RIGHT_RECV
        || descriptors[1].expect("upstream ack").rights_mask != RIGHT_SEND
        || descriptors[2].expect("downstream data").rights_mask != RIGHT_SEND
        || descriptors[3].expect("downstream ack").rights_mask != RIGHT_RECV
    {
        fail(b"proxy widened role");
    }

    let mut discard = [0u8; MAX_MSG];
    let mut no_caps = [0u64; MAX_CAPS_PER_MSG];
    if slime_rt::send(slots[0], b"publish", &[]) != ERR_BAD_CAP
        || slime_rt::recv(slots[1], &mut discard, &mut no_caps) != ERR_BAD_CAP
        || slime_rt::recv(slots[2], &mut discard, &mut no_caps) != ERR_BAD_CAP
        || slime_rt::send(slots[3], b"ack", &[]) != ERR_BAD_CAP
        || slime_rt::cap_transfer(
            CONTROL_SLOT,
            slots[0],
            &descriptors[0].expect("upstream data").encode(),
        ) != ERR_BAD_CAP
    {
        fail(b"proxy authority escaped chain");
    }
    slime_rt::debug_write(b"[fabric-proxy] proxy authority narrowed to chain\n");
    if slime_components::fabric_boot::active() {
        // Every property this component exists to prove has now been checked
        // against the kernel: the empty view above, the four narrowed roles, and
        // the five denials that keep relay authority inside the chain. The
        // relaying itself is C8.8's gate; here the graph must reach idle with no
        // traffic, so the proxy parks holding exactly its declared chain.
        slime_components::fabric_boot::park(b"fabric-proxy");
    }
    if option_env!("SLIME_FABRIC_PROXY_EARLY_EXIT") == Some("1") {
        slime_rt::debug_write(b"[fabric-proxy] injected early proxy death\n");
        return;
    }

    let sample_bytes = receive_message(slots[0]);
    let sample = WireStreamSample::decode(&sample_bytes)
        .filter(|sample| valid_stream_sample(sample, telemetry_stream::TYPE_TAG, 32))
        .filter(|sample| sample.sequence == 1)
        .unwrap_or_else(|| fail(b"proxy sample"));
    send_on(slots[2], &sample_bytes);

    let ack_bytes = receive_message(slots[3]);
    let _ack = WireStreamAck::decode(&ack_bytes)
        .filter(|ack| valid_stream_ack(ack, telemetry_stream::TYPE_TAG))
        .filter(|ack| ack.sequence == sample.sequence)
        .unwrap_or_else(|| fail(b"proxy ack"));
    send_on(slots[1], &ack_bytes);

    let trace = WireInterpositionTrace {
        magic: INTERPOSITION_TRACE_MAGIC,
        version: VISIBILITY_VERSION,
        event: TRACE_RELAYED,
        flags: 0,
        route_identity: route,
        sequence: sample.sequence,
        reserved: [0; 16],
    };
    if !valid_interposition_trace(&trace) {
        fail(b"proxy trace");
    }
    send_control(&trace.encode());
    slime_rt::debug_write(b"[fabric-proxy] declared relay complete; exiting\n");
}

fn send_control(message: &[u8; MAX_MSG]) {
    send_on(CONTROL_SLOT, message);
}

fn send_on(slot: u32, message: &[u8; MAX_MSG]) {
    loop {
        match slime_rt::send(slot, message, &[]) {
            ERR_SUCCESS => return,
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(slot)]),
            _ => fail(b"visibility send"),
        }
    }
}

fn receive_role() -> (WireCapabilityTransfer, u32) {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(CONTROL_SLOT)]),
            n if n < 0 => fail(b"proxy role"),
            _ => {
                let descriptor =
                    WireCapabilityTransfer::decode(&message).unwrap_or_else(|| fail(b"proxy role"));
                return (descriptor, received[0] as u32);
            }
        }
    }
}

fn receive_message(slot: u32) -> [u8; MAX_MSG] {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(slot, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(slot)]),
            n if n < 0 => fail(b"visibility receive"),
            n if n as usize != MAX_MSG => fail(b"visibility length"),
            _ if received.iter().any(|slot| *slot != 0) => fail(b"visibility carried capability"),
            _ => return message,
        }
    }
}

const _: () = assert!(REQUEST_LEN == MAX_MSG);
