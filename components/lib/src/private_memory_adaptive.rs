//! Userspace behaviour for the adaptive private-memory plane.
//!
//! The fixed planes prove that a *declared per-instance quota* is the live
//! ceiling. This one exists for the question a fixed quota cannot express: when
//! several subjects share one entitlement, when a maximum is a permission
//! rather than a promise, and when an incarnation is replaced, does the root
//! adjudicate each request against the declared policy, in receipt order, with
//! the same answer every boot?
//!
//! Three properties are load-bearing here and are what the coordinator's
//! structure is for:
//!
//! * **The schedule is fixed; the outcomes are not asserted.** Every request is
//!   a synchronous `call`, so the root receives them in exactly the order
//!   [`schedule`] lists. The coordinator records whether each was served and
//!   never states what it expected, because the claim under test is that two
//!   boots of the same image produce the same grant/refusal sequence — an
//!   expectation compiled in here would make the gate compare this file against
//!   itself. The only outcomes a holder does assert are structural rather than
//!   policy-dependent: a refused request must charge nothing, a served one must
//!   deliver exactly what it asked for, and a task with no entitlement must be
//!   refused its first page.
//! * **Every holder measures its own region.** A holder never reads the
//!   composition: it queries its extent, verifies newly served pages are zero
//!   before stamping them, and re-reads earlier stamps. The gate compares that
//!   against the root's own accounting, which the holder cannot see.
//! * **Bases are reported, not assumed.** A window base belongs to the task's
//!   VSpace plan, so it must be identical across boots. It is carried into the
//!   schedule digest for that reason.
//!
//! The wire protocol is `contracts/private-memory-probe/v1`, unchanged. This
//! plane uses the subset it needs, with the plane-specific reading of the two
//! overloaded fields that the existing capacity and stress bodies already use:
//! a `GROW` request carries its page delta in `address` and its reply carries
//! the extent held *after* the attempt, while a `VERIFY` reply carries the
//! window base. `FAULT` and `FINISH` are the two deliberate ends.

use core::fmt::{self, Write};
use slime_proto::private_memory_probe::{self as protocol, WireMessage};
use slime_rt::Termination;

const PAGE_BYTES: usize = 4096;
const WORDS_PER_PAGE: usize = PAGE_BYTES / 8;

/// The declared maxima this plane's composition uses, in pages.
///
/// Deliberately not powers of two and not derived from anything the host
/// observed: 257 MiB and 1537 MiB exercise the arithmetic a 2 MiB-aligned
/// planner is most likely to get wrong, and 1537 MiB is larger than either
/// reference machine's RAM precisely because an elastic maximum is a
/// permission. A subject may ask; admission never reserved it.
const GUARANTEED_MAXIMUM: usize = 65_792;
const POOL_MAXIMUM: usize = 393_472;
const RESTART_MAXIMUM: usize = 16_384;
/// The guaranteed holder's promise, and the restartable holder's. Small, and
/// simultaneously reservable on both reference machines, because a guarantee is
/// a commitment admission must fund in full before anything is published.
const GUARANTEED_GUARANTEE: usize = 4_096;
const RESTART_GUARANTEE: usize = 2_048;

/// The unit request the repeated schedule uses, in pages: 1 MiB plus one page,
/// so neither a large-frame planner nor a page-granular one can serve a round
/// without leaving a remainder.
const UNIT: usize = 257;
/// Rounds of the repeated `(pool-a, pool-b, guaranteed)` request cycle.
const ROUNDS: usize = 3;
/// Opening bind requests, the repeated cycle, three refusal boundaries, and a
/// closing cycle that proves a fitting request survives them.
const STEPS: usize = 3 + 3 * ROUNDS + 3 + 3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Guaranteed,
    PoolA,
    PoolB,
    Restart,
    Denied,
}

impl Role {
    const ALL: [Role; 5] = [
        Role::Guaranteed,
        Role::PoolA,
        Role::PoolB,
        Role::Restart,
        Role::Denied,
    ];

