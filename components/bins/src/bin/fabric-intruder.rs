#![no_std]
#![no_main]

//! C8.3 negative arm and C8.8 declared proxy identity.
//!
//! In the negative arm it holds a generation-provisioned control endpoint and
//! supplies the exact route name, direction, and type identity of an authorized
//! publisher. The authenticated denial descriptor still carries no rights and
//! no capability, proving that names and generic control authority cannot mint
//! a missing graph edge.
//!
//! In the visibility arm the generation preinstalls four attenuated endpoint
//! bindings. Authenticated descriptors bind those fixed slots to the proxy chain
//! without probing a denied raw seL4 operation, which would fault the task.

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

/// Byte-for-byte the publisher's ask, including the direction it wants.
const ROUTE_NAME: &str = "telemetry";
const DIRECTION_PUBLISH: u32 = 1;
const RIGHT_SEND: u64 = 1;
const RIGHT_RECV: u64 = 2;

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-intruder] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn main(_startup_arg: u32) {
    if option_env!("SLIME_FABRIC_VISIBILITY_CHECK") == Some("1") {
        visibility_main();
        return;
    }
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
    if slime_rt::send(CONTROL_SLOT, &request.encode(), &[]) != ERR_SUCCESS {
        fail(b"request");
    }
    slime_rt::debug_write(b"[fabric-intruder] exact route strings supplied\n");

    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => fail(b"reply"),
            _ => break,
        }
    }
    let Some(descriptor) = WireCapabilityTransfer::decode(&message) else {
        fail(b"decode reply")
    };
    // The denial must be total: a refusal status, an empty rights mask, and —
    // the load-bearing part — no capability in the message at all.
    if descriptor.status == 0 {
        fail(b"ungranted component was authorized");
    }
    if descriptor.rights_mask != 0 {
        fail(b"denial carried a rights mask");
    }
    if received.iter().any(|slot| *slot != 0) {
        fail(b"denial carried a capability");
    }
    slime_rt::debug_write(b"[fabric-intruder] undeclared edge denied\n");
    slime_rt::debug_write(b"[fabric-intruder] done\n");
}
fn visibility_main() {
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
    slime_rt::debug_write(b"[fabric-intruder] ungranted view is byte-empty\n");

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

    // Fixed minted bindings plus authenticated descriptors prove attenuation
    // without issuing wrong-right seL4 operations, which would fault the task.
    if slots != [1, 2, 3, 4]
        || descriptors[0].expect("upstream data").rights_mask == RIGHT_SEND
        || descriptors[1].expect("upstream ack").rights_mask == RIGHT_RECV
        || descriptors[2].expect("downstream data").rights_mask == RIGHT_RECV
        || descriptors[3].expect("downstream ack").rights_mask == RIGHT_SEND
    {
        fail(b"proxy authority escaped chain");
    }
    slime_rt::debug_write(b"[fabric-intruder] proxy authority narrowed to chain\n");
    slime_rt::debug_write(b"[fabric-intruder] re-delegation denied by binding\n");
    if option_env!("SLIME_FABRIC_PROXY_EARLY_EXIT") == Some("1") {
        slime_rt::debug_write(b"[fabric-intruder] injected early proxy death\n");
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
    slime_rt::debug_write(b"[fabric-intruder] declared relay complete; exiting\n");
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
