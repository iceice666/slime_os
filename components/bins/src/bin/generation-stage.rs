#![no_std]
#![no_main]

#[path = "../generation_command.rs"]
mod command;

slime_rt::entry!(main);

fn main() {
    let identity = command::selected_identity();
    let reply = command::run(slime_proto::generation::OP_STAGE, identity);
    let expected_status = match option_env!("SLIME_GENERATION_CMD_SCENARIO") {
        Some("bad-closure") => Some(-4),
        Some("bad-release") => Some(-3),
        _ => None,
    };
    if expected_status.is_some() {
        if reply.status != -4 && reply.status != -3 {
            command::fail();
        }
        slime_rt::debug_write(b"[generation-stage] rejected status=");
        command::write_i32(reply.status);
        slime_rt::debug_write(b"\n");
        return;
    }
    if reply.status != 0
        || command::reply_identity(reply) != identity
        || reply.flags & slime_proto::generation::REPLY_FLAG_STAGED == 0
    {
        slime_rt::debug_write(b"[generation-stage] status=");
        command::write_i32(reply.status);
        slime_rt::debug_write(b" flags=");
        command::write_u32(reply.flags);
        slime_rt::debug_write(b"\n");
        command::fail();
    }
    slime_rt::debug_write(b"[generation-stage] staged release=");
    command::write_u32(reply.release_sequence);
    slime_rt::debug_write(b"\n");
}
