#![no_std]
#![no_main]

//! The C8.7 operation plane's monotonic-time input.
//!
//! Time reaches the fabric only through this capability-routed record, never a
//! poll or a kernel clock, so every expiry and deadline transition in the gate
//! happens at an instant the transcript can name. The component advances the
//! clock one step per phase signal from client A and then exits, which is what
//! makes "the result expired" and "the goal timed out" orderable observations
//! rather than a race.

#[path = "../fabric_operation_scenario.rs"]
mod scenario;

use slime_rt::{ERR_BAD_CAP, ERR_PEER_DEAD, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

/// The fabric control endpoint this service drives the clock over.
const CONTROL_SLOT: u32 = 0;
/// Its half of the phase channel client A signals on.
const PHASE_SLOT: u32 = 1;

fn main(_startup_arg: u32) {
    if slime_components::fabric_boot::active() {
        // The full-graph boot carries no phase channel: phases exist to order a
        // scenario's transcript, and the boot gate runs no scenario. The clock
        // still launches as its own declared identity holding only its control
        // endpoint, and parks — advancing time here would drive expiry in a
        // graph that is supposed to be at rest.
        slime_components::fabric_boot::park_only(b"fabric-op-time");
    }
    loop {
        match try_wait_phase(3) {
            Some(()) => {
                scenario::send_time(CONTROL_SLOT, 4_000_100);
                slime_rt::debug_write(b"[fabric-op-time] bounded time advanced\n");
                return;
            }
            None => slime_rt::wait(&[slime_rt::WaitSource::Endpoint(PHASE_SLOT)]),
        }
    }
}

fn try_wait_phase(expected: u8) -> Option<()> {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    match slime_rt::recv(PHASE_SLOT, &mut bytes, &mut caps) {
        ERR_WOULDBLOCK => None,
        // No phase channel at all. The seL4 root launches every component the
        // generation declares, so this boot also starts one *unconfigured*
        // instance that `init` never spawned; only the spawned copy holds the
        // runtime-minted phase end. Probing the authority distinguishes the two
        // — neither an env flag nor the manifest-derived layout can, since both
        // tasks are built from the same image and the same generation.
        ERR_BAD_CAP => slime_components::fabric_boot::park_only(b"fabric-op-time"),
        ERR_PEER_DEAD => scenario::fail(b"time phase peer died"),
        value if value < 0 => scenario::fail(b"time phase receive"),
        1 if bytes[0] == expected => Some(()),
        _ => scenario::fail(b"time phase mismatch"),
    }
}
