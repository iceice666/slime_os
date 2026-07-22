#![no_std]

// Protocol modules are generated from contracts/*/v1 schemas.
pub mod block;
pub mod fs;
pub mod generation;
pub mod spawn;
pub mod store;

pub fn valid_fs_request(request: &fs::WireFsRequest) -> bool {
    let name_len = request.name_len as usize;
    let base_valid = request.magic == fs::FS_MAGIC
        && request.version == fs::FORMAT_VERSION
        && matches!(
            request.op,
            fs::OP_LIST | fs::OP_READ | fs::OP_WRITE | fs::OP_DERIVE
        )
        && request.flags == 0
        && request.reserved0 == 0
        && name_len <= fs::MAX_NAME_BYTES
        && request.name[name_len..].iter().all(|byte| *byte == 0)
        && valid_name(&request.name[..name_len], request.op == fs::OP_LIST);
    if !base_valid {
        return false;
    }
    let zero_hash =
        request.hash0 == 0 && request.hash1 == 0 && request.hash2 == 0 && request.hash3 == 0;
    match request.op {
        fs::OP_LIST | fs::OP_READ | fs::OP_DERIVE => request.payload_len == 0 && zero_hash,
        fs::OP_WRITE => request.payload_len <= 32 * 1024 && !zero_hash,
        _ => false,
    }
}

pub fn valid_fs_reply(reply: &fs::WireFsReply) -> bool {
    reply.magic == fs::FS_MAGIC
        && reply.version == fs::FORMAT_VERSION
        && reply.entry_count as usize <= fs::MAX_ENTRIES
        && reply.reserved == 0
}
fn valid_name(name: &[u8], allow_empty: bool) -> bool {
    if name.is_empty() {
        return allow_empty;
    }
    name != b"."
        && name != b".."
        && name
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-'))
}

pub fn valid_spawn_request(request: &spawn::WireSpawnRequest) -> bool {
    if request.magic != spawn::SPAWN_MAGIC || request.version != spawn::FORMAT_VERSION {
        return false;
    }
    if request.flags == spawn::REQUEST_FLAG_WAIT {
        return request.command_len == 0
            && request.argument_count == 0
            && request.environment_count == 0
            && request.capability_roles == 0
            && request.command.iter().all(|byte| *byte == 0)
            && request.environment.iter().all(|byte| *byte == 0)
            && u64::from_le_bytes(request.arguments) != 0
            && request.grant_rights == 0
            && request.reserved.iter().all(|byte| *byte == 0);
    }
    request.flags == 0
        && request.command_len > 0
        && request.command_len as usize <= spawn::MAX_COMMAND_BYTES
        && request.argument_count as usize <= spawn::MAX_ARGUMENTS
        && request.environment_count as usize <= spawn::MAX_ENVIRONMENT
        && packed_fields_valid(&request.arguments, request.argument_count as usize)
        && packed_fields_valid(&request.environment, request.environment_count as usize)
}

fn packed_fields_valid<const N: usize>(bytes: &[u8; N], count: usize) -> bool {
    let mut offset = 0;
    for _ in 0..count {
        let Some(length) = bytes.get(offset).copied().map(usize::from) else {
            return false;
        };
        if length == 0 || offset + 1 + length > bytes.len() {
            return false;
        }
        offset += 1 + length;
    }
    bytes[offset..].iter().all(|byte| *byte == 0)
}

pub fn valid_spawn_reply(reply: &spawn::WireSpawnReply) -> bool {
    reply.magic == spawn::SPAWN_MAGIC && reply.version == spawn::FORMAT_VERSION
}
