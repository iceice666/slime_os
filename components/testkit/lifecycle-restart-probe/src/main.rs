#![no_std]
#![no_main]

//! C9.4's plane: lifecycle transitions, supervised restart, and parameter authority.
//!
//! Five instances of this binary, told apart by the authority the generation
//! grants each — the same shape `scheduling-class-probe`, `wait-set-probe`, and
//! `clock-authority-probe` use, and for the same reason: what a component may do
//! is authenticated generation data, so the role is discovered rather than
//! compiled in.
//!
//! - `lifecycle-supervisor` holds the restart and parameter authority. It is the
//!   userspace supervisor C9.4 requires, and the root gains none of its policy:
//!   the supervisor observes each death, asks the root what the generation
//!   admits, waits the declared backoff on a real C9.1 timer, and spawns the
//!   replacement itself. It drives all three of the milestone's restart causes in
//!   order — fault, clean exit, and declared unhealthiness — then keeps restarting
//!   until the declared attempt bound is spent, and observes that exhaustion is
//!   terminal rather than merely unproductive.
//!
//!   It also proves the two staleness claims. Between the first death and the
//!   replacement's launch it re-invokes the *predecessor's* supervision handle
//!   through an operation needing a live subject, and the refusal is the evidence
//!   that a stale handle cannot reach the replacement — `TaskId`s never alias, so
//!   there is no request shape that redirects. And it writes a parameter before
//!   the restart and reads it after, which is the observation that parameter state
//!   belongs to the *declaration* rather than to a task.
//!
//! - `lifecycle-worker` is the restarted subject. It reads its own state, and its
//!   `predecessor_cause` is how it selects what to do: a first launch faults, the
//!   replacement after a fault exits cleanly, the one after that declares itself
//!   unhealthy, and every later one exits cleanly so the attempt bound is what
//!   ends the sequence rather than the script running out. Each incarnation also
//!   walks the declared transition graph and reports the parameter its supervisor
//!   left, so "a restarted component holds fresh authority and its original
//!   configuration" is one transcript rather than an inference.
//!
//! - `lifecycle-graph` proves the transition graph is enforced rather than
//!   documented: it walks every declared edge in order, then asks for one the
//!   generation does not declare and is refused without moving. Its declared
//!   parameter edge is write-only, which is how it is told apart from the denied
//!   instance below — read and write are separate authorities, and the probe
//!   observes that separation rather than assuming it.
//!
//! - `lifecycle-denied` is named by no restart policy and holds no parameter
//!   edge. It reads its state back — an answer rather than a refusal, because
//!   being in the graph is a property of being declared — and then proves it can
//!   reach nothing: no slot it holds admits a restart, and its own parameters are
//!   unreachable because the generation declares it no reflexive edge.

use boot_contracts::lifecycle_policy::{
    CAUSE_EXIT, CAUSE_FAULT, CAUSE_UNHEALTHY, STATE_ERROR, STATE_INITIALIZE, STATE_READY,
    STATE_RUNNING, STATE_STOP, UNDECLARED_CAUSE_ID,
};
use slime_rt::{
    LifecycleStateInfo, PARAMETER_SELF_SLOT, debug_write, exit, lifecycle_parameter_read,
    lifecycle_parameter_write, lifecycle_restart_admit, lifecycle_state_advance,
    lifecycle_state_read, monotonic_read, resolve_binding, spawn, supervision_status, timer_arm,
    unhealthy,
};

/// The parameter key the supervisor writes and every worker incarnation reads.
///
/// One key, because the claim is that a *value* survives a restart, and a second
/// key would only repeat it.
const CONFIG_KEY: u64 = 7;

/// The value the supervisor writes before the first restart.
const CONFIG_VALUE: u64 = 4242;

/// A key nothing ever writes, so a read of it answers "no value" rather than
/// "no authority" — the two refusals C9.4 requires to be distinguishable.
const ABSENT_KEY: u64 = 9;

/// Counter ticks the supervisor blocks for between status polls.
///
/// Short relative to the declared backoff, so it never dominates the delay the
/// gate measures, and long enough that a lower-band worker reaches its own exit.
const POLL_DELAY_TICKS: u64 = 100_000;

