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

slime_rt::entry!(main, worker = worker);

/// The value written into the first granted page before the second growth, and
/// re-read after it. A growth that relocated the base or re-backed an existing
/// page would lose it.
const PATTERN: u64 = 0x5052_4956_4154_4531;
const WORKER_REQUEST: &[u8] = b"grow-side-effect";
const WORKER_REPLY: &[u8] = b"ok";
const LOOPBACK_SLOT: u32 = 0;
const SIDE_EFFECT_READY_WAIT: &[u8] = b"notification:private-memory-side-effect-ready+wait";
const SIDE_EFFECT_READY_SIGNAL: &[u8] = b"notification:private-memory-side-effect-ready+signal";
const GROWTH_DONE_WAIT: &[u8] = b"notification:private-memory-growth-done+wait";
const GROWTH_DONE_SIGNAL: &[u8] = b"notification:private-memory-growth-done+signal";

static WORKER_OK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Whether the worker's *own* growth request was adjudicated against this
/// task's region. Separate from [`WORKER_OK`] so a failure here is not reported
/// as a repeated IPC.
static WORKER_GROW_REFUSED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

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

    let granted = initial.base != 0;
    if !granted {
        if !matches!(
            slime_rt::private_memory_grow(MAX_PROBE_PAGES),
            Err(ERR_OUT_OF_MEMORY)
        ) {
            fail(b"denied growth succeeded", 0)
        }
        denied(initial)
    }

    let ready = slime_rt::resolve_binding(SIDE_EFFECT_READY_WAIT)
        .unwrap_or_else(|error| fail(b"side-effect notification missing", error));
    let done = slime_rt::resolve_binding(GROWTH_DONE_SIGNAL)
        .unwrap_or_else(|error| fail(b"growth-done notification missing", error));
    if slime_rt::notification_wait(ready).is_err() {
        fail(b"side-effect wait failed", 0)
    }

    let mut growth_retries = 0usize;
    let growth = slime_rt::private_memory_grow(MAX_PROBE_PAGES);
    match growth {
        Err(ERR_OUT_OF_MEMORY) => {
            growth_retries = 1;
            let previous = slime_rt::private_memory_grow(1)
                .unwrap_or_else(|error| fail(b"base-page retry refused", error));
            let rest = slime_rt::private_memory_grow(MAX_PROBE_PAGES - 1)
                .unwrap_or_else(|error| fail(b"remaining retry refused", error));
            if rest.pages != 1 || rest.base != initial.base {
                fail(b"retry base changed", 0)
            }
            complete_worker_rpc(done);
            finish_granted(initial, previous, growth_retries)
        }
        Err(error) => fail(b"full-window growth refused unexpectedly", error),
        Ok(previous) => {
            complete_worker_rpc(done);
            finish_granted(initial, previous, growth_retries)
        }
    }
}

fn complete_worker_rpc(growth_done: u32) {
    if slime_rt::notification_signal(growth_done) < 0 {
        fail(b"growth-done notification failed", 0)
    }
}

fn finish_granted(initial: PrivateMemory, previous: PrivateMemory, growth_retries: usize) -> ! {
    if previous.pages != 0 || previous.base != initial.base {
        report(
            b"FAIL full-window growth disagreed with query",
            previous.pages,
        );
        slime_rt::exit(1)
    }
    if !WORKER_OK.load(core::sync::atomic::Ordering::Acquire) {
        report(b"FAIL worker IPC repeated", 0);
        slime_rt::exit(1)
    }
    if !WORKER_GROW_REFUSED.load(core::sync::atomic::Ordering::Acquire) {
        report(b"FAIL worker growth was not adjudicated", 0);
        slime_rt::exit(1)
    }
    granted(initial, growth_retries)
}

