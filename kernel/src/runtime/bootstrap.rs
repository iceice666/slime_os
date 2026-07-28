use alloc::vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::capability::{
    Capability, DirectoryAuthority, KernelObject, PciFunctionInfo, RIGHT_BLOCK_READ,
    RIGHT_BLOCK_WRITE, RIGHT_BOOT_UPDATE, RIGHT_DIRECTORY_DERIVE, RIGHT_DIRECTORY_LIST,
    RIGHT_DIRECTORY_READ, RIGHT_DIRECTORY_WRITE, RIGHT_EXEC, RIGHT_HEALTH_CONFIRM,
    RIGHT_INPUT_READ, RIGHT_RECV, RIGHT_SEND, RIGHT_SPAWN, RIGHT_STORE_READ, RIGHT_STORE_WRITE,
    RIGHT_SUPERVISE, RIGHT_TRANSFER, Rights,
};
use crate::generation::{self, Generation};
use crate::{ipc, println, serial_println, task};

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
    if option_env!("SLIME_GENERATION_CMD_CHECK") == Some("1") && generation.number == 8 {
        serial_println!("[generation-command] scripted check active");
    }
    if option_env!("SLIME_DANGO_CHECK") == Some("1") && generation.number == 7 {
        crate::input::install_script(
            b"$(sysinfo)\n(with-env {MODE=ci} (with-cwd docs (with-stdin data $(echo ok))))\n$(inject)\n$(echo a b c)\n\x1b",
        );
    }
    if option_env!("SLIME_POWERBOX_CHECK") == Some("1") && generation.number == 9 {
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
    let init = generation
        .component_bytes("init")
        .expect("init object missing");
    let console = generation
        .component_bytes("console")
        .expect("console object missing");
    let dango = generation
        .component_bytes("dango")
        .expect("dango object missing");
    let sysinfo = generation
        .component_bytes("sysinfo")
        .expect("sysinfo object missing");
    let storage_probe = generation
        .component_bytes("storage-probe")
        .expect("storage-probe object missing");
    let storage_writer = generation
        .component_bytes("storage-writer")
        .expect("storage-writer object missing");
    let storage_fault_probe = generation
        .component_bytes("storage-fault-probe")
        .expect("storage-fault-probe object missing");
    let storage_store_probe = generation
        .component_bytes("storage-store-probe")
        .expect("storage-store-probe object missing");
    let generation_manager = generation
        .component_bytes("generation-manager")
        .expect("generation-manager object missing");
    let spawn_service = generation
        .component_bytes("spawn-service")
        .expect("spawn-service object missing");
    let filesystem_service = generation
        .component_bytes("filesystem-service")
        .expect("filesystem-service object missing");
    let directory_probe = generation
        .component_bytes("directory-probe")
        .expect("directory-probe object missing");
    let generation_list = generation
        .component_bytes("generation-list")
        .expect("generation-list object missing");
    let generation_inspect = generation
        .component_bytes("generation-inspect")
        .expect("generation-inspect object missing");
    let generation_stage = generation
        .component_bytes("generation-stage")
        .expect("generation-stage object missing");
    let generation_select = generation
        .component_bytes("generation-select")
        .expect("generation-select object missing");
    let generation_rollback = generation
        .component_bytes("generation-rollback")
        .expect("generation-rollback object missing");
    let powerbox_chooser = generation
        .component_bytes("powerbox-chooser")
        .expect("powerbox-chooser object missing");
    let powerbox_probe = generation
        .component_bytes("powerbox-probe")
        .expect("powerbox-probe object missing");
    let sample_lender = generation
        .component_bytes("sample-lender")
        .expect("sample-lender object missing");
    let sample_receiver = generation
        .component_bytes("sample-receiver")
        .expect("sample-receiver object missing");
    let fabric_service = generation
        .component_bytes("fabric-service")
        .expect("fabric-service object missing");
    let fabric_publisher = generation
        .component_bytes("fabric-publisher")
        .expect("fabric-publisher object missing");
    let fabric_subscriber = generation
        .component_bytes("fabric-subscriber")
        .expect("fabric-subscriber object missing");
    let fabric_intruder = generation
        .component_bytes("fabric-intruder")
        .expect("fabric-intruder object missing");
    let fabric_publisher_b = generation
        .component_bytes("fabric-publisher-b")
        .expect("fabric-publisher-b object missing");
    let fabric_subscriber_b = generation
        .component_bytes("fabric-subscriber-b")
        .expect("fabric-subscriber-b object missing");
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
    let storage_capability = match generation.number {
        2 | 3 => optional_block_function().map(|function| Capability {
            object: KernelObject::BlockDevice(function),
            rights: RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE | RIGHT_TRANSFER,
        }),
        4 => Some(Capability {
            object: KernelObject::ObjectStore,
            rights: RIGHT_STORE_READ | RIGHT_STORE_WRITE | RIGHT_TRANSFER,
        }),
        _ => optional_block_function().map(|function| Capability {
            object: KernelObject::BlockDevice(function),
            rights: RIGHT_BLOCK_READ | RIGHT_TRANSFER,
        }),
    };
    let transfer_functions = block_functions();
    let transfer_receiver = transfer_functions
        .iter()
        .find(|function| function.device == 5)
        .copied();
    let transfer_source = transfer_functions
        .iter()
        .find(|function| function.device == 6)
        .copied();

    let storage_component = match generation.number {
        2 => storage_writer,
        3 => storage_fault_probe,
        4 => storage_store_probe,
        _ => storage_probe,
    };
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
    let mut caps = vec![
        Capability {
            object: KernelObject::EndpointFactory,
            rights: crate::capability::RIGHT_ENDPOINT_CREATE,
        },
        executable(generation, "console", console),
        endpoint(console_output, RIGHT_RECV),
        executable(generation, "dango", dango),
        endpoint(dango_output, RIGHT_SEND),
        executable(generation, "spawn-service", spawn_service),
        executable(generation, "sysinfo", sysinfo),
        executable(
            generation,
            "echo-agent",
            generation
                .component_bytes("echo-agent")
                .expect("echo-agent object missing"),
        ),
        executable(
            generation,
            storage_component_name(generation.number),
            storage_component,
        ),
    ];
    caps.push(storage_capability.unwrap_or(Capability {
        object: KernelObject::ObjectStore,
        rights: RIGHT_STORE_READ,
    }));
    caps.extend([
        executable(generation, "generation-manager", generation_manager),
        Capability {
            object: KernelObject::GenerationControl,
            rights: RIGHT_HEALTH_CONFIRM | RIGHT_BOOT_UPDATE | RIGHT_TRANSFER,
        },
        endpoint(dango_spawn, RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER),
        endpoint(service_spawn, RIGHT_SEND | RIGHT_RECV),
        executable(generation, "filesystem-service", filesystem_service),
        executable(generation, "directory-probe", directory_probe),
        endpoint(directory_client, RIGHT_SEND | RIGHT_RECV),
        endpoint(directory_service, RIGHT_SEND | RIGHT_RECV),
        Capability {
            object: KernelObject::ObjectStore,
            rights: RIGHT_STORE_READ | RIGHT_STORE_WRITE | RIGHT_TRANSFER,
        },
        Capability {
            object: KernelObject::Directory(DirectoryAuthority::root(directory_fixture_root())),
            rights: RIGHT_DIRECTORY_READ
                | RIGHT_DIRECTORY_WRITE
                | RIGHT_DIRECTORY_LIST
                | RIGHT_DIRECTORY_DERIVE
                | RIGHT_TRANSFER,
        },
        Capability {
            object: KernelObject::Input,
            rights: RIGHT_INPUT_READ | RIGHT_TRANSFER,
        },
        executable(generation, "generation-list", generation_list),
        executable(generation, "generation-inspect", generation_inspect),
        executable(generation, "generation-stage", generation_stage),
        executable(generation, "generation-select", generation_select),
        executable(generation, "generation-rollback", generation_rollback),
        endpoint(generation_list_client, RIGHT_SEND | RIGHT_RECV),
        endpoint(generation_inspect_client, RIGHT_SEND | RIGHT_RECV),
        endpoint(generation_stage_client, RIGHT_SEND | RIGHT_RECV),
        endpoint(generation_select_client, RIGHT_SEND | RIGHT_RECV),
        endpoint(generation_rollback_client, RIGHT_SEND | RIGHT_RECV),
        endpoint(
            generation_list_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        ),
        endpoint(
            generation_inspect_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        ),
        endpoint(
            generation_stage_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        ),
        endpoint(
            generation_select_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        ),
        endpoint(
            generation_rollback_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        ),
        executable(generation, "powerbox-chooser", powerbox_chooser),
        executable(generation, "powerbox-probe", powerbox_probe),
        endpoint(powerbox_client, RIGHT_SEND | RIGHT_RECV),
        endpoint(powerbox_service, RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER),
        // C7.2 shared-buffer factory, slot 40. Placed before the optional
        // transfer block so its slot is fixed on every boot. Creation authority
        // only, and transferable so init can derive-copy it into the components
        // the generation grants it to. A holder still allocates nothing without
        // a `shared-buffer-budget` entry (C7.3): the grant authorizes the
        // operation, the budget bounds it.
        Capability {
            object: KernelObject::SharedBufferFactory,
            rights: crate::capability::RIGHT_BUFFER_CREATE | RIGHT_TRANSFER,
        },
        // C7.7 sample plane, slots 41-44. Two real components exchange a
        // >MAX_MSG payload through the shared-buffer syscalls; init spawns both
        // and hands the lender the receiver's supervision handle so the loan
        // names its receiver by capability rather than an ambient task id.
        executable(generation, "sample-lender", sample_lender),
        executable(generation, "sample-receiver", sample_receiver),
        endpoint(sample_lender_side, RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER),
        endpoint(
            sample_receiver_side,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        ),
        // C8.3/C8.4 fabric control plane, slots 45-58. The fabric holds one
        // control endpoint per client and its own endpoint and shared-buffer
        // factory grants; each client holds only its half of its control
        // channel. Route endpoints are not minted here: the fabric creates them
        // and moves each participant a narrowed, non-transferable role through
        // `SYS_CAP_TRANSFER`, so a route capability never exists in init's
        // table at all.
        executable(generation, "fabric-service", fabric_service),
        executable(generation, "fabric-publisher", fabric_publisher),
        executable(generation, "fabric-subscriber", fabric_subscriber),
        executable(generation, "fabric-intruder", fabric_intruder),
        executable(generation, "fabric-publisher-b", fabric_publisher_b),
        executable(generation, "fabric-subscriber-b", fabric_subscriber_b),
        endpoint(fabric_publisher_client, RIGHT_SEND | RIGHT_RECV),
        endpoint(fabric_subscriber_client, RIGHT_SEND | RIGHT_RECV),
        endpoint(fabric_intruder_client, RIGHT_SEND | RIGHT_RECV),
        endpoint(fabric_publisher_b_client, RIGHT_SEND | RIGHT_RECV),
        endpoint(fabric_subscriber_b_client, RIGHT_SEND | RIGHT_RECV),
        // The service side of each control channel. Transferable so init can
        // grant them into the fabric; the fabric itself never re-delegates one.
        endpoint(
            fabric_publisher_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        ),
        endpoint(
            fabric_subscriber_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        ),
        endpoint(
            fabric_intruder_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        ),
        endpoint(
            fabric_publisher_b_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        ),
        endpoint(
            fabric_subscriber_b_service,
            RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        ),
    ]);
    if generation.number == 13 {
        caps.extend([
            endpoint(fabric_time_client, RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER),
            endpoint(
                fabric_time_service,
                RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
            ),
        ]);
    }
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

    let spawn_budget = generation
        .component_named("init")
        .expect("init component missing")
        .spawn_budget;
    serial_println!("[generation] spawning init");
    serial_println!(
        "[generation] launching init with {} capabilities",
        caps.len()
    );
    task::spawn_with_caps_for(init, caps, None, spawn_budget).expect("failed to launch init")
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
    let recovery = generation
        .component_bytes("recovery")
        .expect("recovery object missing");
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
fn endpoint(endpoint: ipc::Endpoint, rights: Rights) -> Capability {
    Capability {
        object: KernelObject::Endpoint(endpoint),
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

fn storage_component_name(generation: u64) -> &'static str {
    match generation {
        2 => "storage-writer",
        3 => "storage-fault-probe",
        4 => "storage-store-probe",
        _ => "storage-probe",
    }
}

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
        "fabric-publisher-b" => &FABRIC_PUBLISHER_B_ID,
        "fabric-subscriber-b" => &FABRIC_SUBSCRIBER_B_ID,
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
            let persistent = matches!(
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
        let dango_check = option_env!("SLIME_DANGO_CHECK") == Some("1")
            && GENERATION_NUMBER.load(Ordering::Relaxed) == 7;
        let generation_command_check = option_env!("SLIME_GENERATION_CMD_CHECK") == Some("1")
            && GENERATION_NUMBER.load(Ordering::Relaxed) == 8;
        let powerbox_check = option_env!("SLIME_POWERBOX_CHECK") == Some("1")
            && GENERATION_NUMBER.load(Ordering::Relaxed) == 9;
        let sample_plane_check = option_env!("SLIME_SAMPLE_PLANE_CHECK") == Some("1")
            && GENERATION_NUMBER.load(Ordering::Relaxed) == 10;
        let fabric_authority_check = option_env!("SLIME_FABRIC_AUTHORITY_CHECK") == Some("1")
            && GENERATION_NUMBER.load(Ordering::Relaxed) == 11;
        let fabric_stream_check = option_env!("SLIME_FABRIC_STREAM_CHECK") == Some("1")
            && GENERATION_NUMBER.load(Ordering::Relaxed) == 12;
        let fabric_qos_check = option_env!("SLIME_FABRIC_QOS_CHECK") == Some("1")
            && GENERATION_NUMBER.load(Ordering::Relaxed) == 13;
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
        let optional_fabric_manager =
            (fabric_authority_check || fabric_stream_check || fabric_qos_check)
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
            || optional_fabric_qos_component;
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
