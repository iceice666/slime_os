#![no_std]
#![no_main]

//! Servo PWM over the one device page the generation grants this driver,
//! serving `contracts/pwm-servo/v1` on its endpoint. Each request arrives as
//! a call and is answered through the caller's reply capability, which cannot
//! block, so a client that falls silent cannot hold the driver out of its
//! failsafe sweep.
//!
//! The root binds the device the generation declares; a generation that
//! declares one this platform lacks (every QEMU plane) binds nothing, and the
//! driver stays resident answering `STATUS_NO_DEVICE`. Before anything else
//! the page's word 0 is read: a virtio transport's magic there means the
//! ordinal resolved to something that is not a PWM block, and the driver
//! refuses the same way. What the page's words mean is the platform's
//! register model in `block`, which also refuses a page it does not know.
//!
//! No capability bound narrows a page to a channel, so which channels this
//! driver may drive is policy here: only channels `0..=MAX_CHANNEL` are ever
//! programmed, and the model touches the shared words with those channels'
//! bits alone.
//!
//! A channel driving above the idle width is returned to it when the device
//! has accepted no request for that channel for `FAILSAFE_NS`; requests for
//! other channels and requests this driver refuses do not extend the window.
//! A return the device refuses is retried every `FAILSAFE_RETRY_MS`.

mod block;

use block::Model;
use slime_components::servo_failsafe::Failsafe;
use slime_components::tick_clock::TickClock;
use slime_proto::pwm_servo::{
    CLOCK_HZ, FAILSAFE_NS, FORMAT_VERSION, MAX_CHANNEL, MAX_PERIOD_US, MAX_PULSE_US, MIN_PERIOD_US,
    MIN_PULSE_US, PULSE_DISABLE, PWM_SERVO_MAGIC, STATUS_BAD_CHANNEL, STATUS_BAD_PERIOD,
    STATUS_BAD_PULSE, STATUS_MALFORMED, STATUS_NO_DEVICE, STATUS_OK, WirePwmServoReply,
    WirePwmServoRequest,
};
use slime_proto::valid_pwm_servo_request;
use slime_rt::{
    ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, debug_write, exit, io_device_bind, io_mmio_map,
    monotonic_frequency, monotonic_read, recv, reply, yield_now,
};

slime_rt::entry!(main);

const PEER_SLOT: u32 = 0;
const DEVICE_SLOT: u32 = 1;
const MMIO_SLOT: u32 = 2;
const MMIO_BASE: u64 = 0x0000_0020_0000_0000;
const PAGE: u32 = 4096;
const VIRTIO_MAGIC: u32 = 0x7472_6976;
const CHANNELS: usize = MAX_CHANNEL as usize + 1;
/// The width a channel is returned to when its holder falls silent: an ESC's
/// idle, a servo's low end.
const IDLE_PULSE_US: u32 = MIN_PULSE_US;
const FAILSAFE_MS: i64 = (FAILSAFE_NS / 1_000_000) as i64;
/// How soon a return to idle the device refused is attempted again: short
/// against the window, long enough that a persistent fault does not flood the
/// console with a readback report every scheduling turn.
const FAILSAFE_RETRY_MS: i64 = 1_000;

/// The mapped device page: the first 4 KiB of the block, at `MMIO_BASE`.
#[derive(Clone, Copy)]
pub struct Page {
    base: usize,
}

impl Page {
    /// The word at `offset`, which must lie inside the page.
    pub fn read(self, offset: usize) -> u32 {
        // SAFETY: `base` is the page the root mapped for this task at
        // `MMIO_BASE`; the model keeps every offset below `PAGE`.
        unsafe { ((self.base + offset) as *const u32).read_volatile() }
    }
    /// Write `value` to the word at `offset`, which must lie inside the page.
    pub fn write(self, offset: usize, value: u32) {
        // SAFETY: as `read`.
        unsafe { ((self.base + offset) as *mut u32).write_volatile(value) }
    }
}

/// A bound PWM block and the clock its silence windows are measured on. The
/// clock is read only while a device is bound: with none, an idle turn makes no
/// root call.
#[derive(Clone, Copy)]
struct Device {
    model: Model,
    clock: TickClock,
}

impl Device {
    fn now_ms(self) -> i64 {
        let now = monotonic_read().unwrap_or_else(|_| fail(b"monotonic read"));
        self.clock.millis(now)
    }
}

struct Driver {
    device: Option<Device>,
    failsafe: Failsafe<CHANNELS>,
}

