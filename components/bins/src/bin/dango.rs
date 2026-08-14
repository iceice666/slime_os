#![no_std]
#![no_main]

use slime_components::dango_runtime::{Launch, MAX_LINE_BYTES, parse};
use slime_proto::{
    capability_transfer::OBJECT_KIND_DIRECTORY,
    spawn::{
        CAPABILITY_ROLE_STDIN, CAPABILITY_ROLE_WORKING_DIRECTORY, WireSpawnReply, WireSpawnRequest,
    },
    valid_spawn_reply,
};
use slime_rt::{
    CapabilityDisposition, ERR_WOULDBLOCK, InputKey, MAX_CAPS_PER_MSG, MAX_MSG, Termination,
};

const SPAWN_SLOT: u32 = 0;
const CONSOLE_SLOT: u32 = 1;
const INPUT_SLOT: u32 = 2;
const CWD_ROOT_SLOT: u32 = 3;
/// Preinstalled sender paired with echo-agent's declared stdin endpoint.
const STDIN_SEND_SLOT: u32 = 4;
const SHARED_BUFFER_FACTORY_SLOT: u32 = 5;
// A free page-aligned user address, borrowed only for the startup self-check.
const SHARED_BUFFER_PROBE_BASE: u64 = 0x0000_0005_0000_0000;
const RIGHT_TRANSFER: u32 = 4;
const RIGHT_DIRECTORY_READ: u32 = 1 << 19;

/// Fail with a named reason on serial.
///
/// Every exit site used to be a bare `slime_rt::exit(1)`, so a session that ran
/// its command correctly and then stopped reported nothing about why — the plane
/// failed with a transcript that showed only success. `debug_write` rather than
/// `console`, deliberately: the console path is one of the things that can be
/// broken, and a diagnostic that travels over the mechanism under test says
/// nothing when that mechanism is the fault.
fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[dango] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

