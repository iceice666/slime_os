#![no_std]
#![no_main]

use boot_contracts::generation::{
    RIGHT_CLOCK_MONOTONIC_READ, RIGHT_CLOCK_SIMULATED_ADVANCE, RIGHT_CLOCK_SIMULATED_READ,
    RIGHT_CLOCK_TIMER_USE,
};
use sel4::{MessageInfoBuilder, cap};
use slime_proto::syscall_abi::clock_labels;
use slime_rt::{
    ERR_BAD_CAP, ERR_INVALID_ARG, ERR_OUT_OF_MEMORY, debug_write, exit, monotonic_read,
    notification_poll, notification_signal, notification_wait, simulated_time_advance,
    simulated_time_read, timer_arm, timer_cancel, yield_now,
};

const TIMER_NOTIFICATION_SLOT: u32 = 0;
const TIMER_BADGE: u64 = 1 << 9;
const CANCEL_SPINS: usize = 100_000;
const EXPIRY_SPINS: usize = 4_000_000;
const SIMULATED_READER_READY_SLOT: u32 = 0;
const SIMULATED_READER_READY_BADGE: u64 = 1;

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    let rights = rights_mask();
    if rights == RIGHT_CLOCK_MONOTONIC_READ {
        run_monotonic();
    } else if rights == RIGHT_CLOCK_TIMER_USE {
        run_timer();
    } else if rights == RIGHT_CLOCK_SIMULATED_READ {
        run_simulated_reader();
    } else if rights == RIGHT_CLOCK_SIMULATED_ADVANCE {
        run_simulated_advancer();
    } else if rights == 0 {
        run_denied();
    } else {
        fail(b"unexpected authority set")
    }
}

fn rights_mask() -> u64 {
    let mut rights = 0;
    if let Ok(first) = monotonic_read()
        && monotonic_read().is_ok_and(|second| second >= first)
    {
        rights |= RIGHT_CLOCK_MONOTONIC_READ;
    }
    if let Ok(timer) = timer_arm(1_000_000_000) {
        if timer_cancel(timer) != 0 {
            fail(b"timer authority probe cancellation")
        }
        rights |= RIGHT_CLOCK_TIMER_USE;
    }
    if simulated_time_read().is_ok() {
        rights |= RIGHT_CLOCK_SIMULATED_READ;
    }
    if simulated_time_advance(0).is_ok() {
        rights |= RIGHT_CLOCK_SIMULATED_ADVANCE;
    }
    rights
}

fn run_monotonic() -> ! {
    let first = monotonic_read().unwrap_or_else(|_| fail(b"monotonic first read"));
    let second = wait_for_monotonic_advance(first);
    if timer_arm(1) != Err(ERR_BAD_CAP)
        || simulated_time_read() != Err(ERR_BAD_CAP)
        || simulated_time_advance(0) != Err(ERR_BAD_CAP)
    {
        fail(b"monotonic holder gained another authority")
    }
    write_pair(
        b"[clock-probe:monotonic] first=",
        first,
        b" second=",
        second,
    );
    exit(0)
}

fn wait_for_monotonic_advance(first: u64) -> u64 {
    for _ in 0..CANCEL_SPINS {
        let next = monotonic_read().unwrap_or_else(|_| fail(b"monotonic second read"));
        if next < first {
            fail(b"monotonic clock regressed")
        }
        if next > first {
            return next;
        }
        yield_now();
    }
    fail(b"monotonic clock did not advance")
}

fn run_timer() -> ! {
    if monotonic_read() != Err(ERR_BAD_CAP)
        || simulated_time_read() != Err(ERR_BAD_CAP)
        || simulated_time_advance(0) != Err(ERR_BAD_CAP)
    {
        fail(b"timer holder gained another authority")
    }
    if notification_poll(TIMER_NOTIFICATION_SLOT) != Ok(None) {
        fail(b"timer notification was not initially clear")
    }

    let cancelled = timer_arm(50_000_000).unwrap_or_else(|_| fail(b"cancel timer arm"));
    if timer_cancel(cancelled) != 0 {
        fail(b"timer cancel")
    }
    for _ in 0..CANCEL_SPINS {
        if notification_poll(TIMER_NOTIFICATION_SLOT) != Ok(None) {
            fail(b"cancelled timer delivered")
        }
        yield_now();
    }
    debug_write(b"[clock-probe:timer] cancel silent=1\n");

    let first = timer_arm(20_000_000).unwrap_or_else(|_| fail(b"expiry timer arm"));
    let second = timer_arm(40_000_000).unwrap_or_else(|_| fail(b"peer timer arm"));
    if first == second {
        fail(b"timer ids collided")
    }
    if timer_arm(60_000_000) != Err(ERR_OUT_OF_MEMORY) {
        fail(b"timer quota did not bind")
    }
    debug_write(b"[clock-probe:timer] quota refused=1 peer-live=1\n");

    let badge = wait_for_badge();
    if badge != TIMER_BADGE {
        fail(b"timer delivered wrong badge")
    }
    if notification_poll(TIMER_NOTIFICATION_SLOT) != Ok(None) {
        fail(b"timer delivered more than once")
    }
    if timer_cancel(first) != ERR_BAD_CAP {
        fail(b"expired timer remained live")
    }
    debug_write(b"[clock-probe:timer] expired badge=0x200 once=1 peer-intact=1 teardown-live=1\n");
    exit(0)
}

