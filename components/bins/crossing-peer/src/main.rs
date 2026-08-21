#![no_std]
#![no_main]

use slime_rt::{ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

/// The edge a transferred channel end arrives on. First spawn grant, so slot 0.
const CARRIER_SLOT: u32 = 0;
/// The edge that releases this peer. Second spawn grant, so slot 1.
const GATE_SLOT: u32 = 1;

/// Receives one narrowed logical endpoint authority over the static carrier,
/// proves the sender retained its copy, then sustains native rendezvous traffic
/// for longer than the retired logical channel lifetime bound.
fn main(_startup_arg: u32) {
    let mut payload = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    let landed = loop {
        match slime_rt::recv(CARRIER_SLOT, &mut payload, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => fail(b"collecting the delegated end"),
            _ if caps[0] == 0 => fail(b"delegation carried no authority"),
            _ => break caps[0] as u32,
        }
    };
    if slime_rt::send(landed, b"survived", &[]) != slime_rt::ERR_SUCCESS {
        fail(b"delegated endpoint could not send");
    }
    for _ in 0..49 {
        loop {
            match slime_rt::recv(CARRIER_SLOT, &mut payload, &mut caps) {
                ERR_WOULDBLOCK => slime_rt::yield_now(),
                4 if &payload[..4] == b"ping" => break,
                n if n < 0 => fail(b"crossing rendezvous stopped answering"),
                _ => fail(b"crossing rendezvous payload differed"),
            }
        }
        if slime_rt::send(GATE_SLOT, b"pong", &[]) != slime_rt::ERR_SUCCESS {
            fail(b"crossing reply failed");
        }
    }
    slime_rt::debug_write(b"[crossing-peer] used narrowed delegated endpoint\n");
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[crossing-peer] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}
