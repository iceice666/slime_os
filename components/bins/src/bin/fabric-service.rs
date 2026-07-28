#![no_std]
#![no_main]

//! C8.3 fabric control plane: attenuated endpoint provisioning.
//!
//! A userspace service that owns every route endpoint in the
//! generation's data fabric and hands each participant exactly one
//! non-transferable role capability. The kernel supplies one generic mechanism
//! — `SYS_CAP_TRANSFER`, a bounded narrow-on-transfer move — and knows nothing
//! of routes, schemas, or graph roles; all of that policy lives here.
//!
//! Three properties this service exists to make true:
//!
//! 1. **A role is one direction.** A publisher's endpoint carries `RIGHT_SEND`
//!    and nothing else; a subscriber's carries `RIGHT_RECV`. The two halves of
//!    a route are separate kernel endpoints, so a publisher cannot receive on
//!    its route even by misusing the capability it holds.
//! 2. **A provisioned endpoint is terminal.** Every move omits
//!    `RIGHT_TRANSFER`, so a participant cannot re-delegate its role or mint a
//!    downstream edge. Non-delegability is enforced by the kernel at the moment
//!    of transfer, not by convention afterwards.
//! 3. **Names grant nothing.** A client is authenticated by the
//!    generation-provisioned control endpoint its request arrived on — the
//!    binding init established at spawn — never by the route name, direction,
//!    or type identity the request carries. Those fields are read and ignored:
//!    the answer comes from the graph table, keyed by the caller's identity.
//!
//! The service sweeps every control endpoint with the non-blocking `recv` ABI
//! and, only once all of them return `ERR_WOULDBLOCK`, parks in `SYS_WAIT`
//! across the whole set. It consumes no CPU while idle and never polls.
//!
//! **Lifetime.** This service runs one provisioning round and exits, rather
//! than living for the generation. That is deliberate at C8.3: provisioning is
//! the whole of the control plane so far, a route role is minted exactly once
//! (`Client::answered`), and a bounded run is what lets the gate assert both
//! declared directions were claimed before teardown. Exiting cleanly is
//! therefore success, and the kernel's `on_idle` treats it as such rather than
//! listing it persistent. C8.4 gives the service ongoing work — brokering
//! samples across the routes it provisioned — and the loop becomes unbounded
//! then. The parking behaviour under test here is identical either way.