include!(concat!(env!("OUT_DIR"), "/dango_profile.rs"));
mod generation_profile {
    include!(concat!(env!("OUT_DIR"), "/fabric_profile.rs"));
}

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    console(b"[dango] native runtime ready\n");
    // C7.2/C7.3: prove the generation-declared shared-buffer quota is live
    // before accepting input. Fatal on failure — the generation granted
    // authority the kernel did not honour.
    if !slime_components::shared_buffer_probe::probe_and_report(
        b"[dango]",
        SHARED_BUFFER_FACTORY_SLOT,
        SHARED_BUFFER_PROBE_BASE,
    ) {
        fail(b"shared-buffer quota probe");
    }
    let mut line = [0u8; MAX_LINE_BYTES];
    let mut len = 0;
    console(b"dango> ");
    loop {
        match slime_rt::input_read(INPUT_SLOT) {
            Ok(None) => slime_rt::yield_now(),
            Err(_) => fail(b"input read"),
            Ok(Some(event)) if !event.pressed => {}
            Ok(Some(event)) => match event.key {
                InputKey::Character(character) if character.is_ascii() && len < line.len() => {
                    line[len] = character as u8;
                    len += 1;
                    if generation_profile::GENERATION_BOOT_ACTION != "dango" {
                        console(&[character as u8]);
                    }
                }
                InputKey::Space if len < line.len() => {
                    line[len] = b' ';
                    len += 1;
                    if generation_profile::GENERATION_BOOT_ACTION != "dango" {
                        console(b" ");
                    }
                }
                InputKey::Backspace if len > 0 => {
                    len -= 1;
                    if generation_profile::GENERATION_BOOT_ACTION != "dango" {
                        console(b"\x08 \x08");
                    }
                }
                InputKey::Enter => {
                    if generation_profile::GENERATION_BOOT_ACTION == "dango" {
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
                    // The session owns both edges, so it closes both. A native
                    // Endpoint reports no peer death, so neither service can
                    // infer the shell is gone — each blocks in `recv` forever
                    // and holds the graph open (B53).
                    shutdown_service();
                    close_console();
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
    match wait(reply.supervision_slot) {
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
    }
    if launch.stdin.is_some() {
        roles |= CAPABILITY_ROLE_STDIN;
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
    if reply.status == 0
        && let Some(stdin) = launch.stdin
        && send_all(STDIN_SEND_SLOT, stdin) < 0
    {
        return error_reply(slime_rt::ERR_BAD_CAP as i32);
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
        supervision_slot: 0,
        detail: 0,
    }
}

fn send_request(request: WireSpawnRequest, caps: &[u32]) -> WireSpawnReply {
    let encoded = request.encode();
    loop {
        let result = match caps {
            [] => slime_rt::send(SPAWN_SLOT, &encoded, &[]),
            [capability] => slime_rt::capability_delegate(
                SPAWN_SLOT,
                *capability,
                CapabilityDisposition::Move,
                OBJECT_KIND_DIRECTORY,
                RIGHT_DIRECTORY_READ as u64,
                &encoded,
            ),
            _ => slime_rt::ERR_INVALID_ARG,
        };
        match result {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => fail(b"spawn request send"),
            _ => break,
        }
    }
    receive_reply()
}

fn receive_reply() -> WireSpawnReply {
    let mut reply = [0u8; MAX_MSG];
    let mut received_caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(SPAWN_SLOT, &mut reply, &mut received_caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => fail(b"spawn reply recv"),
            n => {
                let Some(decoded) = WireSpawnReply::decode(&reply[..n as usize]) else {
                    fail(b"spawn reply decode")
                };
                if !valid_spawn_reply(&decoded) {
                    fail(b"spawn reply shape");
                }
                // A supervision handle has no kernel object to travel in the
                // message, so an export addressed here arrives alone and is
                // claimed rather than read out of the received-capability
                // array, which carries only native Endpoint handles (B46).
                return WireSpawnReply {
                    supervision_slot: slime_rt::capability_import().unwrap_or(0),
                    ..decoded
                };
            }
        }
    }
}

/// Wait directly on the delegated supervision authority.
fn wait(handle: u32) -> Termination {
    loop {
        match slime_rt::supervision_status(handle) {
            Ok(Some(termination)) => return termination,
            Ok(None) => slime_rt::yield_now(),
            Err(_) => return Termination::PeerLoss,
        }
    }
}

/// Write to the console, in message-sized chunks.
///
/// `MAX_LINE_BYTES` is 128 and `MAX_MSG` is 64, so a line long enough to fill
/// half the input buffer cannot cross in one send: the transport refuses an
/// oversized payload with `ERR_INVALID_ARG` before it reaches the kernel. The
/// second scripted line is 65 bytes, which made the echo fail and ended the
/// session one byte past the bound (B53). A buffer larger than one message must
/// not assume one send.
fn console(payload: &[u8]) {
    for chunk in payload.chunks(MAX_MSG) {
        loop {
            match slime_rt::send(CONSOLE_SLOT, chunk, &[]) {
                ERR_WOULDBLOCK => slime_rt::yield_now(),
                result if result < 0 => fail(b"console send"),
                _ => break,
            }
        }
    }
}

/// Ask the spawn service to shut down, and wait for it to stop answering.
///
/// The protocol already carries `REQUEST_FLAG_SHUTDOWN`; nothing sent it, so the
/// service blocked in `recv` for the rest of the boot. A native Endpoint reports
/// no peer death, so the shell that owns the edge is the only party that can say
/// the session is over.
fn shutdown_service() {
    let request = WireSpawnRequest {
        magic: slime_proto::spawn::SPAWN_MAGIC,
        version: slime_proto::spawn::FORMAT_VERSION,
        flags: slime_proto::spawn::REQUEST_FLAG_SHUTDOWN,
        command_len: 0,
        argument_count: 0,
        environment_count: 0,
        capability_roles: 0,
        client_budget: CLIENT_BUDGET,
        command: [0; 16],
        arguments: [0; 8],
        environment: [0; 8],
        grant_rights: 0,
        reserved: [0; 6],
    };
    let encoded = request.encode();
    loop {
        match slime_rt::send(SPAWN_SLOT, &encoded, &[]) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => fail(b"shutdown send"),
            _ => return,
        }
    }
}

/// Close the console, which exits on this exact message.
fn close_console() {
    loop {
        match slime_rt::send(CONSOLE_SLOT, b"SLIME.CONSOLE.CLOSE", &[]) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => fail(b"console close"),
            _ => return,
        }
    }
}
