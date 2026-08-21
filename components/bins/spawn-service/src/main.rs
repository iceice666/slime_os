#![no_std]
#![no_main]

use slime_proto::{
    capability_transfer::OBJECT_KIND_SUPERVISION,
    spawn::{
        CAPABILITY_ROLE_STDIN, CAPABILITY_ROLE_WORKING_DIRECTORY, REQUEST_FLAG_SHUTDOWN,
        REQUEST_FLAG_WAIT, REQUEST_LEN, WireSpawnReply, WireSpawnRequest,
    },
    valid_spawn_request,
};
use slime_rt::{
    CapabilityDisposition, ERR_BAD_CAP, ERR_INVALID_ARG, ERR_OUT_OF_MEMORY, ERR_WOULDBLOCK,
    MAX_CAPS_PER_MSG, MAX_MSG, SpawnGrant, Termination,
};
// B59: the capability-rights vocabulary is generated from
// `contracts/generation/v5/schema.zt`; these were local copies of the same
// bit numbering.
use boot_contracts::generation::{RIGHT_DIRECTORY_READ, RIGHT_SUPERVISE};

slime_rt::entry!(main);

const STATUS_OK: i32 = 0;
const STATUS_BAD_REQUEST: i32 = ERR_INVALID_ARG as i32;
const STATUS_NOT_ALLOWED: i32 = ERR_BAD_CAP as i32;
const STATUS_BUDGET_EXHAUSTED: i32 = ERR_OUT_OF_MEMORY as i32;
const SYSINFO_CONTEXT_SLOT: u32 = 3;
const ECHO_CONTEXT_SLOT: u32 = 6;
// A free page-aligned user address, borrowed only for the startup self-check.
const SHARED_BUFFER_PROBE_BASE: u64 = 0x0000_0004_0000_0000;

include!(concat!(env!("OUT_DIR"), "/command_profile.rs"));

#[derive(Clone, Copy)]
struct LiveChild {
    supervision_slot: u32,
    termination: Option<Termination>,
}

fn main(_startup_arg: u32) {
    slime_rt::debug_write(b"[spawn-service] ready\n");
    // The shared-buffer factory slot is resolved by capability *role* rather than
    // read from the generated table (CP2/B70). Role, not grant name: grant names
    // differ per generation -- this component's echo executable is
    // `spawn-service-echo` under `valid.zti` and `spawn-service-echo-agent` under
    // `sel4-dango.zti` -- so a name written here would couple this source to one
    // manifest, which is the coupling being removed. Kind plus rights is the
    // question `components/bins/build.rs` already asked of the manifest, now asked
    // of the root at runtime.
    //
    // Only the factory is unambiguous in every generation declaring this
    // component: exactly one `bufferCreate` capability. The command executables
    // are not resolved this way either -- two or three share one kind and rights
    // set -- so `COMMAND_PROFILE` still supplies them.
    let factory_slot = slime_rt::resolve_binding(b"kind:sharedBufferFactory+bufferCreate")
        .unwrap_or_else(|_| slime_rt::exit(1));
    // The RPC endpoint is *not* resolved by role: `sel4-dango.zti` grants this
    // component three `send`+`recv` endpoints -- the RPC channel plus one context
    // endpoint per command -- so the role query is ambiguous and refuses. That
    // refusal is correct: which of the three carries requests is a fact about the
    // graph's shape rather than about the capability, so `RPC_SLOT` stays derived
    // until a binding carries a logical role the component can name.
    let rpc_slot = RPC_SLOT;
    // C7.2/C7.3: prove this component's generation-declared shared-buffer
    // quota is live before serving requests. A failure here is fatal: the
    // generation granted authority the kernel did not honour.
    if !slime_components::shared_buffer_probe::probe_and_report(
        b"[spawn-service]",
        factory_slot,
        SHARED_BUFFER_PROBE_BASE,
    ) {
        slime_rt::exit(1);
    }
    let mut live = [None; CLIENT_BUDGET];
    loop {
        reap(&mut live);
        let mut message = [0u8; MAX_MSG];
        let mut received_caps = [0u64; MAX_CAPS_PER_MSG];
        match slime_rt::recv(rpc_slot, &mut message, &mut received_caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => slime_rt::exit(1),
            n => {
                slime_rt::debug_write(b"[spawn-service] request\n");
                if shutdown_requested(&message[..n as usize], &received_caps) {
                    slime_rt::debug_write(b"[spawn-service] shutdown received\n");
                    while live
                        .iter()
                        .flatten()
                        .any(|child| child.termination.is_none())
                    {
                        reap(&mut live);
                        slime_rt::yield_now();
                    }
                    slime_rt::debug_write(b"[spawn-service] complete\n");
                    slime_rt::exit(0);
                }
                let (reply, supervision) =
                    handle(&message[..n as usize], &received_caps, &mut live);
                send_reply(rpc_slot, reply, supervision);
            }
        }
    }
}

