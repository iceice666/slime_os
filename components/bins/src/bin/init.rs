#![no_std]
#![no_main]

slime_rt::entry!(main);

use slime_rt::{Rights, SpawnGrant};

const RIGHT_SEND: Rights = 1;
const RIGHT_RECV: Rights = 2;
const RIGHT_TRANSFER: Rights = 4;
const RIGHT_BLOCK_READ: Rights = 1 << 10;
const RIGHT_BLOCK_WRITE: Rights = 1 << 11;
const RIGHT_STORE_READ: Rights = 1 << 12;
const RIGHT_STORE_WRITE: Rights = 1 << 13;
const RIGHT_HEALTH_CONFIRM: Rights = 1 << 14;
const RIGHT_BOOT_UPDATE: Rights = 1 << 15;
const RIGHT_EXEC: Rights = 1 << 3;
const RIGHT_SPAWN: Rights = 1 << 16;
const RIGHT_ENDPOINT_CREATE: Rights = 1 << 17;
const RIGHT_DIRECTORY_READ: Rights = 1 << 19;
const RIGHT_DIRECTORY_WRITE: Rights = 1 << 20;
const RIGHT_DIRECTORY_LIST: Rights = 1 << 21;
const RIGHT_DIRECTORY_DERIVE: Rights = 1 << 22;
const RIGHT_INPUT_READ: Rights = 1 << 23;
const RIGHT_BUFFER_CREATE: Rights = 1 << 24;
const RIGHT_SUPERVISE: Rights = 1 << 18;

// Manifest-derived bootstrap slot order is emitted by the host builder.
const CONSOLE_CAPS: [SpawnGrant; 1] = [grant(2, RIGHT_RECV)];
const STORAGE_PROBE_READ_CAPS: [SpawnGrant; 1] = [grant(9, RIGHT_BLOCK_READ)];
const STORAGE_PROBE_WRITE_CAPS: [SpawnGrant; 1] = [grant(9, RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE)];
const STORAGE_PROBE_STORE_CAPS: [SpawnGrant; 1] = [grant(9, RIGHT_STORE_READ | RIGHT_STORE_WRITE)];
const GENERATION_MANAGER_CAPS: [SpawnGrant; 6] = [
    grant(31, RIGHT_SEND | RIGHT_RECV),
    grant(11, RIGHT_HEALTH_CONFIRM | RIGHT_BOOT_UPDATE),
    grant(32, RIGHT_SEND | RIGHT_RECV),
    grant(33, RIGHT_SEND | RIGHT_RECV),
    grant(34, RIGHT_SEND | RIGHT_RECV),
    grant(35, RIGHT_SEND | RIGHT_RECV),
];
const RECOVERY_CAPS: [SpawnGrant; 2] = [
    grant(2, RIGHT_BOOT_UPDATE),
    grant(3, RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE),
];

fn dango_caps() -> [SpawnGrant; 6] {
    [
        grant(12, RIGHT_SEND | RIGHT_RECV),
        grant(4, RIGHT_SEND | RIGHT_TRANSFER),
        grant(20, RIGHT_INPUT_READ),
        grant(
            19,
            RIGHT_DIRECTORY_READ | RIGHT_DIRECTORY_DERIVE | RIGHT_TRANSFER,
        ),
        grant(0, RIGHT_ENDPOINT_CREATE),
        grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
    ]
}

fn spawn_service_caps() -> [SpawnGrant; 5] {
    [
        grant(13, RIGHT_SEND | RIGHT_RECV),
        grant(6, RIGHT_EXEC | RIGHT_SPAWN),
        grant(7, RIGHT_EXEC | RIGHT_SPAWN),
        grant(0, RIGHT_ENDPOINT_CREATE),
        grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
    ]
}

fn filesystem_caps() -> [SpawnGrant; 2] {
    [
        grant(17, RIGHT_SEND | RIGHT_RECV),
        grant(18, RIGHT_STORE_READ | RIGHT_STORE_WRITE),
    ]
}

const DIRECTORY_PROBE_CAPS: [SpawnGrant; 2] = [
    grant(16, RIGHT_SEND | RIGHT_RECV),
    grant(
        19,
        RIGHT_TRANSFER
            | RIGHT_DIRECTORY_READ
            | RIGHT_DIRECTORY_WRITE
            | RIGHT_DIRECTORY_LIST
            | RIGHT_DIRECTORY_DERIVE,
    ),
];

