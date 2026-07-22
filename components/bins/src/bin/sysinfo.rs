#![no_std]
#![no_main]
#[path = "../launch_context.rs"]
mod launch_context;

slime_rt::entry!(main);

fn main() {
    let context = launch_context::receive();
    slime_rt::debug_write(b"[sysinfo] command=sysinfo args=");
    launch_context::debug_decimal(context.argument_count as usize);
    slime_rt::debug_write(b" env=");
    launch_context::debug_decimal(context.environment_count as usize);
    slime_rt::debug_write(b" cwd=none stdin=none\n");
    slime_rt::debug_write(b"[sysinfo] spawned through profile\n");
}
