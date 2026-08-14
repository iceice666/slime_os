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

use slime_rt::{ERR_PEER_DEAD, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

/// The fabric control endpoint this service drives the clock over.
const CONTROL_SLOT: u32 = 0;
/// Its half of the phase channel client A signals on.
const PHASE_SLOT: u32 = 1;

fn main(_startup_arg: u32) {
    // The boot plane declares this component but gives it no work, and the
    // discriminator is the build profile rather than a startup argument: the
    // root delivers a nonzero action only to the bootstrap instance, so every
    // participant on every plane read zero and parked.
    if slime_components::fabric_boot::active() {
        slime_components::fabric_boot::park_only(b"fabric-op-time");
    }
    loop {
        match try_wait_phase(3) {
            Some(()) => {
                scenario::send_time(CONTROL_SLOT, 4_000_100);
                slime_rt::debug_write(b"[fabric-op-time] bounded time advanced\n");
                return;
            }
            None => slime_rt::yield_now(),
        }
    }
}

fn try_wait_phase(expected: u8) -> Option<()> {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    match slime_rt::recv(PHASE_SLOT, &mut bytes, &mut caps) {
        ERR_WOULDBLOCK => None,
        ERR_PEER_DEAD => scenario::fail(b"time phase peer died"),
        value if value < 0 => scenario::fail(b"time phase receive"),
        1 if bytes[0] == expected => Some(()),
        _ => scenario::fail(b"time phase mismatch"),
    }
}