    fn control(self) -> &'static [u8] {
        match self {
            Role::Guaranteed => b"adaptive-guaranteed-control",
            Role::PoolA => b"adaptive-pool-a-control",
            Role::PoolB => b"adaptive-pool-b-control",
            Role::Restart => b"adaptive-restart-control",
            Role::Denied => b"adaptive-denied-control",
        }
    }

    /// The spawn authority the coordinator holds for this role, if the role is
    /// dynamically spawned. `PoolB` has none: it is a root-autostart subject,
    /// which is how this plane gets two subjects of one entitlement under
    /// genuinely different owners.
    fn executable(self) -> Option<&'static [u8]> {
        match self {
            Role::Guaranteed => Some(b"adaptive-guaranteed-executable"),
            Role::PoolA => Some(b"adaptive-pool-a-executable"),
            Role::PoolB => None,
            Role::Restart => Some(b"adaptive-restart-executable"),
            Role::Denied => Some(b"adaptive-denied-executable"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Role::Guaranteed => "guaranteed",
            Role::PoolA => "pool-a",
            Role::PoolB => "pool-b",
            Role::Restart => "restart",
            Role::Denied => "denied",
        }
    }

    /// The instance identity the composition declares, which is also what the
    /// root's own markers name. Printed by the coordinator so the gate can join
    /// a component observation to a root adjudication without a lookup table.
    fn instance(self) -> &'static str {
        match self {
            Role::Guaranteed => "private-adaptive-guaranteed",
            Role::PoolA => "private-adaptive-pool-a",
            Role::PoolB => "private-adaptive-pool-b",
            Role::Restart => "private-adaptive-restart",
            Role::Denied => "private-adaptive-denied",
        }
    }

    /// Also the protocol's `holder` field, so a reply cannot be attributed to
    /// the wrong subject.
    fn ordinal(self) -> u32 {
        match self {
            Role::Guaranteed => 0,
            Role::PoolA => 1,
            Role::PoolB => 2,
            Role::Restart => 3,
            Role::Denied => 4,
        }
    }

    fn slot(self) -> usize {
        self.ordinal() as usize
    }

    /// The largest extent this role may ever reach, used only to bound the
    /// holder's own arithmetic. It is not an expectation about any individual
    /// request: the root adjudicates, and a request inside this bound may still
    /// be refused because the shared pool cannot fund it.
    fn ceiling(self) -> usize {
        match self {
            Role::Guaranteed => GUARANTEED_MAXIMUM,
            Role::PoolA | Role::PoolB => POOL_MAXIMUM,
            Role::Restart => RESTART_MAXIMUM,
            Role::Denied => 0,
        }
    }
}

/// One entry of the fixed request schedule: which subject asks, and for how
/// many pages. No expected outcome — see the module comment.
struct Step {
    role: Role,
    delta: usize,
}

const fn step(role: Role, delta: usize) -> Step {
    Step { role, delta }
}

/// The schedule, in receipt order.
///
/// Its shape is the claim. The opening three requests bind each subject and
/// take the guaranteed holder to exactly its promise. The repeated
/// `(pool-a, pool-b, guaranteed)` cycle is what makes "deterministic outcomes
/// for a coordinated repeated request schedule" checkable: two cohort members
/// alternate small requests against one shared pool while a guaranteed
/// neighbour interleaves, so a root that served whoever asked first, or that
/// stranded one cohort member's backing behind the other's, produces a
/// different sequence. The three large requests then reach three different
/// boundaries — a request inside the declared maximum that no inventory can
/// fund, one page past a subject maximum, and one page past the guaranteed
/// subject's maximum — and the closing cycle proves a fitting request is still
/// served after all three.
fn schedule() -> [Step; STEPS] {
    // Pages each pool subject holds when it reaches its large request: the
    // opening bind plus one per round.
    let held = (ROUNDS + 1) * UNIT;
    [
        step(Role::PoolB, UNIT),
        step(Role::PoolA, UNIT),
        step(Role::Guaranteed, GUARANTEED_GUARANTEE),
        step(Role::PoolA, UNIT),
        step(Role::PoolB, UNIT),
        step(Role::Guaranteed, UNIT),
        step(Role::PoolA, UNIT),
        step(Role::PoolB, UNIT),
        step(Role::Guaranteed, UNIT),
        step(Role::PoolA, UNIT),
        step(Role::PoolB, UNIT),
        step(Role::Guaranteed, UNIT),
        // Exactly the subject maximum, which is inside the permission and far
        // beyond anything the shared pool holds.
        step(Role::PoolA, POOL_MAXIMUM - held),
        // One page past the subject maximum, so the refusal is the declaration
        // rather than the inventory.
        step(Role::PoolB, POOL_MAXIMUM - held + 1),
        // One page past the guaranteed subject's own maximum, from an extent
        // that already holds its whole guarantee.
        step(
            Role::Guaranteed,
            GUARANTEED_MAXIMUM - GUARANTEED_GUARANTEE - ROUNDS * UNIT + 1,
        ),
        step(Role::PoolA, UNIT),
        step(Role::PoolB, UNIT),
        step(Role::Guaranteed, UNIT),
    ]
}

