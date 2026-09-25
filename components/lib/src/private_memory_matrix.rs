//! Userspace behaviour for the adaptive private-memory inventory matrix.
//!
//! One image, booted unchanged against several kernel-visible RAM inventories.
//! The policy declares no capacity: the bulk and small subjects hold
//! pool-relative maxima, so what they can reach is whatever the booted
//! inventory leaves after the root's own commitments. The coordinator never
//! states how much that is. It drives every holder to an exact refusal and
//! reports what each holder measured, and the gate compares those reports
//! across inventories and against the root's own adjudication and census.
//!
//! Three schedules run in sequence, each ending in exhaustion:
//!
//! * **bulk** — one holder grows in maximal large-frame transactions, then
//!   halves its request after every refusal down to a single page, while the
//!   small holder sits bound and idle with the whole pool as its maximum and
//!   a guaranteed holder keeps half its promise. At the one-page refusal the
//!   guaranteed holder must still redeem the other half, and its first page
//!   past the promise must be refused, as must the idle holder's first page.
//!   A fresh incarnation spawned under that pressure is funded by the
//!   operational reserve and refused its first page without harm.
//! * **small** — once the bulk holder exits, a different holder takes its
//!   returned capacity in requests that never cover an aligned 2 MiB span, so
//!   every page is a base page with its own slot and descriptor.
//! * **mixed** — replacement bulk and small incarnations alternate against
//!   one pool until both are refused a single page.
//!
//! Twenty coordinated cycles follow: a holder grows, dies by fault or exit,
//! and the other subject is served the same capacity and must read it zeroed,
//! while the guaranteed peer re-reads its whole pattern.
//!
//! Every served page is read as zero and stamped with a value that names its
//! holder, incarnation and word; every resident page is re-read at each
//! verification. A later incarnation reading its predecessor's stamp, or a
//! holder reading another's, fails on the first word.

use core::fmt::{self, Write};
use slime_proto::private_memory_probe::{self as protocol, WireMessage};
use slime_rt::Termination;

const PAGE_BYTES: usize = 4096;
const WORDS_PER_PAGE: usize = PAGE_BYTES / 8;
/// The guaranteed subject's promise and its fixed maximum, in pages. The
/// maximum is twice the promise so one page past the promise is inside the
/// permission and its refusal is the pool's, not the declaration's.
const GUARANTEE: usize = 1024;
const GUARANTEED_MAXIMUM: usize = 2 * GUARANTEE;
/// One maximal bulk transaction: sixty-four 2 MiB frames, the root's
/// per-transaction extent bound.
const BULK_UNIT: usize = 64 * 512;
/// One small transaction. Odd and below 512, so from an aligned base no
/// request ever covers a whole aligned 2 MiB span.
const SMALL_UNIT: usize = 511;
/// Coordinated death-and-reuse cycles, and the pages each victim holds. Odd,
/// so every cycle carries both large frames and base pages.
const CYCLES: usize = 20;
const CYCLE_PAGES: usize = 4095;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Guaranteed,
    Bulk,
    Small,
}

impl Role {
    const ALL: [Role; 3] = [Role::Guaranteed, Role::Bulk, Role::Small];

    fn control(self) -> &'static [u8] {
        match self {
            Role::Guaranteed => b"matrix-guaranteed-control",
            Role::Bulk => b"matrix-bulk-control",
            Role::Small => b"matrix-small-control",
        }
    }

    fn executable(self) -> &'static [u8] {
        match self {
            Role::Guaranteed => b"matrix-guaranteed-executable",
            Role::Bulk => b"matrix-bulk-executable",
            Role::Small => b"matrix-small-executable",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Role::Guaranteed => "guaranteed",
            Role::Bulk => "bulk",
            Role::Small => "small",
        }
    }

    fn instance(self) -> &'static str {
        match self {
            Role::Guaranteed => "private-matrix-guaranteed",
            Role::Bulk => "private-matrix-bulk",
            Role::Small => "private-matrix-small",
        }
    }

    fn ordinal(self) -> u32 {
        match self {
            Role::Guaranteed => 0,
            Role::Bulk => 1,
            Role::Small => 2,
        }
    }

    fn slot(self) -> usize {
        self.ordinal() as usize
    }
}

