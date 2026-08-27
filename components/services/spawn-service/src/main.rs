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
use boot_contracts::generation::{MAX_SPAWN_BUDGET, RIGHT_DIRECTORY_READ, RIGHT_SUPERVISE};

slime_rt::entry!(main);

const STATUS_OK: i32 = 0;
const STATUS_BAD_REQUEST: i32 = ERR_INVALID_ARG as i32;
const STATUS_NOT_ALLOWED: i32 = ERR_BAD_CAP as i32;
const STATUS_BUDGET_EXHAUSTED: i32 = ERR_OUT_OF_MEMORY as i32;
/// The root refused a spawn this service had already authorized.
///
/// Its own code rather than the root's: `ERR_BAD_CAP` is `STATUS_NOT_ALLOWED`
/// here, so forwarding it would tell a client its command is undeclared when
/// the service had in fact resolved and authorized it. `ERR_OUT_OF_MEMORY` is
/// likewise taken by the budget arm. `ERR_PEER_DEAD` names neither an
/// authorization nor a budget outcome and no other arm produces it, so it
/// carries "authorized, not delivered" unambiguously; the root's own code
/// travels in the reply's `detail`.
const STATUS_SPAWN_REFUSED: i32 = slime_rt::ERR_PEER_DEAD as i32;
// A free page-aligned user address, borrowed only for the startup self-check.
const SHARED_BUFFER_PROBE_BASE: u64 = 0x0000_0004_0000_0000;

/// Storage for the live-child table, sized by the *published* ceiling on a
/// declared spawn budget rather than by any one generation's value.
///
/// `CLIENT_BUDGET` used to be a build-script constant parsed out of one
/// generation manifest and used in type position, which is what made this
/// component buildable only against that manifest (B70). The contract's
/// `MAX_SPAWN_BUDGET` is the bound `boot_contracts` already enforces on every
/// admitted generation, so it sizes the array; the generation's own number is a
/// runtime admission bound over that array, read once at startup.
const MAX_LIVE_CHILDREN: usize = MAX_SPAWN_BUDGET as usize;

/// Longest binding name this component composes: `spawn-service-` plus a
/// 16-byte command plus `-context`.
const MAX_COMMAND_BINDING: usize = COMMAND_BINDING_PREFIX.len()
    + slime_proto::spawn::MAX_COMMAND_BYTES
    + COMMAND_CONTEXT_SUFFIX.len();

/// The grant namespace a command's authority is declared in.
///
/// A command is authorized by the *existence* of `spawn-service-<command>` in
/// this instance's own bindings, and launched through the slot that binding
/// names. Both facts come from the root's view of the authenticated generation,
/// so an undeclared command has no binding to resolve and is refused before
/// anything is spawned — the check the build-script `COMMAND_PROFILE` table
/// performed against a manifest this image was compiled against.
///
/// A name rather than a capability role, because a role cannot answer this:
/// every command executable is `executable+exec,spawn`, so the role query
/// correctly refuses two or three identical answers. The names are uniform
/// across every generation declaring this service, which is what makes them
/// safe to write here where a per-plane spelling was not.
const COMMAND_BINDING_PREFIX: &[u8] = b"spawn-service-";

/// Suffix naming the launch-context endpoint paired with a command.
const COMMAND_CONTEXT_SUFFIX: &[u8] = b"-context";

#[derive(Clone, Copy)]
struct LiveChild {
    supervision_slot: u32,
    termination: Option<Termination>,
}

fn main(_startup_arg: u32) {
    slime_rt::debug_write(b"[spawn-service] ready\n");
    // The shared-buffer factory slot is resolved by capability *role* rather
    // than read from a generated table (CP2/B70): exactly one `bufferCreate`
    // capability is granted in every generation declaring this component, so
    // kind plus rights identifies it without naming a grant.
    let factory_slot = slime_rt::resolve_binding(b"kind:sharedBufferFactory+bufferCreate")
        .unwrap_or_else(|_| slime_rt::exit(1));
    // The request endpoint is resolved by its stable generation binding name.
    // A capability-role query can be ambiguous when this service also holds
    // launch-context endpoints, so refusing ambiguity and asking by name avoids
    // a plausible but wrong lowest-slot answer.
    let rpc_slot =
        slime_rt::resolve_binding(b"spawn-service-rpc").unwrap_or_else(|_| slime_rt::exit(1));
    let budget = declared_budget();
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
    let mut live = [None; MAX_LIVE_CHILDREN];
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
                    handle(&message[..n as usize], &received_caps, &mut live, budget);
                send_reply(rpc_slot, reply, supervision);
            }
        }
    }
}

/// This instance's declared live-child budget, asked of the root.
///
/// The number is authenticated generation data, not a client's claim: it is the
/// same `spawnBudget` the root itself uses to bound `serve_spawn`, so admitting
/// one request per live child up to it can never exceed what the root would
/// honour. Every request states the budget it believes in and `valid_request`
/// refuses a disagreement, which is why this must be resolved before the first
/// request is served rather than derived from one.
///
/// Fatal on refusal or on a value outside the published ceiling. A refusal means
/// the generation grants this instance no spawn authority, so a spawn service is
/// exactly what it cannot be; a zero budget means it may hold no child, which is
/// the same. Continuing with a guess would serve requests against a bound the
/// root does not share.
fn declared_budget() -> usize {
    match slime_rt::spawn_budget() {
        Ok(budget) if budget >= 1 && usize::from(budget) <= MAX_LIVE_CHILDREN => {
            usize::from(budget)
        }
        _ => {
            slime_rt::debug_write(b"[spawn-service] no declared spawn budget\n");
            slime_rt::exit(1)
        }
    }
}

