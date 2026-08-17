#![no_std]
#![no_main]

slime_rt::entry!(main);

use slime_rt::{CapabilityDisposition, Rights, SpawnGrant};

/// The resolved fabric profile for the graph this binary was built against.
/// Init reads only `FABRIC_MINTED_GRANTS` from it: how many capabilities each
/// child's manifest says its owner must supply at spawn. That count used to be
/// a hardcoded list here, which was the stream graph's, so every other plane's
/// spawn was refused for a count init had no way to know (B50/R2).
#[allow(dead_code)]
mod profile {
    include!(concat!(env!("OUT_DIR"), "/fabric_profile.rs"));
}

/// How many capabilities `component`'s manifest says its owner must hand it at
/// spawn. The root matches a spawn request positionally against the child's
/// declarations in ascending destination-slot order and refuses any other
/// count, so this is the one number init must agree with -- and it is read
/// from the resolved profile rather than restated here per plane.
fn declared_minted_grants(component: &[u8]) -> usize {
    profile::FABRIC_MINTED_GRANTS
        .iter()
        .find(|(holder, _)| *holder == component)
        .map_or(0, |(_, count)| *count)
}

const RIGHT_SEND: Rights = 1;
const RIGHT_TRANSFER: Rights = 4;
const RIGHT_BLOCK_READ: Rights = 1 << 10;
const RIGHT_BLOCK_WRITE: Rights = 1 << 11;
const RIGHT_STORE_READ: Rights = 1 << 12;
const RIGHT_STORE_WRITE: Rights = 1 << 13;
const RIGHT_HEALTH_CONFIRM: Rights = 1 << 14;
const RIGHT_BOOT_UPDATE: Rights = 1 << 15;
const RIGHT_EXEC: Rights = 1 << 3;
const RIGHT_SPAWN: Rights = 1 << 16;
const RIGHT_DIRECTORY_READ: Rights = 1 << 19;
const RIGHT_DIRECTORY_WRITE: Rights = 1 << 20;
const RIGHT_DIRECTORY_LIST: Rights = 1 << 21;
const RIGHT_DIRECTORY_DERIVE: Rights = 1 << 22;
const RIGHT_INPUT_READ: Rights = 1 << 23;
const RIGHT_BUFFER_CREATE: Rights = 1 << 24;
const RIGHT_SUPERVISE: Rights = 1 << 18;

// Manifest-derived bootstrap slot order is emitted by the host builder.
const CONSOLE_CAPS: [SpawnGrant; 0] = [];
const STORAGE_PROBE_READ_CAPS: [SpawnGrant; 1] = [grant(STORAGE_CAPABILITY_SLOT, RIGHT_BLOCK_READ)];
const STORAGE_PROBE_WRITE_CAPS: [SpawnGrant; 1] = [grant(
    STORAGE_CAPABILITY_SLOT,
    RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE,
)];
const STORAGE_PROBE_STORE_CAPS: [SpawnGrant; 1] = [grant(
    OBJECT_STORE_SLOT_0,
    RIGHT_STORE_READ | RIGHT_STORE_WRITE,
)];
const GENERATION_MANAGER_CAPS: [SpawnGrant; 1] = [grant(
    GENERATION_CONTROL_SLOT,
    RIGHT_HEALTH_CONFIRM | RIGHT_BOOT_UPDATE,
)];

fn dango_caps() -> [SpawnGrant; 4] {
    [
        grant(INPUT_SLOT, RIGHT_INPUT_READ),
        grant(
            DIRECTORY_SLOT,
            RIGHT_DIRECTORY_READ | RIGHT_DIRECTORY_DERIVE | RIGHT_TRANSFER,
        ),
        grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
        grant(DANGO_OUTPUT_SLOT, RIGHT_TRANSFER),
    ]
}

fn spawn_service_caps() -> [SpawnGrant; 3] {
    [
        grant(SYSINFO_SLOT, RIGHT_EXEC | RIGHT_SPAWN),
        grant(ECHO_AGENT_SLOT, RIGHT_EXEC | RIGHT_SPAWN),
        grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
    ]
}

fn filesystem_caps() -> [SpawnGrant; 1] {
    [grant(
        OBJECT_STORE_SLOT,
        RIGHT_STORE_READ | RIGHT_STORE_WRITE,
    )]
}

const DIRECTORY_PROBE_CAPS: [SpawnGrant; 1] = [grant(
    DIRECTORY_SLOT,
    RIGHT_TRANSFER
        | RIGHT_DIRECTORY_READ
        | RIGHT_DIRECTORY_WRITE
        | RIGHT_DIRECTORY_LIST
        | RIGHT_DIRECTORY_DERIVE,
)];

const GENERATION_LIST_CAPS: [SpawnGrant; 0] = [];
const GENERATION_INSPECT_CAPS: [SpawnGrant; 0] = [];
const GENERATION_STAGE_CAPS: [SpawnGrant; 0] = [];
const GENERATION_SELECT_CAPS: [SpawnGrant; 0] = [];
const GENERATION_ROLLBACK_CAPS: [SpawnGrant; 0] = [];
// Init's slot numbers come from the generation's boot layout, emitted by
// `scripts/build/boot_layout.py` into `OUT_DIR` at component build time. The
// kernel places each capability at the slot the same layout names, so the
// component that uses a slot and the kernel that fills it read one source
// rather than two hand-maintained lists that agreed by inspection (B10).
//
// A label this generation does not declare is `SLOT_ABSENT`. Every generation
// emits the same constant names, and runtime branches test those values rather
// than hidden build flags.
include!(concat!(env!("OUT_DIR"), "/boot_layout.rs"));

const fn grant(slot: u32, rights: Rights) -> SpawnGrant {
    SpawnGrant { slot, rights }
}

#[derive(Clone, Copy)]
enum StorageProbe {
    Store,
    Writer,
    Fault,
    Reader,
}

fn storage_probe() -> (u32, StorageProbe) {
    if STORAGE_WRITER_SLOT != SLOT_ABSENT {
        (STORAGE_WRITER_SLOT, StorageProbe::Writer)
    } else if STORAGE_FAULT_PROBE_SLOT != SLOT_ABSENT {
        (STORAGE_FAULT_PROBE_SLOT, StorageProbe::Fault)
    } else if STORAGE_STORE_PROBE_SLOT != SLOT_ABSENT {
        (STORAGE_STORE_PROBE_SLOT, StorageProbe::Store)
    } else {
        (STORAGE_PROBE_SLOT, StorageProbe::Reader)
    }
}

fn storage_caps(probe: StorageProbe) -> &'static [SpawnGrant] {
    match probe {
        StorageProbe::Store => &STORAGE_PROBE_STORE_CAPS,
        StorageProbe::Writer | StorageProbe::Fault => &STORAGE_PROBE_WRITE_CAPS,
        StorageProbe::Reader => &STORAGE_PROBE_READ_CAPS,
    }
}

/// The authenticated boot action, as the root delivers it in this thread's
/// first C parameter. The numbering is `boot_contracts::generation::BootAction`
/// and is fixed by the generation contract, not by this file.
mod boot_action {
    pub const PRODUCT: u32 = 1;
    pub const BOOT: u32 = 2;
    pub const CALL: u32 = 3;
    pub const CHANNEL: u32 = 4;
    pub const CROSSING: u32 = 5;
    pub const DANGO: u32 = 6;
    pub const DIRECTORY: u32 = 7;
    pub const FILESYSTEM: u32 = 8;
    pub const GENERATION: u32 = 9;
    pub const INPUT: u32 = 10;
    pub const LOAN: u32 = 11;
    pub const OPERATION: u32 = 12;
    pub const POWERBOX: u32 = 13;
    pub const QOS: u32 = 14;
    pub const RECLAMATION: u32 = 15;
    pub const RECOVERY: u32 = 16;
    pub const ROLLBACK: u32 = 17;
    pub const SAMPLE: u32 = 18;
    pub const SPAWN: u32 = 19;
    pub const STORAGE: u32 = 20;
    pub const STORE: u32 = 21;
    pub const STREAM: u32 = 22;
    pub const SUPERVISION: u32 = 23;
    pub const TRANSFER: u32 = 24;
    pub const VISIBILITY: u32 = 25;
    /// The 48-instance ceiling graph (B49).
    pub const STRESS: u32 = 26;
    /// C8.12's matching, visibility, and denial matrix.
    pub const MATRIX: u32 = 27;
    /// C8.13's concurrent cross-plane traffic and resource ceilings.
    pub const TRAFFIC: u32 = 28;

    // The table above is a hand copy of the contract's numbering, and the two
    // are an ABI: the root passes one of these words to this thread and this
    // file matches on it. Renumbering a variant in the contract without
    // updating this table would silently compose a different plane, so the
    // agreement is asserted at compile time rather than left to review.
    use boot_contracts::generation::BootAction;
    const _: () = assert!(PRODUCT == BootAction::Product.id());
    const _: () = assert!(BOOT == BootAction::Boot.id());
    const _: () = assert!(CALL == BootAction::Call.id());
    const _: () = assert!(CHANNEL == BootAction::Channel.id());
    const _: () = assert!(CROSSING == BootAction::Crossing.id());
    const _: () = assert!(DANGO == BootAction::Dango.id());
    const _: () = assert!(DIRECTORY == BootAction::Directory.id());
    const _: () = assert!(FILESYSTEM == BootAction::Filesystem.id());
    const _: () = assert!(GENERATION == BootAction::Generation.id());
    const _: () = assert!(INPUT == BootAction::Input.id());
    const _: () = assert!(LOAN == BootAction::Loan.id());
    const _: () = assert!(OPERATION == BootAction::Operation.id());
    const _: () = assert!(POWERBOX == BootAction::Powerbox.id());
    const _: () = assert!(QOS == BootAction::Qos.id());
    const _: () = assert!(RECLAMATION == BootAction::Reclamation.id());
    const _: () = assert!(RECOVERY == BootAction::Recovery.id());
    const _: () = assert!(ROLLBACK == BootAction::Rollback.id());
    const _: () = assert!(SAMPLE == BootAction::Sample.id());
    const _: () = assert!(SPAWN == BootAction::Spawn.id());
    const _: () = assert!(STORAGE == BootAction::Storage.id());
    const _: () = assert!(STORE == BootAction::Store.id());
    const _: () = assert!(STREAM == BootAction::Stream.id());
    const _: () = assert!(SUPERVISION == BootAction::Supervision.id());
    const _: () = assert!(TRANSFER == BootAction::Transfer.id());
    const _: () = assert!(VISIBILITY == BootAction::Visibility.id());
    const _: () = assert!(STRESS == BootAction::Stress.id());
    const _: () = assert!(MATRIX == BootAction::Matrix.id());
    const _: () = assert!(TRAFFIC == BootAction::Traffic.id());
}

