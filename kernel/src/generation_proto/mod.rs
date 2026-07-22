#[path = "gen.rs"]
mod generated;

pub use generated::*;

pub const GENERATION_E_OK: i32 = 0;
pub const GENERATION_E_BAD_REQUEST: i32 = -1;
pub const GENERATION_E_NOT_FOUND: i32 = -2;
pub const GENERATION_E_BAD_RELEASE: i32 = -3;
pub const GENERATION_E_BAD_CLOSURE: i32 = -4;
pub const GENERATION_E_CONFLICT: i32 = -5;
pub const GENERATION_E_DEVICE: i32 = -6;

pub fn valid_request(request: &WireGenerationRequest) -> bool {
    request.magic == GENERATION_MAGIC
        && request.version == FORMAT_VERSION
        && matches!(
            request.op,
            OP_LIST | OP_INSPECT | OP_STAGE | OP_SELECT | OP_ROLLBACK
        )
        && request.flags == 0
        && request.reserved.iter().all(|byte| *byte == 0)
        && match request.op {
            OP_LIST | OP_ROLLBACK => request_identity(request) == [0; 32],
            OP_INSPECT | OP_STAGE | OP_SELECT => request_identity(request) != [0; 32],
            _ => false,
        }
}

pub fn request_identity(request: &WireGenerationRequest) -> [u8; 32] {
    words_to_identity([
        request.generation0,
        request.generation1,
        request.generation2,
        request.generation3,
    ])
}

pub fn reply_identity(reply: &WireGenerationReply) -> [u8; 32] {
    words_to_identity([
        reply.generation0,
        reply.generation1,
        reply.generation2,
        reply.generation3,
    ])
}

pub fn identity_words(identity: [u8; 32]) -> [u64; 4] {
    core::array::from_fn(|index| {
        u64::from_le_bytes(identity[index * 8..index * 8 + 8].try_into().unwrap())
    })
}

fn words_to_identity(words: [u64; 4]) -> [u8; 32] {
    let mut identity = [0u8; 32];
    for (index, word) in words.iter().enumerate() {
        identity[index * 8..index * 8 + 8].copy_from_slice(&word.to_le_bytes());
    }
    identity
}
