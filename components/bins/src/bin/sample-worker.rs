//! Proves a component runs two threads (B47).
//!
//! Two threads of one process: they share a CSpace and a VSpace — the worker
//! writes to the console through the same slot the main thread holds, which is
//! only possible because the capability is the *process's* — while each owns
//! its own TCB, stack, IPC buffer, transfer window, and schedule.
//!
//! What the plane checks is that both markers appear. The worker's marker is
//! the load-bearing one: it can only be printed by a thread that reached its
//! own entry point, on its own stack, and made a syscall through its own IPC
//! buffer. A second TCB the root configured but never started, or one whose
//! buffer overlapped the main thread's, produces no such line.

#![no_std]
#![no_main]

slime_rt::entry!(main, worker = worker);

/// How long the main thread waits for the worker before reporting.
///
/// A count rather than a timer: this component holds no timer capability, and
/// the point is only to let the other thread be scheduled. Both threads run at
/// the same priority on one core, so the yield is what actually hands over.
const HANDOVER_SPINS: u32 = 64;

fn main(_startup_arg: u32) {
    slime_rt::debug_write(b"[sample-worker] main thread running\n");

    // Let the worker run. Without this the main thread could reach its exit
    // before the worker was ever scheduled, and the plane would be asserting
    // on timing rather than on whether two threads exist.
    for _ in 0..HANDOVER_SPINS {
        slime_rt::yield_now();
    }

    slime_rt::debug_write(b"[sample-worker] main thread done\n");
}

/// The second thread's body.
///
/// Every syscall here goes through this thread's own IPC buffer, at an address
/// derived from the thread index the root left in `TPIDR_EL0`. If that index
/// were wrong, or the buffer shared with the main thread, this would corrupt
/// the other thread's in-flight message rather than print.
fn worker(_startup_arg: u32) {
    slime_rt::debug_write(b"[sample-worker] worker thread running\n");
}