pub fn run(_: u32) {
    if slime_rt::resolve_binding(Role::Bulk.executable()).is_ok() {
        coordinate()
    }
    let role = Role::ALL
        .into_iter()
        .find(|role| slime_rt::resolve_binding(role.control()).is_ok())
        .unwrap_or_else(|| fail("holder", "no control endpoint resolved"));
    let endpoint = slime_rt::resolve_binding(role.control())
        .unwrap_or_else(|_| fail(role.label(), "control endpoint"));
    hold(role, endpoint)
}

fn fail(label: &str, reason: &str) -> ! {
    trace(format_args!("[private-matrix:{label}] FAIL {reason}"));
    slime_rt::exit(1)
}

fn message(operation: u32, role: Role, incarnation: u32) -> WireMessage {
    WireMessage {
        version: protocol::FORMAT_VERSION,
        operation,
        holder: role.ordinal(),
        incarnation,
        address: 0,
        reserved: [0; 40],
    }
}

fn decode(label: &str, bytes: &[u8]) -> WireMessage {
    if bytes.len() != protocol::MESSAGE_LEN {
        fail(label, "message length");
    }
    let value = WireMessage::decode(bytes).unwrap_or_else(|| fail(label, "message decode"));
    if value.version != protocol::FORMAT_VERSION
        || value.holder as usize >= Role::ALL.len()
        || value.reserved != [0; 40]
    {
        fail(label, "message bounds");
    }
    value
}

// ---------------------------------------------------------------------------
// Coordinator
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Subject {
    endpoint: u32,
    handle: Option<u32>,
    base: u64,
    pages: usize,
    incarnation: u32,
    live: bool,
}

type Subjects = [Subject; 3];

/// What one exhaustion walk observed for one holder.
#[derive(Clone, Copy, Default)]
struct Walk {
    served: usize,
    refused: usize,
}

fn coordinate() -> ! {
    let mut subjects: Subjects = [Subject {
        endpoint: 0,
        handle: None,
        base: 0,
        pages: 0,
        incarnation: 0,
        live: false,
    }; 3];
    for role in Role::ALL {
        subjects[role.slot()].endpoint = slime_rt::resolve_binding(role.control())
            .unwrap_or_else(|_| fail("coordinator", "control endpoint"));
    }

    // Bulk schedule. The guaranteed holder takes half its promise before any
    // pressure exists, so the other half is redeemed at the exhaustion point.
    spawn_subject(&mut subjects, Role::Guaranteed);
    let half = GUARANTEE / 2;
    if request(&mut subjects, Role::Guaranteed, half) != half {
        fail("coordinator", "a guarantee was not served before pressure");
    }
    // The small holder is bound first and stays idle. Its maximum is the whole
    // pool, so if a maximum reserved anything the bulk walk below would stop
    // short of the pool the root reports.
    spawn_subject(&mut subjects, Role::Small);
    spawn_subject(&mut subjects, Role::Bulk);
    let bulk = exhaust(&mut subjects, Role::Bulk, BULK_UNIT);
    let redeemed = request(&mut subjects, Role::Guaranteed, GUARANTEE - half);
    let beyond = request(&mut subjects, Role::Guaranteed, 1);
    trace(format_args!(
        "[private-matrix] guarantee pressure redeemed={} beyond_promise_served={} pages={}",
        u8::from(redeemed == GUARANTEE),
        u8::from(beyond != redeemed),
        subjects[Role::Guaranteed.slot()].pages,
    ));
    if redeemed != GUARANTEE || beyond != redeemed {
        fail("coordinator", "the guarantee was not the pool's exception");
    }
    let idle = request(&mut subjects, Role::Small, 1);
    trace(format_args!(
        "[private-matrix] idle maximum subject={} pages={} refused={}",
        Role::Small.instance(),
        idle,
        u8::from(idle == 0),
    ));
    if idle != 0 {
        fail(
            "coordinator",
            "an idle maximum held capacity the walk could not reach",
        );
    }
    // A construction while the pool is empty: the operational reserve funds
    // the task, and the task's own first page is refused like any other.
    finish(&mut subjects, Role::Small);
    respawn(&mut subjects, Role::Small);
    let starved = request(&mut subjects, Role::Small, 1);
    trace(format_args!(
        "[private-matrix] reserve spawn subject={} incarnation={} pages={} refused={}",
        Role::Small.instance(),
        subjects[Role::Small.slot()].incarnation,
        starved,
        u8::from(starved == 0),
    ));
    if starved != 0 {
        fail("coordinator", "an exhausted pool served a fresh holder");
    }
    let resident = verify_all(&mut subjects);
    report("bulk", &subjects, &[(Role::Bulk, bulk)], resident);
    finish(&mut subjects, Role::Bulk);

    // Small schedule: the returned capacity, taken back page-granularly by a
    // different holder.
    let small = exhaust(&mut subjects, Role::Small, SMALL_UNIT);
    let resident = verify_all(&mut subjects);
    report("small", &subjects, &[(Role::Small, small)], resident);
    finish(&mut subjects, Role::Small);

    // Mixed schedule: replacement incarnations of both shapes alternating.
    for role in [Role::Bulk, Role::Small] {
        respawn(&mut subjects, role);
    }
    let walks = exhaust_pair(&mut subjects);
    let resident = verify_all(&mut subjects);
    report(
        "mixed",
        &subjects,
        &[(Role::Bulk, walks[0]), (Role::Small, walks[1])],
        resident,
    );
    for role in [Role::Bulk, Role::Small] {
        finish(&mut subjects, role);
    }

    cycles(&mut subjects);
    finish(&mut subjects, Role::Guaranteed);
    trace(format_args!(
        "[private-matrix] complete schedules=3 cycles={CYCLES}"
    ));
    slime_rt::exit(0)
}

