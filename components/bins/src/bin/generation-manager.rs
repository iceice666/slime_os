#![no_std]
#![no_main]

use slime_proto::generation::{self, WireGenerationReply, WireGenerationRequest};
use slime_rt::{ERR_PEER_DEAD, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

const CLIENT_SLOTS: [u32; 5] = [0, 2, 3, 4, 5];
const GENERATION_CONTROL_SLOT: u32 = 1;
const FAILING_PENDING_GENERATION: u64 = 99;
const KNOWN_GOOD_GENERATION: u64 = 1;

fn main() {
    let generation = option_env!("SLIME_GENERATION_NUMBER")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(KNOWN_GOOD_GENERATION);
    if generation == FAILING_PENDING_GENERATION {
        slime_rt::debug_write(b"[generation-manager] explicit unhealthy status\n");
        slime_rt::unhealthy();
    }
    if generation != KNOWN_GOOD_GENERATION && option_env!("SLIME_GENERATION_CMD_CHECK") != Some("1")
    {
        slime_rt::debug_write(b"[generation-manager] confirming pending generation\n");
        if slime_rt::health_confirm(GENERATION_CONTROL_SLOT) < 0 {
            slime_rt::debug_write(b"[generation-manager] confirmation rejected\n");
            slime_rt::exit(1);
        }
        slime_rt::debug_write(b"[generation-manager] pending generation confirmed\n");
    } else {
        slime_rt::debug_write(b"[generation-manager] known-good generation active\n");
    }
    if option_env!("SLIME_GENERATION_CMD_CHECK") != Some("1") {
        return;
    }
    slime_rt::debug_write(b"[generation-manager] update service ready\n");
    let mut connected = [true; CLIENT_SLOTS.len()];
    loop {
        let mut progressed = false;
        for (index, slot) in CLIENT_SLOTS.iter().copied().enumerate() {
            if !connected[index] {
                continue;
            }
            let mut message = [0u8; MAX_MSG];
            let mut caps = [0u64; MAX_CAPS_PER_MSG];
            match slime_rt::recv(slot, &mut message, &mut caps) {
                ERR_WOULDBLOCK => {}
                ERR_PEER_DEAD => connected[index] = false,
                n if n < 0 => slime_rt::exit(1),
                n => {
                    progressed = true;
                    if caps.iter().any(|slot| *slot != 0) {
                        slime_rt::exit(1);
                    }
                    let reply = transact(&message[..n as usize]);
                    send_reply(slot, reply);
                }
            }
        }
        if !connected.iter().any(|connected| *connected) {
            slime_rt::exit(0);
        }
        if !progressed {
            slime_rt::yield_now();
        }
    }
}

fn transact(message: &[u8]) -> WireGenerationReply {
    let Some(request) = WireGenerationRequest::decode(message) else {
        return bad_reply();
    };
    let encoded = request.encode();
    let mut response = [0u8; generation::REPLY_LEN];
    if slime_rt::generation_transact(GENERATION_CONTROL_SLOT, &encoded, &mut response) < 0 {
        return bad_reply();
    }
    WireGenerationReply::decode(&response).unwrap_or_else(bad_reply)
}

fn send_reply(slot: u32, reply: WireGenerationReply) {
    let encoded = reply.encode();
    loop {
        match slime_rt::send(slot, &encoded, &[]) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            ERR_PEER_DEAD => return,
            result if result < 0 => slime_rt::exit(1),
            _ => return,
        }
    }
}

fn bad_reply() -> WireGenerationReply {
    WireGenerationReply {
        magic: generation::GENERATION_MAGIC,
        version: generation::FORMAT_VERSION,
        status: -1,
        flags: 0,
        count: 0,
        generation_number: 0,
        release_sequence: 0,
        remaining_attempts: 0,
        generation0: 0,
        generation1: 0,
        generation2: 0,
        generation3: 0,
    }
}
