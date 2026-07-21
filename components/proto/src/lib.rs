#![no_std]

// Protocol modules are generated from contracts/*/v1 schemas.
pub mod block;
pub mod spawn;
pub mod store;

pub fn valid_spawn_request(request: &spawn::WireSpawnRequest) -> bool {
    request.magic == spawn::SPAWN_MAGIC
        && request.version == spawn::FORMAT_VERSION
        && request.command_len > 0
        && request.command_len as usize <= spawn::MAX_COMMAND_BYTES
        && request.argument_count as usize <= spawn::MAX_ARGUMENTS
        && request.environment_count as usize <= spawn::MAX_ENVIRONMENT
}

pub fn valid_spawn_reply(reply: &spawn::WireSpawnReply) -> bool {
    reply.magic == spawn::SPAWN_MAGIC && reply.version == spawn::FORMAT_VERSION
}
