#![no_std]
#![no_main]

use slime_proto::{
    spawn::{
        CAPABILITY_ROLE_GRANT, CAPABILITY_ROLE_STDERR, CAPABILITY_ROLE_STDIN,
        CAPABILITY_ROLE_STDOUT, CAPABILITY_ROLE_WORKING_DIRECTORY, MAX_GRANTS, REQUEST_LEN,
        WireSpawnReply, WireSpawnRequest,
    },
    valid_spawn_request,
};
use slime_rt::{
    ERR_BAD_CAP, ERR_INVALID_ARG, ERR_OUT_OF_MEMORY, ERR_PEER_DEAD, ERR_WOULDBLOCK,
    MAX_CAPS_PER_MSG, MAX_MSG, SpawnGrant,
};

slime_rt::entry!(main);

const RPC_SLOT: u32 = 0;
const FACTORY_SLOT: u32 = 1;
const STATUS_OK: i32 = 0;
const STATUS_BAD_REQUEST: i32 = ERR_INVALID_ARG as i32;
const STATUS_NOT_ALLOWED: i32 = ERR_BAD_CAP as i32;
const STATUS_BUDGET_EXHAUSTED: i32 = ERR_OUT_OF_MEMORY as i32;
const RIGHT_SEND: u32 = 1;
const RIGHT_RECV: u32 = 2;

include!(concat!(env!("OUT_DIR"), "/command_profile.rs"));

#[derive(Clone, Copy)]
struct LiveChild {
    supervision_slot: u32,
}

fn main() {
    slime_rt::debug_write(b"[spawn-service] ready\n");
    let (control_send, control_recv) = match slime_rt::endpoint_create(FACTORY_SLOT) {
        Ok(pair) => pair,
        Err(_) => slime_rt::exit(1),
    };
    let mut live = [None; CLIENT_BUDGET];
    loop {
        reap(&mut live);
        let mut message = [0u8; MAX_MSG];
        let mut received_caps = [0u64; MAX_CAPS_PER_MSG];
        match slime_rt::recv(RPC_SLOT, &mut message, &mut received_caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            ERR_PEER_DEAD => {
                shutdown_children(control_send, &mut live);
                slime_rt::exit(0)
            }
            n if n < 0 => slime_rt::exit(1),
            n => {
                slime_rt::debug_write(b"[spawn-service] request\n");
                let reply = handle(
                    &message[..n as usize],
                    &received_caps,
                    control_recv,
                    &mut live,
                );
                send_reply(reply);
            }
        }
    }
}

fn handle(
    message: &[u8],
    received_caps: &[u64; MAX_CAPS_PER_MSG],
    control_recv: u32,
    live: &mut [Option<LiveChild>; CLIENT_BUDGET],
) -> WireSpawnReply {
    let response = handle_inner(message, received_caps, control_recv, live);
    release_received_caps(received_caps);
    response
}

fn handle_inner(
    message: &[u8],
    received_caps: &[u64; MAX_CAPS_PER_MSG],
    control_recv: u32,
    live: &mut [Option<LiveChild>; CLIENT_BUDGET],
) -> WireSpawnReply {
    let Some(request) = WireSpawnRequest::decode(message) else {
        return reply(STATUS_BAD_REQUEST, 0, 0);
    };
    if !valid_request(&request, received_caps) {
        return reply(STATUS_BAD_REQUEST, 0, 0);
    }
    let command = &request.command[..request.command_len as usize];
    let Some(profile_index) = COMMAND_PROFILE.iter().position(|entry| entry.0 == command) else {
        return reply(STATUS_NOT_ALLOWED, 0, 0);
    };
    let Some(slot) = live.iter().position(Option::is_none) else {
        return reply(STATUS_BUDGET_EXHAUSTED, 0, 0);
    };

    let mut grants = [SpawnGrant { slot: 0, rights: 0 }; MAX_CAPS_PER_MSG + 1];
    let mut grant_count = 0;
    for (role, rights) in [
        (CAPABILITY_ROLE_STDIN, RIGHT_RECV),
        (CAPABILITY_ROLE_STDOUT, RIGHT_SEND),
        (CAPABILITY_ROLE_STDERR, RIGHT_SEND),
        (CAPABILITY_ROLE_GRANT, request.grant_rights),
    ] {
        if request.capability_roles & role != 0 {
            grants[grant_count] = SpawnGrant {
                slot: received_caps[grant_count] as u32,
                rights,
            };
            grant_count += 1;
        }
    }
    grants[grant_count] = SpawnGrant {
        slot: control_recv,
        rights: RIGHT_RECV,
    };
    grant_count += 1;

    let executable_slot = COMMAND_PROFILE[profile_index].2;
    slime_rt::debug_write(b"[spawn-service] spawning child\n");
    match slime_rt::spawn(executable_slot, &grants[..grant_count]) {
        Ok(spawned) => {
            slime_rt::debug_write(b"[spawn-service] child spawned\n");
            live[slot] = Some(LiveChild {
                supervision_slot: spawned.supervision_slot,
            });
            reply(STATUS_OK, spawned.task_id, spawned.supervision_slot)
        }
        Err(error) => reply(error as i32, 0, 0),
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
    const SUPPORTED_ROLES: u8 = CAPABILITY_ROLE_STDIN
        | CAPABILITY_ROLE_STDOUT
        | CAPABILITY_ROLE_STDERR
        | CAPABILITY_ROLE_GRANT;
    let capability_count = request.capability_roles.count_ones() as usize;
    valid_spawn_request(request)
        && request.flags == 0
        && request.client_budget as usize == CLIENT_BUDGET
        && request.capability_roles & CAPABILITY_ROLE_WORKING_DIRECTORY == 0
        && request.capability_roles & !SUPPORTED_ROLES == 0
        && request.argument_count == 0
        && request.environment_count == 0
        && request.arguments.iter().all(|byte| *byte == 0)
        && request.environment.iter().all(|byte| *byte == 0)
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

fn reap(live: &mut [Option<LiveChild>; CLIENT_BUDGET]) {
    for child in live.iter_mut() {
        let Some(live_child) = *child else {
            continue;
        };
        match slime_rt::supervision_status(live_child.supervision_slot) {
            Ok(None) => {}
            Ok(Some(_)) => *child = None,
            Err(_) => slime_rt::exit(1),
        }
    }
}

fn shutdown_children(control_send: u32, live: &mut [Option<LiveChild>; CLIENT_BUDGET]) {
    let mut remaining = live.iter().filter(|child| child.is_some()).count();
    while remaining > 0 {
        match slime_rt::send(control_send, b"shutdown", &[]) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => slime_rt::exit(1),
            _ => remaining -= 1,
        }
    }
    while live.iter().any(Option::is_some) {
        reap(live);
        slime_rt::yield_now();
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
    WireSpawnReply {
        magic: slime_proto::spawn::SPAWN_MAGIC,
        version: slime_proto::spawn::FORMAT_VERSION,
        status,
        termination_kind: 0,
        task_id,
        supervision_slot,
        detail: 0,
    }
}

const _: () = assert!(REQUEST_LEN == MAX_MSG);
const _: () = assert!(CLIENT_BUDGET > 0);
