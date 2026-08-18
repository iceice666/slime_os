#![no_std]
#![no_main]

slime_rt::entry!(main);

#[path = "../loan_plane.rs"]
mod loan_plane;
use loan_plane::{PEER_PARK_YIELDS, drive_loan_plane};
#[path = "../spawn_plane.rs"]
mod spawn_plane;
use spawn_plane::drive_spawn_plane;
#[path = "../crossing_plane.rs"]
mod crossing_plane;
use crossing_plane::drive_crossing_plane;
#[path = "../supervision_plane.rs"]
mod supervision_plane;
use supervision_plane::drive_supervision_plane;

use slime_rt::{Rights, SpawnGrant};
// B59: the capability-rights vocabulary is generated from
// `contracts/generation/v5/schema.zt`; these were local copies of the same
// bit numbering.
use boot_contracts::generation::{
    RIGHT_BUFFER_CREATE, RIGHT_DIRECTORY_READ, RIGHT_DIRECTORY_WRITE, RIGHT_EXEC, RIGHT_SEND,
    RIGHT_SPAWN, RIGHT_SUPERVISE, RIGHT_TRANSFER,
};

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

// Manifest-derived bootstrap slot order is emitted by the host builder.
const CONSOLE_CAPS: [SpawnGrant; 0] = [];

fn spawn_service_caps() -> [SpawnGrant; 3] {
    // The two executables spawn-service may launch, and the factory it allocates
    // from. Ascending declared slot is the order the root matches against, so
    // this list's order is load-bearing while the numbers in it are not (CP2/B70).
    [
        grant(
            resolve_executable(b"executable:sysinfo"),
            RIGHT_EXEC | RIGHT_SPAWN,
        ),
        grant(
            resolve_executable(b"executable:echo-agent"),
            RIGHT_EXEC | RIGHT_SPAWN,
        ),
        grant(resolve_buffer_factory(), RIGHT_BUFFER_CREATE),
    ]
}

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