/// Twenty coordinated deaths, each followed by a different holder's reuse.
///
/// The victim alternates between the two pool subjects and its end between a
/// fault and a clean exit, so both reclamation paths and both mapping shapes
/// return capacity. The reuser is always the other subject: it is served the
/// same number of pages the victim held and must read every one as zero. The
/// guaranteed peer keeps its whole pattern throughout and re-reads it after
/// every cycle. The root's census after each reuser's exit is what the gate
/// compares against the census taken before the first cycle.
fn cycles(subjects: &mut Subjects) {
    trace(format_args!(
        "[private-matrix] cycles begin count={CYCLES} pages={CYCLE_PAGES}"
    ));
    for cycle in 0..CYCLES {
        let (victim, reuser) = if cycle % 2 == 0 {
            (Role::Bulk, Role::Small)
        } else {
            (Role::Small, Role::Bulk)
        };
        let fault = cycle % 4 < 2;
        respawn(subjects, victim);
        if request(subjects, victim, CYCLE_PAGES) != CYCLE_PAGES {
            fail("coordinator", "a cycle victim was not served");
        }
        if fault {
            call(subjects, victim, protocol::FAULT, 0);
            let handle = subjects[victim.slot()]
                .handle
                .take()
                .unwrap_or_else(|| fail("coordinator", "victim handle"));
            await_end(handle, true);
            subjects[victim.slot()].live = false;
        } else {
            finish(subjects, victim);
        }
        respawn(subjects, reuser);
        if request(subjects, reuser, CYCLE_PAGES) != CYCLE_PAGES {
            fail(
                "coordinator",
                "returned capacity was not served to another holder",
            );
        }
        let peer = verify_all(subjects);
        finish(subjects, reuser);
        trace(format_args!(
            "[private-matrix] cycle={} victim={} incarnation={} end={} reuser={} incarnation={} pages={} zeroed=1 peer_pages={}",
            cycle,
            victim.instance(),
            subjects[victim.slot()].incarnation,
            if fault { "fault" } else { "exit" },
            reuser.instance(),
            subjects[reuser.slot()].incarnation,
            CYCLE_PAGES,
            peer - CYCLE_PAGES,
        ));
    }
}

/// Grow one holder until a single page is refused.
///
/// A refusal halves the request; a success repeats it. The walk therefore
/// ends only when the one-page request is refused, and every refusal before
/// it is followed by a smaller request the holder was able to take.
fn exhaust(subjects: &mut Subjects, role: Role, unit: usize) -> Walk {
    let mut delta = unit;
    let mut walk = Walk::default();
    loop {
        let before = subjects[role.slot()].pages;
        if request(subjects, role, delta) == before {
            walk.refused += 1;
            if delta == 1 {
                return walk;
            }
            delta /= 2;
        } else {
            walk.served += 1;
        }
    }
}