fn shutdown_requested(message: &[u8], received_caps: &[u64; MAX_CAPS_PER_MSG]) -> bool {
    let Some(request) = WireSpawnRequest::decode(message) else {
        return false;
    };
    request.flags == REQUEST_FLAG_SHUTDOWN
        && valid_spawn_request(&request)
        && received_caps.iter().all(|slot| *slot == 0)
}

fn handle(
    message: &[u8],
    received_caps: &[u64; MAX_CAPS_PER_MSG],
    live: &mut [Option<LiveChild>; CLIENT_BUDGET],
) -> (WireSpawnReply, Option<u32>) {
    // A working-directory capability has no kernel object to travel in the
    // message, so its export arrives alone and is claimed here rather than read
    // out of the received-capability array, which since B46 carries only native
    // Endpoint handles. Claimed before validation so a refused request still
    // releases the authority its client handed over.
    let claimed = slime_rt::capability_import().ok();
    let response = handle_inner(message, claimed, live);
    release_received_caps(received_caps);
    if response.0.status != STATUS_OK
        && let Some(slot) = claimed
        && slime_rt::cap_drop(slot) != 0
    {
        slime_rt::exit(1);
    }
    response
}

fn handle_inner(
    message: &[u8],
    claimed: Option<u32>,
    live: &mut [Option<LiveChild>; CLIENT_BUDGET],
) -> (WireSpawnReply, Option<u32>) {
    let Some(request) = WireSpawnRequest::decode(message) else {
        return (reply(STATUS_BAD_REQUEST, 0), None);
    };
    if !valid_request(&request, claimed) {
        return (reply(STATUS_BAD_REQUEST, 0), None);
    }
    if request.flags == REQUEST_FLAG_WAIT {
        return (wait_reply(request_handle(&request), live), None);
    }
    let command = &request.command[..request.command_len as usize];
    let Some(profile_index) = COMMAND_PROFILE.iter().position(|entry| entry.0 == command) else {
        return (reply(STATUS_NOT_ALLOWED, 0), None);
    };
    let Some(slot) = live.iter().position(Option::is_none) else {
        return (reply(STATUS_BUDGET_EXHAUSTED, 0), None);
    };

    let context_slot = match command {
        b"sysinfo" => SYSINFO_CONTEXT_SLOT,
        b"echo" => ECHO_CONTEXT_SLOT,
        _ => return (reply(STATUS_NOT_ALLOWED, 0), None),
    };
    let executable_slot = COMMAND_PROFILE[profile_index].2;
    slime_rt::debug_write(b"[spawn-service] spawning child\n");

    let directory_grant = [SpawnGrant {
        slot: claimed.unwrap_or(0),
        rights: RIGHT_DIRECTORY_READ,
    }];
    let grants = if request.capability_roles & CAPABILITY_ROLE_WORKING_DIRECTORY != 0 {
        &directory_grant[..]
    } else {
        &directory_grant[..0]
    };
    match slime_rt::spawn(executable_slot, grants) {
        Ok(spawned) => {
            if send_context(context_slot, &request).is_err() {
                while let Ok(None) = slime_rt::supervision_status(spawned.supervision_slot) {
                    slime_rt::yield_now();
                }
                return (reply(STATUS_BAD_REQUEST, 0), None);
            }
            live[slot] = Some(LiveChild {
                supervision_slot: spawned.supervision_slot,
                termination: None,
            });
            (
                reply(STATUS_OK, spawned.supervision_slot),
                Some(spawned.supervision_slot),
            )
        }
        Err(error) => (reply(error as i32, 0), None),
    }
}

fn send_context(slot: u32, request: &WireSpawnRequest) -> Result<(), i64> {
    let encoded = request.encode();
    loop {
        match slime_rt::send(slot, &encoded, &[]) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => return Err(result),
            _ => return Ok(()),
        }
    }
}

