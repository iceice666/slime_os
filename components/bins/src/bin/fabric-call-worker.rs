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

#[path = "../call_broker.rs"]
mod call_broker;

slime_rt::entry!(main);

/// Endpoint factory, granted by the generation. The worker mints both halves of
/// its route through it; no participant holds one.
const FACTORY_SLOT: u32 = 0;
/// Shared-buffer factory, backing the one copy a large call payload makes.
const BUFFER_FACTORY_SLOT: u32 = 1;
/// Control endpoints, in the order the fabric granted them: the two clients,
/// the server, then the capability-routed clock. The slot a request arrives on
/// *is* the caller's identity.
const FIRST_CONTROL_SLOT: u32 = 2;

fn main(_startup_arg: u32) {
    call_broker::Broker::new(
        FACTORY_SLOT,
        BUFFER_FACTORY_SLOT,
        [FIRST_CONTROL_SLOT, FIRST_CONTROL_SLOT + 1],
        FIRST_CONTROL_SLOT + 2,
        FIRST_CONTROL_SLOT + 3,
        0,
    )
    .run();
    slime_rt::debug_write(b"[fabric-call-worker] call plane complete\n");
}
