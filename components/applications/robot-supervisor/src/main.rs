#![no_std]
#![no_main]

//! C9.6's userspace supervisor: the controller's restart authority.
//!
//! The root gains none of this component's policy. It observes the controller's
//! death, asks the root what the generation admits, waits the declared backoff
//! on a real C9.1 timer, and spawns the replacement itself — the C9.4 division
//! this milestone composes rather than re-derives.
//!
//! What is new here is *what the restart is a restart of*. C9.4's subject held
//! nothing but its own lifecycle state, so "fresh authority" meant a fresh
//! supervision handle. The controller this component restarts is a live
//! participant on two fabric routes: it holds a stream subscribe role on
//! `telemetry` and a call client role on `parameters`. So the replacement must
//! come up holding *reissued* fabric authority — the root reinstalls both
//! declared control endpoints and every declared notification at spawn, and the
//! replacement re-requests its stream role from the broker — and the graph must
//! resume carrying samples through it. That is C9.6's second required check, and
//! it is a claim about the composition rather than about this component.
//!
//! The controller's configuration is a parameter written once here, before the
//! first death. Every incarnation reads it back, so "the replacement holds fresh
//! capabilities and its original configuration" is one transcript rather than an
//! inference: parameter state belongs to the declared instance, which outlives
//! every task representing it.

use boot_contracts::lifecycle_policy::STATE_RUNNING;
use boot_contracts::scheduling_class::CLASS_NORMAL;
use slime_rt::{
    debug_write, exit, lifecycle_parameter_write, lifecycle_restart_admit, lifecycle_state_advance,
    monotonic_read, notification_signal, resolve_binding, spawn, supervision_status, timer_arm,
};

slime_rt::entry!(main);

/// The parameter key every controller incarnation reads back.
const CONFIG_KEY: u64 = 3;

/// The commanded scale the controller applies to each telemetry sample.
///
/// The controller's command is `sample * SCALE`, so this value is what makes the
/// actuator's applied values a function of *configuration* as well as of input —
/// and therefore what makes surviving a restart observable in the data rather
/// than only in a marker.
const CONFIG_VALUE: u64 = 7;

/// Ticks blocked between status polls.
///
/// Blocking rather than spinning is load-bearing: the controller runs at the
/// same `normal` band as this component and the burner runs below both, so a
/// busy poll here would starve the very task whose death this loop waits for.
///
/// Wider than C9.4's `lifecycle-restart-probe` uses for the same field: that
/// plane never shares its run with a `bestEffort` load. Here every wake-up
/// preempts the burner immediately -- strict priority admits no time-slicing
/// across bands -- so a 500,000ns interval left it re-preempted every fraction
/// of a chunk. Widened past the sensor's own 2,000,000ns tick period so a
/// sensor-tick-sized gap can still land a whole burner chunk without this
/// poll cutting it short first.
const POLL_DELAY_TICKS: u64 = 4_000_000;

/// This component's declared wait binding on `robot-supervisor-tick`.
const TICK_SLOT: u32 = 0;

