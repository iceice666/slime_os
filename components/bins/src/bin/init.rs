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
const CONSOLE_CAPS: [SpawnGrant; 1] = [grant(CONSOLE_OUTPUT_SLOT, RIGHT_RECV)];
const STORAGE_PROBE_READ_CAPS: [SpawnGrant; 1] = [grant(STORAGE_CAPABILITY_SLOT, RIGHT_BLOCK_READ)];
const STORAGE_PROBE_WRITE_CAPS: [SpawnGrant; 1] = [grant(
    STORAGE_CAPABILITY_SLOT,
    RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE,
)];
const STORAGE_PROBE_STORE_CAPS: [SpawnGrant; 1] = [grant(
    OBJECT_STORE_SLOT_0,
    RIGHT_STORE_READ | RIGHT_STORE_WRITE,
)];
const GENERATION_MANAGER_CAPS: [SpawnGrant; 6] = [
    grant(GENERATION_LIST_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
    grant(
        GENERATION_CONTROL_SLOT,
        RIGHT_HEALTH_CONFIRM | RIGHT_BOOT_UPDATE,
    ),
    grant(GENERATION_INSPECT_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
    grant(GENERATION_STAGE_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
    grant(GENERATION_SELECT_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
    grant(GENERATION_ROLLBACK_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
];
const RECOVERY_CAPS: [SpawnGrant; 2] = [
    grant(2, RIGHT_BOOT_UPDATE),
    grant(3, RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE),
];

fn dango_caps() -> [SpawnGrant; 6] {
    [
        grant(DANGO_SPAWN_SLOT, RIGHT_SEND | RIGHT_RECV),
        grant(DANGO_OUTPUT_SLOT, RIGHT_SEND | RIGHT_TRANSFER),
        grant(INPUT_SLOT, RIGHT_INPUT_READ),
        grant(
            DIRECTORY_SLOT,
            RIGHT_DIRECTORY_READ | RIGHT_DIRECTORY_DERIVE | RIGHT_TRANSFER,
        ),
        grant(ENDPOINT_FACTORY_SLOT, RIGHT_ENDPOINT_CREATE),
        grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
    ]
}

fn spawn_service_caps() -> [SpawnGrant; 5] {
    [
        grant(SERVICE_SPAWN_SLOT, RIGHT_SEND | RIGHT_RECV),
        grant(SYSINFO_SLOT, RIGHT_EXEC | RIGHT_SPAWN),
        grant(ECHO_AGENT_SLOT, RIGHT_EXEC | RIGHT_SPAWN),
        grant(ENDPOINT_FACTORY_SLOT, RIGHT_ENDPOINT_CREATE),
        grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
    ]
}

fn filesystem_caps() -> [SpawnGrant; 2] {
    [
        grant(DIRECTORY_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
        grant(OBJECT_STORE_SLOT, RIGHT_STORE_READ | RIGHT_STORE_WRITE),
    ]
}

const DIRECTORY_PROBE_CAPS: [SpawnGrant; 2] = [
    grant(DIRECTORY_CLIENT_SLOT, RIGHT_SEND | RIGHT_RECV),
    grant(
        DIRECTORY_SLOT,
        RIGHT_TRANSFER
            | RIGHT_DIRECTORY_READ
            | RIGHT_DIRECTORY_WRITE
            | RIGHT_DIRECTORY_LIST
            | RIGHT_DIRECTORY_DERIVE,
    ),
];

const GENERATION_LIST_CAPS: [SpawnGrant; 1] =
    [grant(GENERATION_LIST_CLIENT_SLOT, RIGHT_SEND | RIGHT_RECV)];
const GENERATION_INSPECT_CAPS: [SpawnGrant; 1] = [grant(
    GENERATION_INSPECT_CLIENT_SLOT,
    RIGHT_SEND | RIGHT_RECV,
)];
const GENERATION_STAGE_CAPS: [SpawnGrant; 1] =
    [grant(GENERATION_STAGE_CLIENT_SLOT, RIGHT_SEND | RIGHT_RECV)];
const GENERATION_SELECT_CAPS: [SpawnGrant; 1] = [grant(
    GENERATION_SELECT_CLIENT_SLOT,
    RIGHT_SEND | RIGHT_RECV,
)];
const GENERATION_ROLLBACK_CAPS: [SpawnGrant; 1] = [grant(
    GENERATION_ROLLBACK_CLIENT_SLOT,
    RIGHT_SEND | RIGHT_RECV,
)];
const POWERBOX_CHOOSER_CAPS: [SpawnGrant; 3] = [
    grant(POWERBOX_SERVICE_SLOT, RIGHT_SEND | RIGHT_RECV),
    grant(
        DIRECTORY_SLOT,
        RIGHT_DIRECTORY_READ | RIGHT_DIRECTORY_DERIVE | RIGHT_TRANSFER,
    ),
    grant(INPUT_SLOT, RIGHT_INPUT_READ),
];
// Init's slot numbers come from the generation's boot layout, emitted by
// `scripts/build/boot_layout.py` into `OUT_DIR` at component build time. The
// kernel places each capability at the slot the same layout names, so the
// component that uses a slot and the kernel that fills it read one source
// rather than two hand-maintained lists that agreed by inspection (B10).
//
// A label this generation does not declare is `SLOT_ABSENT`. Every generation
// emits the same set of constant *names*, so a body gated by a check flag still
// compiles under every profile; only the values differ.
include!(concat!(env!("OUT_DIR"), "/boot_layout.rs"));

// The transfer pair is not in the layout: bootstrap appends it past the
// layout's high-water mark, and only when the platform enumerates both block
// devices, so no generation can declare its slots.
//
// 61 and 62 are also where generation 13 puts the fabric clock channel and 14
// puts the call phase channels. There is no conflict: transfer runs under
// generation 9, whose table gives all four of those labels `SLOT_ABSENT`, and
// the two profiles that do declare 61/62 never carry a transfer pair — their
// layouts already reach 63 slots, so bootstrap's append would exceed
// `MAX_CAPS` and trip the assert rather than overwrite anything.
const TRANSFER_RECEIVER_SLOT: u32 = 61;
const TRANSFER_SOURCE_SLOT: u32 = 62;

const POWERBOX_PROBE_CAPS: [SpawnGrant; 1] = [grant(POWERBOX_CLIENT_SLOT, RIGHT_SEND | RIGHT_RECV)];

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

fn storage_executable_slot() -> u32 {
    match option_env!("SLIME_GENERATION_NUMBER") {
        Some("2") => STORAGE_WRITER_SLOT,
        Some("3") => STORAGE_FAULT_PROBE_SLOT,
        Some("4") => STORAGE_STORE_PROBE_SLOT,
        _ => STORAGE_PROBE_SLOT,
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
            drive_probe_plane(
                SEL4_TRANSFER_PROBE_SLOT,
                b"[init] transfer probe spawned\n",
                b"transfer",
            );
            slime_rt::debug_write(b"[init] transfer plane complete\n");
            slime_rt::exit(0)
        }
        action::INPUT => {
            drive_probe_plane(
                SEL4_INPUT_PROBE_SLOT,
                b"[init] input probe spawned\n",
                b"input",
            );
            slime_rt::debug_write(b"[init] input plane complete\n");
            slime_rt::exit(0)
        }
        action::DIRECTORY => {
            drive_probe_plane(
                SEL4_DIRECTORY_PROBE_SLOT,
                b"[init] directory probe spawned\n",
                b"directory",
            );
            slime_rt::debug_write(b"[init] directory plane complete\n");
            slime_rt::exit(0)
        }
        action::RECOVERY => {
            drive_probe_plane(
                SEL4_RECOVERY_PROBE_SLOT,
                b"[init] recovery probe spawned\n",
                b"recovery",
            );
            slime_rt::debug_write(b"[init] recovery plane complete\n");
            slime_rt::exit(0)
        }
        action::ROLLBACK => {
            drive_probe_plane(
                SEL4_ROLLBACK_PROBE_SLOT,
                b"[init] rollback probe spawned\n",
                b"rollback",
            );
            slime_rt::debug_write(b"[init] rollback plane complete\n");
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
    if option_env!("SLIME_RECOVERY_IMAGE") == Some("1") {
        slime_rt::debug_write(b"[init] launching recovery graph\n");
        spawn_or_fail(1, &RECOVERY_CAPS);
        return;
    }
    // The x86 oracle's QoS composition. Its seL4 counterpart is selected by the
    // `QOS` boot action and composed by `compose_declared_graph`, so this flag
    // now names exactly one plane.
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
    // The x86 oracle's operation composition; the seL4 plane is the
    // `OPERATION` boot action.
    if option_env!("SLIME_FABRIC_OPERATION_CHECK") == Some("1") {
        launch_fabric_operations();
        slime_rt::debug_write(b"[init] fabric operation complete\n");
        slime_rt::exit(0);
    }
    // The x86 oracle's visibility composition; the seL4 plane is the
    // `VISIBILITY` boot action.
    if option_env!("SLIME_FABRIC_VISIBILITY_CHECK") == Some("1") {
        launch_fabric_graph();
        slime_rt::debug_write(b"[init] fabric visibility complete\n");
        slime_rt::exit(0);
    }
    // Every seL4 plane is selected by the authenticated boot action the root
    // delivered at activation, not by a build flag. `PRODUCT` returns here and
    // launches the declared graph below; every other action composes its plane
    // and exits.
    compose_declared_graph(startup_arg);
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
        spawn_or_fail(FILESYSTEM_SERVICE_SLOT, &filesystem_caps());
        spawn_or_fail(DIRECTORY_PROBE_SLOT, &DIRECTORY_PROBE_CAPS);
    }
    if option_env!("SLIME_POWERBOX_CHECK") == Some("1") {
        spawn_or_fail(CONSOLE_SLOT, &CONSOLE_CAPS);
        spawn_or_fail(POWERBOX_CHOOSER_SLOT, &POWERBOX_CHOOSER_CAPS);
        spawn_and_wait(POWERBOX_PROBE_SLOT, &POWERBOX_PROBE_CAPS);
        slime_rt::debug_write(b"[init] powerbox scenario complete\n");
        slime_rt::exit(0);
    }
    if option_env!("SLIME_GENERATION_CMD_CHECK") != Some("1")
        && option_env!("SLIME_POWERBOX_CHECK") != Some("1")
    {
        spawn_or_fail(CONSOLE_SLOT, &CONSOLE_CAPS);
        // A generation that does not declare dango does not get one. The seL4
        // profile (P5.2) is the first such generation: dango drives the input
        // and directory planes, which `slime-root` does not mediate, so it is
        // not in that graph. Guarding on the layout rather than on a build flag
        // keeps this a fact the generation states — `SLOT_ABSENT` is what the
        // boot layout emits for a label the profile drops — in the same shape
        // as the storage-probe guard below.
        if DANGO_SLOT != SLOT_ABSENT {
            spawn_or_fail(DANGO_SLOT, &dango_caps());
        }
        spawn_or_fail(SPAWN_SERVICE_SLOT, &spawn_service_caps());
        if option_env!("SLIME_DANGO_CHECK") != Some("1")
            && option_env!("SLIME_GENERATION_NUMBER") != Some("9")
        {
            // With no block device attached, bootstrap hands init an ObjectStore
            // fallback in the storage slot instead of a block capability, so the
            // storage-probe's BLOCK_READ derive is rejected. That is the expected
            // no-disk case (the kernel's `on_idle` already tolerates an absent
            // storage-probe), so skip it rather than aborting the whole graph.
            //
            // The selected generation's layout names exactly one storage probe
            // executable. Product boots name none, so the resolved slot is
            // `SLOT_ABSENT`; storage scenarios name their writer/fault/store
            // executable rather than the read probe's label.
            let storage_slot = storage_executable_slot();
            if storage_slot != SLOT_ABSENT {
                spawn_optional_storage(storage_slot, storage_caps());
            }
        }
    }
    // As for dango above: the generation-manager drives the generation-management
    // plane, which the seL4 profile's root task does not mediate, so that
    // profile's layout leaves this label absent.
    if option_env!("SLIME_GENERATION_CMD_CHECK") != Some("1")
        && GENERATION_MANAGER_SLOT != SLOT_ABSENT
    {
        spawn_or_fail(GENERATION_MANAGER_SLOT, &GENERATION_MANAGER_CAPS);
    }
    if option_env!("SLIME_GENERATION_CMD_CHECK") == Some("1") {
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
/// Subscribers, by participant index. Their supervision handles must exist
/// before the fabric starts: a downstream loan names its receiver through a
/// `RIGHT_SUPERVISE` capability rather than an ambient task id.
const BOOT_SUBSCRIBERS: [usize; 3] = [1, 3, 4];

const fn boot_executable_slot(participant: usize) -> u32 {
    BOOT_FIRST_EXECUTABLE_SLOT + participant as u32
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

/// Drive the P5.4.9 full-graph boot: every C8 role in one generation.
///
/// Only reachable under `SLIME_SEL4_BOOT_CHECK`, whose generation is
/// `contracts/generation/v1/fixtures/sel4-boot.zti`. That generation also sets
/// the oracle's `SLIME_FABRIC_BOOT_CHECK`, so all sixteen participants, the
/// fabric, and its two route workers are the x86 binaries unmodified — every one
/// of them selects its full-graph behaviour through `fabric_boot::active`.
///
/// **What differs from `launch_fabric_boot`, and why.** The oracle's boot layout
/// numbers both halves of all sixteen control channels, because its kernel
/// materializes a declared channel into the bootstrap component's layout slots.
/// This root numbers a launched component's declared ends from its own cursor,
/// which resumes above the factory grants staging installed — so a declared
/// control reaches the fabric at a slot no `FABRIC_FIRST_CONTROL_SLOT + index`
/// describes. Every other seL4 plane mints its controls for exactly this reason;
/// this one mints sixteen instead of four or five.
///
/// Everything else is the oracle's composition, in its order: subscribers first
/// so the fabric can be granted their supervision handles, the fabric next with
/// the two worker executables it spawns itself, then the remaining participants,
/// then one yield so every role request is enqueued before any supervision
/// descriptor follows it on the same channel.
///
/// Init does not exit. The gate's exit condition is the whole graph at healthy
/// blocked idle, so init parks on the fabric's handle — a component terminating
/// here is a failure, not something to wait for.
fn drive_boot_plane() -> ! {
    let mut service_sides = [0u32; BOOT_PARTICIPANTS];
    let mut client_sides = [0u32; BOOT_PARTICIPANTS];
    for index in 0..BOOT_PARTICIPANTS {
        let (service_side, client_side) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
            .unwrap_or_else(|_| fail_boot(b"control endpoint"));
        service_sides[index] = service_side;
        client_sides[index] = client_side;
    }
    slime_rt::debug_write(b"[init] fabric boot control channels minted\n");

    let mut supervision = [0u32; BOOT_PARTICIPANTS];
    for participant in BOOT_SUBSCRIBERS {
        supervision[participant] = spawn_boot_child(participant, client_sides[participant]);
    }
    slime_rt::debug_write(b"[init] fabric boot subscribers spawned\n");

    // Grant order *is* the fabric's slot layout, read from the resolved profile
    // rather than from constants of its own: the two factories, one control per
    // stream participant, the subscriber supervision handles, then the call and
    // operation planes' controls, and last the two worker executables.
    const BOOT_FABRIC_GRANTS: usize = 2 + BOOT_PARTICIPANTS + BOOT_SUBSCRIBERS.len() + 2;
    let mut grants = [SpawnGrant { slot: 0, rights: 0 }; BOOT_FABRIC_GRANTS];
    let mut count = 0;
    let mut push = |slot: u32, rights: Rights| {
        grants[count] = grant(slot, rights);
        count += 1;
    };
    push(ENDPOINT_FACTORY_SLOT, RIGHT_ENDPOINT_CREATE);
    push(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE);
    for side in service_sides.iter().take(BOOT_STREAM_PARTICIPANTS) {
        push(*side, RIGHT_SEND | RIGHT_RECV);
    }
    for participant in BOOT_SUBSCRIBERS {
        push(supervision[participant], RIGHT_SUPERVISE);
    }
    for side in service_sides.iter().skip(BOOT_STREAM_PARTICIPANTS) {
        push(*side, RIGHT_SEND | RIGHT_RECV);
    }
    push(BOOT_CALL_WORKER_SLOT, RIGHT_EXEC | RIGHT_SPAWN);
    push(BOOT_OP_WORKER_SLOT, RIGHT_EXEC | RIGHT_SPAWN);
    let fabric = slime_rt::spawn(FABRIC_SERVICE_SLOT, &grants[..count])
        .unwrap_or_else(|_| fail_boot(b"spawn fabric"));
    slime_rt::debug_write(b"[init] fabric boot service spawned\n");

    // The fabric holds derived copies now. Init releases every service-side
    // half and the subscriber handles it only held to hand on: a spawn grant is
    // a copy, so keeping them would leave sixteen channels with a holder that
    // never reads them and sixteen handles init has no claim on.
    for slot in service_sides {
        if slime_rt::cap_drop(slot) < 0 {
            fail_boot(b"drop copied service side");
        }
    }
    for participant in BOOT_SUBSCRIBERS {
        if slime_rt::cap_drop(supervision[participant]) < 0 {
            fail_boot(b"drop subscriber handle");
        }
        supervision[participant] = 0;
    }

    for participant in 0..BOOT_PARTICIPANTS {
        if !BOOT_SUBSCRIBERS.contains(&participant) {
            supervision[participant] = spawn_boot_child(participant, client_sides[participant]);
        }
    }
    slime_rt::debug_write(b"[init] fabric boot participants spawned\n");

    // One yield, so every participant's role request is enqueued before any
    // supervision descriptor follows it. The brokers read one request then one
    // descriptor per client on the same channel, and a channel is a queue — a
    // descriptor that arrives first is consumed *as* the request.
    slime_rt::yield_now();

    for (participant, handle) in supervision.iter_mut().enumerate() {
        if let Some((direction, route)) = boot_supervision_edge(participant) {
            transfer_supervision(client_sides[participant], *handle, direction, route);
            // `cap_transfer` moves, so the slot is empty; marking it keeps the
            // release below from dropping a handle that no longer exists.
            *handle = 0;
        }
    }
    slime_rt::debug_write(b"[init] fabric boot supervision transferred\n");

    for (participant, handle) in supervision.iter().enumerate() {
        for slot in [boot_executable_slot(participant), client_sides[participant]] {
            if slime_rt::cap_drop(slot) < 0 {
                fail_boot(b"release a bootstrap-only capability");
            }
        }
        if *handle != 0 && slime_rt::cap_drop(*handle) < 0 {
            fail_boot(b"release a participant handle");
        }
    }
    slime_rt::debug_write(b"[init] fabric boot graph launched\n");

    loop {
        slime_rt::wait(&[slime_rt::WaitSource::Supervision(fabric.supervision_slot)]);
    }
}

/// Spawn one full-graph participant with its own minted control endpoint and
/// nothing else, returning the supervision handle init keeps.
fn spawn_boot_child(participant: usize, control: u32) -> u32 {
    slime_rt::spawn(
        boot_executable_slot(participant),
        &[grant(control, RIGHT_SEND | RIGHT_RECV)],
    )
    .unwrap_or_else(|_| fail_boot(b"spawn a boot participant"))
    .supervision_slot
}

fn fail_boot(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] fabric boot fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
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
    for slot in [FABRIC_SUBSCRIBER_B_SLOT, FABRIC_SUBSCRIBER_B_CLIENT_SLOT] {
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

/// Drive the P5.3.1 channel plane: the operations `slime-root` newly mediates,
/// each exercised so its outcome is a serial marker rather than an inference.
///
/// Only reachable under `SLIME_SEL4_CHANNEL_CHECK`, whose generation is
/// `contracts/generation/v1/fixtures/sel4-channel.zti`; see the `.md` beside it
/// for why the two channels are shaped the way they are.
fn drive_channel_plane() {
    // The message is deliberately over sixteen bytes. At or below that bound
    // the transport packs a payload into the fast message registers and the
    // transfer window is never touched, so a shorter line would leave the whole
    // staging path — the part that maps a child's window frame into the root —
    // unexercised.
    const LINE: &[u8] = b"[console] channel plane carried this line\n";
    const _: () = assert!(LINE.len() > 16);
    const _: () = assert!(LINE.len() <= slime_rt::MAX_MSG);

    // Let `console` reach its `recv` first, so the send below lands on a peer
    // that is genuinely parked in the kernel rather than one that has not run
    // yet. Both components are runnable from activation and nothing orders
    // them, so without this the send is a fast-path enqueue to a queue nobody
    // is waiting on and the wake path is never taken.
    //
    // `yield_now` is the whole mechanism a component has for this: it holds no
    // capability naming `console` and cannot observe another task's state. The
    // count is a bound, not a timing assumption — the marker the gate asserts
    // is the root's own `parked` line, so a boot where this proved too few
    // fails rather than passing with the arm skipped.
    for _ in 0..PEER_PARK_YIELDS {
        slime_rt::yield_now();
    }

    // One send to a component parked in `recv`. `console` is blocked in the
    // kernel when this lands, because the root holds its reply rather than
    // answering `ERR_WOULDBLOCK`, so this is the wake path.
    if slime_rt::send(CONSOLE_SEND_SLOT, LINE, &[]) != slime_rt::ERR_SUCCESS {
        fail(b"send to console");
    }
    slime_rt::debug_write(b"[init] parked receiver sent\n");

    // A capability-carrying send. This slice mediates no transferable logical
    // resource — loans are P5.3.2 — so the refusal is the designed answer, and
    // it must arrive as a bounded error with this component still running.
    if slime_rt::send(CONSOLE_SEND_SLOT, b"caps", &[CONSOLE_SEND_SLOT]) == slime_rt::ERR_SUCCESS {
        fail(b"capability transfer was permitted");
    }
    slime_rt::debug_write(b"[init] capability transfer denied\n");

    // Fill the self-edge past its depth. Nothing drains a queue whose only
    // reader is this task, so the refusal is deterministic rather than a race
    // against a peer.
    let mut queued = 0;
    let mut refused = false;
    for _ in 0..(CHANNEL_DEPTH + 1) {
        match slime_rt::send(SERVICE_SPAWN_SLOT, b"fill", &[]) {
            slime_rt::ERR_SUCCESS => queued += 1,
            slime_rt::ERR_WOULDBLOCK => {
                refused = true;
                break;
            }
            _ => fail(b"unexpected send failure"),
        }
    }
    if !refused || queued != CHANNEL_DEPTH {
        fail(b"a full queue accepted more than its depth");
    }
    slime_rt::debug_write(b"[init] queue full refused\n");

    // A wait on a source that is already ready. The queue above is non-empty,
    // so this must answer at once; parking here would deadlock this component
    // against itself, which is exactly what the readiness probe prevents.
    slime_rt::wait(&[slime_rt::WaitSource::Endpoint(SERVICE_SPAWN_SLOT)]);
    slime_rt::debug_write(b"[init] ready wait answered\n");

    // Drain what was queued, so the receive path is exercised on a queue this
    // component filled itself and the counts in the root's marker balance.
    let mut drained = 0;
    let mut payload = [0u8; slime_rt::MAX_MSG];
    let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    while drained < queued {
        match slime_rt::recv(SERVICE_SPAWN_SLOT, &mut payload, &mut caps) {
            n if n < 0 => fail(b"drain failed"),
            n => {
                if &payload[..n as usize] != b"fill" {
                    fail(b"drained the wrong bytes");
                }
                drained += 1;
            }
        }
    }
    slime_rt::debug_write(b"[init] queue drained\n");

    // Leave `console` parked on an empty channel before exiting, so its
    // reply is owed at the moment its peer dies. Without this the send above
    // is still queued when init exits, console's next `recv` finds it
    // immediately, and the death-wake path is never taken — the graph drains
    // either way, so nothing in the transcript would say the arm was skipped.
    //
    // The yields let console consume the queued message and block again. As
    // with the wait before the first send, the count is a bound rather than a
    // timing assumption: the gate asserts the root's own `woken=1` marker, so a
    // boot where this proved too few fails instead of quietly passing.
    for _ in 0..PEER_PARK_YIELDS {
        slime_rt::yield_now();
    }
}

/// Drive the P5.3.2 loan plane, as the lender.
///
/// Only reachable under `SLIME_SEL4_LOAN_CHECK`, whose generation is
/// `contracts/generation/v1/fixtures/sel4-loan.zti`; see the `.md` beside it.
///
/// This is `sample-lender`'s shape, and deliberately not `sample-lender`
/// itself: that component is spawned by init on x86 and receives its peer
/// through a spawn grant, which this cutover has no mechanism for until P5.3.3.
/// Init stands in as the lender so the *loan* plane can be exercised without
/// the *spawn* plane. The receiver is the real `sample-receiver`, unmodified —
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

    // A loan requires an irreversibly sealed source, so an unsealed one must be
    // refused. Checked before sealing, because afterwards it is unobservable.
    if slime_rt::shared_buffer_loan(buffer.slot, RECEIVER_SLOT, 0, PAYLOAD_LEN).is_ok() {
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
    if slime_rt::shared_buffer_loan(buffer.slot, 63, 0, PAYLOAD_LEN).is_ok() {
        fail_loan(b"an empty slot named a receiver");
    }
    // A slot holding the wrong *kind*. The buffer's own slot is real authority
    // this component holds — it is the source of the loan — and it still names
    // no receiver, so the check is on kind rather than on possession.
    if slime_rt::shared_buffer_loan(buffer.slot, buffer.slot, 0, PAYLOAD_LEN).is_ok() {
        fail_loan(b"a buffer slot named a receiver");
    }
    slime_rt::debug_write(b"[init] unnamed receiver denied\n");

    // A real channel to a real peer, over an edge the generation declared
    // `transferable = false`. Everything else about this loan would succeed —
    // the source is sealed, the receiver is a live task at the other end of a
    // channel this component holds — so the only thing refusing it is the
    // generation's delegation bit, which is what makes that bit load-bearing
    // rather than decorative.
    if slime_rt::shared_buffer_loan(buffer.slot, CONSOLE_SEND_SLOT, 0, PAYLOAD_LEN).is_ok() {
        fail_loan(b"an undelegated channel carried a loan");
    }
    slime_rt::debug_write(b"[init] undelegated loan denied\n");

    let loan = match slime_rt::shared_buffer_loan(buffer.slot, RECEIVER_SLOT, 0, PAYLOAD_LEN) {
        Ok(loan) => loan,
        Err(_) => fail_loan(b"loan"),
    };
    slime_rt::debug_write(b"[init] loan created\n");

    // The loan ceiling is one. A second loan of the same sealed region is
    // therefore refused by the quota rather than by anything about the range.
    if slime_rt::shared_buffer_loan(buffer.slot, RECEIVER_SLOT, 0, PAGE).is_ok() {
        fail_loan(b"loan quota did not bite");
    }
    slime_rt::debug_write(b"[init] loan quota refused\n");

    // Only the descriptor crosses the channel; the payload never enters a
    // queue. The loan capability rides with it, which is the transfer this
    // slice adds — and it is the loan, not the buffer, that moves: the receiver
    // gets a read-only window onto an exact subrange, not the region.
    let descriptor = sample_descriptor(loan.id, PAYLOAD_LEN);
    if slime_rt::send(RECEIVER_SLOT, &descriptor, &[loan.slot]) != slime_rt::ERR_SUCCESS {
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
                slime_rt::wait(&[slime_rt::WaitSource::Endpoint(RECEIVER_SLOT)]);
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
        CONSOLE_SEND_SLOT,
        b"[console] unrelated holder intact\n",
        &[],
    ) != slime_rt::ERR_SUCCESS
    {
        fail_loan(b"notify unrelated holder");
    }
    for _ in 0..PEER_PARK_YIELDS {
        slime_rt::yield_now();
    }

    // Strand one loan in flight, deliberately.
    //
    // Everything above settles cleanly, which leaves one reclamation path
    // untested: a capability parked between its send and a receive that never
    // happens. In flight it belongs to no table — this component can no longer
    // name it and the receiver cannot yet — so neither end's teardown reaches
    // it, and the root's own transit reclamation is the only thing that can.
    //
    // A fault injection found this. With `transit.reclaim` removed the gate
    // still passed, because no boot had ever left a capability in flight; the
    // arm was uncovered and looked covered.
    //
    // `STRAND_SLOT` is a second channel to `console` that console never reads —
    // it loops on slot 0 alone. So this send queues and is never collected,
    // deterministically, rather than racing a peer that might consume it.
    let stranded = match slime_rt::shared_buffer_create(SHARED_BUFFER_FACTORY_SLOT, 1, true) {
        Ok(buffer) => buffer,
        Err(_) => fail_loan(b"strand region"),
    };
    if slime_rt::shared_buffer_seal(stranded.slot) != slime_rt::ERR_SUCCESS {
        fail_loan(b"strand seal");
    }
    let stranded_loan = match slime_rt::shared_buffer_loan(stranded.slot, STRAND_SLOT, 0, 4096) {
        Ok(loan) => loan,
        Err(_) => fail_loan(b"strand loan"),
    };
    if slime_rt::send(STRAND_SLOT, b"stranded", &[stranded_loan.slot]) != slime_rt::ERR_SUCCESS {
        fail_loan(b"strand send");
    }
    slime_rt::debug_write(b"[init] loan stranded in flight\n");
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

/// Depth of one directed logical channel, mirroring
/// `slime-root/src/ipc.rs::CHANNEL_CAPACITY`.
const CHANNEL_DEPTH: usize = 16;

/// The slot init sends to `console` on.
///
/// `dango-output` rather than `console-output`, because the boot layout's two
/// entries describe the two *halves* of a channel and only this one declares
/// the send side. The root requires an end's rights to be contained in the
/// rights its layout slot declares, so the half a component holds and the label
/// it is placed under have to agree.
const CONSOLE_SEND_SLOT: u32 = DANGO_OUTPUT_SLOT;

/// Yields given up so a peer can reach its first `recv` and park. Generous
/// against the two operations `console` issues before blocking — a transfer
/// window bind and the receive itself — while still bounding the wait.
const PEER_PARK_YIELDS: usize = 64;

/// Participants the P5.5.2 stream plane launches: two publishers, two
/// subscribers, and the undeclared component whose denial C8.3 names.
///
/// Five, which is what the *full* C8.4 stream plane needs rather than the
/// smallest graph that carries one sample. The second publisher originates the
/// `>MAX_INLINE_BYTES` sample and spans both routes; the second subscriber
/// stalls, so KEEP_LAST eviction has an observable cost; and the two routes
/// together are what makes the fan-in many-to-many.
const STREAM_PLANE_CLIENTS: usize = 5;

/// Which control pair each participant is handed, by index into the arrays
/// `drive_stream_plane` mints.
///
/// This is **not** the spawn order, and the two must not be conflated. The
/// index fixes the *control-slot* number the fabric addresses a component by
/// (`FIRST_CONTROL_SLOT + index`), so it must match `FABRIC_STREAM_CONTROL_GRANTS`
/// in `scripts/build/build-generation.py` — the service authenticates a caller
/// by which control slot its request arrived on, and a disagreement here would
/// hand one component another's identity. The spawn order is separately
/// constrained by the supervision handles the fabric needs, and is stated at
/// `drive_stream_plane`.
///
/// Named rather than written as literals at each spawn, because a bare `1` and
/// a bare `4` at two distant call sites is exactly how the two orderings would
/// drift into each other.
const STREAM_PUBLISHER: usize = 0;
const STREAM_SUBSCRIBER: usize = 1;
const STREAM_INTRUDER: usize = 2;
const STREAM_PUBLISHER_B: usize = 3;
const STREAM_SUBSCRIBER_B: usize = 4;

/// Grants the spawn plane's widest spawn carries (B15).
///
/// Six, which is B15's own exit-condition number and the size of this file's
/// largest real grant list — `GENERATION_MANAGER_CAPS` and `dango_caps()` are
/// both six. It is over the four records `slime-root` admitted before P5.5.1,
/// which is the whole point: at `GRANT_RECORD_BYTES` each, six records are 96
/// bytes against a 64-byte message bound, so this spawn is refused outright by
/// a root that reads the grant array with the message reader.
const WIDE_SPAWN_GRANTS: usize = 6;

/// The channel to `sample-receiver`, which is also how the loan names its
/// receiver.
///
/// One slot for both because the root resolves the loan's receiver as the task
/// at the other end of this channel — see
/// `slime-root/src/main.rs::serve_buffer_loan` for why that stands in for the
/// supervision handle the retired kernel uses, and what replaces it in P5.3.3.
const RECEIVER_SLOT: u32 = SAMPLE_RECEIVER_SIDE_SLOT;

/// A second channel to `console`, which console never reads: it loops on slot 0
/// alone. Used to strand one loan in flight deterministically — see
/// `drive_loan_plane`'s last block for why that path needs covering.
const STRAND_SLOT: u32 = POWERBOX_CLIENT_SLOT;

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] channel plane fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

/// Drive the P5.3.4 sample plane: `launch_sample_plane`'s composition, on seL4.
///
/// Only reachable under `SLIME_SEL4_SAMPLE_CHECK`, whose generation is
/// `contracts/generation/v1/fixtures/sel4-sample.zti`; see the `.md` beside it.
///
/// Both components are **unmodified** — the same `sample-lender` and
/// `sample-receiver` the x86 oracle's `just sample_plane_live_check` runs, with
/// no seL4 branch in either. That is P5.3's whole claim.
///
/// The one difference from `launch_sample_plane` above is where the channel
/// comes from. There it is a generation-declared edge and init holds both
/// halves at two layout-named slots; here it is minted at runtime through the
/// declared endpoint factory, because a `source == target` grant is a loopback
/// and yields one slot rather than two. The components cannot tell: each
/// receives its half at its own slot 0 either way.
///
/// Spawn order is load-bearing, exactly as it is on x86. The receiver goes
/// first because the lender names its loan receiver through a `RIGHT_SUPERVISE`
/// handle, which cannot exist until the receiver does.
fn drive_sample_plane() {
    let (lender_side, receiver_side) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_sample(b"endpoint create"));

    let receiver = slime_rt::spawn(
        SAMPLE_RECEIVER_SLOT,
        &[grant(receiver_side, RIGHT_SEND | RIGHT_RECV)],
    )
    .unwrap_or_else(|_| fail_sample(b"spawn receiver"));

    // The lender's three grants, in the order `sample-lender.rs` compiles
    // against: `PEER_SLOT = 0`, `FACTORY_SLOT = 1`, `RECEIVER_SLOT = 2`. The
    // component never learns those numbers — the order of this list is what
    // fixes them — and this is the same list `launch_sample_plane` passes.
    let lender = slime_rt::spawn(
        SAMPLE_LENDER_SLOT,
        &[
            grant(lender_side, RIGHT_SEND | RIGHT_RECV),
            grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
            grant(receiver.supervision_slot, RIGHT_SUPERVISE),
        ],
    )
    .unwrap_or_else(|_| fail_sample(b"spawn lender"));

    // B14: init's declared budget is two, and both are now live. A third spawn
    // must be refused by the generation's own number rather than by a global
    // table size — and refused as `ERR_OUT_OF_MEMORY`, which is what
    // distinguishes "you have reached your ceiling" from "you named something
    // you do not hold".
    if slime_rt::spawn(SAMPLE_RECEIVER_SLOT, &[]) != Err(slime_rt::ERR_OUT_OF_MEMORY) {
        fail_sample(b"spawn budget did not bite");
    }
    slime_rt::debug_write(b"[init] spawn budget refused\n");

    for handle in [receiver.supervision_slot, lender.supervision_slot] {
        loop {
            match slime_rt::supervision_status(handle) {
                Ok(None) => slime_rt::wait(&[slime_rt::WaitSource::Supervision(handle)]),
                Ok(Some(slime_rt::Termination::Exit(0))) => break,
                _ => fail_sample(b"a sample component did not exit cleanly"),
            }
        }
    }

    // B14's second half: the budget is a *live-child* cap, not a lifetime one.
    // Both children have now exited, so the ceiling that refused above must
    // admit again — which it can only do if a dead task stops being counted.
    //
    // This is the arm that distinguishes the two readings. A budget derived
    // from a table that never releases its dead would still have refused the
    // spawn above, and would still refuse here; only a live-child count
    // recovers.
    //
    // The spawn is *authorized* and then immediately unwound: the point is
    // whether the ceiling admits it, not what the child does. Granting it no
    // channel means it would fail its own `recv`, so the handle is dropped and
    // the child left to exit on its own — the gate scopes its component-failure
    // scan to the composition above, which has already completed.
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
/// Only reachable under `SLIME_SEL4_STREAM_CHECK`, whose generation is
/// `contracts/generation/v1/fixtures/sel4-stream.zti`; see the `.md` beside it.
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
/// `qos` selects the QoS plane rather than the plain stream plane. Both
/// compose here; the QoS boot additionally mints a capability-routed clock.
/// The distinction is the generation's own `bootAction`, delivered at
/// activation, so the x86 and seL4 QoS generations no longer have to be told
/// apart by pairing two build flags. `fabric-service`, `fabric-publisher-b`,
/// and `fabric-subscriber-b` still select their QoS *behaviour* from the
/// oracle's `SLIME_FABRIC_QOS_CHECK`, which keeps the three participants
/// byte-identical between the planes as `check-sel4-stream-plane.py` demands.
fn drive_stream_plane(qos: bool) {
    // One control pair per participant, all minted before anything is spawned.
    // Init keeps the service half of each to hand the fabric and gives each
    // client its own half, so no two clients share an identity.
    //
    // Minting up front is what lets the *spawn* order differ from the *slot*
    // order. The fabric numbers its controls `FIRST_CONTROL_SLOT + index` in
    // `FABRIC_STREAM_CONTROL_GRANTS`'s order, while the graph requires the
    // subscribers to be constructed first; both hold because the pairs exist
    // before either ordering is applied.
    let mut service_sides = [0u32; STREAM_PLANE_CLIENTS];
    let mut client_sides = [0u32; STREAM_PLANE_CLIENTS];
    for index in 0..STREAM_PLANE_CLIENTS {
        let (service_side, client_side) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
            .unwrap_or_else(|_| fail_stream(b"control endpoint"));
        service_sides[index] = service_side;
        client_sides[index] = client_side;
    }
    // B17's subject, minted here for the same reason: it must be granted at
    // spawn, and the arm that uses it runs in the child. See the grant below.
    let (_probe_anchor, probe_narrowed) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_stream(b"transfer probe endpoint"));
    let (probe_carrier_send, probe_carrier_recv) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_stream(b"transfer probe carrier"));
    // P5.4.5's clock. Minted only for the QoS plane, and with the same
    // `endpoint_create` every other control pair uses, so the plane that does
    // not declare it mints nothing and its channel count is unchanged.
    //
    // Init keeps neither end: the service half is grant 9 to the fabric and the
    // client half is grant 3 to `fabric-publisher-b`, which is what drives the
    // scheduled boundaries. Both are ordinary spawn grants, so nothing here
    // depends on B25's post-spawn introduction.
    let (time_service, time_client) = if qos {
        slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
            .unwrap_or_else(|_| fail_stream(b"qos time endpoint"))
    } else {
        (0, 0)
    };
    slime_rt::debug_write(b"[init] fabric control channels minted\n");

    // Both subscribers first: the fabric is granted a supervision handle naming
    // each, and a handle cannot name a task that does not exist.
    let subscriber = slime_rt::spawn(
        FABRIC_SUBSCRIBER_SLOT,
        &[grant(
            client_sides[STREAM_SUBSCRIBER],
            RIGHT_SEND | RIGHT_RECV,
        )],
    )
    .unwrap_or_else(|_| fail_stream(b"spawn subscriber"));
    let subscriber_b = slime_rt::spawn(
        FABRIC_SUBSCRIBER_B_SLOT,
        &[grant(
            client_sides[STREAM_SUBSCRIBER_B],
            RIGHT_SEND | RIGHT_RECV,
        )],
    )
    .unwrap_or_else(|_| fail_stream(b"spawn subscriber-b"));

    // The fabric's own authority, in exactly the order and shape
    // `launch_fabric_graph` gives it on x86: an endpoint factory to mint route
    // halves with, a shared-buffer factory for the one copy each large sample
    // makes, one control endpoint per client, and one supervision handle per
    // subscriber. Both factories are *narrowing copies* of init's own — a
    // factory is not an endpoint, so granting one does not move it — which is
    // why init can hand the same authority on and keep it.
    //
    // Grant order *is* the fabric's slot layout: `FACTORY_SLOT = 0`,
    // `BUFFER_FACTORY_SLOT = 1`, the controls from `FABRIC_FIRST_CONTROL_SLOT`,
    // and the supervision handles after them — all read from the generated
    // profile, so a hole here would shift every slot the service addresses.
    //
    // P5.4.5 adds one more, and its position is not free: `fabric-service`
    // reads its clock at a literal `TIME_SLOT = 9`, and the nine grants above
    // fill 0..=8 exactly, so the time channel is grant 9 or it is nothing. The
    // array is sized from the plane rather than a constant so the two cannot
    // drift — a participant added above moves this grant and breaks the build
    // rather than silently handing the fabric a control endpoint where it
    // expects a clock.
    const STREAM_GRANTS: usize = 4 + STREAM_PLANE_CLIENTS;
    const QOS_GRANTS: usize = STREAM_GRANTS + 1;
    const _: () = assert!(STREAM_GRANTS == 9);
    let mut grants = [grant(ENDPOINT_FACTORY_SLOT, RIGHT_ENDPOINT_CREATE); QOS_GRANTS];
    grants[1] = grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE);
    for (index, service_side) in service_sides.iter().enumerate() {
        grants[2 + index] = grant(*service_side, RIGHT_SEND | RIGHT_RECV);
    }
    grants[2 + STREAM_PLANE_CLIENTS] = grant(subscriber.supervision_slot, RIGHT_SUPERVISE);
    grants[3 + STREAM_PLANE_CLIENTS] = grant(subscriber_b.supervision_slot, RIGHT_SUPERVISE);
    grants[STREAM_GRANTS] = grant(time_service, RIGHT_SEND | RIGHT_RECV);
    // The QoS plane grants all ten; every other stream plane grants the first
    // nine and never mints the clock, which is what keeps `sel4_stream_check`'s
    // observed layout byte-for-byte unchanged.
    let fabric = slime_rt::spawn(
        FABRIC_SERVICE_SLOT,
        if qos {
            &grants[..]
        } else {
            &grants[..STREAM_GRANTS]
        },
    )
    .unwrap_or_else(|_| fail_stream(b"spawn fabric"));
    slime_rt::debug_write(b"[init] fabric service spawned\n");

    // Let both subscribers reach the fabric before either publisher exists
    // (B18). Same device `launch_fabric_graph` uses on x86, and needed here for
    // a sharper reason than message ordering: `deliver` refuses a subscriber
    // whose `matched_publishers` is zero, and `refresh_matches` counts only
    // publishers the fabric has *already provisioned*. A subscriber that asks
    // after `fabric-publisher` has finished therefore matches nothing, is
    // delivered nothing, and loses nothing — so `fabric-subscriber-b` fails
    // its own `the stall was never reported as loss` assertion, on a boot where
    // the fabric behaved correctly.
    //
    // The subscribers were spawned first because the fabric needs their
    // supervision handles, but spawning is not running: without this they are
    // merely *created* before the publishers, and which one reaches its control
    // endpoint first is a scheduling detail. One yield is enough, and for the
    // reason the x86 comment gives — `SYS_YIELD` puts the caller at the back of
    // a FIFO ready queue, so every task spawned above runs until it blocks, and
    // a subscriber blocks in `recv` only after its request is enqueued.
    slime_rt::yield_now();

    // B17: the transfer contract's subset test needs a capability holding
    // `RIGHT_TRANSFER` that is *strictly narrower than its kind admits*, and a
    // spawn grant is what produces one — the requested mask is installed
    // verbatim, so this end lands as send+transfer where `Endpoint` admits
    // send+recv+transfer.
    //
    // No other capability in this graph has that shape. A provisioned route
    // role carries no transfer bit at all, a factory carries its one operation
    // right, and an endpoint straight from `endpoint_create` holds exactly what
    // its kind admits — so a widening mask on any of them is refused by an
    // earlier rule and never reaches the subset test.
    //
    // Granted to the publisher because that component already owns this graph's
    // other two transfer-rule denials, so all three sit together and each says
    // which rule it proves. The end belongs to no route and carries no traffic;
    // the other half stays with init and is never used, since what is under
    // test is the refusal rather than a delivery.
    let publisher = slime_rt::spawn(
        FABRIC_PUBLISHER_SLOT,
        &[
            grant(client_sides[STREAM_PUBLISHER], RIGHT_SEND | RIGHT_RECV),
            grant(probe_narrowed, RIGHT_SEND | RIGHT_TRANSFER),
            grant(probe_carrier_send, RIGHT_SEND | RIGHT_RECV),
            grant(probe_carrier_recv, RIGHT_SEND | RIGHT_RECV),
        ],
    )
    .unwrap_or_else(|_| fail_stream(b"spawn publisher"));
    // `fabric-publisher-b` originates the `>MAX_INLINE_BYTES` sample, so it
    // needs a buffer factory of its own and a supervision handle naming the
    // fabric: its upstream loan names the fabric as receiver by capability.
    // Its slot order is the component's own `CONTROL_SLOT`/`FACTORY_SLOT`/
    // `FABRIC_SLOT`.
    //
    // P5.4.5 adds its clock as grant 3, matching `fabric-publisher-b`'s own
    // `TIME_SLOT = 3`. It drives the scheduled boundaries — deadline, lifespan,
    // liveliness, lease — so the component that publishes is also the one that
    // says what time it is, which is how the oracle's QoS gate wires it.
    let publisher_b_grants = [
        grant(client_sides[STREAM_PUBLISHER_B], RIGHT_SEND | RIGHT_RECV),
        grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
        grant(fabric.supervision_slot, RIGHT_SUPERVISE),
        grant(time_client, RIGHT_SEND | RIGHT_RECV),
    ];
    let publisher_b = slime_rt::spawn(
        FABRIC_PUBLISHER_B_SLOT,
        if qos {
            &publisher_b_grants[..]
        } else {
            &publisher_b_grants[..3]
        },
    )
    .unwrap_or_else(|_| fail_stream(b"spawn publisher-b"));
    let intruder = slime_rt::spawn(
        FABRIC_INTRUDER_SLOT,
        &[grant(
            client_sides[STREAM_INTRUDER],
            RIGHT_SEND | RIGHT_RECV,
        )],
    )
    .unwrap_or_else(|_| fail_stream(b"spawn intruder"));

    // Spawn grants are copies. Drop init's retained control ends and the
    // private subset-test endpoints now that every child holds its copy; if
    // init keeps them, peer-death cannot retire the fabric's last queues and
    // the service remains parked forever after all participants finish.
    for slot in
        client_sides
            .into_iter()
            .chain([probe_narrowed, probe_carrier_send, probe_carrier_recv])
    {
        if slime_rt::cap_drop(slot) != slime_rt::ERR_SUCCESS {
            fail_stream(b"drop retained participant authority");
        }
    }
    if qos && slime_rt::cap_drop(time_client) != slime_rt::ERR_SUCCESS {
        fail_stream(b"drop retained time authority");
    }
    slime_rt::debug_write(b"[init] fabric participants spawned\n");

    // Init waits on every participant and on the fabric itself. Waiting rather
    // than spinning is also what makes a fabric that dies wake init instead of
    // going unnoticed.
    for handle in [
        publisher.supervision_slot,
        publisher_b.supervision_slot,
        intruder.supervision_slot,
        subscriber.supervision_slot,
        subscriber_b.supervision_slot,
        fabric.supervision_slot,
    ] {
        loop {
            match slime_rt::supervision_status(handle) {
                Ok(None) => slime_rt::wait(&[slime_rt::WaitSource::Supervision(handle)]),
                Ok(Some(slime_rt::Termination::Exit(0))) => break,
                _ => fail_stream(b"a fabric component did not exit cleanly"),
            }
        }
    }
}

/// Which control pair each call participant is handed, by index into the arrays
/// [`drive_call_plane`] mints.
///
/// The order **is** the fabric's control-slot layout: the broker resolves a
/// control by `FABRIC_FIRST_CONTROL_SLOT + index` into `FABRIC_CALL_CLIENTS`,
/// which the generated profile emits in `FABRIC_CALL_CONTROL_GRANTS` order.
/// Reordering these four renames every caller identity the broker
/// authenticates, so they are named rather than open-coded.
const CALL_PLANE_CLIENTS: usize = 4;
const CALL_CLIENT: usize = 0;
const CALL_CLIENT_B: usize = 1;
const CALL_SERVER: usize = 2;
const CALL_TIME: usize = 3;

/// Drive the P5.4.6 call plane: the C8.6 bounded-native-call graph the x86
/// oracle builds — two clients, a server, and a capability-routed clock over
/// one `ParameterCall` route.
///
/// Only reachable under `SLIME_SEL4_CALL_CHECK`, whose generation is
/// `contracts/generation/v1/fixtures/sel4-call.zti`.
///
/// **Why init mints these rather than the generation declaring them.** The
/// first version of this plane declared all five control channels as
/// generation grants, and failed in two independent ways that a spawn-time
/// binding removes at once:
///
/// * the root numbers a launched component's channel ends from its own cursor,
///   which resumes *above* the factory grants staging installed — so the fabric
///   received `[0, 3, 4, 5, 6]`, and no `base + index` describes a set with a
///   hole in it;
/// * `build_generation` sorts grants by `(name, source, target)`, so
///   `fabric-call-client-b-control` sorted ahead of `fabric-call-client-control`
///   and the broker would have bound client B's identity to client A's slot
///   even had the run been contiguous.
///
/// Minting here makes both moot: a spawn grant lands at its index in the
/// requested list (`construct_child` installs `0..count` in order), so the
/// fabric's slots are `FACTORY_SLOT` 0, `BUFFER_FACTORY_SLOT` 1, then the four
/// controls from `FABRIC_FIRST_CONTROL_SLOT` — exactly what the broker
/// compiles against, and exactly how `drive_stream_plane` already works.
///
/// **Spawn order matches the x86 oracle.** A spawn grant is a non-consuming
/// copy on both kernels. Init therefore spawns the fabric first, keeps the
/// participant half of every control pair, spawns each participant, and moves
/// its supervision handle to the broker over that authenticated half. The
/// broker still receives the request first and the matching identity second;
/// no participant needs authority naming itself.
fn drive_call_plane() {
    // One control pair per participant, all minted before anything is spawned,
    // so the spawn order below is free to differ from the slot order above.
    let mut service_sides = [0u32; CALL_PLANE_CLIENTS];
    let mut client_sides = [0u32; CALL_PLANE_CLIENTS];
    for index in 0..CALL_PLANE_CLIENTS {
        let (service_side, client_side) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
            .unwrap_or_else(|_| fail_call(b"control endpoint"));
        service_sides[index] = service_side;
        client_sides[index] = client_side;
    }
    // The clock's phase channel. `fabric-call-time` waits on its slot 1 for
    // each phase marker and only then advances the broker's clock, so the
    // timeout and retry-exhaustion arms fire at a point the scenario chooses
    // rather than whenever the boot happens to reach them.
    let (phase_client, phase_time) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_call(b"time phase endpoint"));
    // Client A and client B coordinate their interleaving over a private pair
    // that reaches the broker not at all — it carries no route traffic, so it
    // is not a fabric edge and the broker never sees it.
    let (client_phase_a, client_phase_b) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_call(b"client phase endpoint"));
    slime_rt::debug_write(b"[init] call control channels minted\n");

    // ---- the fabric, holding one control end per participant ----
    //
    // Grant order *is* the fabric's slot layout, and the broker reads every one
    // of these numbers out of the generated profile rather than a literal.
    // Spawn copies each service side, so init can discard its extra copy while
    // retaining the opposite `client_side` used for the introduction below.
    let mut grants = [grant(ENDPOINT_FACTORY_SLOT, RIGHT_ENDPOINT_CREATE); 2 + CALL_PLANE_CLIENTS];
    grants[1] = grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE);
    for (index, service_side) in service_sides.iter().enumerate() {
        grants[2 + index] = grant(*service_side, RIGHT_SEND | RIGHT_RECV);
    }
    let service = slime_rt::spawn(FABRIC_SERVICE_SLOT, &grants)
        .unwrap_or_else(|_| fail_call(b"spawn fabric"));
    slime_rt::debug_write(b"[init] call fabric spawned\n");
    for slot in service_sides {
        if slime_rt::cap_drop(slot) < 0 {
            fail_call(b"drop copied service side");
        }
    }

    // ---- participants, introduced by their parent ----
    //
    // The participants send their role requests first. Init then sends each
    // participant's supervision handle from the copied participant half of
    // that same control channel. This preserves the broker's trust boundary:
    // the parent vouches for the task identity, and the participant never
    // receives a self-naming handle.
    //
    // Each participant's own slot order is the scenario's:
    // `CONTROL_SLOT` 0, `FACTORY_SLOT` 1, `FABRIC_SUPERVISION_SLOT` 2, then
    // whatever phase channels that component uses.
    let client = slime_rt::spawn(
        FABRIC_CALL_CLIENT_SLOT,
        &[
            grant(client_sides[CALL_CLIENT], RIGHT_SEND | RIGHT_RECV),
            grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
            grant(service.supervision_slot, RIGHT_SUPERVISE),
            grant(phase_client, RIGHT_SEND),
            grant(client_phase_a, RIGHT_SEND | RIGHT_RECV),
        ],
    )
    .unwrap_or_else(|_| fail_call(b"spawn call client"));
    let client_b = slime_rt::spawn(
        FABRIC_CALL_CLIENT_B_SLOT,
        &[
            grant(client_sides[CALL_CLIENT_B], RIGHT_SEND | RIGHT_RECV),
            grant(client_phase_b, RIGHT_SEND | RIGHT_RECV),
        ],
    )
    .unwrap_or_else(|_| fail_call(b"spawn call client-b"));
    let server = slime_rt::spawn(
        FABRIC_CALL_SERVER_SLOT,
        &[
            grant(client_sides[CALL_SERVER], RIGHT_SEND | RIGHT_RECV),
            grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
            grant(service.supervision_slot, RIGHT_SUPERVISE),
        ],
    )
    .unwrap_or_else(|_| fail_call(b"spawn call server"));
    slime_rt::debug_write(b"[init] call participants spawned\n");
    slime_rt::yield_now();
    for (control, supervision, direction) in [
        (
            client_sides[CALL_CLIENT],
            client.supervision_slot,
            boot_contracts::fabric_graph::DIRECTION_CLIENT,
        ),
        (
            client_sides[CALL_CLIENT_B],
            client_b.supervision_slot,
            boot_contracts::fabric_graph::DIRECTION_CLIENT,
        ),
        (
            client_sides[CALL_SERVER],
            server.supervision_slot,
            boot_contracts::fabric_graph::DIRECTION_SERVER,
        ),
    ] {
        transfer_supervision(control, supervision, direction, call_route_identity());
        if slime_rt::cap_drop(control) < 0 {
            fail_call(b"drop introduction side");
        }
    }
    slime_rt::debug_write(b"[init] call supervision delegated\n");

    // The clock last: it advances time only on a phase marker, so nothing it
    // does can fire before the participants are ready to observe it.
    let time = slime_rt::spawn(
        FABRIC_CALL_TIME_SLOT,
        &[
            grant(client_sides[CALL_TIME], RIGHT_SEND | RIGHT_RECV),
            grant(phase_time, RIGHT_RECV),
        ],
    )
    .unwrap_or_else(|_| fail_call(b"spawn call time"));

    // The time component needs no supervision introduction. Drop init's copied
    // participant half after the spawn; the fabric and time task retain the two
    // live ends.
    if slime_rt::cap_drop(client_sides[CALL_TIME]) < 0 {
        fail_call(b"drop time control copy");
    }

    // Every participant runs to a clean exit. Waiting rather than spinning is
    // also what makes a component that dies wake init instead of going
    // unnoticed.
    for handle in [time.supervision_slot, service.supervision_slot] {
        loop {
            match slime_rt::supervision_status(handle) {
                Ok(None) => slime_rt::wait(&[slime_rt::WaitSource::Supervision(handle)]),
                Ok(Some(slime_rt::Termination::Exit(0))) => break,
                _ => fail_call(b"a call component did not exit cleanly"),
            }
        }
    }
}

fn fail_call(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] call plane fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

/// Which control pair each operation participant is handed, by index into the
/// arrays [`drive_operation_plane`] mints.
///
/// The order **is** the fabric's control-slot layout: the broker resolves a
/// control by `FABRIC_FIRST_CONTROL_SLOT + index` into `FABRIC_OPERATION_CLIENTS`,
/// which the generated profile emits in `FABRIC_OPERATION_CONTROL_GRANTS` order.
/// The replacement's channel follows them at
/// `FABRIC_FIRST_CONTROL_SLOT + OP_PLANE_CONTROLS`, which is the literal
/// `fabric-service.rs` passes as `replacement_control`.
const OP_PLANE_CONTROLS: usize = 4;
const OP_CLIENT: usize = 0;
const OP_CLIENT_B: usize = 1;
const OP_SERVER: usize = 2;
const OP_TIME: usize = 3;

/// Drive the P5.4.7 operation plane: the C8.7 bounded-native-operation graph
/// the x86 oracle builds — two clients, a supervised replacement for the
/// second, a server, and a capability-routed clock over the `navigation` route
/// plus client A's private `nav-backup` route.
///
/// Only reachable under `SLIME_SEL4_OPERATION_CHECK`, whose generation is
/// `contracts/generation/v1/fixtures/sel4-operation.zti`. That generation also
/// sets the oracle's `SLIME_FABRIC_OPERATION_CHECK`, so the broker and all five
/// participants are the x86 binaries unmodified; only this composition differs.
///
/// **Why init mints the control channels.** `drive_call_plane`'s reason, which
/// applies unchanged: the root numbers a launched component's channel ends from
/// its own cursor, which resumes above the factory grants staging installed, so
/// a declared control grant reaches the fabric at a slot no `base + index`
/// describes. The grants stay in the manifest because `_control_sources` derives
/// `FABRIC_OPERATION_CLIENTS` — the table the broker maps a control slot to a
/// caller identity with — from exactly those grant names. They name; the minted
/// endpoints authorize.
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
    let mut service_sides = [0u32; OP_PLANE_CONTROLS];
    let mut client_sides = [0u32; OP_PLANE_CONTROLS];
    for index in 0..OP_PLANE_CONTROLS {
        let (service_side, client_side) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
            .unwrap_or_else(|_| fail_operation(b"control endpoint"));
        service_sides[index] = service_side;
        client_sides[index] = client_side;
    }
    // The replacement's control channel. Distinct from client B's, because the
    // broker must be able to admit the replacement on an endpoint the dead
    // participant never held — that is what makes the restarted identity a
    // parent-vouched fact rather than an inherited one.
    let (replacement_service, replacement_control) =
        slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
            .unwrap_or_else(|_| fail_operation(b"replacement endpoint"));
    // Client A and client B interleave over a private pair the broker never
    // sees. The replacement inherits client B's half by copy, which is how the
    // restarted participant resumes the phase conversation its predecessor left.
    let (phase_a, phase_b) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_operation(b"client phase endpoint"));
    // Client A's release for the clock, kept separate from the A/B pair so a
    // phase meant for the clock can never be consumed by client B.
    let (phase_time_client, phase_time_service) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_operation(b"time phase endpoint"));
    let (restart_start_send, restart_start_recv) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_operation(b"restart barrier endpoint"));
    slime_rt::debug_write(b"[init] operation control channels minted\n");

    // ---- the fabric, holding one control end per participant ----
    //
    // Grant order *is* the fabric's slot layout, and the broker reads every one
    // of these numbers out of the generated profile rather than a literal. The
    // buffer factory occupies slot 1 for the same reason it does on the call
    // plane: the control block begins at `FABRIC_FIRST_CONTROL_SLOT`.
    let mut grants = [grant(ENDPOINT_FACTORY_SLOT, RIGHT_ENDPOINT_CREATE); 3 + OP_PLANE_CONTROLS];
    grants[1] = grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE);
    for (index, service_side) in service_sides.iter().enumerate() {
        grants[2 + index] = grant(*service_side, RIGHT_SEND | RIGHT_RECV);
    }
    grants[2 + OP_PLANE_CONTROLS] = grant(replacement_service, RIGHT_SEND | RIGHT_RECV);
    let service = slime_rt::spawn(FABRIC_SERVICE_SLOT, &grants)
        .unwrap_or_else(|_| fail_operation(b"spawn fabric"));
    slime_rt::debug_write(b"[init] operation fabric spawned\n");
    for slot in service_sides {
        if slime_rt::cap_drop(slot) < 0 {
            fail_operation(b"drop copied service side");
        }
    }
    if slime_rt::cap_drop(replacement_service) < 0 {
        fail_operation(b"drop copied replacement side");
    }

    // ---- participants, introduced by their parent ----
    //
    // Each participant's own slot order is the scenario's: `CONTROL_SLOT` 0,
    // then whatever phase channels that component uses.
    let client = slime_rt::spawn(
        FABRIC_OP_CLIENT_SLOT,
        &[
            grant(client_sides[OP_CLIENT], RIGHT_SEND | RIGHT_RECV),
            grant(phase_a, RIGHT_SEND | RIGHT_RECV),
            grant(phase_time_client, RIGHT_SEND),
        ],
    )
    .unwrap_or_else(|_| fail_operation(b"spawn operation client"));
    let client_b = slime_rt::spawn(
        FABRIC_OP_CLIENT_B_SLOT,
        &[
            grant(client_sides[OP_CLIENT_B], RIGHT_SEND | RIGHT_RECV),
            grant(phase_b, RIGHT_SEND | RIGHT_RECV),
        ],
    )
    .unwrap_or_else(|_| fail_operation(b"spawn operation client-b"));
    let server = slime_rt::spawn(
        FABRIC_OP_SERVER_SLOT,
        &[grant(client_sides[OP_SERVER], RIGHT_SEND | RIGHT_RECV)],
    )
    .unwrap_or_else(|_| fail_operation(b"spawn operation server"));
    slime_rt::debug_write(b"[init] operation participants spawned\n");
    slime_rt::yield_now();
    for (control, supervision) in [
        (client_sides[OP_CLIENT], client.supervision_slot),
        (client_sides[OP_CLIENT_B], client_b.supervision_slot),
    ] {
        transfer_supervision(
            control,
            supervision,
            boot_contracts::fabric_graph::DIRECTION_CLIENT,
            operation_route_identity(),
        );
        if slime_rt::cap_drop(control) < 0 {
            fail_operation(b"drop introduction side");
        }
    }
    transfer_supervision(
        client_sides[OP_SERVER],
        server.supervision_slot,
        boot_contracts::fabric_graph::DIRECTION_SERVER,
        operation_route_identity(),
    );
    if slime_rt::cap_drop(client_sides[OP_SERVER]) < 0 {
        fail_operation(b"drop server introduction side");
    }
    slime_rt::debug_write(b"[init] operation supervision delegated\n");

    // ---- the replacement, vouched for before it may speak ----
    let replacement = slime_rt::spawn(
        FABRIC_OP_CLIENT_B_RESTART_SLOT,
        &[
            grant(replacement_control, RIGHT_SEND | RIGHT_RECV),
            grant(phase_b, RIGHT_SEND | RIGHT_RECV),
            grant(restart_start_recv, RIGHT_RECV),
        ],
    )
    .unwrap_or_else(|_| fail_operation(b"spawn operation replacement"));
    transfer_supervision(
        replacement_control,
        replacement.supervision_slot,
        boot_contracts::fabric_graph::DIRECTION_CLIENT,
        operation_route_identity(),
    );
    if slime_rt::cap_drop(replacement_control) < 0 {
        fail_operation(b"drop replacement introduction side");
    }
    slime_rt::debug_write(b"[init] operation replacement introduced\n");

    // The clock last: it advances time only on a phase marker, so nothing it
    // does can fire before the participants are ready to observe it.
    let time = slime_rt::spawn(
        FABRIC_OP_TIME_SLOT,
        &[
            grant(client_sides[OP_TIME], RIGHT_SEND | RIGHT_RECV),
            grant(phase_time_service, RIGHT_RECV),
        ],
    )
    .unwrap_or_else(|_| fail_operation(b"spawn operation time"));

    // Release the replacement. Sent only now, so its role request cannot reach
    // the broker before the whole graph exists and before the original client B
    // has had the chance to produce the retained result the replacement claims.
    loop {
        match slime_rt::send(restart_start_send, &[1], &[]) {
            slime_rt::ERR_SUCCESS => break,
            slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
            _ => fail_operation(b"release the replacement"),
        }
    }
    slime_rt::debug_write(b"[init] operation replacement released\n");

    // Init holds no route capability and no phase end it still needs: every
    // participant received its own copy at spawn. Releasing them here is what
    // makes each channel's liveness a property of its participants rather than
    // of the parent outliving them.
    for slot in [
        client_sides[OP_TIME],
        phase_a,
        phase_b,
        phase_time_client,
        phase_time_service,
        restart_start_send,
        restart_start_recv,
    ] {
        if slime_rt::cap_drop(slot) < 0 {
            fail_operation(b"release a retained plane end");
        }
    }

    // The two handles init still holds: every other one moved to the broker as
    // that participant's parent-vouched identity.
    for handle in [time.supervision_slot, service.supervision_slot] {
        loop {
            match slime_rt::supervision_status(handle) {
                Ok(None) => slime_rt::wait(&[slime_rt::WaitSource::Supervision(handle)]),
                Ok(Some(slime_rt::Termination::Exit(0))) => break,
                _ => fail_operation(b"an operation component did not exit cleanly"),
            }
        }
    }
}