pub fn run(_: u32) {
    let coordinator = Role::Guaranteed
        .executable()
        .is_some_and(|name| slime_rt::resolve_binding(name).is_ok());
    if coordinator {
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
    trace(format_args!("[private-adaptive:{label}] FAIL {reason}"));
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

/// Per-subject state the coordinator maintains. `pages` is what the subject
/// reported after its last request, so "served" is derived from the subject's
/// own measurement rather than from anything the coordinator assumed.
#[derive(Clone, Copy)]
struct Subject {
    endpoint: u32,
    handle: Option<u32>,
    base: u64,
    pages: usize,
    incarnation: u32,
}

type Subjects = [Subject; 5];

fn coordinate() -> ! {
    // The coordinator is a declared instance the policy does not name as a
    // subject. Deny-by-default must therefore refuse it a single page before
    // any holder exists, and the refusal must be a structural error it survives
    // rather than a fault.
    if slime_rt::private_memory_grow(1).is_ok() {
        fail(
            "coordinator",
            "a task with no entitlement was served a page",
        );
    }
    let query = slime_rt::private_memory_grow(0)
        .unwrap_or_else(|_| fail("coordinator", "extent query refused"));
    if query.base != 0 || query.pages != 0 {
        fail("coordinator", "a denied task received a window");
    }
    trace(format_args!(
        "[private-adaptive] coordinator subject=private-adaptive-coordinator entitlement=none base=0x0 pages=0 refused=1"
    ));

    let mut subjects: Subjects = [Subject {
        endpoint: 0,
        handle: None,
        base: 0,
        pages: 0,
        incarnation: 0,
    }; 5];
    for role in Role::ALL {
        subjects[role.slot()].endpoint = slime_rt::resolve_binding(role.control())
            .unwrap_or_else(|_| fail("coordinator", "control endpoint"));
    }

    // `pool-b` is the boot-staged subject: the root bound its entitlement
    // before publication, so it is already live. Every other role is a real
    // dynamic spawn adjudicated against the same policy.
    for role in [Role::Guaranteed, Role::PoolA] {
        spawn_subject(&mut subjects, role);
    }
    for role in [Role::Guaranteed, Role::PoolA, Role::PoolB] {
        subjects[role.slot()].base = query_base(&subjects, role);
    }

    // A second live incarnation of a bound subject must not acquire the
    // entitlement again. Attempted while the first is demonstrably holding its
    // window, so a refusal cannot be confused with the subject being absent.
    let executable = Role::Guaranteed
        .executable()
        .and_then(|name| slime_rt::resolve_binding(name).ok())
        .unwrap_or_else(|| fail("coordinator", "duplicate executable"));
    if slime_rt::spawn(executable, &[]).is_ok() {
        fail(
            "coordinator",
            "a second incarnation bound a live entitlement",
        );
    }
    trace(format_args!(
        "[private-adaptive] duplicate subject={} refused=1 live_base=0x{:x}",
        Role::Guaranteed.instance(),
        subjects[Role::Guaranteed.slot()].base,
    ));

    let mut served = 0usize;
    let mut refused = 0usize;
    let mut digest = 0xcbf2_9ce4_8422_2325u64;
    for (number, entry) in schedule().into_iter().enumerate() {
        let role = entry.role;
        let before = subjects[role.slot()].pages;
        let after = request(&subjects, role, entry.delta);
        if after != before && after != before + entry.delta {
            fail(
                "coordinator",
                "a subject reported an extent it was not offered",
            );
        }
        let granted = after != before;
        subjects[role.slot()].pages = after;
        if granted {
            served += 1;
        } else {
            refused += 1;
        }
        for value in [
            number as u64,
            role.ordinal().into(),
            entry.delta as u64,
            granted.into(),
            after as u64,
            subjects[role.slot()].base,
        ] {
            digest = mix(digest, value);
        }
        trace(format_args!(
            "[private-adaptive] step={} subject={} delta={} served={} pages={} base=0x{:x}",
            number,
            role.instance(),
            entry.delta,
            u8::from(granted),
            after,
            subjects[role.slot()].base,
        ));
    }
    trace(format_args!(
        "[private-adaptive] schedule steps={STEPS} served={served} refused={refused} digest=0x{digest:016x}"
    ));

    restart_phase(&mut subjects);
    denied_phase(&mut subjects);

    // Every surviving subject still owns the same window and the same bytes
    // after a peer faulted, a peer exited, and three requests were refused.
    for role in [Role::Guaranteed, Role::PoolA, Role::PoolB] {
        if query_base(&subjects, role) != subjects[role.slot()].base {
            fail("coordinator", "a surviving subject's window base moved");
        }
    }
    trace(format_args!(
        "[private-adaptive] stable subjects=3 guaranteed_pages={} pool_a_pages={} pool_b_pages={}",
        subjects[Role::Guaranteed.slot()].pages,
        subjects[Role::PoolA.slot()].pages,
        subjects[Role::PoolB.slot()].pages,
    ));

    for role in [Role::Guaranteed, Role::PoolA, Role::PoolB] {
        finish(&mut subjects, role);
    }
    trace(format_args!(
        "[private-adaptive] complete steps={STEPS} served={served} refused={refused} restarts=1 duplicates=1 denied=1 exits=5 digest=0x{digest:016x}"
    ));
    slime_rt::exit(0)
}

/// FNV-1a over the schedule's observations. The digest exists so one line
/// carries the whole grant/refusal sequence, including each window base, for a
/// cross-boot comparison; the gate still compares the individual step lines.
fn mix(state: u64, value: u64) -> u64 {
    let mut state = state;
    for byte in value.to_le_bytes() {
        state ^= u64::from(byte);
        state = state.wrapping_mul(0x0000_0100_0000_01b3);
    }
    state
}

fn spawn_subject(subjects: &mut Subjects, role: Role) {
    let executable = role
        .executable()
        .and_then(|name| slime_rt::resolve_binding(name).ok())
        .unwrap_or_else(|| fail("coordinator", "spawn authority"));
    let spawned =
        slime_rt::spawn(executable, &[]).unwrap_or_else(|_| fail("coordinator", "spawn refused"));
    subjects[role.slot()].handle = Some(spawned.supervision_slot);
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

/// Ask `role` for `delta` more pages; answer the extent it holds afterwards.
fn request(subjects: &Subjects, role: Role, delta: usize) -> usize {
    let pages = call(subjects, role, protocol::GROW, delta as u64);
    usize::try_from(pages).unwrap_or_else(|_| fail("coordinator", "extent reply"))
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
    if let Some(handle) = subjects[role.slot()].handle {
        await_end(handle, false);
    }
}

/// The restartable holder: bind, hold its guarantee, die by fault, and be
/// replaced. The replacement must bind the *same* entitlement — not a second
/// copy of it — and must be served zeroed backing rather than the dead
/// incarnation's bytes.
fn restart_phase(subjects: &mut Subjects) {
    spawn_subject(subjects, Role::Restart);
    let base = query_base(subjects, Role::Restart);
    subjects[Role::Restart.slot()].base = base;
    let pages = request(subjects, Role::Restart, RESTART_GUARANTEE);
    if pages != RESTART_GUARANTEE {
        fail("coordinator", "a declared guarantee was not served in full");
    }
    subjects[Role::Restart.slot()].pages = pages;
    trace(format_args!(
        "[private-adaptive] restart subject={} incarnation=0 base=0x{:x} pages={} end=fault",
        Role::Restart.instance(),
        base,
        pages,
    ));
    call(subjects, Role::Restart, protocol::FAULT, 0);
    let handle = subjects[Role::Restart.slot()]
        .handle
        .unwrap_or_else(|| fail("coordinator", "restart handle"));
    await_end(handle, true);
    // Collecting a termination *is* the reclamation: the root drops the
    // caller's slot as it hands the outcome over, so an explicit drop here
    // would be a double free. The inverse is the assertion worth making, and
    // it is strictly stronger: a collected handle must refuse a second query,
    // which a leaked one would answer.
    //
    // Made immediately, before the replacement is spawned. Slot allocation
    // returns the lowest free slot, so the next spawn reuses this number; a
    // later re-query would read the replacement's live handle and pass for
    // the wrong reason.
    if slime_rt::supervision_status(handle).is_ok() {
        fail(
            "coordinator",
            "a collected supervision handle answered twice",
        );
    }
    subjects[Role::Restart.slot()].handle = None;

    subjects[Role::Restart.slot()].incarnation = 1;
    subjects[Role::Restart.slot()].pages = 0;
    spawn_subject(subjects, Role::Restart);
    let replacement = query_base(subjects, Role::Restart);
    let pages = request(subjects, Role::Restart, RESTART_GUARANTEE);
    if pages != RESTART_GUARANTEE {
        fail(
            "coordinator",
            "a replacement did not receive its entitlement",
        );
    }
    subjects[Role::Restart.slot()].pages = pages;
    trace(format_args!(
        "[private-adaptive] restart subject={} incarnation=1 base=0x{:x} pages={} same_base={} end=exit",
        Role::Restart.instance(),
        replacement,
        pages,
        u8::from(replacement == base),
    ));
    finish(subjects, Role::Restart);
}

/// The omitted subject: a declared instance the policy never names. It must be
/// refused its first page, keep running, and end by its own clean exit.
fn denied_phase(subjects: &mut Subjects) {
    spawn_subject(subjects, Role::Denied);
    let pages = request(subjects, Role::Denied, 1);
    if pages != 0 {
        fail("coordinator", "an omitted subject was served a page");
    }
    trace(format_args!(
        "[private-adaptive] denied subject={} pages=0 base=0x0 refused=1 end=exit",
        Role::Denied.instance(),
    ));
    finish(subjects, Role::Denied);
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
    if initial.pages != 0 {
        fail(label, "a fresh incarnation arrived already backed");
    }
    // A bound subject holds a window before it holds a page; an omitted
    // instance holds neither. Both are measured, never assumed.
    if (base == 0) != (role == Role::Denied) {
        fail(label, "window presence disagrees with the declared policy");
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
                    .unwrap_or_else(|_| fail(label, "growth delta"));
                pages = grow(role, base, pages, delta, &mut refused, request.incarnation);
                pages as u64
            }
            protocol::VERIFY | protocol::FAULT | protocol::FINISH => {
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
                "[private-adaptive:{label}] end incarnation={} pages={pages} refused={refused} kind=exit",
                request.incarnation,
            ));
            slime_rt::exit(0);
        }
        if request.operation == protocol::FAULT {
            trace(format_args!(
                "[private-adaptive:{label}] end incarnation={} pages={pages} refused={refused} kind=fault",
                request.incarnation,
            ));
            // The first page past the committed extent. Under an adaptive
            // policy that address is still inside the declared address
            // maximum, so this is the unbacked-but-permitted case rather than
            // the beyond-the-window one: growth permission is not a mapping.
            fault_at(base + pages * PAGE_BYTES);
            fail(label, "the unbacked extent did not fault");
        }
    }
}

