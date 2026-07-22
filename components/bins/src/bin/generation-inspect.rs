#![no_std]
#![no_main]

#[path = "../generation_command.rs"]
mod command;

slime_rt::entry!(main);

fn main() {
    let identity = command::selected_identity();
    let reply = command::run(slime_proto::generation::OP_INSPECT, identity);
    if reply.status != 0 || command::reply_identity(reply) != identity {
        command::fail();
    }
    slime_rt::debug_write(b"[generation-inspect] generation=");
    command::write_u32(reply.generation_number);
    slime_rt::debug_write(b" objects=");
    command::write_u32(reply.count);
    slime_rt::debug_write(b" release=");
    command::write_u32(reply.release_sequence);
    slime_rt::debug_write(b" flags=");
    command::write_u32(reply.flags);
    slime_rt::debug_write(b"\n");
}