/// Logical authority slots the denied instance sweeps when proving it can
/// restart nothing.
///
/// The whole space its declared bindings could occupy, not one chosen slot: the
/// claim is that *this component* cannot restart anything, and a single slot
/// would leave the rest of its table unexamined. These index the root-side
/// `AuthorityTable` that `RESTART_ADMIT`'s operand names, which is a different
/// numbering from the child CSpace's own CPtrs — a distinction an earlier
/// revision conflated by also naming its TCB CPtr here, which both double-counted
/// a swept slot and claimed to exercise a capability the operand cannot reach
/// (found by review).
const DENIED_SLOT_SWEEP: u32 = 8;

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    let state = match lifecycle_state_read() {
        Ok(state) => state,
        Err(error) => fail_with(b"state read", error),
    };
    // The supervisor is the one instance holding the worker's executable grant,
    // resolved by the *grant's* own name because `executable:` is the bootstrap
    // instance's layout view and this component is not it (CP2/B70).
    if let Ok(executable) = resolve_binding(b"lifecycle-worker-executable") {
        run_supervisor(executable, state);
    }
    // Everything else is told apart by the authority the generation grants it. A
    // worker is the instance its own restart policy names, which it learns by
    // asking whether the root admits attempts for it — including after its
    // budget is spent, which is why a recorded predecessor cause also counts.
    if state.attempts_remaining > 0 || state.predecessor_cause != UNDECLARED_CAUSE_ID {
        run_worker(state);
    }
    // The graph walker holds a write-only reflexive edge and the denied instance
    // holds none, so the probe that separates them is which direction resolves.
    // Read is tried first because it changes nothing.
    if lifecycle_parameter_read(PARAMETER_SELF_SLOT, CONFIG_KEY).is_err()
        && lifecycle_parameter_write(PARAMETER_SELF_SLOT, CONFIG_KEY, 1).is_ok()
    {
        run_graph_walker(state);
    }
    run_denied(state)
}