// The x86 storage-probe selection cascade and the generation-command caps tables
// were deleted here (B70). Every executable they named -- `storage-writer`,
// `storage-fault-probe`, `storage-store-probe`, `storage-probe`,
// `filesystem-service`, and the five `generation-*` commands -- is declared by
// none of the 28 seL4 manifests, so each constant resolved `SLOT_ABSENT` and
// every branch testing it was unreachable on this kernel. The seL4 planes reach
// the same behavior through their own `bootAction`: `drive_storage_plane`,
// `drive_store_plane`, `drive_filesystem_plane`, and `drive_generation_plane`
// name `sel4-storage-probe`, `sel4-store-probe`, `sel4-filesystem-service`, and
// `sel4-generation-client`/`-manager`, which the fixtures do declare.

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
                resolve_executable(b"executable:sel4-transfer-probe"),
                b"[init] transfer probe spawned\n",
                b"transfer",
                Some(2),
            );
            slime_rt::debug_write(b"[init] transfer plane complete\n");
            slime_rt::exit(0)
        }
        action::INPUT => {
            drive_probe_plane_with_token(
                resolve_executable(b"executable:sel4-input-probe"),
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
                resolve_executable(b"executable:sel4-directory-probe"),
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
                resolve_executable(b"executable:sel4-recovery-probe"),
                b"[init] recovery probe spawned\n",
                b"recovery",
                Some(2),
            );
            slime_rt::debug_write(b"[init] recovery plane complete\n");
            slime_rt::exit(0)
        }
        action::ROLLBACK => {
            drive_probe_plane_with_token(
                resolve_executable(b"executable:sel4-rollback-probe"),
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

    // The product graph the seL4 `product` generation declares: console,
    // spawn-service, and the two executables spawn-service may launch. Both are
    // resolved through the root rather than compiled in (CP2/B70), correct only
    // since B71 made the boot-layout resource derive from the same
    // `InstanceBinding` records the root places from.
    //
    // The filesystem, storage, and generation-command branches that stood here
    // were deleted with the constants they tested: `sel4.zti` is the only
    // generation reaching this body, and it declares `console`, `spawn-service`,
    // `sysinfo`, `echo-agent`, and `init` -- nothing else. Every branch was
    // therefore dead on this kernel, and the seL4 planes that do exercise those
    // components reach them through their own `bootAction` instead.
    let console_executable =
        slime_rt::resolve_binding(b"executable:console").unwrap_or_else(|_| slime_rt::exit(1));
    let component_console = slime_rt::spawn(console_executable, &CONSOLE_CAPS)
        .unwrap_or_else(|_| slime_rt::exit(1))
        .supervision_slot;
    let spawn_service_executable = slime_rt::resolve_binding(b"executable:spawn-service")
        .unwrap_or_else(|_| slime_rt::exit(1));
    let component_spawn_service = slime_rt::spawn(spawn_service_executable, &spawn_service_caps())
        .unwrap_or_else(|_| slime_rt::exit(1))
        .supervision_slot;

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
    if slime_rt::send(resolve_spawn_service_rpc(), &shutdown.encode(), &[]) != slime_rt::ERR_SUCCESS
    {
        slime_rt::exit(1);
    }
    wait_clean(&[component_spawn_service]);
    if slime_rt::send(console_send_slot(), b"SLIME.CONSOLE.CLOSE", &[]) != slime_rt::ERR_SUCCESS {
        slime_rt::exit(1);
    }
    wait_clean(&[component_console]);
    slime_rt::debug_write(b"[init] component services completed\n");
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
    let publisher = spawn_boot(b"executable:fabric-publisher");
    let subscriber = spawn_boot(b"executable:fabric-subscriber");
    let publisher_b = spawn_boot(b"executable:fabric-publisher-b");
    let subscriber_b = spawn_boot(b"executable:fabric-subscriber-b");
    let observer = spawn_boot(b"executable:fabric-observer");
    let proxy = spawn_boot(b"executable:fabric-proxy");
    // Holds a real control endpoint and is granted no edge; its denial is the
    // plane's authority evidence, so it needs no handle from anyone.
    spawn_boot(b"executable:fabric-probe");
    slime_rt::debug_write(b"[init] fabric boot stream participants spawned\n");

    // Matched positionally against the child's declarations in ascending
    // destination-slot order: the factory at 1, then the six handles at 9..14.
    let fabric = spawn_boot_with(
        b"executable:fabric-service",
        &[
            grant(resolve_own_buffer_factory(), RIGHT_BUFFER_CREATE),
            grant(publisher, RIGHT_SUPERVISE),
            grant(subscriber, RIGHT_SUPERVISE),
            grant(publisher_b, RIGHT_SUPERVISE),
            grant(subscriber_b, RIGHT_SUPERVISE),
            grant(observer, RIGHT_SUPERVISE),
            grant(proxy, RIGHT_SUPERVISE),
        ],
    );
    slime_rt::debug_write(b"[init] fabric boot stream broker spawned\n");

    let call_client = spawn_boot(b"executable:fabric-call-client");
    let call_client_b = spawn_boot(b"executable:fabric-call-client-b");
    let call_server = spawn_boot(b"executable:fabric-call-server");
    // The clock asks for nothing and is named by no handle: the worker observes
    // its exit through the control endpoint's own peer state.
    spawn_boot(b"executable:fabric-call-time");
    // The call worker copies large payloads, so it holds buffer-creation
    // authority of its own, bounded by its declared quota.
    spawn_boot_with(
        b"executable:fabric-call-worker",
        &[
            grant(resolve_own_buffer_factory(), RIGHT_BUFFER_CREATE),
            grant(call_client, RIGHT_SUPERVISE),
            grant(call_client_b, RIGHT_SUPERVISE),
            grant(call_server, RIGHT_SUPERVISE),
        ],
    );
    slime_rt::debug_write(b"[init] fabric boot call plane spawned\n");

    let op_client = spawn_boot(b"executable:fabric-op-client");
    let op_client_b = spawn_boot(b"executable:fabric-op-client-b");
    let op_server = spawn_boot(b"executable:fabric-op-server");
    spawn_boot(b"executable:fabric-op-time");
    let op_restart = spawn_boot(b"executable:fabric-op-client-b-restart");
    spawn_boot_with(
        b"executable:fabric-op-worker",
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
    let publisher = spawn_boot(b"executable:fabric-publisher");
    let subscriber = spawn_boot(b"executable:fabric-subscriber");
    let publisher_b = spawn_boot_with(
        b"executable:fabric-publisher-b",
        &[grant(resolve_own_buffer_factory(), RIGHT_BUFFER_CREATE)],
    );
    let subscriber_b = spawn_boot(b"executable:fabric-subscriber-b");
    let observer = spawn_boot(b"executable:fabric-observer");
    let proxy = spawn_boot(b"executable:fabric-proxy");
    let probe = spawn_boot(b"executable:fabric-probe");
    slime_rt::debug_write(b"[init] traffic stream participants spawned\n");

    let fabric = spawn_boot_with(
        b"executable:fabric-service",
        &[
            grant(resolve_own_buffer_factory(), RIGHT_BUFFER_CREATE),
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
        b"executable:fabric-call-client",
        &[grant(resolve_own_buffer_factory(), RIGHT_BUFFER_CREATE)],
    );
    let call_client_b = spawn_boot(b"executable:fabric-call-client-b");
    let call_server = spawn_boot_with(
        b"executable:fabric-call-server",
        &[grant(resolve_own_buffer_factory(), RIGHT_BUFFER_CREATE)],
    );
    let call_time = spawn_boot(b"executable:fabric-call-time");
    let call_worker = spawn_boot_with(
        b"executable:fabric-call-worker",
        &[
            grant(resolve_own_buffer_factory(), RIGHT_BUFFER_CREATE),
            grant(call_client, RIGHT_SUPERVISE),
            grant(call_client_b, RIGHT_SUPERVISE),
            grant(call_server, RIGHT_SUPERVISE),
        ],
    );
    slime_rt::debug_write(b"[init] traffic call plane spawned\n");

    let op_client = spawn_boot(b"executable:fabric-op-client");
    let op_client_b = spawn_boot(b"executable:fabric-op-client-b");
    let op_server = spawn_boot(b"executable:fabric-op-server");
    let op_time = spawn_boot(b"executable:fabric-op-time");
    let op_restart = spawn_boot(b"executable:fabric-op-client-b-restart");
    let op_worker = spawn_boot_with(
        b"executable:fabric-op-worker",
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

/// One boot-layout executable slot, resolved through the root by name.
///
/// CP2/B70: the slot number is a fact about the active generation, so this image
/// asks for it rather than compiling it in. A generation whose layout declares no
/// such executable is a real answer — the caller asked to launch a component this
/// composition does not have — so it exits rather than falling back to a guess.
fn resolve_executable(name: &[u8]) -> u32 {
    slime_rt::resolve_binding(name).unwrap_or_else(|_| slime_rt::exit(1))
}

/// Init's request endpoint to `spawn-service`, resolved by grant name.
///
/// A plain grant lookup: `spawn-service-rpc` is an ordinary endpoint binding in
/// init's own list, so no prefix is needed and the root answers only from that
/// list. This replaces `SPAWN_SERVICE_RPC_SLOT`, the last compiled slot in the
/// product graph's shutdown path (CP2/B70).
fn resolve_spawn_service_rpc() -> u32 {
    slime_rt::resolve_binding(b"spawn-service-rpc").unwrap_or_else(|_| slime_rt::exit(1))
}

/// Init's shared-buffer factory slot, resolved through the root by capability
/// role rather than compiled in.
///
/// `kind:sharedBufferFactory+bufferCreate` asks by what the capability *is*, and
/// the root refuses an ambiguous answer, so this is only usable where the
/// generation grants init exactly one factory — every plane but the full-graph
/// `boot` and `traffic` compositions, which hold two and use
/// [`resolve_own_buffer_factory`].
///
/// Deliberately not "the factory granted to me": that spelling looked like the
/// general rule and is not. Under the product graph init holds one factory whose
/// grant target is `spawn-service`, not itself, so a target test resolves nothing
/// exactly where this is needed most.
fn resolve_buffer_factory() -> u32 {
    slime_rt::resolve_binding(b"kind:sharedBufferFactory+bufferCreate")
        .unwrap_or_else(|_| slime_rt::exit(1))
}

/// Init's *own* shared-buffer factory, for the two compositions that grant it
/// two.
///
/// The full-graph `boot` and `traffic` generations bind both
/// `init-shared-buffer-factory` and `fabric-service-shared-buffer-factory` to
/// init, so `resolve_buffer_factory`'s role query is ambiguous there and refuses.
/// A grant name is unambiguous, and these two names are stable across every
/// generation reaching this code: `traffic`, `fault`, and `saturation` share one
/// manifest, differing only in generation number.
///
/// Which of the two is delegated does not change what the receiver may do — a
/// shared-buffer quota binds to the receiving task, not to the factory capability
/// handed over, verified by delegating the other one and observing the boot plane
/// stay green. So this names init's own for the same reason the source reads
/// better for it, not because the authority differs.
fn resolve_own_buffer_factory() -> u32 {
    slime_rt::resolve_binding(b"init-shared-buffer-factory").unwrap_or_else(|_| slime_rt::exit(1))
}

/// Spawn one boot participant that its manifest grants nothing, returning the
/// supervision handle init keeps.
fn spawn_boot(executable: &[u8]) -> u32 {
    spawn_boot_with(executable, &[])
}

/// Spawn one boot participant with the exact grant vector its manifest declares.
///
/// The count must equal what `preflight_spawn_grants` derives from the
/// generation — the child's minted bindings plus its spawn-crossing grant
/// bindings — or the root refuses the spawn with nothing constructed. Both
/// numbers come from the same manifest, so a disagreement is a fixture defect
/// rather than something to reconcile here.
///
/// `executable` is the component's name, not a slot: the root resolves it from
/// the boot layout it placed these capabilities from (CP2/B70), so this image
/// carries no plane's slot numbering. `executable:` names the layout's component
/// identity domain, which is what keeps a channel of the same name from
/// answering.
fn spawn_boot_with(executable: &[u8], grants: &[SpawnGrant]) -> u32 {
    let executable_slot = match slime_rt::resolve_binding(executable) {
        Ok(slot) => slot,
        Err(error) => {
            slime_rt::debug_write(b"[init] fabric boot unresolved executable error=");
            write_i64(error);
            slime_rt::debug_write(b"\n");
            fail_boot(b"resolve participant executable")
        }
    };
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
    let client = slime_rt::spawn(resolve_executable(b"executable:fabric-op-client"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
    let client_b = slime_rt::spawn(resolve_executable(b"executable:fabric-op-client-b"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
    let server = slime_rt::spawn(resolve_executable(b"executable:fabric-op-server"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
    let replacement = slime_rt::spawn(
        resolve_executable(b"executable:fabric-op-client-b-restart"),
        &[],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    slime_rt::debug_write(b"[init] operation participants spawned\n");
    slime_rt::debug_write(b"[init] operation replacement introduced\n");
    // What init still passes is exactly what the generation cannot place: the
    // shared-buffer factory it holds, and one supervision handle per
    // participant, which only exist once those tasks do. Matching is positional
    // against ascending declared slot: factory at 1, then the handles.
    let service = slime_rt::spawn(
        resolve_executable(b"executable:fabric-service"),
        &[
            grant(resolve_buffer_factory(), RIGHT_BUFFER_CREATE),
            grant(client.supervision_slot, RIGHT_SUPERVISE),
            grant(client_b.supervision_slot, RIGHT_SUPERVISE),
            grant(server.supervision_slot, RIGHT_SUPERVISE),
            grant(replacement.supervision_slot, RIGHT_SUPERVISE),
        ],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    slime_rt::debug_write(b"[init] operation fabric spawned\n");
    slime_rt::debug_write(b"[init] operation supervision delegated\n");
    let time = slime_rt::spawn(resolve_executable(b"executable:fabric-op-time"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
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
        resolve_executable(b"executable:fabric-call-client"),
        &[grant(resolve_buffer_factory(), RIGHT_BUFFER_CREATE)],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    let client_b = slime_rt::spawn(resolve_executable(b"executable:fabric-call-client-b"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
    let server = slime_rt::spawn(
        resolve_executable(b"executable:fabric-call-server"),
        &[grant(resolve_buffer_factory(), RIGHT_BUFFER_CREATE)],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    slime_rt::debug_write(b"[init] call participants spawned\n");
    // What init still passes is exactly what the generation cannot place: the
    // shared-buffer factory it holds, and one supervision handle per
    // participant, which only exist once those tasks do. Matching is positional
    // against ascending declared slot: factory at 1, then the handles.
    let service = slime_rt::spawn(
        resolve_executable(b"executable:fabric-service"),
        &[
            grant(resolve_buffer_factory(), RIGHT_BUFFER_CREATE),
            grant(client.supervision_slot, RIGHT_SUPERVISE),
            grant(client_b.supervision_slot, RIGHT_SUPERVISE),
            grant(server.supervision_slot, RIGHT_SUPERVISE),
        ],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    slime_rt::debug_write(b"[init] call fabric spawned\n");
    slime_rt::debug_write(b"[init] call supervision delegated\n");
    let time = slime_rt::spawn(resolve_executable(b"executable:fabric-call-time"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
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
    // `console` is an *executable* slot the boot layout declares and no grant
    // binds, so resolving it exercises the layout half of the query — the half
    // `CONSOLE_SLOT` was the compiled stand-in for. The `executable:` prefix names
    // which of the layout's two identity domains is meant; without it the root
    // treats the name as a grant and refuses, which is what keeps a layout entry
    // from ever shadowing a grant.
    let console_executable = slime_rt::resolve_binding(b"executable:console")
        .unwrap_or_else(|_| fail(b"no console executable in this generation's layout"));
    let console =
        slime_rt::spawn(console_executable, &[]).unwrap_or_else(|_| fail(b"spawn console"));
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

/// The channel init uses for console output in the active generation.
///
/// Product generations name it `console-output`; the standalone channel and loan
/// planes retain the older `dango-output` edge. CP2 resolves whichever the active
/// generation declares by asking the root, rather than choosing between two
/// compile-time constants on the authenticated boot action.
///
/// That branch is what the milestone removes. `DANGO_OUTPUT_SLOT` and
/// `CONSOLE_OUTPUT_SLOT` were both baked into this image from one manifest's
/// layout, so the binary carried every graph's numbering and selected among them
/// at runtime anyway — the coupling B70 names, one step removed. Asking by name
/// answers the same question without the image knowing either number.
fn console_send_slot() -> u32 {
    // The two names the generations that reach this code give one edge:
    // `console-output` under the product graph, `dango-output` under the channel
    // and loan planes. Verified against the fixtures rather than assumed — a
    // third spelling, `dango-console-rpc`, was listed here and was dead code: the
    // dango plane binds that name to `console`, not to `init`, and binds `init`
    // no console edge at all, so this function is never reached there.
    //
    // No generation binds both, so this is a disjoint lookup rather than a
    // precedence rule.
    //
    // A pair of names is still a manifest fact in this source, and a smaller one
    // than the slot numbers it replaces: the numbers differed per generation and
    // had to be selected by boot action, while a name is stable across every
    // generation declaring that edge. Giving the edge one name across the
    // fixtures is a fixture change that would delete the list.
    //
    // The root answers only from this instance's own binding list, so a name this
    // generation does not give `init` is refused rather than resolved from the
    // shared boot layout. An earlier root did consult that layout, which declares
    // every plane's edges, and this call site is where it went wrong: `init`
    // asked, received another plane's edge, and sent into an endpoint nobody was
    // waiting on.
    for name in [b"console-output".as_slice(), b"dango-output".as_slice()] {
        if let Ok(slot) = slime_rt::resolve_binding(name) {
            return slot;
        }
    }
    fail(b"no console output binding in this generation")
}

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
    let receiver = slime_rt::spawn(resolve_executable(b"executable:sample-receiver"), &[])
        .unwrap_or_else(|_| fail_sample(b"spawn receiver"));
    // Matched positionally against ascending declared slot, exactly as
    // `launch_fabric_calls` matches: the lender's factory at 1, then the
    // receiver's supervision handle at 2. The channel is a declared endpoint the
    // root installs on both sides, so it is not in this list.
    let lender = slime_rt::spawn(
        resolve_executable(b"executable:sample-lender"),
        &[
            grant(resolve_buffer_factory(), RIGHT_BUFFER_CREATE),
            grant(receiver.supervision_slot, RIGHT_SUPERVISE),
        ],
    )
    .unwrap_or_else(|_| fail_sample(b"spawn lender"));
    if slime_rt::spawn(resolve_executable(b"executable:sample-receiver"), &[])
        != Err(slime_rt::ERR_BAD_CAP)
    {
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
    let reaped = slime_rt::spawn(resolve_executable(b"executable:sample-receiver"), &[])
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
    let publisher = spawn_boot(b"executable:fabric-publisher");
    let subscriber = spawn_boot(b"executable:fabric-subscriber");
    let publisher_b = spawn_boot_with(
        b"executable:fabric-publisher-b",
        &[grant(resolve_buffer_factory(), RIGHT_BUFFER_CREATE)],
    );
    let subscriber_b = spawn_boot(b"executable:fabric-subscriber-b");
    let observer = spawn_boot(b"executable:fabric-observer");
    let proxy = spawn_boot(b"executable:fabric-proxy");
    let probe = spawn_boot(b"executable:fabric-probe");
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
        grant(resolve_buffer_factory(), RIGHT_BUFFER_CREATE),
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
    let service = spawn_boot_with(b"executable:fabric-service", &grants);
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
    let chooser = slime_rt::spawn(resolve_executable(b"executable:powerbox-chooser"), &[])
        .unwrap_or_else(|_| fail_plane(b"powerbox", b"spawn chooser"));
    slime_rt::debug_write(b"[init] powerbox chooser spawned\n");
    let probe = slime_rt::spawn(resolve_executable(b"executable:powerbox-probe"), &[])
        .unwrap_or_else(|_| fail_plane(b"powerbox", b"spawn probe"));
    slime_rt::debug_write(b"[init] powerbox probe spawned\n");
    wait_clean(&[probe.supervision_slot, chooser.supervision_slot]);
}

/// One boot-layout executable slot, resolved by name through the root.
///
/// CP2's replacement for the `build.rs`-generated `*_SLOT` constants. `reason` is
/// the component name reported when the active generation's layout declares no
/// such executable, which is a real answer rather than a failure to paper over:
/// a plane that cannot find the component it is about to launch must say so
/// instead of spawning whatever sits at a guessed slot.
fn layout_executable(query: &[u8], reason: &'static [u8]) -> u32 {
    slime_rt::resolve_binding(query).unwrap_or_else(|_| fail_plane(b"dango", reason))
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
    // The three executables this plane launches are layout roles, resolved by
    // name through the root rather than compiled in (CP2). `executable:` names
    // which of the layout's two identity domains is meant.
    let console_slot = layout_executable(b"executable:console", b"console");
    let service_slot = layout_executable(b"executable:spawn-service", b"spawn-service");
    let dango_slot = layout_executable(b"executable:dango", b"dango");
    let console = slime_rt::spawn(
        console_slot,
        &[grant(resolve_buffer_factory(), RIGHT_BUFFER_CREATE)],
    )
    .unwrap_or_else(|_| fail_plane(b"dango", b"spawn console"));
    slime_rt::debug_write(b"[init] console spawned\n");
    // The spawn service receives its factory and the two executables it may
    // launch. Both executable grants are sourced by init, so init is the party
    // that must pass them; ascending declared slot is the order the root pairs
    // requests with declarations in.
    let service = slime_rt::spawn(
        service_slot,
        &[
            grant(resolve_buffer_factory(), RIGHT_BUFFER_CREATE),
            grant(6, RIGHT_EXEC | RIGHT_SPAWN),
            grant(7, RIGHT_EXEC | RIGHT_SPAWN),
        ],
    )
    .unwrap_or_else(|_| fail_plane(b"dango", b"spawn service"));
    slime_rt::debug_write(b"[init] spawn service spawned\n");
    let dango =
        slime_rt::spawn(dango_slot, &[]).unwrap_or_else(|_| fail_plane(b"dango", b"spawn dango"));
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
    let service = slime_rt::spawn(
        resolve_executable(b"executable:sel4-filesystem-service"),
        &[],
    )
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
    // `directory-probe`, not `sel4-directory-probe`: this plane and the
    // directory plane declare different executables, and only the fixtures say
    // which. Verified against `sel4-filesystem.layout`.
    let client = slime_rt::spawn(resolve_executable(b"executable:directory-probe"), &[])
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
    let client = slime_rt::spawn(
        resolve_executable(b"executable:sel4-generation-client"),
        &[],
    )
    .unwrap_or_else(|_| fail_plane(b"generation", b"spawn client"));
    slime_rt::debug_write(b"[init] generation client spawned\n");
    let manager = slime_rt::spawn(
        resolve_executable(b"executable:sel4-generation-manager"),
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
        resolve_executable(b"executable:sel4-store-probe"),
        b"[init] store probe spawned\n",
        b"store",
        Some(2),
    );
}

/// Drive the P5.4.2c storage plane: spawn the probe holding its block
/// capability and require a clean exit.
fn drive_storage_plane() {
    drive_probe_plane_with_token(
        resolve_executable(b"executable:sel4-storage-probe"),
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

/// More lifetimes than the old monotonic root allocator could sustain while
/// keeping only one child live at a time.
const RECLAMATION_LOOP_CHILDREN: u32 = 80;

fn drive_reclamation_plane() {
    if slime_rt::spawn(resolve_executable(b"executable:supervision-child"), &[]).is_ok() {
        fail_reclamation(b"forced construction unwind unexpectedly succeeded");
    }
    slime_rt::debug_write(b"[init] reclamation construction unwind returned\n");
    let mut completed = 0u32;
    for _ in 0..RECLAMATION_LOOP_CHILDREN {
        let child = slime_rt::spawn(resolve_executable(b"executable:supervision-child"), &[])
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
    let fault = slime_rt::spawn(resolve_executable(b"executable:reclamation-fault"), &[])
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
    let publisher = slime_rt::spawn(resolve_executable(b"executable:fabric-publisher"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
    let subscriber = slime_rt::spawn(resolve_executable(b"executable:fabric-subscriber"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
    let publisher_b = slime_rt::spawn(
        resolve_executable(b"executable:fabric-publisher-b"),
        &[grant(resolve_buffer_factory(), RIGHT_BUFFER_CREATE)],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));
    let subscriber_b = slime_rt::spawn(resolve_executable(b"executable:fabric-subscriber-b"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
    // The declared interposition proxy precedes the fabric for the same reason
    // every ring participant does: the fabric is granted a supervision handle
    // naming it, and a handle cannot exist before its task. A native Endpoint
    // reports no peer death, so this handle is the only way the broker can
    // observe a hop through a dead proxy rather than blocking on it forever.
    let intruder = slime_rt::spawn(resolve_executable(b"executable:fabric-intruder"), &[])
        .unwrap_or_else(|_| slime_rt::exit(1));
    // What init still passes is exactly what the generation cannot place: the
    // shared-buffer factory it holds, and one supervision handle per ring
    // participant and declared proxy, which only exist once those tasks do.
    // Matching is positional against ascending declared slot: factory at 1,
    // plane declaring an interposition names the proxy among them. Which those
    // are is a manifest fact, not a build flag: `FABRIC_MINTED_GRANTS` states
    // how many capabilities this child's owner must supply, so the set is
    // sliced to that generated count.
    let grants = [
        grant(resolve_buffer_factory(), RIGHT_BUFFER_CREATE),
        grant(publisher.supervision_slot, RIGHT_SUPERVISE),
        grant(subscriber.supervision_slot, RIGHT_SUPERVISE),
        grant(intruder.supervision_slot, RIGHT_SUPERVISE),
        grant(publisher_b.supervision_slot, RIGHT_SUPERVISE),
        grant(subscriber_b.supervision_slot, RIGHT_SUPERVISE),
    ];
    let without_proxy = [grants[0], grants[1], grants[2], grants[4], grants[5]];
    let declared = declared_minted_grants(b"fabric-service");
    let service = slime_rt::spawn(
        resolve_executable(b"executable:fabric-service"),
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
