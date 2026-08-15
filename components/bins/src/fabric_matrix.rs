//! Client half of the C8.12 matching, visibility, and denial matrix.
//!
//! Shared by every participant on the matrix plane, so the plane's protocol
//! lives in one place rather than being restated seven times with seven chances
//! to disagree. Each caller supplies only what distinguishes it: which route
//! name and type tag it asks under, and what it expects back.
//!
//! The three answers a request can receive are all admissible outcomes of the
//! matrix, not errors:
//!
//! * a **role**, when the caller's (component, route, type) tuple is exactly
//!   one the graph declares;
//! * a **denial**, when any part of that tuple disagrees — with no rights, no
//!   capability, and no route identity, so a refused caller learns only that it
//!   was refused;
//! * a **filtered view page**, from the read-only introspection cursor.
//!
//! A caller that asserts it was denied is asserting all three of those absences,
//! not merely a nonzero status. [`Outcome::Denied`] carries the status alone for
//! that reason: the rest is checked here, once, so no caller can forget to.

#![allow(dead_code)]

use slime_proto::capability_transfer::{
    CAPABILITY_TRANSFER_MAGIC, FABRIC_REQUEST_MAGIC, FORMAT_VERSION, REQUEST_LEN,
    WireCapabilityTransfer, WireFabricRequest,
};
use slime_rt::{ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

mod generation_profile {
    include!(concat!(env!("OUT_DIR"), "/fabric_profile.rs"));
}

/// Control endpoint to the matrix broker. Init binds it to exactly one
/// component at spawn, so it is also this participant's identity.
pub const CONTROL_SLOT: u32 = 0;

/// Whether the authenticated generation declares the matrix boot action.
pub fn active() -> bool {
    generation_profile::GENERATION_BOOT_ACTION == "matrix"
}

/// What the broker answered.
pub enum Outcome {
    /// One narrowed, non-delegable role on the requested route.
    Role(WireCapabilityTransfer),
    /// A refusal. The status names which disagreement refused it; every other
    /// field was already checked to be empty.
    Denied(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Transport,
    /// A reply that is neither a well-formed role nor a well-formed denial.
    /// Distinct from a denial: a malformed answer is a broker defect, while a
    /// denial is the plane working.
    InvalidRecord,
    /// A denial that carried something a denial must never carry.
    LeakyDenial,
}

/// Ask for a role on `route_name` under `type_identity`, and read the answer.
pub fn request_role(
    route_name: &str,
    type_identity: u64,
    direction: u32,
) -> Result<Outcome, Error> {
    if route_name.len() > 32 {
        return Err(Error::InvalidRecord);
    }
    let mut name = [0u8; 32];
    name[..route_name.len()].copy_from_slice(route_name.as_bytes());
    let request = WireFabricRequest {
        magic: FABRIC_REQUEST_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        direction,
        type_identity,
        route_name_len: route_name.len() as u32,
        route_name: name,
        reserved: [0; 4],
    };
    if slime_rt::send(CONTROL_SLOT, &request.encode(), &[]) != ERR_SUCCESS {
        return Err(Error::Transport);
    }
    receive_answer()
}

/// Read one answer from the control endpoint.
///
/// The capability check is the load-bearing one and is done here rather than at
/// each call site: a denial that arrived with a capability attached would be a
/// refusal that granted something, which no caller should have to remember to
/// look for.
fn receive_answer() -> Result<Outcome, Error> {
    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    let length = loop {
        match slime_rt::recv(CONTROL_SLOT, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => return Err(Error::Transport),
            n => break n as usize,
        }
    };
    let carried = received.iter().any(|slot| *slot != 0);
    for slot in received.into_iter().filter(|slot| *slot != 0) {
        let _ = slime_rt::cap_drop(slot as u32);
    }
    if length != MAX_MSG {
        return Err(Error::InvalidRecord);
    }
    let descriptor = WireCapabilityTransfer::decode(&message).ok_or(Error::InvalidRecord)?;
    if descriptor.magic != CAPABILITY_TRANSFER_MAGIC || descriptor.version != FORMAT_VERSION {
        return Err(Error::InvalidRecord);
    }
    if descriptor.status == 0 {
        return Ok(Outcome::Role(descriptor));
    }
    // Everything a denial must withhold, checked in one place. A refusal that
    // echoed the route would confirm the edge exists; one that carried rights or
    // a capability would not be a refusal at all.
    if carried
        || descriptor.rights_mask != 0
        || descriptor.object_kind != 0
        || descriptor.direction != 0
        || descriptor.route_identity.iter().any(|byte| *byte != 0)
    {
        return Err(Error::LeakyDenial);
    }
    Ok(Outcome::Denied(descriptor.status))
}

const _: () = assert!(REQUEST_LEN == MAX_MSG);
