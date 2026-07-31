use alloc::vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::capability::MAX_CAPS;
use crate::capability::{
    Capability, DirectoryAuthority, KernelObject, PciFunctionInfo, RIGHT_BLOCK_READ,
    RIGHT_BLOCK_WRITE, RIGHT_BOOT_UPDATE, RIGHT_DIRECTORY_DERIVE, RIGHT_DIRECTORY_LIST,
    RIGHT_DIRECTORY_READ, RIGHT_DIRECTORY_WRITE, RIGHT_EXEC, RIGHT_HEALTH_CONFIRM,
    RIGHT_INPUT_READ, RIGHT_RECV, RIGHT_SEND, RIGHT_SPAWN, RIGHT_STORE_READ, RIGHT_STORE_WRITE,
    RIGHT_SUPERVISE, RIGHT_TRANSFER, Rights,
};
use crate::generation::{self, Generation};
use crate::{ipc, println, serial_println, task};
use boot_contracts::boot_layout::{self, BootLayout};

static INIT_ID: AtomicU64 = AtomicU64::new(0);
static CONSOLE_ID: AtomicU64 = AtomicU64::new(0);
static DANGO_ID: AtomicU64 = AtomicU64::new(0);
static SYSINFO_ID: AtomicU64 = AtomicU64::new(0);
static STORAGE_PROBE_ID: AtomicU64 = AtomicU64::new(0);
static STORAGE_WRITER_ID: AtomicU64 = AtomicU64::new(0);
static STORAGE_FAULT_ID: AtomicU64 = AtomicU64::new(0);
static STORAGE_STORE_ID: AtomicU64 = AtomicU64::new(0);
static GENERATION_MANAGER_ID: AtomicU64 = AtomicU64::new(0);
static SPAWN_SERVICE_ID: AtomicU64 = AtomicU64::new(0);
static FILESYSTEM_ID: AtomicU64 = AtomicU64::new(0);
static DIRECTORY_PROBE_ID: AtomicU64 = AtomicU64::new(0);
static GENERATION_LIST_ID: AtomicU64 = AtomicU64::new(0);
static GENERATION_INSPECT_ID: AtomicU64 = AtomicU64::new(0);
static GENERATION_STAGE_ID: AtomicU64 = AtomicU64::new(0);
static GENERATION_SELECT_ID: AtomicU64 = AtomicU64::new(0);
static GENERATION_ROLLBACK_ID: AtomicU64 = AtomicU64::new(0);
static POWERBOX_CHOOSER_ID: AtomicU64 = AtomicU64::new(0);
static POWERBOX_PROBE_ID: AtomicU64 = AtomicU64::new(0);
static SAMPLE_LENDER_ID: AtomicU64 = AtomicU64::new(0);
static SAMPLE_RECEIVER_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_SERVICE_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_PUBLISHER_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_SUBSCRIBER_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_INTRUDER_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_PUBLISHER_B_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_SUBSCRIBER_B_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_CALL_CLIENT_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_CALL_CLIENT_B_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_CALL_SERVER_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_CALL_TIME_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_OP_CLIENT_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_OP_CLIENT_B_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_OP_CLIENT_B_RESTART_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_OP_SERVER_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_OP_TIME_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_PROBE_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_PROXY_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_OBSERVER_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_CALL_WORKER_ID: AtomicU64 = AtomicU64::new(0);
static FABRIC_OP_WORKER_ID: AtomicU64 = AtomicU64::new(0);
static GENERATION_NUMBER: AtomicU64 = AtomicU64::new(0);
static RECOVERY_ID: AtomicU64 = AtomicU64::new(0);

pub fn start() -> ! {
    let bytes = crate::boot::generation();
    let generation = generation::decode(bytes).expect("invalid generation manifest");
    assert_eq!(
        generation.identity,
        crate::boot::generation_identity(),
        "handoff generation identity mismatch"
    );
    crate::generation_manager::init();
    GENERATION_NUMBER.store(generation.number, Ordering::Relaxed);
    if generation.number == 8 {
        serial_println!("[generation-command] scripted check active");
    }
    if generation.number == 7 {
        crate::input::install_script(
            b"$(sysinfo)\n(with-env {MODE=ci} (with-cwd docs (with-stdin data $(echo ok))))\n$(inject)\n$(echo a b c)\n\x1b",
        );
    }
    if generation.number == 9 {
        crate::input::install_script(b"\n\x1b");
        serial_println!("[powerbox] scripted check active");
    }
    serial_println!(
        "[generation] selected {:02x?} parent={:02x?} target={}",
        generation.identity,
        generation.parent,
        generation.target,
    );
    serial_println!(
        "[generation] decoded generation {}: {} objects, {} components, {} grants",
        generation.number,
        generation.object_count(),
        generation.component_count(),
        generation.grant_count(),
    );
    reclamation_probe(&generation);
    let init_id = launch_init(&generation);
    INIT_ID.store(init_id, Ordering::Relaxed);
    // Install init's generation-declared shared-buffer quota (C7.3). A
    // generation that declares no budget leaves the deny-by-default quota, so
    // no component allocates shared buffers implicitly. Spawned children
    // receive their own quota through `record_spawn`.
    task::set_shared_buffer_quota(
        init_id,
        crate::generation::shared_buffer_quota(&generation, "init"),
    );
    task::set_on_idle(on_idle);
    task::run()
}

/// Prove on the live boot path that a spawn and its release conserve frames
/// (B9).
///
/// The kernel test harness measures the same conservation, but against a
/// synthetic image. This runs the real `spawn_with_caps_for` over a real
/// generation component, so the frames counted are the ones an actual boot
/// maps: image segments, stack, and the user-half page tables behind them.
///
/// Scope, stated precisely: this exercises the **release** path
/// (`release_unscheduled` -> `AddressSpace::drop` -> `free_user_half`), not the
/// scheduler's reaper. The task is never scheduled, so it never terminates.
/// Running it to exit would need the scheduler, and the reaper's own evidence
/// is that `just spawn_service_check` and `just dango_check` boot real
/// components that exit through `terminate` and still report a healthy slice.
/// Both halves share `AddressSpace::drop`, which is where every frame actually
/// goes back, so a leak in the common path fails here first.
///
/// Deliberately before `launch_init`: the graph has not started, so nothing
/// else is allocating and the delta is attributable.
///
/// The first cycle is discarded: it may grow the kernel heap for the task's
/// bookkeeping, which later cycles reuse. What must hold afterwards is zero
/// drift, which is exactly what a per-spawn leak would break.
fn reclamation_probe(generation: &Generation<'static>) {
    let Some(image) = generation.component_bytes("sysinfo") else {
        return;
    };
    let free = || crate::memory::pmm::FRAME_ALLOCATOR.lock().free_frames();

    let Ok(warm_up) = task::spawn_with_caps(image, alloc::vec::Vec::new()) else {
        serial_println!("[reclaim] probe skipped: spawn unavailable");
        return;
    };
    task::release_unscheduled(warm_up);

    let before = free();
    let mut cost = 0;
    for _ in 0..4 {
        let start = free();
        let Ok(id) = task::spawn_with_caps(image, alloc::vec::Vec::new()) else {
            serial_println!("[reclaim] probe skipped: spawn unavailable");
            return;
        };
        cost = start.saturating_sub(free());
        task::release_unscheduled(id);
    }
    let after = free();
    if after == before {
        serial_println!(
            "[reclaim] spawn/exit conserves frames: {} per cycle, 0 drift",
            cost
        );
    } else {
        serial_println!(
            "[reclaim] spawn/exit leaked: {} frame(s) over 4 cycles",
            before.saturating_sub(after)
        );
    }
}

