#![no_std]
#![no_main]

use slime_proto::{
    spawn::{
        CAPABILITY_ROLE_GRANT, CAPABILITY_ROLE_STDERR, CAPABILITY_ROLE_STDIN,
        CAPABILITY_ROLE_STDOUT, CAPABILITY_ROLE_WORKING_DIRECTORY, MAX_GRANTS, REQUEST_FLAG_WAIT,
        REQUEST_LEN, WireSpawnReply, WireSpawnRequest,
    },
    valid_spawn_request,
};
use slime_rt::{
    ERR_BAD_CAP, ERR_INVALID_ARG, ERR_OUT_OF_MEMORY, ERR_PEER_DEAD, ERR_WOULDBLOCK,
    MAX_CAPS_PER_MSG, MAX_MSG, SpawnGrant, Termination,
};

slime_rt::entry!(main);

const RPC_SLOT: u32 = 0;
const STATUS_OK: i32 = 0;
const STATUS_BAD_REQUEST: i32 = ERR_INVALID_ARG as i32;
const STATUS_NOT_ALLOWED: i32 = ERR_BAD_CAP as i32;
const STATUS_BUDGET_EXHAUSTED: i32 = ERR_OUT_OF_MEMORY as i32;
const RIGHT_SEND: u32 = 1;
const RIGHT_RECV: u32 = 2;
const RIGHT_DIRECTORY_READ: u32 = 1 << 19;

include!(concat!(env!("OUT_DIR"), "/command_profile.rs"));

#[derive(Clone, Copy)]
struct LiveChild {
    task_id: u64,
    supervision_slot: u32,
    termination: Option<Termination>,
}

fn main() {
    slime_rt::debug_write(b"[spawn-service] ready\n");
    let mut live = [None; CLIENT_BUDGET];
    loop {
        reap(&mut live);
        let mut message = [0u8; MAX_MSG];
        let mut received_caps = [0u64; MAX_CAPS_PER_MSG];
        match slime_rt::recv(RPC_SLOT, &mut message, &mut received_caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            ERR_PEER_DEAD => slime_rt::exit(0),
            n if n < 0 => slime_rt::exit(1),
            n => {
                slime_rt::debug_write(b"[spawn-service] request\n");
                let reply = handle(&message[..n as usize], &received_caps, &mut live);
                send_reply(reply);
            }
        }
    }
}

fn handle(
    message: &[u8],
    received_caps: &[u64; MAX_CAPS_PER_MSG],
    live: &mut [Option<LiveChild>; CLIENT_BUDGET],
) -> WireSpawnReply {
    let response = handle_inner(message, received_caps, live);
    release_received_caps(received_caps);
    response
}