fn fail_operation(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] operation plane fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn fail_stream(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] stream plane fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

/// Drive the P5.4.8 visibility plane: the C8.8 filtered-introspection and
/// declared-interposition graph the x86 oracle builds — the telemetry and
/// diagnostics routes with `fabric-intruder` as the *declared proxy* on the
/// telemetry subscriber's chain.
///
/// Only reachable under `SLIME_SEL4_VISIBILITY_CHECK`, whose generation is
/// `contracts/generation/v1/fixtures/sel4-visibility.zti`. That generation also
/// sets the oracle's `SLIME_FABRIC_VISIBILITY_CHECK`, so the broker and all five
/// participants are the x86 binaries unmodified; only this composition differs.
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
    let mut service_sides = [0u32; STREAM_PLANE_CLIENTS];
    let mut client_sides = [0u32; STREAM_PLANE_CLIENTS];
    for index in 0..STREAM_PLANE_CLIENTS {
        let (service_side, client_side) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
            .unwrap_or_else(|_| fail_visibility(b"control endpoint"));
        service_sides[index] = service_side;
        client_sides[index] = client_side;
    }
    slime_rt::debug_write(b"[init] visibility control channels minted\n");

    // The fabric's own authority. Grant order *is* its slot layout:
    // `FACTORY_SLOT` 0, the buffer factory at 1, then the five controls from
    // `FABRIC_FIRST_CONTROL_SLOT` in `FABRIC_STREAM_CONTROL_GRANTS` order —
    // which is the order `STREAM_PUBLISHER..STREAM_SUBSCRIBER_B` name.
    //
    // The buffer factory is granted even though `visibility_broker::run` drops
    // it immediately: the slot must exist for the controls to start at 2, and
    // dropping authority it will not use is the broker being explicit rather
    // than holding a factory through a plane that allocates nothing.
    let mut grants =
        [grant(ENDPOINT_FACTORY_SLOT, RIGHT_ENDPOINT_CREATE); 2 + STREAM_PLANE_CLIENTS];
    grants[1] = grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE);
    for (index, service_side) in service_sides.iter().enumerate() {
        grants[2 + index] = grant(*service_side, RIGHT_SEND | RIGHT_RECV);
    }
    let fabric = slime_rt::spawn(FABRIC_SERVICE_SLOT, &grants)
        .unwrap_or_else(|_| fail_visibility(b"spawn fabric"));
    slime_rt::debug_write(b"[init] visibility fabric spawned\n");
    for slot in service_sides {
        if slime_rt::cap_drop(slot) < 0 {
            fail_visibility(b"drop copied service side");
        }
    }

    // Each participant holds exactly one capability: its own control endpoint.
    // Every route half it ends up with arrives from the broker at runtime, which
    // is what makes "the proxy relays only its declared route" a statement about
    // provisioning rather than about what init handed out.
    let mut spawned = [None; STREAM_PLANE_CLIENTS];
    for (index, executable) in [
        (STREAM_PUBLISHER, FABRIC_PUBLISHER_SLOT),
        (STREAM_SUBSCRIBER, FABRIC_SUBSCRIBER_SLOT),
        (STREAM_INTRUDER, FABRIC_INTRUDER_SLOT),
        (STREAM_PUBLISHER_B, FABRIC_PUBLISHER_B_SLOT),
        (STREAM_SUBSCRIBER_B, FABRIC_SUBSCRIBER_B_SLOT),
    ] {
        let child = slime_rt::spawn(
            executable,
            &[grant(client_sides[index], RIGHT_SEND | RIGHT_RECV)],
        )
        .unwrap_or_else(|_| fail_visibility(b"spawn visibility participant"));
        if slime_rt::cap_drop(client_sides[index]) < 0 {
            fail_visibility(b"drop copied client side");
        }
        spawned[index] = Some(child.supervision_slot);
    }
    slime_rt::debug_write(b"[init] visibility participants spawned\n");

    // Every participant exits cleanly, the proxy included. C8.8's third check
    // needs the proxy to *die* mid-plane, but a declared relay that completed
    // and then exited is a clean exit — the loss the subscriber observes is the
    // broker seeing its chain endpoint close, not a fault.
    for handle in spawned.iter().chain(&[Some(fabric.supervision_slot)]) {
        let Some(handle) = handle else {
            fail_visibility(b"a participant was never spawned");
        };
        loop {
            match slime_rt::supervision_status(*handle) {
                Ok(None) => slime_rt::wait(&[slime_rt::WaitSource::Supervision(*handle)]),
                Ok(Some(slime_rt::Termination::Exit(0))) => break,
                _ => fail_visibility(b"a visibility component did not exit cleanly"),
            }
        }
    }
}

