#![no_std]
#![no_main]

#[path = "../fabric_call_scenario.rs"]
mod scenario;

use slime_rt::{ERR_BAD_CAP, ERR_PEER_DEAD, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, WaitSource};

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    // A generation-launched copy has only control slot 0. The seL4 call driver
    // later spawns a second copy with the runtime-minted phase end at slot 1.
    // Probe the authority itself rather than guessing from an env flag or the
    // manifest-derived layout, neither of which distinguishes the two tasks.
    // If phase 1 is already queued, this probe is also its receive; do not drop
    // the byte and then wait forever for a marker that was consumed here.
    let mut probe = [0u8; MAX_MSG];
    let mut probe_caps = [0u64; MAX_CAPS_PER_MSG];
    let early_phase = match slime_rt::recv(1, &mut probe, &mut probe_caps) {
        ERR_BAD_CAP => slime_components::fabric_boot::park_only(b"fabric-call-time"),
        ERR_WOULDBLOCK => None,
        ERR_PEER_DEAD => scenario::fail(b"time phase peer died"),
        value if value < 0 => scenario::fail(b"time phase receive"),
        1 => Some(probe[0]),
        _ => scenario::fail(b"time phase mismatch"),
    };
    let mut phases = PhaseBuffer::new(early_phase);
    phases.wait(1);
    scenario::send_time(0, 1_000_025);
    phases.wait(2);
    scenario::send_time(0, 2_000_050);
    // Phase 3 is a completion barrier sent only after client A has observed the
    // peer-death terminal. It must not advance the clock.
    phases.wait(3);
    slime_rt::debug_write(b"[fabric-call-time] bounded time completed\n");
}

struct PhaseBuffer {
    ready: u8,
}

impl PhaseBuffer {
    const fn new(buffered: Option<u8>) -> Self {
        Self {
            ready: match buffered {
                Some(phase @ 1..=3) => 1 << phase,
                _ => 0,
            },
        }
    }

    fn wait(&mut self, expected: u8) {
        let expected_bit = 1 << expected;
        if self.ready & expected_bit != 0 {
            self.ready &= !expected_bit;
            return;
        }
        let mut bytes = [0u8; MAX_MSG];
        let mut caps = [0u64; MAX_CAPS_PER_MSG];
        loop {
            match slime_rt::recv(1, &mut bytes, &mut caps) {
                ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(1)]),
                ERR_PEER_DEAD => scenario::fail(b"time phase peer died"),
                value if value < 0 => scenario::fail(b"time phase receive"),
                1 if (1..=3).contains(&bytes[0]) => {
                    let bit = 1 << bytes[0];
                    if bit == expected_bit {
                        return;
                    }
                    self.ready |= bit;
                }
                _ => scenario::fail(b"time phase mismatch"),
            }
        }
    }
}
