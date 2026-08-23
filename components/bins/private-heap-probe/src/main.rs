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
    // Which instance this is, decided *before* the self-check runs so its
    // console label can name the role. The allocator answers, not a build flag:
    // a component with no declared quota has no region, so its base is zero,
    // and one that decided from a build flag would pass against a root that had
    // stopped honouring declarations.
    //
    // Three instances share this image, and two of them are granted a region.
    // The self-check prints the same three lines for each, so without a
    // per-role label a gate matching them would match whichever instance the
    // scheduler ran first — and its reuse and batching assertions would be
    // about an instance it did not choose. The label is what keeps each
    // instance's evidence attributable to it (B63).
    if slime_rt::private_heap_stats().base == 0 {
        self_check(b"[private-heap-probe:denied]");
        denied()
    }
    // C10.4: the instance the generation also gave a shared-buffer factory.
    //
    // Asked by *kind* first, which is the post-B70 convention and the phrasing
    // that matches the question — "whichever factory this instance holds, if
    // any" rather than "does slot 1 happen to hold one".
    //
    // But `resolve_binding` is itself gated on `SERVICE_CAPABILITY_TRANSFER`,
    // and generation admission grants that service only to an instance whose
    // grants make it necessary: an endpoint grant, a transferable grant, or a
    // minted binding. This holder has exactly one non-transferable
    // `sharedBufferFactory`, so it is deliberately *not* given the service, and
    // the query is refused rather than answered. Widening the fixture to earn
    // the service would mean granting authority the arm does not need purely so
    // it could ask a question — which is a worse trade than reading the slot the
    // generation bound.
    //
    // So: the kind query when the instance can make it, and the declared slot
    // otherwise. Either way the *answer is checked* — `shared_buffer_create`
    // must actually succeed — so a root that stopped installing the grant takes
    // the granted path rather than the both-planes one, which is the property
    // that matters. The slot number is a fallback, never the assertion.
    //
    // The created buffer is *carried into* the arm rather than created again
    // there: this holder's declared `bufferCount` is one, so a second create
    // would be refused by its own quota and the arm would be measuring its own
    // detection step.
    let factory = slime_rt::resolve_binding(b"kind:sharedBufferFactory+bufferCreate")
        .unwrap_or(DECLARED_FACTORY_SLOT);
    match slime_rt::shared_buffer_create(factory, 1, true) {
        Ok(buffer) => {
            self_check(b"[private-heap-probe:both]");
            both_planes(factory, buffer.slot)
        }
        Err(_) => {
            self_check(b"[private-heap-probe:granted]");
            granted()
        }
    }
}

/// Slot the private-memory plane's fixture binds the both-planes instance's
/// shared-buffer factory into.
///
/// A fallback for the `kind:` query above, not a substitute for it: this
/// instance is not granted `SERVICE_CAPABILITY_TRANSFER`, so it cannot ask.
/// Whether the slot holds a factory is still decided by using it.
const DECLARED_FACTORY_SLOT: u32 = 1;

/// The startup self-check, in the C7 shared-buffer probe's shape: it prints its
/// own outcome and reports whether the component may proceed.
///
/// A denied component runs it too — having no quota is an answer, not a failure,
/// and the check's own denied arm is what states that.
fn self_check(label: &[u8]) {
    if !slime_rt::private_heap_probe::probe_and_report(label) {
        slime_rt::debug_write(label);
        slime_rt::debug_write(b" FAIL startup self-check\n");
        slime_rt::exit(1)
    }
}

/// Where the buffer is mapped: below the private window, which
/// `child_vspace::private_window` places above the image and thread pages.
const BUFFER_BASE: u64 = 0x0000_000A_0000_0000;

