#![no_std]
#![no_main]
#[path = "../launch_context.rs"]
mod launch_context;
use slime_proto::spawn::{CAPABILITY_ROLE_STDIN, CAPABILITY_ROLE_WORKING_DIRECTORY};
use slime_rt::{ERR_PEER_DEAD, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_DIRECTORY_PATH, MAX_MSG};

slime_rt::entry!(main);

fn main() {
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
        loop {
            match slime_rt::recv(stdin_slot, &mut payload, &mut caps) {
                ERR_WOULDBLOCK => slime_rt::yield_now(),
                ERR_PEER_DEAD => slime_rt::exit(1),
                n if n < 0 => slime_rt::exit(1),
                n => {
                    if &payload[..n as usize] != b"data" || caps.iter().any(|slot| *slot != 0) {
                        slime_rt::exit(1);
                    }
                    break;
                }
            }
        }
    }
    slime_rt::debug_write(b"[echo-agent] command=echo args=");
    launch_context::debug_decimal(context.argument_count as usize);
    slime_rt::debug_write(b" env=");
    launch_context::debug_decimal(context.environment_count as usize);
    slime_rt::debug_write(b" cwd=");
    slime_rt::debug_write(
        if context.capability_roles & CAPABILITY_ROLE_WORKING_DIRECTORY != 0 {
            b"explicit"
        } else {
            b"none"
        },
    );
    slime_rt::debug_write(b" stdin=");
    slime_rt::debug_write(if context.capability_roles & CAPABILITY_ROLE_STDIN != 0 {
        b"explicit\n"
    } else {
        b"none\n"
    });
    slime_rt::debug_write(b"echo-agent{tool=echo,value=");
    if let Some(argument) = launch_context::field(&context.arguments, 0) {
        slime_rt::debug_write(argument);
    }
    slime_rt::debug_write(b",env=");
    if let Some(environment) = launch_context::field(&context.environment, 0) {
        slime_rt::debug_write(environment);
    }
    slime_rt::debug_write(b"}\n");
}
