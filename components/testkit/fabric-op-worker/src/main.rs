#![no_std]
#![no_main]

//! C8.10 bounded operation route worker: the `navigation` and `nav-backup`
//! routes, and nothing else.
//!
//! The widest of the three declared workers at 9 of `MAX_WAIT_SOURCES = 9`
//! wake sources — two client slots contributing a control endpoint, a send
//! capacity, and a supervision handle each, plus the server endpoint, the clock,
//! and the server's supervision handle. It sits at the kernel bound with zero
//! headroom, which is why the generation declares that peak and checks it at
//! build time rather than discovering it when a boot overflows.
//!
//! Its own task and its own capability table, like [`fabric-call-worker`]: the
//! two number their control slots from the same base without colliding, because
//! a slot number only names an object within one table.

#[path = "../../../lib/src/operation_broker.rs"]
mod operation_broker;

// The trace emitter, included here rather than by the broker: a file may be a
// module only once per crate, and `fabric-service` includes both brokers. Each
// binary that hosts a broker therefore owns the include, and the broker reaches
// it through `super`.
#[path = "../../../lib/src/fabric_trace_log.rs"]
mod trace_log;

slime_rt::entry!(main);

const CLIENT_A_SLOT: u32 = 2;
const CLIENT_B_SLOT: u32 = 3;
const SERVER_SLOT: u32 = 4;
const TIME_SLOT: u32 = 5;
const REPLACEMENT_SLOT: u32 = 6;
const BACKUP_ROUTE_SLOT: u32 = 7;
const CLIENT_A_SUPERVISION_SLOT: u32 = 8;
const CLIENT_B_SUPERVISION_SLOT: u32 = 9;
const SERVER_SUPERVISION_SLOT: u32 = 10;
const REPLACEMENT_SUPERVISION_SLOT: u32 = 11;
/// C8.13: the concurrent traffic plane drives the real restart scenario, so
/// this worker needs the release-barrier endpoint the standalone C8.7 plane
/// declares. The boot-parked plane never reached `pump_replacement`'s
/// admission arm (client B parks rather than exiting), so it never needed one.
const RESTART_START_SLOT: u32 = 12;

fn main(_startup_arg: u32) {
    // The ceilings this worker admits traffic against come from the graph the
    // root authenticated, not from a per-plane table a build script rendered
    // into `OUT_DIR` (B70/CP2). An unanswerable query is a composition this
    // binary cannot serve, so it exits rather than assuming a default.
    let limits = boot_contracts::fabric_graph::RuntimeLimits::load(slime_rt::graph_query)
        .unwrap_or_else(|_| slime_rt::exit(1));
    operation_broker::Broker::new(
        operation_broker::Wiring {
            clients: [CLIENT_A_SLOT, CLIENT_B_SLOT],
            server: SERVER_SLOT,
            time_control: TIME_SLOT,
            replacement_control: REPLACEMENT_SLOT,
            replacement_start: Some(RESTART_START_SLOT),
            backup_route: BACKUP_ROUTE_SLOT,
            supervision: [
                CLIENT_A_SUPERVISION_SLOT,
                CLIENT_B_SUPERVISION_SLOT,
                SERVER_SUPERVISION_SLOT,
            ],
            replacement_supervision: REPLACEMENT_SUPERVISION_SLOT,
        },
        limits,
    )
    .run();
    slime_rt::debug_write(b"[fabric-op-worker] operation plane complete\n");
}