const GENERATION_LIST_CAPS: [SpawnGrant; 1] = [grant(26, RIGHT_SEND | RIGHT_RECV)];
const GENERATION_INSPECT_CAPS: [SpawnGrant; 1] = [grant(27, RIGHT_SEND | RIGHT_RECV)];
const GENERATION_STAGE_CAPS: [SpawnGrant; 1] = [grant(28, RIGHT_SEND | RIGHT_RECV)];
const GENERATION_SELECT_CAPS: [SpawnGrant; 1] = [grant(29, RIGHT_SEND | RIGHT_RECV)];
const GENERATION_ROLLBACK_CAPS: [SpawnGrant; 1] = [grant(30, RIGHT_SEND | RIGHT_RECV)];
const POWERBOX_CHOOSER_CAPS: [SpawnGrant; 3] = [
    grant(39, RIGHT_SEND | RIGHT_RECV),
    grant(
        19,
        RIGHT_DIRECTORY_READ | RIGHT_DIRECTORY_DERIVE | RIGHT_TRANSFER,
    ),
    grant(20, RIGHT_INPUT_READ),
];
// C7.2 shared-buffer factory, minted by bootstrap at a fixed slot ahead of the
// optional transfer block so every index below is stable on any boot.
const SHARED_BUFFER_FACTORY_SLOT: u32 = 40;
// C7.7 sample plane: two components and the channel joining them.
const SAMPLE_LENDER_SLOT: u32 = 41;
const SAMPLE_RECEIVER_SLOT: u32 = 42;
const SAMPLE_LENDER_ENDPOINT_SLOT: u32 = 43;
const SAMPLE_RECEIVER_ENDPOINT_SLOT: u32 = 44;
// C8.3/C8.4 fabric plane: the service, its five clients, and the two halves of
// each control channel. Init holds no route capability at all — the fabric
// mints those and moves each participant a narrowed role.
const FABRIC_SERVICE_SLOT: u32 = 45;
const FABRIC_PUBLISHER_SLOT: u32 = 46;
const FABRIC_SUBSCRIBER_SLOT: u32 = 47;
const FABRIC_INTRUDER_SLOT: u32 = 48;
const FABRIC_PUBLISHER_B_SLOT: u32 = 49;
const FABRIC_SUBSCRIBER_B_SLOT: u32 = 50;
const FABRIC_PUBLISHER_CONTROL_SLOT: u32 = 51;
const FABRIC_SUBSCRIBER_CONTROL_SLOT: u32 = 52;
const FABRIC_INTRUDER_CONTROL_SLOT: u32 = 53;
const FABRIC_PUBLISHER_B_CONTROL_SLOT: u32 = 54;
const FABRIC_SUBSCRIBER_B_CONTROL_SLOT: u32 = 55;
const FABRIC_PUBLISHER_SERVICE_SLOT: u32 = 56;
const FABRIC_SUBSCRIBER_SERVICE_SLOT: u32 = 57;
const FABRIC_INTRUDER_SERVICE_SLOT: u32 = 58;
const FABRIC_PUBLISHER_B_SERVICE_SLOT: u32 = 59;
const FABRIC_SUBSCRIBER_B_SERVICE_SLOT: u32 = 60;
const FABRIC_TIME_CLIENT_SLOT: u32 = 61;
const FABRIC_TIME_SERVICE_SLOT: u32 = 62;
const TRANSFER_RECEIVER_SLOT: u32 = 61;
const TRANSFER_SOURCE_SLOT: u32 = 62;
// C8.6 reuses three existing fabric executable/control pairs. The call gate is
// a mutually exclusive generation profile, so no capability table grows.
const FABRIC_CALL_CLIENT_SLOT: u32 = FABRIC_PUBLISHER_SLOT;
const FABRIC_CALL_CLIENT_B_SLOT: u32 = FABRIC_SUBSCRIBER_SLOT;
const FABRIC_CALL_SERVER_SLOT: u32 = FABRIC_PUBLISHER_B_SLOT;
const FABRIC_CALL_CLIENT_CONTROL_SLOT: u32 = FABRIC_PUBLISHER_CONTROL_SLOT;
const FABRIC_CALL_CLIENT_B_CONTROL_SLOT: u32 = FABRIC_SUBSCRIBER_CONTROL_SLOT;
const FABRIC_CALL_SERVER_CONTROL_SLOT: u32 = FABRIC_PUBLISHER_B_CONTROL_SLOT;
const FABRIC_CALL_CLIENT_SERVICE_SLOT: u32 = FABRIC_PUBLISHER_SERVICE_SLOT;
const FABRIC_CALL_CLIENT_B_SERVICE_SLOT: u32 = FABRIC_SUBSCRIBER_SERVICE_SLOT;
const FABRIC_CALL_SERVER_SERVICE_SLOT: u32 = FABRIC_PUBLISHER_B_SERVICE_SLOT;
const FABRIC_CALL_TIME_SLOT: u32 = FABRIC_INTRUDER_SLOT;
const FABRIC_CALL_TIME_CONTROL_SLOT: u32 = FABRIC_INTRUDER_CONTROL_SLOT;
const FABRIC_CALL_TIME_SERVICE_SLOT: u32 = FABRIC_INTRUDER_SERVICE_SLOT;
const FABRIC_CALL_PHASE_TIME_SLOT: u32 = 61;
const FABRIC_CALL_PHASE_CLIENT_SLOT: u32 = 62;
// C8.7 reuses the same fabric executable/control pairs as C8.6: the operation
// gate is a mutually exclusive generation profile, so no capability table grows.
// The two phase channels are minted at runtime rather than granted.
const FABRIC_OP_CLIENT_SLOT: u32 = FABRIC_PUBLISHER_SLOT;
const FABRIC_OP_CLIENT_B_SLOT: u32 = FABRIC_SUBSCRIBER_SLOT;
const FABRIC_OP_SERVER_SLOT: u32 = FABRIC_INTRUDER_SLOT;
const FABRIC_OP_TIME_SLOT: u32 = FABRIC_PUBLISHER_B_SLOT;
const FABRIC_OP_CLIENT_CONTROL_SLOT: u32 = FABRIC_PUBLISHER_CONTROL_SLOT;
const FABRIC_OP_CLIENT_B_CONTROL_SLOT: u32 = FABRIC_SUBSCRIBER_CONTROL_SLOT;
const FABRIC_OP_SERVER_CONTROL_SLOT: u32 = FABRIC_PUBLISHER_B_CONTROL_SLOT;
const FABRIC_OP_TIME_CONTROL_SLOT: u32 = FABRIC_INTRUDER_CONTROL_SLOT;
const FABRIC_OP_CLIENT_SERVICE_SLOT: u32 = FABRIC_PUBLISHER_SERVICE_SLOT;
const FABRIC_OP_CLIENT_B_SERVICE_SLOT: u32 = FABRIC_SUBSCRIBER_SERVICE_SLOT;
const FABRIC_OP_SERVER_SERVICE_SLOT: u32 = FABRIC_PUBLISHER_B_SERVICE_SLOT;
const FABRIC_OP_TIME_SERVICE_SLOT: u32 = FABRIC_INTRUDER_SERVICE_SLOT;
const FABRIC_OP_CLIENT_B_RESTART_SLOT: u32 = FABRIC_SUBSCRIBER_B_SLOT;

const POWERBOX_PROBE_CAPS: [SpawnGrant; 1] = [grant(38, RIGHT_SEND | RIGHT_RECV)];

const fn grant(slot: u32, rights: Rights) -> SpawnGrant {
    SpawnGrant { slot, rights }
}

fn storage_caps() -> &'static [SpawnGrant] {
    match option_env!("SLIME_GENERATION_NUMBER") {
        Some("2") | Some("3") => &STORAGE_PROBE_WRITE_CAPS,
        Some("4") => &STORAGE_PROBE_STORE_CAPS,
        _ => &STORAGE_PROBE_READ_CAPS,
    }
}