/// Compose the graph the generation selected.
///
/// The selector is authenticated generation data delivered at activation, so
/// two builds of this image cannot disagree about which graph they boot: the
/// image is byte-identical across every manifest and only the admitted
/// `bootAction` differs.
///
/// Returns for `PRODUCT`, whose graph the caller launches; every other action
/// runs its plane to completion and exits.
fn compose_declared_graph(startup_arg: u32) {
    use boot_action as action;
    match startup_arg {
        action::BOOT => drive_boot_plane(),
        action::CHANNEL => {
            drive_channel_plane();
            slime_rt::debug_write(b"[init] channel plane complete\n");
            slime_rt::exit(0)
        }
        action::LOAN => {
            drive_loan_plane();
            slime_rt::debug_write(b"[init] loan plane complete\n");
            slime_rt::exit(0)
        }
        action::SPAWN => {
            drive_spawn_plane();
            slime_rt::debug_write(b"[init] spawn plane complete\n");
            slime_rt::exit(0)
        }
        action::SAMPLE => {
            drive_sample_plane();
            slime_rt::debug_write(b"[init] sample plane complete\n");
            slime_rt::exit(0)
        }
        action::STREAM | action::QOS => {
            drive_stream_plane(startup_arg == boot_action::QOS);
            slime_rt::debug_write(b"[init] fabric stream complete\n");
            slime_rt::exit(0)
        }
        action::SUPERVISION => {
            drive_supervision_plane();
            slime_rt::debug_write(b"[init] supervision plane complete\n");
            slime_rt::exit(0)
        }
        action::RECLAMATION => {
            drive_reclamation_plane();
            slime_rt::debug_write(b"[init] reclamation plane complete\n");
            slime_rt::exit(0)
        }
        action::CROSSING => {
            drive_crossing_plane();
            slime_rt::debug_write(b"[init] crossing plane complete\n");
            slime_rt::exit(0)
        }
        action::CALL => {
            drive_call_plane();
            slime_rt::debug_write(b"[init] call plane complete\n");
            slime_rt::exit(0)
        }
        action::OPERATION => {
            drive_operation_plane();
            slime_rt::debug_write(b"[init] operation plane complete\n");
            slime_rt::exit(0)
        }
        action::VISIBILITY => {
            drive_visibility_plane();
            slime_rt::debug_write(b"[init] visibility plane complete\n");
            slime_rt::exit(0)
        }
        action::MATRIX => {
            drive_matrix_plane();
            slime_rt::debug_write(b"[init] matrix plane complete\n");
            slime_rt::exit(0)
        }
        action::TRAFFIC => drive_traffic_plane(),
        action::STORAGE => {
            drive_storage_plane();
            slime_rt::debug_write(b"[init] storage plane complete\n");
            slime_rt::exit(0)
        }
        action::STORE => {
            drive_store_plane();
            slime_rt::debug_write(b"[init] store plane complete\n");
            slime_rt::exit(0)
        }
        action::POWERBOX => {
            drive_powerbox_plane();
            slime_rt::debug_write(b"[init] powerbox plane complete\n");
            slime_rt::exit(0)
        }
        action::DANGO => {
            drive_dango_plane();
            slime_rt::debug_write(b"[init] dango plane complete\n");
            slime_rt::exit(0)
        }
        action::FILESYSTEM => {
            drive_filesystem_plane();
            slime_rt::debug_write(b"[init] filesystem plane complete\n");
            slime_rt::exit(0)
        }
        action::GENERATION => {
            drive_generation_plane();
            slime_rt::debug_write(b"[init] generation plane complete\n");
            slime_rt::exit(0)
        }
        action::TRANSFER => {
            drive_probe_plane_with_token(
                SEL4_TRANSFER_PROBE_SLOT,
                b"[init] transfer probe spawned\n",
                b"transfer",
                Some(2),
            );
            slime_rt::debug_write(b"[init] transfer plane complete\n");
            slime_rt::exit(0)
        }
        action::INPUT => {
            drive_probe_plane_with_token(
                SEL4_INPUT_PROBE_SLOT,
                b"[init] input probe spawned\n",
                b"input",
                // Init's own end of `sel4-input-probe-run-token`. The idle
                // instance holds no such edge, which is what tells the two
                // instances of this executable apart.
                Some(2),
            );
            slime_rt::debug_write(b"[init] input plane complete\n");
            slime_rt::exit(0)
        }
        action::DIRECTORY => {
            drive_probe_plane_with_token(
                SEL4_DIRECTORY_PROBE_SLOT,
                b"[init] directory probe spawned\n",
                b"directory",
                // Init's own end of `sel4-directory-probe-run-token`; the idle
                // instance holds a loopback nobody sends on.
                Some(2),
            );
            slime_rt::debug_write(b"[init] directory plane complete\n");
            slime_rt::exit(0)
        }
        action::RECOVERY => {
            drive_probe_plane_with_token(
                SEL4_RECOVERY_PROBE_SLOT,
                b"[init] recovery probe spawned\n",
                b"recovery",
                Some(2),
            );
            slime_rt::debug_write(b"[init] recovery plane complete\n");
            slime_rt::exit(0)
        }
        action::ROLLBACK => {
            drive_probe_plane_with_token(
                SEL4_ROLLBACK_PROBE_SLOT,
                b"[init] rollback probe spawned\n",
                b"rollback",
                Some(2),
            );
            slime_rt::debug_write(b"[init] rollback plane complete\n");
            slime_rt::exit(0)
        }
        // The stress plane declares the largest graph the root's CSpace
        // admits and nothing else: what it proves is that every instance is
        // constructed and stays bounded, which the root reports itself. init
        // has no scenario to drive (B49).
        action::STRESS => {
            slime_rt::debug_write(b"[init] stress plane complete\n");
            slime_rt::exit(0)
        }
        action::PRODUCT => {}
        // An action this image does not implement is a generation the graph
        // cannot compose, which is a boot failure rather than a silent
        // fallthrough to some other graph.
        _ => {
            slime_rt::debug_write(b"[init] unknown boot action\n");
            slime_rt::exit(1)
        }
    }
}

fn main(startup_arg: u32) {
    if option_env!("SLIME_BOOT_SELECTION_FAIL") == Some("1") {
        slime_rt::debug_write(b"[init] reporting unhealthy boot\n");
        slime_rt::unhealthy();
    }
    // The authenticated manifest action selects every non-product composition.
    // `PRODUCT` returns so the ordinary component graph below can launch.
    compose_declared_graph(startup_arg);
    slime_rt::debug_write(b"[init] launching component graph\n");

    if FILESYSTEM_SERVICE_SLOT != SLOT_ABSENT && DIRECTORY_PROBE_SLOT != SLOT_ABSENT {
        spawn_or_fail(FILESYSTEM_SERVICE_SLOT, &filesystem_caps());
        spawn_or_fail(DIRECTORY_PROBE_SLOT, &DIRECTORY_PROBE_CAPS);
    }
    let mut component_console = None;
    let mut component_spawn_service = None;
    let mut component_dango = None;
    let generation_command_plane = GENERATION_LIST_SLOT != SLOT_ABSENT;
    if !generation_command_plane {
        component_console = Some(
            slime_rt::spawn(CONSOLE_SLOT, &CONSOLE_CAPS)
                .unwrap_or_else(|_| slime_rt::exit(1))
                .supervision_slot,
        );
        if DANGO_SLOT != SLOT_ABSENT {
            component_dango = Some(
                slime_rt::spawn(DANGO_SLOT, &dango_caps())
                    .unwrap_or_else(|_| slime_rt::exit(1))
                    .supervision_slot,
            );
        }
        component_spawn_service = Some(
            slime_rt::spawn(SPAWN_SERVICE_SLOT, &spawn_service_caps())
                .unwrap_or_else(|_| slime_rt::exit(1))
                .supervision_slot,
        );
        let (storage_slot, storage_probe) = storage_probe();
        if storage_slot != SLOT_ABSENT {
            spawn_optional_storage(storage_slot, storage_caps(storage_probe));
        }
    }
    if !generation_command_plane && GENERATION_MANAGER_SLOT != SLOT_ABSENT {
        spawn_or_fail(GENERATION_MANAGER_SLOT, &GENERATION_MANAGER_CAPS);
    }
    if generation_command_plane {
        let negative_scenario = matches!(
            option_env!("SLIME_GENERATION_CMD_SCENARIO"),
            Some("bad-closure" | "bad-release")
        );
        spawn_or_fail(GENERATION_MANAGER_SLOT, &GENERATION_MANAGER_CAPS);
        spawn_and_wait(GENERATION_LIST_SLOT, &GENERATION_LIST_CAPS);
        if !negative_scenario {
            spawn_and_wait(GENERATION_INSPECT_SLOT, &GENERATION_INSPECT_CAPS);
        }
        spawn_and_wait(GENERATION_STAGE_SLOT, &GENERATION_STAGE_CAPS);
        if negative_scenario {
            slime_rt::debug_write(b"[init] negative generation scenario complete\n");
            slime_rt::exit(0);
        }
        spawn_and_wait(GENERATION_SELECT_SLOT, &GENERATION_SELECT_CAPS);
        spawn_and_wait(GENERATION_ROLLBACK_SLOT, &GENERATION_ROLLBACK_CAPS);
    }

    if let Some(handle) = component_dango {
        wait_terminated(&[handle]);
    }
    if let Some(handle) = component_spawn_service {
        let shutdown = slime_proto::spawn::WireSpawnRequest {
            magic: slime_proto::spawn::SPAWN_MAGIC,
            version: slime_proto::spawn::FORMAT_VERSION,
            flags: slime_proto::spawn::REQUEST_FLAG_SHUTDOWN,
            command_len: 0,
            argument_count: 0,
            environment_count: 0,
            capability_roles: 0,
            client_budget: 0,
            command: [0; 16],
            arguments: [0; 8],
            environment: [0; 8],
            grant_rights: 0,
            reserved: [0; 6],
        };
        if slime_rt::send(SPAWN_SERVICE_RPC_SLOT, &shutdown.encode(), &[]) != slime_rt::ERR_SUCCESS
        {
            slime_rt::exit(1);
        }
        wait_clean(&[handle]);
    }
    if let Some(handle) = component_console {
        if slime_rt::send(console_send_slot(), b"SLIME.CONSOLE.CLOSE", &[]) != slime_rt::ERR_SUCCESS
        {
            slime_rt::exit(1);
        }
        wait_clean(&[handle]);
    }
    if component_spawn_service.is_some() || component_console.is_some() {
        slime_rt::debug_write(b"[init] component services completed\n");
    }
    slime_rt::debug_write(b"[init] spawn graph launched\n");
    slime_rt::exit(0);
}

/// Drive the P5.4.9 full-graph boot: every C8 role in one generation.
///
/// Only reachable for the authenticated `boot` action declared by
/// `contracts/generation/v1/fixtures/sel4-boot.zti`. Every participant reads
/// the same generation-derived action from its generated profile.
///
/// **Every control endpoint is placed by the generation, not by init.** Each is
/// a declared native seL4 Endpoint whose two halves the root installs into the
/// instances that declared them, before either runs. Init holds no route
/// capability and mints nothing, so `SEL4_BOOT_LAYOUT` numbers only the
/// twenty-one things the generation places. That is also what binds a control
/// endpoint to one component identity: a worker answers "which component is
/// asking" from the endpoint a request arrived on, and no party can forge,
/// share, or re-derive one.
///
/// **Init spawns all nineteen, including both route workers.** A supervision
/// handle cannot exist before its subject task, and a minted binding is only
/// satisfiable by the instance that owns it — `preflight_spawn_grants` refuses
/// `declared.owner != caller_instance`. The workers need handles naming call and
/// operation participants, and only init ever holds those, so only init can
/// spawn the workers. Having `fabric-service` spawn them (as the retired
/// custom-kernel graph did) left both workers with no control endpoints and no
/// handles, because neither an endpoint grant nor an endpoint-kind minted
/// binding can cross a spawn boundary (B55).
///
/// **Spawn order is load-bearing.** Every task whose supervision handle appears
/// in a later grant vector must already exist, so the six stream-ring and proxy
/// identities precede `fabric-service`, and the seven call and operation
/// identities precede their workers.
///
/// Init does not exit. The gate's exit condition is the whole graph at healthy
/// blocked idle, so init parks on the fabric's handle — a component terminating
/// here is a failure, not something to wait for.
fn drive_boot_plane() -> ! {
    // Every control endpoint below is a generation-declared native seL4
    // Endpoint the root installed into both declaring instances before this
    // task, or any child it spawns, ran at all. Init places nothing.
    slime_rt::debug_write(b"[init] fabric boot control channels minted\n");
    // The stream broker's loan receivers, in the order the resolved profile
    // numbers them at slots 9..14. `FABRIC_SUPERVISION` derives that order, and
    // the fixture's minted bindings must agree row for row.
    let publisher = spawn_boot(FABRIC_PUBLISHER_SLOT);
    let subscriber = spawn_boot(FABRIC_SUBSCRIBER_SLOT);
    let publisher_b = spawn_boot(FABRIC_PUBLISHER_B_SLOT);
    let subscriber_b = spawn_boot(FABRIC_SUBSCRIBER_B_SLOT);
    let observer = spawn_boot(FABRIC_OBSERVER_SLOT);
    let proxy = spawn_boot(FABRIC_PROXY_SLOT);
    // Holds a real control endpoint and is granted no edge; its denial is the
    // plane's authority evidence, so it needs no handle from anyone.
    spawn_boot(FABRIC_PROBE_SLOT);
    slime_rt::debug_write(b"[init] fabric boot stream participants spawned\n");

    // Matched positionally against the child's declarations in ascending
    // destination-slot order: the factory at 1, then the six handles at 9..14.
    let fabric = spawn_boot_with(
        FABRIC_SERVICE_SLOT,
        &[
            grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
            grant(publisher, RIGHT_SUPERVISE),
            grant(subscriber, RIGHT_SUPERVISE),
            grant(publisher_b, RIGHT_SUPERVISE),
            grant(subscriber_b, RIGHT_SUPERVISE),
            grant(observer, RIGHT_SUPERVISE),
            grant(proxy, RIGHT_SUPERVISE),
        ],
    );
    slime_rt::debug_write(b"[init] fabric boot stream broker spawned\n");

    let call_client = spawn_boot(FABRIC_CALL_CLIENT_SLOT);
    let call_client_b = spawn_boot(FABRIC_CALL_CLIENT_B_SLOT);
    let call_server = spawn_boot(FABRIC_CALL_SERVER_SLOT);
    // The clock asks for nothing and is named by no handle: the worker observes
    // its exit through the control endpoint's own peer state.
    spawn_boot(FABRIC_CALL_TIME_SLOT);
    // The call worker copies large payloads, so it holds buffer-creation
    // authority of its own, bounded by its declared quota.
    spawn_boot_with(
        FABRIC_CALL_WORKER_SLOT,
        &[
            grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
            grant(call_client, RIGHT_SUPERVISE),
            grant(call_client_b, RIGHT_SUPERVISE),
            grant(call_server, RIGHT_SUPERVISE),
        ],
    );
    slime_rt::debug_write(b"[init] fabric boot call plane spawned\n");

    let op_client = spawn_boot(FABRIC_OP_CLIENT_SLOT);
    let op_client_b = spawn_boot(FABRIC_OP_CLIENT_B_SLOT);
    let op_server = spawn_boot(FABRIC_OP_SERVER_SLOT);
    spawn_boot(FABRIC_OP_TIME_SLOT);
    let op_restart = spawn_boot(FABRIC_OP_CLIENT_B_RESTART_SLOT);
    spawn_boot_with(
        FABRIC_OP_WORKER_SLOT,
        &[
            grant(op_client, RIGHT_SUPERVISE),
            grant(op_client_b, RIGHT_SUPERVISE),
            grant(op_server, RIGHT_SUPERVISE),
            grant(op_restart, RIGHT_SUPERVISE),
        ],
    );
    slime_rt::debug_write(b"[init] fabric boot operation plane spawned\n");
    slime_rt::debug_write(b"[init] fabric boot graph spawned with static endpoints\n");
    loop {
        match slime_rt::supervision_status(fabric) {
            Ok(None) => slime_rt::yield_now(),
            _ => fail_boot(b"fabric left healthy idle"),
        }
    }
}