/// Alternate two holders' walks against one pool, each on its own delta.
fn exhaust_pair(subjects: &mut Subjects) -> [Walk; 2] {
    let roles = [(Role::Bulk, BULK_UNIT), (Role::Small, SMALL_UNIT)];
    let mut deltas = [roles[0].1, roles[1].1];
    let mut done = [false; 2];
    let mut walks = [Walk::default(); 2];
    while !(done[0] && done[1]) {
        for (index, (role, _)) in roles.into_iter().enumerate() {
            if done[index] {
                continue;
            }
            let before = subjects[role.slot()].pages;
            if request(subjects, role, deltas[index]) == before {
                walks[index].refused += 1;
                if deltas[index] == 1 {
                    done[index] = true;
                } else {
                    deltas[index] /= 2;
                }
            } else {
                walks[index].served += 1;
            }
        }
    }
    walks
}

/// Re-read every resident holder's whole extent; answer the pages verified.
fn verify_all(subjects: &mut Subjects) -> usize {
    let mut resident = 0usize;
    for role in Role::ALL {
        if !subjects[role.slot()].live {
            continue;
        }
        if query_base(subjects, role) != subjects[role.slot()].base {
            fail("coordinator", "a resident holder's window base moved");
        }
        resident += subjects[role.slot()].pages;
    }
    resident
}

fn report(schedule: &str, subjects: &Subjects, walks: &[(Role, Walk)], resident: usize) {
    for (role, walk) in walks {
        trace(format_args!(
            "[private-matrix] exhausted schedule={} subject={} incarnation={} pages={} served={} refused={} final_delta=1",
            schedule,
            role.instance(),
            subjects[role.slot()].incarnation,
            subjects[role.slot()].pages,
            walk.served,
            walk.refused,
        ));
    }
    trace(format_args!(
        "[private-matrix] resident schedule={} pages={} bytes={} guaranteed={} bulk={} small={}",
        schedule,
        resident,
        resident * PAGE_BYTES,
        subjects[Role::Guaranteed.slot()].pages,
        live_pages(subjects, Role::Bulk),
        live_pages(subjects, Role::Small),
    ));
}

fn live_pages(subjects: &Subjects, role: Role) -> usize {
    if subjects[role.slot()].live {
        subjects[role.slot()].pages
    } else {
        0
    }
}

/// Spawn the next incarnation of a subject whose previous one has ended.
fn respawn(subjects: &mut Subjects, role: Role) {
    if subjects[role.slot()].live {
        fail("coordinator", "respawned a live subject");
    }
    subjects[role.slot()].incarnation += 1;
    spawn_subject(subjects, role);
}

fn spawn_subject(subjects: &mut Subjects, role: Role) {
    let executable = slime_rt::resolve_binding(role.executable())
        .unwrap_or_else(|_| fail("coordinator", "spawn authority"));
    let spawned =
        slime_rt::spawn(executable, &[]).unwrap_or_else(|_| fail("coordinator", "spawn refused"));
    let subject = &mut subjects[role.slot()];
    subject.handle = Some(spawned.supervision_slot);
    subject.live = true;
    subject.pages = 0;
    subjects[role.slot()].base = query_base(subjects, role);
}

fn call(subjects: &Subjects, role: Role, operation: u32, address: u64) -> u64 {
    let entry = subjects[role.slot()];
    let mut response = [0u8; slime_rt::MAX_MSG];
    let mut request = message(operation, role, entry.incarnation);
    request.address = address;
    let size = slime_rt::call(entry.endpoint, &request.encode(), &mut response);
    if size != protocol::MESSAGE_LEN as i64 {
        fail("coordinator", "command reply length");
    }
    let reply = decode("coordinator", &response[..size as usize]);
    if reply.operation != protocol::ACKNOWLEDGE
        || reply.holder != role.ordinal()
        || reply.incarnation != entry.incarnation
    {
        fail("coordinator", "reply identity");
    }
    reply.address
}

/// Ask `role` for `delta` more pages; answer and record the extent it holds.
fn request(subjects: &mut Subjects, role: Role, delta: usize) -> usize {
    let before = subjects[role.slot()].pages;
    let pages = usize::try_from(call(subjects, role, protocol::GROW, delta as u64))
        .unwrap_or_else(|_| fail("coordinator", "extent reply"));
    if pages != before && pages != before + delta {
        fail(
            "coordinator",
            "a subject reported an extent it was not offered",
        );
    }
    subjects[role.slot()].pages = pages;
    pages
}

