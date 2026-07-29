#![no_std]
#![no_main]

#[path = "../fabric_call_scenario.rs"]
mod scenario;

use slime_rt::{ERR_PEER_DEAD, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, WaitSource};

slime_rt::entry!(main);

fn main() {
    for phase in 1..=3 {
        wait_phase(phase);
        let now_ns = match phase {
            1 => 1_000_025,
            2 => 2_000_050,
            _ => 3_000_075,
        };
        scenario::send_time(0, now_ns);
    }
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