/// Drive the C8.13 concurrent traffic plane: the C8.10 three-worker layout,
/// spawned exactly as `drive_boot_plane` spawns it, driving every non-parked
/// worker's real scenario at once instead of parking.
///
/// Only reachable for the authenticated `traffic` action declared by
/// `contracts/generation/v1/fixtures/sel4-traffic.zti`, which is
/// `sel4-boot.zti` with `bootAction`/`generation` changed plus the additional
/// grants real traffic needs that the parked boot scenario never exercised:
/// call-plane phase-barrier edges, an operation-plane restart-release edge,
/// three shared-buffer factories, and `transferable = true` on the call
/// control grants (a downstream loan crosses authority, so delegating one
/// needs the right the parked scenario never asked for). The milestone's
/// requirement is that the *same* declared partition C8.10 already proved
/// collision-free now carries real call and operation traffic concurrently
/// with *untimed* stream traffic (`fabric_boot::active()` reading `"traffic"`
/// differently than `"boot"` is what makes every participant but the observer
/// and proxy run its real per-plane scenario instead of
/// `provision_and_park`/`park_only`) under one fixed schedule. QoS-timed
/// stream traffic is deliberately absent: `fabric-service::qos_check` stays
/// `"qos"`-only, because timing it here needs its own generation-level
/// clock-grant wiring this milestone does not yet do -- see the C8.13 devlog
/// entry's open risks.
///
/// **Why `wait_clean` and not `drive_boot_plane`'s infinite idle loop.** The
/// boot plane's participants never produce traffic, so its brokers never
/// reach their own `finished()` and the plane is a permanent snapshot a gate
/// inspects from outside. Here every participant runs its real scenario to
/// completion, so every worker's own `run()` returns and every handle below
/// is expected to settle clean -- the ordinary completion path every other
/// concurrent plane in this file uses.
fn drive_traffic_plane() -> ! {
    slime_rt::debug_write(b"[init] traffic control channels minted\n");
    let publisher = spawn_boot(FABRIC_PUBLISHER_SLOT);
    let subscriber = spawn_boot(FABRIC_SUBSCRIBER_SLOT);
    let publisher_b = spawn_boot_with(
        FABRIC_PUBLISHER_B_SLOT,
        &[grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE)],
    );
    let subscriber_b = spawn_boot(FABRIC_SUBSCRIBER_B_SLOT);
    let observer = spawn_boot(FABRIC_OBSERVER_SLOT);
    let proxy = spawn_boot(FABRIC_PROXY_SLOT);
    let probe = spawn_boot(FABRIC_PROBE_SLOT);
    slime_rt::debug_write(b"[init] traffic stream participants spawned\n");

    let fabric = spawn_boot_with(
        FABRIC_SERVICE_SLOT,
        &[
            grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
            grant(publisher, RIGHT_SUPERVISE),
            grant(subscriber, RIGHT_SUPERVISE),
            grant(publisher_b, RIGHT_SUPERVISE),
            grant(subscriber_b, RIGHT_SUPERVISE),
            grant(observer, RIGHT_SUPERVISE),
            grant(proxy, RIGHT_SUPERVISE),
        ],
    );
    slime_rt::debug_write(b"[init] traffic stream broker spawned\n");

    let call_client = spawn_boot_with(
        FABRIC_CALL_CLIENT_SLOT,
        &[grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE)],
    );
    let call_client_b = spawn_boot(FABRIC_CALL_CLIENT_B_SLOT);
    let call_server = spawn_boot_with(
        FABRIC_CALL_SERVER_SLOT,
        &[grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE)],
    );
    let call_time = spawn_boot(FABRIC_CALL_TIME_SLOT);
    let call_worker = spawn_boot_with(
        FABRIC_CALL_WORKER_SLOT,
        &[
            grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
            grant(call_client, RIGHT_SUPERVISE),
            grant(call_client_b, RIGHT_SUPERVISE),
            grant(call_server, RIGHT_SUPERVISE),
        ],
    );
    slime_rt::debug_write(b"[init] traffic call plane spawned\n");

    let op_client = spawn_boot(FABRIC_OP_CLIENT_SLOT);
    let op_client_b = spawn_boot(FABRIC_OP_CLIENT_B_SLOT);
    let op_server = spawn_boot(FABRIC_OP_SERVER_SLOT);
    let op_time = spawn_boot(FABRIC_OP_TIME_SLOT);
    let op_restart = spawn_boot(FABRIC_OP_CLIENT_B_RESTART_SLOT);
    let op_worker = spawn_boot_with(
        FABRIC_OP_WORKER_SLOT,
        &[
            grant(op_client, RIGHT_SUPERVISE),
            grant(op_client_b, RIGHT_SUPERVISE),
            grant(op_server, RIGHT_SUPERVISE),
            grant(op_restart, RIGHT_SUPERVISE),
        ],
    );
    slime_rt::debug_write(b"[init] traffic operation plane spawned\n");
    slime_rt::debug_write(b"[init] traffic graph spawned with static endpoints\n");

    // Neither the proxy nor the observer ever contacts the stream broker under
    // `"traffic"` (`fabric_boot::full_graph_active` parks both without
    // requesting a role -- see `fabric-observer::main` and
    // `fabric-service::traffic_graph` for why the observer cannot join real
    // delivery here the way `boot_graph`'s permanently parked copy safely
    // does), so both are the spawned tasks this plane does not wait to exit --
    // each is expected to still be healthy-idle once everything else has
    // settled.
    //
    // C8.14 inverts that for the proxy alone. Its fault plane is this same
    // composition built with the interposition hop compiled to exit instead of
    // park, because a hop dying is the one degradation no participant can
    // script for itself. So on that build the hop is *waited on* rather than
    // checked idle: an injected departure is a declared clean exit, and
    // requiring it here is what proves the graph observed the hop leave rather
    // than merely tolerating a task it stopped tracking.
    const PROXY_DIES: bool = option_env!("SLIME_FABRIC_PROXY_EARLY_EXIT").is_some();
    wait_clean(&[
        publisher,
        subscriber,
        publisher_b,
        subscriber_b,
        probe,
        fabric,
        call_client,
        call_client_b,
        call_server,
        call_time,
        call_worker,
        op_client,
        op_client_b,
        op_server,
        op_time,
        op_restart,
        op_worker,
    ]);
    if PROXY_DIES {
        wait_clean(&[proxy]);
    } else {
        expect_parked(proxy);
    }
    expect_parked(observer);
    slime_rt::debug_write(b"[init] traffic plane reclaimed\n");
    slime_rt::exit(0)
}

/// Spawn one boot participant that its manifest grants nothing, returning the
/// supervision handle init keeps.
fn spawn_boot(executable_slot: u32) -> u32 {
    spawn_boot_with(executable_slot, &[])
}

/// Spawn one boot participant with the exact grant vector its manifest declares.
///
/// The count must equal what `preflight_spawn_grants` derives from the
/// generation — the child's minted bindings plus its spawn-crossing grant
/// bindings — or the root refuses the spawn with nothing constructed. Both
/// numbers come from the same manifest, so a disagreement is a fixture defect
/// rather than something to reconcile here.
fn spawn_boot_with(executable_slot: u32, grants: &[SpawnGrant]) -> u32 {
    match slime_rt::spawn(executable_slot, grants) {
        Ok(spawned) => spawned.supervision_slot,
        Err(error) => {
            slime_rt::debug_write(b"[init] fabric boot spawn failed slot=");
            write_u32(executable_slot);
            slime_rt::debug_write(b" grants=");
            write_u32(grants.len() as u32);
            slime_rt::debug_write(b" error=");
            write_i64(error);
            slime_rt::debug_write(b"\n");
            fail_boot(b"spawn participant")
        }
    }
}

fn fail_boot(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] fabric boot fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

/// Require one spawned task to still be healthy-idle, never having exited.
///
/// The complement of [`wait_clean`], for a structural role a plane declares but
/// drives no traffic through: its correct outcome is blocked idle, so an exit of
/// any status is the failure.
fn expect_parked(handle: u32) {
    match slime_rt::supervision_status(handle) {
        Ok(None) => {}
        _ => fail_boot(b"parked participant left healthy idle"),
    }
}
fn wait_clean(handles: &[u32]) {
    for handle in handles {
        loop {
            match slime_rt::supervision_status(*handle) {
                Ok(None) => slime_rt::yield_now(),
                Ok(Some(slime_rt::Termination::Exit(0))) => break,
                other => {
                    slime_rt::debug_write(b"[init] unclean handle=");
                    write_u32(*handle);
                    slime_rt::debug_write(b" kind=");
                    write_u32(match other {
                        Ok(Some(slime_rt::Termination::Exit(_))) => 1,
                        Ok(Some(slime_rt::Termination::Fault(_))) => 2,
                        Ok(Some(slime_rt::Termination::Timeout)) => 3,
                        Ok(Some(slime_rt::Termination::PeerLoss)) => 4,
                        Ok(Some(slime_rt::Termination::Unhealthy)) => 5,
                        _ => 9,
                    });
                    slime_rt::debug_write(b"\n");
                    slime_rt::exit(1)
                }
            }
        }
    }
}

fn wait_terminated(handles: &[u32]) {
    for handle in handles {
        loop {
            match slime_rt::supervision_status(*handle) {
                Ok(None) => slime_rt::yield_now(),
                Ok(Some(slime_rt::Termination::Exit(_))) => break,
                _ => slime_rt::exit(1),
            }
        }
    }
}

