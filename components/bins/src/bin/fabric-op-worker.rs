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

slime_rt::entry!(main);

/// Endpoint factory, granted by the generation.
const FACTORY_SLOT: u32 = 0;
/// Control endpoints, in the order the fabric granted them: the two clients,
/// the server, the clock, then client B's replacement channel.
const FIRST_CONTROL_SLOT: u32 = 1;

fn main(_startup_arg: u32) {
    operation_broker::Broker::new(
        FACTORY_SLOT,
        [FIRST_CONTROL_SLOT, FIRST_CONTROL_SLOT + 1],
        FIRST_CONTROL_SLOT + 2,
        FIRST_CONTROL_SLOT + 3,
        FIRST_CONTROL_SLOT + 4,
    )
    .run();
    slime_rt::debug_write(b"[fabric-op-worker] operation plane complete\n");
}