fn main() {
    if option_env!("SLIME_RECOVERY_IMAGE") == Some("1") {
        slime_rt::debug_write(b"[init] launching recovery graph\n");
        spawn_or_fail(1, &RECOVERY_CAPS);
        return;
    }
    if option_env!("SLIME_FABRIC_QOS_CHECK") == Some("1") {
        for slot in 1..FABRIC_SERVICE_SLOT {
            if slot != SHARED_BUFFER_FACTORY_SLOT {
                let _ = slime_rt::cap_drop(slot);
            }
        }
        launch_fabric_graph();
        slime_rt::debug_write(b"[init] fabric QoS complete\n");
        slime_rt::exit(0);
    }
    if option_env!("SLIME_FABRIC_CALL_CHECK") == Some("1") {
        launch_fabric_calls();
        slime_rt::debug_write(b"[init] fabric call complete\n");
        slime_rt::exit(0);
    }
    if option_env!("SLIME_FABRIC_OPERATION_CHECK") == Some("1") {
        launch_fabric_operations();
        slime_rt::debug_write(b"[init] fabric operation complete\n");
        slime_rt::exit(0);
    }
    if option_env!("SLIME_FABRIC_VISIBILITY_CHECK") == Some("1") {
        launch_fabric_graph();
        slime_rt::debug_write(b"[init] fabric visibility complete\n");
        slime_rt::exit(0);
    }
    // Both halves of the guard, matching `launch_fabric_boot_init`. The kernel
    // hands init a different capability layout for this generation, so keying on
    // the env alone would make generation 1 — built with the same env by
    // `build-generation.py` — walk a layout it was not given.
    if option_env!("SLIME_FABRIC_BOOT_CHECK") == Some("1")
        && option_env!("SLIME_GENERATION_NUMBER") == Some("17")
    {
        launch_fabric_boot();
    }
    slime_rt::debug_write(b"[init] launching component graph\n");
    if option_env!("SLIME_TRANSFER_RECEIVER") == Some("1") {
        if slime_rt::generation_receive(TRANSFER_RECEIVER_SLOT, TRANSFER_SOURCE_SLOT) == 0 {
            slime_rt::debug_write(b"[init] generation transfer installed\n");
            slime_rt::exit(0);
        }
        slime_rt::exit(1);
    }
    if option_env!("SLIME_TRANSFER_ACTIVATE") == Some("1") {
        slime_rt::debug_write(b"[init] transferred generation healthy\n");
        slime_rt::exit(0);
    }

    if matches!(option_env!("SLIME_GENERATION_NUMBER"), Some("6" | "7")) {
        spawn_or_fail(14, &filesystem_caps());
        spawn_or_fail(15, &DIRECTORY_PROBE_CAPS);
    }
    if option_env!("SLIME_POWERBOX_CHECK") == Some("1") {
        spawn_or_fail(1, &CONSOLE_CAPS);
        spawn_or_fail(36, &POWERBOX_CHOOSER_CAPS);
        spawn_and_wait(37, &POWERBOX_PROBE_CAPS);
        slime_rt::debug_write(b"[init] powerbox scenario complete\n");
        slime_rt::exit(0);
    }
    if option_env!("SLIME_GENERATION_CMD_CHECK") != Some("1")
        && option_env!("SLIME_POWERBOX_CHECK") != Some("1")
    {
        spawn_or_fail(1, &CONSOLE_CAPS);
        spawn_or_fail(3, &dango_caps());
        spawn_or_fail(5, &spawn_service_caps());
        if option_env!("SLIME_DANGO_CHECK") != Some("1")
            && option_env!("SLIME_GENERATION_NUMBER") != Some("9")
        {
            // With no block device attached, bootstrap hands init an ObjectStore
            // fallback in the storage slot instead of a block capability, so the
            // storage-probe's BLOCK_READ derive is rejected. That is the expected
            // no-disk case (the kernel's `on_idle` already tolerates an absent
            // storage-probe), so skip it rather than aborting the whole graph.
            spawn_optional_storage(8, storage_caps());
        }
    }
    if option_env!("SLIME_GENERATION_CMD_CHECK") != Some("1") {
        spawn_or_fail(10, &GENERATION_MANAGER_CAPS);
    }
    if option_env!("SLIME_GENERATION_CMD_CHECK") == Some("1") {
        let negative_scenario = matches!(
            option_env!("SLIME_GENERATION_CMD_SCENARIO"),
            Some("bad-closure" | "bad-release")
        );
        spawn_or_fail(10, &GENERATION_MANAGER_CAPS);
        spawn_and_wait(21, &GENERATION_LIST_CAPS);
        if !negative_scenario {
            spawn_and_wait(22, &GENERATION_INSPECT_CAPS);
        }
        spawn_and_wait(23, &GENERATION_STAGE_CAPS);
        if negative_scenario {
            slime_rt::debug_write(b"[init] negative generation scenario complete\n");
            slime_rt::exit(0);
        }
        spawn_and_wait(24, &GENERATION_SELECT_CAPS);
        spawn_and_wait(25, &GENERATION_ROLLBACK_CAPS);
    }
    if option_env!("SLIME_SAMPLE_PLANE_CHECK") == Some("1") {
        launch_sample_plane();
        slime_rt::debug_write(b"[init] sample plane complete\n");
        slime_rt::exit(0);
    }
    // C8.3 and C8.4 launch the same graph: the fabric provisions every declared
    // edge, then brokers the samples those edges carry. The two gates differ
    // only in what they assert about one boot, so one launch serves both rather
    // than two scenarios drifting apart.
    if option_env!("SLIME_FABRIC_AUTHORITY_CHECK") == Some("1") {
        launch_fabric_graph();
        slime_rt::debug_write(b"[init] fabric authority complete\n");
        slime_rt::exit(0);
    }
    if option_env!("SLIME_FABRIC_STREAM_CHECK") == Some("1") {
        launch_fabric_graph();
        slime_rt::debug_write(b"[init] fabric stream complete\n");
        slime_rt::exit(0);
    }
    slime_rt::debug_write(b"[init] spawn graph launched\n");
    slime_rt::exit(0);
}

// C8.10 full-graph boot layout, matching `launch_fabric_boot_init`'s vector.
// Init's own factories first, then the three executables the fabric needs, then
// one executable per participant, then both halves of each control channel.
const BOOT_ENDPOINT_FACTORY_SLOT: u32 = 0;
const BOOT_BUFFER_FACTORY_SLOT: u32 = 1;
const BOOT_FABRIC_SERVICE_SLOT: u32 = 2;
const BOOT_CALL_WORKER_SLOT: u32 = 3;
const BOOT_OP_WORKER_SLOT: u32 = 4;
/// Participants in the exact order the kernel laid them out. Index into this
/// table drives every slot below, so a participant added or removed moves one
/// entry rather than a scattering of constants.
const BOOT_PARTICIPANTS: usize = 16;
/// The first [`BOOT_STREAM_PARTICIPANTS`] entries are the stream plane; the rest
/// are the call and operation planes and the operation replacement channel.
const BOOT_STREAM_PARTICIPANTS: usize = 7;
const BOOT_FIRST_EXECUTABLE_SLOT: u32 = 5;
const BOOT_FIRST_CONTROL_SLOT: u32 = BOOT_FIRST_EXECUTABLE_SLOT + BOOT_PARTICIPANTS as u32;
/// Subscribers, by participant index. Their supervision handles must exist
/// before the fabric starts: a downstream loan names its receiver through a
/// `RIGHT_SUPERVISE` capability rather than an ambient task id.
const BOOT_SUBSCRIBERS: [usize; 3] = [1, 3, 4];

const fn boot_executable_slot(participant: usize) -> u32 {
    BOOT_FIRST_EXECUTABLE_SLOT + participant as u32
}

/// Participant's own half of its control channel.
const fn boot_client_slot(participant: usize) -> u32 {
    BOOT_FIRST_CONTROL_SLOT + (participant as u32) * 2
}

/// Fabric's half of the same channel.
const fn boot_service_slot(participant: usize) -> u32 {
    boot_client_slot(participant) + 1
}