/// Drive the P5.4.3 powerbox plane (M6.6): a chooser holding directory
/// authority the requester lacks, handing over one narrowed view on selection.
///
/// The probe's single grant is the RPC endpoint. It holds no directory
/// capability at all, which is the milestone's point: the only way it can name
/// an object is for the chooser to mint one and transfer it, and the chooser
/// mints only what the user's selection gesture named.
fn drive_powerbox_plane() {
    let plane: &[u8] = b"powerbox";
    let (chooser_side, probe_side) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_plane(plane, b"rpc endpoint"));

    let chooser = slime_rt::spawn(
        POWERBOX_CHOOSER_SLOT,
        &[grant(chooser_side, RIGHT_SEND | RIGHT_RECV)],
    )
    .unwrap_or_else(|_| fail_plane(plane, b"spawn the chooser"));
    slime_rt::debug_write(b"[init] powerbox chooser spawned\n");

    let probe = slime_rt::spawn(
        POWERBOX_PROBE_SLOT,
        &[grant(probe_side, RIGHT_SEND | RIGHT_RECV)],
    )
    .unwrap_or_else(|_| fail_plane(plane, b"spawn the probe"));
    slime_rt::debug_write(b"[init] powerbox probe spawned\n");

    // Dropped so the chooser's serve loop ends when the probe exits.
    for slot in [chooser_side, probe_side] {
        if slime_rt::cap_drop(slot) < 0 {
            fail_plane(plane, b"release the rpc ends");
        }
    }

    for handle in [probe.supervision_slot, chooser.supervision_slot] {
        loop {
            match slime_rt::supervision_status(handle) {
                Ok(None) => slime_rt::wait(&[slime_rt::WaitSource::Supervision(handle)]),
                Ok(Some(slime_rt::Termination::Exit(0))) => break,
                _ => fail_plane(plane, b"a powerbox component did not exit cleanly"),
            }
        }
    }
}