/// Launch the C8.7 operation plane: one fabric brokering two clients and one
/// server on the declared `navigation` operation route, plus the capability-routed
/// clock that makes expiry and timeout deterministic.
///
/// Init holds no route capability. Every control is a generation-declared native
/// Endpoint whose two halves the root installs into the fabric and the
/// participant that declared them — so "which component is asking" is a
/// capability fact established by the manifest, not a claim in a message. That
/// binding is exactly what makes the milestone's authority denials hold: client
/// B cannot observe, retrieve, or cancel client A's operation even knowing its
/// identity, because its requests arrive on a different endpoint.
///
/// **Spawn order is load-bearing.** The fabric starts before any participant so
/// no goal can arrive before there is a broker to correlate it, and the clock
/// starts last so no time advance precedes the operations it must expire.
fn launch_fabric_operations() {
    // Controls are generation-declared native Endpoints, installed by the
    // root into both halves before anything runs. Init mints nothing.
    slime_rt::debug_write(b"[init] operation control channels minted\n");
    // The participants precede the broker because it is granted a supervision
    // handle naming each of them, and a handle cannot exist before its task. A
    // native Endpoint reports no peer death, so those handles are the only way
    // the broker observes a participant exit rather than blocking on it.
    let client = slime_rt::spawn(FABRIC_OP_CLIENT_SLOT, &[]).unwrap_or_else(|_| slime_rt::exit(1));
    let client_b =
        slime_rt::spawn(FABRIC_OP_CLIENT_B_SLOT, &[]).unwrap_or_else(|_| slime_rt::exit(1));
    let server = slime_rt::spawn(FABRIC_OP_SERVER_SLOT, &[]).unwrap_or_else(|_| slime_rt::exit(1));
    let replacement =
        slime_rt::spawn(FABRIC_OP_CLIENT_B_RESTART_SLOT, &[]).unwrap_or_else(|_| slime_rt::exit(1));
    slime_rt::debug_write(b"[init] operation participants spawned\n");
    slime_rt::debug_write(b"[init] operation replacement introduced\n");
    // What init still passes is exactly what the generation cannot place: the
    // shared-buffer factory it holds, and one supervision handle per
    // participant, which only exist once those tasks do. Matching is positional
    // against ascending declared slot: factory at 1, then the handles.
    let service = slime_rt::spawn(
        FABRIC_SERVICE_SLOT,
        &[
            grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
            grant(client.supervision_slot, RIGHT_SUPERVISE),
            grant(client_b.supervision_slot, RIGHT_SUPERVISE),
            grant(server.supervision_slot, RIGHT_SUPERVISE),
            grant(replacement.supervision_slot, RIGHT_SUPERVISE),
        ],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    slime_rt::debug_write(b"[init] operation fabric spawned\n");
    slime_rt::debug_write(b"[init] operation supervision delegated\n");
    let time = slime_rt::spawn(FABRIC_OP_TIME_SLOT, &[]).unwrap_or_else(|_| slime_rt::exit(1));
    slime_rt::debug_write(b"[init] operation replacement released\n");
    wait_clean(&[
        client.supervision_slot,
        client_b.supervision_slot,
        server.supervision_slot,
        replacement.supervision_slot,
        time.supervision_slot,
        service.supervision_slot,
    ]);
}

fn launch_fabric_calls() {
    // Every control endpoint is a generation-declared native Endpoint: the
    // root materializes both halves from the manifest's grants and installs
    // each side into the instance that declared it, before any of them runs.
    // Init therefore holds no route capability and mints nothing.
    slime_rt::debug_write(b"[init] call control channels minted\n");
    // The participants precede the fabric because the broker is granted a
    // supervision handle naming each of them, and a handle cannot exist before
    // its task. A native Endpoint reports no peer death, so those handles are
    // the only way the broker observes a participant exit rather than blocking
    // on it forever.
    let client = slime_rt::spawn(
        FABRIC_CALL_CLIENT_SLOT,
        &[grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE)],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    let client_b =
        slime_rt::spawn(FABRIC_CALL_CLIENT_B_SLOT, &[]).unwrap_or_else(|_| slime_rt::exit(1));
    let server = slime_rt::spawn(
        FABRIC_CALL_SERVER_SLOT,
        &[grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE)],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    slime_rt::debug_write(b"[init] call participants spawned\n");
    // What init still passes is exactly what the generation cannot place: the
    // shared-buffer factory it holds, and one supervision handle per
    // participant, which only exist once those tasks do. Matching is positional
    // against ascending declared slot: factory at 1, then the handles.
    let service = slime_rt::spawn(
        FABRIC_SERVICE_SLOT,
        &[
            grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
            grant(client.supervision_slot, RIGHT_SUPERVISE),
            grant(client_b.supervision_slot, RIGHT_SUPERVISE),
            grant(server.supervision_slot, RIGHT_SUPERVISE),
        ],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    slime_rt::debug_write(b"[init] call fabric spawned\n");
    slime_rt::debug_write(b"[init] call supervision delegated\n");
    let time = slime_rt::spawn(FABRIC_CALL_TIME_SLOT, &[]).unwrap_or_else(|_| slime_rt::exit(1));
    wait_clean(&[
        client.supervision_slot,
        client_b.supervision_slot,
        server.supervision_slot,
        time.supervision_slot,
        service.supervision_slot,
    ]);
}

/// Prove native endpoint rendezvous and unrelated progress while the sender is
/// blocked in the kernel rather than filling a root-mediated queue.
fn drive_channel_plane() {
    const LINE: &[u8] = b"[console] channel plane carried this line\n";
    const CLOSE: &[u8] = b"SLIME.CONSOLE.CLOSE";
    let console = slime_rt::spawn(CONSOLE_SLOT, &[]).unwrap_or_else(|_| fail(b"spawn console"));
    for _ in 0..PEER_PARK_YIELDS {
        slime_rt::yield_now();
    }
    slime_rt::debug_write(b"[init] rendezvous send entering\n");
    if slime_rt::send(console_send_slot(), LINE, &[]) != slime_rt::ERR_SUCCESS {
        fail(b"native rendezvous send");
    }
    slime_rt::debug_write(b"[init] rendezvous send completed\n");
    if slime_rt::send(console_send_slot(), CLOSE, &[]) != slime_rt::ERR_SUCCESS {
        fail(b"console close");
    }
    wait_clean(&[console.supervision_slot]);
    slime_rt::debug_write(b"[init] channel receiver completed\n");
}

/// Drive the P5.3.2 loan plane, as the lender.
///
/// Only reachable for the authenticated `loan` action declared by
/// `contracts/generation/v1/fixtures/sel4-loan.zti`; see the `.md` beside it.
///
/// This is `sample-lender`'s shape, and deliberately not `sample-lender`
/// itself: that component is spawned by init on x86 and receives its peer
/// through a spawn grant. Init stands in as the lender so the *loan* plane can
/// be exercised without depending on the *spawn* plane's composition. The receiver is the real `sample-receiver`, unmodified —
/// which is the point: a component written against the retired kernel's loan
/// ABI runs unchanged on seL4.
fn drive_loan_plane() {
    const PAGE: u64 = 4096;
    const PAGES: usize = 2;
    const PAYLOAD_LEN: u64 = PAGES as u64 * PAGE;
    const BASE: u64 = 0x0000_0009_0000_0000;
    // The whole point of a loan: a payload the control message cannot carry.
    const _: () = assert!(PAYLOAD_LEN > slime_rt::MAX_MSG as u64);

    // ---- B13: the factory grant, independent of the budget ----
    //
    // The generation declares init a budget *and* a `bufferCreate` grant, and
    // the two are independent gates: the grant authorizes the operation, the
    // budget bounds it. Naming a slot that holds no factory must therefore be
    // refused however much quota the holder has left — which is the whole
    // ceiling here, since this runs first.
    //
    // `MAX_CAPS - 1` is inside the table and init was granted nothing there.
    if slime_rt::shared_buffer_create(63, 1, true).is_ok() {
        fail_loan(b"an empty slot named a buffer factory");
    }
    // A slot holding real authority of another kind, so the check is on kind
    // rather than on possession.
    if slime_rt::shared_buffer_create(RECEIVER_SLOT, 1, true).is_ok() {
        fail_loan(b"a channel slot named a buffer factory");
    }
    slime_rt::debug_write(b"[init] ungranted buffer factory refused\n");

    // ---- the four quota ceilings, each at ceiling + 1 ----
    //
    // Run before the loan, because a refusal must be a refusal against an
    // ungrazed ceiling rather than against whatever the loan happened to leave.
    // The generation declares init 4 pages / 2 buffers / 2 mappings / 1 loan;
    // every probe below asks for exactly one more than one of those.
    probe_quota_ceilings(BASE);

    let buffer = match slime_rt::shared_buffer_create(SHARED_BUFFER_FACTORY_SLOT, PAGES, true) {
        Ok(buffer) => buffer,
        Err(_) => fail_loan(b"create"),
    };
    if slime_rt::shared_buffer_map(buffer.slot, BASE, 0, PAYLOAD_LEN, true) != slime_rt::ERR_SUCCESS
    {
        fail_loan(b"writable map");
    }
    // SAFETY: the root installed a writable mapping of exactly `PAYLOAD_LEN`
    // bytes at `BASE`, and it stays mapped until the unmap below.
    unsafe {
        let bytes = BASE as *mut u8;
        for index in 0..PAYLOAD_LEN as usize {
            bytes.add(index).write_volatile((index % 251) as u8);
        }
    }
    slime_rt::debug_write(b"[init] payload written\n");

    // The receiver has to be running before it can be loaned to: a loan names
    // its receiver as the unique live holder of the channel's other end, so
    // with nothing spawned the root answers `absent-or-ambiguous` (B52) --
    // including for the unsealed probe below, which would otherwise be
    // refused for the wrong reason and pass vacuously.
    //
    // One grant: the receiver's own end of the channel init keeps the other
    // half of. That edge is generation-declared, so the preflight expects
    // exactly it -- which is what the docstring above says this cutover lacked
    // "until P5.3.3", and now has.
    if slime_rt::spawn(SAMPLE_RECEIVER_SLOT, &[]).is_err() {
        fail_loan(b"spawn the receiver");
    }
    slime_rt::debug_write(b"[init] loan receiver spawned\n");

    // A loan requires an irreversibly sealed source, so an unsealed one must be
    // refused. Checked before sealing, because afterwards it is unobservable.
    if slime_rt::shared_buffer_loan(buffer.slot, RECEIVER_SLOT, 0, PAYLOAD_LEN, false).is_ok() {
        fail_loan(b"unsealed region was loanable");
    }
    slime_rt::debug_write(b"[init] unsealed loan denied\n");

    if slime_rt::shared_buffer_seal(buffer.slot) != slime_rt::ERR_SUCCESS {
        fail_loan(b"seal");
    }

    // How the receiver is named is the exit condition's own words — "a receiver
    // named by capability" — so the ways of naming one badly are checked before
    // the way that works. Each must be refused, and each for its own reason.
    //
    // A slot holding nothing. `MAX_CAPS - 1` is inside the table's bounds and
    // this component was granted nothing there, so this is the empty-slot case
    // rather than an out-of-range one.
    if slime_rt::shared_buffer_loan(buffer.slot, 63, 0, PAYLOAD_LEN, false).is_ok() {
        fail_loan(b"an empty slot named a receiver");
    }
    // A slot holding the wrong *kind*. The buffer's own slot is real authority
    // this component holds — it is the source of the loan — and it still names
    // no receiver, so the check is on kind rather than on possession.
    if slime_rt::shared_buffer_loan(buffer.slot, buffer.slot, 0, PAYLOAD_LEN, false).is_ok() {
        fail_loan(b"a buffer slot named a receiver");
    }
    slime_rt::debug_write(b"[init] unnamed receiver denied\n");

    // A real channel to a real peer, over an edge the generation declared
    // `transferable = false`. Everything else about this loan would succeed —
    // the source is sealed, the receiver is a live task at the other end of a
    // channel this component holds — so the only thing refusing it is the
    // generation's delegation bit, which is what makes that bit load-bearing
    // rather than decorative.
    if slime_rt::shared_buffer_loan(buffer.slot, console_send_slot(), 0, PAYLOAD_LEN, false).is_ok()
    {
        fail_loan(b"an undelegated channel carried a loan");
    }
    slime_rt::debug_write(b"[init] undelegated loan denied\n");

    let loan = match slime_rt::shared_buffer_loan(buffer.slot, RECEIVER_SLOT, 0, PAYLOAD_LEN, false)
    {
        Ok(loan) => loan,
        Err(_) => fail_loan(b"loan"),
    };
    slime_rt::debug_write(b"[init] loan created\n");

    // The loan ceiling is one. A second loan of the same sealed region is
    // therefore refused by the quota rather than by anything about the range.
    if slime_rt::shared_buffer_loan(buffer.slot, RECEIVER_SLOT, 0, PAGE, false).is_ok() {
        fail_loan(b"loan quota did not bite");
    }
    slime_rt::debug_write(b"[init] loan quota refused\n");

    // Only the descriptor crosses the channel; the payload never enters a
    // queue. The loan capability rides with it, which is the transfer this
    // slice adds — and it is the loan, not the buffer, that moves: the receiver
    // gets a read-only window onto an exact subrange, not the region.
    let descriptor = sample_descriptor(loan.id, PAYLOAD_LEN);
    if slime_rt::capability_delegate(
        RECEIVER_SLOT,
        loan.slot,
        CapabilityDisposition::Move,
        slime_proto::capability_transfer::OBJECT_KIND_SHARED_BUFFER_LOAN,
        1 << 9,
        &descriptor,
    ) != slime_rt::ERR_SUCCESS
    {
        fail_loan(b"send descriptor");
    }
    slime_rt::debug_write(b"[init] loan transferred\n");

    // The capability moved, so this component can no longer name it. Naming it
    // again must be refused: a transfer that left the sender holding the
    // capability would be a copy, not a move.
    if slime_rt::shared_buffer_return(loan.slot) == slime_rt::ERR_SUCCESS {
        fail_loan(b"transferred loan still nameable");
    }
    slime_rt::debug_write(b"[init] transferred loan released by sender\n");

    // Wait for the receiver to settle before reclaiming. Not politeness: this
    // component's own termination would settle every loan it owns, so exiting
    // early would reclaim the region out from under a receiver that has not
    // mapped it yet. That retention is the C7.5 property under test.
    let mut done = [0u8; slime_rt::MAX_MSG];
    let mut no_caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(RECEIVER_SLOT, &mut done, &mut no_caps) {
            slime_rt::ERR_WOULDBLOCK => {
                slime_rt::yield_now();
            }
            n if n < 0 => fail_loan(b"await receiver"),
            _ => break,
        }
    }
    slime_rt::debug_write(b"[init] receiver settled\n");

    // With the loan returned, the creator may reclaim.
    if slime_rt::shared_buffer_unmap(buffer.slot, BASE) != slime_rt::ERR_SUCCESS {
        fail_loan(b"unmap");
    }
    if slime_rt::shared_buffer_release(buffer.slot) != slime_rt::ERR_SUCCESS {
        fail_loan(b"release");
    }
    if slime_rt::shared_buffer_release(buffer.slot) != slime_rt::ERR_BAD_CAP {
        fail_loan(b"released buffer still nameable");
    }
    slime_rt::debug_write(b"[init] released\n");

    // Let `console` — the third holder, which took no part in any of the above
    // — prove its own quota is intact. This is the "without disturbing an
    // unrelated holder" half: init exhausted all four of its own ceilings, and
    // console's are untouched.
    if slime_rt::send(
        console_send_slot(),
        b"[console] unrelated holder intact\n",
        &[],
    ) != slime_rt::ERR_SUCCESS
    {
        fail_loan(b"notify unrelated holder");
    }
    for _ in 0..PEER_PARK_YIELDS {
        slime_rt::yield_now();
    }
    // Leave one finalized logical export unclaimed. The root must reclaim it
    // when the graph drains; otherwise the terminal capability summary remains
    // nonzero. Retain the source so init's normal task cleanup independently
    // reclaims the buffer itself.
    let abandoned = slime_rt::shared_buffer_create(SHARED_BUFFER_FACTORY_SLOT, 1, true)
        .unwrap_or_else(|_| fail_loan(b"create abandoned export source"));
    if slime_rt::capability_delegate(
        DANGO_OUTPUT_SLOT,
        abandoned.slot,
        CapabilityDisposition::Retain,
        slime_proto::capability_transfer::OBJECT_KIND_SHARED_BUFFER,
        RIGHT_TRANSFER,
        &[b'x'; 64],
    ) != slime_rt::ERR_SUCCESS
    {
        fail_loan(b"leave export ticket");
    }
    if slime_rt::send(console_send_slot(), b"SLIME.CONSOLE.CLOSE", &[]) != slime_rt::ERR_SUCCESS {
        fail_loan(b"close console");
    }
    slime_rt::debug_write(b"[init] export ticket left for reclamation\n");
}

/// Ask for exactly one more than each declared ceiling, and require a refusal.
///
/// Each probe is a single operation past one ceiling with the other three
/// unspent, so a refusal names the class it was aimed at rather than whichever
/// limit happened to be reached first. The root prints the class it refused on,
/// which is what the gate asserts — the wire status collapses all four to
/// `ERR_OUT_OF_MEMORY` by design.
fn probe_quota_ceilings(base: u64) {
    const PAGE: u64 = 4096;
    // Pages: the ceiling is 4, so a single 5-page region can never fit.
    if slime_rt::shared_buffer_create(SHARED_BUFFER_FACTORY_SLOT, 5, true).is_ok() {
        fail_loan(b"page quota did not bite");
    }
    slime_rt::debug_write(b"[init] page quota refused\n");

    // Buffers: the ceiling is 2. Three single-page regions exceed it while
    // staying inside the 4-page budget, so it is the buffer count that refuses.
    let first = match slime_rt::shared_buffer_create(SHARED_BUFFER_FACTORY_SLOT, 1, true) {
        Ok(buffer) => buffer,
        Err(_) => fail_loan(b"first probe region"),
    };
    let second = match slime_rt::shared_buffer_create(SHARED_BUFFER_FACTORY_SLOT, 1, true) {
        Ok(buffer) => buffer,
        Err(_) => fail_loan(b"second probe region"),
    };
    if slime_rt::shared_buffer_create(SHARED_BUFFER_FACTORY_SLOT, 1, true).is_ok() {
        fail_loan(b"buffer quota did not bite");
    }
    slime_rt::debug_write(b"[init] buffer quota refused\n");

    // Mappings: the ceiling is 2. Two land, the third is refused — and it is a
    // mapping of a region already charged, so no page or buffer limit is
    // involved.
    for (index, buffer) in [first, second].into_iter().enumerate() {
        if slime_rt::shared_buffer_map(buffer.slot, base + index as u64 * PAGE, 0, PAGE, true)
            != slime_rt::ERR_SUCCESS
        {
            fail_loan(b"probe mapping");
        }
    }
    if slime_rt::shared_buffer_map(first.slot, base + 2 * PAGE, 0, PAGE, true)
        == slime_rt::ERR_SUCCESS
    {
        fail_loan(b"mapping quota did not bite");
    }
    slime_rt::debug_write(b"[init] mapping quota refused\n");

    // Hand every probe resource back, so the loan below runs against ceilings
    // that are entirely unspent. A probe that left a charge behind would make
    // the loan's own refusals ambiguous.
    for (index, buffer) in [first, second].into_iter().enumerate() {
        if slime_rt::shared_buffer_unmap(buffer.slot, base + index as u64 * PAGE)
            != slime_rt::ERR_SUCCESS
        {
            fail_loan(b"probe unmap");
        }
        if slime_rt::shared_buffer_release(buffer.slot) != slime_rt::ERR_SUCCESS {
            fail_loan(b"probe release");
        }
    }
    slime_rt::debug_write(b"[init] quota probes reclaimed\n");
}

/// The 64-byte sample descriptor naming this loan, in the wire form
/// `sample-receiver` validates.
fn sample_descriptor(loan_id: u64, length: u64) -> [u8; slime_rt::MAX_MSG] {
    slime_proto::sample_descriptor::WireSampleDescriptor {
        magic: slime_proto::sample_descriptor::SAMPLE_DESCRIPTOR_MAGIC,
        version: slime_proto::sample_descriptor::FORMAT_VERSION,
        flags: slime_proto::sample_descriptor::FLAG_LAST,
        capability_kind: slime_proto::sample_descriptor::CAPABILITY_KIND_LOAN,
        loan_id,
        offset: 0,
        length,
        type_identity: slime_proto::interface_schema::telemetry_stream::TYPE_TAG,
        sequence: 1,
        reserved: [0; 8],
    }
    .encode()
}

fn fail_loan(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] loan plane fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

/// The channel init uses for console output in the active generation.
///
/// Product generations name it `console-output`; the standalone channel and
/// loan planes retain the older `dango-output` edge. Both values come from the
/// manifest-derived boot layout, so choosing by authenticated boot action
/// preserves one binary across the graphs without a build flag.
fn console_send_slot() -> u32 {
    if matches!(profile::GENERATION_BOOT_ACTION, "channel" | "loan") {
        DANGO_OUTPUT_SLOT
    } else {
        CONSOLE_OUTPUT_SLOT
    }
}

/// Yields given up so a peer can reach its first `recv` and park. Generous
/// against the two operations `console` issues before blocking — a transfer
/// window bind and the receive itself — while still bounding the wait.
const PEER_PARK_YIELDS: usize = 64;

/// The channel to `sample-receiver`, which is also how the loan names its
/// receiver.
///
/// One slot for both because the root resolves the loan's receiver as the task
/// at the other end of this channel — see
/// `slime-root/src/main.rs::serve_buffer_loan` for why that stands in for the
/// supervision handle the retired kernel uses, and what replaces it in P5.3.3.
const RECEIVER_SLOT: u32 = SAMPLE_RECEIVER_SIDE_SLOT;

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] channel plane fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