/// Launch the C8.10 full graph: every declared C8 role in one generation.
///
/// **What makes this the milestone rather than a bigger scenario.** Before this,
/// the stream, call, and operation planes were mutually exclusive generation
/// profiles that physically aliased one range of init's capability slots — only
/// one could exist per boot, and "which plane" was chosen by rewriting slots at
/// bootstrap. Here all three coexist in disjoint slots, so no profile-dependent
/// rewrite happens at all, and the roles that were one binary switching on an
/// env flag (`fabric-intruder`) are three separate component identities.
///
/// **Spawn order is load-bearing, in two directions that must both hold.**
/// Subscribers first, because the fabric needs their supervision handles to
/// exist before it starts. The fabric next, so no participant can send a role
/// request before there is a worker to answer it. Everyone else after.
///
/// **Init keeps no route authority.** It mints the control channels and hands
/// out both halves, and that spawn-time binding is the whole basis on which a
/// worker later decides "which component is asking". The two route workers are
/// spawned by the fabric rather than here, so the component that binds their
/// control endpoints is the component that created them.
///
/// Init does not exit. The gate's exit condition is a fully provisioned graph at
/// healthy blocked idle, so init parks on a supervision handle it never expects
/// to fire — a component terminating is a failure the kernel reports, not
/// something init waits for.
fn launch_fabric_boot() -> ! {
    let mut supervision = [0u32; BOOT_PARTICIPANTS];
    for participant in BOOT_SUBSCRIBERS {
        supervision[participant] = spawn_boot_participant(participant);
    }
    slime_rt::debug_write(b"[init] fabric boot subscribers spawned\n");

    // The fabric's authority: its two factories, the two worker executables it
    // spawns, and one service-side control endpoint per participant. Nothing
    // here is a route capability — it mints those itself.
    // Grant order *is* the fabric's slot layout, and the fabric derives its
    // numbering from the resolved profile rather than from constants of its own:
    // its two factories, one control endpoint per stream participant, the
    // subscriber supervision handles, then the call and operation planes'
    // controls, and last the two worker executables. A grant is a
    // non-consuming derive-copy into a fresh table, so these land at 0, 1, 2...
    // in the fabric regardless of which slots they occupy here.
    const BOOT_FABRIC_GRANTS: usize = 2 + BOOT_PARTICIPANTS + BOOT_SUBSCRIBERS.len() + 2;
    let mut grants = [SpawnGrant { slot: 0, rights: 0 }; BOOT_FABRIC_GRANTS];
    let mut count = 0;
    let mut push = |slot: u32, rights: Rights| {
        grants[count] = grant(slot, rights);
        count += 1;
    };
    push(BOOT_ENDPOINT_FACTORY_SLOT, RIGHT_ENDPOINT_CREATE);
    push(BOOT_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE);
    for participant in 0..BOOT_STREAM_PARTICIPANTS {
        push(boot_service_slot(participant), RIGHT_SEND | RIGHT_RECV);
    }
    // Subscriber supervision handles, directly after the stream controls: a
    // downstream loan names its receiver through one of these, and the resolved
    // profile numbers them at exactly this offset.
    for participant in BOOT_SUBSCRIBERS {
        push(supervision[participant], RIGHT_SUPERVISE);
    }
    for participant in BOOT_STREAM_PARTICIPANTS..BOOT_PARTICIPANTS {
        push(boot_service_slot(participant), RIGHT_SEND | RIGHT_RECV);
    }
    push(BOOT_CALL_WORKER_SLOT, RIGHT_EXEC | RIGHT_SPAWN);
    push(BOOT_OP_WORKER_SLOT, RIGHT_EXEC | RIGHT_SPAWN);
    let fabric =
        slime_rt::spawn(BOOT_FABRIC_SERVICE_SLOT, &grants[..count]).unwrap_or_else(|error| {
            slime_rt::debug_write(b"[init] fabric boot service spawn failed error=");
            write_i64(error);
            slime_rt::debug_write(b"\n");
            slime_rt::exit(1)
        });
    slime_rt::debug_write(b"[init] fabric boot service spawned\n");

    // The fabric holds its own derived copies now, so init releases the
    // service-side halves, the worker executables, and the buffer factory it
    // only held to hand on. Released here rather than at the end because the
    // remaining participants each add a supervision handle: init launches with
    // 53 of 64 slots occupied, and holding all sixteen handles on top of every
    // control half would exhaust the table partway through the graph — a
    // failure that looks like a spawn error rather than a leak.
    for participant in 0..BOOT_PARTICIPANTS {
        if slime_rt::cap_drop(boot_service_slot(participant)) < 0 {
            slime_rt::exit(1);
        }
    }
    for slot in [
        BOOT_FABRIC_SERVICE_SLOT,
        BOOT_CALL_WORKER_SLOT,
        BOOT_OP_WORKER_SLOT,
        BOOT_BUFFER_FACTORY_SLOT,
    ] {
        if slime_rt::cap_drop(slot) < 0 {
            slime_rt::exit(1);
        }
    }
    // The subscribers' supervision handles are the fabric's now; init keeps no
    // claim on components it has finished launching.
    for participant in BOOT_SUBSCRIBERS {
        if slime_rt::cap_drop(supervision[participant]) < 0 {
            slime_rt::exit(1);
        }
        supervision[participant] = 0;
    }

    // Everyone else.
    for (participant, handle) in supervision.iter_mut().enumerate() {
        if !BOOT_SUBSCRIBERS.contains(&participant) {
            *handle = spawn_boot_participant(participant);
        }
    }
    slime_rt::debug_write(b"[init] fabric boot participants spawned\n");

    // Let every participant run far enough to send its role request before any
    // supervision descriptor follows it.
    //
    // **This ordering is load-bearing.** The request/response brokers read one
    // role request and then one supervision descriptor per client, on the same
    // control channel, and a channel is a queue. `consume_request` accepts
    // whatever arrives first — so a supervision descriptor sent ahead of the
    // request is silently consumed *as* the request, its capability released,
    // and the request behind it then fails the supervision shape check. Both
    // messages are well-formed; only their order is wrong.
    //
    // One yield is sufficient, and deterministically so rather than by luck.
    // Scheduling is cooperative — the APIC timer handler advances ticks and
    // never preempts — and `SYS_YIELD` pushes the caller onto the back of a FIFO
    // ready queue. So every participant spawned above runs before init resumes,
    // and each runs until it blocks. Their first act is a send on a control
    // channel whose queue is empty, and a send blocks only on a full queue, so
    // each request is enqueued before its sender parks in `recv`.
    slime_rt::yield_now();

    // The request/response brokers name a peer by capability rather than by an
    // identity claimed in a message, so each call and operation participant's
    // supervision handle is moved to its worker over that participant's own
    // authenticated control channel. Stream participants need none: the stream
    // broker binds a subscriber through the handle init already granted it.
    for (participant, handle) in supervision.iter_mut().enumerate() {
        if let Some((direction, route)) = boot_supervision_edge(participant) {
            transfer_supervision(boot_client_slot(participant), *handle, direction, route);
            // `cap_transfer` *moves* the capability, so the slot is empty now.
            // Marking it here is what keeps the release loop below from trying
            // to drop a handle that no longer exists.
            *handle = 0;
        }
    }
    slime_rt::debug_write(b"[init] fabric boot supervision transferred\n");

    // Release everything init only held to hand on: each participant's
    // executable, its control half, and its supervision handle. The kernel's own
    // health sweep reports a component that dies, so a handle retained here would
    // buy nothing and cost a slot.
    for (participant, handle) in supervision.iter().enumerate() {
        for slot in [
            boot_executable_slot(participant),
            boot_client_slot(participant),
        ] {
            if slime_rt::cap_drop(slot) < 0 {
                slime_rt::exit(1);
            }
        }
        // Zero means init no longer holds that handle: a subscriber's was
        // released with the fabric's grants above, and a call or operation
        // participant's was consumed by the move to its worker.
        if *handle != 0 && slime_rt::cap_drop(*handle) < 0 {
            slime_rt::exit(1);
        }
    }
    slime_rt::debug_write(b"[init] fabric boot graph launched\n");

    // Park on the fabric's supervision handle — the one capability init keeps.
    // Init must not exit: the gate's exit condition is the whole graph at
    // healthy blocked idle, and a terminated init would make this a finished
    // generation rather than an idle one. Waiting on the fabric rather than
    // spinning also means a fabric that dies wakes init instead of going
    // unnoticed.
    loop {
        slime_rt::wait(&[slime_rt::WaitSource::Supervision(fabric.supervision_slot)]);
    }
}