/// Restart a failing worker under declared policy, and prove the bound is
/// terminal.
///
/// Every decision here is this component's: the root answers what the generation
/// declares and refuses what it does not, and it never restarts anything itself.
fn run_supervisor(executable: u32, state: LifecycleStateInfo) -> ! {
    report(b"[lifecycle-supervisor] state", state);
    let mut attempt = 0u32;
    // The supervisor's own parameter, written through its declared reflexive
    // edge. Reported rather than asserted: what matters below is the *worker's*
    // value surviving a restart, and this write proves the reflexive edge is
    // reachable at all.
    match lifecycle_parameter_write(PARAMETER_SELF_SLOT, CONFIG_KEY, CONFIG_VALUE) {
        Ok(previous) => write_value(b"[lifecycle-supervisor] parameter previous=", previous),
        Err(error) => fail_with(b"supervisor parameter write", error),
    }
    // The health dependency, observed as a refusal before it is satisfied. The
    // generation declares that the worker does not start until this supervisor
    // reaches `Running`, and this supervisor is installed in `Initialize` — so
    // the first spawn attempt must be refused. Taken before the advance rather
    // than after, because a check that only ever runs once the condition holds
    // observes nothing: the claim is that a dependent whose dependency is down
    // is *not started*.
    match spawn(executable, &[]) {
        Ok(_) => fail(b"a spawn was admitted while its declared dependency was unsatisfied"),
        Err(error) => write_value(
            b"[lifecycle-supervisor] dependency refused error=",
            neg(error),
        ),
    }
    // Now satisfy it. The edge names `Running`, and this is the declared edge out
    // of the state the root installed.
    advance(STATE_RUNNING);
    let mut handle = match spawn(executable, &[]) {
        Ok(child) => child.supervision_slot,
        Err(error) => fail_with(b"initial worker spawn", error),
    };
    write_value(b"[lifecycle-supervisor] launched handle=", handle as u64);
    // The worker's configuration, written through the handle naming it while it
    // is live. Parameter state belongs to the declared *instance*, so this one
    // write is what every later incarnation reads back — which is the observation
    // that a restarted component keeps its configuration while holding entirely
    // fresh capabilities.
    match lifecycle_parameter_write(handle, CONFIG_KEY, CONFIG_VALUE) {
        Ok(previous) => write_value(
            b"[lifecycle-supervisor] worker parameter previous=",
            previous,
        ),
        Err(error) => fail_with(b"worker parameter write", error),
    }
    loop {
        // A second handle naming the same task, taken *before* the status poll
        // consumes the first. `STATUS` is a consuming operation — that is its
        // contract, not a defect — so a supervisor that needs both the outcome
        // and a subject for `RESTART_ADMIT` must derive one, and the derive
        // carries the source's own rights so no authority is added.
        let subject = match slime_rt::supervision_derive(handle) {
            Ok(derived) => derived,
            Err(error) => fail_with(b"derive restart subject", error),
        };
        // Wait for the worker to end. Polling `STATUS` rather than blocking on a
        // declared wait source keeps this plane's evidence about *restart* rather
        // than about C9.2's dispatch, which has its own gate.
        //
        // Between polls the supervisor *blocks on a C9.1 timer*, and that is
        // load-bearing rather than a nicety now that the worker is declared at a
        // lower band. Under strict priority on one vCPU neither a busy poll nor
        // `yield_now` at 254 ever lets a 150-band child run — a yield
        // re-schedules within the same band — so the supervisor would spin
        // forever waiting for a death its own loop prevented. Blocking hands the
        // CPU down, which is exactly the observation C9.3's plane makes from the
        // foreground side.
        let outcome = loop {
            match supervision_status(handle) {
                Ok(Some(termination)) => break termination,
                Ok(None) => sleep(POLL_DELAY_TICKS),
                Err(error) => fail_with(b"supervision status", error),
            }
        };
        write_value(
            b"[lifecycle-supervisor] observed death attempt=",
            attempt as u64,
        );
        report_termination(outcome);
        // The staleness check. `STATUS` consumed the polled handle, so a second
        // use of that same slot must be refused — the handle named one task
        // lifetime and no request shape redirects it at a successor.
        match supervision_status(handle) {
            Ok(_) => fail(b"a consumed predecessor handle answered a second time"),
            Err(error) => write_value(
                b"[lifecycle-supervisor] stale handle refused error=",
                neg(error),
            ),
        }
        // Ask the root what the generation admits. It charges the declared
        // attempt against the *instance*, answers the declared backoff instant,
        // and refuses a cause the policy does not name.
        let admission = match lifecycle_restart_admit(subject) {
            Ok(admission) => admission,
            Err(error) => {
                // Exhaustion, or a cause the policy does not restart on. Either
                // way the sequence is over, and the terminal claim is the
                // *spawn* being refused too — checked below rather than inferred
                // from this refusal alone.
                write_value(b"[lifecycle-supervisor] restart refused error=", neg(error));
                match spawn(executable, &[]) {
                    Ok(_) => fail(b"a spawn was admitted after the attempt bound was spent"),
                    Err(spawn_error) => write_value(
                        b"[lifecycle-supervisor] terminal spawn refused error=",
                        neg(spawn_error),
                    ),
                }
                debug_write(b"[lifecycle-supervisor] attempts exhausted\n");
                debug_write(b"[lifecycle-supervisor] supervisor complete\n");
                exit(0)
            }
        };
        write_value(
            b"[lifecycle-supervisor] restart admitted remaining=",
            admission.attempts_remaining as u64,
        );
        // The derived handle named the dead task and has served its purpose.
        // Dropped so the table does not fill across the attempt sequence.
        let _ = slime_rt::cap_drop(subject);
        // The backoff, observed as a refusal before it is waited. `RESTART_ADMIT`
        // answered an instant; a supervisor that skips its own wait is refused by
        // the mechanism rather than trusted to honour a number it was merely
        // told, and this is that observation. Attempted only on the first restart
        // so the transcript carries exactly one such refusal, which the gate
        // counts.
        if attempt == 0 {
            match spawn(executable, &[]) {
                Ok(_) => fail(b"a spawn was admitted before the declared backoff elapsed"),
                Err(error) => {
                    write_value(b"[lifecycle-supervisor] backoff refused error=", neg(error))
                }
            }
        }
        // Wait the declared backoff on a real timer against C9.1's clock, not a
        // spin count.
        wait_until(admission.ready_at);
        handle = match spawn(executable, &[]) {
            Ok(child) => child.supervision_slot,
            Err(error) => fail_with(b"restart spawn", error),
        };
        attempt = attempt.saturating_add(1);
        write_value(b"[lifecycle-supervisor] restarted handle=", handle as u64);
    }
}

/// Block on one C9.1 timer expiry, handing the CPU to any lower band.
///
/// The declared wait Notification is this component's only one, so a wake here
/// is the timer's.
///
/// A failed arm fails the plane rather than returning. Both callers re-invoke
/// this from a loop whose condition a failed arm did not change, so returning
/// would degenerate into an unbounded spin at the root's 254 default — which is
/// exactly the condition that starves the 150-band worker and times the gate
/// out, i.e. the failure mode blocking exists to avoid. The generation grants
/// this holder `timerUse` with a quota of two and at most one timer is ever
/// live, so the path is unreachable here; treating it as fatal keeps the comment
/// and the code stating the same property (found by review).
fn sleep(ticks: u64) {
    if let Err(error) = timer_arm(ticks) {
        fail_with(b"timer arm", error)
    }
    let _ = slime_rt::notification_wait(0);
}