/// Drive the P5.4.3 dango plane (M6.4): a scripted console session that
/// launches commands through the spawn service.
///
/// Four components and two channels. The grant lists are the components' own
/// slot layouts — `spawn-service.rs` and `dango.rs` compile against fixed
/// positions, and the *order of these lists* is what fixes them, exactly as
/// `drive_sample_plane` fixes the lender's three.
fn drive_dango_plane() {
    let plane: &[u8] = b"dango";
    // This plane's layout places the shared-buffer factory at 4; the base
    // layout's `SHARED_BUFFER_FACTORY_SLOT` is a different number, and passing
    // it would hand children a slot init does not hold.
    const DANGO_BUFFER_FACTORY_SLOT: u32 = 4;

    // Console first: dango sends its output there, and the channel must exist
    // before either end is granted.
    let (console_side, dango_console) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_plane(plane, b"console endpoint"));
    let (spawn_side, dango_spawn) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_plane(plane, b"spawn endpoint"));

    // `console.rs`: RPC end, then the shared-buffer factory.
    let console = slime_rt::spawn(
        CONSOLE_SLOT,
        &[
            grant(console_side, RIGHT_SEND | RIGHT_RECV),
            grant(DANGO_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
        ],
    )
    .unwrap_or_else(|_| fail_plane(plane, b"spawn the console"));

    // `spawn-service.rs`: RPC at 0, then the endpoint factory, then the
    // shared-buffer factory.
    //
    // Its two *executables* are not here. The generation grants them to the
    // spawn service directly, so the root places them in its table above this
    // list — which is what puts them at 1 and 2, the positions
    // `component_slot` bakes into the command profile. Init could not pass them
    // on if it wanted to: it does not hold them.
    // Exactly one grant, and the count is load-bearing: `spawn-service.rs`
    // names its two executables at slots 1 and 2, and the root numbers declared
    // executables above the parent's grant list. One channel end ahead of them
    // is what puts them there. Its endpoint and shared-buffer factories are its
    // own declared grants, landing above the executables at 3 and 4 — which is
    // exactly what `SHARED_BUFFER_FACTORY_SLOT = 4` in that component says.
    let spawn_service = slime_rt::spawn(
        SPAWN_SERVICE_SLOT,
        &[grant(spawn_side, RIGHT_SEND | RIGHT_RECV)],
    )
    .unwrap_or_else(|_| fail_plane(plane, b"spawn the spawn service"));
    slime_rt::debug_write(b"[init] spawn service spawned\n");

    // `dango.rs` compiles against six fixed slots: spawn RPC, console, input,
    // cwd root, endpoint factory, shared-buffer factory.
    //
    // Init places the first two; the rest are the generation's own grants to
    // dango, installed by the root *above* this list in its fixed order —
    // directory, input, factories. That order suits `powerbox-chooser.rs`,
    // which reads a directory before input, and it is the reverse of what
    // dango wants.
    //
    // The two are reconciled by the grant list's *length*, not by the root's
    // order: three entries here push the declared authority to 3..=6, so
    // dango's input lands at 4 rather than 2 — which would be wrong. Two
    // entries put directory at 2 and input at 3, and dango reads input at 2.
    //
    // So this composition cannot satisfy dango with the current order, and the
    // gap is recorded as B30 rather than papered over by renumbering a
    // component the oracle also builds.
    let dango = slime_rt::spawn(
        DANGO_SLOT,
        &[
            grant(dango_spawn, RIGHT_SEND | RIGHT_RECV),
            grant(dango_console, RIGHT_SEND),
        ],
    )
    .unwrap_or_else(|_| fail_plane(plane, b"spawn dango"));
    slime_rt::debug_write(b"[init] dango spawned\n");

    for slot in [console_side, dango_console, spawn_side, dango_spawn] {
        if slime_rt::cap_drop(slot) < 0 {
            fail_plane(plane, b"release the plane's endpoints");
        }
    }

    // Dango exits when its script reaches the escape byte, and its status is
    // *nonzero by design*: the scripted session includes a refused launch
    // (`$(inject)`, denied at profile resolution) and a parse error, so the
    // oracle's own `check-dango.py` expects both and the component reports the
    // last failure. What matters is that it terminated rather than faulted.
    loop {
        match slime_rt::supervision_status(dango.supervision_slot) {
            Ok(None) => {
                slime_rt::wait(&[slime_rt::WaitSource::Supervision(dango.supervision_slot)]);
            }
            Ok(Some(slime_rt::Termination::Exit(_))) => break,
            _ => fail_plane(plane, b"dango faulted rather than exiting"),
        }
    }
    // The services follow once their peer is gone.
    for handle in [spawn_service.supervision_slot, console.supervision_slot] {
        loop {
            match slime_rt::supervision_status(handle) {
                Ok(None) => slime_rt::wait(&[slime_rt::WaitSource::Supervision(handle)]),
                Ok(Some(slime_rt::Termination::Exit(0))) => break,
                _ => fail_plane(plane, b"a dango service did not exit cleanly"),
            }
        }
    }
}

