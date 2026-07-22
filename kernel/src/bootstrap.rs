use alloc::vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::capability::{
    Capability, DirectoryAuthority, KernelObject, PciFunctionInfo, RIGHT_BLOCK_READ,
    RIGHT_BLOCK_WRITE, RIGHT_BOOT_UPDATE, RIGHT_DIRECTORY_DERIVE, RIGHT_DIRECTORY_LIST,
    RIGHT_DIRECTORY_READ, RIGHT_DIRECTORY_WRITE, RIGHT_EXEC, RIGHT_HEALTH_CONFIRM, RIGHT_RECV,
    RIGHT_SEND, RIGHT_SPAWN, RIGHT_STORE_READ, RIGHT_STORE_WRITE, RIGHT_TRANSFER,
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
        "init",
        "spawn-service",
        RIGHT_EXEC | RIGHT_SPAWN,
    );
    serial_println!("[generation] sysinfo executable grant valid");
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
    serial_println!("[generation] filesystem grants valid");
    let (storage_component, storage_capability) = match generation.number {
        2 => (
            storage_writer,
            Capability {
                object: KernelObject::BlockDevice(default_block_function()),
                rights: RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE | RIGHT_TRANSFER,
            },
        ),
        3 => (
            storage_fault_probe,
            Capability {
                object: KernelObject::BlockDevice(default_block_function()),
                rights: RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE | RIGHT_TRANSFER,
            },
        ),
        4 => (
            storage_store_probe,
            Capability {
                object: KernelObject::ObjectStore,
                rights: RIGHT_STORE_READ | RIGHT_STORE_WRITE | RIGHT_TRANSFER,
            },
        ),
        _ => (
            storage_probe,
            Capability {
                object: KernelObject::BlockDevice(default_block_function()),
                rights: RIGHT_BLOCK_READ | RIGHT_TRANSFER,
            },
        ),
    };
    serial_println!("[generation] bootstrap grants valid");

    let (console_output, dango_output) = ipc::channel();
    let (dango_spawn, service_spawn) = ipc::channel();
    let (directory_client, directory_service) = ipc::channel();
    let caps = vec![
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
            storage_component_name(generation.number),
            storage_component,
        ),
        storage_capability,
        executable(generation, "generation-manager", generation_manager),
        Capability {
            object: KernelObject::GenerationControl,
            rights: RIGHT_HEALTH_CONFIRM | RIGHT_TRANSFER,
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
    ];

    let spawn_budget = generation
        .component_named("init")
        .expect("init component missing")
        .spawn_budget;
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

fn default_block_function() -> PciFunctionInfo {
    crate::pci::enumerate()
        .expect("block-device enumeration failed")
        .into_iter()
        .find(|function| {
            (function.vendor_id == 0x1af4 && function.device_id == 0x1042)
                || function.class_code & 0x00ff_ffff == 0x010802
        })
        .expect("block device missing")
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
    let grant = generation
        .grant_named(name)
        .expect("required grant missing");
    let source_name = generation
        .component(grant.source)
        .expect("grant source")
        .name;

    let target_name = generation
        .component(grant.target)
        .expect("grant target")
        .name;
    assert_eq!(
        (source_name, target_name, grant.rights),
        (source, target, rights),
        "grant declaration changed"
    );
    grant
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
        ("recovery", RECOVERY_ID.load(Ordering::Relaxed)),
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
        let optional_confirmation_absent = name == "generation-manager"
            && directory_run
            && matches!(reason, Some(task::TermReason::Exit(1)));
        healthy &= matches!(reason, Some(task::TermReason::Exit(0)))
            || optional_storage_absent
            || optional_confirmation_absent;
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
