use slime_proto::generation::{self, WireGenerationReply, WireGenerationRequest};
use slime_rt::{ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

const RPC_SLOT: u32 = 0;
const ZERO_IDENTITY: [u8; 32] = [0; 32];

pub fn run(op: u8, identity: [u8; 32]) -> WireGenerationReply {
    let words = identity_words(identity);
    let request = WireGenerationRequest {
        magic: generation::GENERATION_MAGIC,
        version: generation::FORMAT_VERSION,
        op,
        flags: 0,
        reserved: [0; 6],
        generation0: words[0],
        generation1: words[1],
        generation2: words[2],
        generation3: words[3],
    };
    let encoded = request.encode();
    loop {
        match slime_rt::send(RPC_SLOT, &encoded, &[]) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => fail_with(b"send", result),
            _ => break,
        }
    }
    let mut reply = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(RPC_SLOT, &mut reply, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => fail_with(b"recv", result),
            n => {
                if caps.iter().any(|slot| *slot != 0) {
                    fail();
                }
                let Some(reply) = WireGenerationReply::decode(&reply[..n as usize]) else {
                    fail();
                };
                if reply.magic != generation::GENERATION_MAGIC
                    || reply.version != generation::FORMAT_VERSION
                {
                    slime_rt::debug_write(b"[generation-command] bad reply header\n");
                    fail();
                }
                return reply;
            }
        }
    }
}

#[allow(dead_code)]
pub const fn zero_identity() -> [u8; 32] {
    ZERO_IDENTITY
}

#[allow(dead_code)]
pub fn selected_identity() -> [u8; 32] {
    let reply = run(generation::OP_LIST, ZERO_IDENTITY);
    if reply.status != 0 {
        fail();
    }
    reply_identity(reply)
}
#[allow(dead_code)]
pub fn reply_identity(reply: WireGenerationReply) -> [u8; 32] {
    let mut identity = [0u8; 32];
    for (index, word) in [
        reply.generation0,
        reply.generation1,
        reply.generation2,
        reply.generation3,
    ]
    .into_iter()
    .enumerate()
    {
        identity[index * 8..index * 8 + 8].copy_from_slice(&word.to_le_bytes());
    }
    identity
}
#[allow(dead_code)]
pub fn write_i32(value: i32) {
    if value < 0 {
        slime_rt::debug_write(b"-");
        write_u32(value.unsigned_abs());
    } else {
        write_u32(value as u32);
    }
}
#[allow(dead_code)]
pub fn write_u32(mut value: u32) {
    let mut buffer = [0u8; 10];
    let mut cursor = buffer.len();
    if value == 0 {
        slime_rt::debug_write(b"0");
        return;
    }
    while value != 0 {
        cursor -= 1;
        buffer[cursor] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    slime_rt::debug_write(&buffer[cursor..]);
}

fn identity_words(identity: [u8; 32]) -> [u64; 4] {
    core::array::from_fn(|index| {
        u64::from_le_bytes(identity[index * 8..index * 8 + 8].try_into().unwrap())
    })
}
fn fail_with(stage: &[u8], error: i64) -> ! {
    slime_rt::debug_write(b"[generation-command] ");
    slime_rt::debug_write(stage);
    slime_rt::debug_write(b" error\n");
    let _ = error;
    slime_rt::exit(1)
}

pub fn fail() -> ! {
    slime_rt::debug_write(b"[generation-command] failed\n");
    slime_rt::exit(1)
}