/// Run the sample composition over generation-declared endpoint and factory
/// bindings, with one supervision handle handed over at spawn.
///
/// The split is the same one `launch_fabric_calls` makes: the channel and the
/// lender's buffer factory are edges the generation fixes before either task
/// runs, so they are ordinary grants; the receiver's supervision handle cannot
/// exist until the receiver does, so it is the one capability init still passes.
/// That also fixes the spawn order — the receiver first, because a handle
/// naming it cannot precede it.
fn drive_sample_plane() {
    let receiver = slime_rt::spawn(SAMPLE_RECEIVER_SLOT, &[])
        .unwrap_or_else(|_| fail_sample(b"spawn receiver"));
    // Matched positionally against ascending declared slot, exactly as
    // `launch_fabric_calls` matches: the lender's factory at 1, then the
    // receiver's supervision handle at 2. The channel is a declared endpoint the
    // root installs on both sides, so it is not in this list.
    let lender = slime_rt::spawn(
        SAMPLE_LENDER_SLOT,
        &[
            grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
            grant(receiver.supervision_slot, RIGHT_SUPERVISE),
        ],
    )
    .unwrap_or_else(|_| fail_sample(b"spawn lender"));
    if slime_rt::spawn(SAMPLE_RECEIVER_SLOT, &[]) != Err(slime_rt::ERR_BAD_CAP) {
        fail_sample(b"a live instance was spawned twice");
    }
    slime_rt::debug_write(b"[init] spawn budget refused\n");
    for handle in [receiver.supervision_slot, lender.supervision_slot] {
        loop {
            match slime_rt::supervision_status(handle) {
                Ok(None) => slime_rt::yield_now(),
                Ok(Some(slime_rt::Termination::Exit(0))) => break,
                _ => fail_sample(b"a sample component did not exit cleanly"),
            }
        }
    }
    let reaped = slime_rt::spawn(SAMPLE_RECEIVER_SLOT, &[])
        .unwrap_or_else(|_| fail_sample(b"budget did not recover after a child exited"));
    slime_rt::debug_write(b"[init] spawn budget recovered\n");
    if slime_rt::cap_drop(reaped.supervision_slot) != slime_rt::ERR_SUCCESS {
        fail_sample(b"dropping the reaped handle");
    }
}

fn fail_sample(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] sample plane fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

/// Drive the P5.5.2 stream plane: the full C8.4 graph the x86 oracle builds —
/// two publishers, two subscribers, two routes, the `>MAX_INLINE_BYTES`
/// descriptor and loan path, and KEEP_LAST eviction under a stalled subscriber.
///
/// Only reachable for the authenticated `stream` or `qos` action declared by
/// the corresponding seL4 generation manifest.
///
/// This is `launch_fabric_graph`'s shape and its spawn order, on the same
/// authority argument. Init holds **no route capability at all**: it mints one
/// control channel per participant, and the binding between a control endpoint
/// and a component identity is established here, at spawn. That is what the
/// fabric authenticates against — a client cannot forge, share, or re-derive
/// one, so "which component is asking" is a capability fact rather than a claim
/// in a message.
///
/// **Spawn order is load-bearing**, exactly as it is on x86. Both subscribers
/// start before the fabric, because a downstream loan names its receiver
/// through a `RIGHT_SUPERVISE` capability rather than an ambient task id, so
/// those handles must exist before the service does. The publishers follow the
/// fabric, so no sample arrives before there is a broker for it.
///
/// `fabric-intruder` is spawned holding a real control endpoint on purpose.
/// The denial under test is not "no channel" but "no declared edge".
/// `qos` records which plane booted; both compose identically here, because the
/// QoS graph's extra edge — the capability-routed clock between
/// `fabric-publisher-b` and the broker — is a declared grant the root installs
/// rather than anything init places. The distinction is the generation's own
/// `bootAction`, delivered at activation, so the QoS generation needs no paired
/// build flag: the service and participants select behavior from the same
/// generated boot action.
fn drive_stream_plane(_qos: bool) {
    launch_fabric_graph(b"fabric", b" service spawned\n");
}

