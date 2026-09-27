#![no_std]
#![no_main]

use slime_proto::network_service::{self as net, WireNetworkLaunch};
use slime_rt::{InputKey, debug_write, exit};

slime_rt::entry!(main);

fn main(_: u32) {
    let input = binding(b"http-launch-input");
    let service = binding(b"http-launch-service");
    let client = binding(b"http-launch-client");
    let rate = slime_rt::monotonic_frequency().unwrap_or_else(|_| fail());
    let deadline = slime_rt::monotonic_read()
        .ok()
        .and_then(|now| rate.checked_mul(30).and_then(|span| now.checked_add(span)))
        .unwrap_or_else(|| fail());
    debug_write(b"[http-launcher] ready\n");
    let seed = receive(input, deadline);
    if seed.kind != net::LAUNCH_SEED || seed.payload[..32].iter().all(|byte| *byte == 0) {
        fail();
    }
    send(service, &seed.encode(), deadline);
    let mut offset = 0usize;
    let mut total = None;
    loop {
        let chunk = receive(input, deadline);
        if chunk.kind != net::LAUNCH_URL
            || usize::from(chunk.offset) != offset
            || total.is_some_and(|length| length != chunk.total_len)
        {
            fail();
        }
        total = Some(chunk.total_len);
        offset += usize::from(chunk.length);
        send(client, &chunk.encode(), deadline);
        if offset == usize::from(chunk.total_len) {
            break;
        }
    }
    debug_write(b"[http-launcher] delivered seed=1 url=1\n");
}

fn binding(name: &[u8]) -> u32 {
    slime_rt::resolve_binding(name).unwrap_or_else(|_| fail())
}

fn check_deadline(deadline: u64) {
    if slime_rt::monotonic_read().map_or(true, |now| now >= deadline) {
        fail();
    }
}

fn send(slot: u32, bytes: &[u8], deadline: u64) {
    check_deadline(deadline);
    // Native endpoint send is a rendezvous. The boot composition must keep the
    // declared receiver alive; the host qualification bounds a failed startup.
    if slime_rt::send(slot, bytes, &[]) != slime_rt::ERR_SUCCESS {
        fail();
    }
}

fn receive(slot: u32, deadline: u64) -> WireNetworkLaunch {
    let mut bytes = [0; net::LAUNCH_BYTES];
    let mut digits = 0usize;
    loop {
        check_deadline(deadline);
        let Some(event) = slime_rt::input_read(slot).unwrap_or_else(|_| fail()) else {
            slime_rt::yield_now();
            continue;
        };
        if !event.pressed {
            continue;
        }
        match event.key {
            InputKey::Enter if digits == bytes.len() * 2 => break,
            InputKey::Character(character) if digits < bytes.len() * 2 => {
                let value = match character {
                    '0'..='9' => character as u8 - b'0',
                    'a'..='f' => character as u8 - b'a' + 10,
                    _ => fail(),
                };
                bytes[digits / 2] = (bytes[digits / 2] << 4) | value;
                digits += 1;
            }
            _ => fail(),
        }
    }
    let frame = WireNetworkLaunch::decode(&bytes).unwrap_or_else(|| fail());
    if !slime_proto::valid_network_launch(&frame) {
        fail();
    }
    frame
}

fn fail() -> ! {
    // Input can contain a secret seed. Errors never echo its bytes.
    debug_write(b"[http-launcher] fail: launch input or delivery\n");
    exit(1)
}
