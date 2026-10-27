#![no_std]
#![no_main]
#[path = "../../../lib/src/launch_context.rs"]
#[allow(dead_code)]
mod launch_context;
use slime_rt::console_line::Line;

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    let context = launch_context::receive();
    Line::<128>::new()
        .bytes(b"[sysinfo] command=sysinfo args=")
        .decimal(u64::from(context.argument_count))
        .bytes(b" env=")
        .decimal(u64::from(context.environment_count))
        .bytes(b" cwd=none stdin=none\n")
        .emit();
    slime_rt::debug_write(b"[sysinfo] spawned through profile\n");
}
