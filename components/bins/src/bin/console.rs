#![no_std]
#![no_main]

use slime_rt::{MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

/// A free page-aligned address, borrowed only for the startup self-check.
const QUOTA_PROBE_BASE: u64 = 0x0000_0006_0000_0000;

/// The shared-buffer factory this component is granted, when it is granted one.
///
/// Slot 0 is the channel end every `console` addresses, and a root-launched
/// component's runtime-numbered slots start above its executables — of which
/// this component has none — so a `bufferCreate` grant lands at 1. Named here
/// rather than passed as a literal so the coupling is visible: B13 made the
/// grant load-bearing, and before that this slot resolved to nothing and the
/// quota alone admitted the allocation.
const SHARED_BUFFER_FACTORY_SLOT: u32 = 1;
const CLOSE: &[u8] = b"SLIME.CONSOLE.CLOSE";

fn main(_startup_arg: u32) {
    let mut buf = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    if option_env!("SLIME_SEL4_CHANNEL_CHECK") == Some("1") {
        slime_rt::debug_write(b"[console] unrelated progress while sender blocked\n");
    }
    loop {
        let n = slime_rt::recv_blocking(0, &mut buf, &mut caps);
        match n {
            n if n < 0 => slime_rt::exit(1),
            n => {
                if &buf[..n as usize] == CLOSE {
                    slime_rt::debug_write(b"[console] channel close received\n");
                    slime_rt::debug_write(b"[console] channel plane complete\n");
                    return;
                }
                slime_rt::debug_write(&buf[..n as usize]);
                // P5.3.2's unrelated holder. Under the loan-plane generation
                // this component is a declared shared-buffer holder that takes
                // no part in the loan, and the milestone requires the loan's
                // quota exhaustion to leave it undisturbed.
                //
                // Receiving and printing does not show that — it exercises the
                // channel plane, not the quota plane. So on the message init
                // sends *after* exhausting all four of its own ceilings, this
                // runs the same bounded create/map/write/seal/unmap/release the
                // startup probe performs, against its own declared quota. A
                // ceiling that leaked across holders fails here.
                //
                // Guarded, because P5.3.1's gate asserts this component's
                // output exactly and its generation declares it no quota.
                if option_env!("SLIME_SEL4_LOAN_CHECK") == Some("1")
                    && !slime_components::shared_buffer_probe::probe_and_report(
                        b"[console]",
                        SHARED_BUFFER_FACTORY_SLOT,
                        QUOTA_PROBE_BASE,
                    )
                {
                    slime_rt::exit(1);
                }
            }
        }
    }
}