/// The (direction, route) a participant's supervision handle belongs to, or
/// `None` when no worker will consume one.
///
/// The descriptor names the exact edge the handle is for, because that is what
/// the broker verifies before accepting it: a handle minted for one route may
/// not be replayed as authority on another. Indices follow the boot layout — the
/// stream plane (0–6), then the call plane, then the operation plane.
///
/// Three kinds of participant get `None`, each for its own reason:
///
/// - a **stream** participant, because the stream broker binds a subscriber
///   through the handle init already granted the fabric at spawn;
/// - a **clock** (`fabric-call-time`, `fabric-op-time`), because a broker parks
///   on its time endpoint but never provisions a role for it;
/// - the **operation replacement** (`fabric-op-client-b-restart`), because
///   `operation_broker::pump_replacement` reads that channel only while client
///   B's slot is vacant, and in this boot client B never leaves. Sending it a
///   descriptor would move a capability out of init to a reader that cannot run,
///   orphaning it — a leak the gate cannot see, since nothing fails.
fn boot_supervision_edge(participant: usize) -> Option<(u32, [u8; 32])> {
    use boot_contracts::fabric_graph::{DIRECTION_CLIENT, DIRECTION_SERVER};
    match participant {
        7 | 8 => Some((DIRECTION_CLIENT, call_route_identity())),
        9 => Some((DIRECTION_SERVER, call_route_identity())),
        11 | 12 => Some((DIRECTION_CLIENT, operation_route_identity())),
        13 => Some((DIRECTION_SERVER, operation_route_identity())),
        _ => None,
    }
}

/// Spawn one full-graph participant with its own control endpoint and nothing
/// else, returning the supervision handle init keeps.
fn spawn_boot_participant(participant: usize) -> u32 {
    let spawned = slime_rt::spawn(
        boot_executable_slot(participant),
        &[grant(
            boot_client_slot(participant),
            RIGHT_SEND | RIGHT_RECV,
        )],
    )
    .unwrap_or_else(|error| {
        slime_rt::debug_write(b"[init] fabric boot spawn failed participant=");
        write_u32(participant as u32);
        slime_rt::debug_write(b" error=");
        write_i64(error);
        slime_rt::debug_write(b"\n");
        slime_rt::exit(1)
    });
    spawned.supervision_slot
}