fn launch_init(generation: &Generation<'static>) -> task::TaskId {
    if generation.component_named("recovery").is_some() {
        return launch_recovery_init(generation);
    }
    // C8.10 full-graph boot. A separate layout reached by an early return,
    // mirroring `launch_recovery_init`, rather than more overlays on the table
    // below.
    //
    // That table is 61 of `MAX_CAPS = 64` before this milestone adds anything,
    // so the three new roles cannot be appended: they need nine slots against
    // three free. A fabric-only layout holds only what the fabric graph needs
    // and leaves every earlier gate's slots exactly where they were.
    //
    // Selected by what the generation's layout declares, like the recovery fork
    // above it. Only the full-graph layout gives init the fabric's own route
    // workers; every other layout leaves the fabric to spawn them. The
    // component *list* is the same in all of them — one manifest declares every
    // component any profile uses — so the layout, not the manifest, is what
    // distinguishes a profile.
    //
    // This was a `SLIME_FABRIC_BOOT_CHECK` flag compared against generation 17,
    // which meant the gate taking this path built a different kernel binary
    // than every other gate (B10).
    let layout = generation::boot_layout(generation);
    if layout_declares(
        &layout,
        boot_layout::component_identity("fabric-call-worker"),
    ) {
        return launch_fabric_boot_init(generation);
    }
    let init = generation
        .component_bytes("init")
        .expect("init object missing");
    serial_println!("[generation] validating bootstrap grants");

    require_grant(
        generation,
        "endpoint-factory",
        "init",
        "init",
        crate::capability::RIGHT_ENDPOINT_CREATE,
    );
    serial_println!("[generation] endpoint grant valid");
    require_grant(
        generation,
        "spawn-service-rpc",
        "dango",
        "spawn-service",
        RIGHT_SEND | RIGHT_RECV,
    );
    serial_println!("[generation] rpc grant valid");
    require_grant(
        generation,
        "spawn-service-sysinfo",
        "spawn-service",
        "sysinfo",
        RIGHT_EXEC | RIGHT_SPAWN,
    );
    require_grant(
        generation,
        "spawn-service-echo",
        "spawn-service",
        "echo-agent",
        RIGHT_EXEC | RIGHT_SPAWN,
    );
    serial_println!("[generation] command executable grants valid");
    require_grant(
        generation,
        "console-output",
        "console",
        "dango",
        RIGHT_SEND | RIGHT_TRANSFER,
    );
    serial_println!("[generation] console grant valid");
    require_grant(
        generation,
        "console-input",
        "init",
        "dango",
        RIGHT_INPUT_READ,
    );
    serial_println!("[generation] input grant valid");
    require_grant(
        generation,
        "dango-cwd-root",
        "init",
        "dango",
        RIGHT_DIRECTORY_READ | RIGHT_DIRECTORY_DERIVE | RIGHT_TRANSFER,
    );
    require_grant(
        generation,
        "dango-endpoint-factory",
        "init",
        "dango",
        crate::capability::RIGHT_ENDPOINT_CREATE,
    );
    require_grant(
        generation,
        "spawn-service-endpoint-factory",
        "init",
        "spawn-service",
        crate::capability::RIGHT_ENDPOINT_CREATE,
    );
    serial_println!("[generation] Dango context grants valid");
    require_grant(
        generation,
        "block-read",
        "init",
        "storage-probe",
        RIGHT_BLOCK_READ,
    );
    serial_println!("[generation] block read grant valid");
    require_grant(
        generation,
        "block-write-check",
        "init",
        "storage-writer",
        RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE,
    );
    serial_println!("[generation] block write grant valid");
    require_grant(
        generation,
        "block-fault-check",
        "init",
        "storage-fault-probe",
        RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE,
    );
    serial_println!("[generation] block fault grant valid");
    require_grant(
        generation,
        "health-confirmation",
        "init",
        "generation-manager",
        RIGHT_HEALTH_CONFIRM,
    );
    serial_println!("[generation] health grant valid");
    require_grant(
        generation,
        "generation-boot-update",
        "init",
        "generation-manager",
        RIGHT_BOOT_UPDATE,
    );
    for client in [
        "generation-list",
        "generation-inspect",
        "generation-stage",
        "generation-select",
        "generation-rollback",
    ] {
        require_grant(
            generation,
            "generation-management-rpc",
            client,
            "generation-manager",
            RIGHT_SEND | RIGHT_RECV,
        );
    }
    serial_println!("[generation] update grants valid");
    require_grant(
        generation,
        "store-access",
        "init",
        "storage-store-probe",
        RIGHT_STORE_READ | RIGHT_STORE_WRITE,
    );
    serial_println!("[generation] store grant valid");
    require_grant(
        generation,
        "filesystem-rpc",
        "directory-probe",
        "filesystem-service",
        RIGHT_SEND | RIGHT_RECV,
    );
    require_grant(
        generation,
        "filesystem-store",
        "init",
        "filesystem-service",
        RIGHT_STORE_READ | RIGHT_STORE_WRITE,
    );
    require_grant(
        generation,
        "filesystem-root",
        "init",
        "directory-probe",
        RIGHT_TRANSFER
            | RIGHT_DIRECTORY_READ
            | RIGHT_DIRECTORY_WRITE
            | RIGHT_DIRECTORY_LIST
            | RIGHT_DIRECTORY_DERIVE,
    );
    require_grant(
        generation,
        "powerbox-rpc",
        "powerbox-probe",
        "powerbox-chooser",
        RIGHT_SEND | RIGHT_RECV,
    );
    require_grant(
        generation,
        "powerbox-root",
        "init",
        "powerbox-chooser",
        RIGHT_DIRECTORY_READ | RIGHT_DIRECTORY_DERIVE | RIGHT_TRANSFER,
    );
    require_grant(
        generation,
        "powerbox-input",
        "init",
        "powerbox-chooser",
        RIGHT_INPUT_READ,
    );
    serial_println!("[generation] powerbox grants valid");
    require_grant(
        generation,
        "dango-shared-buffer-factory",
        "init",
        "dango",
        crate::capability::RIGHT_BUFFER_CREATE,
    );
    require_grant(
        generation,
        "spawn-service-shared-buffer-factory",
        "init",
        "spawn-service",
        crate::capability::RIGHT_BUFFER_CREATE,
    );
    require_grant(
        generation,
        "sample-lender-shared-buffer-factory",
        "init",
        "sample-lender",
        crate::capability::RIGHT_BUFFER_CREATE,
    );
    require_grant(
        generation,
        "sample-plane-channel",
        "sample-lender",
        "sample-receiver",
        RIGHT_SEND | RIGHT_RECV,
    );
    require_grant(
        generation,
        "sample-plane-receiver-supervision",
        "init",
        "sample-lender",
        RIGHT_SUPERVISE,
    );
    serial_println!("[generation] shared-buffer factory grants valid");
    // C8.3 fabric control plane. Every client reaches the fabric through a
    // generation-declared control endpoint; the fabric mints route endpoints
    // through its own factory grant. `fabric-intruder` gets a control endpoint
    // too — it is declared as a client of the fabric but appears in no route,
    // which is exactly the case the milestone must deny.
    require_grant(
        generation,
        "fabric-endpoint-factory",
        "init",
        "fabric-service",
        crate::capability::RIGHT_ENDPOINT_CREATE,
    );
    for (client, grant) in [
        ("fabric-publisher", "fabric-publisher-control"),
        ("fabric-subscriber", "fabric-subscriber-control"),
        ("fabric-intruder", "fabric-intruder-control"),
        ("fabric-publisher-b", "fabric-publisher-b-control"),
        ("fabric-subscriber-b", "fabric-subscriber-b-control"),
    ] {
        require_grant(
            generation,
            grant,
            client,
            "fabric-service",
            RIGHT_SEND | RIGHT_RECV,
        );
    }
    for (client, grant) in [
        ("fabric-call-client", "fabric-call-client-control"),
        ("fabric-call-client-b", "fabric-call-client-b-control"),
        ("fabric-call-server", "fabric-call-server-control"),
        ("fabric-call-time", "fabric-call-time-control"),
        // C8.7 operation plane. Each participant reaches the fabric only through
        // its own generation-declared control endpoint, which is what the broker
        // authenticates against instead of trusting an identity in a record.
        ("fabric-op-client", "fabric-op-client-control"),
        ("fabric-op-client-b", "fabric-op-client-b-control"),
        ("fabric-op-server", "fabric-op-server-control"),
        ("fabric-op-time", "fabric-op-time-control"),
    ] {
        require_grant(
            generation,
            grant,
            client,
            "fabric-service",
            RIGHT_SEND | RIGHT_RECV,
        );
    }
    // C8.4 brokering: the fabric owns the one copy each large sample makes, so
    // it needs creation authority of its own. Its `shared-buffer-budget` entry
    // bounds that copy, which is what keeps a fan-out inside a declared quota.
    require_grant(
        generation,
        "fabric-shared-buffer-factory",
        "init",
        "fabric-service",
        crate::capability::RIGHT_BUFFER_CREATE,
    );
    serial_println!("[generation] fabric control grants valid");
    serial_println!("[generation] filesystem grants valid");
    let transfer_functions = block_functions();
    let transfer_receiver = transfer_functions
        .iter()
        .find(|function| function.device == 5)
        .copied();
    let transfer_source = transfer_functions
        .iter()
        .find(|function| function.device == 6)
        .copied();

    serial_println!("[generation] bootstrap grants valid");

    let (console_output, dango_output) = ipc::channel();
    let (dango_spawn, service_spawn) = ipc::channel();
    let (directory_client, directory_service) = ipc::channel();
    let (generation_list_client, generation_list_service) = ipc::channel();
    let (generation_inspect_client, generation_inspect_service) = ipc::channel();
    let (generation_stage_client, generation_stage_service) = ipc::channel();
    let (generation_select_client, generation_select_service) = ipc::channel();
    let (generation_rollback_client, generation_rollback_service) = ipc::channel();
    let (powerbox_client, powerbox_service) = ipc::channel();
    let (sample_lender_side, sample_receiver_side) = ipc::channel();
    let (fabric_publisher_client, fabric_publisher_service) = ipc::channel();
    let (fabric_subscriber_client, fabric_subscriber_service) = ipc::channel();
    let (fabric_intruder_client, fabric_intruder_service) = ipc::channel();
    let (fabric_publisher_b_client, fabric_publisher_b_service) = ipc::channel();
    let (fabric_subscriber_b_client, fabric_subscriber_b_service) = ipc::channel();
    let (fabric_time_client, fabric_time_service) = ipc::channel();
    let (fabric_call_client_control, fabric_call_client_service) = ipc::channel();
    let (fabric_call_client_b_control, fabric_call_client_b_service) = ipc::channel();
    let (fabric_call_server_control, fabric_call_server_service) = ipc::channel();
    let (fabric_call_time_control, fabric_call_time_service) = ipc::channel();
    let (fabric_call_phase_client, fabric_call_phase_time) = ipc::channel();
    // C8.7 operation control plane. Its own channels rather than the call
    // plane's: the two profiles occupy the same capability slots but only one
    // is ever placed, and a single `ipc::Endpoint` cannot be moved into both
    // branches.
    let (fabric_op_client_control, fabric_op_client_service) = ipc::channel();
    let (fabric_op_client_b_control, fabric_op_client_b_service) = ipc::channel();
    let (fabric_op_server_control, fabric_op_server_service) = ipc::channel();
    let (fabric_op_time_control, fabric_op_time_service) = ipc::channel();
    // Init's capability table is placed by the layout this generation declares,
    // not by the order of this block. Every capability is still minted here —
    // only the kernel knows what a channel is, or which half of one a client
    // holds — but where each lands is generation data. A capability the layout
    // does not name, or a declared slot nothing fills, stops the boot.
    let mut placer = LayoutPlacer::new(layout);
    placer.role(
        boot_layout::Role::EndpointFactory,
        "endpoint factory",
        KernelObject::EndpointFactory,
    );
    placer.executable(generation, "console");
    placer.endpoint("console-output", console_output, RIGHT_RECV);
    placer.executable(generation, "dango");
    placer.endpoint("dango-output", dango_output, RIGHT_SEND);
    placer.executable(generation, "spawn-service");
    placer.executable(generation, "sysinfo");
    placer.executable(generation, "echo-agent");
    placer.one_of(generation, &STORAGE_COMPONENTS, "storage component");
    // The storage capability is the one slot whose object the layout cannot
    // name. It declares the authority a present block device carries; when the
    // platform enumerates none, a read-only object store stands in. Those
    // fallback rights are the kernel's, not the layout's — applying the
    // declared block rights to an `ObjectStore` would grant an authority the
    // object does not answer for. `NO_DISK_FALLBACK` in
    // `scripts/check/check-boot-layout-resource.py` is this fallback's
    // host-side twin.
    if let Some(entry) = placer.entry_for_role(boot_layout::Role::StorageCapability) {
        let capability = optional_block_function()
            .map(|function| Capability {
                object: KernelObject::BlockDevice(function),
                rights: entry.rights,
            })
            .unwrap_or(Capability {
                object: KernelObject::ObjectStore,
                rights: RIGHT_STORE_READ,
            });
        placer.slots[entry.slot as usize] = Some(capability);
    } else {
        // Generation 4 declares an object store outright: it exercises the
        // store service rather than a block device, so no probe applies.
        placer.role(
            boot_layout::Role::ObjectStore,
            "storage object store",
            KernelObject::ObjectStore,
        );
    }
    placer.executable(generation, "generation-manager");
    placer.role(
        boot_layout::Role::GenerationControl,
        "generation control",
        KernelObject::GenerationControl,
    );
    placer.endpoint(
        "dango-spawn",
        dango_spawn,
        RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
    );
    placer.endpoint("service-spawn", service_spawn, RIGHT_SEND | RIGHT_RECV);
    placer.executable(generation, "filesystem-service");
    placer.executable(generation, "directory-probe");
    placer.endpoint(
        "directory-client",
        directory_client,
        RIGHT_SEND | RIGHT_RECV,
    );
    placer.endpoint(
        "directory-service",
        directory_service,
        RIGHT_SEND | RIGHT_RECV,
    );
    placer.role(
        boot_layout::Role::ObjectStore,
        "filesystem object store",
        KernelObject::ObjectStore,
    );
    placer.role(
        boot_layout::Role::DirectoryRoot,
        "directory root",
        KernelObject::Directory(DirectoryAuthority::root(directory_fixture_root())),
    );
    placer.role(boot_layout::Role::Input, "input", KernelObject::Input);
    placer.executable(generation, "generation-list");
    placer.executable(generation, "generation-inspect");
    placer.executable(generation, "generation-stage");
    placer.executable(generation, "generation-select");
    placer.executable(generation, "generation-rollback");
    placer.endpoint(
        "generation-list-client",
        generation_list_client,
        RIGHT_SEND | RIGHT_RECV,
    );
    placer.endpoint(
        "generation-inspect-client",
        generation_inspect_client,
        RIGHT_SEND | RIGHT_RECV,
    );
    placer.endpoint(
        "generation-stage-client",
        generation_stage_client,
        RIGHT_SEND | RIGHT_RECV,
    );
    placer.endpoint(
        "generation-select-client",
        generation_select_client,
        RIGHT_SEND | RIGHT_RECV,
    );
    placer.endpoint(
        "generation-rollback-client",
        generation_rollback_client,
        RIGHT_SEND | RIGHT_RECV,
    );
    placer.endpoint(
        "generation-list-service",
        generation_list_service,
        RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
    );
    placer.endpoint(
        "generation-inspect-service",
        generation_inspect_service,
        RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
    );
    placer.endpoint(
        "generation-stage-service",
        generation_stage_service,
        RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
    );
    placer.endpoint(
        "generation-select-service",
        generation_select_service,
        RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
    );
    placer.endpoint(
        "generation-rollback-service",
        generation_rollback_service,
        RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
    );
    placer.executable(generation, "powerbox-chooser");
    placer.executable(generation, "powerbox-probe");
    placer.endpoint("powerbox-client", powerbox_client, RIGHT_SEND | RIGHT_RECV);
    placer.endpoint(
        "powerbox-service",
        powerbox_service,
        RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
    );
    // C7.2 shared-buffer factory. Creation authority only, and transferable so
    // init can derive-copy it into the components the generation grants it to.
    // A holder still allocates nothing without a `shared-buffer-budget` entry
    // (C7.3): the grant authorizes the operation, the budget bounds it.
    placer.role(
        boot_layout::Role::SharedBufferFactory,
        "shared-buffer factory",
        KernelObject::SharedBufferFactory,
    );
    // C7.7 sample plane. Two real components exchange a >MAX_MSG payload
    // through the shared-buffer syscalls; init spawns both and hands the lender
    // the receiver's supervision handle so the loan names its receiver by
    // capability rather than an ambient task id.
    placer.executable(generation, "sample-lender");
    placer.executable(generation, "sample-receiver");
    placer.endpoint(
        "sample-lender-side",
        sample_lender_side,
        RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
    );
    placer.endpoint(
        "sample-receiver-side",
        sample_receiver_side,
        RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
    );
    // C8.3/C8.4 fabric control plane. The fabric holds one control endpoint per
    // client and its own endpoint and shared-buffer factory grants; each client
    // holds only its half of its control channel. Route endpoints are not
    // minted here: the fabric creates them and moves each participant a
    // narrowed, non-transferable role through `SYS_CAP_TRANSFER`, so a route
    // capability never exists in init's table at all.
    //
    // The call and operation profiles (generations 14 and 15) declare their own
    // participants in the same slot range: the three planes are mutually
    // exclusive, so none grows init's table past `MAX_CAPS`. That reuse was a
    // block of `caps[46] = ...` rewrites; it is now the layout saying which
    // component each slot holds, and the branches below only decide which set
    // of capabilities to mint.
    placer.executable_as_declared(generation, "fabric-service");
    // The call and operation profiles replace most but not all of the stream
    // plane's slots, and they stop at different points. Generation 14 rewrote
    // slots 46-49 and left 50 holding `fabric-subscriber-b`; generation 15 took
    // 50 as well but left its control channel at 55 and 60. Those leftovers are
    // inert — no stream route exists in either profile — but init is still
    // handed them, and the old `caps[46] = ...` block is why. Preserving it
    // exactly is the point: the layout now records which slots a rewrite loop
    // happened to cover, instead of that being implied by an index range.
    if placer.declares_component("fabric-subscriber-b") {
        placer.executable(generation, "fabric-subscriber-b");
    }
    placer.endpoint(
        "fabric-subscriber-b-client",
        fabric_subscriber_b_client,
        RIGHT_SEND | RIGHT_RECV,
    );
    placer.endpoint(
        "fabric-subscriber-b-service",
        fabric_subscriber_b_service,
        RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
    );
    if placer.declares_component("fabric-call-client") {
        placer.executable_as_declared(generation, "fabric-call-client");
        placer.executable_as_declared(generation, "fabric-call-client-b");
        placer.executable_as_declared(generation, "fabric-call-time");
        placer.executable_as_declared(generation, "fabric-call-server");
        placer.endpoint(
            "fabric-call-client-control",
            fabric_call_client_control,
            RIGHT_SEND | RIGHT_RECV,
        );
        placer.endpoint(
            "fabric-call-client-b-control",
            fabric_call_client_b_control,
            RIGHT_SEND | RIGHT_RECV,
        );
        placer.endpoint(
            "fabric-call-time-control",
            fabric_call_time_control,
            RIGHT_SEND | RIGHT_RECV,
        );
        placer.endpoint(
            "fabric-call-server-control",
            fabric_call_server_control,
            RIGHT_SEND | RIGHT_RECV,
        );
        placer.endpoint(
            "fabric-call-client-service",
            fabric_call_client_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        );
        placer.endpoint(
            "fabric-call-client-b-service",
            fabric_call_client_b_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        );
        placer.endpoint(
            "fabric-call-time-service",
            fabric_call_time_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        );
        placer.endpoint(
            "fabric-call-server-service",
            fabric_call_server_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        );
        placer.endpoint("fabric-call-phase-time", fabric_call_phase_time, RIGHT_RECV);
        placer.endpoint(
            "fabric-call-phase-client",
            fabric_call_phase_client,
            RIGHT_SEND,
        );
    } else if placer.declares_component("fabric-op-client") {
        placer.executable_as_declared(generation, "fabric-op-client");
        placer.executable_as_declared(generation, "fabric-op-client-b");
        placer.executable_as_declared(generation, "fabric-op-server");
        placer.executable_as_declared(generation, "fabric-op-time");
        placer.executable_as_declared(generation, "fabric-op-client-b-restart");
        placer.endpoint(
            "fabric-op-client-control",
            fabric_op_client_control,
            RIGHT_SEND | RIGHT_RECV,
        );
        placer.endpoint(
            "fabric-op-client-b-control",
            fabric_op_client_b_control,
            RIGHT_SEND | RIGHT_RECV,
        );
        placer.endpoint(
            "fabric-op-time-control",
            fabric_op_time_control,
            RIGHT_SEND | RIGHT_RECV,
        );
        placer.endpoint(
            "fabric-op-server-control",
            fabric_op_server_control,
            RIGHT_SEND | RIGHT_RECV,
        );
        placer.endpoint(
            "fabric-op-client-service",
            fabric_op_client_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        );
        placer.endpoint(
            "fabric-op-client-b-service",
            fabric_op_client_b_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        );
        placer.endpoint(
            "fabric-op-time-service",
            fabric_op_time_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        );
        placer.endpoint(
            "fabric-op-server-service",
            fabric_op_server_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        );
    } else {
        placer.executable(generation, "fabric-publisher");
        placer.executable(generation, "fabric-subscriber");
        placer.executable(generation, "fabric-intruder");
        placer.executable(generation, "fabric-publisher-b");
        placer.endpoint(
            "fabric-publisher-client",
            fabric_publisher_client,
            RIGHT_SEND | RIGHT_RECV,
        );
        placer.endpoint(
            "fabric-subscriber-client",
            fabric_subscriber_client,
            RIGHT_SEND | RIGHT_RECV,
        );
        placer.endpoint(
            "fabric-intruder-client",
            fabric_intruder_client,
            RIGHT_SEND | RIGHT_RECV,
        );
        placer.endpoint(
            "fabric-publisher-b-client",
            fabric_publisher_b_client,
            RIGHT_SEND | RIGHT_RECV,
        );
        // The service side of each control channel. Transferable so init can
        // grant them into the fabric; the fabric itself never re-delegates one.
        placer.endpoint(
            "fabric-publisher-service",
            fabric_publisher_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        );
        placer.endpoint(
            "fabric-subscriber-service",
            fabric_subscriber_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        );
        placer.endpoint(
            "fabric-intruder-service",
            fabric_intruder_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        );
        placer.endpoint(
            "fabric-publisher-b-service",
            fabric_publisher_b_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        );
        if placer.declares_channel("fabric-time-client") {
            placer.endpoint(
                "fabric-time-client",
                fabric_time_client,
                RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
            );
            placer.endpoint(
                "fabric-time-service",
                fabric_time_service,
                RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
            );
        }
    }
    let mut caps = placer.finish();
    // The transfer pair is appended rather than placed: it exists only when the
    // platform enumerates both block devices, so no layout declares it. It
    // lands past the layout's high-water mark, which is why this is the one
    // path that can outgrow the capability table.
    if let (Some(receiver), Some(source)) = (transfer_receiver, transfer_source) {
        caps.extend([
            Capability {
                object: KernelObject::BlockDevice(receiver),
                rights: RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE | RIGHT_BOOT_UPDATE,
            },
            Capability {
                object: KernelObject::BlockDevice(source),
                rights: RIGHT_BLOCK_READ | RIGHT_TRANSFER,
            },
        ]);
    }
    assert!(
        caps.len() <= MAX_CAPS,
        "init layout exceeds the kernel capability table"
    );

    let spawn_budget = generation
        .component_named("init")
        .expect("init component missing")
        .spawn_budget;
    serial_println!("[generation] spawning init");
    serial_println!(
        "[generation] launching init with {} capabilities",
        caps.len()
    );
    dump_boot_layout("init", &caps);
    task::spawn_with_caps_for(init, caps, None, spawn_budget).expect("failed to launch init")
}

