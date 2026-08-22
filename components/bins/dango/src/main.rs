#![no_std]
#![no_main]

use boot_contracts::generation::BootAction;
use slime_components::dango_runtime::{Launch, MAX_LINE_BYTES, parse};
use slime_components::generation_composition;
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

// B59: rights bit numbering is generated from
// `contracts/generation/v5/schema.zt`. The powerbox/fs protocols carry a
// 32-bit rights field, so the generated `u64` constants are narrowed here
// rather than re-spelled as separate `u32` literals.
const RIGHT_TRANSFER: u32 = boot_contracts::generation::RIGHT_TRANSFER as u32;
const RIGHT_DIRECTORY_READ: u32 = boot_contracts::generation::RIGHT_DIRECTORY_READ as u32;

const CONSOLE_SLOT: u32 = 1;
const INPUT_SLOT: u32 = 2;
const CWD_ROOT_SLOT: u32 = 3;
/// Preinstalled sender paired with echo-agent's declared stdin endpoint.
const STDIN_SEND_SLOT: u32 = 4;
const SHARED_BUFFER_FACTORY_SLOT: u32 = 5;
// A free page-aligned user address, borrowed only for the startup self-check.
const SHARED_BUFFER_PROBE_BASE: u64 = 0x0000_0005_0000_0000;

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

/// Everything about this session that the generation declares rather than the
/// source states: the spawn service's endpoint and the budget both ends admit
/// against.
///
/// Resolved once before the first keystroke and carried, so the request path
/// makes no syscall a positional constant used to make for free, and so a
/// generation that declares neither fact fails at startup rather than on the
/// first command.
#[derive(Clone, Copy)]
struct Session {
    spawn_slot: u32,
    budget: u8,
}

impl Session {
    /// Ask the root for both facts.
    ///
    /// The endpoint is named: `SPAWN_SLOT` was a positional constant beside a
    /// `build.rs`-generated table, and the name asks the root for this
    /// instance's own binding instead, so the image carries no generation's
    /// numbering (CP2/B70). The role query cannot answer it -- this session also
    /// holds a console endpoint and a stdin sender of the same kind and rights.
    ///
    /// The budget replaces `CLIENT_BUDGET`, parsed out of one generation
    /// manifest by this crate's build script. Every request states the budget it
    /// believes in and the service refuses a disagreement, so both ends must
    /// read one authenticated source; the generation declares the same
    /// `spawnBudget` for the shell and the service, so each asking for its own
    /// asks one question.
    ///
    /// Fatal when the root will not say, on the rule [`scripted_plane`]
    /// follows: a guessed budget would be carried by every request and refused
    /// by the service, presenting as an unexplained denial rather than a missing
    /// declaration. Narrowed to the wire field's width here, so an over-wide
    /// declaration fails at its source rather than truncating into a different
    /// number.
    fn resolve() -> Self {
        let spawn_slot = slime_rt::resolve_binding(b"spawn-service-rpc")
            .unwrap_or_else(|_| fail(b"the generation declares no spawn-service endpoint here"));
        let budget = match slime_rt::spawn_budget() {
            Ok(budget) => {
                u8::try_from(budget).unwrap_or_else(|_| fail(b"declared budget exceeds the wire"))
            }
            Err(_) => fail(b"the root did not answer this session's spawn budget"),
        };
        Self { spawn_slot, budget }
    }
}

/// Whether this session is the scripted `dango` plane rather than an
/// interactive console.
///
/// The plane drives input from a script and asserts on the echoed transcript,
/// so it echoes whole lines at Enter while an interactive session echoes each
/// keystroke. Read from the root (B70) rather than from a `build.rs`-private
/// per-plane string, which is what let this component be built only inside this
/// crate.
///
/// Fatal when the root cannot say, rather than defaulting. Three of the four
/// call sites are `!scripted_plane()`, so this is the one migrated predicate
/// where an unanswered query does not fall through to "not this plane" but
/// actively selects the *other* echo mode. The two modes emit the same bytes in
/// a different order, so the difference is invisible to a marker-based
/// transcript assertion -- measured, not assumed: forcing the root to answer
/// the wrong composition left `just sel4_dango_check` green. A mode this
/// component cannot verify and no gate can observe must not be guessed.
fn scripted_plane() -> bool {
    match generation_composition::boot_action() {
        Some(action) => action == BootAction::Dango,
        None => fail(b"the root did not answer which composition this session is"),
    }
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
    let session = Session::resolve();
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
                    if !scripted_plane() {
                        console(&[character as u8]);
                    }
                }
                InputKey::Space if len < line.len() => {
                    line[len] = b' ';
                    len += 1;
                    if !scripted_plane() {
                        console(b" ");
                    }
                }
                InputKey::Backspace if len > 0 => {
                    len -= 1;
                    if !scripted_plane() {
                        console(b"\x08 \x08");
                    }
                }
                InputKey::Enter => {
                    if scripted_plane() {
                        console(&line[..len]);
                    }
                    console(b"\n");
                    if len != 0 {
                        evaluate(session, &line[..len]);
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
                    shutdown_service(session);
                    close_console();
                    return;
                }
                _ => {}
            },
        }
    }
}

