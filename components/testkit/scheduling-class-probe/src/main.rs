#![no_std]
#![no_main]

//! C9.3's plane: a declared scheduling class, and the promotion authority over it.
//!
//! Four instances of this binary, told apart by the class the generation declares
//! for each and by the grants it holds — the same shape `clock-authority-probe`
//! and `wait-set-probe` use, and for the same reason: what a component may do is
//! authenticated generation data, so the role is discovered rather than compiled
//! in.
//!
//! - `sched-burner` is `bestEffort`. It burns CPU in a bounded, non-yielding
//!   loop, emitting a progress marker per chunk. This is the saturating workload
//!   the milestone's first required check names.
//! - `sched-foreground` is `foreground`. It arms a short C9.1 timer, blocks on
//!   its declared Notification, and emits a progress step per expiry.
//!
//!   Blocking is what makes the observation possible at all, and the reason is
//!   worth stating because the obvious design does not work. Under strict
//!   priority on one vCPU, a `foreground` component that merely spins or yields
//!   runs to completion before a `bestEffort` component is scheduled even once —
//!   so its markers would all precede the burner's and would prove only the
//!   launch order. By blocking on a timer the foreground *hands the CPU to the
//!   burner*, and each expiry then preempts a loop that is demonstrably running,
//!   because the burner's own chunk markers bracket the wake. The evidence is
//!   therefore interleaving, which a priority-ignoring scheduler cannot produce:
//!   it would let the burner's 200M-iteration loop finish first.
//! - `sched-controller` is `normal` and holds promotion authority over the child
//!   it spawns. It proves three things in order: a declared promotion applies and
//!   is visible in the subject's own band; a request above the edge's declared
//!   ceiling is refused without changing anything; and the operation cannot be
//!   pointed at the caller itself, because the subject comes from a capability
//!   and this component holds no capability naming itself.
//! - `sched-denied` is named by no class entry and holds no promotion authority.
//!   It reads its class back as `undeclared` at the root's own child priority —
//!   an answer rather than a refusal, because every thread runs at some
//!   priority, but deliberately not a *band*, since the generation placed it in
//!   none. It also proves `undeclared` is unassignable: asking to be promoted to
//!   it is refused.

use boot_contracts::scheduling_class::{
    CLASS_BEST_EFFORT, CLASS_FOREGROUND, CLASS_NORMAL, UNDECLARED_CLASS_ID,
};
use slime_rt::{
    SchedulingClassInfo, debug_write, exit, notification_wait, resolve_binding,
    scheduling_class_promote, scheduling_class_read, spawn, timer_arm,
};

/// Progress steps the foreground instance emits, one per timer expiry.
///
/// Several rather than one: a single wake could land in the gap before the
/// burner was ever scheduled. A sequence that keeps completing while a lower
/// band is runnable is the property under test.
const FOREGROUND_STEPS: u32 = 6;

/// Ticks the foreground sleeps between steps.
///
/// Long enough that the burner is certain to be scheduled and to emit at least
/// one chunk marker while this instance is blocked; short enough that six of
/// them finish well inside the plane's watchdog.
const FOREGROUND_DELAY_TICKS: u64 = 2_000_000;

/// Iterations the `bestEffort` instance spins for, per chunk.
const BURN_CHUNK_ITERATIONS: u64 = 20_000_000;

/// Chunks the burner runs. The product is the same 200M-iteration bound
/// `sample-worker` uses for the B48 observation this generalizes from two
/// threads of one component to two components.
const BURN_CHUNKS: u32 = 10;

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    // Which role this instance is, read from the class the generation declared
    // for it plus whether it holds the executable grant a controller needs.
    let class = match scheduling_class_read() {
        Ok(class) => class,
        Err(error) => fail_with(b"class read", error),
    };
    report(b"class", class);
    match class.class_id {
        CLASS_BEST_EFFORT => run_burner(),
        CLASS_FOREGROUND => run_foreground(),
        // The controller is the one `normal` instance, and it is the one holding
        // the executable grant for the child it promotes. Resolved by the
        // *grant's* own name because `executable:` is the bootstrap instance's
        // layout view and this component is not it (CP2/B70).
        CLASS_NORMAL => match resolve_binding(b"sched-promotable-executable") {
            Ok(slot) => run_controller(slot),
            Err(error) => fail_with(b"resolve promotable child executable", error),
        },
        // The instance the policy names no class for. It reads `undeclared`
        // rather than `normal`: the generation placed it in no band, so naming
        // one would report a priority it is not running at.
        UNDECLARED_CLASS_ID => run_denied(class),
        _ => fail(b"instance resolved to no declared class"),
    }
}

