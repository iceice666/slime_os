#![no_std]
#![no_main]

use slime_rt::{ERR_PEER_DEAD, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

fn main() {
    let mut buf = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(0, &mut buf, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            ERR_PEER_DEAD => return,
            n if n < 0 => slime_rt::exit(1),
            n => {
                slime_rt::debug_write(&buf[..n as usize]);
            }
        }
    }
}