/// Drive the P5.4.3 filesystem plane (M6.3's other half): a service that
/// resolves names in a snapshot tree, and a client that must ask it.
///
/// The same shape as the generation plane — mint one channel, spawn the service
/// first so it is listening, then the client — and for the same reason: the
/// authority each holds is placed by the generation, and init composes only the
/// channel between them.
fn drive_filesystem_plane() {
    let plane: &[u8] = b"filesystem";
    let (service_side, client_side) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_plane(plane, b"rpc endpoint"));

    let service = slime_rt::spawn(
        SEL4_FILESYSTEM_SERVICE_SLOT,
        &[grant(service_side, RIGHT_SEND | RIGHT_RECV)],
    )
    .unwrap_or_else(|_| fail_plane(plane, b"spawn the service"));
    slime_rt::debug_write(b"[init] filesystem service spawned\n");

    let client = slime_rt::spawn(
        DIRECTORY_PROBE_SLOT,
        &[grant(client_side, RIGHT_SEND | RIGHT_RECV)],
    )
    .unwrap_or_else(|_| fail_plane(plane, b"spawn the client"));
    slime_rt::debug_write(b"[init] directory probe spawned\n");

    // Dropped so the service sees `ERR_PEER_DEAD` when the client exits: while
    // init still names an end, the peer looks alive.
    for slot in [service_side, client_side] {
        if slime_rt::cap_drop(slot) < 0 {
            fail_plane(plane, b"release the rpc ends");
        }
    }

    for handle in [client.supervision_slot, service.supervision_slot] {
        loop {
            match slime_rt::supervision_status(handle) {
                Ok(None) => slime_rt::wait(&[slime_rt::WaitSource::Supervision(handle)]),
                Ok(Some(slime_rt::Termination::Exit(0))) => break,
                _ => fail_plane(plane, b"a filesystem component did not exit cleanly"),
            }
        }
    }

    // The root also launches an unconfigured copy of every declared component
    // (P5.2), and `directory-probe` is the oracle's own binary — shared with
    // `just directory_check`, so it carries no seL4 authority probe and fails
    // rather than parking when it finds no capability.
    //
    // That copy is not this plane's subject, and init never held a handle on
    // it, so nothing here observes it. The gate scopes its lifecycle assertion
    // the same way; what is required is the *spawned* client's clean exit,
    // which the loop above waits for.
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
    let plane: &[u8] = b"generation";
    let (manager_side, client_side) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_plane(plane, b"rpc endpoint"));

    // The manager first, so the service is listening before the client's first
    // send. Its single grant is the client-facing end, which doubles as its run
    // token: a root-launched copy holds nothing there and parks.
    let manager = slime_rt::spawn(
        SEL4_GENERATION_MANAGER_SLOT,
        &[grant(manager_side, RIGHT_SEND | RIGHT_RECV)],
    )
    .unwrap_or_else(|_| fail_plane(plane, b"spawn the manager"));
    slime_rt::debug_write(b"[init] generation manager spawned\n");

    let client = slime_rt::spawn(
        SEL4_GENERATION_CLIENT_SLOT,
        &[grant(client_side, RIGHT_SEND | RIGHT_RECV)],
    )
    .unwrap_or_else(|_| fail_plane(plane, b"spawn the client"));
    slime_rt::debug_write(b"[init] generation client spawned\n");

    // Init drops its copies now that both children hold theirs.
    //
    // Not tidiness: `peer_alive` is a property of who still names a queue end,
    // so an init that kept its copies would keep the manager's peer looking
    // alive forever after the client exited — and the manager's serve loop
    // ends on `ERR_PEER_DEAD`. Since B25 a spawn grant is a copy, so dropping
    // here removes only init's name for the ends, not the children's.
    for slot in [manager_side, client_side] {
        if slime_rt::cap_drop(slot) < 0 {
            fail_plane(plane, b"release the rpc ends");
        }
    }

    for handle in [client.supervision_slot, manager.supervision_slot] {
        loop {
            match slime_rt::supervision_status(handle) {
                Ok(None) => slime_rt::wait(&[slime_rt::WaitSource::Supervision(handle)]),
                Ok(Some(slime_rt::Termination::Exit(0))) => break,
                _ => fail_plane(plane, b"a generation component did not exit cleanly"),
            }
        }
    }
}

