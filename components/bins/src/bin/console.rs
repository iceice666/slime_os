#![no_std]
#![no_main]

use slime_rt::{ERR_PEER_DEAD, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

/// A free page-aligned address, borrowed only for the startup self-check.
const QUOTA_PROBE_BASE: u64 = 0x0000_0006_0000_0000;

fn main() {
    let mut buf = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(0, &mut buf, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::wait(&[slime_rt::WaitSource::Endpoint(0)]),
            ERR_PEER_DEAD => return,
            n if n < 0 => slime_rt::exit(1),
            n => {
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
                        0,
                        QUOTA_PROBE_BASE,
                    )
                {
                    slime_rt::exit(1);
                }
            }
        }
    }
}
