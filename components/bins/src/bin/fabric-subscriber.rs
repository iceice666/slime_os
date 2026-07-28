#![no_std]
#![no_main]

//! C8.3 fabric subscriber: the receiving half of the attenuated route.
//!
//! Mirrors `fabric-publisher`. It receives a `RIGHT_RECV`-only endpoint through
//! the kernel's narrow-on-transfer move, proves it cannot publish on its own
//! route or re-delegate the role, and only then consumes the sample.

use boot_contracts::fabric_graph::{CONTRACT_KIND_STREAM, DIRECTION_SUBSCRIBE, route_identity};
use slime_proto::capability_transfer::{
    FABRIC_REQUEST_MAGIC, FORMAT_VERSION, OBJECT_KIND_ENDPOINT, REQUEST_LEN,
    WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::interface_schema::telemetry_stream;
use slime_proto::valid_capability_transfer;
use slime_rt::{ERR_BAD_CAP, ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, WaitSource};

slime_rt::entry!(main);

/// Control endpoint to the fabric — this component's only starting authority.
const CONTROL_SLOT: u32 = 0;

const RIGHT_RECV: u64 = 2;

const ROUTE_NAME: &str = "telemetry";

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-subscriber] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn main() {
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
        fail(b"request");
    }
    slime_rt::debug_write(b"[fabric-subscriber] role requested\n");

    let (descriptor, route_slot) = receive_role();
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
    if descriptor.rights_mask != RIGHT_RECV {
        fail(b"subscriber role carries more than receive authority");
    }
    slime_rt::debug_write(b"[fabric-subscriber] subscribe role received\n");

    // A subscriber has no publish authority on its own route, so it can never
    // inject a sample that its peers would read as coming from the publisher.
    if slime_rt::send(route_slot, b"forged", &[]) != ERR_BAD_CAP {
        fail(b"subscriber could publish on its route");
    }
    slime_rt::debug_write(b"[fabric-subscriber] route publish denied\n");

    // The role is terminal here too: it cannot be handed on.
    let redelegation = WireCapabilityTransfer {
        rights_mask: RIGHT_RECV,
        ..descriptor
    };
    if slime_rt::cap_transfer(CONTROL_SLOT, route_slot, &redelegation.encode()) != ERR_BAD_CAP {
        fail(b"subscriber re-delegated its route");
    }
    slime_rt::debug_write(b"[fabric-subscriber] re-delegation denied\n");

    let mut sample = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    let length = loop {
        match slime_rt::recv(route_slot, &mut sample, &mut received) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(route_slot)]),
            n if n < 0 => fail(b"receive sample"),
            n => break n as usize,
        }
    };
    if length != MAX_MSG {
        fail(b"sample is not one control message");
    }
    // The sample carries the admitted type's generation-local tag: a route
    // delivers exactly the interface the graph declared for it.
    let tag = u64::from_le_bytes(sample[..8].try_into().expect("sample header"));
    if tag != telemetry_stream::TYPE_TAG {
        fail(b"sample type tag does not match the route interface");
    }
    if sample[8..]
        .iter()
        .enumerate()
        .any(|(index, byte)| *byte != (index % 251) as u8)
    {
        fail(b"sample payload mismatch");
    }
    slime_rt::debug_write(b"[fabric-subscriber] sample received\n");
    slime_rt::debug_write(b"[fabric-subscriber] done\n");
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