fn wait_for_badge() -> u64 {
    for _ in 0..EXPIRY_SPINS {
        match notification_poll(TIMER_NOTIFICATION_SLOT) {
            Ok(Some(badge)) => return badge,
            Ok(None) => yield_now(),
            Err(_) => fail(b"timer notification poll"),
        }
    }
    fail(b"timer did not expire")
}

fn run_simulated_reader() -> ! {
    if monotonic_read() != Err(ERR_BAD_CAP)
        || timer_arm(1) != Err(ERR_BAD_CAP)
        || simulated_time_advance(1) != Err(ERR_BAD_CAP)
    {
        fail(b"simulated reader gained another authority")
    }
    let first = simulated_time_read().unwrap_or_else(|_| fail(b"simulated first read"));
    for _ in 0..CANCEL_SPINS {
        yield_now();
    }
    let second = simulated_time_read().unwrap_or_else(|_| fail(b"simulated second read"));
    if first != second {
        fail(b"simulated clock advanced before readiness")
    }
    write_pair(b"[clock-probe:sim-read] first=", first, b" second=", second);
    if notification_signal(SIMULATED_READER_READY_SLOT) != 0 {
        fail(b"simulated reader readiness signal")
    }
    exit(0)
}

fn run_simulated_advancer() -> ! {
    if notification_wait(SIMULATED_READER_READY_SLOT) != Ok(SIMULATED_READER_READY_BADGE) {
        fail(b"simulated reader readiness wait")
    }
    if monotonic_read() != Err(ERR_BAD_CAP)
        || timer_arm(1) != Err(ERR_BAD_CAP)
        || simulated_time_read() != Err(ERR_BAD_CAP)
    {
        fail(b"simulated advancer gained another authority")
    }
    let first = simulated_time_advance(7).unwrap_or_else(|_| fail(b"simulated first advance"));
    let second = simulated_time_advance(0).unwrap_or_else(|_| fail(b"simulated second advance"));
    if second != first + 7 {
        fail(b"simulated clock advanced incorrectly")
    }
    if simulated_time_advance(u64::MAX) != Err(ERR_INVALID_ARG) {
        fail(b"simulated overflow changed state")
    }
    let after = simulated_time_advance(0).unwrap_or_else(|_| fail(b"simulated post-overflow read"));
    if after != second {
        fail(b"simulated overflow was not atomic")
    }
    write_pair(
        b"[clock-probe:sim-advance] before=",
        first,
        b" after=",
        after,
    );
    exit(0)
}

fn malformed_clock_request() -> i64 {
    let endpoint = cap::Endpoint::from_bits(slime_rt::ROOT_SERVICE_SLOT);
    let info = MessageInfoBuilder::default()
        .label(clock_labels::MONOTONIC_READ)
        .length(1)
        .build();
    let reply = endpoint.call_with_mrs(info, [0; 4]);
    if reply.info.length() < 1 {
        ERR_INVALID_ARG
    } else {
        reply.msg[0] as i64
    }
}

fn run_denied() -> ! {
    let monotonic = monotonic_read();
    let timer = timer_arm(1);
    let simulated_read = simulated_time_read();
    let simulated_advance = simulated_time_advance(1);
    if monotonic != Err(ERR_BAD_CAP)
        || timer != Err(ERR_BAD_CAP)
        || simulated_read != Err(ERR_BAD_CAP)
        || simulated_advance != Err(ERR_BAD_CAP)
    {
        fail(b"undeclared operation was admitted")
    }
    if malformed_clock_request() != ERR_INVALID_ARG {
        fail(b"malformed clock request was not distinguished")
    }
    debug_write(
        b"[clock-probe:denied] monotonic=-1 timer=-1 sim-read=-1 sim-advance=-1 malformed=-4\n",
    );
    exit(0)
}

fn write_pair(prefix: &[u8], first: u64, middle: &[u8], second: u64) {
    let mut first_digits = [0u8; 20];
    let mut second_digits = [0u8; 20];
    debug_write(prefix);
    debug_write(decimal(first, &mut first_digits));
    debug_write(middle);
    debug_write(decimal(second, &mut second_digits));
    debug_write(b"\n");
}

fn decimal(value: u64, digits: &mut [u8; 20]) -> &[u8] {
    if value == 0 {
        digits[19] = b'0';
        return &digits[19..];
    }
    let mut remaining = value;
    let mut index = digits.len();
    while remaining != 0 {
        index -= 1;
        digits[index] = b'0' + (remaining % 10) as u8;
        remaining /= 10;
    }
    &digits[index..]
}

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[clock-probe] FAIL ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