fn query_base(subjects: &Subjects, role: Role) -> u64 {
    let base = call(subjects, role, protocol::VERIFY, 0);
    if base == 0 {
        fail("coordinator", "a bound subject reported no window");
    }
    base
}

fn finish(subjects: &mut Subjects, role: Role) {
    call(subjects, role, protocol::FINISH, 0);
    if let Some(handle) = subjects[role.slot()].handle.take() {
        await_end(handle, false);
    }
    subjects[role.slot()].live = false;
}

fn await_end(handle: u32, fault: bool) {
    for _ in 0..262_144 {
        match slime_rt::supervision_status(handle) {
            Ok(None) => slime_rt::yield_now(),
            Ok(Some(Termination::Fault(_))) if fault => return,
            Ok(Some(Termination::Exit(0))) if !fault => return,
            _ => fail("coordinator", "unexpected termination"),
        }
    }
    fail("coordinator", "termination bound")
}

// ---------------------------------------------------------------------------
// Holder
// ---------------------------------------------------------------------------

fn hold(role: Role, endpoint: u32) -> ! {
    let label = role.label();
    let initial =
        slime_rt::private_memory_grow(0).unwrap_or_else(|_| fail(label, "extent query refused"));
    let base = initial.base;
    if initial.pages != 0 || base == 0 {
        fail(
            label,
            "a fresh incarnation did not arrive with an empty window",
        );
    }
    let mut pages = 0usize;
    let mut refused = 0usize;
    let mut incarnation = None;
    loop {
        let mut bytes = [0u8; slime_rt::MAX_MSG];
        let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
        if slime_rt::recv_blocking(endpoint, &mut bytes, &mut caps) != protocol::MESSAGE_LEN as i64
        {
            fail(label, "receive");
        }
        let request = decode(label, &bytes[..protocol::MESSAGE_LEN]);
        if request.holder != role.ordinal()
            || incarnation.is_some_and(|value| value != request.incarnation)
        {
            fail(label, "command identity");
        }
        incarnation = Some(request.incarnation);
        let reply_address = match request.operation {
            protocol::GROW => {
                let delta = usize::try_from(request.address)
                    .ok()
                    .filter(|delta| *delta != 0)
                    .unwrap_or_else(|| fail(label, "growth delta"));
                pages = grow(role, base, pages, delta, &mut refused, request.incarnation);
                pages as u64
            }
            protocol::VERIFY | protocol::FINISH | protocol::FAULT => {
                verify(role, base, pages, request.incarnation, refused);
                base as u64
            }
            _ => fail(label, "operation"),
        };
        let mut reply = message(protocol::ACKNOWLEDGE, role, request.incarnation);
        reply.address = reply_address;
        if slime_rt::reply(&reply.encode()) != slime_rt::ERR_SUCCESS {
            fail(label, "reply");
        }
        if request.operation == protocol::FINISH {
            trace(format_args!(
                "[private-matrix:{label}] end incarnation={} pages={pages} refused={refused} kind=exit",
                request.incarnation,
            ));
            slime_rt::exit(0);
        }
        if request.operation == protocol::FAULT {
            trace(format_args!(
                "[private-matrix:{label}] end incarnation={} pages={pages} refused={refused} kind=fault",
                request.incarnation,
            ));
            // The first page past the committed extent: inside the permitted
            // window, never backed, so the access faults.
            fault_at(base + pages * PAGE_BYTES);
            fail(label, "the unbacked extent did not fault");
        }
    }
}

/// Request `delta` more pages and answer the extent held afterwards.
///
/// Only the new pages are touched here. A refusal must leave the extent where
/// it was; the whole extent is re-read at the next verification rather than
/// after every request, which would make an exhaustion walk quadratic.
fn grow(
    role: Role,
    base: usize,
    pages: usize,
    delta: usize,
    refused: &mut usize,
    incarnation: u32,
) -> usize {
    let label = role.label();
    let Ok(previous) = slime_rt::private_memory_grow(delta) else {
        *refused += 1;
        let current =
            slime_rt::private_memory_grow(0).unwrap_or_else(|_| fail(label, "extent query"));
        if current.base != base || current.pages != pages {
            fail(label, "a refused request changed the extent");
        }
        return pages;
    };
    if previous.base != base || previous.pages != pages {
        fail(label, "growth disagreed with the measured extent");
    }
    let grown = pages + delta;
    if role == Role::Guaranteed && grown > GUARANTEED_MAXIMUM {
        fail(label, "a growth exceeded the declared maximum");
    }
    let current = slime_rt::private_memory_grow(0).unwrap_or_else(|_| fail(label, "extent query"));
    if current.base != base || current.pages != grown {
        fail(label, "the extent after growth disagrees with the request");
    }
    stamp_new(role, base, pages, grown, incarnation);
    grown
}

