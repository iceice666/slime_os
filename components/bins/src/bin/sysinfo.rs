#![no_std]
#![no_main]

slime_rt::entry!(main);

fn main() {
    if slime_rt::send(0, b"[sysinfo] spawned through profile\n", &[]) < 0 {
        slime_rt::exit(1);
    }
    let mut message = [0u8; slime_rt::MAX_MSG];
    let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(1, &mut message, &mut caps) {
            slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n >= 0 => return,
            _ => slime_rt::exit(1),
        }
    }
}