/// Drive the P5.4.6 call plane: the C8.6 bounded-native-call graph the x86
/// oracle builds — two clients, a server, and a capability-routed clock over
/// one `ParameterCall` route.
///
/// Only reachable for the authenticated `call` action declared by
/// `contracts/generation/v1/fixtures/sel4-call.zti`.
///
/// **Why the generation declares these controls rather than init minting them.**
/// An earlier version had init mint all five and hand out the halves at spawn,
/// because a root-materialized grant landed at the fabric's own channel cursor —
/// which resumed *above* the factory grants staging installed, so the fabric
/// received `[0, 3, 4, 5, 6]` and no `base + index` describes a set with a hole
/// in it. Declared native Endpoints removed both that hole and the ordering
/// hazard behind it (`build_generation` sorts grants by `(name, source, target)`,
/// so `fabric-call-client-b-control` sorted ahead of
/// `fabric-call-client-control` and would have bound client B's identity to
/// client A's slot): each side is installed at the slot its own binding names,
/// so the fabric's controls are contiguous from `FABRIC_FIRST_CONTROL_SLOT` in
/// grant order, which is what the broker compiles against.
///
/// **Spawn order matches the x86 oracle.** A spawn grant is a non-consuming
/// copy. Init spawns each participant, then the fabric, and moves each
/// supervision handle to the broker — the one authority the generation cannot
/// place, because it does not exist until the task does. The broker still
/// receives the request first and the matching identity second; no participant
/// needs authority naming itself.
fn drive_call_plane() {
    launch_fabric_calls();
}

/// Drive the P5.4.7 operation plane: the C8.7 bounded-native-operation graph
/// the x86 oracle builds — two clients, a supervised replacement for the
/// second, a server, and a capability-routed clock over the `navigation` route
/// plus client A's private `nav-backup` route.
///
/// Only reachable for the authenticated `operation` action declared by
/// `contracts/generation/v1/fixtures/sel4-operation.zti`; broker and participants
/// read the same generated profile while only this composition differs.
///
/// **Why the generation declares the control channels.** `drive_call_plane`'s
/// reason, which applies unchanged: each control is a native Endpoint the root
/// installs into both declaring instances at the slot each binding names, so the
/// broker's controls are contiguous from `base` in grant order. The grant names
/// are also what `_control_sources` derives `FABRIC_OPERATION_CLIENTS` from —
/// the table the broker maps a control slot to a caller identity with.
///
/// **The replacement is the restart arm.** C8.7 requires a participant restart
/// to be deterministic: the broker keeps client B's authenticated index, its
/// correlation high-water mark, and its retained results, while the replacement
/// receives a *fresh* non-delegable role. Init spawns the replacement on its own
/// authenticated control and vouches for it exactly as for the others, then
/// releases it through a private barrier so its role request cannot reach the
/// broker before the original client has produced the retained result the
/// replacement is supposed to find.
fn drive_operation_plane() {
    launch_fabric_operations();
}

/// Drive the P5.4.8 visibility plane: the C8.8 filtered-introspection and
/// declared-interposition graph the x86 oracle builds — the telemetry and
/// diagnostics routes with `fabric-intruder` as the *declared proxy* on the
/// telemetry subscriber's chain.
///
/// Only reachable for the authenticated `visibility` action declared by
/// `contracts/generation/v1/fixtures/sel4-visibility.zti`; broker and participants
/// read the same generated profile while only this composition differs.
///
/// This is `drive_stream_plane`'s shape with two differences, both consequences
/// of what the visibility broker does rather than choices made here:
///
/// * **No supervision handles.** The stream fabric is granted one per subscriber
///   so it can name a loan receiver. The visibility broker mints every route
///   half itself and hands out narrowed, non-delegable roles, so it needs no
///   handle and init keeps all six.
/// * **No spawn ordering constraint.** Nothing in this plane requires a task to
///   exist before another's grants are built, because no grant names a task.
///   Participants are spawned in control-slot order purely so the transcript
///   reads in the order the broker will answer them.
fn drive_visibility_plane() {
    launch_fabric_graph(b"visibility", b" fabric spawned\n");
}

/// Drive the C8.12 matrix plane: matching, filtered visibility, and denial
/// exercised together against one graph.
///
/// Only reachable for the authenticated `matrix` action declared by
/// `contracts/generation/v1/fixtures/sel4-matrix.zti`.
///
/// The composition is `launch_fabric_graph`'s, with three differences the
/// milestone forces rather than choices made here:
///
/// * **Seven participants, not five.** The ungranted probe, the declared
///   interposition proxy, and the read-only observer are three distinct task
///   identities with non-overlapping grants, so a denial can never be confused
///   for a role the graph granted somewhere else. `fabric-intruder`, which
///   carried all three roles at once behind an env switch, is absent.
/// * **The probe holds a real control endpoint.** The denial under test is "no
///   declared edge", not "no channel": a component refused for lack of a
///   channel would prove nothing about the graph.
/// * **One supervision handle per ring participant and the proxy.** The broker
///   needs the proxy's to observe a hop through a dead one — a native Endpoint
///   reports no peer death — and each ring holder's to name a loan receiver.
///   The probe gets none: it asks for nothing the broker must reclaim.
fn drive_matrix_plane() {
    plane_marker(b"matrix", b" control channels minted\n");
    let publisher = spawn_boot(FABRIC_PUBLISHER_SLOT);
    let subscriber = spawn_boot(FABRIC_SUBSCRIBER_SLOT);
    let publisher_b = spawn_boot_with(
        FABRIC_PUBLISHER_B_SLOT,
        &[grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE)],
    );
    let subscriber_b = spawn_boot(FABRIC_SUBSCRIBER_B_SLOT);
    let observer = spawn_boot(FABRIC_OBSERVER_SLOT);
    let proxy = spawn_boot(FABRIC_PROXY_SLOT);
    let probe = spawn_boot(FABRIC_PROBE_SLOT);
    plane_marker(b"matrix", b" participants spawned\n");

    // Positional against the child's ascending declared slot, exactly as every
    // other fabric plane: the factory first, then one supervision handle per
    // holder in the order `FABRIC_SUPERVISION` lists them. `FABRIC_MINTED_GRANTS`
    // states how many the generation expects, so a composition that drifted from
    // the fixture is refused at spawn rather than mis-bound.
    //
    // The probe's handle is in the vector even though it holds no edge. The
    // broker's dispatch loop needs it to know the refused caller has stopped
    // asking — a native Endpoint reports no peer death — and it grants the probe
    // nothing: the fabric holds the handle, not the probe.
    let grants = [
        grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
        grant(publisher, RIGHT_SUPERVISE),
        grant(subscriber, RIGHT_SUPERVISE),
        grant(publisher_b, RIGHT_SUPERVISE),
        grant(subscriber_b, RIGHT_SUPERVISE),
        grant(observer, RIGHT_SUPERVISE),
        grant(probe, RIGHT_SUPERVISE),
        grant(proxy, RIGHT_SUPERVISE),
    ];
    let declared = declared_minted_grants(b"fabric-service");
    if declared != grants.len() {
        slime_rt::debug_write(b"[init] matrix plane fail: fabric-service grant count\n");
        slime_rt::exit(1);
    }
    let service = spawn_boot_with(FABRIC_SERVICE_SLOT, &grants);
    plane_marker(b"matrix", b" fabric spawned\n");

    wait_clean(&[
        publisher,
        subscriber,
        publisher_b,
        subscriber_b,
        observer,
        proxy,
        probe,
        service,
    ]);
}

/// Drive the P5.4.3 powerbox plane (M6.6): a chooser holding directory
/// authority the requester lacks, handing over one narrowed view on selection.
///
/// The probe's single grant is the RPC endpoint. It holds no directory
/// capability at all, which is the milestone's point: the only way it can name
/// an object is for the chooser to mint one and transfer it, and the chooser
/// mints only what the user's selection gesture named.
fn drive_powerbox_plane() {
    let chooser = slime_rt::spawn(POWERBOX_CHOOSER_SLOT, &[])
        .unwrap_or_else(|_| fail_plane(b"powerbox", b"spawn chooser"));
    slime_rt::debug_write(b"[init] powerbox chooser spawned\n");
    let probe = slime_rt::spawn(POWERBOX_PROBE_SLOT, &[])
        .unwrap_or_else(|_| fail_plane(b"powerbox", b"spawn probe"));
    slime_rt::debug_write(b"[init] powerbox probe spawned\n");
    wait_clean(&[probe.supervision_slot, chooser.supervision_slot]);
}

/// Drive the P5.4.3 dango plane (M6.4): a scripted console session that
/// launches commands through the spawn service.
///
/// Four components and two channels. The grant lists are the components' own
/// slot layouts — `spawn-service.rs` and `dango.rs` compile against fixed
/// positions, and the *order of these lists* is what fixes them, exactly as
/// `drive_sample_plane` fixes the lender's three.
fn drive_dango_plane() {
    // Console's shared-buffer factory is a declared init-to-console grant, so
    // init holds the source and must hand it over at spawn. Every endpoint on
    // this plane is a generation-declared object the root installs into both
    // ends itself, so no endpoint half crosses here.
    let console = slime_rt::spawn(
        CONSOLE_SLOT,
        &[grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE)],
    )
    .unwrap_or_else(|_| fail_plane(b"dango", b"spawn console"));
    slime_rt::debug_write(b"[init] console spawned\n");
    // The spawn service receives its factory and the two executables it may
    // launch. Both executable grants are sourced by init, so init is the party
    // that must pass them; ascending declared slot is the order the root pairs
    // requests with declarations in.
    let service = slime_rt::spawn(
        SPAWN_SERVICE_SLOT,
        &[
            grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
            grant(6, RIGHT_EXEC | RIGHT_SPAWN),
            grant(7, RIGHT_EXEC | RIGHT_SPAWN),
        ],
    )
    .unwrap_or_else(|_| fail_plane(b"dango", b"spawn service"));
    slime_rt::debug_write(b"[init] spawn service spawned\n");
    let dango =
        slime_rt::spawn(DANGO_SLOT, &[]).unwrap_or_else(|_| fail_plane(b"dango", b"spawn dango"));
    slime_rt::debug_write(b"[init] dango spawned\n");
    wait_terminated(&[
        dango.supervision_slot,
        service.supervision_slot,
        console.supervision_slot,
    ]);
}

/// Drive the P5.4.3 filesystem plane (M6.3's other half): a service that
/// resolves names in a snapshot tree, and a client that must ask it.
///
/// The same shape as the generation plane — mint one channel, spawn the service
/// first so it is listening, then the client — and for the same reason: the
/// authority each holds is placed by the generation, and init composes only the
/// channel between them.
fn drive_filesystem_plane() {
    let service = slime_rt::spawn(SEL4_FILESYSTEM_SERVICE_SLOT, &[])
        .unwrap_or_else(|_| fail_plane(b"filesystem", b"spawn service"));
    slime_rt::debug_write(b"[init] filesystem service spawned\n");
    // The service announces its store is open on a declared edge, and the
    // client is not spawned until it does. Opening the store is hundreds of
    // block round trips; a client that sent its first request into that window
    // got no reply and failed its own arm.
    let mut ready = [0u8; slime_rt::MAX_MSG];
    let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    if slime_rt::recv_blocking(3, &mut ready, &mut caps) < 0 {
        fail_plane(b"filesystem", b"await service readiness");
    }
    let client = slime_rt::spawn(DIRECTORY_PROBE_SLOT, &[])
        .unwrap_or_else(|_| fail_plane(b"filesystem", b"spawn client"));
    slime_rt::debug_write(b"[init] filesystem client spawned\n");
    // The client's exit is init's to observe, through the handle its spawn
    // returned. The service cannot: a native Endpoint reports no peer death, so
    // init closes it on the same declared edge the readiness announcement came
    // over.
    wait_clean(&[client.supervision_slot]);
    if slime_rt::send(3, b"SLIME.FILESYSTEM.CLOSE", &[]) != slime_rt::ERR_SUCCESS {
        fail_plane(b"filesystem", b"close the service");
    }
    wait_clean(&[service.supervision_slot]);
}