fn main(_startup_arg: u32) {
    let class = match slime_rt::scheduling_class_read() {
        Ok(class) => class,
        Err(error) => fail_with(b"class read", error),
    };
    if class.class_id != CLASS_NORMAL {
        fail(b"the supervisor is not in the declared normal band")
    }

    // The executable grant the generation gives this component over the
    // controller. Resolved by the grant's own name because `executable:` is the
    // bootstrap instance's layout view and this component is not it (CP2/B70).
    let executable = match resolve_binding(b"robot-controller-executable") {
        Ok(slot) => slot,
        Err(error) => fail_with(b"resolve controller executable", error),
    };

    // The one observation `fabric-call-worker` cannot make for itself: only
    // this component's owner can mint a supervision handle over the
    // controller, and demanding an unproducible one was rejected, so the
    // broker's client slot can only clear on an explicit signal that no
    // replacement will ever come. Resolved once, up front, so its absence
    // fails loudly before any incarnation runs rather than only once the
    // last restart is refused.
    let retired = match resolve_binding(b"notification:robot-controller-retired") {
        Ok(slot) => slot,
        Err(error) => fail_with(b"resolve controller retirement notification", error),
    };
    // The retirement signal alone cannot reach a broker parked in
    // `notification_wait` on its own wake object — a different Notification
    // than this one — so nothing would ever rouse it to re-sweep and observe
    // the retirement. This is that wake, held under the same badge scheme
    // `robot-controller`/`robot-actuator`/`robot-clock` already signal it
    // under.
    let call_wake = match resolve_binding(b"notification:fabric-call-worker-parameters-ready") {
        Ok(slot) => slot,
        Err(error) => fail_with(b"resolve call broker wake", error),
    };

    // The declared health dependency, observed as a refusal before it is
    // satisfied: the generation says the controller does not start until this
    // supervisor reaches `Running`, and the root installs it in `Initialize`.
    // Taken before the advance, because a check that runs only once the
    // condition holds observes nothing.
    match spawn(executable, &[]) {
        Ok(_) => fail(b"the controller started while its declared dependency was unsatisfied"),
        Err(error) => write_value(
            b"[robot-supervisor] dependency refused error=",
            error.unsigned_abs(),
        ),
    }
    if let Err(error) = lifecycle_state_advance(STATE_RUNNING) {
        fail_with(b"advance to Running", error)
    }
    debug_write(b"[robot-supervisor] running\n");

    let mut handle = match spawn(executable, &[]) {
        Ok(child) => child.supervision_slot,
        Err(error) => fail_with(b"initial controller spawn", error),
    };
    debug_write(b"[robot-supervisor] controller launched\n");

    // The controller's configuration, written through the handle naming it while
    // it is live. One write, before any death: every later incarnation reads
    // this same value back.
    match lifecycle_parameter_write(handle, CONFIG_KEY, CONFIG_VALUE) {
        Ok(previous) => write_value(b"[robot-supervisor] parameter previous=", previous),
        Err(error) => fail_with(b"controller parameter write", error),
    }

    let mut attempt = 0u32;
    loop {
        // A second handle naming the same task, derived *before* the status poll
        // consumes the first. `STATUS` is a consuming operation by contract, so
        // a supervisor needing both the outcome and a subject for
        // `RESTART_ADMIT` must derive one; the derive carries the source's own
        // rights, so no authority is added.
        let subject = match slime_rt::supervision_derive(handle) {
            Ok(derived) => derived,
            Err(error) => fail_with(b"derive restart subject", error),
        };
        let outcome = loop {
            match supervision_status(handle) {
                Ok(Some(termination)) => break termination,
                Ok(None) => sleep(POLL_DELAY_TICKS),
                Err(error) => fail_with(b"supervision status", error),
            }
        };
        // The cause as *this component* can observe it through its handle. The
        // authoritative record is the root's, which the replacement reads back
        // from its own lifecycle state — a strictly narrower thing, and the
        // distinction C9.4 recorded rather than glossed.
        report_termination(outcome);

        let admission = match lifecycle_restart_admit(subject) {
            Ok(admission) => admission,
            Err(error) => {
                // Exhaustion, or a cause the policy does not restart on. Either
                // way the restart sequence is over. The graph's own completion
                // is what ends this plane, so this component reports and exits
                // rather than treating the bound as a failure.
                write_value(
                    b"[robot-supervisor] restart refused error=",
                    error.unsigned_abs(),
                );
                let _ = slime_rt::cap_drop(subject);
                // No further incarnation will ever hold the generation-owned
                // call endpoint again, so the call broker's slot for it is
                // safe to retire.
                let _ = notification_signal(retired);
                let _ = notification_signal(call_wake);
                write_value(b"[robot-supervisor] restarts total=", attempt as u64);
                debug_write(b"[robot-supervisor] supervision complete\n");
                exit(0)
            }
        };
        write_value(
            b"[robot-supervisor] restart admitted remaining=",
            admission.attempts_remaining as u64,
        );
        // The derived handle named the dead task and has served its purpose.
        let _ = slime_rt::cap_drop(subject);
        // Wait the declared backoff against the real clock, not a spin count.
        wait_until(admission.ready_at);
        handle = match spawn(executable, &[]) {
            Ok(child) => child.supervision_slot,
            Err(error) => fail_with(b"restart spawn", error),
        };
        attempt = attempt.saturating_add(1);
        // The replacement's fabric authority is reissued by the root at spawn:
        // both declared control endpoints and every declared notification are
        // reinstalled into the new task's own CSpace. This marker is the
        // supervisor's half of that claim; the controller's own re-provisioning
        // marker is the other half, and the gate requires both.
        write_value(
            b"[robot-supervisor] controller restarted attempt=",
            attempt as u64,
        );
    }
}

/// Block on one C9.1 timer expiry, handing the CPU to any lower band.
///
/// A failed arm is fatal rather than a return: both callers re-invoke this from
/// a loop whose condition a failed arm did not change, so returning would
/// degenerate into an unbounded spin at this component's band — which is
/// precisely the starvation blocking exists to avoid. The generation grants this
/// holder `timerUse` with a quota of two and at most one timer is ever live, so
/// the path is unreachable.
fn sleep(ticks: u64) {
    if let Err(error) = timer_arm(ticks) {
        fail_with(b"timer arm", error)
    }
    let _ = slime_rt::notification_wait(TICK_SLOT);
}

/// Block until the monotonic clock reaches `ready_at`.
///
/// The clock is re-read after each wake rather than the wake being trusted: the
/// root's own early-spawn refusal is keyed on the counter, not on the signal.
fn wait_until(ready_at: u64) {
    loop {
        let now = match monotonic_read() {
            Ok(now) => now,
            Err(error) => fail_with(b"backoff clock read", error),
        };
        if now >= ready_at {
            write_value(b"[robot-supervisor] backoff elapsed now=", now);
            return;
        }
        sleep(ready_at - now);
    }
}

/// Report the terminal cause this supervisor could observe through its handle.
///
/// `Timeout` and `PeerLoss` are matched because `slime_rt::Termination` still
/// declares them — a pre-existing wire-decode surface B76 left in place after
/// removing every mechanism that could produce one. Neither is reachable from
/// this root, so reaching either arm is a decode defect rather than a cause the
/// supervisor should act on, and it fails rather than restarting on a cause the
/// generation could never have declared.
fn report_termination(termination: slime_rt::Termination) {
    match termination {
        slime_rt::Termination::Exit(status) => {
            write_value(
                b"[robot-supervisor] controller exited status=",
                status.unsigned_abs(),
            );
        }
        slime_rt::Termination::Fault(detail) => {
            write_value(b"[robot-supervisor] controller faulted detail=", detail);
        }
        slime_rt::Termination::Unhealthy => {
            debug_write(b"[robot-supervisor] controller declared unhealthy\n");
        }
        slime_rt::Termination::Timeout | slime_rt::Termination::PeerLoss => {
            fail(b"the root reported a cause no mechanism in it can produce")
        }
    }
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
    debug_write(b"[robot-supervisor] FAIL ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}

fn fail_with(reason: &[u8], error: i64) -> ! {
    debug_write(b"[robot-supervisor] FAIL ");
    debug_write(reason);
    write_value(b" error=", error.unsigned_abs());
    exit(1)
}
