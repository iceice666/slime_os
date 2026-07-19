#![no_std]
#![no_main]

use slime_rt::{ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

const CONSOLE_SLOT: u32 = 2;

slime_rt::entry!(main);

fn main() {
    send_console(b"[dango] resolve sysinfo via capability\n");
    send_console(b"[dango] echo-agent tool call\n");

    receive_result(0);
    receive_result(1);
}

fn send_console(payload: &[u8]) {
    slime_rt::send(CONSOLE_SLOT, payload, &[]);
}

fn receive_result(slot: u32) {
    let mut buf = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(slot, &mut buf, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => slime_rt::exit(1),
            n => {
                send_console(&buf[..n as usize]);
                return;
            }
        }
    }
}
