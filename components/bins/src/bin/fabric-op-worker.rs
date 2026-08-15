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

#[path = "../operation_broker.rs"]
mod operation_broker;

// The trace emitter, included here rather than by the broker: a file may be a
// module only once per crate, and `fabric-service` includes both brokers. Each
// binary that hosts a broker therefore owns the include, and the broker reaches
// it through `super`.
#[path = "../fabric_trace_log.rs"]
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

fn main(_startup_arg: u32) {
    operation_broker::Broker::new(
        [CLIENT_A_SLOT, CLIENT_B_SLOT],
        SERVER_SLOT,
        TIME_SLOT,
        REPLACEMENT_SLOT,
        None,
        BACKUP_ROUTE_SLOT,
        [
            CLIENT_A_SUPERVISION_SLOT,
            CLIENT_B_SUPERVISION_SLOT,
            SERVER_SUPERVISION_SLOT,
        ],
        REPLACEMENT_SUPERVISION_SLOT,
    )
    .run();
    slime_rt::debug_write(b"[fabric-op-worker] operation plane complete\n");
}