/// Drive the P5.4.2c store plane: the same composition as the storage plane,
/// over the probe that runs M5.4 policy in userspace.
///
/// Separate generation and separate probe, one driver: what differs between the
/// two planes is which component is spawned and what it proves, not how init
/// composes it.
fn drive_store_plane() {
    drive_probe_plane(
        SEL4_STORE_PROBE_SLOT,
        b"[init] store probe spawned\n",
        b"store",
    );
}

/// Drive the P5.4.2c storage plane: spawn the probe holding its block
/// capability and require a clean exit.
fn drive_storage_plane() {
    drive_probe_plane(
        SEL4_STORAGE_PROBE_SLOT,
        b"[init] storage probe spawned\n",
        b"storage",
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
fn drive_probe_plane(executable: u32, spawned_marker: &[u8], plane: &'static [u8]) {
    // One grant, and it is not the device: the probe's block capability is
    // granted to *it* by the generation, so the root places it in the probe's
    // own table before it runs. Init never holds the device and could not pass
    // it on — which is the authority claim this plane makes.
    //
    // What init does hand over is a run token. The root launches every declared
    // component, so an unconfigured copy of the probe also starts holding the
    // same device capability; the token is the one thing only the spawned
    // instance has, and probing it is how that instance knows to run.
    let (init_side, run_token) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_plane(plane, b"run token endpoint"));
    let probe = slime_rt::spawn(executable, &[grant(run_token, RIGHT_SEND | RIGHT_RECV)])
        .unwrap_or_else(|_| fail_plane(plane, b"spawn the probe"));
    for slot in [init_side, run_token] {
        if slime_rt::cap_drop(slot) < 0 {
            fail_plane(plane, b"release the run token");
        }
    }
    slime_rt::debug_write(spawned_marker);
    loop {
        match slime_rt::supervision_status(probe.supervision_slot) {
            Ok(None) => {
                slime_rt::wait(&[slime_rt::WaitSource::Supervision(probe.supervision_slot)])
            }
            Ok(Some(slime_rt::Termination::Exit(0))) => break,
            _ => fail_plane(plane, b"the probe did not exit cleanly"),
        }
    }
}

fn fail_plane(plane: &[u8], reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] ");
    slime_rt::debug_write(plane);
    slime_rt::debug_write(b" plane fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn fail_visibility(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] visibility plane fail: ");
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
/// executables, hand each one the capabilities its layout names, and observe
/// termination through a supervision handle.
///
/// Only reachable under `SLIME_SEL4_SPAWN_CHECK`, whose generation is
/// `contracts/generation/v1/fixtures/sel4-spawn.zti`; see the `.md` beside it.
///
/// The two children are `console` and `sysinfo`, both **unmodified** — the same
/// binaries the x86 oracle runs. That is the milestone's claim: a component
/// written against the retired kernel's spawn ABI is started by `slime-root`
/// with no seL4 branch in it. `sysinfo` is the useful one to wait on, because
/// it runs to completion and exits 0 of its own accord; `console` loops until
/// its peer dies, which is what makes it the right subject for the
/// still-live arm.
fn drive_spawn_plane() {
    // ---- an ungranted executable slot is refused ----
    //
    // Before any real spawn, so the refusal is against an untouched table.
    // `MAX_CAPS - 1` is inside the table's bounds and this generation grants
    // init nothing there.
    if slime_rt::spawn(63, &[]).is_ok() {
        fail_spawn(b"an empty slot named an executable");
    }
    // A slot holding real authority of the wrong kind. Init holds its endpoint
    // factory here — a capability it genuinely has — so the check is on kind
    // rather than on possession.
    if slime_rt::spawn(ENDPOINT_FACTORY_SLOT, &[]).is_ok() {
        fail_spawn(b"a factory slot named an executable");
    }
    slime_rt::debug_write(b"[init] ungranted executable refused\n");

    // ---- a grant naming authority the parent does not hold ----
    //
    // The rights must be a subset of what init holds at that slot. Init holds
    // the factory with `endpointCreate` alone, so asking to hand on
    // `bufferCreate` as well is asking the root to manufacture authority no
    // generation declared.
    if slime_rt::spawn(
        CONSOLE_SLOT,
        &[grant(
            ENDPOINT_FACTORY_SLOT,
            RIGHT_ENDPOINT_CREATE | RIGHT_BUFFER_CREATE,
        )],
    )
    .is_ok()
    {
        fail_spawn(b"a spawn widened its own grant");
    }
    slime_rt::debug_write(b"[init] widened grant refused\n");

    // The executable slot cannot be handed to the child: that is authority to
    // create this child, and passing it on would let the child re-spawn its own
    // image outside its parent's budget.
    if slime_rt::spawn(CONSOLE_SLOT, &[grant(CONSOLE_SLOT, RIGHT_EXEC)]).is_ok() {
        fail_spawn(b"a child was granted its own executable");
    }
    slime_rt::debug_write(b"[init] self-executable grant refused\n");

    // ---- the real spawn: console, holding the channel half init brokers ----
    //
    // `console.rs` loops on slot 0, which is where its first spawn grant lands.
    // Nothing in that component knows the number: it is fixed by the order of
    // this list, exactly as the retired kernel fixes it.
    let (console_side, console_child_side) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_spawn(b"console endpoint"));
    let console = slime_rt::spawn(CONSOLE_SLOT, &[grant(console_child_side, RIGHT_RECV)])
        .unwrap_or_else(|_| fail_spawn(b"console"));
    slime_rt::debug_write(b"[init] console spawned\n");

    // A live child has no outcome yet, and the query must say so rather than
    // block or invent one.
    match slime_rt::supervision_status(console.supervision_slot) {
        Ok(None) => slime_rt::debug_write(b"[init] live child reports no outcome\n"),
        _ => fail_spawn(b"a live child reported an outcome"),
    };

    // Spawn grants copy authority, matching the x86 oracle. The parent may
    // still use the exact endpoint slot it granted while the child receives
    // the same side at its slot 0.
    match slime_rt::send(console_child_side, b"after", &[]) {
        slime_rt::ERR_SUCCESS | slime_rt::ERR_WOULDBLOCK => {}
        _ => fail_spawn(b"a copied channel end stopped resolving"),
    }
    if slime_rt::send(console_side, b"[console] spawned child reached\n", &[])
        != slime_rt::ERR_SUCCESS
    {
        fail_spawn(b"the peer half stopped working");
    }
    slime_rt::debug_write(b"[init] granted channel end copied\n");

    // ---- spawn and wait: sysinfo runs to completion ----
    //
    // `sysinfo` receives its launch context on slot 0 and exits 0. Init parks
    // in `wait` on the supervision handle and is woken by the child's death,
    // which is the arm no channel can produce: the readiness event is a task
    // ending, not a queue filling.
    //
    // `sysinfo` reads its launch context from slot 0, and no generation edge
    // connects init to it: the channel is minted here, at runtime, through the
    // declared endpoint factory. That is the mechanism `spawn-service` uses on
    // every x86 boot, and it is what "distributes the channel halves init
    // brokers" means — init holds both ends of a fresh pair, hands one to the
    // child at spawn, and writes the context down the half it kept.
    let (service_side, child_side) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_spawn(b"endpoint create"));
    slime_rt::debug_write(b"[init] context channel minted\n");

    // ---- B15: a spawn carrying more grants than one control message holds ----
    //
    // The grant array crosses the transfer window as a staged payload, and
    // until P5.5.1 the root read it with the *message* bound — 64 bytes, four
    // records — where the retired kernel's `sys_spawn` admits sixty-four. Real
    // x86 callers already exceed four: `GENERATION_MANAGER_CAPS` and
    // `dango_caps()` are six each, and `launch_fabric_graph` hands the fabric
    // nine. Every one of them would have been refused here while succeeding on
    // the oracle, which is the one property P5.4 has to be able to claim.
    //
    // Six is B15's own exit-condition number, not a round one. Five more pairs
    // are minted so the list is six *distinct* slots: `preflight_spawn_grants`
    // refuses a repeated slot, and the child must receive six independently
    // addressable capabilities.
    //
    // `child_side` stays first because that is the slot ordering fixes:
    // `sysinfo` reads its launch context from slot 0 and knows nothing of the
    // five behind it.
    let mut retained = [0u32; WIDE_SPAWN_GRANTS - 1];
    let mut granted = [grant(child_side, RIGHT_SEND | RIGHT_RECV); WIDE_SPAWN_GRANTS];
    for (index, keep) in retained.iter_mut().enumerate() {
        let (ours, theirs) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
            .unwrap_or_else(|_| fail_spawn(b"wide grant endpoint"));
        *keep = ours;
        granted[index + 1] = grant(theirs, RIGHT_SEND | RIGHT_RECV);
    }

    let sysinfo =
        slime_rt::spawn(SYSINFO_SLOT, &granted).unwrap_or_else(|_| fail_spawn(b"sysinfo"));
    slime_rt::debug_write(b"[init] sysinfo spawned\n");

    // Every one of the six landed and remains usable by the parent. Filling a
    // copied end may answer success until its queue fills, then `WouldBlock`;
    // either result proves the slot still resolves. The retained peer halves
    // must continue to send as well.
    for slot in granted.iter().map(|record| record.slot) {
        match slime_rt::send(slot, b"copied", &[]) {
            slime_rt::ERR_SUCCESS | slime_rt::ERR_WOULDBLOCK => {}
            _ => fail_spawn(b"a copied grant stopped resolving"),
        }
    }
    for slot in retained {
        if slime_rt::send(slot, b"kept", &[]) != slime_rt::ERR_SUCCESS {
            fail_spawn(b"a peer half stopped working");
        }
    }
    slime_rt::debug_write(b"[init] six grants copied\n");

    // The launch context the child is blocked reading. Sent after the spawn, so
    // it lands on a task that exists; `sysinfo` parks in `recv` until it does.
    let mut command = [0u8; slime_proto::spawn::MAX_COMMAND_BYTES];
    command[..7].copy_from_slice(b"sysinfo");
    let context = slime_proto::spawn::WireSpawnRequest {
        magic: slime_proto::spawn::SPAWN_MAGIC,
        version: slime_proto::spawn::FORMAT_VERSION,
        flags: 0,
        command_len: 7,
        argument_count: 0,
        environment_count: 0,
        capability_roles: 0,
        client_budget: 0,
        command,
        arguments: [0; 8],
        environment: [0; 8],
        grant_rights: 0,
        reserved: [0; 6],
    };
    if slime_rt::send(service_side, &context.encode(), &[]) != slime_rt::ERR_SUCCESS {
        fail_spawn(b"launch context");
    }
    slime_rt::debug_write(b"[init] launch context sent\n");
    loop {
        match slime_rt::supervision_status(sysinfo.supervision_slot) {
            Ok(None) => {
                slime_rt::wait(&[slime_rt::WaitSource::Supervision(sysinfo.supervision_slot)])
            }
            Ok(Some(slime_rt::Termination::Exit(0))) => break,
            _ => fail_spawn(b"sysinfo did not exit cleanly"),
        }
    }
    slime_rt::debug_write(b"[init] sysinfo outcome collected\n");

    // Collecting an outcome consumes the handle, so a second query finds
    // nothing. That is what makes the outcome single-use rather than a fact a
    // parent can re-read forever.
    if slime_rt::supervision_status(sysinfo.supervision_slot).is_ok() {
        fail_spawn(b"a collected handle answered twice");
    }
    slime_rt::debug_write(b"[init] collected handle consumed\n");

    // Dropping the console handle releases init's authority over it while the
    // child is still live. `spawn_or_fail` does exactly this on every x86 boot,
    // so an unimplemented `cap_drop` would abort the product graph.
    if slime_rt::cap_drop(console.supervision_slot) != slime_rt::ERR_SUCCESS {
        fail_spawn(b"dropping a live handle");
    }
    if slime_rt::supervision_status(console.supervision_slot).is_ok() {
        fail_spawn(b"a dropped handle still answered");
    }
    slime_rt::debug_write(b"[init] dropped handle released\n");
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
                Ok(None) => {
                    slime_rt::wait(&[slime_rt::WaitSource::Supervision(child.supervision_slot)])
                }
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
            Ok(None) => {
                slime_rt::wait(&[slime_rt::WaitSource::Supervision(fault.supervision_slot)])
            }
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
/// Only reachable under `SLIME_SEL4_SUPERVISION_CHECK`, whose generation is
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
    // ---- a handle parked in transit, across the crossing ----
    //
    // Init holds *both* ends of this pair and moves the handle to itself,
    // deferring the matching `recv` until after the loop. A capability is
    // parked in `Transit` from the move until the receive, so this puts a
    // supervision handle in the one state where no table holds it — the case a
    // sweep reading only `GraphTables` frees by mistake, and the reason
    // `Transit::holds_supervision` exists.
    //
    // `cap_transfer`, not `send`'s capability attachment: `send` gates on
    // `Resource::is_transferable`, which is `true` for a loan and nothing else,
    // deliberately — that path lets a component redistribute authority at
    // runtime to whoever holds a channel. `cap_transfer` gates on
    // `RIGHT_TRANSFER` held on the capability itself, which is authority the
    // generation placed, and it is the path `transfer_supervision` already uses
    // to hand fabric participants' handles to their workers. The handle carries
    // that right because `sel4-supervision.zti` declares the executable grant
    // transferable.
    //
    // Init as its own receiver, rather than a second component: `cap_transfer`
    // needs a peer that *collects* a capability, and every unmodified component
    // either ignores the caps array or never receives at all.
    let (transit_send, transit_recv) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_supervision(b"transit endpoint"));
    let in_flight = slime_rt::spawn(SUPERVISION_CHILD_SLOT, &[])
        .unwrap_or_else(|_| fail_supervision(b"transit child"));
    // Parked on until it dies, but *not* collected: `wait` is woken by the
    // termination and leaves the handle in place, where `supervision_status`
    // would consume it. That is what this arm needs — a record that exists,
    // owed to a holder, while the loop below runs. A child still running has no
    // record at all, so there would be nothing for the sweep to get wrong.
    slime_rt::wait(&[slime_rt::WaitSource::Supervision(
        in_flight.supervision_slot,
    )]);
    let descriptor = slime_proto::capability_transfer::WireCapabilityTransfer {
        magic: slime_proto::capability_transfer::CAPABILITY_TRANSFER_MAGIC,
        version: slime_proto::capability_transfer::FORMAT_VERSION,
        status: 0,
        flags: 0,
        object_kind: slime_proto::capability_transfer::OBJECT_KIND_SUPERVISION,
        direction: 0,
        rights_mask: RIGHT_SUPERVISE,
        route_identity: [0u8; 32],
    };
    // ---- B25: a second handle naming a task already supervised ----
    //
    // Asserted here because this is the one plane where init holds a supervision
    // handle it has not yet given away. The derived handle must be usable on its
    // own *and* leave the source usable, which is the whole point: a spawn grant
    // copies but must precede the child, and `cap_transfer` moves, so before this
    // operation a parent could not introduce one child to two others.
    let derived = slime_rt::supervision_derive(in_flight.supervision_slot)
        .unwrap_or_else(|_| fail_supervision(b"derive a second supervision handle"));
    if derived == in_flight.supervision_slot {
        fail_supervision(b"derive returned the source slot");
    }
    // Both name the same child, and that child has already terminated above, so
    // each handle independently reports the same outcome. Querying the derived
    // one first proves it carries real authority rather than being a placeholder;
    // `supervision_status` consumes the slot it answers, which is why the source
    // is still intact for the transfer below.
    if !matches!(slime_rt::supervision_status(derived), Ok(Some(_))) {
        fail_supervision(b"derived handle reported no outcome");
    }
    slime_rt::debug_write(b"[init] second supervision handle derived\n");
    if slime_rt::cap_transfer(
        transit_send,
        in_flight.supervision_slot,
        &descriptor.encode(),
    ) != slime_rt::ERR_SUCCESS
    {
        fail_supervision(b"parking a handle in transit");
    }
    // The handle is gone from init's own table: a capability send is a move,
    // so from here until the `recv` below it is held by `Transit` alone.
    if slime_rt::supervision_status(in_flight.supervision_slot).is_ok() {
        fail_supervision(b"a sent handle stayed in the sender's table");
    }
    slime_rt::debug_write(b"[init] supervision handle parked in transit\n");

    // ---- a handle init keeps across the crossing ----
    //
    // Waited on but deliberately not collected, for the same reason: the record
    // must exist, and be owed to a live holder, throughout the loop. `wait`
    // leaves the handle installed; only `supervision_status` consumes it.
    //
    // Spawned *after* the transfer above, and that ordering matters: the
    // transfer frees the slot the first handle occupied, and slot assignment
    // takes the lowest free slot, so a spawn before the transfer would be
    // handed the same number and the two handles would alias.
    let retained = slime_rt::spawn(SUPERVISION_CHILD_SLOT, &[])
        .unwrap_or_else(|_| fail_supervision(b"retained child"));
    slime_rt::wait(&[slime_rt::WaitSource::Supervision(retained.supervision_slot)]);
    slime_rt::debug_write(b"[init] supervision handle retained\n");

    // ---- the loop: more tasks than MAX_RECORDS, collected as it goes ----
    //
    // Each child is spawned, waited on, collected, and its handle consumed by
    // the collection. Two handles (above) stay outstanding throughout, so the
    // live record count never approaches the bound while the lifetime count
    // sails past it.
    let mut collected = 0u32;
    for _ in 0..SUPERVISION_LOOP_CHILDREN {
        let child = slime_rt::spawn(SUPERVISION_CHILD_SLOT, &[])
            .unwrap_or_else(|_| fail_supervision(b"loop child spawn"));
        loop {
            match slime_rt::supervision_status(child.supervision_slot) {
                Ok(None) => {
                    slime_rt::wait(&[slime_rt::WaitSource::Supervision(child.supervision_slot)])
                }
                Ok(Some(slime_rt::Termination::Exit(0))) => break,
                // The failure B16 produces: a dropped record makes the child
                // look permanently live, so this arm is the one that fires if
                // the sweep is removed and the wait stops being answerable.
                _ => fail_supervision(b"a loop child did not exit cleanly"),
            }
        }
        collected += 1;
    }
    if collected != SUPERVISION_LOOP_CHILDREN {
        fail_supervision(b"the loop did not run to completion");
    }
    slime_rt::debug_write(b"[init] supervision lifetime bound crossed\n");

    // ---- the retained handle still answers, after the crossing ----
    match slime_rt::supervision_status(retained.supervision_slot) {
        Ok(Some(slime_rt::Termination::Exit(0))) => {
            slime_rt::debug_write(b"[init] retained handle answered after crossing\n");
        }
        _ => fail_supervision(b"a retained handle lost its outcome"),
    }

    // ---- the parked handle is collectable, after the crossing ----
    //
    // Collected only now, having sat in `Transit` across every sweep the loop
    // triggered. A sweep ignoring `Transit` would have freed this record while
    // it was in flight, and the query below would answer `WouldBlock` forever:
    // B16, reintroduced by its own fix. This is the arm fault injection #2
    // removes the `Transit` predicate to check.
    let mut payload = [0u8; slime_rt::MAX_MSG];
    let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    let received = slime_rt::recv(transit_recv, &mut payload, &mut caps);
    if received < 0 {
        fail_supervision(b"collecting the parked handle");
    }
    let landed = caps[0] as u32;
    if landed == 0 {
        fail_supervision(b"the parked handle landed in no slot");
    }
    match slime_rt::supervision_status(landed) {
        Ok(Some(slime_rt::Termination::Exit(0))) => {
            slime_rt::debug_write(b"[init] transit handle survived crossing\n");
        }
        _ => fail_supervision(b"a handle parked across the crossing lost its outcome"),
    }
}

