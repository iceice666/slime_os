#![no_std]
#![no_main]

#[path = "../generation_command.rs"]
mod command;

slime_rt::entry!(main);

fn main() {
    let reply = command::run(slime_proto::generation::OP_LIST, command::zero_identity());
    if reply.status != 0 {
        slime_rt::debug_write(b"[generation-list] status=");
        command::write_i32(reply.status);
        slime_rt::debug_write(b"\n");
        command::fail();
    }
    let request = slime_proto::generation::WireGenerationRequest {
        magic: slime_proto::generation::GENERATION_MAGIC,
        version: slime_proto::generation::FORMAT_VERSION,
        op: slime_proto::generation::OP_LIST,
        flags: 0,
        reserved: [0; 6],
        generation0: 0,
        generation1: 0,
        generation2: 0,
        generation3: 0,
    }
    .encode();
    let mut direct_reply = [0u8; slime_proto::generation::REPLY_LEN];
    if slime_rt::generation_transact(0, &request, &mut direct_reply) != slime_rt::ERR_BAD_CAP {
        command::fail();
    }
    slime_rt::debug_write(b"[generation-list] direct boot update denied\n");
    slime_rt::debug_write(b"[generation-list] count=");
    command::write_u32(reply.count);
    slime_rt::debug_write(b" accepted-release=");
    command::write_u32(reply.release_sequence);
    slime_rt::debug_write(b"\n");
}
