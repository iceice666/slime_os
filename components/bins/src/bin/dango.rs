#![no_std]
#![no_main]

use slime_proto::{
    spawn::{CAPABILITY_ROLE_STDOUT, WireSpawnReply, WireSpawnRequest},
    valid_spawn_reply,
};
use slime_rt::{ERR_BAD_CAP, ERR_OUT_OF_MEMORY, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

const SPAWN_SLOT: u32 = 0;
const CONSOLE_SLOT: u32 = 1;

include!(concat!(env!("OUT_DIR"), "/dango_profile.rs"));

slime_rt::entry!(main);

fn main() {
    slime_rt::debug_write(b"[dango] spawn client started\n");
    send_console(b"[dango] resolve sysinfo through command profile\n");
    let sysinfo = request("sysinfo", CAPABILITY_ROLE_STDOUT, &[CONSOLE_SLOT]);
    if sysinfo.status != 0 {
        slime_rt::exit(1);
    }
    slime_rt::debug_write(b"[dango] spawned child accepted\n");

    slime_rt::debug_write(b"[dango] reject command outside profile\n");
    let denied = request("inject", 0, &[]);
    if denied.status != ERR_BAD_CAP as i32 {
        slime_rt::exit(1);
    }
    slime_rt::debug_write(b"[dango] reject budget overflow\n");
    let exhausted = request("sysinfo", 0, &[]);
    if exhausted.status != ERR_OUT_OF_MEMORY as i32 {
        slime_rt::exit(1);
    }
    slime_rt::debug_write(b"[dango] spawn profile verified\n");
}

fn request(command: &str, capability_roles: u8, caps: &[u32]) -> WireSpawnReply {
    let mut command_bytes = [0u8; 16];
    command_bytes[..command.len()].copy_from_slice(command.as_bytes());
    let request = WireSpawnRequest {
        magic: slime_proto::spawn::SPAWN_MAGIC,
        version: slime_proto::spawn::FORMAT_VERSION,
        flags: 0,
        command_len: command.len() as u16,
        argument_count: 0,
        environment_count: 0,
        capability_roles,
        client_budget: CLIENT_BUDGET,
        command: command_bytes,
        arguments: [0; 8],
        environment: [0; 8],
        grant_rights: 0,
        reserved: [0; 6],
    };
    slime_rt::debug_write(b"[dango] sending spawn request\n");
    let encoded = request.encode();
    loop {
        match slime_rt::send(SPAWN_SLOT, &encoded, caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            ERR_BAD_CAP => {
                slime_rt::debug_write(b"[dango] spawn send bad cap\n");
                slime_rt::exit(1)
            }
            result if result < 0 => {
                slime_rt::debug_write(b"[dango] spawn send other failure\n");
                slime_rt::exit(1)
            }
            _ => {
                slime_rt::debug_write(b"[dango] spawn request sent\n");
                break;
            }
        }
    }
    let mut reply = [0u8; MAX_MSG];
    let mut received_caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(SPAWN_SLOT, &mut reply, &mut received_caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => slime_rt::exit(1),
            n => {
                let Some(decoded) = WireSpawnReply::decode(&reply[..n as usize]) else {
                    slime_rt::exit(1)
                };
                if !valid_spawn_reply(&decoded) {
                    slime_rt::exit(1);
                }
                return decoded;
            }
        }
    }
}

fn send_console(payload: &[u8]) {
    if slime_rt::send(CONSOLE_SLOT, payload, &[]) < 0 {
        slime_rt::exit(1);
    }
}