/// Block until the monotonic clock reaches `ready_at`.
///
/// Arms a real C9.1 timer per remaining interval rather than spinning, because
/// "backoff is observed against a clock" is the check and a spin would satisfy
/// the wait while proving nothing about the clock. The clock is re-read after
/// each wake rather than the wake being trusted: the root's own refusal is keyed
/// on the counter, not on the signal.
fn wait_until(ready_at: u64) {
    loop {
        let now = match monotonic_read() {
            Ok(now) => now,
            Err(error) => fail_with(b"backoff clock read", error),
        };
        if now >= ready_at {
            write_value(b"[lifecycle-supervisor] backoff elapsed now=", now);
            return;
        }
        sleep(ready_at - now);
    }
}

/// One incarnation of the restarted subject.
///
/// The predecessor's terminal cause selects what this one does, so the three
/// causes C9.4 requires to be distinguishable are driven by the *observation*
/// rather than by a counter the component keeps.
fn run_worker(state: LifecycleStateInfo) -> ! {
    report(b"[lifecycle-worker] state", state);
    // The configuration written before the first restart. Read through the
    // reflexive edge the generation declares for this instance, so a replacement
    // observing it is observing state that outlived its predecessor.
    match lifecycle_parameter_read(PARAMETER_SELF_SLOT, CONFIG_KEY) {
        Ok(value) => write_value(b"[lifecycle-worker] parameter value=", value),
        Err(error) => write_value(b"[lifecycle-worker] parameter absent error=", neg(error)),
    }
    // The other half of C9.4's last required check: a key this instance has
    // *never* been given must answer differently from one it may not ask about.
    // Taken over the same declared edge as the read above, so the only thing
    // that differs between the two answers is whether a value exists — which is
    // what makes "distinguishable" a property of the mechanism rather than of
    // which component happened to ask.
    match lifecycle_parameter_read(PARAMETER_SELF_SLOT, ABSENT_KEY) {
        Ok(_) => fail(b"a key that was never written answered a value"),
        Err(error) => write_value(b"[lifecycle-worker] unset key refused error=", neg(error)),
    }
    // Walk into the running state, which is the edge every incarnation takes and
    // therefore the evidence that a replacement re-derives its state from the
    // generation rather than continuing its predecessor's.
    advance(STATE_RUNNING);
    match state.predecessor_cause {
        // First launch: fault. A real dereference of a null address, so the
        // cause the root records is the kernel's rather than a status word the
        // component chose.
        UNDECLARED_CAUSE_ID => {
            debug_write(b"[lifecycle-worker] faulting\n");
            // SAFETY: deliberately invalid, to produce a real VM fault. This is
            // the plane's fault injection; the root's supervision path is what
            // is under test.
            unsafe {
                core::ptr::null_mut::<u64>().write_volatile(1);
            }
            fail(b"a null write did not fault")
        }
        // After a fault: exit cleanly, so the next incarnation observes the
        // `exit` cause and the two are distinguishable in one transcript.
        CAUSE_FAULT => {
            debug_write(b"[lifecycle-worker] exiting after fault\n");
            advance(STATE_STOP);
            exit(0)
        }
        // After a clean exit: declare unhealthiness, the third cause.
        CAUSE_EXIT => {
            debug_write(b"[lifecycle-worker] declaring unhealthy\n");
            advance(STATE_ERROR);
            unhealthy()
        }
        // After unhealthiness, and for every later incarnation: exit cleanly, so
        // what ends the sequence is the declared attempt bound rather than the
        // script running out of cases.
        CAUSE_UNHEALTHY => {
            debug_write(b"[lifecycle-worker] exiting after unhealthy\n");
            advance(STATE_STOP);
            exit(0)
        }
        _ => {
            debug_write(b"[lifecycle-worker] exiting\n");
            advance(STATE_STOP);
            exit(0)
        }
    }
}
/// Walk every declared edge, then prove an undeclared one is refused.
fn run_graph_walker(state: LifecycleStateInfo) -> ! {
    report(b"[lifecycle-graph] state", state);
    if state.state_id != STATE_INITIALIZE {
        fail(b"an instance did not start in the declared initial state")
    }
    advance(STATE_RUNNING);
    advance(STATE_READY);
    // `Ready -> Initialize` is not declared: a component cannot re-enter the
    // graph's entry state, so this is the edge that distinguishes a graph the
    // root enforces from one it merely carries.
    match lifecycle_state_advance(STATE_INITIALIZE) {
        Ok(_) => fail(b"an undeclared transition was admitted"),
        Err(error) => write_value(
            b"[lifecycle-graph] undeclared edge refused error=",
            neg(error),
        ),
    }
    // And the refused request changed nothing.
    match lifecycle_state_read() {
        Ok(after) if after.state_id == STATE_READY => {
            debug_write(b"[lifecycle-graph] state unchanged after refusal\n");
        }
        Ok(_) => fail(b"a refused transition moved the state anyway"),
        Err(error) => fail_with(b"graph state reread", error),
    }
    advance(STATE_STOP);
    debug_write(b"[lifecycle-graph] graph complete\n");
    exit(0)
}