/// Burn the CPU at the lowest declared band without ever yielding.
///
/// Chunked so the transcript records that the loop was *still running* across
/// the foreground's wakes. One marker at each end would leave "was it preempted
/// or merely scheduled after?" unanswerable from the transcript.
fn run_burner() -> ! {
    debug_write(b"[sched-burner] bestEffort spinning\n");
    let mut sink = 0u64;
    for chunk in 0..BURN_CHUNKS {
        // No `yield_now` anywhere in here. A yield would hand the CPU over
        // voluntarily and prove nothing about preemption.
        for step in 0..BURN_CHUNK_ITERATIONS {
            // Opaque enough that the optimizer cannot fold the loop away; a loop
            // compiled to nothing would "pass" while testing nothing.
            sink = sink.wrapping_add(step).rotate_left(1);
            core::hint::spin_loop();
        }
        write_value(b"[sched-burner] chunk=", chunk as u64);
    }
    if sink == u64::MAX {
        debug_write(b"[sched-burner] spin sink saturated\n");
    }
    debug_write(b"[sched-burner] bestEffort complete\n");
    exit(0)
}

/// Make ordered progress at the highest declared band, across the burner.
///
/// Each step blocks on a C9.1 timer rather than spinning or yielding. Blocking
/// is what lets the lower band run at all under strict priority, so each expiry
/// preempts a loop the transcript shows to be in flight — see the module
/// comment for why the spinning alternative proves nothing.
fn run_foreground() -> ! {
    let wake = match resolve_binding(b"notification:sched-foreground-tick+wait") {
        Ok(slot) => slot,
        Err(error) => fail_with(b"resolve foreground tick notification", error),
    };
    for step in 0..FOREGROUND_STEPS {
        if let Err(error) = timer_arm(FOREGROUND_DELAY_TICKS) {
            fail_with(b"arm foreground tick", error)
        }
        // Blocks until the root signals the declared badge on expiry. This is
        // the handover: while parked, the only runnable component is the burner.
        let badges = match notification_wait(wake) {
            Ok(badges) => badges,
            Err(error) => fail_with(b"block on foreground tick", error),
        };
        if badges == 0 {
            fail(b"foreground woke on no badge")
        }
        write_value(b"[sched-foreground] progress step=", step as u64);
    }
    debug_write(b"[sched-foreground] foreground complete\n");
    exit(0)
}