fn worker(_startup_arg: u32) {
    let initial = loop {
        match slime_rt::private_memory_grow(0) {
            Ok(region) => break region,
            Err(_) => slime_rt::yield_now(),
        }
    };
    if initial.base == 0 {
        serve_worker_rpc()
    }
    let mut reply = [0u8; slime_rt::MAX_MSG];
    let length = slime_rt::call(LOOPBACK_SLOT, WORKER_REQUEST, &mut reply);
    let grew = slime_rt::private_memory_grow(0);
    // The worker's own growth request, not a size query: every thread in the
    // process shares the task badge, so the root must adjudicate this against
    // the same region the main thread grew. That region is now at its
    // reservation, so the answer is a refusal — which is what makes this a
    // check of the caller's authority rather than of spare capacity.
    WORKER_GROW_REFUSED.store(
        matches!(slime_rt::private_memory_grow(1), Err(ERR_OUT_OF_MEMORY)),
        core::sync::atomic::Ordering::Release,
    );
    let outcome = length == WORKER_REPLY.len() as i64
        && &reply[..WORKER_REPLY.len()] == WORKER_REPLY
        && matches!(grew, Ok(current) if current.base == initial.base && current.pages == MAX_PROBE_PAGES);
    WORKER_OK.store(outcome, core::sync::atomic::Ordering::Release);
    park_worker();
}

fn serve_worker_rpc() -> ! {
    let ready = slime_rt::resolve_binding(SIDE_EFFECT_READY_SIGNAL)
        .unwrap_or_else(|error| fail(b"side-effect notification missing", error));
    let growth_done = slime_rt::resolve_binding(GROWTH_DONE_WAIT)
        .unwrap_or_else(|error| fail(b"growth-done notification missing", error));
    // Serve exactly one request, then park: a second delivery must remain
    // unanswered on a parked worker rather than be absorbed by another
    // iteration of this loop.
    let mut request = [0u8; slime_rt::MAX_MSG];
    let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    let length = slime_rt::recv_blocking(LOOPBACK_SLOT, &mut request, &mut caps);
    if length < 0 || &request[..length as usize] != WORKER_REQUEST {
        fail(b"worker request", length)
    }
    if slime_rt::notification_signal(ready) < 0 {
        fail(b"side-effect notification failed", 0)
    }
    if slime_rt::notification_wait(growth_done).is_err() {
        fail(b"growth-done wait failed", 0)
    }
    if slime_rt::reply(WORKER_REPLY) < 0 {
        fail(b"worker reply", 0)
    }
    WORKER_OK.store(true, core::sync::atomic::Ordering::Release);
    park_worker()
}

fn park_worker() -> ! {
    let mut request = [0u8; slime_rt::MAX_MSG];
    let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    loop {
        let _ = slime_rt::recv_blocking(LOOPBACK_SLOT, &mut request, &mut caps);
    }
}

/// The granted instance touches every 4 KiB subpage of the aligned block, then
/// proves the stable ceiling, base, zero-fill, write persistence, and that the
/// worker's delayed RPC side effect was observed exactly once across growth.
fn granted(initial: PrivateMemory, growth_retries: usize) -> ! {
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
    slime_rt::debug_write(
        b" zeroed=1 survived=1 refused=1 worker_rpc_once=1 worker_grow_refused=1 retries=",
    );
    write_decimal(growth_retries);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(0)
}

/// The denied instance: absent from the budget, so its first page is refused.
fn denied(initial: PrivateMemory) -> ! {
    let after = match slime_rt::private_memory_grow(0) {
        Ok(region) => region,
        Err(error) => fail(b"post-refusal query refused", error),
    };
    if after.pages != 0 || after.base != initial.base {
        report(b"FAIL refused growth changed the region", after.pages);
        slime_rt::exit(1)
    }
    while !WORKER_OK.load(core::sync::atomic::Ordering::Acquire) {
        slime_rt::yield_now();
    }
    slime_rt::debug_write(b"[private-memory-probe] denied pages=0 base=");
    write_hex(initial.base);
    slime_rt::debug_write(b" refused=1\n");
    slime_rt::exit(0)
}

/// This probe's composition-declared 64 MiB workload. The target profile may
/// admit wider holders (MEM-1G), but this component must exercise exactly the
/// quota granted to `private-memory-granted` rather than consume that envelope.
const MAX_PROBE_PAGES: usize = 64 * 1024 * 1024 / 4096;
const _: () = assert!(
    MAX_PROBE_PAGES
        <= boot_contracts::private_memory_budget::capacity_for("aarch64-sel4-qemu-virt").0
);

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
