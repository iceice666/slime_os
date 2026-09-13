#![no_std]
#![no_main]

//! MEM-64M's reuse subject: one declared holder, relaunched at the 64 MiB
//! ceiling until the root has reclaimed and re-served that quota twenty times.
//!
//! The rest of MEM-64M is about whether a declared quota is reachable *once*:
//! `private-memory-probe` grows to its ceiling, proves the refusal past it, and
//! exits. That leaves the milestone's reuse clause untested, because a root that
//! leaked every page, slot, or descriptor a dead holder owned would still pass
//! a single-shot plane — the leak becomes observable only when a later task has
//! to be served from what an earlier one returned.
//!
//! So this probe is deliberately not a longer-running variant of that one. Each
//! incarnation is a whole life: it grows to exactly its declared ceiling, reads
//! every page before writing it, stamps a value no other incarnation writes,
//! reads all of them back, and then ends — half of them by an orderly exit and
//! half by a deliberate VM fault. Both are reclamation paths the root
//! implements separately (`reclaim_dead_task` then `reclaim_task_objects` on
//! each), and the milestone requires immediate reuse after either, so a plane
//! that exercised only one would leave half the clause unobserved.
//!
//! The cycle number arrives over a declared endpoint rather than being derived
//! locally: an incarnation cannot count its own predecessors, and a pattern
//! that did not vary between lives could not tell "the root zeroed this page"
//! from "the root re-served a page this incarnation had already written".

use slime_rt::PrivateMemory;

slime_rt::entry!(main);

/// Pages this incarnation grows to. The composition declares the same ceiling;
/// the root adjudicates against its own copy, so a probe that asked for more
/// would be refused rather than widening anything.
const CYCLE_PAGES: usize = 64 * 1024 * 1024 / 4096;
const _: () = assert!(
    CYCLE_PAGES <= boot_contracts::private_memory_budget::capacity_for("aarch64-sel4-qemu-virt").0
);

/// Words per 4 KiB page, so the touch loop steps one `u64` into each page.
const WORDS_PER_PAGE: usize = 4096 / 8;

/// The endpoint init sends this incarnation's cycle number over. Pinned by the
/// composition at logical slot 0 for this holder, the same way every other
/// probe's run token is: the private region takes the low addresses, not the
/// low capability numbers, so there is no collision to resolve here.
const CYCLE_TOKEN_SLOT: u32 = 0;

/// Yields spent waiting for init's cycle token before giving up. Init sends it
/// immediately after the spawn returns, so arrival is a scheduling question
/// rather than a protocol one; the bound exists so a plane that stopped sending
/// fails fast instead of hanging until the gate's timeout.
const TOKEN_YIELDS: usize = 4096;

fn main(_startup_arg: u32) {
    let cycle = match cycle_token() {
        Some(cycle) => cycle,
        None => fail(b"cycle token never arrived", 0),
    };

    // Asking costs nothing and needs no quota, so this must answer for every
    // incarnation. `pages == 0` is the load-bearing half: a fresh task must be
    // served a region with nothing backed, not one still carrying a dead
    // task's committed pages.
    let region = match slime_rt::private_memory_grow(0) {
        Ok(region) => region,
        Err(error) => fail(b"size query refused", error),
    };
    if region.pages != 0 {
        report_fail(cycle, b"region arrived already backed", region.pages);
    }
    if region.base == 0 {
        report_fail(cycle, b"declared holder received no region", 0);
    }

    let grown = match slime_rt::private_memory_grow(CYCLE_PAGES) {
        Ok(previous) => previous,
        Err(error) => fail(b"declared growth refused", error),
    };
    if grown.pages != 0 || grown.base != region.base {
        report_fail(cycle, b"growth disagreed with the query", grown.pages);
    }

    touch(cycle, &region);

    slime_rt::debug_write(b"[private-cycle-probe] cycle=");
    write_decimal(cycle as usize);
    slime_rt::debug_write(b" pages=");
    write_decimal(CYCLE_PAGES);
    slime_rt::debug_write(b" base=");
    write_hex(region.base);
    slime_rt::debug_write(b" stamp=");
    write_hex(stamp(cycle) as usize);
    slime_rt::debug_write(b" zeroed=1 verified=1 end=");

    // Odd cycles die by fault, even ones by exit. Alternating rather than
    // running ten of each in sequence: a reclamation defect that only appears
    // when a faulted task's backing is re-served to an exiting one (or the
    // reverse) survives two separate runs of one path, and this orders both
    // transitions ten times each.
    if cycle % 2 == 0 {
        slime_rt::debug_write(b"exit\n");
        slime_rt::exit(0)
    }
    slime_rt::debug_write(b"fault\n");
    // A write one page *past* the declared window rather than to a null
    // pointer. Both are unmapped, but a null dereference is undefined behaviour
    // the optimizer may assume unreachable; this address is derived from a base
    // the root chose at runtime, so the store cannot be folded away, and it is
    // exactly the page `Region::admit` refuses.
    //
    // The store does trap -- the "did not trap" report below never appears --
    // but this configuration reports it as `access: Execute status: 0`, the
    // same shape `reclamation-fault`'s own deliberate write produces. The
    // gate therefore asserts `kind=VirtualMemory` and leaves the access open.
    let unmapped = region.base + CYCLE_PAGES * 4096;
    // SAFETY: never mapped. The root reserves exactly `CYCLE_PAGES` pages at
    // `region.base`, and the private window's reservation ends there, so this
    // address is outside every mapping this task holds. The fault it raises is
    // the point: MEM-64M requires immediate reuse after a deliberate fault with
    // the whole quota committed.
    unsafe { (unmapped as *mut u64).write_volatile(stamp(cycle)) }
    // Unreachable: seL4 delivers the fault on the faulting instruction. Kept so
    // the arm cannot fall through into a clean exit and report a fault it never
    // raised.
    report_fail(cycle, b"deliberate fault did not trap", CYCLE_PAGES)
}

