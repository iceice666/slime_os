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
/// the point is only to let the other thread be scheduled. The worker runs
/// below this thread, so these yields are what let it start at all — and once
/// it is spinning, this thread's own progress is what proves the priority
/// reached the TCB.
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

    // Burn CPU without yielding, at this thread's declared priority (B48).
    //
    // The fixture declares `workerPriority` below the main thread's, so a
    // priority-respecting scheduler must preempt this loop whenever the main
    // thread becomes runnable. The plane asserts the main thread's completion
    // marker appears *while* this loop is still running, which cannot happen
    // if both threads share one priority: on a single core the round-robin
    // would let this run to its bound first.
    //
    // No `yield_now` anywhere in here. A yield would hand over voluntarily and
    // prove nothing about preemption.
    let mut sink = 0u64;
    for step in 0..STARVATION_SPINS {
        // Opaque enough that the optimizer cannot fold the loop away; a loop
        // compiled to nothing would "pass" while testing nothing.
        sink = sink.wrapping_add(step).rotate_left(1);
        core::hint::spin_loop();
    }
    if sink == u64::MAX {
        slime_rt::debug_write(b"[sample-worker] spin sink saturated\n");
    }
    slime_rt::debug_write(b"[sample-worker] worker thread done\n");
}

/// Iterations the low-priority worker spins for.
///
/// Long enough that a scheduler ignoring the declared priority would finish it
/// before the main thread's markers, short enough that the plane terminates.
const STARVATION_SPINS: u64 = 200_000_000;