/// Whether this is the client's request to stop serving.
///
/// Deliberately does *not* check the stated budget, where a launch request
/// does: a shutdown launches nothing, so there is no admission bound for the
/// two ends to agree on, and `init`'s product-graph shutdown states zero. The
/// authority to close the service is the endpoint the request arrived on.
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
    live: &mut [Option<LiveChild>; MAX_LIVE_CHILDREN],
    budget: usize,
) -> (WireSpawnReply, Option<u32>) {
    // A working-directory capability has no kernel object to travel in the
    // message, so its export arrives alone and is claimed here rather than read
    // out of the received-capability array, which since B46 carries only native
    // Endpoint handles. Claimed before validation so a refused request still
    // releases the authority its client handed over.
    let claimed = slime_rt::capability_import().ok();
    let response = handle_inner(message, claimed, live, budget);
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
    live: &mut [Option<LiveChild>; MAX_LIVE_CHILDREN],
    budget: usize,
) -> (WireSpawnReply, Option<u32>) {
    let Some(request) = WireSpawnRequest::decode(message) else {
        return (reply(STATUS_BAD_REQUEST, 0), None);
    };
    if !valid_request(&request, claimed, budget) {
        return (reply(STATUS_BAD_REQUEST, 0), None);
    }
    if request.flags == REQUEST_FLAG_WAIT {
        return (wait_reply(request_handle(&request), live), None);
    }
    let command = &request.command[..request.command_len as usize];
    // Authorization and dispatch are one question asked of the authenticated
    // generation: a command exists for this service exactly when the generation
    // binds it an executable *and* a launch context. Both are resolved before
    // anything is spawned, so a name the generation does not declare -- or one
    // that names only half the pair -- is refused rather than half-served.
    let Some(executable_slot) = command_binding(command, b"") else {
        return (reply(STATUS_NOT_ALLOWED, 0), None);
    };
    let Some(context_slot) = command_binding(command, COMMAND_CONTEXT_SUFFIX) else {
        return (reply(STATUS_NOT_ALLOWED, 0), None);
    };
    // Admission against the generation's declared budget, over the published
    // capacity this table is sized for. The prefix is what the generation
    // authorizes; the rest of the array is storage, not permission.
    let Some(slot) = live
        .get(..budget)
        .and_then(|admitted| admitted.iter().position(Option::is_none))
    else {
        return (reply(STATUS_BUDGET_EXHAUSTED, 0), None);
    };
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
        // A root refusal is this service's failure to deliver an authorized
        // spawn, not a refusal of the request. The distinct status keeps the
        // service's authorization decision separate; the root's code travels in
        // `detail` as diagnosis rather than policy.
        Err(error) => (
            detailed_reply(STATUS_SPAWN_REFUSED, 0, 0, error as u64),
            None,
        ),
    }
}

/// Which of this instance's slots the generation binds for `command`.
///
/// `suffix` selects the pair member: empty for the command's executable,
/// `-context` for the endpoint its launch context travels on. The name is
/// composed into a fixed buffer bounded by the protocol's own command length,
/// so an over-long or non-UTF-8 command is refused by the root's own name
/// admissibility rather than truncated into a different name.
///
/// The root answers from this instance's *own* bindings, so the reply is the
/// authority the generation granted this service and nothing else: a command
/// naming another component's grant resolves nothing.
fn command_binding(command: &[u8], suffix: &[u8]) -> Option<u32> {
    let mut name = [0u8; MAX_COMMAND_BINDING];
    let prefix = COMMAND_BINDING_PREFIX.len();
    let body = prefix.checked_add(command.len())?;
    let end = body.checked_add(suffix.len())?;
    let frame = name.get_mut(..end)?;
    frame
        .get_mut(..prefix)?
        .copy_from_slice(COMMAND_BINDING_PREFIX);
    frame.get_mut(prefix..body)?.copy_from_slice(command);
    frame.get_mut(body..)?.copy_from_slice(suffix);
    slime_rt::resolve_binding(frame).ok()
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
///
/// `budget` is this instance's authenticated declared budget, not a compiled
/// constant: a client that states a different one is talking to a service it
/// was not declared against, so the request is refused rather than served
/// against a bound the two do not share (B70).
fn valid_request(request: &WireSpawnRequest, claimed: Option<u32>, budget: usize) -> bool {
    const SUPPORTED_ROLES: u8 = CAPABILITY_ROLE_WORKING_DIRECTORY | CAPABILITY_ROLE_STDIN;
    let wants_directory = request.capability_roles & CAPABILITY_ROLE_WORKING_DIRECTORY != 0;
    valid_spawn_request(request)
        && usize::from(request.client_budget) == budget
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

fn wait_reply(handle: u32, live: &mut [Option<LiveChild>; MAX_LIVE_CHILDREN]) -> WireSpawnReply {
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

fn reap(live: &mut [Option<LiveChild>; MAX_LIVE_CHILDREN]) {
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
// The wire field is one byte, so a published ceiling above 255 would make a
// declared budget unstateable in a request and the equality check unreachable.
const _: () = assert!(MAX_LIVE_CHILDREN >= 1 && MAX_LIVE_CHILDREN <= u8::MAX as usize);