fn handle_inner(
    message: &[u8],
    received_caps: &[u64; MAX_CAPS_PER_MSG],
    live: &mut [Option<LiveChild>; CLIENT_BUDGET],
) -> WireSpawnReply {
    let Some(request) = WireSpawnRequest::decode(message) else {
        return reply(STATUS_BAD_REQUEST, 0, 0);
    };
    if !valid_request(&request, received_caps) {
        return reply(STATUS_BAD_REQUEST, 0, 0);
    }
    if request.flags == REQUEST_FLAG_WAIT {
        return wait_reply(request_task_id(&request), live);
    }
    let command = &request.command[..request.command_len as usize];
    let Some(profile_index) = COMMAND_PROFILE.iter().position(|entry| entry.0 == command) else {
        return reply(STATUS_NOT_ALLOWED, 0, 0);
    };
    let Some(slot) = live.iter().position(Option::is_none) else {
        return reply(STATUS_BUDGET_EXHAUSTED, 0, 0);
    };

    let mut grants = [SpawnGrant { slot: 0, rights: 0 }; MAX_CAPS_PER_MSG + 1];
    let (context_send, context_recv) = match slime_rt::endpoint_create(3) {
        Ok(pair) => pair,
        Err(error) => return reply(error as i32, 0, 0),
    };
    grants[0] = SpawnGrant {
        slot: context_recv,
        rights: RIGHT_RECV,
    };
    let mut grant_count = 1;
    for (role, rights) in [
        (CAPABILITY_ROLE_WORKING_DIRECTORY, RIGHT_DIRECTORY_READ),
        (CAPABILITY_ROLE_STDIN, RIGHT_RECV),
        (CAPABILITY_ROLE_STDOUT, RIGHT_SEND),
        (CAPABILITY_ROLE_STDERR, RIGHT_SEND),
        (CAPABILITY_ROLE_GRANT, request.grant_rights),
    ] {
        if request.capability_roles & role != 0 {
            grants[grant_count] = SpawnGrant {
                slot: received_caps[grant_count - 1] as u32,
                rights,
            };
            grant_count += 1;
        }
    }

    let executable_slot = COMMAND_PROFILE[profile_index].2;
    slime_rt::debug_write(b"[spawn-service] spawning child\n");

    match slime_rt::spawn(executable_slot, &grants[..grant_count]) {
        Ok(spawned) => {
            if send_context(context_send, &request).is_err() {
                let _ = slime_rt::cap_drop(context_send);
                let _ = slime_rt::cap_drop(context_recv);
                while let Ok(None) = slime_rt::supervision_status(spawned.supervision_slot) {
                    slime_rt::yield_now();
                }
                return reply(STATUS_BAD_REQUEST, 0, 0);
            }
            let _ = slime_rt::cap_drop(context_send);
            let _ = slime_rt::cap_drop(context_recv);
            live[slot] = Some(LiveChild {
                task_id: spawned.task_id,
                supervision_slot: spawned.supervision_slot,
                termination: None,
            });
            reply(STATUS_OK, spawned.task_id, 0)
        }
        Err(error) => {
            let _ = slime_rt::cap_drop(context_send);
            let _ = slime_rt::cap_drop(context_recv);
            reply(error as i32, 0, 0)
        }
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

fn valid_request(request: &WireSpawnRequest, received_caps: &[u64; MAX_CAPS_PER_MSG]) -> bool {
    const SUPPORTED_ROLES: u8 = CAPABILITY_ROLE_WORKING_DIRECTORY
        | CAPABILITY_ROLE_STDIN
        | CAPABILITY_ROLE_STDOUT
        | CAPABILITY_ROLE_STDERR
        | CAPABILITY_ROLE_GRANT;
    let capability_count = request.capability_roles.count_ones() as usize;
    valid_spawn_request(request)
        && request.client_budget as usize == CLIENT_BUDGET
        && request.capability_roles & !SUPPORTED_ROLES == 0
        && request.reserved.iter().all(|byte| *byte == 0)
        && usize::from(request.capability_roles & CAPABILITY_ROLE_GRANT != 0) <= MAX_GRANTS
        && (request.capability_roles & CAPABILITY_ROLE_GRANT != 0) == (request.grant_rights != 0)
        && capability_count <= MAX_CAPS_PER_MSG
        && received_caps[..capability_count]
            .iter()
            .all(|slot| *slot != 0)
        && received_caps[capability_count..]
            .iter()
            .all(|slot| *slot == 0)
}

fn request_task_id(request: &WireSpawnRequest) -> u64 {
    u64::from_le_bytes(request.arguments)
}

fn wait_reply(task_id: u64, live: &mut [Option<LiveChild>; CLIENT_BUDGET]) -> WireSpawnReply {
    let Some(index) = live
        .iter()
        .position(|child| child.is_some_and(|child| child.task_id == task_id))
    else {
        return reply(STATUS_NOT_ALLOWED, 0, 0);
    };
    let Some(child) = live[index] else {
        return reply(STATUS_NOT_ALLOWED, 0, 0);
    };
    let Some(termination) = child.termination else {
        return reply(ERR_WOULDBLOCK as i32, task_id, 0);
    };
    live[index] = None;
    termination_reply(task_id, termination)
}

fn termination_reply(task_id: u64, termination: Termination) -> WireSpawnReply {
    match termination {
        Termination::Exit(status) => detailed_reply(0, 1, task_id, status as u64),
        Termination::Fault(detail) => detailed_reply(0, 2, task_id, detail),
        Termination::Timeout => detailed_reply(0, 3, task_id, 0),
        Termination::PeerLoss => detailed_reply(0, 4, task_id, 0),
        Termination::Unhealthy => detailed_reply(0, 5, task_id, 0),
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

fn send_reply(reply: WireSpawnReply) {
    let encoded = reply.encode();
    loop {
        match slime_rt::send(RPC_SLOT, &encoded, &[]) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            ERR_PEER_DEAD => slime_rt::exit(0),
            result if result < 0 => slime_rt::exit(1),
            _ => return,
        }
    }
}

const fn reply(status: i32, task_id: u64, supervision_slot: u32) -> WireSpawnReply {
    detailed_reply(status, 0, task_id, supervision_slot as u64)
}

const fn detailed_reply(
    status: i32,
    termination_kind: u32,
    task_id: u64,
    detail: u64,
) -> WireSpawnReply {
    WireSpawnReply {
        magic: slime_proto::spawn::SPAWN_MAGIC,
        version: slime_proto::spawn::FORMAT_VERSION,
        status,
        termination_kind,
        task_id,
        supervision_slot: 0,
        detail,
    }
}

const _: () = assert!(REQUEST_LEN == MAX_MSG);
const _: () = assert!(CLIENT_BUDGET > 0);
