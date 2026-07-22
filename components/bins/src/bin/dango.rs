#![no_std]
#![no_main]

use slime_components::dango_runtime::{Launch, MAX_LINE_BYTES, parse};
use slime_proto::{
    spawn::{
        CAPABILITY_ROLE_STDIN, CAPABILITY_ROLE_WORKING_DIRECTORY, REQUEST_FLAG_WAIT,
        WireSpawnReply, WireSpawnRequest,
    },
    valid_spawn_reply,
};
use slime_rt::{ERR_WOULDBLOCK, InputKey, MAX_CAPS_PER_MSG, MAX_MSG, Termination};

const SPAWN_SLOT: u32 = 0;
const CONSOLE_SLOT: u32 = 1;
const INPUT_SLOT: u32 = 2;
const CWD_ROOT_SLOT: u32 = 3;
const ENDPOINT_FACTORY_SLOT: u32 = 4;
const RIGHT_TRANSFER: u32 = 4;
const RIGHT_DIRECTORY_READ: u32 = 1 << 19;

include!(concat!(env!("OUT_DIR"), "/dango_profile.rs"));

slime_rt::entry!(main);

fn main() {
    console(b"[dango] native runtime ready\n");
    let mut line = [0u8; MAX_LINE_BYTES];
    let mut len = 0;
    console(b"dango> ");
    loop {
        match slime_rt::input_read(INPUT_SLOT) {
            Ok(None) => slime_rt::yield_now(),
            Err(_) => slime_rt::exit(1),
            Ok(Some(event)) if !event.pressed => {}
            Ok(Some(event)) => match event.key {
                InputKey::Character(character) if character.is_ascii() && len < line.len() => {
                    line[len] = character as u8;
                    len += 1;
                    if option_env!("SLIME_DANGO_CHECK") != Some("1") {
                        console(&[character as u8]);
                    }
                }
                InputKey::Space if len < line.len() => {
                    line[len] = b' ';
                    len += 1;
                    if option_env!("SLIME_DANGO_CHECK") != Some("1") {
                        console(b" ");
                    }
                }
                InputKey::Backspace if len > 0 => {
                    len -= 1;
                    if option_env!("SLIME_DANGO_CHECK") != Some("1") {
                        console(b"\x08 \x08");
                    }
                }
                InputKey::Enter => {
                    if option_env!("SLIME_DANGO_CHECK") == Some("1") {
                        console(&line[..len]);
                    }
                    console(b"\n");
                    if len != 0 {
                        evaluate(&line[..len]);
                    }
                    len = 0;
                    console(b"dango> ");
                }
                InputKey::Escape => {
                    console(b"\n[dango] interactive session closed\n");
                    return;
                }
                _ => {}
            },
        }
    }
}

fn evaluate(line: &[u8]) {
    let launch = match parse(line) {
        Ok(launch) => launch,
        Err(_) => {
            console(b"parse-error\n");
            return;
        }
    };
    if !COMMAND_NAMES.contains(&launch.command) {
        console(b"resolve-denied\n");
        return;
    }
    console(b"resolved:profile\n");
    let reply = spawn(&launch);
    if reply.status != 0 {
        console(b"spawn-error\n");
        return;
    }
    console(b"spawn-request:accepted\n");
    match wait(reply.task_id) {
        Termination::Exit(0) => console(b"result:exit:0\n"),
        Termination::Exit(_) => console(b"IO.Exit:status\n"),
        Termination::Fault(_) => console(b"result:fault\n"),
        Termination::Timeout => console(b"result:timeout\n"),
        Termination::PeerLoss => console(b"result:peer-loss\n"),
        Termination::Unhealthy => console(b"result:revocation\n"),
    }
}

