#![no_std]
#![no_main]

#[path = "../../../lib/src/generation_command.rs"]
mod command;
use slime_rt::console_line::Line;

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    let identity = command::selected_identity();
    let reply = command::run(slime_proto::generation::OP_INSPECT, identity);
    if reply.status != 0 || command::reply_identity(reply) != identity {
        command::fail();
    }
    Line::<160>::new()
        .bytes(b"[generation-inspect] generation=")
        .decimal(u64::from(reply.generation_number))
        .bytes(b" objects=")
        .decimal(u64::from(reply.count))
        .bytes(b" release=")
        .decimal(u64::from(reply.release_sequence))
        .bytes(b" flags=")
        .decimal(u64::from(reply.flags))
        .bytes(b"\n")
        .emit();
}