/// Read every page as zero, stamp it, then read every stamp back.
///
/// Three passes rather than one fused loop, and the order is the assertion: a
/// root that re-served a live page would fail the first pass, one that mapped
/// two logical pages onto the same frame would fail the third, and fusing the
/// write into the first pass would let an aliased page satisfy its own
/// read-back. The stamp mixes the page index for the same reason.
fn touch(cycle: u64, region: &PrivateMemory) {
    let base = region.base as *mut u64;
    let stamp = stamp(cycle);
    for page in 0..CYCLE_PAGES {
        // SAFETY: the root admitted and mapped exactly `CYCLE_PAGES` pages
        // read-write at `region.base` for this task, and the offset stays
        // inside the page selected by the loop.
        let fresh = unsafe { base.add(page * WORDS_PER_PAGE).read_volatile() };
        if fresh != 0 {
            report_fail(cycle, b"a served page was not zero", page);
        }
    }
    for page in 0..CYCLE_PAGES {
        // SAFETY: as above; the same admitted, mapped, writable region.
        unsafe {
            base.add(page * WORDS_PER_PAGE)
                .write_volatile(stamp ^ page as u64)
        }
    }
    for page in 0..CYCLE_PAGES {
        // SAFETY: as above.
        let written = unsafe { base.add(page * WORDS_PER_PAGE).read_volatile() };
        if written != stamp ^ page as u64 {
            report_fail(cycle, b"a stamped page did not read back", page);
        }
    }
}

/// The value this incarnation stamps, distinct for every cycle and never zero.
///
/// Never zero so the next incarnation's zero test cannot be satisfied by a page
/// this one wrote, and distinct per cycle so a page carried across two lives
/// names the life it came from rather than merely being non-zero.
const fn stamp(cycle: u64) -> u64 {
    0x_4D45_4D36_3400_0000 | (cycle + 1)
}

/// Init's cycle number for this incarnation, or `None` if it never arrived.
fn cycle_token() -> Option<u64> {
    let mut bytes = [0u8; slime_rt::MAX_MSG];
    let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    for _ in 0..TOKEN_YIELDS {
        match slime_rt::recv(CYCLE_TOKEN_SLOT, &mut bytes, &mut caps) {
            slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => return None,
            length if length as usize >= 1 => return Some(u64::from(bytes[0])),
            _ => return None,
        }
    }
    None
}

fn report_fail(cycle: u64, reason: &[u8], detail: usize) -> ! {
    slime_rt::debug_write(b"[private-cycle-probe] FAIL cycle=");
    write_decimal(cycle as usize);
    slime_rt::debug_write(b" ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b" detail=");
    write_decimal(detail);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn fail(reason: &[u8], error: i64) -> ! {
    slime_rt::debug_write(b"[private-cycle-probe] FAIL ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b" error=");
    if error < 0 {
        slime_rt::debug_write(b"-");
        write_decimal(error.unsigned_abs() as usize);
    } else {
        write_decimal(error as usize);
    }
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

fn write_decimal(value: usize) {
    let mut digits = [0u8; 20];
    let mut length = 0;
    let mut remaining = value;
    loop {
        digits[length] = b'0' + (remaining % 10) as u8;
        length += 1;
        remaining /= 10;
        if remaining == 0 {
            break;
        }
    }
    let mut reversed = [0u8; 20];
    for index in 0..length {
        reversed[index] = digits[length - 1 - index];
    }
    slime_rt::debug_write(&reversed[..length]);
}

fn write_hex(value: usize) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut buffer = [0u8; 18];
    buffer[0] = b'0';
    buffer[1] = b'x';
    let mut length = 2;
    let mut shift = 60;
    let mut leading = true;
    loop {
        let nibble = (value >> shift) & 0xf;
        if nibble != 0 || !leading || shift == 0 {
            buffer[length] = DIGITS[nibble];
            length += 1;
            leading = false;
        }
        if shift == 0 {
            break;
        }
        shift -= 4;
    }
    slime_rt::debug_write(&buffer[..length]);
}
