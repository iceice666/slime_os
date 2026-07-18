use alloc::vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::capability::{Capability, KernelObject, RIGHT_EXEC, RIGHT_RECV, RIGHT_SEND};
use crate::generation::{self, Generation};
use crate::{ipc, serial_println, task};

static INIT_ID: AtomicU64 = AtomicU64::new(0);
static CONSOLE_ID: AtomicU64 = AtomicU64::new(0);
static DANGO_ID: AtomicU64 = AtomicU64::new(0);
static SYSINFO_ID: AtomicU64 = AtomicU64::new(0);
static ECHO_ID: AtomicU64 = AtomicU64::new(0);

pub fn start() -> ! {
    let bytes = crate::limine::generation_module();
    let generation = generation::decode(bytes).expect("invalid generation manifest");
    serial_println!(
        "[generation] decoded generation {}: {} objects, {} components, {} grants",
        generation.number,
        generation.objects.len(),
        generation.components.len(),
        generation.grants.len(),
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

    require_grant(generation, "console-output", "console", "dango");
    require_grant(generation, "system-information", "init", "sysinfo");
    require_grant(generation, "echo-request", "echo-agent", "dango");
    require_grant(generation, "echo-reply", "dango", "echo-agent");

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
    ];

    task::spawn_with_caps(init, caps).expect("failed to launch init")
}

fn executable(bytes: &'static [u8]) -> Capability {
    Capability {
        object: KernelObject::Executable(bytes),
        rights: RIGHT_EXEC,
    }
}

fn endpoint(endpoint: ipc::Endpoint, rights: u32) -> Capability {
    Capability {
        object: KernelObject::Endpoint(endpoint),
        rights,
    }
}

fn require_grant(generation: &Generation<'_>, name: &str, source: &str, target: &str) {
    let grant = generation.grant(name).expect("required grant missing");
    let source_name = generation.components[grant.source].name;
    let target_name = generation.components[grant.target].name;
    assert_eq!(
        (source_name, target_name),
        (source, target),
        "grant endpoints changed"
    );
}

pub fn record_spawn(component: &'static str, id: task::TaskId) {
    let slot = match component {
        "console" => &CONSOLE_ID,
        "dango" => &DANGO_ID,
        "sysinfo" => &SYSINFO_ID,
        "echo-agent" => &ECHO_ID,
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
    ];
    let mut healthy = true;
    for (name, id) in checks {
        let reason = task::termination_summary(id);
        serial_println!("[generation] {} terminated: {:?}", name, reason);
        healthy &= matches!(reason, Some(task::TermReason::Exit(0)));
    }
    if healthy {
        serial_println!("[generation] vertical slice healthy");
        crate::exit_qemu(crate::QemuExitCode::Success);
    } else {
        crate::exit_qemu(crate::QemuExitCode::Failed);
    }
    crate::hlt_loop()
}
