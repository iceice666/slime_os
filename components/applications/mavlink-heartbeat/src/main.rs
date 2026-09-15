#![no_std]
#![no_main]

//! One MAVLink v2 HEARTBEAT per second, handed to a serial driver over
//! `contracts/serial-device/v1`.
//!
//! The cadence is a fixed grid on the root's monotonic counter: one period is
//! the counter's own rate, and each deadline is the previous one plus a
//! period, so the time a send takes never accumulates. A beat that finds its
//! deadline already passed goes out at once and the grid is kept; a refused or
//! failed send is reported and the next beat is still due on time.
//!
//! Each send is a call, so this component is parked on the driver's answer
//! and the driver never waits on it. A clock or timer the generation declared
//! but the root refuses is fatal: every later deadline would be meaningless.

use slime_proto::mavlink::encode_heartbeat;
use slime_proto::mavlink_heartbeat::FRAME_LEN;
use slime_proto::serial_device::{
    FORMAT_VERSION, MAX_PAYLOAD, OP_WRITE, SERIAL_MAGIC, STATUS_BAD_LENGTH, STATUS_BAD_OP,
    STATUS_DEVICE_ERROR, STATUS_MALFORMED, STATUS_NO_DEVICE, STATUS_OK, STATUS_TIMEOUT,
    WireSerialReply, WireSerialRequest,
};
use slime_proto::valid_serial_reply;
use slime_rt::{
    MAX_MSG, call, debug_write, exit, monotonic_frequency, monotonic_read, notification_wait,
    resolve_binding, timer_arm,
};

slime_rt::entry!(main);

/// This component's endpoint to the serial driver.
const DRIVER_SLOT: u32 = 0;
/// The declared timer's badge bit on this component's tick notification.
const TIMER_BADGE: u64 = 1 << 9;

fn main(_: u32) {
    let tick = resolve_binding(b"notification:mavlink-heartbeat-tick+wait")
        .unwrap_or_else(|_| fail(b"tick binding"));
    let period = monotonic_frequency().unwrap_or_else(|_| fail(b"clock rate"));
    let mut deadline = clock();
    let mut seq: u8 = 0;
    loop {
        let now = clock();
        let status = send(&encode_heartbeat(seq));
        write_number(b"[mavlink-heartbeat] send seq=", u64::from(seq));
        debug_write(b" status=");
        debug_write(status);
        write_number(b" deadline=", deadline);
        write_number(b" now=", now);
        debug_write(b"\n");
        deadline = deadline.saturating_add(period);
        wait_until(tick, deadline);
        seq = seq.wrapping_add(1);
    }
}

/// Send one frame and name the driver's answer.
fn send(frame: &[u8; FRAME_LEN]) -> &'static [u8] {
    let mut payload = [0u8; MAX_PAYLOAD];
    payload[..FRAME_LEN].copy_from_slice(frame);
    let request = WireSerialRequest {
        magic: SERIAL_MAGIC,
        version: FORMAT_VERSION,
        op: OP_WRITE,
        flags: 0,
        length: FRAME_LEN as u16,
        reserved: [0; 2],
        payload,
    }
    .encode();
    let mut answer = [0u8; MAX_MSG];
    let received = call(DRIVER_SLOT, &request, &mut answer);
    if received < 0 {
        fail(b"driver call");
    }
    let Some(reply) =
        WireSerialReply::decode(&answer[..received as usize]).filter(valid_serial_reply)
    else {
        return b"bad-reply";
    };
    match reply.status {
        STATUS_OK if reply.bytes_written as usize == FRAME_LEN => b"ok",
        STATUS_OK => b"bad-reply",
        STATUS_BAD_LENGTH => b"bad-length",
        STATUS_BAD_OP => b"bad-op",
        STATUS_NO_DEVICE => b"no-device",
        STATUS_DEVICE_ERROR => b"device-error",
        STATUS_TIMEOUT => b"timeout",
        STATUS_MALFORMED => b"malformed",
        _ => b"bad-reply",
    }
}

/// Block until the monotonic counter reaches `deadline`, arming at most one
/// timer per wait and re-reading the counter after each expiry.
fn wait_until(tick: u32, deadline: u64) {
    loop {
        let now = clock();
        if now >= deadline {
            return;
        }
        // Fatal rather than returning: the loop condition is unchanged by a
        // failed arm, so returning would spin. One timer is live at a time and
        // the generation grants a quota of two.
        if timer_arm(deadline - now).is_err() {
            fail(b"timer arm");
        }
        loop {
            let badge = notification_wait(tick).unwrap_or_else(|_| fail(b"tick wait"));
            // A wake without the timer bit is a signal this component does
            // not interpret; the armed timer is still live, so keep waiting.
            if badge & TIMER_BADGE != 0 {
                break;
            }
        }
    }
}

fn clock() -> u64 {
    monotonic_read().unwrap_or_else(|_| fail(b"monotonic read"))
}

fn write_number(prefix: &[u8], mut value: u64) {
    debug_write(prefix);
    let mut digits = [0u8; 20];
    let mut index = digits.len();
    if value == 0 {
        index -= 1;
        digits[index] = b'0';
    }
    while value != 0 {
        index -= 1;
        digits[index] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    debug_write(&digits[index..]);
}

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[mavlink-heartbeat] FAIL ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
