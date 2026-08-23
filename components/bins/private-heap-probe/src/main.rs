#![no_std]
#![no_main]

//! C10.3's subject: a real component allocating through ordinary Rust
//! collections inside its generation-declared private-memory ceiling.
//!
//! C10.2 proved the declared quota is the live ceiling by growing raw pages one
//! at a time. That is the mechanism, not the thing a component wants: nothing
//! could allocate a `Vec`. This image is the same two-outcome plane one milestone
//! on — the granted instance runs `Vec`, `Box`, and `String` across
//! reallocations that cross a growth batch, frees them, and takes the memory
//! again without the root serving another page; the instance the budget omits
//! finds no region at all and says so.
//!
//! Which instance this image is running as is *not* compiled in, for the same
//! reason as in C10.2: it asks its own allocator what region it has. A component
//! that decided from a build flag would pass against a root that had stopped
//! honouring declarations.
//!
//! Three assertions this image makes that the root cannot make for it:
//!
//! * **the ceiling is reachable by ordinary Rust.** The startup self-check
//!   allocates until it has crossed a growth batch, checks every element
//!   survived the reallocations, and reports the pages it took.
//! * **exhaustion is structural.** After the self-check, the granted instance
//!   deliberately allocates past its declared ceiling with `try_reserve` and must
//!   observe an `Err` while staying alive to report it. A fault, a hang, or a
//!   silent truncation all fail the plane instead.
//! * **freed memory comes back.** The self-check's reuse phase reallocates what
//!   it just freed and requires the root's growth count not to move, which is
//!   the only evidence that a component bound by a small declared quota can run
//!   longer than its first burst of allocations.

extern crate alloc;

use alloc::vec::Vec;

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    // The startup self-check, in the C7 shared-buffer probe's shape: it prints
    // its own outcome and returns whether the component may proceed. A denied
    // component proceeds too — having no quota is an answer, not a failure.
    if !slime_rt::private_heap_probe::probe_and_report(b"[private-heap-probe]") {
        slime_rt::debug_write(b"[private-heap-probe] FAIL startup self-check\n");
        slime_rt::exit(1)
    }

    // Which instance this is: the allocator answers, not a build flag. A
    // component with no declared quota has no region, so its base is zero.
    if slime_rt::private_heap_stats().base == 0 {
        denied()
    }
    granted()
}

/// The granted instance: prove the ceiling binds structurally.
///
/// The self-check has already shown the quota is usable. What is left is the
/// milestone's third required check — that running *out* of it is an error the
/// component observes. So: allocate deliberately past the declared ceiling and
/// require a refusal, then keep running and report, which a faulted or hung
/// component could not do.
fn granted() -> ! {
    // Larger than any quota this plane declares, so the request cannot be
    // served whatever the batching policy is, and bounded so a component with a
    // wrongly *large* installed ceiling still terminates rather than allocating
    // until the machine notices.
    const BEYOND_CEILING: usize = slime_rt::GROWTH_PAGES * 4096 * 64;

    let mut past: Vec<u8> = Vec::new();
    if past.try_reserve(BEYOND_CEILING).is_ok() {
        slime_rt::debug_write(b"[private-heap-probe] FAIL allocated past the ceiling\n");
        slime_rt::exit(1)
    }

    // Alive, and the allocator is still usable: a refusal must leave the heap
    // exactly as it was rather than poisoning it. Reallocating after the refusal
    // is what proves that, and it must come from the free list.
    let after_refusal = slime_rt::private_heap_stats();
    let mut small: Vec<u64> = Vec::new();
    if small.try_reserve(64).is_err() {
        slime_rt::debug_write(b"[private-heap-probe] FAIL refusal poisoned the heap\n");
        slime_rt::exit(1)
    }
    for index in 0..64u64 {
        small.push(index);
    }
    if small.iter().sum::<u64>() != (0..64u64).sum() {
        slime_rt::debug_write(b"[private-heap-probe] FAIL post-refusal data was wrong\n");
        slime_rt::exit(1)
    }
    let end = slime_rt::private_heap_stats();
    if end.growths != after_refusal.growths {
        slime_rt::debug_write(b"[private-heap-probe] FAIL refused request charged a page\n");
        slime_rt::exit(1)
    }
    drop(small);

    // `growths` is the load-bearing number here rather than `pages`: it is how
    // the gate tells a component that reused its free list from one that kept
    // asking for more. The self-check already reported its own reuse phase;
    // this line reports the total after the refusal, which must not have moved.
    slime_rt::debug_write(b"[private-heap-probe] granted pages=");
    write_decimal(end.pages);
    slime_rt::debug_write(b" growths=");
    write_decimal(end.growths);
    slime_rt::debug_write(b" refused=1 reused=1\n");
    slime_rt::exit(0)
}

/// The instance the budget omits: no region, so no allocation at all.
///
/// Stronger than "gets less": the allocator must report no base, and an
/// allocation must fail rather than fall back to some other memory. A component
/// with no declared quota that could still allocate would mean the ceiling is
/// advisory.
fn denied() -> ! {
    let mut any: Vec<u64> = Vec::new();
    if any.try_reserve(1).is_ok() {
        slime_rt::debug_write(b"[private-heap-probe] FAIL allocated with no declared quota\n");
        slime_rt::exit(1)
    }
    let stats = slime_rt::private_heap_stats();
    if stats.pages != 0 || stats.growths != 0 {
        slime_rt::debug_write(b"[private-heap-probe] FAIL denied instance holds pages\n");
        slime_rt::exit(1)
    }
    slime_rt::debug_write(b"[private-heap-probe] denied pages=0 growths=0 refused=1\n");
    slime_rt::exit(0)
}

fn write_decimal(value: usize) {
    let mut digits = [0u8; 20];
    let mut index = digits.len();
    let mut remaining = value;
    loop {
        index -= 1;
        digits[index] = b'0' + (remaining % 10) as u8;
        remaining /= 10;
        if remaining == 0 {
            break;
        }
    }
    slime_rt::debug_write(&digits[index..]);
}