/// Request `delta` more pages and answer the extent held afterwards.
///
/// A refusal must be free: the extent, base, and every earlier stamp are
/// re-measured after one, because a root that charged a refused request would
/// otherwise be visible only as a number the gate has no independent copy of.
fn grow(
    role: Role,
    base: usize,
    pages: usize,
    delta: usize,
    refused: &mut usize,
    incarnation: u32,
) -> usize {
    let label = role.label();
    if delta == 0 {
        fail(label, "empty growth request");
    }
    let Ok(previous) = slime_rt::private_memory_grow(delta) else {
        *refused += 1;
        let current =
            slime_rt::private_memory_grow(0).unwrap_or_else(|_| fail(label, "extent query"));
        if current.base != base || current.pages != pages {
            fail(label, "a refused request changed the extent");
        }
        sweep(role, base, 0, pages, incarnation, false);
        trace(format_args!(
            "[private-adaptive:{label}] request incarnation={incarnation} delta={delta} served=0 pages={pages} base=0x{base:x} zeroed=0 stable=1"
        ));
        return pages;
    };
    if previous.base != base || previous.pages != pages {
        fail(label, "growth disagreed with the measured extent");
    }
    let grown = pages + delta;
    if grown > role.ceiling() {
        fail(label, "a growth exceeded the declared maximum");
    }
    let current = slime_rt::private_memory_grow(0).unwrap_or_else(|_| fail(label, "extent query"));
    if current.base != base || current.pages != grown {
        fail(label, "the extent after growth disagrees with the request");
    }
    // Newly served pages must read as zero before anything writes them, then
    // carry this incarnation's stamp; earlier pages must still carry theirs.
    sweep(role, base, pages, grown, incarnation, true);
    sweep(role, base, 0, pages, incarnation, false);
    trace(format_args!(
        "[private-adaptive:{label}] request incarnation={incarnation} delta={delta} served=1 pages={grown} base=0x{base:x} zeroed=1 stable=1"
    ));
    grown
}

