#![no_std]
#![no_main]

//! Bounded `parameters` call-route worker shared by the boot, traffic, and
//! robot planes.
//!
//! The binary names roles, never CSpace positions. Each selected composition's
//! authenticated grants resolve to the slots root installed for this instance,
//! so adding or reordering unrelated authority cannot silently retarget a role.

#[path = "../../../lib/src/call_broker.rs"]
mod call_broker;

// The trace emitter, included here rather than by the broker: a file may be a
// module only once per crate, and `fabric-service` includes both brokers. Each
// binary that hosts a broker therefore owns the include, and the broker reaches
// it through `super`.
#[path = "../../../lib/src/fabric_trace_log.rs"]
mod trace_log;

use boot_contracts::generation::BootAction;

slime_rt::entry!(main);

const TRAFFIC_CLIENTS: [Option<&[u8]>; 2] =
    [Some(b"fabric-call-client"), Some(b"fabric-call-client-b")];
const ROBOT_CLIENTS: [Option<&[u8]>; 2] = [Some(b"robot-controller"), None];

struct Composition {
    clients: [Option<&'static [u8]>; 2],
    client_control_grants: [Option<&'static [u8]>; 2],
    client_supervision: [Option<&'static [u8]>; 2],
    /// Notification grant naming a client's fallback retirement signal, for a
    /// client [`client_supervision`] leaves `None`. `None` here means that
    /// client either carries supervision or has none at all.
    client_retirement: [Option<&'static [u8]>; 2],
    server: &'static [u8],
    server_control_grant: &'static [u8],
    clock: &'static [u8],
    clock_control_grant: &'static [u8],
    wake_name: &'static [u8],
}

const TRAFFIC: Composition = Composition {
    clients: TRAFFIC_CLIENTS,
    client_control_grants: [
        Some(b"fabric-call-client-control"),
        Some(b"fabric-call-client-b-control"),
    ],
    client_supervision: TRAFFIC_CLIENTS,
    client_retirement: [None, None],
    server: b"fabric-call-server",
    server_control_grant: b"fabric-call-server-control",
    clock: b"fabric-call-time",
    clock_control_grant: b"fabric-call-time-control",
    wake_name: b"notification:fabric-service-parameters-ready",
};

const ROBOT: Composition = Composition {
    clients: ROBOT_CLIENTS,
    // The controller also holds a stream control edge. Its call edge therefore
    // carries the explicit `-call-control` qualifier rather than competing for
    // the generic component control name; the broker still resolves the grant
    // itself, not a position copied from the fixture.
    client_control_grants: [Some(b"robot-controller-call-control"), None],
    // Only the controller's owner can mint its supervision binding, and that
    // owner does not spawn this broker. Demanding the unproducible handle was
    // rejected: root reinstalls the generation-owned call endpoint into each
    // replacement controller, so the broker's endpoint remains valid without
    // observing the task identity.
    client_supervision: [None, None],
    // The controller's owner *can* observe its own supervision loop
    // concluding, though, and signals this notification once it has: no
    // further restart will ever be admitted, so the generation-owned endpoint
    // this broker holds will never receive from a replacement again. Without
    // this the client slot would remain `Some` forever — the endpoint really
    // does survive every replacement — and `run`'s exit predicate would never
    // see every client absent.
    client_retirement: [Some(b"notification:robot-controller-retired"), None],
    server: b"robot-actuator",
    server_control_grant: b"robot-actuator-control",
    clock: b"robot-clock",
    clock_control_grant: b"robot-clock-control",
    wake_name: b"notification:fabric-call-worker-parameters-ready",
};

fn main(_startup_arg: u32) {
    // `Boot` and `Traffic` share one composition: both fixtures declare the
    // identical `fabric-call-{client,client-b,server,time}-control` grants and
    // `minted:*-supervision` bindings this worker predates C9.6 by resolving,
    // so C8.10's boot plane is the same shape as the dedicated traffic plane
    // rather than a third one.
    let composition = if slime_components::generation_composition::is(BootAction::Boot)
        || slime_components::generation_composition::is(BootAction::Traffic)
    {
        &TRAFFIC
    } else if slime_components::generation_composition::is(BootAction::RobotRuntime) {
        &ROBOT
    } else {
        fail(b"unsupported boot action");
    };

    // The ceilings this worker admits traffic against come from the graph the
    // root authenticated, not from a per-plane table rendered into `OUT_DIR`
    // (B70/CP2). An unanswerable query is a composition this binary cannot serve.
    let limits = boot_contracts::fabric_graph::RuntimeLimits::load(slime_rt::graph_query)
        .unwrap_or_else(|_| fail(b"runtime limits"));

    let clients = [
        resolve_optional_control(composition.clients[0], composition.client_control_grants[0]),
        resolve_optional_control(composition.clients[1], composition.client_control_grants[1]),
    ];
    let server = resolve_required(composition.server_control_grant);
    let clock = resolve_required(composition.clock_control_grant);
    let supervision = [
        resolve_optional_supervision(composition.client_supervision[0]),
        resolve_optional_supervision(composition.client_supervision[1]),
        Some(resolve_supervision(composition.server)),
        Some(resolve_supervision(composition.clock)),
    ];
    let retirement = [
        resolve_optional_retirement(composition.client_retirement[0]),
        resolve_optional_retirement(composition.client_retirement[1]),
    ];

    call_broker::Broker::new(
        buffer_factory_slot(),
        clients,
        composition.clients,
        server,
        composition.server,
        clock,
        supervision,
        retirement,
        composition.wake_name,
        limits,
    )
    .run();
    slime_rt::debug_write(b"[fabric-call-worker] call plane complete\n");
}

/// Resolve a declared endpoint grant. Failure names the missing edge before
/// exiting: a required role without its control edge is a composition defect,
/// not a reason to fall back to the old slot layout.
fn resolve_required(grant: &'static [u8]) -> u32 {
    slime_rt::resolve_binding(grant).unwrap_or_else(|_| {
        slime_rt::debug_write(b"[fabric-call-worker] missing binding: ");
        slime_rt::debug_write(grant);
        slime_rt::debug_write(b"\n");
        slime_rt::exit(1)
    })
}

fn resolve_optional_control(
    component: Option<&'static [u8]>,
    grant: Option<&'static [u8]>,
) -> Option<u32> {
    match (component, grant) {
        (None, None) => None,
        (Some(_), Some(grant)) => Some(resolve_required(grant)),
        _ => fail(b"incomplete optional client role"),
    }
}

/// Resolve the factory by kind first, matching `fabric-service`: ordinary
/// grants are discoverable on that axis. Minted bindings live in a separate
/// table, so the worker-specific conventional name is the required fallback.
fn buffer_factory_slot() -> u32 {
    slime_rt::resolve_binding(b"kind:sharedBufferFactory+bufferCreate")
        .or_else(|_| slime_rt::resolve_binding(b"minted:fabric-call-worker-shared-buffer-factory"))
        .unwrap_or_else(|_| fail(b"shared-buffer factory grant"))
}

fn resolve_optional_supervision(component: Option<&'static [u8]>) -> Option<u32> {
    component.map(resolve_supervision)
}

/// Ask root for `minted:<component>-supervision`, the name fixed by generation
/// construction. Formatting the name here rather than accepting a slot keeps
/// the client role and its death authority tied to one identity (B76).
fn resolve_supervision(component: &'static [u8]) -> u32 {
    const PREFIX: &[u8] = b"minted:";
    const SUFFIX: &[u8] = b"-supervision";
    let mut name = [0u8; 64];
    let end = PREFIX.len() + component.len() + SUFFIX.len();
    if end > name.len() {
        fail(b"supervision name exceeds bound");
    }
    name[..PREFIX.len()].copy_from_slice(PREFIX);
    name[PREFIX.len()..PREFIX.len() + component.len()].copy_from_slice(component);
    name[PREFIX.len() + component.len()..end].copy_from_slice(SUFFIX);
    slime_rt::resolve_binding(&name[..end]).unwrap_or_else(|_| {
        slime_rt::debug_write(b"[fabric-call-worker] missing binding: ");
        slime_rt::debug_write(&name[..end]);
        slime_rt::debug_write(b"\n");
        slime_rt::exit(1)
    })
}

fn resolve_optional_retirement(grant: Option<&'static [u8]>) -> Option<u32> {
    grant.map(|grant| {
        slime_rt::resolve_binding(grant).unwrap_or_else(|_| {
            slime_rt::debug_write(b"[fabric-call-worker] missing binding: ");
            slime_rt::debug_write(grant);
            slime_rt::debug_write(b"\n");
            slime_rt::exit(1)
        })
    })
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric-call-worker] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}