use boot_contracts::fabric_graph::{
    CONTRACT_KIND_STREAM, DIRECTION_PUBLISH, DIRECTION_SUBSCRIBE, route_identity,
};
use slime_proto::capability_transfer::{
    CAPABILITY_TRANSFER_MAGIC, FORMAT_VERSION, OBJECT_KIND_ENDPOINT, REQUEST_LEN, TRANSFER_LEN,
    WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::interface_schema::telemetry_stream;
use slime_proto::valid_fabric_request;
use slime_rt::{
    ERR_PEER_DEAD, ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, Rights, WaitSource,
};

slime_rt::entry!(main);

include!(concat!(env!("OUT_DIR"), "/fabric_profile.rs"));

/// `EndpointFactory`, granted by the generation. The fabric mints both halves
/// of every route through it; no participant holds one.
const FACTORY_SLOT: u32 = 0;
/// Control endpoints, one per client, in the order init granted them. The slot
/// a request arrives on *is* the caller's identity: init bound each to exactly
/// one component at spawn, and no component can forge or re-derive one.
const PUBLISHER_CONTROL_SLOT: u32 = 1;
const SUBSCRIBER_CONTROL_SLOT: u32 = 2;
const INTRUDER_CONTROL_SLOT: u32 = 3;

const RIGHT_SEND: Rights = 1;
const RIGHT_RECV: Rights = 2;

/// The route this generation's stream edge runs over. Folded at runtime with
/// the generated C8.1 interface identity so it cannot drift from the admitted
/// schema.
const ROUTE_NAME: &str = "telemetry";

/// Provisioning denial. Distinct from a malformed request so the transcript
/// shows *why* an edge was refused.
const STATUS_NOT_GRANTED: i32 = -1;
const STATUS_BAD_REQUEST: i32 = -2;

/// One client's control binding: the slot init gave the fabric for it, and the
/// component identity that slot authenticates.
struct Client {
    control_slot: u32,
    component: &'static [u8],
    /// Set once this control endpoint has been answered. A route role is minted
    /// once; a second request over the same endpoint is refused rather than
    /// silently issuing a duplicate edge.
    answered: bool,
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn main() {
    // Both halves of the route are minted here and never leave except as
    // narrowed, non-transferable moves. Holding both is what lets the fabric
    // give a publisher send authority without ever giving it receive authority.
    let (publish_side, subscribe_side) = match slime_rt::endpoint_create(FACTORY_SLOT) {
        Ok(pair) => pair,
        Err(_) => fail(b"route endpoints"),
    };
    slime_rt::debug_write(b"[fabric] route endpoints minted\n");

    let route = route_identity(
        ROUTE_NAME,
        &telemetry_stream::INTERFACE_IDENTITY,
        CONTRACT_KIND_STREAM,
    );

    let mut clients = [
        Client {
            control_slot: PUBLISHER_CONTROL_SLOT,
            component: b"fabric-publisher",
            answered: false,
        },
        Client {
            control_slot: SUBSCRIBER_CONTROL_SLOT,
            component: b"fabric-subscriber",
            answered: false,
        },
        Client {
            control_slot: INTRUDER_CONTROL_SLOT,
            component: b"fabric-intruder",
            answered: false,
        },
    ];

    // One capability per direction, consumed by the first grant that claims it.
    // A second publisher on this route would find the slot already moved, which
    // is the same answer the graph gives: one declared edge, one endpoint.
    let mut publish_capability = Some(publish_side);
    let mut subscribe_capability = Some(subscribe_side);

    let mut parked = false;
    while clients.iter().any(|client| !client.answered) {
        // Sweep every unanswered control endpoint through its non-blocking ABI
        // first. Only when all of them would block is parking correct: probing
        // before parking is what closes the lost-wakeup window.
        let mut progressed = false;
        for client in clients.iter_mut().filter(|client| !client.answered) {
            let mut message = [0u8; MAX_MSG];
            let mut received = [0u64; MAX_CAPS_PER_MSG];
            let control_slot = client.control_slot;
            let length = match slime_rt::recv(control_slot, &mut message, &mut received) {
                ERR_WOULDBLOCK => continue,
                ERR_PEER_DEAD => {
                    // A client that died before asking gets no edge, and its
                    // route capability stays with the fabric.
                    slime_rt::debug_write(b"[fabric] control peer died: ");
                    slime_rt::debug_write(client.component);
                    slime_rt::debug_write(b"\n");
                    client.answered = true;
                    progressed = true;
                    continue;
                }
                n if n < 0 => fail(b"control recv"),
                n => n as usize,
            };
            progressed = true;
            client.answered = true;
            // A provisioning request carries no capabilities. One that does is
            // malformed, and its caps are released rather than retained.
            for slot in received.iter().filter(|slot| **slot != 0) {
                let _ = slime_rt::cap_drop(*slot as u32);
            }

            let request = match WireFabricRequest::decode(&message[..length.min(MAX_MSG)]) {
                Some(request) if length == REQUEST_LEN && valid_fabric_request(&request) => request,
                _ => {
                    deny(control_slot, &route, STATUS_BAD_REQUEST);
                    continue;
                }
            };

            // The request's own route name, direction, and type identity are
            // read here only to be discarded. Authority comes from the caller's
            // control endpoint and the generation graph, so a component
            // supplying the exact strings of a route it was never granted gets
            // the same answer as one supplying nothing.
            let _ = (request.direction, request.type_identity, request.route_name);

            let Some(direction) = declared_direction(client.component) else {
                slime_rt::debug_write(b"[fabric] ungranted component denied: ");
                slime_rt::debug_write(client.component);
                slime_rt::debug_write(b"\n");
                deny(control_slot, &route, STATUS_NOT_GRANTED);
                continue;
            };

            // The role decides the rights, and the rights are one direction only.
            let (capability, rights) = match direction {
                DIRECTION_PUBLISH => (&mut publish_capability, RIGHT_SEND),
                DIRECTION_SUBSCRIBE => (&mut subscribe_capability, RIGHT_RECV),
                _ => {
                    deny(control_slot, &route, STATUS_NOT_GRANTED);
                    continue;
                }
            };
            let Some(slot) = capability.take() else {
                deny(control_slot, &route, STATUS_NOT_GRANTED);
                continue;
            };

            // The descriptor states exactly what the kernel is about to
            // install. `RIGHT_TRANSFER` is absent from the mask and
            // `FLAG_RETAIN_TRANSFER` is unset, so the destination receives a
            // role it cannot re-delegate.
            let descriptor = WireCapabilityTransfer {
                magic: CAPABILITY_TRANSFER_MAGIC,
                version: FORMAT_VERSION,
                status: 0,
                flags: 0,
                object_kind: OBJECT_KIND_ENDPOINT,
                direction,
                rights_mask: rights,
                route_identity: route,
            };
            if slime_rt::cap_transfer(control_slot, slot, &descriptor.encode()) != ERR_SUCCESS {
                fail(b"provisioning transfer");
            }
            slime_rt::debug_write(b"[fabric] provisioned ");
            slime_rt::debug_write(client.component);
            slime_rt::debug_write(if direction == DIRECTION_PUBLISH {
                b" publish\n" as &[u8]
            } else {
                b" subscribe\n"
            });
        }
        if progressed {
            continue;
        }
        // Every source would block: park across the whole set at once. This is
        // the only place the service waits, and it burns no CPU doing it.
        if !parked {
            slime_rt::debug_write(b"[fabric] idle: parked on control endpoints\n");
            parked = true;
        }
        slime_rt::wait(&[
            WaitSource::Endpoint(PUBLISHER_CONTROL_SLOT),
            WaitSource::Endpoint(SUBSCRIBER_CONTROL_SLOT),
            WaitSource::Endpoint(INTRUDER_CONTROL_SLOT),
        ]);
    }

    // The fabric holds no route capability it did not hand out: both declared
    // directions were claimed by the components the graph names.
    if publish_capability.is_some() || subscribe_capability.is_some() {
        fail(b"a declared route endpoint was never provisioned");
    }
    slime_rt::debug_write(b"[fabric] control plane complete\n");
}

/// The direction the generation declared for `component` on [`ROUTE_NAME`], or
/// `None` when the graph declares no such edge. Deny by default: authority is
/// never ambient, so absence from the table is a denial, not a default role.
fn declared_direction(component: &[u8]) -> Option<u32> {
    FABRIC_PARTICIPANTS
        .iter()
        .find(|(name, route, _, _)| *name == component && *route == ROUTE_NAME)
        .map(|(_, _, _, direction)| *direction)
}

/// Answer a request the graph does not authorize. A denial is the same record
/// with a nonzero status, an empty rights mask, and no capability attached: the
/// caller learns it was refused without learning anything about the route.
fn deny(control_slot: u32, route: &[u8; 32], status: i32) {
    let descriptor = WireCapabilityTransfer {
        magic: CAPABILITY_TRANSFER_MAGIC,
        version: FORMAT_VERSION,
        status,
        flags: 0,
        object_kind: 0,
        direction: 0,
        rights_mask: 0,
        route_identity: *route,
    };
    let encoded = descriptor.encode();
    loop {
        match slime_rt::send(control_slot, &encoded, &[]) {
            ERR_SUCCESS => return,
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(control_slot)]),
            _ => fail(b"deny reply"),
        }
    }
}

const _: () = assert!(REQUEST_LEN == MAX_MSG);
const _: () = assert!(TRANSFER_LEN == MAX_MSG);
