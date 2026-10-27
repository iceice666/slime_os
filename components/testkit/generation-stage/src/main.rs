#![no_std]
#![no_main]

#[path = "../../../lib/src/generation_command.rs"]
mod command;

use slime_rt::console_line::Line;

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
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
        Line::<64>::new()
            .bytes(b"[generation-stage] rejected status=")
            .signed(i64::from(reply.status))
            .bytes(b"\n")
            .emit();
        return;
    }
    if reply.status != 0
        || command::reply_identity(reply) != identity
        || reply.flags & slime_proto::generation::REPLY_FLAG_STAGED == 0
    {
        Line::<64>::new()
            .bytes(b"[generation-stage] status=")
            .signed(i64::from(reply.status))
            .bytes(b" flags=")
            .decimal(u64::from(reply.flags))
            .bytes(b"\n")
            .emit();
        command::fail();
    }
    Line::<64>::new()
        .bytes(b"[generation-stage] staged release=")
        .decimal(u64::from(reply.release_sequence))
        .bytes(b"\n")
        .emit();
}
