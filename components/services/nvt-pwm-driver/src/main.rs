#![no_std]
#![no_main]

//! Servo PWM on the Novatek NT98690 PWM block, over the one page the
//! generation grants this driver, serving `contracts/pwm-servo/v1` on its
//! endpoint.
//!
//! The page (offset 0 of the block, 4 KiB) holds every register channels 0–7
//! need, and also channels 8–12's and the shared `ENABLE`/`DISABLE`/`LOAD`
//! words. PWM12 is the SoC's core-voltage regulator, so no capability bound
//! can express what this driver may touch; the policy here is the only guard:
//! only channels `0..=MAX_CHANNEL`'s `CTRL`/`PERIOD`/`EXT` words are written,
//! and the shared words are written with this driver's channel bits alone.
//!
//! The root binds the device the generation declares; a generation that
//! declares one this platform lacks (every QEMU plane) binds nothing, and the
//! driver stays resident answering `STATUS_NO_DEVICE`. Before the first write
//! the page's word 0 is read: a virtio transport's magic there means the
//! ordinal resolved to something that is not a PWM block, and the driver
//! refuses the same way.

use slime_components::nvt_pwm::{
    DISABLE_OFFSET, ENABLE_OFFSET, LOAD_OFFSET, ctrl_offset, ext_offset, period_offset,
    period_words,
};
use slime_components::tick_clock::TickClock;
use slime_proto::pwm_servo::{
    CLOCK_HZ, FAILSAFE_NS, FORMAT_VERSION, MAX_CHANNEL, MAX_PERIOD_US, MAX_PULSE_US, MIN_PERIOD_US,
    MIN_PULSE_US, PULSE_DISABLE, PWM_SERVO_MAGIC, STATUS_BAD_CHANNEL, STATUS_BAD_PERIOD,
    STATUS_BAD_PULSE, STATUS_DEVICE_ERROR, STATUS_MALFORMED, STATUS_NO_DEVICE, STATUS_OK,
    WirePwmServoReply, WirePwmServoRequest,
};
use slime_proto::valid_pwm_servo_request;
use slime_rt::{
    ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, debug_write, exit, io_device_bind, io_mmio_map,
    monotonic_frequency, monotonic_read, recv, send, yield_now,
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

/// The mapped PWM page.
#[derive(Clone, Copy)]
struct Block {
    base: usize,
}

impl Block {
    fn read(self, offset: usize) -> u32 {
        // SAFETY: `base` is the page the root mapped for this task at
        // `MMIO_BASE`, and every offset used is a documented register below
        // 4 KiB.
        unsafe { ((self.base + offset) as *const u32).read_volatile() }
    }
    fn write(self, offset: usize, value: u32) {
        // SAFETY: as `read`.
        unsafe { ((self.base + offset) as *mut u32).write_volatile(value) }
    }
}

/// One channel's programmed state, for the failsafe.
#[derive(Clone, Copy)]
struct Channel {
    period_us: u32,
    pulse_us: u32,
}

struct Driver {
    block: Option<Block>,
    channels: [Channel; CHANNELS],
}

fn main(_: u32) {
    // A platform with nothing behind the declared ordinal refuses the bind or,
    // with an empty inventory, the map; either is the device's absence, not
    // a fault of this driver.
    let block = io_device_bind(DEVICE_SLOT)
        .ok()
        .and_then(|device| {
            io_mmio_map(DEVICE_SLOT, MMIO_SLOT, device.epoch, MMIO_BASE, 0, PAGE).ok()
        })
        .map(|mapping| Block {
            base: mapping.base as usize,
        })
        .filter(|block| block.read(0) != VIRTIO_MAGIC);
    match block {
        Some(_) => {
            write_number(b"[nvt-pwm-driver] ready clk_hz=", u64::from(CLOCK_HZ));
            debug_write(b"\n");
        }
        None => {
            debug_write(b"[nvt-pwm-driver] device absent, refusing requests\n");
        }
    }
    let mut driver = Driver {
        block,
        channels: [Channel {
            period_us: MIN_PERIOD_US,
            pulse_us: PULSE_DISABLE,
        }; CHANNELS],
    };
    // The silence window exists for a channel a live device is driving; with
    // no device there is nothing to return to idle, and the clock is left
    // alone rather than read on every idle turn.
    let mut clock = driver.block.map(|_| {
        let rate = monotonic_frequency().unwrap_or_else(|_| fail(b"clock rate"));
        let base = monotonic_read().unwrap_or_else(|_| fail(b"monotonic read"));
        let clock = TickClock::new(rate, base).unwrap_or_else(|_| fail(b"clock rate too slow"));
        (clock, clock.millis(base))
    });

    loop {
        let mut bytes = [0u8; MAX_MSG];
        let mut caps = [0u64; MAX_CAPS_PER_MSG];
        match recv(PEER_SLOT, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => {
                if let Some((clock, last_command_ms)) = clock.as_mut()
                    && driver.any_live()
                {
                    let now = monotonic_read().unwrap_or_else(|_| fail(b"monotonic read"));
                    let now_ms = clock.millis(now);
                    if now_ms - *last_command_ms >= FAILSAFE_MS && driver.failsafe() {
                        *last_command_ms = now_ms;
                    }
                }
                yield_now();
            }
            result if result < 0 => fail(b"peer receive"),
            result => {
                let answer = driver.serve(&bytes[..result as usize]).encode();
                if let Some((clock, last_command_ms)) = clock.as_mut() {
                    let now = monotonic_read().unwrap_or_else(|_| fail(b"monotonic read"));
                    *last_command_ms = clock.millis(now);
                }
                // The client's exchange is a send followed by a receive on the
                // same endpoint, so the answer goes back the same way and waits
                // for the client to reach its receive.
                loop {
                    match send(PEER_SLOT, &answer, &[]) {
                        ERR_WOULDBLOCK => yield_now(),
                        result if result < 0 => fail(b"answer"),
                        _ => break,
                    }
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
        debug_write(b"[nvt-pwm-driver] request ch=");
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
        let Some(block) = self.block else {
            return answer(STATUS_NO_DEVICE, 0);
        };
        let channel = request.channel;
        let (status, detail) = if request.pulse_us == PULSE_DISABLE {
            disable(block, channel)
        } else {
            program(block, channel, request.period_us, request.pulse_us)
        };
        if status == STATUS_OK {
            self.channels[channel as usize] = Channel {
                period_us: request.period_us,
                pulse_us: request.pulse_us,
            };
        }
        answer(status, detail)
    }

    /// Whether any channel is driving above the idle width, which is the only
    /// state the silence window guards.
    fn any_live(&self) -> bool {
        self.channels
            .iter()
            .any(|channel| channel.pulse_us > IDLE_PULSE_US)
    }

    /// Return every channel driving above idle to the idle width. `true` when
    /// something was programmed, so the silence window restarts from now.
    fn failsafe(&mut self) -> bool {
        let Some(block) = self.block else {
            return false;
        };
        let mut fired = false;
        for (index, channel) in self.channels.iter_mut().enumerate() {
            if channel.pulse_us <= IDLE_PULSE_US {
                continue;
            }
            let ch = index as u32;
            write_number(b"[nvt-pwm-driver] failsafe ch=", u64::from(ch));
            write_number(b" pulse_us=", u64::from(IDLE_PULSE_US));
            debug_write(b"\n");
            let (status, _) = program(block, ch, channel.period_us, IDLE_PULSE_US);
            if status == STATUS_OK {
                channel.pulse_us = IDLE_PULSE_US;
            }
            fired = true;
        }
        fired
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

/// Program `channel` free-running with `pulse_us` high in each `period_us`
/// frame: `CTRL` 0, then the period pair, then `ENABLE` for a stopped channel
/// or `LOAD` for a running one so the new frame latches without a glitch.
/// Every word is read back; the reply's detail is the `PERIOD` word read.
fn program(block: Block, channel: u32, period_us: u32, pulse_us: u32) -> (i32, u32) {
    let Some((period, ext)) = period_words(0, pulse_us, period_us) else {
        return (STATUS_BAD_PULSE, 0);
    };
    let bit = 1u32 << channel;
    block.write(ctrl_offset(channel), 0);
    block.write(period_offset(channel), period);
    block.write(ext_offset(channel), ext);
    if block.read(ENABLE_OFFSET) & bit == 0 {
        block.write(ENABLE_OFFSET, bit);
    } else {
        block.write(LOAD_OFFSET, bit);
    }
    let enabled = block.read(ENABLE_OFFSET) & bit != 0;
    write_number(b"[nvt-pwm-driver] check enable ch=", u64::from(channel));
    debug_write(if enabled {
        b" = ok\n"
    } else {
        b" = mismatch\n"
    });
    let period_read = block.read(period_offset(channel));
    let ext_read = block.read(ext_offset(channel));
    report_word(b"[nvt-pwm-driver] check period = ", period_read, period);
    report_word(b"[nvt-pwm-driver] check ext = ", ext_read, ext);
    if enabled && period_read == period && ext_read == ext {
        (STATUS_OK, period_read)
    } else {
        (STATUS_DEVICE_ERROR, period_read)
    }
}

/// Stop `channel`: the pad idles low. Read back that its enable cleared.
fn disable(block: Block, channel: u32) -> (i32, u32) {
    let bit = 1u32 << channel;
    block.write(DISABLE_OFFSET, bit);
    let enabled = block.read(ENABLE_OFFSET) & bit != 0;
    write_number(b"[nvt-pwm-driver] check enable ch=", u64::from(channel));
    debug_write(if enabled {
        b" = mismatch\n"
    } else {
        b" = off\n"
    });
    let period_read = block.read(period_offset(channel));
    if enabled {
        (STATUS_DEVICE_ERROR, period_read)
    } else {
        (STATUS_OK, period_read)
    }
}

fn report_word(prefix: &[u8], read: u32, expected: u32) {
    debug_write(prefix);
    write_hex(read);
    debug_write(if read == expected {
        b" ok\n"
    } else {
        b" mismatch\n"
    });
}

fn write_hex(value: u32) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0u8; 10];
    out[0] = b'0';
    out[1] = b'x';
    for (index, byte) in out[2..].iter_mut().enumerate() {
        *byte = HEX[((value >> (28 - index * 4)) & 0xf) as usize];
    }
    debug_write(&out);
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
    debug_write(b"[nvt-pwm-driver] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
