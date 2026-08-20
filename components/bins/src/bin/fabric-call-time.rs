#![no_std]
#![no_main]

#[path = "../fabric_call_scenario.rs"]
mod scenario;

use slime_rt::{ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    // The boot plane declares this component but gives it no work, and the
    // discriminator is the build profile rather than a startup argument: the
    // root delivers a nonzero action only to the bootstrap instance, so every
    // participant on every plane read zero and parked.
    if slime_components::fabric_boot::active() {
        slime_components::fabric_boot::park_only(b"fabric-call-time");
    }
    // This binary's badge bit on the broker's wake notification.
    scenario::resolve_wake_slot();
    let mut phases = PhaseBuffer::new(None);
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
            // A barrier is all this component is waiting on, and the signaller
            // blocks in `send`: polling here would leave both sides waiting.
            match slime_rt::recv_blocking(1, &mut bytes, &mut caps) {
                ERR_WOULDBLOCK => slime_rt::yield_now(),
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
