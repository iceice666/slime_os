use alloc::vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::capability::{
    Capability, KernelObject, RIGHT_BLOCK_READ, RIGHT_BLOCK_WRITE, RIGHT_EXEC,
    RIGHT_HEALTH_CONFIRM, RIGHT_RECV, RIGHT_SEND, RIGHT_STORE_READ, RIGHT_STORE_WRITE,
    RIGHT_TRANSFER,
};
use crate::generation::{self, Generation};
use crate::{ipc, println, serial_println, task};

static INIT_ID: AtomicU64 = AtomicU64::new(0);
static CONSOLE_ID: AtomicU64 = AtomicU64::new(0);
static DANGO_ID: AtomicU64 = AtomicU64::new(0);
static SYSINFO_ID: AtomicU64 = AtomicU64::new(0);
static ECHO_ID: AtomicU64 = AtomicU64::new(0);
static STORAGE_PROBE_ID: AtomicU64 = AtomicU64::new(0);
static STORAGE_WRITER_ID: AtomicU64 = AtomicU64::new(0);
static STORAGE_FAULT_ID: AtomicU64 = AtomicU64::new(0);
static STORAGE_STORE_ID: AtomicU64 = AtomicU64::new(0);
static GENERATION_MANAGER_ID: AtomicU64 = AtomicU64::new(0);

pub fn start() -> ! {
    let bytes = crate::boot::generation();
    let generation = generation::decode(bytes).expect("invalid generation manifest");
    assert_eq!(
        generation.identity,
        crate::boot::generation_identity(),
        "handoff generation identity mismatch"
    );
    crate::generation_manager::init();
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
    let echo = generation
        .component_bytes("echo-agent")
        .expect("echo-agent object missing");
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

    require_grant(generation, "console-output", "console", "dango");
    require_grant(generation, "system-information", "init", "sysinfo");
    require_grant(generation, "echo-request", "echo-agent", "dango");
    require_grant(generation, "echo-reply", "dango", "echo-agent");
    require_grant(generation, "block-read", "init", "storage-probe");
    require_grant(generation, "block-write-check", "init", "storage-writer");
    require_grant(
        generation,
        "block-fault-check",
        "init",
        "storage-fault-probe",
    );
    require_grant(
        generation,
        "health-confirmation",
        "init",
        "generation-manager",
    );
    require_grant(generation, "store-access", "init", "storage-store-probe");
    let (storage_component, storage_capability) = match generation.number {
        2 => (
            storage_writer,
            Capability {
                object: KernelObject::BlockDevice,
                rights: RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE | RIGHT_TRANSFER,
            },
        ),
        3 => (
            storage_fault_probe,
            Capability {
                object: KernelObject::BlockDevice,
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
                object: KernelObject::BlockDevice,
                rights: RIGHT_BLOCK_READ | RIGHT_TRANSFER,
            },
        ),
    };

    let (dango_sysinfo, sysinfo_output) = ipc::channel();
    let (dango_echo, echo_output) = ipc::channel();
    let (console_output, dango_output) = ipc::channel();

    let caps = vec![
        executable(console),
        endpoint(console_output, RIGHT_RECV),
        executable(dango),
        endpoint(dango_sysinfo, RIGHT_RECV),
        endpoint(dango_echo, RIGHT_RECV),
        endpoint(dango_output, RIGHT_SEND),
        executable(sysinfo),
        endpoint(sysinfo_output, RIGHT_SEND),
        executable(echo),
        endpoint(echo_output, RIGHT_SEND),
        executable(storage_component),
        storage_capability,
        executable(generation_manager),
        Capability {
            object: KernelObject::GenerationControl,
            rights: RIGHT_HEALTH_CONFIRM | RIGHT_TRANSFER,
        },
    ];

    task::spawn_with_caps(init, caps).expect("failed to launch init")
}

fn executable(bytes: &'static [u8]) -> Capability {
    Capability {
        object: KernelObject::Executable(bytes),
        rights: RIGHT_EXEC,
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

fn require_grant<'a>(
    generation: &Generation<'a>,
    name: &str,
    source: &str,
    target: &str,
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
        (source_name, target_name),
        (source, target),
        "grant endpoints changed"
    );
    grant
}

fn storage_probe_required() -> bool {
    crate::pci::enumerate().is_ok_and(|functions| {
        functions.iter().any(|function| {
            function.vendor_id == 0x1af4 && matches!(function.device_id, 0x1001 | 0x1042)
        })
    })
}

pub fn record_spawn(component: &'static str, id: task::TaskId) {
    let slot = match component {
        "console" => &CONSOLE_ID,
        "dango" => &DANGO_ID,
        "sysinfo" => &SYSINFO_ID,
        "echo-agent" => &ECHO_ID,
        "storage-probe" => &STORAGE_PROBE_ID,
        "storage-writer" => &STORAGE_WRITER_ID,
        "storage-fault-probe" => &STORAGE_FAULT_ID,
        "generation-manager" => &GENERATION_MANAGER_ID,
        "storage-store-probe" => &STORAGE_STORE_ID,
        _ => return,
    };
    slot.store(id, Ordering::Relaxed);
}

extern "C" fn on_idle() {
    let checks = [
        ("init", INIT_ID.load(Ordering::Relaxed)),
        ("console", CONSOLE_ID.load(Ordering::Relaxed)),
        ("dango", DANGO_ID.load(Ordering::Relaxed)),
        ("sysinfo", SYSINFO_ID.load(Ordering::Relaxed)),
        ("echo-agent", ECHO_ID.load(Ordering::Relaxed)),
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
    ];
    let mut healthy = true;
    for (name, id) in checks {
        if id == 0 {
            continue;
        }
        let reason = task::termination_summary(id);
        serial_println!("[generation] {} terminated: {:?}", name, reason);
        let optional_storage_absent = name == "storage-probe"
            && !storage_probe_required()
            && matches!(reason, Some(task::TermReason::Exit(1)));
        healthy &= matches!(reason, Some(task::TermReason::Exit(0))) || optional_storage_absent;
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