fn evaluate(session: Session, line: &[u8]) {
    let launch = match parse(line) {
        Ok(launch) => launch,
        Err(_) => {
            console(b"parse-error\n");
            return;
        }
    };
    // The spawn service is the only authority on which commands exist, so the
    // request is sent and its answer reported. This shell used to hold its own
    // `COMMAND_NAMES` copy of the profile, generated into this crate's `OUT_DIR`
    // from one manifest, and refused locally before asking (B70). That copy was
    // never the decision -- the service re-checked every name against its own
    // table -- so removing it drops a duplicated policy rather than a check, and
    // the denial is now reported by the party that owns it.
    let Some(reply) = spawn(session, &launch) else {
        console(b"spawn-error\n");
        return;
    };
    if reply.status == slime_rt::ERR_BAD_CAP as i32 {
        console(b"resolve-denied\n");
        return;
    }
    if reply.status != 0 {
        console(b"spawn-error\n");
        return;
    }
    // Resolution is reported before the acceptance it implies: the service
    // resolves the command's declared executable and launch context before it
    // spawns anything, so an accepted reply is evidence of both, in that order.
    console(b"resolved:profile\n");
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

/// One request's outcome, distinguishing a local failure from the service's own
/// answer.
///
/// `None` is this session failing before the service was consulted -- a working
/// directory it could not derive, or a stdin payload it could not deliver.
/// `Some` is the reply the service sent, whose status is the service's decision
/// and is reported as such. The two used to be collapsed into one synthetic
/// reply carrying `ERR_BAD_CAP`, which is now the service's denial code and so
/// can no longer double as a local error.
fn spawn(session: Session, launch: &Launch<'_>) -> Option<WireSpawnReply> {
    let mut command = [0u8; 16];
    command[..launch.command.len()].copy_from_slice(launch.command);
    let mut roles = 0;
    let mut caps = [0u32; MAX_CAPS_PER_MSG];
    let mut cap_count = 0;

    if let Some(cwd) = launch.cwd {
        let derived =
            slime_rt::directory_derive(CWD_ROOT_SLOT, cwd, RIGHT_DIRECTORY_READ | RIGHT_TRANSFER)
                .ok()?;
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
        client_budget: session.budget,
        command,
        arguments: launch.arguments.bytes,
        environment: launch.environment.bytes,
        grant_rights: 0,
        reserved: [0; 6],
    };
    let reply = send_request(session, request, &caps[..cap_count]);
    if reply.status == 0
        && let Some(stdin) = launch.stdin
        && send_all(STDIN_SEND_SLOT, stdin) < 0
    {
        return None;
    }
    Some(reply)
}

fn send_all(slot: u32, payload: &[u8]) -> i64 {
    loop {
        match slime_rt::send(slot, payload, &[]) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            result => return result,
        }
    }
}

fn send_request(session: Session, request: WireSpawnRequest, caps: &[u32]) -> WireSpawnReply {
    let encoded = request.encode();
    loop {
        let result = match caps {
            [] => slime_rt::send(session.spawn_slot, &encoded, &[]),
            [capability] => slime_rt::capability_delegate(
                session.spawn_slot,
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
    receive_reply(session)
}

fn receive_reply(session: Session) -> WireSpawnReply {
    let mut reply = [0u8; MAX_MSG];
    let mut received_caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(session.spawn_slot, &mut reply, &mut received_caps) {
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
fn shutdown_service(session: Session) {
    let request = WireSpawnRequest {
        magic: slime_proto::spawn::SPAWN_MAGIC,
        version: slime_proto::spawn::FORMAT_VERSION,
        flags: slime_proto::spawn::REQUEST_FLAG_SHUTDOWN,
        command_len: 0,
        argument_count: 0,
        environment_count: 0,
        capability_roles: 0,
        client_budget: session.budget,
        command: [0; 16],
        arguments: [0; 8],
        environment: [0; 8],
        grant_rights: 0,
        reserved: [0; 6],
    };
    let encoded = request.encode();
    loop {
        match slime_rt::send(session.spawn_slot, &encoded, &[]) {
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
