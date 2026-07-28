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
    let service = if option_env!("SLIME_FABRIC_QOS_CHECK") == Some("1") {
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
    let publisher_b = if option_env!("SLIME_FABRIC_QOS_CHECK") == Some("1") {
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
