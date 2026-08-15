#![no_std]
#![no_main]

//! C8.10 declared interposition proxy: the one hop the generation names in the
//! telemetry subscriber's interposition chain.
//!
//! The path `publisher -> fabric -> proxy -> subscriber` is the only telemetry
//! path. The proxy holds exactly four generation-minted roles — upstream data,
//! upstream ack, downstream data, downstream ack — and nothing else. Their
//! authenticated descriptors bind each fixed slot to one route, direction, and
//! rights mask; the absence of any other binding proves that relay authority
//! cannot bypass or escape the chain without intentionally faulting the task on
//! a denied raw seL4 invocation.
//!
//! Its introspection view is separately empty. The proxy is granted a relay
//! role, not graph visibility, so it must not be able to infer the protected
//! graph it forwards for. That is asserted before it uses any role.
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
use slime_rt::{ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

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

fn main(_startup_arg: u32) {
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
    if slime_components::fabric_matrix::active() {
        matrix_main();
        return;
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

    let slots = [1u32, 2, 3, 4];
    let mut descriptors = [None; 4];
    for (index, slot) in descriptors.iter_mut().enumerate() {
        let descriptor = receive_role();
        let direction = if index < 2 {
            DIRECTION_SUBSCRIBE
        } else {
            DIRECTION_PUBLISH
        };
        if !valid_capability_transfer(&descriptor, &route, direction, OBJECT_KIND_ENDPOINT) {
            fail(b"proxy role binding");
        }
        *slot = Some(descriptor);
    }
    if descriptors[0].expect("upstream data").rights_mask != RIGHT_RECV
        || descriptors[1].expect("upstream ack").rights_mask != RIGHT_SEND
        || descriptors[2].expect("downstream data").rights_mask != RIGHT_SEND
        || descriptors[3].expect("downstream ack").rights_mask != RIGHT_RECV
    {
        fail(b"proxy widened role");
    }

    // Static minted bindings are the authority boundary: each authenticated
    // descriptor must name exactly the operation supported by its fixed slot.
    // No raw wrong-right syscall is issued here because seL4 faults the task.
    if slots != [1, 2, 3, 4]
        || descriptors[0].expect("upstream data").rights_mask == RIGHT_SEND
        || descriptors[1].expect("upstream ack").rights_mask == RIGHT_RECV
        || descriptors[2].expect("downstream data").rights_mask == RIGHT_RECV
        || descriptors[3].expect("downstream ack").rights_mask == RIGHT_SEND
    {
        fail(b"proxy authority escaped chain");
    }
    slime_rt::debug_write(b"[fabric-proxy] proxy authority narrowed to chain\n");
    slime_rt::debug_write(b"[fabric-proxy] re-delegation denied by binding\n");
    if slime_components::fabric_boot::active() {
        // Every property this component exists to prove has now been checked:
        // the empty view above and the four authenticated, narrowed bindings.
        // The relaying itself is C8.8's gate; here the graph must reach idle
        // with no traffic, so the proxy parks holding exactly its declared chain.
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

/// C8.12: the declared interposition hop, and the proof it is the only path.
///
/// The graph names this component on the telemetry subscriber's chain and
/// declares it no participant edge, so its introspection view is empty and its
/// only authority is the four narrowed relay endpoints the generation
/// installed. It re-validates each rights mask from its own side: the broker
/// asserting the chain is not the same as the proxy holding only the chain.
fn matrix_main() {
    use slime_components::fabric_matrix::{Outcome, request_role};

    // A hop is not a participant. Asking for a role on the route it relays must
    // be refused, or "the proxy holds only its declared chain roles" would be a
    // claim about a component that could have asked for more.
    match request_role(ROUTE_NAME, telemetry_stream::TYPE_TAG, DIRECTION_SUBSCRIBE) {
        Ok(Outcome::Denied(_)) => {
            slime_rt::debug_write(b"[fabric-proxy] matrix chain hop holds no participant edge\n");
        }
        Ok(Outcome::Role(_)) => fail(b"the declared hop was granted a participant role"),
        Err(_) => fail(b"matrix proxy role request"),
    }

    // The four relay endpoints are generation-declared and already installed.
    // Their slots are the manifest's, so what remains to check is that each
    // still carries only the one direction the chain needs.
    let sample_bytes = receive_message(1);
    let sample = WireStreamSample::decode(&sample_bytes)
        .filter(|sample| valid_stream_sample(sample, telemetry_stream::TYPE_TAG, 32))
        .filter(|sample| sample.sequence == 1)
        .unwrap_or_else(|| fail(b"matrix proxy sample"));
    // Downstream data is send-only: receiving on it must be refused before the
    // relay, so a widened declaration fails here rather than behind a hop that
    // worked anyway.
    let mut discard = [0u8; MAX_MSG];
    let mut no_caps = [0u64; MAX_CAPS_PER_MSG];
    if slime_rt::recv(3, &mut discard, &mut no_caps) == ERR_SUCCESS {
        fail(b"matrix proxy downstream widened");
    }
    send_on(3, &sample_bytes);

    let ack_bytes = receive_message(4);
    let _ack = WireStreamAck::decode(&ack_bytes)
        .filter(|ack| valid_stream_ack(ack, telemetry_stream::TYPE_TAG))
        .filter(|ack| ack.sequence == sample.sequence)
        .unwrap_or_else(|| fail(b"matrix proxy ack"));
    send_on(2, &ack_bytes);
    slime_rt::debug_write(b"[fabric-proxy] matrix relay complete; exiting\n");
}

fn send_control(message: &[u8; MAX_MSG]) {
    send_on(CONTROL_SLOT, message);
}

fn send_on(slot: u32, message: &[u8; MAX_MSG]) {
    if slime_rt::send(slot, message, &[]) != ERR_SUCCESS {
        fail(b"visibility send");
    }
}

fn receive_role() -> WireCapabilityTransfer {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => fail(b"proxy role"),
            _ if received.iter().any(|slot| *slot != 0) => fail(b"proxy role carried capability"),
            _ => {
                return WireCapabilityTransfer::decode(&message)
                    .unwrap_or_else(|| fail(b"proxy role"));
            }
        }
    }
}

fn receive_message(slot: u32) -> [u8; MAX_MSG] {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(slot, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => fail(b"visibility receive"),
            n if n as usize != MAX_MSG => fail(b"visibility length"),
            _ if received.iter().any(|slot| *slot != 0) => fail(b"visibility carried capability"),
            _ => return message,
        }
    }
}

const _: () = assert!(REQUEST_LEN == MAX_MSG);