fn verify(role: Role, base: usize, pages: usize, incarnation: u32, refused: usize) {
    let label = role.label();
    let current =
        slime_rt::private_memory_grow(0).unwrap_or_else(|_| fail(label, "extent query refused"));
    if current.base != base || current.pages != pages {
        fail(label, "the extent moved between commands");
    }
    sweep(role, base, 0, pages, incarnation, false);
    trace(format_args!(
        "[private-adaptive:{label}] verified incarnation={incarnation} base=0x{base:x} pages={pages} pattern=1 refused={refused}"
    ));
}

/// The value this role and incarnation stamps. Never zero, and distinct per
/// `(role, incarnation)`, so a page carried across two lives names where it
/// came from rather than merely being non-zero.
fn stamp(role: Role, incarnation: u32) -> u64 {
    0x4144_4150_0000_0000u64
        | (u64::from(role.ordinal() + 1) << 24)
        | ((u64::from(incarnation) + 1) << 8)
}

/// Read, stamp, and re-read the pages in `[start, end)`.
///
/// Two words per page — the first and the last — rather than every word. The
/// fixed 64 MiB and 1 GiB planes already sweep every word of their regions and
/// own that claim; this plane's subject is adjudication across many requests,
/// and a full sweep of every extent after each of eighteen steps would cost
/// more emulated time than the added evidence is worth. Both ends of each page
/// are read so a mapping covering only a page's prefix still fails, and the
/// stamp mixes the word's own index so an aliased page cannot satisfy its own
/// read-back.
fn sweep(role: Role, base: usize, start: usize, end: usize, incarnation: u32, initialize: bool) {
    let label = role.label();
    let stamp = stamp(role, incarnation);
    let pointer = base as *mut u64;
    let ends = |page: usize| {
        [
            page * WORDS_PER_PAGE,
            page * WORDS_PER_PAGE + WORDS_PER_PAGE - 1,
        ]
    };
    if initialize {
        for page in start..end {
            for word in ends(page) {
                // SAFETY: successful growth mapped every page below `end`
                // read-write at `base`, so this index is inside the region.
                if unsafe { pointer.add(word).read_volatile() } != 0 {
                    fail(label, "a newly served word was not zero");
                }
            }
        }
        for page in start..end {
            for word in ends(page) {
                // SAFETY: as above; the same admitted, mapped, writable region.
                unsafe { pointer.add(word).write_volatile(stamp ^ word as u64) }
            }
        }
    }
    for page in start..end {
        for word in ends(page) {
            // SAFETY: as above; the holder retains the region until it ends.
            if unsafe { pointer.add(word).read_volatile() } != stamp ^ word as u64 {
                fail(label, "a stamped word did not read back");
            }
        }
    }
}

/// Store to `address` through inline assembly, to fault deliberately.
///
/// Inline assembly rather than a volatile write through a pointer known to be
/// unmapped: a volatile access still requires a valid pointer, so the Rust
/// store would be undefined behaviour and an optimizer change could delete the
/// instruction the deliberate-fault half of this plane depends on.
fn fault_at(address: usize) {
    #[cfg(target_arch = "aarch64")]
    // SAFETY: one store of a general-purpose register to an address this task's
    // VSpace does not back. Faulting is the intent, and `nostack` holds because
    // the block touches no stack slot.
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
/// root timer or reclamation diagnostics.
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
        slime_rt::debug_write(b"[private-adaptive] FAIL marker bounds\n");
        slime_rt::exit(1);
    }
    line.bytes[line.used] = b'\n';
    slime_rt::debug_write(&line.bytes[..line.used + 1]);
}