/// Every participant the C8.10 full-graph boot launches, with the control grant
/// that binds it to the fabric.
///
/// Order is the layout: init's executable and control slots are derived from
/// this table by index, and `init.rs` walks the same order. The two route
/// workers are absent because the fabric spawns them and mints their controls,
/// so they never occupy an init slot at all.
///
/// `fabric-intruder` is absent too: the probe, proxy, and introspection roles it
/// once carried behind one env switch are three declared components here.
/// The stream plane, in the resolved profile's control order. Every one of these
/// is a participant on a route this generation's stream worker carries.
const FABRIC_BOOT_STREAM: [(&str, &str, &str); 7] = [
    (
        "fabric-publisher",
        "fabric-publisher-control",
        "fabric-publisher-control-service",
    ),
    (
        "fabric-subscriber",
        "fabric-subscriber-control",
        "fabric-subscriber-control-service",
    ),
    (
        "fabric-publisher-b",
        "fabric-publisher-b-control",
        "fabric-publisher-b-control-service",
    ),
    (
        "fabric-subscriber-b",
        "fabric-subscriber-b-control",
        "fabric-subscriber-b-control-service",
    ),
    (
        "fabric-observer",
        "fabric-observer-control",
        "fabric-observer-control-service",
    ),
    (
        "fabric-probe",
        "fabric-probe-control",
        "fabric-probe-control-service",
    ),
    (
        "fabric-proxy",
        "fabric-proxy-control",
        "fabric-proxy-control-service",
    ),
];

