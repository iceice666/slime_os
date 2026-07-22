#![no_std]
#![no_main]

#[path = "../generation_command.rs"]
mod command;

slime_rt::entry!(main);

fn main() {
    let reply = command::run(
        slime_proto::generation::OP_ROLLBACK,
        command::zero_identity(),
    );
    if reply.status != 0 || reply.flags & slime_proto::generation::REPLY_FLAG_PENDING != 0 {
        command::fail();
    }
    slime_rt::debug_write(b"[generation-rollback] known-good restored\n");
}
