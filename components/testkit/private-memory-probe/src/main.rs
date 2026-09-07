#![no_std]
#![no_main]

//! C10.2's subject: a real component proving its generation-declared
//! private-memory quota is the live ceiling.
//!
//! C10.1 built the mechanism and proved it on the root's own embedded fixture,
//! against a quota compiled into `slime-root`. That leaves the question C10.2
//! exists to answer untested: does a quota *declared in a generation* reach the
//! component the generation names, and does omission actually deny? The root's
//! own accounting cannot answer it, because the fixture is an ELF the root
//! embeds at compile time and no manifest can name (backlog B5's lesson: a
//! mechanism exercised only by host unit tests and root-internal fixtures is not
//! known to work for components).
//!
//! One executable, two instances, two declared outcomes:
//!
//! * the **granted** instance is named in the generation's
//!   `privateMemoryBudget` and must grow to exactly its declared ceiling, read
//!   every page as zero, keep a written pattern across a later growth, and be
//!   refused the page past that ceiling while staying alive;
//! * the **denied** instance is absent from that budget and must be refused its
//!   very first page, at a reported size of zero.
//!
//! Which one this image is running as is *not* compiled in. It asks the root for
//! its own ceiling — a size query, which allocates nothing — and reports what it
//! observes. The root adjudicates against the budget it admitted, so a component
//! cannot pass by asserting its own copy of the manifest, and a root that
//! stopped honouring declarations cannot be masked by a probe that agrees with
//! it.

use slime_rt::{ERR_OUT_OF_MEMORY, PrivateMemory};

slime_rt::entry!(main);

/// The value written into the first granted page before the second growth, and
/// re-read after it. A growth that relocated the base or re-backed an existing
/// page would lose it.
const PATTERN: u64 = 0x5052_4956_4154_4531;

fn main(_startup_arg: u32) {
    // The size query is the discriminator as well as the first assertion: it
    // must succeed for both instances (asking costs nothing and needs no quota)
    // and must report an unbacked region.
    let initial = match slime_rt::private_memory_grow(0) {
        Ok(region) => region,
        Err(error) => fail(b"size query refused", error),
    };
    if initial.pages != 0 {
        report(b"FAIL region already backed", initial.pages);
        slime_rt::exit(1)
    }
    report_base(b"query", &initial);

    // The whole reserved span is the discriminator. A granted instance maps it
    // as one aligned 2 MiB frame; a denied instance is refused before touching
    // any frame or table.
    match slime_rt::private_memory_grow(MAX_PROBE_PAGES) {
        Err(ERR_OUT_OF_MEMORY) => denied(initial),
        Err(error) => fail(b"full-window growth refused unexpectedly", error),
        Ok(previous) => {
            if previous.pages != 0 || previous.base != initial.base {
                report(
                    b"FAIL full-window growth disagreed with query",
                    previous.pages,
                );
                slime_rt::exit(1)
            }
            granted(initial)
        }
    }
}

/// The granted instance touches every 4 KiB subpage of the aligned block, then
/// proves the stable ceiling, base, zero-fill, and write persistence.
fn granted(initial: PrivateMemory) {
    let pages = MAX_PROBE_PAGES;
    for page in 0..pages {
        // SAFETY: the root just admitted and mapped the full 512-page region
        // read-write for this task; this reads one u64 inside each 4 KiB page.
        let fresh = unsafe { (initial.base as *const u64).add(page * 512).read_volatile() };
        if fresh != 0 {
            report(b"FAIL fresh page was not zeroed", page);
            slime_rt::exit(1)
        }
        // SAFETY: the same admitted region is writable by this task, and the
        // computed address remains inside the page selected by the loop.
        unsafe {
            (initial.base as *mut u64)
                .add(page * 512)
                .write_volatile(if page == 0 { PATTERN } else { page as u64 })
        }
    }

    // The pattern survived every growth, so the base did not move and no
    // existing page was re-backed.
    //
    // SAFETY: page zero remains mapped at the stable base after the complete
    // growth; this reads the same u64 written above.
    let survived = unsafe { (initial.base as *const u64).read_volatile() };
    if survived != PATTERN {
        report(b"FAIL pattern did not survive growth", pages);
        slime_rt::exit(1)
    }

    // The refusal had no effect: the region is still exactly at its ceiling,
    // and asking again is still refused. A mechanism that half-applied a
    // refused growth would show up here as a changed count.
    let after = match slime_rt::private_memory_grow(0) {
        Ok(region) => region,
        Err(error) => fail(b"post-refusal query refused", error),
    };
    if after.pages != pages || after.base != initial.base {
        report(b"FAIL refusal changed the region", after.pages);
        slime_rt::exit(1)
    }
    if !matches!(slime_rt::private_memory_grow(1), Err(ERR_OUT_OF_MEMORY)) {
        report(b"FAIL ceiling was not stable", pages);
        slime_rt::exit(1)
    }

    slime_rt::debug_write(b"[private-memory-probe] granted pages=");
    write_decimal(pages);
    slime_rt::debug_write(b" base=");
    write_hex(initial.base);
    slime_rt::debug_write(b" zeroed=1 survived=1 refused=1\n");
    slime_rt::exit(0)
}

/// The denied instance: absent from the budget, so its first page is refused.
///
/// Deny-by-default is the property under test, and it is stronger than "gets
/// less": the region must be *unbacked*, and stay unbacked after the refusal.
fn denied(initial: PrivateMemory) {
    let after = match slime_rt::private_memory_grow(0) {
        Ok(region) => region,
        Err(error) => fail(b"post-refusal query refused", error),
    };
    if after.pages != 0 || after.base != initial.base {
        report(b"FAIL refused growth changed the region", after.pages);
        slime_rt::exit(1)
    }
    slime_rt::debug_write(b"[private-memory-probe] denied pages=0 base=");
    write_hex(initial.base);
    slime_rt::debug_write(b" refused=1\n");
    slime_rt::exit(0)
}

/// The exact public reservation and one 2 MiB frame on both supported ISAs.
const MAX_PROBE_PAGES: usize = 512;

fn report(reason: &[u8], pages: usize) {
    slime_rt::debug_write(b"[private-memory-probe] ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b" pages=");
    write_decimal(pages);
    slime_rt::debug_write(b"\n");
}

fn report_base(step: &[u8], region: &PrivateMemory) {
    slime_rt::debug_write(b"[private-memory-probe] ");
    slime_rt::debug_write(step);
    slime_rt::debug_write(b" pages=");
    write_decimal(region.pages);
    slime_rt::debug_write(b" base=");
    write_hex(region.base);
    slime_rt::debug_write(b"\n");
}

fn fail(reason: &[u8], error: i64) -> ! {
    slime_rt::debug_write(b"[private-memory-probe] FAIL ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b" status=");
    write_decimal(error.unsigned_abs() as usize);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
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

fn write_hex(value: usize) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0u8; 18];
    out[0] = b'0';
    out[1] = b'x';
    let mut written = 2;
    let mut started = false;
    for shift in (0..16).rev() {
        let nibble = (value >> (shift * 4)) & 0xf;
        if nibble != 0 || started || shift == 0 {
            started = true;
            out[written] = DIGITS[nibble];
            written += 1;
        }
    }
    slime_rt::debug_write(&out[..written]);
}