/// The call plane, then the operation plane and its replacement channel. Each
/// carries its own capability-routed clock: a worker's time source is a declared
/// participant, never an ambient timer.
const FABRIC_BOOT_REQUEST_RESPONSE: [(&str, &str, &str); 9] = [
    (
        "fabric-call-client",
        "fabric-call-client-control",
        "fabric-call-client-control-service",
    ),
    (
        "fabric-call-client-b",
        "fabric-call-client-b-control",
        "fabric-call-client-b-control-service",
    ),
    (
        "fabric-call-server",
        "fabric-call-server-control",
        "fabric-call-server-control-service",
    ),
    (
        "fabric-call-time",
        "fabric-call-time-control",
        "fabric-call-time-control-service",
    ),
    (
        "fabric-op-client",
        "fabric-op-client-control",
        "fabric-op-client-control-service",
    ),
    (
        "fabric-op-client-b",
        "fabric-op-client-b-control",
        "fabric-op-client-b-control-service",
    ),
    (
        "fabric-op-server",
        "fabric-op-server-control",
        "fabric-op-server-control-service",
    ),
    (
        "fabric-op-time",
        "fabric-op-time-control",
        "fabric-op-time-control-service",
    ),
    (
        "fabric-op-client-b-restart",
        "fabric-op-client-b-restart-control",
        "fabric-op-client-b-restart-control-service",
    ),
];

/// Every participant the C8.10 boot launches: the stream plane, then the two
/// request/response planes. One flat order, because init's executable and
/// control slots are derived from it by index and `init.rs` walks the same
/// order.
const FABRIC_BOOT_PARTICIPANTS: [(&str, &str, &str); 16] = {
    let mut all = [("", "", ""); 16];
    let mut index = 0;
    while index < FABRIC_BOOT_STREAM.len() {
        all[index] = FABRIC_BOOT_STREAM[index];
        index += 1;
    }
    while index < all.len() {
        all[index] = FABRIC_BOOT_REQUEST_RESPONSE[index - FABRIC_BOOT_STREAM.len()];
        index += 1;
    }
    all
};

