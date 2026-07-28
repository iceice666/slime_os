#![no_std]
#![no_main]

//! C8.3 fabric publisher: a participant that receives one attenuated route
//! role and proves it holds nothing more.
//!
//! Asks the fabric for its edge over the generation-provisioned control
//! endpoint, receives a `RIGHT_SEND`-only endpoint through the kernel's
//! narrow-on-transfer move, and then attempts every operation the role must
//! not permit. Each denial is asserted before the operation it guards, so a
//! regression that widens a route role fails here even when the happy path
//! still publishes.

use boot_contracts::fabric_graph::{CONTRACT_KIND_STREAM, DIRECTION_PUBLISH, route_identity};
use slime_proto::capability_transfer::{
    FABRIC_REQUEST_MAGIC, FORMAT_VERSION, OBJECT_KIND_ENDPOINT, REQUEST_LEN,
    WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::interface_schema::telemetry_stream;
use slime_proto::valid_capability_transfer;
use slime_rt::{ERR_BAD_CAP, ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, WaitSource};

slime_rt::entry!(main);

/// Control endpoint to the fabric. The only authority this component starts
/// with: it holds no factory, no route, and no peer endpoint.
const CONTROL_SLOT: u32 = 0;

const RIGHT_SEND: u64 = 1;
const RIGHT_RECV: u64 = 2;

const ROUTE_NAME: &str = "telemetry";

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-publisher] fail: ");
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

    // The request names the route and type it wants. None of it is authority:
    // the fabric answers from the generation graph keyed by this control
    // endpoint, so these fields only prove the ask was well-formed.
    let request = WireFabricRequest {
        magic: FABRIC_REQUEST_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        direction: DIRECTION_PUBLISH,
        type_identity: telemetry_stream::TYPE_TAG,
        route_name_len: ROUTE_NAME.len() as u32,
        route_name: route_name_bytes(),
        reserved: [0; 4],
    };
    if send_request(&request) != ERR_SUCCESS {
        fail(b"request");
    }
    slime_rt::debug_write(b"[fabric-publisher] role requested\n");

    let (descriptor, route_slot) = receive_role();
    if descriptor.status != 0 {
        fail(b"declared publisher was denied");
    }
    if !valid_capability_transfer(&descriptor, &route, DIRECTION_PUBLISH, OBJECT_KIND_ENDPOINT) {
        fail(b"descriptor does not name this role");
    }
    // The descriptor is the kernel's own record of what it installed, so an
    // endpoint arriving with receive or transfer authority would show here.
    if descriptor.rights_mask != RIGHT_SEND {
        fail(b"publisher role carries more than send authority");
    }
    slime_rt::debug_write(b"[fabric-publisher] publish role received\n");

    // A publisher has no receive authority on its own route: the fabric holds
    // the other half of the channel, and this half was narrowed to send.
    let mut discard = [0u8; MAX_MSG];
    let mut no_caps = [0u64; MAX_CAPS_PER_MSG];
    if slime_rt::recv(route_slot, &mut discard, &mut no_caps) != ERR_BAD_CAP {
        fail(b"publisher could receive on its route");
    }
    slime_rt::debug_write(b"[fabric-publisher] route receive denied\n");

    // A provisioned role is terminal. Re-transferring it — even back over the
    // control endpoint, even narrowing further — must fail: the move omitted
    // `RIGHT_TRANSFER`, so the kernel refuses before anything crosses.
    let redelegation = WireCapabilityTransfer {
        rights_mask: RIGHT_SEND,
        ..descriptor
    };
    if slime_rt::cap_transfer(CONTROL_SLOT, route_slot, &redelegation.encode()) != ERR_BAD_CAP {
        fail(b"publisher re-delegated its route");
    }
    slime_rt::debug_write(b"[fabric-publisher] re-delegation denied\n");

    // Nor can it widen its own role by asking the kernel for more than it
    // holds: `derive` inside the transfer path is narrow-only.
    let widened = WireCapabilityTransfer {
        rights_mask: RIGHT_SEND | RIGHT_RECV,
        ..descriptor
    };
    if slime_rt::cap_transfer(CONTROL_SLOT, route_slot, &widened.encode()) != ERR_BAD_CAP {
        fail(b"publisher widened its route rights");
    }
    slime_rt::debug_write(b"[fabric-publisher] widening denied\n");

    // With every denial observed, the role does what it is for.
    let sample = telemetry_sample();
    loop {
        match slime_rt::send(route_slot, &sample, &[]) {
            ERR_SUCCESS => break,
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(route_slot)]),
            _ => fail(b"publish"),
        }
    }
    slime_rt::debug_write(b"[fabric-publisher] sample published\n");
    slime_rt::debug_write(b"[fabric-publisher] done\n");
}

/// A bounded typed sample: the C8.1 type tag followed by a fixed payload. C8.4
/// defines the real stream framing; here the bytes only have to prove that an
/// attenuated route carries data end to end.
fn telemetry_sample() -> [u8; MAX_MSG] {
    let mut sample = [0u8; MAX_MSG];
    sample[..8].copy_from_slice(&telemetry_stream::TYPE_TAG.to_le_bytes());
    for (index, byte) in sample[8..].iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }
    sample
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

/// Block until the fabric answers, returning the descriptor and the slot the
/// moved capability landed in (zero when the answer carried none).
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
