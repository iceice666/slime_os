//! C8.10 full-graph boot arm: what a declared fabric participant does when one
//! generation launches the whole graph at once.
//!
//! The full-graph gate asserts that every declared role is *provisioned* and
//! that the graph then reaches healthy blocked idle **with no traffic**. So a
//! participant here asks for its declared role, proves the capability it
//! received is the narrowed one the graph named, and parks. It publishes
//! nothing, acks nothing, and calls nothing: any sample would make the boot's
//! idle condition depend on a data path C8.4-C8.8 already own, and a
//! regression there would surface as a failure of this gate instead of theirs.
//!
//! Each participant keeps its own identity and its own generation-provisioned
//! control endpoint. This module removes the per-scenario traffic, never the
//! authority checks: the role a participant accepts here is still verified
//! against the exact (route, direction, object kind) tuple the graph declared,
//! so a boot that widened or mis-bound a role fails rather than idling.

#![allow(dead_code)]

use slime_proto::capability_transfer::{
    CAPABILITY_TRANSFER_MAGIC, FABRIC_REQUEST_MAGIC, FORMAT_VERSION,
    OBJECT_KIND_SHARED_BUFFER_LOAN, WireCapabilityTransfer, WireFabricRequest,
};
use slime_proto::valid_capability_transfer;
use slime_rt::{ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};
mod generation_profile {
    include!(concat!(env!("OUT_DIR"), "/fabric_profile.rs"));
}

/// Control endpoint to this participant's route worker. Init binds it to
/// exactly one component at spawn, so it is also this participant's identity.
const CONTROL_SLOT: u32 = 0;

/// Whether the authenticated generation declares the full-graph boot action.
pub fn active() -> bool {
    generation_profile::GENERATION_BOOT_ACTION == "boot"
}

fn fail(name: &[u8], reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[");
    slime_rt::debug_write(name);
    slime_rt::debug_write(b"] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

/// Ask for this participant's declared role, verify every capability the worker
/// moves back, and park.
///
/// `roles` is how many narrowed capabilities the graph declares for this edge:
/// one for every C8.10 boot participant this module serves. A v2 shared ring
/// carries a route's data and credit in one region, so one edge is one
/// `capability_delegate` on the worker side regardless of direction — every
/// caller passes `1`. Receiving a different number is a provisioning defect,
/// so the count is asserted rather than drained: the worker and the
/// participant must agree from the same graph.
pub fn provision_and_park(
    name: &'static [u8],
    route_name: &str,
    route: &[u8; 32],
    type_tag: u64,
    direction: u32,
    roles: usize,
) -> ! {
    provision(name, route_name, type_tag, direction);
    accept_roles(name, &[(*route, direction, roles)]);
    park(name)
}

/// As [`provision_and_park`], for a participant the graph declares on more than
/// one route.
///
/// One request provisions every edge the graph declares for a component, so the
/// worker answers with all of them and each is checked against its own
/// (route, direction) pair. `edges` is `(route identity, direction, capability
/// count)` in the order the graph declares them, because the roles arrive in
/// that order — a participant and its worker read the same table.
pub fn provision_multi_and_park(
    name: &'static [u8],
    route_name: &str,
    type_tag: u64,
    direction: u32,
    edges: &[([u8; 32], u32, usize)],
) -> ! {
    provision(name, route_name, type_tag, direction);
    accept_roles(name, edges);
    park(name)
}

fn provision(name: &'static [u8], route_name: &str, type_tag: u64, direction: u32) {
    let mut route_bytes = [0u8; 32];
    if route_name.len() > route_bytes.len() {
        fail(name, b"boot route name");
    }
    route_bytes[..route_name.len()].copy_from_slice(route_name.as_bytes());
    let request = WireFabricRequest {
        magic: FABRIC_REQUEST_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        direction,
        type_identity: type_tag,
        route_name_len: route_name.len() as u32,
        route_name: route_bytes,
        reserved: [0; 4],
    };
    let encoded = request.encode();
    if slime_rt::send(CONTROL_SLOT, &encoded, &[]) != ERR_SUCCESS {
        fail(name, b"boot role request");
    }
}

fn accept_roles(name: &'static [u8], edges: &[([u8; 32], u32, usize)]) {
    for (route, direction, roles) in edges {
        for _ in 0..*roles {
            let descriptor = receive_role(name);
            if descriptor.status != 0 {
                fail(name, b"declared participant was denied");
            }
            if !valid_capability_transfer(
                &descriptor,
                route,
                *direction,
                OBJECT_KIND_SHARED_BUFFER_LOAN,
            ) {
                fail(name, b"boot role binding");
            }
        }
    }
    slime_rt::debug_write(b"[");
    slime_rt::debug_write(name);
    slime_rt::debug_write(b"] boot role provisioned\n");
}

/// Block forever on the control endpoint.
///
/// This is the boot gate's healthy end state: the ready queue drains, `on_idle`
/// finds the task `Blocked`, and the generation is idle rather than finished.
///
/// An unexpected message is drained rather than ignored. Parking on a ready
/// endpoint would return immediately and turn this into a spin, which would
/// look like a hang instead of the bounded idle the gate asserts.
pub fn park(name: &'static [u8]) -> ! {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => fail(name, b"boot idle control"),
            _ => {
                for slot in received.iter().filter(|slot| **slot != 0) {
                    let _ = slime_rt::cap_drop(*slot as u32);
                }
                slime_rt::debug_write(b"[");
                slime_rt::debug_write(name);
                slime_rt::debug_write(b"] boot idle drained a message\n");
            }
        }
    }
}

/// Park without asking for anything.
///
/// For a component the graph declares but the boot generation gives no work: it
/// must exist as its own task with its own grants, and must not invent traffic
/// to prove it.
pub fn park_only(name: &'static [u8]) -> ! {
    slime_rt::debug_write(b"[");
    slime_rt::debug_write(name);
    slime_rt::debug_write(b"] boot idle without a role\n");
    park(name)
}

/// Read the next role-grant reply on the control endpoint, skipping anything
/// else.
///
/// A worker's control endpoint carries more than role grants: provisioning one
/// edge can immediately satisfy a match and emit a QoS event on this same
/// endpoint (`fabric-service::refresh_matches`), interleaved with this
/// component's own remaining role-grant replies. `WireCapabilityTransfer::decode`
/// performs no magic check of its own — it is a fixed-offset reader, not a
/// discriminated union — so a caller that decoded every message unconditionally
/// would misread a same-sized QoS or stream event as a capability transfer with
/// garbage `status`/`route_identity`. Filed here rather than in `decode` because
/// every *other* reader of these bytes (the fabric replying to a bad request,
/// `deny`'s caller) already knows the message is this format from context; this
/// is the one path sharing an endpoint with more than one record kind.
fn receive_role(name: &'static [u8]) -> WireCapabilityTransfer {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => fail(name, b"boot role reply"),
            _ => {
                let descriptor = WireCapabilityTransfer::decode(&message)
                    .unwrap_or_else(|| fail(name, b"boot role decode"));
                if descriptor.magic == CAPABILITY_TRANSFER_MAGIC {
                    return descriptor;
                }
                // Not a role reply. No other record kind carries a capability,
                // so nothing here is ever exported — but drop defensively rather
                // than assume, exactly as `park`'s drain does.
                for slot in received.iter().filter(|slot| **slot != 0) {
                    let _ = slime_rt::cap_drop(*slot as u32);
                }
            }
        }
    }
}