/// How many channel pairs the crossing plane mints over the boot.
///
/// One more than `channel::MAX_CHANNELS`, for the reason
/// `SUPERVISION_LOOP_CHILDREN` is one more than `MAX_RECORDS`: the bound this
/// crosses is on channels *live at once*, and a graph that releases as it goes
/// must be able to exceed it. A loop that stopped at the bound would pass
/// against the unfixed root and prove nothing.
///
/// Moved 33 → 49 with P5.4.9's raise of `MAX_CHANNELS` to 48. The gate reads
/// both constants from source and refuses `pairs <= bound`, which is what
/// caught this: raising the bound alone would have left a gate that still
/// passes while proving nothing.
const CHANNEL_LOOP_PAIRS: u32 = 49;

/// Drive the channel-crossing plane: mint more channels over one boot than
/// `MAX_CHANNELS` holds at once, and keep sending on every live one.
///
/// Only reachable under `SLIME_SEL4_CROSSING_CHECK`, whose generation is
/// `contracts/generation/v1/fixtures/sel4-crossing.zti`.
///
/// This is backlog **B22**'s exit condition. Before the fix `ChannelTable`
/// never freed an entry and derived each key from `self.len`, so `MAX_CHANNELS`
/// bounded the channels a boot could **ever** mint; the 33rd `endpoint_create`
/// was refused with `ERR_OUT_OF_MEMORY` however few were still held.
///
/// The loop drops both ends of each pair before minting the next, so live
/// occupancy never exceeds three while the lifetime count sails past 32. Two
/// arms then assert what a sweep could plausibly break:
///
/// - a pair **held** across the crossing still carries a message afterwards,
///   which fails if the sweep is too aggressive;
/// - an end **parked in transit** across the crossing is still collectable and
///   still resolves to its queue — the half a predicate over live capability
///   tables alone would miss, exactly as `Transit::holds_supervision` is for
///   B16.
fn drive_crossing_plane() {
    // ---- a channel end parked in transit, across the crossing ----
    //
    // `crossing-peer` is spawned holding two edges: the carrier the end arrives
    // on (slot 0) and a gate it blocks reading (slot 1). Init transfers the end
    // over the carrier and then runs its loop while the peer is parked on the
    // gate, so the capability sits in `Transit` — held by no capability table
    // at all — across every sweep the loop triggers. A sweep reading only
    // `GraphTables` frees that channel, and the end the peer eventually lands
    // names a key the table no longer has.
    //
    // A purpose-built peer rather than init itself, and rather than an
    // unmodified component: `cap_transfer` refuses `endpoint_slot ==
    // capability_slot` and requires a distinct peer, while every unmodified
    // component drains its only queue immediately — closing the in-flight
    // window before the first sweep fires. `supervision-child` set the same
    // precedent for B16's transit arm.
    let (carrier_send, carrier_child) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_crossing(b"carrier endpoint"));
    let (gate_send, gate_child) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_crossing(b"gate endpoint"));
    let peer = slime_rt::spawn(
        CROSSING_PEER_SLOT,
        &[
            grant(carrier_child, RIGHT_SEND | RIGHT_RECV),
            grant(gate_child, RIGHT_SEND | RIGHT_RECV),
        ],
    )
    .unwrap_or_else(|_| fail_crossing(b"crossing peer"));

    // The pair whose end goes in flight. Init mints it — both ends land in
    // init's own table — moves one to the peer, and then **drops the other**.
    //
    // Dropping it is what makes this arm load-bearing rather than decorative.
    // If init kept its half the channel would still be named by a live
    // capability table and `GraphTables::holds_endpoint` alone would preserve
    // it — the arm would pass with the `Transit` half of the predicate deleted,
    // which is exactly the false confidence B16's retro warns about. With both
    // of init's slots gone, the transit entry is the *only* thing naming this
    // channel for the whole loop, so a sweep that does not consult `Transit`
    // frees it.
    let (in_flight_kept, in_flight_moved) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_crossing(b"in-flight endpoint"));
    let descriptor = slime_proto::capability_transfer::WireCapabilityTransfer {
        magic: slime_proto::capability_transfer::CAPABILITY_TRANSFER_MAGIC,
        version: slime_proto::capability_transfer::FORMAT_VERSION,
        status: 0,
        flags: 0,
        object_kind: slime_proto::capability_transfer::OBJECT_KIND_ENDPOINT,
        direction: 0,
        rights_mask: RIGHT_SEND | RIGHT_RECV,
        route_identity: [0u8; 32],
    };
    if slime_rt::cap_transfer(carrier_send, in_flight_moved, &descriptor.encode())
        != slime_rt::ERR_SUCCESS
    {
        fail_crossing(b"parking a channel end in transit");
    }
    // Gone from init's own table: a capability transfer is a move, so this slot
    // no longer resolves.
    if slime_rt::send(in_flight_moved, b"moved", &[]) != slime_rt::ERR_BAD_CAP {
        fail_crossing(b"a transferred end stayed in the sender's table");
    }
    if slime_rt::cap_drop(in_flight_kept) != slime_rt::ERR_SUCCESS {
        fail_crossing(b"releasing init's half of the in-flight pair");
    }
    slime_rt::debug_write(b"[init] channel end parked in transit\n");

    // ---- a pair init keeps across the crossing ----
    //
    // Init holds both ends, so a send on one and a receive on the other
    // round-trips through the forward queue. Both slots rather than one: since
    // B25 a minted pair is the oracle's `ipc::channel()` — two endpoints with
    // two directed queues — so a send and a receive on the *same* end address
    // opposite queues and could never meet.
    let (retained, retained_peer) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
        .unwrap_or_else(|_| fail_crossing(b"retained endpoint"));
    if slime_rt::send(retained, b"before", &[]) != slime_rt::ERR_SUCCESS {
        fail_crossing(b"the retained pair did not carry before the crossing");
    }
    let mut payload = [0u8; slime_rt::MAX_MSG];
    let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    if slime_rt::recv(retained_peer, &mut payload, &mut caps) != 6 {
        fail_crossing(b"the retained pair did not deliver before the crossing");
    }
    slime_rt::debug_write(b"[init] channel pair retained\n");

    // ---- the loop: more channels than MAX_CHANNELS, released as it goes ----
    //
    // Each pair is minted, used, and both of its ends dropped. `cap_drop` is
    // what makes the channel unnameable, which is precisely the condition the
    // sweep derives — the ends are gone from the only table that held them, and
    // nothing is in flight.
    //
    // Against the unfixed root this stops at the 33rd `endpoint_create` with
    // `ERR_OUT_OF_MEMORY`, because three channels are outstanding (carrier,
    // in-flight, retained) and the loop's own are never returned.
    let mut minted = 0u32;
    for _ in 0..CHANNEL_LOOP_PAIRS {
        let (ours, theirs) = slime_rt::endpoint_create(ENDPOINT_FACTORY_SLOT)
            .unwrap_or_else(|_| fail_crossing(b"loop pair mint"));
        // Used before it is released, so the loop exercises a real channel
        // rather than churning table entries: a pair that was minted and never
        // carried anything would not show that a swept table still works.
        if slime_rt::send(ours, b"loop", &[]) != slime_rt::ERR_SUCCESS {
            fail_crossing(b"a loop pair could not carry");
        }
        if slime_rt::recv(theirs, &mut payload, &mut caps) != 4 {
            fail_crossing(b"a loop pair did not deliver");
        }
        if slime_rt::cap_drop(ours) != slime_rt::ERR_SUCCESS
            || slime_rt::cap_drop(theirs) != slime_rt::ERR_SUCCESS
        {
            fail_crossing(b"releasing a loop pair");
        }
        minted += 1;
    }
    if minted != CHANNEL_LOOP_PAIRS {
        fail_crossing(b"the loop did not run to completion");
    }
    slime_rt::debug_write(b"[init] channel lifetime bound crossed\n");

    // ---- the retained pair still carries, after the crossing ----
    //
    // A sweep that freed an entry a live capability still names would leave
    // this slot resolving to nothing, and the send would answer `ERR_BAD_CAP`.
    if slime_rt::send(retained, b"after", &[]) != slime_rt::ERR_SUCCESS {
        fail_crossing(b"a retained pair lost its queue");
    }
    if slime_rt::recv(retained_peer, &mut payload, &mut caps) != 5 {
        fail_crossing(b"a retained pair stopped delivering");
    }
    slime_rt::debug_write(b"[init] retained pair carried after crossing\n");

    // ---- the parked end is collectable, and still resolves ----
    //
    // Releasing the gate is what ends the in-flight window: the peer collects
    // the transferred end only now, having held it in `Transit` across every
    // sweep the loop triggered. A sweep ignoring `Transit` frees that channel
    // while the end is in flight, because init dropped its own half at the
    // start and the transit entry is the only thing naming it — B22's fix
    // reintroducing B22.
    //
    // The claim is that the collected end still *resolves to a queue*, not
    // merely that a slot number arrived. Init cannot observe that directly
    // any more, having given up both of the pair's slots, so the peer proves
    // it from its own side — the transfer split the loopback, giving the peer
    // the consumer end whose send and receive resolve to the two distinct
    // queues that split allocated — and reports the outcome as its exit
    // status. A freed channel makes both answer `ERR_BAD_CAP` and it exits 1,
    // naming which operation failed on its own line first.
    if slime_rt::send(gate_send, b"go", &[]) != slime_rt::ERR_SUCCESS {
        fail_crossing(b"releasing the transit peer");
    }
    loop {
        match slime_rt::supervision_status(peer.supervision_slot) {
            Ok(None) => slime_rt::wait(&[slime_rt::WaitSource::Supervision(peer.supervision_slot)]),
            Ok(Some(slime_rt::Termination::Exit(0))) => break,
            _ => fail_crossing(b"an end parked across the crossing stopped resolving"),
        }
    }
    slime_rt::debug_write(b"[init] transit end survived crossing\n");
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
        &[grant(SAMPLE_RECEIVER_SIDE_SLOT, RIGHT_SEND | RIGHT_RECV)],
    )
    .unwrap_or_else(|_| slime_rt::exit(1));

    let lender = slime_rt::spawn(
        SAMPLE_LENDER_SLOT,
        &[
            grant(SAMPLE_LENDER_SIDE_SLOT, RIGHT_SEND | RIGHT_RECV),
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
            FABRIC_SUBSCRIBER_CLIENT_SLOT,
            RIGHT_SEND | RIGHT_RECV,
        )],
        &[FABRIC_SUBSCRIBER_SLOT, FABRIC_SUBSCRIBER_CLIENT_SLOT],
    );
    let subscriber_b = spawn_fabric_client(
        FABRIC_SUBSCRIBER_B_SLOT,
        &[grant(
            FABRIC_SUBSCRIBER_B_CLIENT_SLOT,
            RIGHT_SEND | RIGHT_RECV,
        )],
        &[FABRIC_SUBSCRIBER_B_SLOT, FABRIC_SUBSCRIBER_B_CLIENT_SLOT],
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
        &[grant(FABRIC_PUBLISHER_CLIENT_SLOT, RIGHT_SEND | RIGHT_RECV)],
        &[FABRIC_PUBLISHER_SLOT, FABRIC_PUBLISHER_CLIENT_SLOT],
    );
    // `fabric-publisher-b` originates the >MAX_MSG sample, so it needs its own
    // buffer factory and a supervision handle naming the fabric: its upstream
    // loan names the fabric as receiver by capability.
    let publisher_b = if option_env!("SLIME_FABRIC_VISIBILITY_CHECK") == Some("1") {
        spawn_fabric_client(
            FABRIC_PUBLISHER_B_SLOT,
            &[grant(
                FABRIC_PUBLISHER_B_CLIENT_SLOT,
                RIGHT_SEND | RIGHT_RECV,
            )],
            &[FABRIC_PUBLISHER_B_SLOT, FABRIC_PUBLISHER_B_CLIENT_SLOT],
        )
    } else if option_env!("SLIME_FABRIC_QOS_CHECK") == Some("1") {
        spawn_fabric_client(
            FABRIC_PUBLISHER_B_SLOT,
            &[
                grant(FABRIC_PUBLISHER_B_CLIENT_SLOT, RIGHT_SEND | RIGHT_RECV),
                grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
                grant(service.supervision_slot, RIGHT_SUPERVISE),
                grant(FABRIC_TIME_CLIENT_SLOT, RIGHT_SEND | RIGHT_RECV),
            ],
            &[FABRIC_PUBLISHER_B_SLOT, FABRIC_PUBLISHER_B_CLIENT_SLOT],
        )
    } else {
        spawn_fabric_client(
            FABRIC_PUBLISHER_B_SLOT,
            &[
                grant(FABRIC_PUBLISHER_B_CLIENT_SLOT, RIGHT_SEND | RIGHT_RECV),
                grant(SHARED_BUFFER_FACTORY_SLOT, RIGHT_BUFFER_CREATE),
                grant(service.supervision_slot, RIGHT_SUPERVISE),
            ],
            &[FABRIC_PUBLISHER_B_SLOT, FABRIC_PUBLISHER_B_CLIENT_SLOT],
        )
    };
    if option_env!("SLIME_FABRIC_QOS_CHECK") == Some("1")
        && slime_rt::cap_drop(FABRIC_TIME_CLIENT_SLOT) < 0
    {
        slime_rt::exit(1);
    }
    let intruder = spawn_fabric_client(
        FABRIC_INTRUDER_SLOT,
        &[grant(FABRIC_INTRUDER_CLIENT_SLOT, RIGHT_SEND | RIGHT_RECV)],
        &[FABRIC_INTRUDER_SLOT, FABRIC_INTRUDER_CLIENT_SLOT],
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