/// C8.10: launch init with one collision-free fabric-only capability layout.
///
/// Every C8 role in one generation — the stream, call, and operation planes,
/// the unauthorized probe, the declared interposition proxy, and the filtered
/// introspection client — against a table that must stay under `MAX_CAPS`.
/// Nothing from the console, Dango, storage, filesystem, generation-management,
/// or powerbox graphs appears: those are not part of the fabric graph, and
/// carrying them is what left the main layout with three free slots.
///
/// Init holds no route capability here either. It mints one control channel per
/// participant, hands the fabric the service side and the participant its own
/// client side, and that spawn-time binding is what the workers authenticate
/// against. The two route workers are spawned *by the fabric*, not by init, so
/// the component that binds a worker's control endpoints to an identity is the
/// same component that created them.
fn launch_fabric_boot_init(generation: &Generation<'static>) -> task::TaskId {
    let init = generation
        .component_bytes("init")
        .expect("init object missing");
    require_grant(
        generation,
        "endpoint-factory",
        "init",
        "init",
        crate::capability::RIGHT_ENDPOINT_CREATE,
    );
    require_grant(
        generation,
        "fabric-endpoint-factory",
        "init",
        "fabric-service",
        crate::capability::RIGHT_ENDPOINT_CREATE,
    );
    require_grant(
        generation,
        "fabric-shared-buffer-factory",
        "init",
        "fabric-service",
        crate::capability::RIGHT_BUFFER_CREATE,
    );
    // The fabric spawns the two route workers, so it — not init — holds their
    // executables. Authority to run a worker is a declared grant like any other.
    for grant in [
        "fabric-call-worker-executable",
        "fabric-op-worker-executable",
    ] {
        require_grant(
            generation,
            grant,
            "init",
            "fabric-service",
            RIGHT_EXEC | RIGHT_SPAWN,
        );
    }
    for (client, grant, _) in FABRIC_BOOT_PARTICIPANTS {
        require_grant(
            generation,
            grant,
            client,
            "fabric-service",
            RIGHT_SEND | RIGHT_RECV,
        );
    }
    serial_println!("[generation] fabric boot control grants valid");

    let mut caps = vec![
        Capability {
            object: KernelObject::EndpointFactory,
            rights: crate::capability::RIGHT_ENDPOINT_CREATE | RIGHT_TRANSFER,
        },
        Capability {
            object: KernelObject::SharedBufferFactory,
            rights: crate::capability::RIGHT_BUFFER_CREATE | RIGHT_TRANSFER,
        },
        executable(
            generation,
            "fabric-service",
            generation
                .component_bytes("fabric-service")
                .expect("fabric-service object missing"),
        ),
        executable(
            generation,
            "fabric-call-worker",
            generation
                .component_bytes("fabric-call-worker")
                .expect("fabric-call-worker object missing"),
        ),
        executable(
            generation,
            "fabric-op-worker",
            generation
                .component_bytes("fabric-op-worker")
                .expect("fabric-op-worker object missing"),
        ),
    ];
    // One executable per participant, then both halves of its control channel.
    // Grouped per participant rather than by kind so init's slot arithmetic is
    // one stride, and a participant added or removed moves one block.
    for (component, _, _) in FABRIC_BOOT_PARTICIPANTS {
        let mut capability = executable(
            generation,
            component,
            generation
                .component_bytes(component)
                .unwrap_or_else(|| panic!("{component} object missing")),
        );
        // `spawn_from_cap` makes a child's supervision handle transferable only
        // when the executable it came from is. The call and operation workers
        // authenticate a participant by a supervision capability moved over its
        // own control channel, so without this init cannot hand those on — and
        // the failure surfaces as the *worker* seeing a dead peer, because init
        // exits mid-launch rather than as an error naming the transfer.
        capability.rights |= RIGHT_TRANSFER;
        caps.push(capability);
    }
    for (_, control, service_control) in FABRIC_BOOT_PARTICIPANTS {
        let (client, service) = ipc::channel();
        // Both halves of one channel, but not interchangeable: the client half
        // goes to the participant and the service half to the fabric. The dump
        // labels them apart so a layout that swapped the two would not compare
        // equal to one that did not.
        caps.push(endpoint(control, client, RIGHT_SEND | RIGHT_RECV));
        caps.push(endpoint(service_control, service, RIGHT_SEND | RIGHT_RECV));
    }
    assert!(
        caps.len() <= crate::capability::MAX_CAPS,
        "fabric boot layout exceeds the kernel capability table"
    );
    serial_println!(
        "[generation] fabric boot layout {} of {} slots",
        caps.len(),
        crate::capability::MAX_CAPS
    );
    let spawn_budget = generation
        .component_named("init")
        .expect("init component missing")
        .spawn_budget;
    dump_boot_layout("fabric-boot", &caps);
    task::spawn_with_caps_for(init, caps, None, spawn_budget)
        .expect("failed to launch fabric boot init")
}

fn launch_recovery_init(generation: &Generation<'static>) -> task::TaskId {
    let recovery_index = recovery_index(generation);
    let init = generation
        .component_bytes("init")
        .expect("init object missing");
    require_grant(
        generation,
        "endpoint-factory",
        "init",
        "init",
        crate::capability::RIGHT_ENDPOINT_CREATE,
    );
    require_grant(
        generation,
        "recovery-control",
        "init",
        "recovery",
        RIGHT_BOOT_UPDATE,
    );
    require_grant(
        generation,
        "recovery-target",
        "init",
        "recovery",
        RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE,
    );
    let function = recovery_block_function(&recovery_index);
    let recovery = generation
        .component_bytes("recovery")
        .expect("recovery object missing");
    let caps = vec![
        Capability {
            object: KernelObject::EndpointFactory,
            rights: crate::capability::RIGHT_ENDPOINT_CREATE,
        },
        executable(generation, "recovery", recovery),
        Capability {
            object: KernelObject::GenerationControl,
            rights: RIGHT_BOOT_UPDATE | RIGHT_TRANSFER,
        },
        Capability {
            object: KernelObject::BlockDevice(function),
            rights: RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE | RIGHT_TRANSFER,
        },
    ];
    dump_boot_layout("recovery", &caps);
    task::spawn_with_caps_for(
        init,
        caps,
        None,
        generation
            .component_named("init")
            .expect("init component missing")
            .spawn_budget,
    )
    .expect("failed to launch recovery init")
}

/// Whether a layout gives `identity` a slot.
fn layout_declares(layout: &BootLayout<'_>, identity: [u8; 32]) -> bool {
    (0..layout.entry_count())
        .filter_map(|index| layout.entry(index))
        .any(|entry| entry.name_identity == identity)
}

/// Places minted capabilities into the slots the generation's boot layout
/// declares, rather than at literal indices in this file.
///
/// The kernel still mints every capability: what a channel is, and which half
/// of it is the client's, is knowable only here. What the layout supplies is
/// *where each one goes*, which is what used to be a `caps[46] = ...` write
/// whose correctness nothing checked.
///
/// Nothing is placed by position. A capability is offered under the name the
/// layout knows it by, and one that the layout does not mention is a fault
/// rather than a capability appended to the end.
struct LayoutPlacer<'a> {
    layout: BootLayout<'a>,
    slots: [Option<Capability>; MAX_CAPS],
}

impl<'a> LayoutPlacer<'a> {
    fn new(layout: BootLayout<'a>) -> Self {
        Self {
            layout,
            slots: [const { None }; MAX_CAPS],
        }
    }

    /// Place `capability` at whichever slot the layout gives `identity`.
    fn place(&mut self, identity: [u8; 32], what: &str, capability: Capability) {
        let entry = (0..self.layout.entry_count())
            .filter_map(|index| self.layout.entry(index))
            .find(|entry| entry.name_identity == identity)
            .unwrap_or_else(|| panic!("boot layout declares no slot for {what}"));
        let slot = entry.slot as usize;
        assert!(
            self.slots[slot].is_none(),
            "boot layout slot {slot} filled twice, second by {what}"
        );
        assert!(
            capability.rights == entry.rights,
            "boot layout slot {slot} ({what}) declares rights {:#x}, kernel minted {:#x}",
            entry.rights,
            capability.rights
        );
        self.slots[slot] = Some(capability);
    }

    fn executable(&mut self, generation: &Generation<'static>, name: &'static str) {
        let bytes = generation
            .component_bytes(name)
            .unwrap_or_else(|| panic!("{name} object missing"));
        let capability = executable(generation, name, bytes);
        self.place(boot_layout::component_identity(name), name, capability);
    }

    /// Place an executable whose declared rights add `RIGHT_TRANSFER`.
    ///
    /// Whether a participant's image is transferable varies by profile — the
    /// call plane grants it on four of its five, the stream plane on none — so
    /// the layout carries it and this reads it back rather than the caller
    /// restating it.
    fn executable_as_declared(&mut self, generation: &Generation<'static>, name: &'static str) {
        let bytes = generation
            .component_bytes(name)
            .unwrap_or_else(|| panic!("{name} object missing"));
        let mut capability = executable(generation, name, bytes);
        let identity = boot_layout::component_identity(name);
        if let Some(entry) = (0..self.layout.entry_count())
            .filter_map(|index| self.layout.entry(index))
            .find(|entry| entry.name_identity == identity)
        {
            capability.rights = entry.rights;
        }
        self.place(identity, name, capability);
    }