fn main(_: u32) {
    // A platform with nothing behind the declared ordinal refuses the bind or,
    // with an empty inventory, the map; either is the device's absence, not
    // a fault of this driver.
    let page = io_device_bind(DEVICE_SLOT)
        .ok()
        .and_then(|device| {
            io_mmio_map(DEVICE_SLOT, MMIO_SLOT, device.epoch, MMIO_BASE, 0, PAGE).ok()
        })
        .map(|mapping| Page {
            base: mapping.base as usize,
        })
        .filter(|page| page.read(0) != VIRTIO_MAGIC);
    let model = match page {
        None => {
            debug_write(b"[pwm-servo-driver] device absent, refusing requests\n");
            None
        }
        Some(page) => match Model::attach(page) {
            Some(model) => {
                write_number(b"[pwm-servo-driver] ready clk_hz=", u64::from(CLOCK_HZ));
                debug_write(b"\n");
                Some(model)
            }
            None => {
                debug_write(
                    b"[pwm-servo-driver] bound page has no register model, refusing requests\n",
                );
                None
            }
        },
    };
    let device = model.map(|model| {
        let rate = monotonic_frequency().unwrap_or_else(|_| fail(b"clock rate"));
        let base = monotonic_read().unwrap_or_else(|_| fail(b"monotonic read"));
        let clock = TickClock::new(rate, base).unwrap_or_else(|_| fail(b"clock rate too slow"));
        Device { model, clock }
    });
    let mut driver = Driver {
        device,
        failsafe: Failsafe::new(MIN_PERIOD_US, IDLE_PULSE_US, FAILSAFE_MS, FAILSAFE_RETRY_MS),
    };

    loop {
        let mut bytes = [0u8; MAX_MSG];
        let mut caps = [0u64; MAX_CAPS_PER_MSG];
        match recv(PEER_SLOT, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => {
                if let Some(device) = driver.device
                    && driver.failsafe.any_live()
                {
                    driver.sweep(device.model, device.now_ms());
                }
                yield_now();
            }
            result if result < 0 => fail(b"peer receive"),
            result => {
                let answer = driver.serve(&bytes[..result as usize]).encode();
                // The request arrived as a call, so its sender is parked on this
                // answer and the reply capability cannot block: a client that
                // falls silent mid-exchange never holds the loop out of the
                // sweep. That capability lives only until the next receive, so
                // the answer goes out before the loop polls again. A request
                // sent without a call has no reply capability and is answered
                // to no one.
                if reply(&answer) < 0 {
                    fail(b"answer");
                }
            }
        }
    }
}

impl Driver {
    /// Answer one request: the protocol's identity, then each bound in the
    /// order channel, period, pulse, each with its own status, then the
    /// device.
    fn serve(&mut self, bytes: &[u8]) -> WirePwmServoReply {
        let Some(request) = WirePwmServoRequest::decode(bytes).filter(valid_pwm_servo_request)
        else {
            return answer(STATUS_MALFORMED, 0);
        };
        debug_write(b"[pwm-servo-driver] request ch=");
        write_number(b"", u64::from(request.channel));
        write_number(b" period_us=", u64::from(request.period_us));
        write_number(b" pulse_us=", u64::from(request.pulse_us));
        debug_write(b"\n");
        if request.channel > MAX_CHANNEL {
            return answer(STATUS_BAD_CHANNEL, 0);
        }
        if !(MIN_PERIOD_US..=MAX_PERIOD_US).contains(&request.period_us) {
            return answer(STATUS_BAD_PERIOD, 0);
        }
        if request.pulse_us != PULSE_DISABLE
            && (!(MIN_PULSE_US..=MAX_PULSE_US).contains(&request.pulse_us)
                || request.pulse_us > request.period_us)
        {
            return answer(STATUS_BAD_PULSE, 0);
        }
        let Some(device) = self.device else {
            return answer(STATUS_NO_DEVICE, 0);
        };
        let channel = request.channel;
        let (status, detail) = if request.pulse_us == PULSE_DISABLE {
            device.model.disable(channel)
        } else {
            device
                .model
                .program(channel, request.period_us, request.pulse_us)
        };
        if status == STATUS_OK {
            self.failsafe.accepted(
                channel as usize,
                request.period_us,
                request.pulse_us,
                device.now_ms(),
            );
        }
        answer(status, detail)
    }

    /// Return each channel whose silence window has passed to the idle width.
    /// A channel the device refuses stays live and is attempted again after
    /// the retry interval, not a whole window.
    fn sweep(&mut self, model: Model, now_ms: i64) {
        let mut due = [(0usize, 0u32); CHANNELS];
        let mut count = 0;
        for entry in self.failsafe.due(now_ms) {
            due[count] = entry;
            count += 1;
        }
        for &(index, period_us) in &due[..count] {
            let ch = index as u32;
            write_number(b"[pwm-servo-driver] failsafe ch=", u64::from(ch));
            write_number(b" pulse_us=", u64::from(IDLE_PULSE_US));
            debug_write(b"\n");
            if model.program(ch, period_us, IDLE_PULSE_US).0 == STATUS_OK {
                self.failsafe.returned(index);
            } else {
                write_number(b"[pwm-servo-driver] failsafe deferred ch=", u64::from(ch));
                debug_write(b"\n");
                self.failsafe.deferred(index, now_ms);
            }
        }
    }
}

fn answer(status: i32, detail: u32) -> WirePwmServoReply {
    WirePwmServoReply {
        magic: PWM_SERVO_MAGIC,
        version: FORMAT_VERSION,
        status,
        detail,
    }
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
    debug_write(b"[pwm-servo-driver] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
