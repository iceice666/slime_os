use alloc::vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::capability::{
    Capability, DirectoryAuthority, KernelObject, PciFunctionInfo, RIGHT_BLOCK_READ,
    RIGHT_BLOCK_WRITE, RIGHT_BOOT_UPDATE, RIGHT_DIRECTORY_DERIVE, RIGHT_DIRECTORY_LIST,
    RIGHT_DIRECTORY_READ, RIGHT_DIRECTORY_WRITE, RIGHT_EXEC, RIGHT_HEALTH_CONFIRM,
    RIGHT_INPUT_READ, RIGHT_RECV, RIGHT_SEND, RIGHT_SPAWN, RIGHT_STORE_READ, RIGHT_STORE_WRITE,
    RIGHT_TRANSFER,
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
    let init_id = launch_init(&generation);
    INIT_ID.store(init_id, Ordering::Relaxed);
    task::set_on_idle(on_idle);
    task::run()
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
    ]);
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
fn endpoint(endpoint: ipc::Endpoint, rights: u32) -> Capability {
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
    rights: u32,
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
        "recovery" => &RECOVERY_ID,
        _ => return,
    };
    slot.store(id, Ordering::Relaxed);
}

fn storage_probe_required() -> bool {
    crate::pci::enumerate().is_ok_and(|functions| {
        functions.iter().any(|function| {
            function.vendor_id == 0x1af4 && matches!(function.device_id, 0x1001 | 0x1042)
        })
    })
}

extern "C" fn on_idle() {
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
    ];
    let mut healthy = true;
    for (name, id) in checks {
        if id == 0 {
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
        healthy &= matches!(reason, Some(task::TermReason::Exit(0)))
            || optional_storage_absent
            || optional_confirmation_absent
            || optional_dango_check_service
            || optional_dango_check_probe
            || optional_generation_command_component
            || optional_powerbox_component
            || optional_powerbox_manager;
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