    /// Place whichever of `candidates` this generation's layout declares.
    ///
    /// Used where one slot holds a different component per profile — the
    /// storage slot names four. The kernel offers the set it can build and the
    /// layout picks, so adding a fifth is a manifest change rather than another
    /// arm of a `match generation.number`.
    fn one_of(
        &mut self,
        generation: &Generation<'static>,
        candidates: &[&'static str],
        what: &str,
    ) {
        let chosen = candidates
            .iter()
            .find(|name| self.declares_component(name))
            .unwrap_or_else(|| panic!("boot layout names no {what}"));
        self.executable(generation, chosen);
    }

    /// Whether this generation's layout gives `name` a slot.
    ///
    /// This is what the boot path asks instead of comparing
    /// `generation.number` against a literal. A profile is identified by the
    /// participants it declares, so the condition states why a branch is taken
    /// rather than encoding a number that happens to mean it.
    fn declares_component(&self, name: &str) -> bool {
        self.declares(boot_layout::component_identity(name))
    }

    /// Whether this generation's layout gives the channel half `label` a slot.
    fn declares_channel(&self, label: &str) -> bool {
        self.declares(boot_layout::channel_identity(label))
    }

    fn declares(&self, identity: [u8; 32]) -> bool {
        layout_declares(&self.layout, identity)
    }

    fn endpoint(&mut self, label: &'static str, endpoint: ipc::Endpoint, rights: Rights) {
        let capability = self::endpoint(label, endpoint, rights);
        self.place(boot_layout::channel_identity(label), label, capability);
    }

    /// Place a capability the layout names by role rather than by identity.
    /// Used for the singular objects — one endpoint factory, one input device —
    /// which no name would distinguish.
    ///
    /// A role is not always unique: generation 4 declares an object store in
    /// both the storage and filesystem slots, identical in every field. The
    /// first *unfilled* entry wins, so repeated calls walk them in declared
    /// order rather than all resolving to the first.
    fn role(&mut self, role: boot_layout::Role, what: &str, object: KernelObject) {
        let entry = (0..self.layout.entry_count())
            .filter_map(|index| self.layout.entry(index))
            .find(|entry| entry.role == role && self.slots[entry.slot as usize].is_none())
            .unwrap_or_else(|| panic!("boot layout declares no free slot for {what}"));
        let slot = entry.slot as usize;
        assert!(
            self.slots[slot].is_none(),
            "boot layout slot {slot} filled twice, second by {what}"
        );
        self.slots[slot] = Some(Capability {
            object,
            rights: entry.rights,
        });
    }

    /// The declared entry for a role, when the kernel needs to read its rights
    /// before deciding what object to put there.
    fn entry_for_role(&self, role: boot_layout::Role) -> Option<boot_layout::LayoutEntry> {
        (0..self.layout.entry_count())
            .filter_map(|index| self.layout.entry(index))
            .find(|entry| entry.role == role)
    }

    /// Collapse to the vector `spawn_with_caps_for` takes, checking that the
    /// layout and the kernel agree in both directions.
    ///
    /// A declared slot left empty means the kernel did not mint something the
    /// layout expects; a filled slot the layout does not declare means the
    /// kernel minted something with nowhere to go. Either way the boot stops
    /// here, naming the slot, rather than launching init with a table its
    /// component images do not address.
    fn finish(self) -> alloc::vec::Vec<Capability> {
        let declared: alloc::vec::Vec<usize> = (0..self.layout.entry_count())
            .filter_map(|index| self.layout.entry(index))
            .map(|entry| entry.slot as usize)
            .collect();
        for slot in &declared {
            assert!(
                self.slots[*slot].is_some(),
                "boot layout declares slot {slot}, but the kernel minted nothing for it"
            );
        }
        for (slot, capability) in self.slots.iter().enumerate() {
            assert!(
                capability.is_none() || declared.contains(&slot),
                "kernel minted a capability for slot {slot}, which the layout does not declare"
            );
        }
        let len = declared.iter().copied().max().map_or(0, |slot| slot + 1);
        let mut caps = alloc::vec::Vec::with_capacity(len);
        for slot in self.slots.into_iter().take(len) {
            caps.push(slot.expect("every slot below the high-water mark is declared"));
        }
        caps
    }
}

/// The stable name of a capability's object kind, for the boot-layout dump.
///
/// Names the kind only. Endpoint identity, executable bytes, and block-device
/// addresses vary per boot or per host, so including them would make the dump
/// unstable across runs and useless as an equivalence fixture.
fn object_kind_name(object: &KernelObject) -> &'static str {
    match object {
        KernelObject::Endpoint(_) => "endpoint",
        KernelObject::EndpointFactory => "endpoint-factory",
        KernelObject::SharedBufferFactory => "shared-buffer-factory",
        KernelObject::Input => "input",
        KernelObject::Executable { .. } => "executable",
        KernelObject::Supervision(_) => "supervision",
        KernelObject::PciFunction(_) => "pci-function",
        KernelObject::DmaMemory(_) => "dma-memory",
        KernelObject::Irq(_) => "irq",
        KernelObject::SharedBuffer(_) => "shared-buffer",
        KernelObject::SharedBufferLoan(_) => "shared-buffer-loan",
        KernelObject::BlockDevice(_) => "block-device",
        KernelObject::ObjectStore => "object-store",
        KernelObject::Directory(_) => "directory",
        KernelObject::GenerationControl => "generation-control",
    }
}

/// Emit init's resolved capability layout to the serial log, one line per slot.
///
/// This is the observable form of a layout that is otherwise only implied by
/// source order, so an equivalence check can compare the layout a generation
/// resolves against the layout the same generation resolved before a change.
/// Executables carry their component name; every other kind is identified by
/// kind and rights alone.
fn dump_boot_layout(path: &str, caps: &[Capability]) {
    serial_println!(
        "[layout] path={} slots={} max={}",
        path,
        caps.len(),
        crate::capability::MAX_CAPS
    );
    for (slot, capability) in caps.iter().enumerate() {
        let name = match &capability.object {
            KernelObject::Executable { name, .. } => name.unwrap_or("?"),
            KernelObject::Endpoint(endpoint) => endpoint.label().unwrap_or("?"),
            _ => "-",
        };
        serial_println!(
            "[layout] {} {} {} {:#x}",
            slot,
            object_kind_name(&capability.object),
            name,
            capability.rights
        );
    }
    serial_println!("[layout] end");
}

fn boot_block_function() -> Option<PciFunctionInfo> {
    crate::pci::enumerate()
        .ok()?
        .into_iter()
        .find(|function| function.vendor_id == 0x8086 && function.device_id == 0x2922)
}

fn block_functions() -> alloc::vec::Vec<PciFunctionInfo> {
    crate::pci::enumerate()
        .unwrap_or_default()
        .into_iter()
        .filter(|function| {
            (function.vendor_id == 0x1af4 && function.device_id == 0x1042)
                || function.class_code & 0x00ff_ffff == 0x010802
        })
        .collect()
}

pub fn primary_block_function() -> Option<PciFunctionInfo> {
    block_functions()
        .into_iter()
        .find(|function| function.device == 5)
        .or_else(boot_block_function)
}

fn optional_block_function() -> Option<PciFunctionInfo> {
    crate::pci::enumerate().ok()?.into_iter().find(|function| {
        (function.vendor_id == 0x1af4 && function.device_id == 0x1042)
            || function.class_code & 0x00ff_ffff == 0x010802
    })
}

fn recovery_index<'a>(
    generation: &'a Generation<'a>,
) -> boot_contracts::recovery::RecoveryIndex<'a> {
    let object = (0..generation.object_count())
        .find_map(|index| {
            generation
                .object(index)
                .ok()
                .filter(|object| object.id == "recovery-index")
        })
        .expect("signed recovery index missing");
    boot_contracts::recovery::RecoveryIndex::decode(object.bytes)
        .expect("signed recovery index invalid")
}

fn recovery_block_function(index: &boot_contracts::recovery::RecoveryIndex<'_>) -> PciFunctionInfo {
    crate::pci::enumerate()
        .expect("recovery target enumeration failed")
        .into_iter()
        .find(|function| crate::recovery::packed_bdf(*function) == index.target_pci_bdf)
        .expect("signed recovery target missing")
}

fn executable(
    generation: &Generation<'static>,
    name: &'static str,
    bytes: &'static [u8],
) -> Capability {
    let spawn_budget = generation
        .component_named(name)
        .expect("executable component missing")
        .spawn_budget;
    Capability {
        object: KernelObject::Executable {
            name: Some(name),
            bytes,
            spawn_budget,
        },
        rights: RIGHT_EXEC | RIGHT_SPAWN,
    }
}

// Every endpoint held by init is delegated to a spawned component, so each
// carries RIGHT_TRANSFER: spawn grants enforce the same transfer-right
// condition as IPC sends.
//
// `label` names which half of which channel this is. Endpoints are otherwise
// indistinguishable in the boot-layout dump — most of init's carry identical
// rights — so without it a check could not tell a client half from its service
// half, and a layout that swapped two control endpoints would compare equal.
fn endpoint(label: &'static str, endpoint: ipc::Endpoint, rights: Rights) -> Capability {
    Capability {
        object: KernelObject::Endpoint(endpoint.with_label(label)),
        rights: rights | RIGHT_TRANSFER,
    }
}

fn directory_fixture_root() -> [u8; 32] {
    [
        0xe8, 0xcd, 0xd1, 0x45, 0x6f, 0xe5, 0x4e, 0x59, 0xe3, 0xb6, 0x1a, 0x65, 0x5a, 0x2f, 0xbb,
        0xfa, 0xf1, 0x6d, 0x89, 0xa8, 0x77, 0x0a, 0xa1, 0x08, 0x05, 0x51, 0xbd, 0x84, 0xf6, 0x6b,
        0x0f, 0xf2,
    ]
}

/// Every component that can occupy the storage slot. Which one a generation
/// puts there is the layout's answer, not this list's: the layout carries name
/// identities rather than names, so the kernel offers the candidates and takes
/// whichever the layout claims.
const STORAGE_COMPONENTS: [&str; 4] = [
    "storage-probe",
    "storage-writer",
    "storage-fault-probe",
    "storage-store-probe",
];

fn require_grant<'a>(
    generation: &Generation<'a>,
    name: &str,
    source: &str,
    target: &str,
    rights: Rights,
) -> crate::generation::Grant<'a> {
    (0..generation.grant_count())
        .filter_map(|index| generation.grant(index).ok())
        .find(|grant| {
            grant.name == name
                && generation
                    .component(grant.source)
                    .is_ok_and(|component| component.name == source)
                && generation
                    .component(grant.target)
                    .is_ok_and(|component| component.name == target)
                && grant.rights == rights
        })
        .expect("required grant missing or changed")
}