/// Exercise the promotion authority the generation declared.
///
/// `executable` is the slot the grant name resolved to, so this component knows
/// no generation's numbering (CP2/B70).
fn run_controller(executable: u32) -> ! {
    let child = match spawn(executable, &[]) {
        Ok(child) => child,
        Err(error) => fail_with(b"promotable child spawn", error),
    };
    write_value(
        b"[sched-controller] spawned subject handle=",
        child.supervision_slot as u64,
    );

    // 1. A declared promotion applies. The request names a *class*; the priority
    //    comes from the generation's own band mapping, so this reply also
    //    reports which band that class resolved to.
    match scheduling_class_promote(child.supervision_slot, CLASS_NORMAL) {
        Ok(promoted) => {
            if promoted.class_id != CLASS_NORMAL {
                fail(b"promotion reported a class other than the one requested")
            }
            report(b"promoted", promoted);
        }
        Err(error) => fail_with(b"declared promotion", error),
    }

    // 2. Above the edge's declared ceiling is refused. The fixture's ceiling is
    //    `normal`, so `foreground` is exactly one band too high — the case that
    //    distinguishes a ceiling that is enforced from one that is merely
    //    written down.
    match scheduling_class_promote(child.supervision_slot, CLASS_FOREGROUND) {
        Ok(_) => fail(b"promotion above the declared ceiling was admitted"),
        Err(error) => write_value(
            b"[sched-controller] above ceiling refused error=",
            neg(error),
        ),
    }

    // 2b. `undeclared` is a name the read side answers with, not a band a
    //     manifest may assign or a promotion may request. Checked from here
    //     rather than from the denied instance because this component holds a
    //     real supervision capability over its subject: the slot resolves, so
    //     the refusal is genuinely about the requested class rather than about
    //     authority the caller lacks.
    match scheduling_class_promote(child.supervision_slot, UNDECLARED_CLASS_ID) {
        Ok(_) => fail(b"undeclared was admitted as a promotion target"),
        Err(error) => write_value(
            b"[sched-controller] undeclared target refused error=",
            neg(error),
        ),
    }

    // 3. The operation cannot be pointed at the caller. `slot` names a subject
    //    through a *capability*, and no capability this component holds names
    //    itself — the root mints a supervision handle only for a task's spawner,
    //    never for the task itself. So there is no request shape that widens the
    //    caller's own class, and the closest a component can come is naming a
    //    slot that is not a supervision capability at all.
    match scheduling_class_promote(SELF_TCB_SLOT, CLASS_FOREGROUND) {
        Ok(_) => fail(b"self-directed promotion was admitted"),
        Err(error) => write_value(
            b"[sched-controller] self promotion refused error=",
            neg(error),
        ),
    }

    // 4. The controller's own class is unchanged by everything above. Holding
    //    promotion authority over another component is not authority over
    //    yourself, and this is the observation of that.
    let mine = match scheduling_class_read() {
        Ok(class) => class,
        Err(error) => fail_with(b"controller class reread", error),
    };
    if mine.class_id != CLASS_NORMAL {
        fail(b"controller's own class changed while promoting a peer")
    }
    report(b"unchanged", mine);
    debug_write(b"[sched-controller] controller complete\n");
    exit(0)
}

/// The deny-by-default answer: an answer, not a refusal.
///
/// `class` is the reading `main` already took, passed in so this asserts on the
/// same observation the role was selected from rather than re-reading and
/// possibly agreeing with itself.
fn run_denied(class: SchedulingClassInfo) -> ! {
    if class.class_id != UNDECLARED_CLASS_ID {
        fail(b"an instance the policy does not name did not read as undeclared")
    }
    // Reported rather than asserted against a literal: this component cannot
    // know the root's child default, and the gate cross-checks the number
    // against the `ScheduleRecord` the builder wrote for this same thread.
    write_value(b"[sched-denied] undeclared priority=", class.priority);
    // Exactly `DENIED_SLOT_SWEEP` refusals follow, and nothing else this
    // instance does produces one — the gate compares the count exactly. Proving
    // `undeclared` is unassignable belongs to the *controller*, which holds a
    // real supervision capability: from here the slot lookup would fail first
    // and the refusal would say nothing about the requested class.
    // It holds no promotion authority, so every slot it could name is refused.
    // Checked over the slots it actually has rather than one chosen slot: "this
    // component cannot promote anything" is the claim.
    for slot in 0..DENIED_SLOT_SWEEP {
        if scheduling_class_promote(slot, CLASS_FOREGROUND).is_ok() {
            fail(b"an instance holding no promotion authority promoted a peer")
        }
    }
    write_value(
        b"[sched-denied] undeclared class id=",
        class.class_id as u64,
    );
    debug_write(b"[sched-denied] no promotion authority\n");
    exit(0)
}

/// This component's own TCB slot, per `slime-root`'s `ChildSlots`. Named here so
/// the self-directed attempt above points at a real slot the component holds
/// rather than at an empty one, which would prove only that empty slots fail.
const SELF_TCB_SLOT: u32 = 2;

/// Slots the denied instance sweeps when proving it can promote nothing.
const DENIED_SLOT_SWEEP: u32 = 8;

/// Negate a syscall error so it prints as an unsigned magnitude.
fn neg(error: i64) -> u64 {
    error.unsigned_abs()
}

fn report(tag: &[u8], class: SchedulingClassInfo) {
    debug_write(b"[sched] ");
    debug_write(tag);
    write_value(b" class=", class.class_id as u64);
    write_value(b"[sched] priority=", class.priority);
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
    debug_write(b"[sched] FAIL ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}

fn fail_with(reason: &[u8], error: i64) -> ! {
    debug_write(b"[sched] FAIL ");
    debug_write(reason);
    write_value(b" error=", neg(error));
    exit(1)
}
