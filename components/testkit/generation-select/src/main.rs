#![no_std]
#![no_main]

#[path = "../../../lib/src/generation_command.rs"]
mod command;
use slime_rt::console_line::Line;

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    let identity = command::selected_identity();
    let reply = command::run(slime_proto::generation::OP_SELECT, identity);
    if reply.status != 0
        || command::reply_identity(reply) != identity
        || reply.flags & slime_proto::generation::REPLY_FLAG_PENDING == 0
    {
        command::fail();
    }
    Line::<128>::new()
        .bytes(b"[generation-select] pending attempts=")
        .decimal(u64::from(reply.remaining_attempts))
        .bytes(b"\n")
        .emit();
}