fn release_received_caps(received_caps: &[u64; MAX_CAPS_PER_MSG]) {
    for slot in received_caps.iter().copied().filter(|slot| *slot != 0) {
        if slime_rt::cap_drop(slot as u32) != 0 {
            slime_rt::exit(1);
        }
    }
}

/// Structural validity, plus the rule that a declared role and a delivered
/// capability must agree: a request claiming a working directory must have
/// brought one, and a request claiming none must not.
fn valid_request(request: &WireSpawnRequest, claimed: Option<u32>) -> bool {
    const SUPPORTED_ROLES: u8 = CAPABILITY_ROLE_WORKING_DIRECTORY | CAPABILITY_ROLE_STDIN;
    let wants_directory = request.capability_roles & CAPABILITY_ROLE_WORKING_DIRECTORY != 0;
    valid_spawn_request(request)
        && request.client_budget as usize == CLIENT_BUDGET
        && request.capability_roles & !SUPPORTED_ROLES == 0
        && request.reserved.iter().all(|byte| *byte == 0)
        && request.grant_rights == 0
        && wants_directory == claimed.is_some()
}

/// The supervision handle a wait request names. The client holds this
/// capability; the numeric task id it used to send is not authority and is
/// gone from the protocol (B42).
fn request_handle(request: &WireSpawnRequest) -> u32 {
    u32::from_le_bytes([
        request.arguments[0],
        request.arguments[1],
        request.arguments[2],
        request.arguments[3],
    ])
}

fn wait_reply(handle: u32, live: &mut [Option<LiveChild>; CLIENT_BUDGET]) -> WireSpawnReply {
    let Some(index) = live
        .iter()
        .position(|child| child.is_some_and(|child| child.supervision_slot == handle))
    else {
        return reply(STATUS_NOT_ALLOWED, 0);
    };
    let Some(child) = live[index] else {
        return reply(STATUS_NOT_ALLOWED, 0);
    };
    let Some(termination) = child.termination else {
        return reply(ERR_WOULDBLOCK as i32, handle);
    };
    live[index] = None;
    termination_reply(handle, termination)
}

fn termination_reply(handle: u32, termination: Termination) -> WireSpawnReply {
    match termination {
        Termination::Exit(status) => detailed_reply(0, 1, handle, status as u64),
        Termination::Fault(detail) => detailed_reply(0, 2, handle, detail),
        Termination::Timeout => detailed_reply(0, 3, handle, 0),
        Termination::PeerLoss => detailed_reply(0, 4, handle, 0),
        Termination::Unhealthy => detailed_reply(0, 5, handle, 0),
    }
}

fn reap(live: &mut [Option<LiveChild>; CLIENT_BUDGET]) {
    for child in live.iter_mut().flatten() {
        if child.termination.is_some() {
            continue;
        }
        match slime_rt::supervision_status(child.supervision_slot) {
            Ok(None) => {}
            Ok(Some(termination)) => child.termination = Some(termination),
            Err(_) => child.termination = Some(Termination::PeerLoss),
        }
    }
}

fn send_reply(rpc_slot: u32, reply: WireSpawnReply, supervision: Option<u32>) {
    let encoded = reply.encode();
    loop {
        let result = match supervision {
            Some(slot) => {
                let transfer =
                    slime_rt::supervision_derive(slot).unwrap_or_else(|_| slime_rt::exit(1));
                let result = slime_rt::capability_delegate(
                    rpc_slot,
                    transfer,
                    CapabilityDisposition::Move,
                    OBJECT_KIND_SUPERVISION,
                    RIGHT_SUPERVISE,
                    &encoded,
                );
                if result < 0 {
                    let _ = slime_rt::cap_drop(transfer);
                }
                result
            }
            None => slime_rt::send(rpc_slot, &encoded, &[]),
        };
        match result {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => slime_rt::exit(1),
            _ => return,
        }
    }
}

const fn reply(status: i32, supervision_slot: u32) -> WireSpawnReply {
    detailed_reply(status, 0, supervision_slot, 0)
}

const fn detailed_reply(
    status: i32,
    termination_kind: u32,
    handle: u32,
    detail: u64,
) -> WireSpawnReply {
    WireSpawnReply {
        magic: slime_proto::spawn::SPAWN_MAGIC,
        version: slime_proto::spawn::FORMAT_VERSION,
        status,
        termination_kind,
        supervision_slot: handle,
        detail,
    }
}

const _: () = assert!(REQUEST_LEN == MAX_MSG);
const _: () = assert!(CLIENT_BUDGET > 0);