/// Drive the P5.4.3 generation plane (M6.5): a management service holding the
/// only block capability, and a client that must ask it.
///
/// Two components and one channel, so unlike the storage planes init composes
/// rather than merely spawns. What it does *not* do is hand the client any
/// device authority — that is the plane's whole claim, and init could not do it
/// anyway: the block capability is granted to the manager by the generation, so
/// init never holds it.
fn drive_generation_plane() {
    // The client precedes the manager: the manager is granted a supervision
    // handle naming it, and a handle cannot exist before its task. A native
    // Endpoint reports no peer death, so that handle is the only way the
    // manager can learn its client is gone rather than merely quiet.
    let client = slime_rt::spawn(SEL4_GENERATION_CLIENT_SLOT, &[])
        .unwrap_or_else(|_| fail_plane(b"generation", b"spawn client"));
    slime_rt::debug_write(b"[init] generation client spawned\n");
    let manager = slime_rt::spawn(
        SEL4_GENERATION_MANAGER_SLOT,
        &[grant(client.supervision_slot, RIGHT_SUPERVISE)],
    )
    .unwrap_or_else(|_| fail_plane(b"generation", b"spawn manager"));
    slime_rt::debug_write(b"[init] generation manager spawned\n");
    // The run token, on init's own end of each declared edge. Both instances of
    // each executable hold a real endpoint at that slot -- the idle ones a
    // loopback nobody sends on -- so arrival is what tells them apart. The root
    // delivers a nonzero boot action only to the bootstrap instance, so
    // `startup_arg` cannot.
    if slime_rt::send(3, b"run", &[]) != slime_rt::ERR_SUCCESS {
        fail_plane(b"generation", b"deliver the manager run token");
    }
    if slime_rt::send(4, b"run", &[]) != slime_rt::ERR_SUCCESS {
        fail_plane(b"generation", b"deliver the client run token");
    }
    wait_clean(&[client.supervision_slot, manager.supervision_slot]);
}

/// Drive the P5.4.2c store plane: the same composition as the storage plane,
/// over the probe that runs M5.4 policy in userspace.
///
/// Separate generation and separate probe, one driver: what differs between the
/// two planes is which component is spawned and what it proves, not how init
/// composes it.
fn drive_store_plane() {
    drive_probe_plane_with_token(
        SEL4_STORE_PROBE_SLOT,
        b"[init] store probe spawned\n",
        b"store",
        Some(2),
    );
}

/// Drive the P5.4.2c storage plane: spawn the probe holding its block
/// capability and require a clean exit.
fn drive_storage_plane() {
    drive_probe_plane_with_token(
        SEL4_STORAGE_PROBE_SLOT,
        b"[init] storage probe spawned\n",
        b"storage",
        Some(2),
    );
}

/// Spawn one probe holding its generation-granted device capability and require
/// a clean exit.
///
/// The composition is deliberately the smallest one that proves the authority
/// path: one child, one grant list, no channels. Everything the plane asserts
/// happens inside the probe, against a real device, through a capability the
/// generation placed — so init's part is to hand it over and observe the
/// outcome.
///
/// `run_token` names a declared endpoint to the spawned instance, for a plane
/// that declares its probe executable twice: the instance init spawns and a
/// root-owned idle one holding the same authority with no session. Sending on it
/// is how the spawned copy learns it is the one that runs, because the root
/// delivers a nonzero boot action only to the bootstrap instance and every other
/// instance — spawned or autostarted — reads zero.
fn drive_probe_plane_with_token(
    executable: u32,
    spawned_marker: &[u8],
    plane: &'static [u8],
    run_token: Option<u32>,
) {
    let probe =
        slime_rt::spawn(executable, &[]).unwrap_or_else(|_| fail_plane(plane, b"spawn probe"));
    slime_rt::debug_write(spawned_marker);
    if let Some(slot) = run_token
        && slime_rt::send(slot, b"run", &[]) != slime_rt::ERR_SUCCESS
    {
        fail_plane(plane, b"deliver the run token");
    }
    wait_clean(&[probe.supervision_slot]);
}

fn fail_plane(plane: &[u8], reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] ");
    slime_rt::debug_write(plane);
    slime_rt::debug_write(b" plane fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}
fn fail_spawn(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] spawn plane fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

/// Drive the P5.3.3 spawn plane: construct children from grant-resolved
/// executables, hand each one the capabilities its declaration names, and
/// observe termination through a supervision handle.
///
/// Only reachable for the authenticated `spawn` action declared by
/// `contracts/generation/v1/fixtures/sel4-spawn.zti`; see the `.md` beside it.
///
/// The two children are `console` and `sysinfo`, both **unmodified** — the same
/// binaries the x86 oracle runs. That is the milestone's claim: a component
/// written against the retired kernel's spawn ABI is started by `slime-root`
/// with no seL4 branch in it. `sysinfo` is the useful one to wait on, because
/// it runs to completion and exits 0 of its own accord; `console` loops until
/// its peer dies, which is what makes it the right subject for the
/// still-live arm.
///
/// What crosses at spawn is *transferable directory authority*, not endpoint
/// halves: an endpoint is a generation-declared seL4 Endpoint the root installs
/// into both ends itself, so a parent has none to hand over. Six views is B15's
/// own exit-condition number — the grant array crosses the transfer window as a
/// staged payload, and six records are 96 bytes, past the 64-byte message bound
/// a narrower reader would apply.
fn drive_spawn_plane() {
    if slime_rt::spawn(63, &[]).is_ok() {
        fail_spawn(b"an empty slot named an executable");
    }
    // A slot holding real authority of another kind. Init genuinely holds its
    // console control endpoint at slot 3, so this is a check on kind rather
    // than on possession.
    if slime_rt::spawn(3, &[]).is_ok() {
        fail_spawn(b"a non-executable capability named an executable");
    }
    slime_rt::debug_write(b"[init] ungranted executable refused\n");
    // The narrowing rule: a grant's rights must be a subset of what the parent
    // holds. Init holds this view with `directoryRead | transfer` alone, so
    // asking to pass on write authority is asking the root to manufacture
    // authority no generation declared.
    if slime_rt::spawn(
        CONSOLE_SLOT,
        &[grant(5, RIGHT_DIRECTORY_READ | RIGHT_DIRECTORY_WRITE)],
    )
    .is_ok()
    {
        fail_spawn(b"a widened grant was accepted");
    }
    slime_rt::debug_write(b"[init] widened grant refused\n");
    // The executable slot is authority to create this child; passing it on
    // would let the child re-spawn its own image outside its parent's budget.
    if slime_rt::spawn(
        CONSOLE_SLOT,
        &[grant(CONSOLE_SLOT, RIGHT_EXEC | RIGHT_SPAWN)],
    )
    .is_ok()
    {
        fail_spawn(b"a child was granted its own executable");
    }
    slime_rt::debug_write(b"[init] self-executable grant refused\n");
    let console = slime_rt::spawn(
        CONSOLE_SLOT,
        &[grant(5, RIGHT_DIRECTORY_READ | RIGHT_TRANSFER)],
    )
    .unwrap_or_else(|_| fail_spawn(b"console"));
    slime_rt::debug_write(b"[init] console spawned\n");
    // A live child has no outcome, and the query says so rather than blocking
    // or inventing one.
    match slime_rt::supervision_status(console.supervision_slot) {
        Ok(None) => {
            slime_rt::debug_write(b"[init] live child reports no outcome\n");
        }
        _ => fail_spawn(b"a live child reported an outcome"),
    }
    // A spawn grant is a copy: the parent can still resolve the slot it
    // granted from.
    let mut root = [0u8; 32];
    let mut scope = [0u8; slime_rt::MAX_DIRECTORY_PATH];
    if slime_rt::directory_inspect(5, RIGHT_DIRECTORY_READ as u32, &mut root, &mut scope).is_err() {
        fail_spawn(b"the granted view stopped resolving");
    }
    slime_rt::debug_write(b"[init] granted view retained\n");
    let wide = [
        grant(6, RIGHT_DIRECTORY_READ | RIGHT_TRANSFER),
        grant(7, RIGHT_DIRECTORY_READ | RIGHT_TRANSFER),
        grant(8, RIGHT_DIRECTORY_READ | RIGHT_TRANSFER),
        grant(9, RIGHT_DIRECTORY_READ | RIGHT_TRANSFER),
        grant(10, RIGHT_DIRECTORY_READ | RIGHT_TRANSFER),
        grant(11, RIGHT_DIRECTORY_READ | RIGHT_TRANSFER),
    ];
    let sysinfo = slime_rt::spawn(SYSINFO_SLOT, &wide).unwrap_or_else(|_| fail_spawn(b"sysinfo"));
    slime_rt::debug_write(b"[init] sysinfo spawned\n");
    for slot in 6..=11 {
        if slime_rt::directory_inspect(slot, RIGHT_DIRECTORY_READ as u32, &mut root, &mut scope)
            .is_err()
        {
            fail_spawn(b"a copied view stopped resolving");
        }
    }
    slime_rt::debug_write(b"[init] six grants copied\n");
    // The launch context, sent down init's own end of the declared endpoint.
    // `sysinfo` is blocked in `recv` on the end the root installed for it.
    if slime_rt::send(4, &launch_context(), &[]) != slime_rt::ERR_SUCCESS {
        fail_spawn(b"deliver the launch context");
    }
    slime_rt::debug_write(b"[init] launch context sent\n");
    wait_clean(&[sysinfo.supervision_slot]);
    slime_rt::debug_write(b"[init] sysinfo outcome collected\n");
    // Collecting consumes the handle, so the outcome is single-use rather than
    // a fact the parent can re-read forever.
    if slime_rt::supervision_status(sysinfo.supervision_slot).is_ok() {
        fail_spawn(b"a collected handle answered twice");
    }
    slime_rt::debug_write(b"[init] collected handle consumed\n");
    // End to end through the unmodified child: `console.rs` `debug_write`s
    // whatever arrives on its slot 0, so this is the child *reading* the
    // endpoint the root installed for it rather than the root reporting it
    // installed one.
    if slime_rt::send(3, b"[console] spawned child reached\n", &[]) != slime_rt::ERR_SUCCESS {
        fail_spawn(b"reach the spawned console");
    }
    // `cap_drop` on a *live* child's handle, exactly as `spawn_or_fail` does on
    // every product boot. Dropped before the close below, so the child is
    // certainly still running: collecting an outcome consumes the handle, which
    // would make this test a no-op on an already-collected one.
    if slime_rt::cap_drop(console.supervision_slot) < 0 {
        fail_spawn(b"drop a live child's handle");
    }
    slime_rt::debug_write(b"[init] dropped handle released\n");
    // The close lets the child exit of its own accord, so the graph reaches the
    // quiescent accounting the gate asserts. Nobody waits on it: the handle is
    // gone, and the root records the termination either way.
    if slime_rt::send(3, b"SLIME.CONSOLE.CLOSE", &[]) != slime_rt::ERR_SUCCESS {
        fail_spawn(b"close the spawned console");
    }
}

/// The launch context `sysinfo` decodes through `launch_context::receive`.
fn launch_context() -> [u8; slime_proto::spawn::REQUEST_LEN] {
    let mut command = [0u8; 16];
    command[..7].copy_from_slice(b"sysinfo");
    slime_proto::spawn::WireSpawnRequest {
        magic: slime_proto::spawn::SPAWN_MAGIC,
        version: slime_proto::spawn::FORMAT_VERSION,
        flags: 0,
        command_len: 7,
        argument_count: 0,
        environment_count: 0,
        capability_roles: 0,
        client_budget: 0,
        command,
        arguments: [0u8; 8],
        environment: [0u8; 8],
        grant_rights: 0,
        reserved: [0u8; 6],
    }
    .encode()
}

/// More lifetimes than the old monotonic root allocator could sustain while
/// keeping only one child live at a time.
const RECLAMATION_LOOP_CHILDREN: u32 = 80;

fn drive_reclamation_plane() {
    if slime_rt::spawn(SUPERVISION_CHILD_SLOT, &[]).is_ok() {
        fail_reclamation(b"forced construction unwind unexpectedly succeeded");
    }
    slime_rt::debug_write(b"[init] reclamation construction unwind returned\n");
    let mut completed = 0u32;
    for _ in 0..RECLAMATION_LOOP_CHILDREN {
        let child = slime_rt::spawn(SUPERVISION_CHILD_SLOT, &[])
            .unwrap_or_else(|_| fail_reclamation(b"loop child spawn"));
        loop {
            match slime_rt::supervision_status(child.supervision_slot) {
                Ok(None) => slime_rt::yield_now(),
                Ok(Some(slime_rt::Termination::Exit(0))) => break,
                _ => fail_reclamation(b"loop child termination"),
            }
        }
        completed += 1;
    }
    if completed != RECLAMATION_LOOP_CHILDREN {
        fail_reclamation(b"lifetime loop incomplete");
    }
    slime_rt::debug_write(b"[init] reclamation lifetime bound crossed\n");
    let fault = slime_rt::spawn(RECLAMATION_FAULT_SLOT, &[])
        .unwrap_or_else(|_| fail_reclamation(b"fault child spawn"));
    loop {
        match slime_rt::supervision_status(fault.supervision_slot) {
            Ok(None) => slime_rt::yield_now(),
            Ok(Some(slime_rt::Termination::Fault(_))) => break,
            _ => fail_reclamation(b"fault child termination"),
        }
    }
    slime_rt::debug_write(b"[init] reclamation fault path reused\n");
}

fn fail_reclamation(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] reclamation plane fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

/// How many children the supervision plane creates over the boot.
///
/// One more than `slime-root`'s current `MAX_RECORDS` (48), which is the whole
/// point: the bound this crosses is on records *awaiting collection*, and a
/// graph that collects as it goes must be able to exceed it. A loop that
/// stopped at the bound would pass against the unfixed root and prove nothing.
const SUPERVISION_LOOP_CHILDREN: u32 = 49;

/// Drive the supervision plane: create more children over one boot than
/// `MAX_RECORDS` can hold at once, and answer correctly for every live handle.
///
/// Only reachable for the authenticated `supervision` action declared by
/// `contracts/generation/v1/fixtures/sel4-supervision.zti`.
///
/// This is backlog B16's exit condition. Before the fix, `Terminations` never
/// reclaimed, so the 33rd child's outcome was dropped silently and its parent
/// waited forever. The gate crosses the bound and then asserts the two things a
/// sweep could plausibly break:
///
/// - a handle held *across* the crossing still answers afterwards, and
/// - a handle **parked in transit** across the crossing is still collectable,
///   which is the half a predicate over live tables alone would miss.
///
/// The loop child is `supervision-child`, which takes no channel:
/// `ChannelTable` never reclaims (B22), so a child needing one would exhaust
/// channels before the loop reached the record bound.
fn drive_supervision_plane() {
    let retained = slime_rt::spawn(SUPERVISION_CHILD_SLOT, &[])
        .unwrap_or_else(|_| fail_supervision(b"retained child"));
    // B25: a second handle naming a task this component already supervises.
    // Neither a spawn grant nor an export could place one twice — a grant must
    // precede the child, and an export moves — so derivation is the only way.
    //
    // Derived while the source is still held and before it is collected, since
    // collection consumes the handle. Both copies then cross the allocation
    // bound below.
    let derived = slime_rt::supervision_derive(retained.supervision_slot)
        .unwrap_or_else(|_| fail_supervision(b"derive a second handle"));
    slime_rt::debug_write(b"[init] second supervision handle derived\n");
    slime_rt::debug_write(b"[init] supervision handle retained\n");
    // The source, collected here so this declaration has no live task and the
    // loop below can reuse it. That is also what makes the derived copy the
    // interesting one: it outlives the handle it came from.
    wait_clean(&[retained.supervision_slot]);
    for _ in 0..SUPERVISION_LOOP_CHILDREN {
        let child = slime_rt::spawn(SUPERVISION_CHILD_SLOT, &[])
            .unwrap_or_else(|_| fail_supervision(b"loop child"));
        wait_clean(&[child.supervision_slot]);
    }
    slime_rt::debug_write(b"[init] supervision lifetime bound crossed\n");
    // The derived copy, held across a boot's worth of allocation *and* past the
    // collection of the handle it came from, still answers. That is the half a
    // predicate over live tables alone would miss: the task is long gone and
    // every other trace of it erased.
    if !matches!(
        slime_rt::supervision_status(derived),
        Ok(Some(slime_rt::Termination::Exit(0)))
    ) {
        fail_supervision(b"the derived handle lost its authority");
    }
    slime_rt::debug_write(b"[init] retained handle answered after crossing\n");
    slime_rt::debug_write(b"[init] derived supervision survived crossing\n");
    // Collecting consumes the handle: the outcome lives in the capability, so a
    // second query must be refused rather than answered from elsewhere (B42).
    if slime_rt::supervision_status(derived).is_ok() {
        fail_supervision(b"a collected handle answered twice");
    }
    slime_rt::debug_write(b"[init] collected handle refused\n");
}

/// More rendezvous exchanges than the retired logical lifetime bound of 48.
/// The direct endpoint is static, so this proves transport stays live without
/// depending on root-mediated channel allocation or sweeping.
const CHANNEL_LOOP_PAIRS: u32 = 49;

/// Drive the direct endpoint crossing plane with one-cap narrowed copy
/// delegation and sustained native request/reply rendezvous.
fn drive_crossing_plane() {
    const CARRIER_SLOT: u32 = 2;
    const GATE_SLOT: u32 = 3;
    let peer = slime_rt::spawn(CROSSING_PEER_SLOT, &[])
        .unwrap_or_else(|_| fail_crossing(b"crossing peer"));
    let descriptor = slime_proto::capability_transfer::WireCapabilityTransfer {
        magic: slime_proto::capability_transfer::CAPABILITY_TRANSFER_MAGIC,
        version: slime_proto::capability_transfer::FORMAT_VERSION,
        status: 0,
        flags: slime_proto::capability_transfer::FLAG_RETAIN_TRANSFER,
        object_kind: slime_proto::capability_transfer::OBJECT_KIND_ENDPOINT,
        direction: 0,
        rights_mask: RIGHT_SEND,
        route_identity: [0u8; 32],
    };
    if slime_rt::capability_delegate(
        CARRIER_SLOT,
        GATE_SLOT,
        CapabilityDisposition::Retain,
        slime_proto::capability_transfer::OBJECT_KIND_ENDPOINT,
        RIGHT_SEND,
        &descriptor.encode(),
    ) != slime_rt::ERR_SUCCESS
    {
        fail_crossing(b"delegate narrowed endpoint");
    }
    slime_rt::debug_write(b"[init] endpoint capability exported before crossing\n");
    let mut payload = [0u8; slime_rt::MAX_MSG];
    let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(GATE_SLOT, &mut payload, &mut caps) {
            slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
            8 if &payload[..8] == b"survived" => break,
            _ => fail_crossing(b"sender copy did not remain usable"),
        }
    }
    slime_rt::debug_write(b"[init] sender retained delegated authority\n");
    slime_rt::debug_write(b"[init] imported endpoint survived crossing\n");
    for _ in 0..CHANNEL_LOOP_PAIRS {
        if slime_rt::send(CARRIER_SLOT, b"ping", &[]) != slime_rt::ERR_SUCCESS {
            fail_crossing(b"native crossing send");
        }
        loop {
            match slime_rt::recv(GATE_SLOT, &mut payload, &mut caps) {
                slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
                4 if &payload[..4] == b"pong" => break,
                _ => fail_crossing(b"native crossing reply"),
            }
        }
    }
    slime_rt::debug_write(b"[init] channel lifetime bound crossed\n");
    loop {
        match slime_rt::supervision_status(peer.supervision_slot) {
            Ok(None) => slime_rt::yield_now(),
            Ok(Some(slime_rt::Termination::Exit(0))) => break,
            _ => fail_crossing(b"crossing peer failed"),
        }
    }
}

