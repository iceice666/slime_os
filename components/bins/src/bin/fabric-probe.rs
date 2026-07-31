#![no_std]
#![no_main]

//! C8.10 unauthorized probe: a component the generation graph declares no edge
//! for, running alongside every authorized plane in one generation.
//!
//! It holds a real, generation-provisioned control endpoint to the fabric — so
//! it is not blocked by lacking a channel — and it supplies the *exact* route
//! name, direction, and type identity the publisher supplies. Everything a
//! naive registry would accept is present.
//!
//! It must still receive nothing. This is the concrete form of "possession of
//! names or generic channel authority cannot mint a graph edge": the fabric
//! authenticates by the control endpoint, and the graph declares no participant
//! for this component, so the request is denied with no capability attached.
//!
//! Distinct from [`fabric-proxy`] and [`fabric-observer`] as its own task with
//! non-overlapping grants: the milestone requires the probe, the declared
//! interposition proxy, and the filtered-introspection client to be three
//! separate identities, so a denial here can never be confused for a role the
//! graph granted somewhere else.

use slime_proto::capability_transfer::{
    FABRIC_REQUEST_MAGIC, FORMAT_VERSION, REQUEST_LEN, WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::interface_schema::telemetry_stream;
use slime_rt::{ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, WaitSource};

slime_rt::entry!(main);

const CONTROL_SLOT: u32 = 0;

/// Byte-for-byte the publisher's ask, including the direction it wants.
const ROUTE_NAME: &str = "telemetry";
const DIRECTION_PUBLISH: u32 = 1;

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-probe] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn main() {
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
    let encoded = request.encode();
    loop {
        match slime_rt::send(CONTROL_SLOT, &encoded, &[]) {
            ERR_SUCCESS => break,
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(CONTROL_SLOT)]),
            _ => fail(b"request"),
        }
    }
    slime_rt::debug_write(b"[fabric-probe] exact route strings supplied\n");

    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(CONTROL_SLOT)]),
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
    slime_rt::debug_write(b"[fabric-probe] undeclared edge denied\n");
    slime_rt::debug_write(b"[fabric-probe] done\n");
    if slime_components::fabric_boot::active() {
        // The denial above is this component's whole assertion, and it holds in
        // the full-graph boot exactly as it does alone: a real control endpoint
        // and the exact route strings still buy nothing. Park rather than exit,
        // because the gate's exit condition is every role blocked — a probe that
        // terminated would be indistinguishable from one that was never
        // launched.
        slime_components::fabric_boot::park(b"fabric-probe");
    }
}

const _: () = assert!(REQUEST_LEN == MAX_MSG);