/// The deny-by-default answers: an answer for state, a refusal for authority.
fn run_denied(state: LifecycleStateInfo) -> ! {
    report(b"[lifecycle-denied] state", state);
    if state.attempts_remaining != 0 {
        fail(b"an instance the policy names no restart policy for reported attempts")
    }
    // No parameter edge at all, reflexive included, so this instance cannot reach
    // even its own configuration. That is what makes parameter state an authority
    // rather than a per-component namespace.
    match lifecycle_parameter_read(PARAMETER_SELF_SLOT, CONFIG_KEY) {
        Ok(_) => fail(b"an instance holding no parameter edge read a parameter"),
        Err(error) => write_value(
            b"[lifecycle-denied] own parameter refused error=",
            neg(error),
        ),
    }
    // And it can restart nothing. Swept over the slots it actually holds rather
    // than one chosen slot: "this component cannot restart anything" is the claim.
    for slot in 0..DENIED_SLOT_SWEEP {
        if lifecycle_restart_admit(slot).is_ok() {
            fail(b"an instance holding no restart authority admitted a restart")
        }
    }
    debug_write(b"[lifecycle-denied] no restart authority\n");
    exit(0)
}

/// Take one declared edge, failing the plane if the generation does not admit it.
fn advance(state_id: u32) {
    match lifecycle_state_advance(state_id) {
        Ok(reached) => {
            if reached != state_id {
                fail(b"an advance reported a state other than the one requested")
            }
            write_value(b"[lifecycle] advanced state=", reached as u64);
        }
        Err(error) => fail_with(b"declared advance", error),
    }
}

/// Negate a syscall error so it prints as an unsigned magnitude.
fn neg(error: i64) -> u64 {
    error.unsigned_abs()
}

fn report(tag: &[u8], state: LifecycleStateInfo) {
    debug_write(tag);
    write_value(b" state=", state.state_id as u64);
    write_value(b"[lifecycle] attempts=", state.attempts_remaining as u64);
    write_value(b"[lifecycle] cause=", state.predecessor_cause as u64);
}
/// Report the terminal cause this supervisor could observe through its handle.
///
/// This is the *supervisor's* view, decoded from the `(kind, detail)` pair
/// `SUPERVISION STATUS` answers — and it is deliberately not the authoritative
/// record. `supervision::Termination::encode` emits only exit and fault, so a
/// component that declares itself unhealthy is seen here as the `exit` that
/// `unhealthy()` performs immediately afterwards, while the root records
/// `unhealthy` and the next incarnation reads that. The distinguishing record is
/// the root's; this line says what the supervisor could observe through its
/// handle, which is a strictly narrower thing (found by review, against an
/// earlier comment claiming the two sides agree).
///
/// The three arms the transport can never produce are matched because
/// `slime_rt::Termination` still declares them — a pre-existing wire-decode
/// surface this milestone deliberately left alone, recorded as a follow-up.
fn report_termination(termination: slime_rt::Termination) {
    let name: &[u8] = match termination {
        slime_rt::Termination::Exit(status) => {
            write_value(b"[lifecycle-supervisor] cause exit status=", status as u64);
            b"exit"
        }
        slime_rt::Termination::Fault(reason) => {
            write_value(b"[lifecycle-supervisor] cause fault reason=", reason);
            b"fault"
        }
        slime_rt::Termination::Timeout => b"timeout",
        slime_rt::Termination::PeerLoss => b"peerLoss",
        slime_rt::Termination::Unhealthy => b"unhealthy",
    };
    debug_write(b"[lifecycle-supervisor] cause=");
    debug_write(name);
    debug_write(b"\n");
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
    debug_write(b"[lifecycle] FAIL ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}

fn fail_with(reason: &[u8], error: i64) -> ! {
    debug_write(b"[lifecycle] FAIL ");
    debug_write(reason);
    write_value(b" error=", neg(error));
    exit(1)
}
