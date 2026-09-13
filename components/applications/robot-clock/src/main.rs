#![no_std]
#![no_main]

//! C9.6's bounded simulated-time source for the robot call plane.
//!
//! Time advances only after the controller releases a declared endpoint
//! barrier. Advancing on startup was rejected because it can move past a
//! deadline before the deliberately unanswered request exists, producing no
//! timeout and therefore proving nothing about deadline handling.

use slime_proto::fabric_call::{CALL_TIME_MAGIC, FORMAT_VERSION, WireCallTimeAdvance};
use slime_rt::{ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

/// The preinstalled endpoint carrying simulated time to the call broker.
const ROUTE_SLOT: u32 = 0;
/// The controller-to-clock barrier endpoint.
const PHASE_SLOT: u32 = 1;
const FIRST_ADVANCE_NS: u64 = 500_000;
const SECOND_ADVANCE_NS: u64 = 1_000_001;
const WAKE_BINDING: &[u8] = b"notification:fabric-call-worker-parameters-ready";

fn main(_startup_arg: u32) {
    // Unlike a profile-generic helper that may tolerate an absent notification,
    // this clock is useful only in the robot call graph. Silently proceeding
    // without the declared edge would let the blocking send deadlock against a
    // parked broker, so the missing name is a hard graph failure (CP2/B70).
    let wake = slime_rt::resolve_binding(WAKE_BINDING)
        .unwrap_or_else(|_| fail(b"resolve notification:fabric-call-worker-parameters-ready"));
    let mut phases = PhaseBuffer::new(None);

    phases.wait(1);
    send_time(FIRST_ADVANCE_NS, wake);
    write_value(b"[robot-clock] advanced now_ns=", FIRST_ADVANCE_NS);

    phases.wait(2);
    send_time(SECOND_ADVANCE_NS, wake);
    write_value(b"[robot-clock] advanced now_ns=", SECOND_ADVANCE_NS);

    // Exiting is a protocol step, not cleanup. The broker first latches this
    // task's supervised termination and only then treats the drained time
    // endpoint as closed (B75). Waiting for `ERR_PEER_DEAD` was rejected because
    // native seL4 Endpoints never report it (B76); a clock that parked here
    // would therefore keep the entire call plane alive forever.
    slime_rt::debug_write(b"[robot-clock] bounded time complete\n");
    slime_rt::exit(0)
}

fn send_time(now_ns: u64, wake: u32) {
    let encoded = WireCallTimeAdvance {
        magic: CALL_TIME_MAGIC,
        version: FORMAT_VERSION,
        flags: 0,
        reserved0: 0,
        now_ns,
        reserved: [0; 40],
    }
    .encode();

    loop {
        // Signal before every blocking-send attempt, not merely once before the
        // loop. The broker may park between attempts, and sending first would
        // leave both peers blocked: the sender waiting for a receive and the
        // broker waiting for the notification that sender can no longer raise.
        if slime_rt::notification_signal(wake) != ERR_SUCCESS {
            fail(b"signal notification:fabric-call-worker-parameters-ready")
        }
        match slime_rt::send(ROUTE_SLOT, &encoded, &[]) {
            ERR_SUCCESS => return,
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            error => fail_with(b"time advance send", error),
        }
    }
}

/// Buffers an early phase rather than turning the barrier into an ordering race.
struct PhaseBuffer {
    ready: u8,
}

impl PhaseBuffer {
    const fn new(buffered: Option<u8>) -> Self {
        Self {
            ready: match buffered {
                Some(phase @ 1..=2) => 1 << phase,
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
            // The controller uses a blocking send for this declared barrier.
            // Polling was rejected because neither side would rendezvous unless
            // the clock is already blocked in the matching endpoint receive.
            match slime_rt::recv_blocking(PHASE_SLOT, &mut bytes, &mut caps) {
                ERR_WOULDBLOCK => slime_rt::yield_now(),
                value if value < 0 => fail_with(b"clock phase receive", value),
                1 if matches!(bytes[0], 1 | 2) => {
                    release_caps(&caps);
                    let bit = 1 << bytes[0];
                    if bit == expected_bit {
                        return;
                    }
                    self.ready |= bit;
                }
                _ => {
                    release_caps(&caps);
                    fail(b"clock phase mismatch")
                }
            }
        }
    }
}

/// A phase byte carries no authority; discard any capability sent with it.
fn release_caps(caps: &[u64; MAX_CAPS_PER_MSG]) {
    for slot in caps.iter().copied().filter(|slot| *slot != 0) {
        let _ = slime_rt::cap_drop(slot as u32);
    }
}

fn write_value(prefix: &[u8], value: u64) {
    let mut digits = [0u8; 20];
    slime_rt::debug_write(prefix);
    slime_rt::debug_write(decimal(value, &mut digits));
    slime_rt::debug_write(b"\n");
}

fn decimal(value: u64, digits: &mut [u8; 20]) -> &[u8] {
    let mut index = digits.len();
    let mut remaining = value;
    if remaining == 0 {
        index -= 1;
        digits[index] = b'0';
    }
    while remaining != 0 {
        index -= 1;
        digits[index] = b'0' + (remaining % 10) as u8;
        remaining /= 10;
    }
    &digits[index..]
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[robot-clock] FAIL ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn fail_with(reason: &[u8], error: i64) -> ! {
    slime_rt::debug_write(b"[robot-clock] FAIL ");
    slime_rt::debug_write(reason);
    write_value(b" error=", error.unsigned_abs());
    slime_rt::exit(1)
}

const _: () = assert!(slime_proto::fabric_call::CALL_TIME_LEN == MAX_MSG);
