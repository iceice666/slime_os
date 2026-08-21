#![no_std]
#![no_main]

//! C8.10 bounded call route worker: the `parameters` route, and nothing else.
//!
//! The generation partitions the graph into three workers over whole routes
//! because one task cannot park on the whole fabric: the declared peaks are
//! stream 8, call 7, and operation 9 wake sources against a `MAX_WAIT_SOURCES`
//! of 9. A single-task fabric would have to poll, which the milestone forbids.
//! The split is therefore forced by the kernel bound, not chosen for tidiness.
//!
//! This worker is its own task with its own capability table, so its control
//! slots are numbered from the same base as every other worker's without
//! colliding: slot 2 here and slot 2 in the operation worker name different
//! objects in different tables. Only the aggregate init hands out has to be
//! disjoint, which is what the resolved profile's `requiredCapabilitySlots`
//! sums.
//!
//! It runs the same `call_broker` the C8.6 gate runs, against the same declared
//! route. What changed is who hosts it: a dedicated task whose park set is one
//! worker's worth of sources, rather than one service multiplexing every plane.

#[path = "../../../lib/src/call_broker.rs"]
mod call_broker;

// The trace emitter, included here rather than by the broker: a file may be a
// module only once per crate, and `fabric-service` includes both brokers. Each
// binary that hosts a broker therefore owns the include, and the broker reaches
// it through `super`.
#[path = "../../../lib/src/fabric_trace_log.rs"]
mod trace_log;

slime_rt::entry!(main);

/// Shared-buffer factory, backing the one copy a large call payload makes.
const BUFFER_FACTORY_SLOT: u32 = 1;
const CLIENT_A_SLOT: u32 = 2;
const CLIENT_B_SLOT: u32 = 3;
const SERVER_SLOT: u32 = 4;
const TIME_SLOT: u32 = 5;
const CLIENT_A_SUPERVISION_SLOT: u32 = 6;
const CLIENT_B_SUPERVISION_SLOT: u32 = 7;
const SERVER_SUPERVISION_SLOT: u32 = 8;
/// The clock's own supervision handle. Its exit is observable no other way: a
/// native Endpoint reports no peer death, and this plane's clock is a separately
/// declared instance, so the server's handle says nothing about it (B76).
const TIME_SUPERVISION_SLOT: u32 = 9;

fn main(_startup_arg: u32) {
    call_broker::Broker::new(
        BUFFER_FACTORY_SLOT,
        [CLIENT_A_SLOT, CLIENT_B_SLOT],
        SERVER_SLOT,
        TIME_SLOT,
        [
            CLIENT_A_SUPERVISION_SLOT,
            CLIENT_B_SUPERVISION_SLOT,
            SERVER_SUPERVISION_SLOT,
            TIME_SUPERVISION_SLOT,
        ],
    )
    .run();
    slime_rt::debug_write(b"[fabric-call-worker] call plane complete\n");
}
