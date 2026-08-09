#![no_std]
#![no_main]

#[path = "../fabric_call_scenario.rs"]
mod scenario;

use slime_rt::{ERR_BAD_CAP, ERR_PEER_DEAD, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, WaitSource};

slime_rt::entry!(main);

fn main() {
    // A generation-launched copy has only control slot 0. The seL4 call driver
    // later spawns a second copy with the runtime-minted phase end at slot 1.
    // Probe the authority itself rather than guessing from an env flag or the
    // manifest-derived layout, neither of which distinguishes the two tasks.
    // If phase 1 is already queued, this probe is also its receive; do not drop
    // the byte and then wait forever for a marker that was consumed here.
    let mut probe = [0u8; MAX_MSG];
    let mut probe_caps = [0u64; MAX_CAPS_PER_MSG];
    let phase_one_ready = match slime_rt::recv(1, &mut probe, &mut probe_caps) {
        ERR_BAD_CAP => slime_components::fabric_boot::park_only(b"fabric-call-time"),
        ERR_WOULDBLOCK => false,
        ERR_PEER_DEAD => scenario::fail(b"time phase peer died"),
        value if value < 0 => scenario::fail(b"time phase receive"),
        1 if probe[0] == 1 => true,
        _ => scenario::fail(b"time phase mismatch"),
    };
    if !phase_one_ready {
        wait_phase(1);
    }
    scenario::send_time(0, 1_000_025);
    wait_phase(2);
    scenario::send_time(0, 2_000_050);
    // Phase 3 is a completion barrier sent only after client A has observed the
    // peer-death terminal. It must not advance the clock: doing so can time out
    // request 11 before the server dequeues the request that intentionally exits.
    wait_phase(3);
    slime_rt::debug_write(b"[fabric-call-time] bounded time advanced\n");
}

fn wait_phase(expected: u8) {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(1, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(1)]),
            ERR_PEER_DEAD => scenario::fail(b"time phase peer died"),
            value if value < 0 => scenario::fail(b"time phase receive"),
            1 if bytes[0] == expected => return,
            _ => scenario::fail(b"time phase mismatch"),
        }
    }
}
