#![no_std]
#![no_main]
#[path = "../../../lib/src/launch_context.rs"]
#[allow(dead_code)]
mod launch_context;
use slime_proto::spawn::{CAPABILITY_ROLE_STDIN, CAPABILITY_ROLE_WORKING_DIRECTORY};
use slime_rt::console_line::Line;
use slime_rt::{MAX_CAPS_PER_MSG, MAX_DIRECTORY_PATH, MAX_MSG};

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    let context = launch_context::receive();
    let cwd_slot = 1;
    let stdin_slot =
        cwd_slot + u32::from(context.capability_roles & CAPABILITY_ROLE_WORKING_DIRECTORY != 0);
    if context.capability_roles & CAPABILITY_ROLE_WORKING_DIRECTORY != 0 {
        let mut root = [0u8; 32];
        let mut scope = [0u8; MAX_DIRECTORY_PATH];
        let Ok(scope_len) = slime_rt::directory_inspect(cwd_slot, 1 << 19, &mut root, &mut scope)
        else {
            slime_rt::exit(1)
        };
        if &scope[..scope_len] != b"docs" {
            slime_rt::exit(1);
        }
    }
    if context.capability_roles & CAPABILITY_ROLE_STDIN != 0 {
        let mut payload = [0u8; MAX_MSG];
        let mut caps = [0u64; MAX_CAPS_PER_MSG];
        let n = slime_rt::recv_blocking(stdin_slot, &mut payload, &mut caps);
        if n < 0 {
            slime_rt::exit(1);
        }
        if &payload[..n as usize] != b"data" || caps.iter().any(|slot| *slot != 0) {
            slime_rt::exit(1);
        }
    }
    Line::<128>::new()
        .bytes(b"[echo-agent] command=echo args=")
        .decimal(u64::from(context.argument_count))
        .bytes(b" env=")
        .decimal(u64::from(context.environment_count))
        .bytes(b" cwd=")
        .bytes(
            if context.capability_roles & CAPABILITY_ROLE_WORKING_DIRECTORY != 0 {
                b"explicit"
            } else {
                b"none"
            },
        )
        .bytes(b" stdin=")
        .bytes(if context.capability_roles & CAPABILITY_ROLE_STDIN != 0 {
            b"explicit\n"
        } else {
            b"none\n"
        })
        .emit();
    let mut line = Line::<1024>::new();
    line.bytes(b"echo-agent{tool=echo,value=");
    if let Some(argument) = context.arguments.argument(0) {
        line.bytes(argument);
    }
    line.bytes(b",env=");
    if let Some(environment) = launch_context::field(&context.environment, 0) {
        line.bytes(environment);
    }
    line.bytes(b"}\n").emit();
}