pub fn record_spawn(component: &'static str, id: task::TaskId) {
    let slot = match component {
        "console" => &CONSOLE_ID,
        "dango" => &DANGO_ID,
        "sysinfo" => &SYSINFO_ID,
        "storage-probe" => &STORAGE_PROBE_ID,
        "storage-writer" => &STORAGE_WRITER_ID,
        "storage-fault-probe" => &STORAGE_FAULT_ID,
        "storage-store-probe" => &STORAGE_STORE_ID,
        "generation-manager" => &GENERATION_MANAGER_ID,
        "spawn-service" => &SPAWN_SERVICE_ID,
        "filesystem-service" => &FILESYSTEM_ID,
        "directory-probe" => &DIRECTORY_PROBE_ID,
        "generation-list" => &GENERATION_LIST_ID,
        "generation-inspect" => &GENERATION_INSPECT_ID,
        "generation-stage" => &GENERATION_STAGE_ID,
        "generation-select" => &GENERATION_SELECT_ID,
        "generation-rollback" => &GENERATION_ROLLBACK_ID,
        "powerbox-chooser" => &POWERBOX_CHOOSER_ID,
        "powerbox-probe" => &POWERBOX_PROBE_ID,
        "sample-lender" => &SAMPLE_LENDER_ID,
        "sample-receiver" => &SAMPLE_RECEIVER_ID,
        "fabric-service" => &FABRIC_SERVICE_ID,
        "fabric-publisher" => &FABRIC_PUBLISHER_ID,
        "fabric-subscriber" => &FABRIC_SUBSCRIBER_ID,
        "fabric-intruder" => &FABRIC_INTRUDER_ID,
        "fabric-call-time" => &FABRIC_CALL_TIME_ID,
        "fabric-publisher-b" => &FABRIC_PUBLISHER_B_ID,
        "fabric-subscriber-b" => &FABRIC_SUBSCRIBER_B_ID,
        "fabric-op-client-b-restart" => &FABRIC_OP_CLIENT_B_RESTART_ID,
        "fabric-call-client" => &FABRIC_CALL_CLIENT_ID,
        "fabric-call-client-b" => &FABRIC_CALL_CLIENT_B_ID,
        "fabric-call-server" => &FABRIC_CALL_SERVER_ID,
        "fabric-op-client" => &FABRIC_OP_CLIENT_ID,
        "fabric-op-client-b" => &FABRIC_OP_CLIENT_B_ID,
        "fabric-op-server" => &FABRIC_OP_SERVER_ID,
        "fabric-op-time" => &FABRIC_OP_TIME_ID,
        "fabric-probe" => &FABRIC_PROBE_ID,
        "fabric-proxy" => &FABRIC_PROXY_ID,
        "fabric-observer" => &FABRIC_OBSERVER_ID,
        "fabric-call-worker" => &FABRIC_CALL_WORKER_ID,
        "fabric-op-worker" => &FABRIC_OP_WORKER_ID,
        "recovery" => &RECOVERY_ID,
        _ => return,
    };
    slot.store(id, Ordering::Relaxed);
    // Install this component's generation-declared shared-buffer quota (C7.3),
    // charged to its own supervision-subtree account. Absent from the budget
    // (or no budget declared) leaves the deny-by-default quota.
    if let Ok(generation) = crate::generation::decode(crate::boot::generation()) {
        task::set_shared_buffer_quota(
            id,
            crate::generation::shared_buffer_quota(&generation, component),
        );
    }
}

fn storage_probe_required() -> bool {
    crate::pci::enumerate().is_ok_and(|functions| {
        functions.iter().any(|function| {
            function.vendor_id == 0x1af4 && matches!(function.device_id, 0x1001 | 0x1042)
        })
    })
}

