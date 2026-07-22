#![no_std]
#![no_main]

#[path = "../generation_command.rs"]
mod command;

slime_rt::entry!(main);

fn main() {
    let identity = command::selected_identity();
    let reply = command::run(slime_proto::generation::OP_SELECT, identity);
    if reply.status != 0
        || command::reply_identity(reply) != identity
        || reply.flags & slime_proto::generation::REPLY_FLAG_PENDING == 0
    {
        command::fail();
    }
    slime_rt::debug_write(b"[generation-select] pending attempts=");
    command::write_u32(reply.remaining_attempts);
    slime_rt::debug_write(b"\n");
}