fn fail_crossing(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] crossing plane fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn fail_supervision(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] supervision plane fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

/// Launch the C8.3/C8.4 fabric plane: one service that owns every route
/// endpoint, and five clients that can only ask it for one.
///
/// Init deliberately holds no route capability. Every control is a
/// generation-declared native Endpoint whose two halves the root installs into
/// the fabric and the client that declared them, so the binding between a
/// control endpoint and a component identity is a manifest fact the fabric
/// authenticates against — a client cannot forge, share, or re-derive one, so
/// "which component is asking" is a capability fact rather than a claim in a
/// message.
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
fn launch_fabric_graph(plane: &[u8], service_spawned: &[u8]) {
    // Every control endpoint is a generation-declared native Endpoint: the
    // root materializes both halves from the manifest's grants and installs
    // each side into the instance that declared it, before any of them runs.
    // Init therefore holds no route capability and mints nothing -- the
    // binding between a control endpoint and a component identity is a
    // generation fact, which is exactly what the fabric authenticates
    // against.
    plane_marker(plane, b" control channels minted\n");
    // Every ring participant starts before the fabric. A v2 stream edge is a
    // writable shared ring the fabric *loans* to its peer, and a loan names
    // its receiver through a supervision capability — so the handle must
    // exist before the service that will use it does. Under v1 only
    // subscribers received loans, which is why only they had to precede it.
    let publisher =
        slime_rt::spawn(FABRIC_PUBLISHER_SLOT, &[]).unwrap_or_else(|_| slime_rt::exit(1));
    let subscriber =
        slime_rt::spawn(FABRIC_SUBSCRIBER_SLOT, &[]).unwrap_or_else(|_| slime_rt::exit(1));
    let publisher_b = slime_rt::spawn(
        FABRIC_PUBLISHER_B_SLOT,
        &[grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE)],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    let subscriber_b =
        slime_rt::spawn(FABRIC_SUBSCRIBER_B_SLOT, &[]).unwrap_or_else(|_| slime_rt::exit(1));
    // The declared interposition proxy precedes the fabric for the same reason
    // every ring participant does: the fabric is granted a supervision handle
    // naming it, and a handle cannot exist before its task. A native Endpoint
    // reports no peer death, so this handle is the only way the broker can
    // observe a hop through a dead proxy rather than blocking on it forever.
    let intruder = slime_rt::spawn(FABRIC_INTRUDER_SLOT, &[]).unwrap_or_else(|_| slime_rt::exit(1));
    // What init still passes is exactly what the generation cannot place: the
    // shared-buffer factory it holds, and one supervision handle per ring
    // participant and declared proxy, which only exist once those tasks do.
    // Matching is positional against ascending declared slot: factory at 1,
    // plane declaring an interposition names the proxy among them. Which those
    // are is a manifest fact, not a build flag: `FABRIC_MINTED_GRANTS` states
    // how many capabilities this child's owner must supply, so the set is
    // sliced to that generated count.
    let grants = [
        grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
        grant(publisher.supervision_slot, RIGHT_SUPERVISE),
        grant(subscriber.supervision_slot, RIGHT_SUPERVISE),
        grant(intruder.supervision_slot, RIGHT_SUPERVISE),
        grant(publisher_b.supervision_slot, RIGHT_SUPERVISE),
        grant(subscriber_b.supervision_slot, RIGHT_SUPERVISE),
    ];
    let without_proxy = [grants[0], grants[1], grants[2], grants[4], grants[5]];
    let declared = declared_minted_grants(b"fabric-service");
    let service = slime_rt::spawn(
        FABRIC_SERVICE_SLOT,
        if declared == grants.len() {
            &grants[..]
        } else if declared == without_proxy.len() {
            &without_proxy[..]
        } else {
            slime_rt::debug_write(b"[init] fabric-service grant count is not a declared shape\n");
            slime_rt::exit(1)
        },
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    plane_marker(plane, service_spawned);
    plane_marker(plane, b" participants spawned\n");
    wait_clean(&[
        publisher.supervision_slot,
        publisher_b.supervision_slot,
        subscriber.supervision_slot,
        subscriber_b.supervision_slot,
        intruder.supervision_slot,
        service.supervision_slot,
    ]);
}

/// One `[init] <plane><suffix>` line. The stream and visibility gates name the
/// same composition differently, so the plane word is a parameter rather than
/// two copies of this launcher.
fn plane_marker(plane: &[u8], suffix: &[u8]) {
    slime_rt::debug_write(b"[init] ");
    slime_rt::debug_write(plane);
    slime_rt::debug_write(suffix);
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
            Ok(None) => slime_rt::yield_now(),
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