/// Launch the C8.7 operation plane: one fabric brokering two clients and one
/// server on the declared `navigation` operation route, plus the capability-routed
/// clock that makes expiry and timeout deterministic.
///
/// Init holds no route capability. It mints the control channels, hands the
/// fabric one service side per participant, and hands each participant only its
/// own client side — so "which component is asking" is a capability fact
/// established here at spawn, not a claim in a message. That binding is exactly
/// what makes the milestone's authority denials hold: client B cannot observe,
/// retrieve, or cancel client A's operation even knowing its identity, because
/// its requests arrive on a different endpoint.
///
/// **Spawn order is load-bearing.** The fabric starts before any participant so
/// no goal can arrive before there is a broker to correlate it, and the clock
/// starts last so no time advance precedes the operations it must expire.
fn launch_fabric_operations() {
    for slot in 1..FABRIC_SERVICE_SLOT {
        let keep = slot == SHARED_BUFFER_FACTORY_SLOT || matches!(slot, 46..=60);
        if !keep && slime_rt::cap_drop(slot) < 0 {
            slime_rt::exit(1);
        }
    }
    // Slot 50 carries the replacement client-B executable in generation 15;
    // keep it until the first participant exits and init performs the restart.
    let (phase_client, phase_client_b) =
        slime_rt::endpoint_create(0).unwrap_or_else(|_| slime_rt::exit(1));
    let (phase_time_client, phase_time_service) =
        slime_rt::endpoint_create(0).unwrap_or_else(|_| slime_rt::exit(1));
    let (replacement_control, replacement_service) =
        slime_rt::endpoint_create(0).unwrap_or_else(|_| slime_rt::exit(1));
    let (restart_start_send, restart_start_recv) =
        slime_rt::endpoint_create(0).unwrap_or_else(|_| slime_rt::exit(1));
    let _service = spawn_fabric_client(
        FABRIC_SERVICE_SLOT,
        &[
            grant(0, RIGHT_ENDPOINT_CREATE),
            grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
            grant(FABRIC_OP_CLIENT_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
            grant(FABRIC_OP_CLIENT_B_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
            grant(FABRIC_OP_SERVER_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
            grant(FABRIC_OP_TIME_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
            grant(replacement_service, RIGHT_SEND | RIGHT_RECV),
        ],
        &[FABRIC_SERVICE_SLOT],
    );
    let client = spawn_fabric_client(
        FABRIC_OP_CLIENT_SLOT,
        &[
            grant(FABRIC_OP_CLIENT_CONTROL_SLOT, RIGHT_SEND | RIGHT_RECV),
            grant(phase_client, RIGHT_SEND | RIGHT_RECV),
            grant(phase_time_client, RIGHT_SEND),
        ],
        &[FABRIC_OP_CLIENT_SLOT],
    );
    let client_b = spawn_fabric_client(
        FABRIC_OP_CLIENT_B_SLOT,
        &[
            grant(FABRIC_OP_CLIENT_B_CONTROL_SLOT, RIGHT_SEND | RIGHT_RECV),
            grant(phase_client_b, RIGHT_SEND | RIGHT_RECV),
        ],
        &[FABRIC_OP_CLIENT_B_SLOT],
    );
    let server = spawn_fabric_client(
        FABRIC_OP_SERVER_SLOT,
        &[grant(
            FABRIC_OP_SERVER_CONTROL_SLOT,
            RIGHT_SEND | RIGHT_RECV,
        )],
        &[FABRIC_OP_SERVER_SLOT],
    );
    let _time = spawn_fabric_client(
        FABRIC_OP_TIME_SLOT,
        &[
            grant(FABRIC_OP_TIME_CONTROL_SLOT, RIGHT_SEND | RIGHT_RECV),
            grant(phase_time_service, RIGHT_RECV),
        ],
        &[FABRIC_OP_TIME_SLOT],
    );

    slime_rt::yield_now();
    for (control, supervision, direction) in [
        (
            FABRIC_OP_CLIENT_CONTROL_SLOT,
            client.supervision_slot,
            boot_contracts::fabric_graph::DIRECTION_CLIENT,
        ),
        (
            FABRIC_OP_CLIENT_B_CONTROL_SLOT,
            client_b.supervision_slot,
            boot_contracts::fabric_graph::DIRECTION_CLIENT,
        ),
        (
            FABRIC_OP_SERVER_CONTROL_SLOT,
            server.supervision_slot,
            boot_contracts::fabric_graph::DIRECTION_SERVER,
        ),
    ] {
        transfer_supervision(control, supervision, direction, operation_route_identity());
        if control != FABRIC_OP_CLIENT_B_CONTROL_SLOT && slime_rt::cap_drop(control) < 0 {
            slime_rt::exit(1);
        }
    }
    // The broker owns client B's original supervision handle. Spawn a
    // replacement immediately on a distinct authenticated control channel;
    // the replacement's own role request queues behind this supervision
    // descriptor, while its scenario blocks until the original client signals
    // that the retained result is ready.
    let client_b_restart = slime_rt::spawn(
        FABRIC_OP_CLIENT_B_RESTART_SLOT,
        &[
            grant(replacement_control, RIGHT_SEND | RIGHT_RECV),
            grant(phase_client_b, RIGHT_SEND | RIGHT_RECV),
            grant(restart_start_recv, RIGHT_RECV),
        ],
    )
    .unwrap_or_else(|error| {
        slime_rt::debug_write(b"[init] operation restart spawn failed error=");
        write_i64(error);
        slime_rt::debug_write(b"\n");
        slime_rt::exit(1)
    });
    slime_rt::debug_write(b"[init] operation replacement spawned\n");
    transfer_supervision(
        replacement_control,
        client_b_restart.supervision_slot,
        boot_contracts::fabric_graph::DIRECTION_CLIENT,
        operation_route_identity(),
    );
    slime_rt::debug_write(b"[init] operation replacement supervision transferred\n");
    if slime_rt::cap_drop(FABRIC_OP_CLIENT_B_CONTROL_SLOT) < 0
        || slime_rt::cap_drop(replacement_control) < 0
    {
        slime_rt::exit(1);
    }

    slime_rt::debug_write(b"[init] operation replacement controls dropped\n");
    loop {
        match slime_rt::send(restart_start_send, &[1], &[]) {
            slime_rt::ERR_SUCCESS => break,
            slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
            _ => slime_rt::exit(1),
        }
    }
    for slot in [
        FABRIC_OP_CLIENT_SERVICE_SLOT,
        FABRIC_OP_SERVER_SERVICE_SLOT,
        FABRIC_OP_TIME_SERVICE_SLOT,
        phase_time_client,
        phase_time_service,
        replacement_service,
        restart_start_send,
        restart_start_recv,
    ] {
        if slime_rt::cap_drop(slot) < 0 {
            slime_rt::exit(1);
        }
    }
    slime_rt::yield_now();
}
fn launch_fabric_calls() {
    // The broker starts first so its supervision handle can name the receiver
    // of every participant's upstream shared payload. Participant supervision
    // handles then travel over their authenticated control channels to let the
    // broker create receiver-bound downstream loans without ambient task ids.
    // The call profile does not launch the second stream subscriber. Release
    // its executable and control endpoint so init has room for a private
    // client/client-B coordination channel plus each spawn supervision handle.
    for slot in [FABRIC_SUBSCRIBER_B_SLOT, FABRIC_SUBSCRIBER_B_CONTROL_SLOT] {
        if slime_rt::cap_drop(slot) < 0 {
            slime_rt::exit(1);
        }
    }
    let (phase_client, phase_client_b) =
        slime_rt::endpoint_create(0).unwrap_or_else(|_| slime_rt::exit(1));
    let service = spawn_fabric_client(
        FABRIC_SERVICE_SLOT,
        &[
            grant(0, RIGHT_ENDPOINT_CREATE),
            grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
            grant(FABRIC_CALL_CLIENT_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
            grant(FABRIC_CALL_CLIENT_B_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
            grant(FABRIC_CALL_SERVER_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
            grant(FABRIC_CALL_TIME_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
        ],
        &[FABRIC_SERVICE_SLOT],
    );
    let client = spawn_fabric_client(
        FABRIC_CALL_CLIENT_SLOT,
        &[
            grant(FABRIC_CALL_CLIENT_CONTROL_SLOT, RIGHT_SEND | RIGHT_RECV),
            grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
            grant(service.supervision_slot, RIGHT_SUPERVISE),
            grant(FABRIC_CALL_PHASE_CLIENT_SLOT, RIGHT_SEND),
            grant(phase_client, RIGHT_SEND | RIGHT_RECV),
        ],
        &[FABRIC_CALL_CLIENT_SLOT],
    );
    let client_b = spawn_fabric_client(
        FABRIC_CALL_CLIENT_B_SLOT,
        &[
            grant(FABRIC_CALL_CLIENT_B_CONTROL_SLOT, RIGHT_SEND | RIGHT_RECV),
            grant(phase_client_b, RIGHT_SEND | RIGHT_RECV),
        ],
        &[FABRIC_CALL_CLIENT_B_SLOT],
    );
    let server = spawn_fabric_client(
        FABRIC_CALL_SERVER_SLOT,
        &[
            grant(FABRIC_CALL_SERVER_CONTROL_SLOT, RIGHT_SEND | RIGHT_RECV),
            grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
            grant(service.supervision_slot, RIGHT_SUPERVISE),
        ],
        &[FABRIC_CALL_SERVER_SLOT],
    );

    slime_rt::yield_now();
    for (control, supervision, direction) in [
        (
            FABRIC_CALL_CLIENT_CONTROL_SLOT,
            client.supervision_slot,
            boot_contracts::fabric_graph::DIRECTION_CLIENT,
        ),
        (
            FABRIC_CALL_CLIENT_B_CONTROL_SLOT,
            client_b.supervision_slot,
            boot_contracts::fabric_graph::DIRECTION_CLIENT,
        ),
        (
            FABRIC_CALL_SERVER_CONTROL_SLOT,
            server.supervision_slot,
            boot_contracts::fabric_graph::DIRECTION_SERVER,
        ),
    ] {
        transfer_supervision(control, supervision, direction, call_route_identity());
        if slime_rt::cap_drop(control) < 0 {
            slime_rt::exit(1);
        }
    }

    let time = spawn_fabric_client(
        FABRIC_CALL_TIME_SLOT,
        &[
            grant(FABRIC_CALL_TIME_CONTROL_SLOT, RIGHT_SEND | RIGHT_RECV),
            grant(FABRIC_CALL_PHASE_TIME_SLOT, RIGHT_RECV),
        ],
        &[FABRIC_CALL_TIME_SLOT],
    );
    for slot in [
        FABRIC_CALL_CLIENT_SERVICE_SLOT,
        FABRIC_CALL_CLIENT_B_SERVICE_SLOT,
        FABRIC_CALL_SERVER_SERVICE_SLOT,
        FABRIC_CALL_TIME_SERVICE_SLOT,
    ] {
        if slime_rt::cap_drop(slot) < 0 {
            slime_rt::exit(1);
        }
    }
    slime_rt::yield_now();
    for handle in [time.supervision_slot, service.supervision_slot] {
        loop {
            match slime_rt::supervision_status(handle) {
                Ok(None) => slime_rt::wait(&[slime_rt::WaitSource::Supervision(handle)]),
                Ok(Some(slime_rt::Termination::Exit(0))) => break,
                _ => slime_rt::exit(1),
            }
        }
    }
}

/// Hand the fabric one participant's supervision handle over that participant's
/// own authenticated control channel.
///
/// The descriptor names the exact (route, direction) edge the handle belongs to,
/// because that is what the broker verifies before accepting it: a handle for
/// one route may not be replayed as authority on another. `route` therefore has
/// to be the caller's plane rather than a fixed one — the call and operation
/// planes both use this path.
fn transfer_supervision(control_slot: u32, supervision_slot: u32, direction: u32, route: [u8; 32]) {
    let descriptor = slime_proto::capability_transfer::WireCapabilityTransfer {
        magic: slime_proto::capability_transfer::CAPABILITY_TRANSFER_MAGIC,
        version: slime_proto::capability_transfer::FORMAT_VERSION,
        status: 0,
        flags: 0,
        object_kind: slime_proto::capability_transfer::OBJECT_KIND_SUPERVISION,
        direction,
        rights_mask: RIGHT_SUPERVISE,
        route_identity: route,
    };
    loop {
        match slime_rt::cap_transfer(control_slot, supervision_slot, &descriptor.encode()) {
            slime_rt::ERR_SUCCESS => return,
            slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
            _ => slime_rt::exit(1),
        }
    }
}

/// The `parameters` call route identity, folded from the admitted C8.1 schema so
/// it cannot drift from the graph.
fn call_route_identity() -> [u8; 32] {
    boot_contracts::fabric_graph::route_identity(
        "parameters",
        &slime_proto::interface_schema::parameter_call::INTERFACE_IDENTITY,
        boot_contracts::fabric_graph::CONTRACT_KIND_CALL,
    )
}

/// The `navigation` operation route identity, folded the same way.
fn operation_route_identity() -> [u8; 32] {
    boot_contracts::fabric_graph::route_identity(
        "navigation",
        &slime_proto::interface_schema::navigation_operation::INTERFACE_IDENTITY,
        boot_contracts::fabric_graph::CONTRACT_KIND_OPERATION,
    )
}

/// Launch the C7.7 sample plane: two real components exchanging a payload
/// larger than `MAX_MSG` through the shared-buffer syscalls.
///
/// The receiver spawns first so its supervision handle exists before the lender
/// launches — the lender names its loan receiver through that capability rather
/// than an ambient task id, which is what makes the loan's receiver binding
/// unforgeable. Init waits on the lender, whose own exit follows the receiver's.
fn launch_sample_plane() {
    let receiver = slime_rt::spawn(
        SAMPLE_RECEIVER_SLOT,
        &[grant(
            SAMPLE_RECEIVER_ENDPOINT_SLOT,
            RIGHT_SEND | RIGHT_RECV,
        )],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));

    let lender = slime_rt::spawn(
        SAMPLE_LENDER_SLOT,
        &[
            grant(SAMPLE_LENDER_ENDPOINT_SLOT, RIGHT_SEND | RIGHT_RECV),
            grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
            grant(receiver.supervision_slot, RIGHT_SUPERVISE),
        ],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));

    for handle in [receiver.supervision_slot, lender.supervision_slot] {
        loop {
            match slime_rt::supervision_status(handle) {
                Ok(None) => slime_rt::wait(&[slime_rt::WaitSource::Supervision(handle)]),
                Ok(Some(slime_rt::Termination::Exit(0))) => break,
                _ => slime_rt::exit(1),
            }
        }
    }
}

/// Launch the C8.3/C8.4 fabric plane: one service that owns every route
/// endpoint, and five clients that can only ask it for one.
///
/// Init deliberately holds no route capability. It mints the control channels
/// and hands the fabric one service side per client, then hands each client
/// only its own client side. The binding between a control endpoint and a
/// component identity is established exactly here, at spawn, and is what the
/// fabric authenticates against — a client cannot forge, share, or re-derive
/// one, so "which component is asking" is a capability fact rather than a
/// claim in a message.
///
/// **Spawn order is load-bearing.** Both subscribers are spawned before the
/// fabric, because the fabric needs their supervision handles: a downstream
/// loan names its receiver through a `RIGHT_SUPERVISE` capability, never
/// through an ambient task id, so the handle must exist before the service
/// starts. The publishers follow the fabric, so no sample can arrive before
/// the service is ready to broker it.
///
/// `fabric-intruder` is spawned with a real control endpoint on purpose: the
/// denial under test is not "no channel" but "no declared edge".
fn launch_fabric_graph() {
    // Init launches with 61 of the kernel's 64 capability slots already
    // occupied, and every spawn returns one more supervision handle. Each
    // grant is a non-consuming derive-copy, so a slot init has handed on is
    // still init's until it says otherwise: release the executable and the
    // control endpoint as soon as the spawn that needed them returns. Nothing
    // is dropped that init still uses — it keeps only the supervision handles
    // it waits on.
    let subscriber = spawn_fabric_client(
        FABRIC_SUBSCRIBER_SLOT,
        &[grant(
            FABRIC_SUBSCRIBER_CONTROL_SLOT,
            RIGHT_SEND | RIGHT_RECV,
        )],
        &[FABRIC_SUBSCRIBER_SLOT, FABRIC_SUBSCRIBER_CONTROL_SLOT],
    );
    let subscriber_b = spawn_fabric_client(
        FABRIC_SUBSCRIBER_B_SLOT,
        &[grant(
            FABRIC_SUBSCRIBER_B_CONTROL_SLOT,
            RIGHT_SEND | RIGHT_RECV,
        )],
        &[FABRIC_SUBSCRIBER_B_SLOT, FABRIC_SUBSCRIBER_B_CONTROL_SLOT],
    );

    // The fabric's own authority: an endpoint factory to mint route halves, a
    // shared-buffer factory for the one copy each large sample makes, one
    // control endpoint per client, and one supervision handle per subscriber.
    // The slot order here is the order `fabric-service` reads them in, emitted
    // into its generated profile from this same manifest.
    let service = if option_env!("SLIME_FABRIC_VISIBILITY_CHECK") == Some("1") {
        spawn_fabric_client(
            FABRIC_SERVICE_SLOT,
            &[
                grant(0, RIGHT_ENDPOINT_CREATE),
                // Keep the service's manifest-derived control endpoints at
                // slots 2..; the visibility service drops this otherwise
                // unused factory before handling its first request.
                grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
                grant(FABRIC_PUBLISHER_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
                grant(FABRIC_SUBSCRIBER_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
                grant(FABRIC_INTRUDER_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
                grant(FABRIC_PUBLISHER_B_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
                grant(FABRIC_SUBSCRIBER_B_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
            ],
            &[
                FABRIC_SERVICE_SLOT,
                FABRIC_PUBLISHER_SERVICE_SLOT,
                FABRIC_SUBSCRIBER_SERVICE_SLOT,
                FABRIC_INTRUDER_SERVICE_SLOT,
                FABRIC_PUBLISHER_B_SERVICE_SLOT,
                FABRIC_SUBSCRIBER_B_SERVICE_SLOT,
            ],
        )
    } else if option_env!("SLIME_FABRIC_QOS_CHECK") == Some("1") {
        spawn_fabric_client(
            FABRIC_SERVICE_SLOT,
            &[
                grant(0, RIGHT_ENDPOINT_CREATE),
                grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
                grant(FABRIC_PUBLISHER_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
                grant(FABRIC_SUBSCRIBER_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
                grant(FABRIC_INTRUDER_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
                grant(FABRIC_PUBLISHER_B_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
                grant(FABRIC_SUBSCRIBER_B_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
                grant(subscriber.supervision_slot, RIGHT_SUPERVISE),
                grant(subscriber_b.supervision_slot, RIGHT_SUPERVISE),
                grant(FABRIC_TIME_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
            ],
            &[
                FABRIC_SERVICE_SLOT,
                FABRIC_PUBLISHER_SERVICE_SLOT,
                FABRIC_SUBSCRIBER_SERVICE_SLOT,
                FABRIC_INTRUDER_SERVICE_SLOT,
                FABRIC_PUBLISHER_B_SERVICE_SLOT,
                FABRIC_SUBSCRIBER_B_SERVICE_SLOT,
                FABRIC_TIME_SERVICE_SLOT,
            ],
        )
    } else {
        spawn_fabric_client(
            FABRIC_SERVICE_SLOT,
            &[
                grant(0, RIGHT_ENDPOINT_CREATE),
                grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
                grant(FABRIC_PUBLISHER_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
                grant(FABRIC_SUBSCRIBER_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
                grant(FABRIC_INTRUDER_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
                grant(FABRIC_PUBLISHER_B_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
                grant(FABRIC_SUBSCRIBER_B_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
                grant(subscriber.supervision_slot, RIGHT_SUPERVISE),
                grant(subscriber_b.supervision_slot, RIGHT_SUPERVISE),
            ],
            &[
                FABRIC_SERVICE_SLOT,
                FABRIC_PUBLISHER_SERVICE_SLOT,
                FABRIC_SUBSCRIBER_SERVICE_SLOT,
                FABRIC_INTRUDER_SERVICE_SLOT,
                FABRIC_PUBLISHER_B_SERVICE_SLOT,
                FABRIC_SUBSCRIBER_B_SERVICE_SLOT,
            ],
        )
    };

    let publisher = spawn_fabric_client(
        FABRIC_PUBLISHER_SLOT,
        &[grant(
            FABRIC_PUBLISHER_CONTROL_SLOT,
            RIGHT_SEND | RIGHT_RECV,
        )],
        &[FABRIC_PUBLISHER_SLOT, FABRIC_PUBLISHER_CONTROL_SLOT],
    );
    // `fabric-publisher-b` originates the >MAX_MSG sample, so it needs its own
    // buffer factory and a supervision handle naming the fabric: its upstream
    // loan names the fabric as receiver by capability.
    let publisher_b = if option_env!("SLIME_FABRIC_VISIBILITY_CHECK") == Some("1") {
        spawn_fabric_client(
            FABRIC_PUBLISHER_B_SLOT,
            &[grant(
                FABRIC_PUBLISHER_B_CONTROL_SLOT,
                RIGHT_SEND | RIGHT_RECV,
            )],
            &[FABRIC_PUBLISHER_B_SLOT, FABRIC_PUBLISHER_B_CONTROL_SLOT],
        )
    } else if option_env!("SLIME_FABRIC_QOS_CHECK") == Some("1") {
        spawn_fabric_client(
            FABRIC_PUBLISHER_B_SLOT,
            &[
                grant(FABRIC_PUBLISHER_B_CONTROL_SLOT, RIGHT_SEND | RIGHT_RECV),
                grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
                grant(service.supervision_slot, RIGHT_SUPERVISE),
                grant(FABRIC_TIME_CLIENT_SLOT, RIGHT_SEND | RIGHT_RECV),
            ],
            &[FABRIC_PUBLISHER_B_SLOT, FABRIC_PUBLISHER_B_CONTROL_SLOT],
        )
    } else {
        spawn_fabric_client(
            FABRIC_PUBLISHER_B_SLOT,
            &[
                grant(FABRIC_PUBLISHER_B_CONTROL_SLOT, RIGHT_SEND | RIGHT_RECV),
                grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
                grant(service.supervision_slot, RIGHT_SUPERVISE),
            ],
            &[FABRIC_PUBLISHER_B_SLOT, FABRIC_PUBLISHER_B_CONTROL_SLOT],
        )
    };
    if option_env!("SLIME_FABRIC_QOS_CHECK") == Some("1")
        && slime_rt::cap_drop(FABRIC_TIME_CLIENT_SLOT) < 0
    {
        slime_rt::exit(1);
    }
    let intruder = spawn_fabric_client(
        FABRIC_INTRUDER_SLOT,
        &[grant(FABRIC_INTRUDER_CONTROL_SLOT, RIGHT_SEND | RIGHT_RECV)],
        &[FABRIC_INTRUDER_SLOT, FABRIC_INTRUDER_CONTROL_SLOT],
    );

    for handle in [
        publisher.supervision_slot,
        publisher_b.supervision_slot,
        intruder.supervision_slot,
        subscriber.supervision_slot,
        subscriber_b.supervision_slot,
        service.supervision_slot,
    ] {
        loop {
            match slime_rt::supervision_status(handle) {
                Ok(None) => slime_rt::wait(&[slime_rt::WaitSource::Supervision(handle)]),
                Ok(Some(slime_rt::Termination::Exit(0))) => break,
                _ => slime_rt::exit(1),
            }
        }
    }
}

/// Spawn one fabric component, then release the slots that spawn consumed.
///
/// `release` names capabilities init holds only to hand on: the executable it
/// just launched, and the control endpoints now owned by the child or the
/// service. A grant is a non-consuming derive-copy, so without this init keeps
/// every one of them and runs out of the kernel's 64 capability slots partway
/// through the graph — a failure that looks like a spawn error rather than a
/// leak. Init keeps only the supervision handle it returns and waits on.
fn spawn_fabric_client(
    executable_slot: u32,
    grants: &[SpawnGrant],
    release: &[u32],
) -> slime_rt::Spawned {
    let spawned = slime_rt::spawn(executable_slot, grants).unwrap_or_else(|error| {
        slime_rt::debug_write(b"[init] fabric spawn failed slot=");
        write_u32(executable_slot);
        slime_rt::debug_write(b" error=");
        write_i64(error);
        slime_rt::debug_write(b"\n");
        slime_rt::exit(1)
    });
    for slot in release {
        if slime_rt::cap_drop(*slot) < 0 {
            slime_rt::exit(1);
        }
    }
    spawned
}

fn spawn_or_fail(executable_slot: u32, grants: &[SpawnGrant]) {
    let spawned = slime_rt::spawn(executable_slot, grants).unwrap_or_else(|error| {
        slime_rt::debug_write(b"[init] spawn failed slot=");
        write_u32(executable_slot);
        slime_rt::debug_write(b" error=");
        write_i64(error);
        slime_rt::debug_write(b"\n");
        slime_rt::exit(1)
    });
    if slime_rt::cap_drop(spawned.supervision_slot) < 0 {
        slime_rt::exit(1);
    }
}

fn spawn_optional_storage(executable_slot: u32, grants: &[SpawnGrant]) {
    match slime_rt::spawn(executable_slot, grants) {
        Ok(spawned) => {
            if slime_rt::cap_drop(spawned.supervision_slot) < 0 {
                slime_rt::exit(1);
            }
        }
        // No block device attached: the storage slot holds an ObjectStore
        // fallback, so the BLOCK_READ derive is rejected. Treat this as the
        // absent-storage case and continue launching the rest of the graph.
        Err(slime_rt::ERR_BAD_CAP) => {
            slime_rt::debug_write(b"[init] storage-probe skipped: no block device\n");
        }
        Err(error) => {
            slime_rt::debug_write(b"[init] spawn failed slot=");
            write_u32(executable_slot);
            slime_rt::debug_write(b" error=");
            write_i64(error);
            slime_rt::debug_write(b"\n");
            slime_rt::exit(1);
        }
    }
}

fn spawn_and_wait(executable_slot: u32, grants: &[SpawnGrant]) {
    let spawned = slime_rt::spawn(executable_slot, grants).unwrap_or_else(|error| {
        slime_rt::debug_write(b"[init] spawn failed slot=");
        write_u32(executable_slot);
        slime_rt::debug_write(b" error=");
        write_i64(error);
        slime_rt::debug_write(b"\n");
        slime_rt::exit(1)
    });
    loop {
        match slime_rt::supervision_status(spawned.supervision_slot) {
            Ok(None) => {
                slime_rt::wait(&[slime_rt::WaitSource::Supervision(spawned.supervision_slot)])
            }
            Ok(Some(slime_rt::Termination::Exit(0))) => return,
            _ => slime_rt::exit(1),
        }
    }
}

fn write_i64(value: i64) {
    if value < 0 {
        slime_rt::debug_write(b"-");
        write_u32(value.unsigned_abs() as u32);
    } else {
        write_u32(value as u32);
    }
}

fn write_u32(mut value: u32) {
    let mut buffer = [0u8; 10];
    let mut cursor = buffer.len();
    if value == 0 {
        slime_rt::debug_write(b"0");
        return;
    }
    while value != 0 {
        cursor -= 1;
        buffer[cursor] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    slime_rt::debug_write(&buffer[cursor..]);
}
