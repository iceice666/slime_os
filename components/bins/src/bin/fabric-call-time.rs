#![no_std]
#![no_main]

#[path = "../fabric_call_scenario.rs"]
mod scenario;

use slime_rt::{ERR_PEER_DEAD, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, WaitSource};

slime_rt::entry!(main);

// The generation's resolved capability layout, generated into `OUT_DIR` by
// `scripts/build/boot_layout.py`. Read here so the phase-channel test below is a
// fact about this build rather than a flag someone has to remember to set.
include!(concat!(env!("OUT_DIR"), "/boot_layout.rs"));

fn main() {
    // Park whenever this build has no phase channel to be driven over, not only
    // on the full-graph boot generation.
    //
    // `fabric_boot::active()` keys on `SLIME_FABRIC_BOOT_CHECK`, which the x86
    // boot generation sets and the seL4 call plane does not — yet the seL4 plane
    // has the same property the guard exists for: `sel4-call.zti` declares no
    // phase grants, so `FABRIC_CALL_PHASE_TIME_SLOT` is `SLOT_ABSENT` and no
    // component on that plane publishes a time phase. Taking the phase path there
    // parked this component on a slot naming nothing and killed the boot with
    // `fail: time phase receive`.
    //
    // Testing the slot rather than adding a second flag: the condition that
    // actually matters is whether a phase channel exists, and the boot layout
    // already answers that. A flag would have to be kept in step with every
    // future generation; this cannot drift.
    if slime_components::fabric_boot::active() || FABRIC_CALL_PHASE_TIME_SLOT == SLOT_ABSENT {
        // As `fabric-op-time`: no phase channel in the boot layout, and a graph
        // at rest must not have its clock advanced underneath it.
        slime_components::fabric_boot::park_only(b"fabric-call-time");
    }
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