extern "C" fn on_idle() {
    // An interactive session must keep running so a human keystroke can wake
    // the blocked REPL; it never auto-exits. `idle_dispatch` parks the CPU
    // (`sti; hlt`) until a wake source re-readies a task, then runs it.
    if option_env!("SLIME_INTERACTIVE") == Some("1") {
        serial_println!("[generation] interactive session idle; awaiting input");
        task::idle_dispatch();
    }
    let directory_run = GENERATION_NUMBER.load(Ordering::Relaxed) == 6;
    let checks = [
        ("init", INIT_ID.load(Ordering::Relaxed)),
        ("console", CONSOLE_ID.load(Ordering::Relaxed)),
        ("dango", DANGO_ID.load(Ordering::Relaxed)),
        ("sysinfo", SYSINFO_ID.load(Ordering::Relaxed)),
        ("storage-probe", STORAGE_PROBE_ID.load(Ordering::Relaxed)),
        ("storage-writer", STORAGE_WRITER_ID.load(Ordering::Relaxed)),
        (
            "storage-fault-probe",
            STORAGE_FAULT_ID.load(Ordering::Relaxed),
        ),
        (
            "storage-store-probe",
            STORAGE_STORE_ID.load(Ordering::Relaxed),
        ),
        (
            "generation-manager",
            GENERATION_MANAGER_ID.load(Ordering::Relaxed),
        ),
        ("spawn-service", SPAWN_SERVICE_ID.load(Ordering::Relaxed)),
        ("filesystem-service", FILESYSTEM_ID.load(Ordering::Relaxed)),
        (
            "directory-probe",
            DIRECTORY_PROBE_ID.load(Ordering::Relaxed),
        ),
        (
            "generation-list",
            GENERATION_LIST_ID.load(Ordering::Relaxed),
        ),
        (
            "generation-inspect",
            GENERATION_INSPECT_ID.load(Ordering::Relaxed),
        ),
        (
            "generation-stage",
            GENERATION_STAGE_ID.load(Ordering::Relaxed),
        ),
        (
            "generation-select",
            GENERATION_SELECT_ID.load(Ordering::Relaxed),
        ),
        (
            "generation-rollback",
            GENERATION_ROLLBACK_ID.load(Ordering::Relaxed),
        ),
        ("recovery", RECOVERY_ID.load(Ordering::Relaxed)),
        (
            "powerbox-chooser",
            POWERBOX_CHOOSER_ID.load(Ordering::Relaxed),
        ),
        ("powerbox-probe", POWERBOX_PROBE_ID.load(Ordering::Relaxed)),
        ("sample-lender", SAMPLE_LENDER_ID.load(Ordering::Relaxed)),
        (
            "sample-receiver",
            SAMPLE_RECEIVER_ID.load(Ordering::Relaxed),
        ),
        ("fabric-service", FABRIC_SERVICE_ID.load(Ordering::Relaxed)),
        (
            "fabric-publisher",
            FABRIC_PUBLISHER_ID.load(Ordering::Relaxed),
        ),
        (
            "fabric-subscriber",
            FABRIC_SUBSCRIBER_ID.load(Ordering::Relaxed),
        ),
        (
            "fabric-intruder",
            FABRIC_INTRUDER_ID.load(Ordering::Relaxed),
        ),
        (
            "fabric-call-client",
            FABRIC_CALL_CLIENT_ID.load(Ordering::Relaxed),
        ),
        (
            "fabric-call-client-b",
            FABRIC_CALL_CLIENT_B_ID.load(Ordering::Relaxed),
        ),
        (
            "fabric-call-time",
            FABRIC_CALL_TIME_ID.load(Ordering::Relaxed),
        ),
        (
            "fabric-call-server",
            FABRIC_CALL_SERVER_ID.load(Ordering::Relaxed),
        ),
        (
            "fabric-op-client",
            FABRIC_OP_CLIENT_ID.load(Ordering::Relaxed),
        ),
        (
            "fabric-op-client-b",
            FABRIC_OP_CLIENT_B_ID.load(Ordering::Relaxed),
        ),
        (
            "fabric-op-client-b-restart",
            FABRIC_OP_CLIENT_B_RESTART_ID.load(Ordering::Relaxed),
        ),
        (
            "fabric-op-server",
            FABRIC_OP_SERVER_ID.load(Ordering::Relaxed),
        ),
        ("fabric-op-time", FABRIC_OP_TIME_ID.load(Ordering::Relaxed)),
        // The second stream pair. Tracked by `record_spawn` since C8.4 but never
        // swept, so until now a crashed `fabric-publisher-b` left the slice
        // "healthy".
        (
            "fabric-publisher-b",
            FABRIC_PUBLISHER_B_ID.load(Ordering::Relaxed),
        ),
        (
            "fabric-subscriber-b",
            FABRIC_SUBSCRIBER_B_ID.load(Ordering::Relaxed),
        ),
        // C8.10's three split roles and two route workers. Absent from this
        // array their failure would be invisible: `record_spawn` tracks them, so
        // a crashed proxy or a worker that never provisioned would leave the
        // slice "healthy" while the boot gate's markers merely went missing.
        ("fabric-probe", FABRIC_PROBE_ID.load(Ordering::Relaxed)),
        ("fabric-proxy", FABRIC_PROXY_ID.load(Ordering::Relaxed)),
        (
            "fabric-observer",
            FABRIC_OBSERVER_ID.load(Ordering::Relaxed),
        ),
        (
            "fabric-call-worker",
            FABRIC_CALL_WORKER_ID.load(Ordering::Relaxed),
        ),
        (
            "fabric-op-worker",
            FABRIC_OP_WORKER_ID.load(Ordering::Relaxed),
        ),
    ];
    let mut healthy = true;
    for (name, id) in checks {
        if id == 0 {
            continue;
        }
        // The ready queue has drained to reach `on_idle`, so any component that
        // has not terminated is cleanly parked in `SYS_WAIT` (Blocked). A
        // long-lived service parked this way is healthy idle; a one-shot probe
        // that never reached `Exit(0)` is a genuine stall and stays unhealthy.
        if task::is_live(id) {
            // C8.10's exit condition *is* blocked idle: every role provisions
            // and then parks on its control endpoint with no traffic. So under
            // the boot check a live fabric task is the healthy outcome, not a
            // stall — the opposite of every other fabric gate, where a live
            // participant means a scenario that never finished.
            //
            // Enumerated rather than matched on a `fabric-` prefix, like every
            // other arm here: a prefix would silently extend this verdict to any
            // future component whose name happened to start that way, including a
            // one-shot probe whose failure to exit is a real stall.
            let fabric_boot_idle = GENERATION_NUMBER.load(Ordering::Relaxed) == 17
                && (name == "init"
                    || FABRIC_BOOT_PARTICIPANTS
                        .iter()
                        .any(|(component, _, _)| *component == name)
                    || matches!(
                        name,
                        "fabric-service" | "fabric-call-worker" | "fabric-op-worker"
                    ));
            let persistent = fabric_boot_idle
                || matches!(
                    name,
                    "console"
                        | "dango"
                        | "spawn-service"
                        | "filesystem-service"
                        | "generation-manager"
                        | "powerbox-chooser"
                );
            serial_println!(
                "[generation] {} idle-blocked (persistent={})",
                name,
                persistent
            );
            healthy &= persistent;
            continue;
        }
        let reason = task::termination_summary(id);
        serial_println!("[generation] {} terminated: {:?}", name, reason);
        let optional_storage_absent = name == "storage-probe"
            && (!storage_probe_required() || directory_run)
            && matches!(reason, Some(task::TermReason::Exit(1)));
        let dango_check = GENERATION_NUMBER.load(Ordering::Relaxed) == 7;
        let generation_command_check = GENERATION_NUMBER.load(Ordering::Relaxed) == 8;
        let powerbox_check = GENERATION_NUMBER.load(Ordering::Relaxed) == 9;
        let sample_plane_check = GENERATION_NUMBER.load(Ordering::Relaxed) == 10;
        let fabric_authority_check = GENERATION_NUMBER.load(Ordering::Relaxed) == 11;
        let fabric_stream_check = GENERATION_NUMBER.load(Ordering::Relaxed) == 12;
        let fabric_qos_check = GENERATION_NUMBER.load(Ordering::Relaxed) == 13;
        let fabric_call_check = GENERATION_NUMBER.load(Ordering::Relaxed) == 14;
        let fabric_operation_check = GENERATION_NUMBER.load(Ordering::Relaxed) == 15;
        let fabric_visibility_check = GENERATION_NUMBER.load(Ordering::Relaxed) == 16;
        let optional_generation_command_component = generation_command_check
            && matches!(name, "init" | "generation-manager")
            && matches!(
                reason,
                Some(task::TermReason::Exit(0) | task::TermReason::PeerLoss)
            );
        let optional_confirmation_absent = name == "generation-manager"
            && (directory_run || dango_check)
            && matches!(reason, Some(task::TermReason::Exit(1)));
        let optional_dango_check_service = dango_check
            && matches!(
                name,
                "init" | "console" | "dango" | "spawn-service" | "filesystem-service"
            )
            && matches!(
                reason,
                Some(task::TermReason::Exit(0) | task::TermReason::PeerLoss)
            );
        let optional_dango_check_probe = dango_check
            && name == "directory-probe"
            && matches!(reason, Some(task::TermReason::Exit(1)));
        let optional_powerbox_component = powerbox_check
            && matches!(
                name,
                "init" | "console" | "powerbox-chooser" | "powerbox-probe"
            )
            && matches!(
                reason,
                Some(task::TermReason::Exit(0) | task::TermReason::PeerLoss)
            );
        let optional_powerbox_manager = powerbox_check
            && name == "generation-manager"
            && matches!(reason, Some(task::TermReason::Exit(1)));
        // The sample-plane scenario runs a bounded two-component exchange and
        // exits; the services it does not use may cleanly terminate or lose a
        // peer as init tears the graph down.
        let optional_sample_plane_component = sample_plane_check
            && matches!(
                name,
                "init"
                    | "console"
                    | "dango"
                    | "spawn-service"
                    | "filesystem-service"
                    | "sample-lender"
                    | "sample-receiver"
            )
            && matches!(
                reason,
                Some(task::TermReason::Exit(0) | task::TermReason::PeerLoss)
            );
        let optional_sample_plane_manager = sample_plane_check
            && name == "generation-manager"
            && matches!(reason, Some(task::TermReason::Exit(1)));
        // The fabric-authority scenario runs a bounded provisioning exchange
        // and exits; the services it does not use may cleanly terminate or lose
        // a peer as init tears the graph down.
        let optional_fabric_component = fabric_authority_check
            && matches!(
                name,
                "init"
                    | "console"
                    | "dango"
                    | "spawn-service"
                    | "filesystem-service"
                    | "fabric-service"
                    | "fabric-publisher"
                    | "fabric-subscriber"
                    | "fabric-intruder"
            )
            && matches!(
                reason,
                Some(task::TermReason::Exit(0) | task::TermReason::PeerLoss)
            );
        let optional_fabric_manager = (fabric_authority_check
            || fabric_stream_check
            || fabric_qos_check
            || fabric_call_check
            || fabric_operation_check
            || fabric_visibility_check)
            && name == "generation-manager"
            && matches!(reason, Some(task::TermReason::Exit(1)));
        // The stream scenario runs the same graph as the authority one, plus
        // the two components that make the fan-out many-to-many.
        let optional_fabric_stream_component = fabric_stream_check
            && matches!(
                name,
                "init"
                    | "console"
                    | "dango"
                    | "spawn-service"
                    | "filesystem-service"
                    | "fabric-service"
                    | "fabric-publisher"
                    | "fabric-subscriber"
                    | "fabric-intruder"
                    | "fabric-publisher-b"
                    | "fabric-subscriber-b"
            )
            && matches!(
                reason,
                Some(task::TermReason::Exit(0) | task::TermReason::PeerLoss)
            );
        let optional_fabric_qos_component = fabric_qos_check
            && matches!(
                name,
                "init"
                    | "console"
                    | "dango"
                    | "spawn-service"
                    | "filesystem-service"
                    | "fabric-service"
                    | "fabric-publisher"
                    | "fabric-subscriber"
                    | "fabric-intruder"
                    | "fabric-publisher-b"
                    | "fabric-subscriber-b"
            )
            && matches!(
                reason,
                Some(task::TermReason::Exit(0) | task::TermReason::PeerLoss)
            );
        let optional_fabric_call_component = fabric_call_check
            && matches!(
                name,
                "init"
                    | "fabric-service"
                    | "fabric-call-client"
                    | "fabric-call-client-b"
                    | "fabric-call-server"
            )
            && matches!(
                reason,
                Some(task::TermReason::Exit(0) | task::TermReason::PeerLoss)
            );
        // The operation scenario runs a bounded composition and exits. The
        // server exits deliberately to inject peer death, and the clients follow
        // once their arms are asserted, so a clean exit or a lost peer is the
        // expected end state for every participant.
        let optional_fabric_operation_component = fabric_operation_check
            && matches!(
                name,
                "init"
                    | "fabric-service"
                    | "fabric-op-client"
                    | "fabric-op-client-b"
                    | "fabric-op-client-b-restart"
                    | "fabric-op-server"
                    | "fabric-op-time"
            )
            && matches!(
                reason,
                Some(task::TermReason::Exit(0) | task::TermReason::PeerLoss)
            );
        let optional_fabric_visibility_component = fabric_visibility_check
            && matches!(
                name,
                "init"
                    | "fabric-service"
                    | "fabric-publisher"
                    | "fabric-subscriber"
                    | "fabric-intruder"
                    | "fabric-publisher-b"
                    | "fabric-subscriber-b"
            )
            && matches!(
                reason,
                Some(task::TermReason::Exit(0) | task::TermReason::PeerLoss)
            );
        healthy &= matches!(reason, Some(task::TermReason::Exit(0)))
            || optional_storage_absent
            || optional_confirmation_absent
            || optional_dango_check_service
            || optional_dango_check_probe
            || optional_generation_command_component
            || optional_powerbox_component
            || optional_powerbox_manager
            || optional_sample_plane_component
            || optional_sample_plane_manager
            || optional_fabric_component
            || optional_fabric_manager
            || optional_fabric_stream_component
            || optional_fabric_qos_component
            || optional_fabric_call_component
            || optional_fabric_operation_component
            || optional_fabric_visibility_component;
    }
    if healthy {
        if crate::boot::bootstate().is_some_and(|state| state.running_pending) {
            serial_println!("[generation] pending generation healthy; awaiting confirmation");
        } else {
            serial_println!("[generation] vertical slice healthy");
            println!("[generation] vertical slice healthy");
        }
        crate::exit_qemu(crate::QemuExitCode::Success);
    } else {
        crate::generation_manager::mark_unhealthy();
        println!("[generation] vertical slice failed");
        crate::exit_qemu(crate::QemuExitCode::Failed);
    }
    crate::hlt_loop()
}