fn verify(role: Role, base: usize, pages: usize, incarnation: u32, refused: usize) {
    let label = role.label();
    let current =
        slime_rt::private_memory_grow(0).unwrap_or_else(|_| fail(label, "extent query refused"));
    if current.base != base || current.pages != pages {
        fail(label, "the extent moved between commands");
    }
    let pointer = base as *const u64;
    let stamp = stamp(role, incarnation);
    for page in 0..pages {
        for word in ends(page) {
            // SAFETY: every page below `pages` was served and mapped read-write
            // at `base`, and the holder retains its region until it ends.
            if unsafe { pointer.add(word).read_volatile() } != stamp ^ word as u64 {
                fail(label, "a resident word did not read back");
            }
        }
    }
    trace(format_args!(
        "[private-matrix:{}] verified incarnation={incarnation} base=0x{base:x} pages={pages} pattern=1 refused={refused}",
        label,
    ));
}

/// The first and last word of a page. A mapping covering only a prefix of the
/// page fails the second; the word index in the stamp makes an alias fail the
/// first.
fn ends(page: usize) -> [usize; 2] {
    [
        page * WORDS_PER_PAGE,
        page * WORDS_PER_PAGE + WORDS_PER_PAGE - 1,
    ]
}

fn stamp(role: Role, incarnation: u32) -> u64 {
    0x4d41_5452_0000_0000u64
        | (u64::from(role.ordinal() + 1) << 24)
        | ((u64::from(incarnation) + 1) << 8)
}

fn stamp_new(role: Role, base: usize, start: usize, end: usize, incarnation: u32) {
    let label = role.label();
    let stamp = stamp(role, incarnation);
    let pointer = base as *mut u64;
    for page in start..end {
        for word in ends(page) {
            // SAFETY: successful growth mapped every page below `end`
            // read-write at `base`, so this index is inside the region.
            unsafe {
                if pointer.add(word).read_volatile() != 0 {
                    fail(label, "a newly served word was not zero");
                }
                pointer.add(word).write_volatile(stamp ^ word as u64);
            }
        }
    }
}

/// Store to `address` through inline assembly, to fault deliberately. A
/// volatile write through an unmapped pointer would be undefined behaviour.
fn fault_at(address: usize) {
    #[cfg(target_arch = "aarch64")]
    // SAFETY: one store to an address this task's VSpace does not back; the
    // fault is the intent and the block touches no stack slot.
    unsafe {
        core::arch::asm!("str xzr, [{address}]", address = in(reg) address, options(nostack));
    }
    #[cfg(target_arch = "riscv64")]
    // SAFETY: the same deliberate unbacked-access fault on RV64.
    unsafe {
        core::arch::asm!("sd zero, 0({address})", address = in(reg) address, options(nostack));
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "riscv64")))]
    let _ = address;
}

/// One bounded console request per marker, so a line cannot interleave with
/// root diagnostics.
fn trace(arguments: fmt::Arguments<'_>) {
    struct Line {
        bytes: [u8; 256],
        used: usize,
    }
    impl Write for Line {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            let end = self
                .used
                .checked_add(text.len())
                .filter(|end| *end < self.bytes.len())
                .ok_or(fmt::Error)?;
            self.bytes[self.used..end].copy_from_slice(text.as_bytes());
            self.used = end;
            Ok(())
        }
    }
    let mut line = Line {
        bytes: [0; 256],
        used: 0,
    };
    if line.write_fmt(arguments).is_err() {
        slime_rt::debug_write(b"[private-matrix] FAIL marker bounds\n");
        slime_rt::exit(1);
    }
    line.bytes[line.used] = b'\n';
    slime_rt::debug_write(&line.bytes[..line.used + 1]);
}