/// The instance holding both a private region and a shared buffer (C10.4).
///
/// The milestone's fourth required check: the two planes are separately
/// accounted, so exhausting one must leave the other's declared ceiling intact.
/// This runs both to their limits in sequence and requires each to keep working
/// after the other has been refused.
///
/// It also asserts the one thing that would make the separation nominal: the
/// buffer must not be mappable into the private window. The window is reserved
/// address space whose frames arrive on demand, so an address inside it that the
/// allocator has not grown into is simply unmapped — nothing about the mapping
/// call itself would fail, and what landed there would be indistinguishable
/// from heap to this component.
fn both_planes(factory_slot: u32, buffer_slot: u32) -> ! {
    // The buffer plane is already at this holder's declared ceiling: it granted
    // one buffer and `main` created it. Confirm that binds before using it as
    // the other plane's control, so "the buffer quota survived private
    // exhaustion" cannot be satisfied by a quota that was never enforced.
    if slime_rt::shared_buffer_create(factory_slot, 1, true).is_ok() {
        slime_rt::debug_write(b"[private-heap-probe:both] FAIL buffer quota did not bind\n");
        slime_rt::exit(1)
    }

    // Exhaust the private plane. The declared quota is small, so this is a
    // handful of batches, and the refusal must be the structured one.
    let mut heap: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
    if heap.try_reserve(slime_rt::GROWTH_PAGES * 4096 * 64).is_ok() {
        slime_rt::debug_write(b"[private-heap-probe:both] FAIL allocated past the ceiling\n");
        slime_rt::exit(1)
    }

    // The private window is not a place a buffer may land. Asked at the base
    // itself, which is the address the allocator's own first page occupies, so a
    // root that admitted this would be handing out storage the component is
    // already using as heap.
    let window_base = slime_rt::private_heap_stats().base as u64;
    if slime_rt::shared_buffer_map(buffer_slot, window_base, 0, 4096, true) == slime_rt::ERR_SUCCESS
    {
        slime_rt::debug_write(
            b"[private-heap-probe:both] FAIL mapped a buffer into the private window\n",
        );
        slime_rt::exit(1)
    }

    // And the same buffer maps fine outside it, so the refusal above was about
    // the destination rather than about the buffer or this holder's rights.
    if slime_rt::shared_buffer_map(buffer_slot, BUFFER_BASE, 0, 4096, true) != slime_rt::ERR_SUCCESS
    {
        slime_rt::debug_write(
            b"[private-heap-probe:both] FAIL buffer would not map outside the window\n",
        );
        slime_rt::exit(1)
    }
    // SAFETY: the root mapped one writable page at exactly this address and
    // this component is single-threaded, so nothing else names the range.
    unsafe {
        (BUFFER_BASE as *mut u64).write_volatile(0x5f5f_4d45_4d5f_5f5f);
        if (BUFFER_BASE as *const u64).read_volatile() != 0x5f5f_4d45_4d5f_5f5f {
            slime_rt::debug_write(
                b"[private-heap-probe:both] FAIL buffer page did not read back\n",
            );
            slime_rt::exit(1)
        }
    }

    // Now the other direction. The buffer plane is at its declared ceiling —
    // confirmed above, and its one mapping is now installed — and the private
    // plane must still serve what its own quota allows. A shared account would
    // have been drained by whichever plane was exhausted first.
    let mut after: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
    if after.try_reserve(64).is_err() {
        slime_rt::debug_write(
            b"[private-heap-probe:both] FAIL buffer exhaustion consumed the private quota\n",
        );
        slime_rt::exit(1)
    }
    for index in 0..64u64 {
        after.push(index);
    }
    if after.iter().sum::<u64>() != (0..64u64).sum() {
        slime_rt::debug_write(b"[private-heap-probe:both] FAIL post-exhaustion data was wrong\n");
        slime_rt::exit(1)
    }
    // Captured before the buffer's lifecycle tail, so the comparison below is
    // against the private account as it stood while both planes were live.
    let stats = slime_rt::private_heap_stats();

    // Complete the buffer's lifecycle, which the C7 probe this arm is shaped
    // after does and which is where the reverse direction of the independence
    // claim lives: seal, unmap, release. Two of the four operations the
    // milestone enumerates for the private region — seal and release — are only
    // exercised on this holder, the one that has both planes, so stopping at
    // "created and mapped" would leave them unasserted here.
    if slime_rt::shared_buffer_seal(buffer_slot) != slime_rt::ERR_SUCCESS {
        slime_rt::debug_write(b"[private-heap-probe:both] FAIL buffer would not seal\n");
        slime_rt::exit(1)
    }
    if slime_rt::shared_buffer_unmap(buffer_slot, BUFFER_BASE) != slime_rt::ERR_SUCCESS {
        slime_rt::debug_write(b"[private-heap-probe:both] FAIL buffer would not unmap\n");
        slime_rt::exit(1)
    }
    if slime_rt::shared_buffer_release(buffer_slot) != slime_rt::ERR_SUCCESS {
        slime_rt::debug_write(b"[private-heap-probe:both] FAIL buffer would not release\n");
        slime_rt::exit(1)
    }
    // Releasing the buffer returned its charge to the *buffer* account and must
    // have returned nothing to the private one: the region's page count is
    // unchanged by a shared-buffer release. Checked rather than assumed, because
    // a release that credited the wrong account is exactly the shape of the
    // defect this arm exists to rule out, and it would otherwise look like
    // extra headroom rather than like an error.
    let released = slime_rt::private_heap_stats();
    if released.pages != stats.pages || released.growths != stats.growths {
        slime_rt::debug_write(
            b"[private-heap-probe:both] FAIL buffer release moved the private account\n",
        );
        slime_rt::exit(1)
    }
    // And the freed buffer allowance is usable again, which is what makes the
    // release a return rather than a discard.
    if slime_rt::shared_buffer_create(factory_slot, 1, true).is_err() {
        slime_rt::debug_write(
            b"[private-heap-probe:both] FAIL released buffer quota was not reusable\n",
        );
        slime_rt::exit(1)
    }

    slime_rt::debug_write(b"[private-heap-probe:both] both pages=");
    write_decimal(stats.pages);
    slime_rt::debug_write(b" growths=");
    write_decimal(stats.growths);
    slime_rt::debug_write(b" buffers=1 window_map_refused=1 outside_map=1 released=1 reused=1\n");
    slime_rt::exit(0)
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
        slime_rt::debug_write(b"[private-heap-probe:granted] FAIL allocated past the ceiling\n");
        slime_rt::exit(1)
    }

    // Alive, and the allocator is still usable: a refusal must leave the heap
    // exactly as it was rather than poisoning it. Reallocating after the refusal
    // is what proves that, and it must come from the free list.
    let after_refusal = slime_rt::private_heap_stats();
    let mut small: Vec<u64> = Vec::new();
    if small.try_reserve(64).is_err() {
        slime_rt::debug_write(b"[private-heap-probe:granted] FAIL refusal poisoned the heap\n");
        slime_rt::exit(1)
    }
    for index in 0..64u64 {
        small.push(index);
    }
    if small.iter().sum::<u64>() != (0..64u64).sum() {
        slime_rt::debug_write(b"[private-heap-probe:granted] FAIL post-refusal data was wrong\n");
        slime_rt::exit(1)
    }
    let end = slime_rt::private_heap_stats();
    if end.growths != after_refusal.growths {
        slime_rt::debug_write(
            b"[private-heap-probe:granted] FAIL refused request charged a page\n",
        );
        slime_rt::exit(1)
    }
    drop(small);

    // `growths` is the load-bearing number here rather than `pages`: it is how
    // the gate tells a component that reused its free list from one that kept
    // asking for more. The self-check already reported its own reuse phase;
    // this line reports the total after the refusal, which must not have moved.
    slime_rt::debug_write(b"[private-heap-probe:granted] granted pages=");
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
        slime_rt::debug_write(
            b"[private-heap-probe:denied] FAIL allocated with no declared quota\n",
        );
        slime_rt::exit(1)
    }
    let stats = slime_rt::private_heap_stats();
    if stats.pages != 0 || stats.growths != 0 {
        slime_rt::debug_write(b"[private-heap-probe:denied] FAIL denied instance holds pages\n");
        slime_rt::exit(1)
    }
    slime_rt::debug_write(b"[private-heap-probe:denied] denied pages=0 growths=0 refused=1\n");
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