fn spawn(launch: &Launch<'_>) -> WireSpawnReply {
    let mut command = [0u8; 16];
    command[..launch.command.len()].copy_from_slice(launch.command);
    let mut roles = 0;
    let mut caps = [0u32; MAX_CAPS_PER_MSG];
    let mut cap_count = 0;
    let mut cwd_slot = None;
    let mut stdin_slots = None;

    if let Some(cwd) = launch.cwd {
        let derived = match slime_rt::directory_derive(
            CWD_ROOT_SLOT,
            cwd,
            RIGHT_DIRECTORY_READ | RIGHT_TRANSFER,
        ) {
            Ok(slot) => slot,
            Err(_) => return error_reply(slime_rt::ERR_BAD_CAP as i32),
        };
        roles |= CAPABILITY_ROLE_WORKING_DIRECTORY;
        caps[cap_count] = derived;
        cap_count += 1;
        cwd_slot = Some(derived);
    }
    if let Some(stdin) = launch.stdin {
        let (send_slot, recv_slot) = match slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT) {
            Ok(pair) => pair,
            Err(_) => return error_reply(slime_rt::ERR_BAD_CAP as i32),
        };
        if send_all(send_slot, stdin) < 0 {
            return error_reply(slime_rt::ERR_BAD_CAP as i32);
        }
        roles |= CAPABILITY_ROLE_STDIN;
        caps[cap_count] = recv_slot;
        cap_count += 1;
        stdin_slots = Some((send_slot, recv_slot));
    }

    let request = WireSpawnRequest {
        magic: slime_proto::spawn::SPAWN_MAGIC,
        version: slime_proto::spawn::FORMAT_VERSION,
        flags: 0,
        command_len: launch.command.len() as u16,
        argument_count: launch.arguments.count,
        environment_count: launch.environment.count,
        capability_roles: roles,
        client_budget: CLIENT_BUDGET,
        command,
        arguments: launch.arguments.bytes,
        environment: launch.environment.bytes,
        grant_rights: 0,
        reserved: [0; 6],
    };
    let reply = send_request(request, &caps[..cap_count]);
    if let Some(slot) = cwd_slot {
        let _ = slime_rt::cap_drop(slot);
    }
    if let Some((send_slot, recv_slot)) = stdin_slots {
        let _ = slime_rt::cap_drop(send_slot);
        let _ = slime_rt::cap_drop(recv_slot);
    }
    reply
}

fn send_all(slot: u32, payload: &[u8]) -> i64 {
    loop {
        match slime_rt::send(slot, payload, &[]) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            result => return result,
        }
    }
}

const fn error_reply(status: i32) -> WireSpawnReply {
    WireSpawnReply {
        magic: slime_proto::spawn::SPAWN_MAGIC,
        version: slime_proto::spawn::FORMAT_VERSION,
        status,
        termination_kind: 0,
        task_id: 0,
        supervision_slot: 0,
        detail: 0,
    }
}

fn send_request(request: WireSpawnRequest, caps: &[u32]) -> WireSpawnReply {
    let encoded = request.encode();
    loop {
        match slime_rt::send(SPAWN_SLOT, &encoded, caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => slime_rt::exit(1),
            _ => break,
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

fn wait(task_id: u64) -> Termination {
    let request = WireSpawnRequest {
        magic: slime_proto::spawn::SPAWN_MAGIC,
        version: slime_proto::spawn::FORMAT_VERSION,
        flags: REQUEST_FLAG_WAIT,
        command_len: 0,
        argument_count: 0,
        environment_count: 0,
        capability_roles: 0,
        client_budget: CLIENT_BUDGET,
        command: [0; 16],
        arguments: task_id.to_le_bytes(),
        environment: [0; 8],
        grant_rights: 0,
        reserved: [0; 6],
    };
    loop {
        let reply = send_request(request, &[]);
        if reply.status == ERR_WOULDBLOCK as i32 {
            slime_rt::yield_now();
            continue;
        }
        if reply.status != 0 {
            return Termination::PeerLoss;
        }
        return match reply.termination_kind {
            1 => Termination::Exit(reply.detail as i64),
            2 => Termination::Fault(reply.detail),
            3 => Termination::Timeout,
            4 => Termination::PeerLoss,
            5 => Termination::Unhealthy,
            _ => Termination::PeerLoss,
        };
    }
}

fn console(payload: &[u8]) {
    loop {
        match slime_rt::send(CONSOLE_SLOT, payload, &[]) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => slime_rt::exit(1),
            _ => return,
        }
    }
}
