#![no_std]
#![no_main]

//! C9.6's declared best-effort load: the CPU contention the robot graph runs
//! under.
//!
//! One instance, declared `bestEffort` by
//! `contracts/generation-manifest/v1/compositions/sel4-robot-runtime.zti`'s
//! `schedulingClass`. It exists so the milestone's first required check — "the
//! graph runs to completion under contention with declared scheduling order
//! preserved" — is observed against a real runnable competitor rather than
//! against an idle vCPU.
//!
//! It never yields. A yield would hand the CPU over voluntarily, so a
//! priority-ignoring scheduler would still produce the transcript the gate
//! reads; only preemption of a demonstrably-running loop distinguishes the two.
//! The chunk markers are what make the loop demonstrably running: they bracket
//! the higher bands' progress, so the gate can assert that the sensor and the
//! controller made ordered progress *between* two chunks of this loop rather
//! than merely before or after all of it.
//!
//! Its own band is read back and reported rather than assumed. A burner that
//! had silently been placed in a higher band would still spin, and the
//! contention claim would be a claim about a composition that no longer exists.

use boot_contracts::scheduling_class::CLASS_BEST_EFFORT;
use slime_rt::{debug_write, exit, scheduling_class_read};

/// Iterations spun per chunk.
///
/// Small relative to C9.3's own burner, deliberately: that plane runs alone
/// against one foreground component, so a chunk only has to fit somewhere in
/// its whole run. Here the burner shares the vCPU with six other `normal`-band
/// participants reacting to every sensor tick, so the only gaps long enough to
/// land a whole chunk are the brief windows between one tick's reaction chain
/// settling and the next tick arriving. A chunk sized to fit inside one such
/// gap is what makes the bracketing observable; C9.3's 20M-iteration chunk
/// never completed even once in this composition's own tick window.
const BURN_CHUNK_ITERATIONS: u64 = 500_000;

/// Chunks run. The product is the same 200M-iteration bound C9.3's own burner
/// uses, so the total contention this plane runs under is unchanged from that
/// plane's — only the reporting granularity differs.
const BURN_CHUNKS: u32 = 400;

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    let class = match scheduling_class_read() {
        Ok(class) => class,
        Err(error) => fail_with(b"class read", error),
    };
    // The declared band is the contention claim. Asserted rather than printed
    // and trusted: a burner promoted into a higher band would still emit every
    // marker below while no longer being the load this plane says it is.
    if class.class_id != CLASS_BEST_EFFORT {
        fail(b"the declared load is not bestEffort")
    }
    write_value(b"[robot-burner] bestEffort priority=", class.priority);

    let mut sink = 0u64;
    for chunk in 0..BURN_CHUNKS {
        // No `yield_now` in here, deliberately: see the module comment.
        for step in 0..BURN_CHUNK_ITERATIONS {
            // Opaque enough that the optimizer cannot fold the loop away. A
            // loop compiled to nothing would pass every marker while applying
            // no load at all.
            sink = sink.wrapping_add(step).rotate_left(1);
            core::hint::spin_loop();
        }
        write_value(b"[robot-burner] chunk=", chunk as u64);
    }
    if sink == u64::MAX {
        debug_write(b"[robot-burner] spin sink saturated\n");
    }
    debug_write(b"[robot-burner] bestEffort complete\n");
    exit(0)
}

fn write_value(prefix: &[u8], value: u64) {
    let mut digits = [0u8; 20];
    debug_write(prefix);
    debug_write(decimal(value, &mut digits));
    debug_write(b"\n");
}

fn decimal(value: u64, digits: &mut [u8; 20]) -> &[u8] {
    let mut index = digits.len();
    let mut remaining = value;
    if remaining == 0 {
        index -= 1;
        digits[index] = b'0';
    }
    while remaining != 0 {
        index -= 1;
        digits[index] = b'0' + (remaining % 10) as u8;
        remaining /= 10;
    }
    &digits[index..]
}

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[robot-burner] FAIL ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}

fn fail_with(reason: &[u8], error: i64) -> ! {
    debug_write(b"[robot-burner] FAIL ");
    debug_write(reason);
    write_value(b" error=", error.unsigned_abs());
    exit(1)
}
