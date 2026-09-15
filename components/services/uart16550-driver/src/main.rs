#![no_std]
#![no_main]

//! Bounded serial transmit over the one device page the generation grants
//! this driver, serving `contracts/serial-device/v1` on its endpoint. Each
//! request arrives as a call and is answered through the caller's reply
//! capability, so the driver never waits on a client.
//!
//! The root binds the device the generation declares; a generation that
//! declares one this platform lacks (every QEMU plane) binds nothing, and the
//! driver stays resident answering `STATUS_NO_DEVICE`. Before anything else
//! the page's word 0 is read: a virtio transport's magic there means the
//! ordinal resolved to something that is not a UART, and the driver refuses
//! the same way. The line itself (clock, divisor, register layout) is the
//! platform's, in `line`; with no line the driver refuses rather than guess.
//!
//! Every wait on the line has a deadline of a few character times at the
//! line's own rate, so a stuck transmitter answers `STATUS_TIMEOUT` with the
//! bytes it took, never a hang.

mod line;

use slime_components::uart16550::{Line, Regs, Uart};
use slime_proto::serial_device::{
    FORMAT_VERSION, MAX_PAYLOAD, OP_WRITE, SERIAL_MAGIC, STATUS_BAD_LENGTH, STATUS_BAD_OP,
    STATUS_MALFORMED, STATUS_NO_DEVICE, STATUS_OK, STATUS_TIMEOUT, WireSerialReply,
    WireSerialRequest,
};
use slime_proto::valid_serial_request;
use slime_rt::{
    MAX_CAPS_PER_MSG, MAX_MSG, debug_write, exit, io_device_bind, io_mmio_map, monotonic_frequency,
    monotonic_read, recv_blocking, reply,
};

slime_rt::entry!(main);

const PEER_SLOT: u32 = 0;
const DEVICE_SLOT: u32 = 1;
const MMIO_SLOT: u32 = 2;
const MMIO_BASE: u64 = 0x0000_0020_0000_0000;
const PAGE: u32 = 4096;
const VIRTIO_MAGIC: u32 = 0x7472_6976;

/// The mapped device page: the first 4 KiB of the port, at `MMIO_BASE`.
#[derive(Clone, Copy)]
struct Page {
    base: usize,
}

impl Page {
    /// The 32-bit word at `offset`, which must lie inside the page.
    fn word(self, offset: usize) -> u32 {
        // SAFETY: `base` is the page the root mapped for this task at
        // `MMIO_BASE`; every offset here is below `PAGE`.
        unsafe { ((self.base + offset) as *const u32).read_volatile() }
    }
}

/// The page seen through the platform's register layout.
struct Port {
    page: Page,
    shift: u8,
    width: u8,
}

impl Regs for Port {
    fn read(&self, index: usize) -> u32 {
        let address = self.page.base + (index << self.shift);
        // SAFETY: `Port` exists only for a line whose layout keeps the
        // eight-register window inside the mapped page.
        unsafe {
            if self.width == 1 {
                u32::from((address as *const u8).read_volatile())
            } else {
                (address as *const u32).read_volatile()
            }
        }
    }
    fn write(&mut self, index: usize, value: u32) {
        let address = self.page.base + (index << self.shift);
        // SAFETY: as `read`.
        unsafe {
            if self.width == 1 {
                (address as *mut u8).write_volatile(value as u8);
            } else {
                (address as *mut u32).write_volatile(value);
            }
        }
    }
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
        .filter(|page| page.word(0) != VIRTIO_MAGIC);
    let mut uart = match page {
        None => {
            debug_write(b"[uart16550-driver] device absent, refusing requests\n");
            None
        }
        Some(page) => match line::config() {
            None => {
                debug_write(b"[uart16550-driver] no line configuration, refusing requests\n");
                None
            }
            Some(line) => bring_up(page, line),
        },
    };

    loop {
        let mut bytes = [0u8; MAX_MSG];
        let mut caps = [0u64; MAX_CAPS_PER_MSG];
        let received = recv_blocking(PEER_SLOT, &mut bytes, &mut caps);
        if received < 0 {
            fail(b"peer receive");
        }
        let answer = serve(uart.as_mut(), &bytes[..received as usize]).encode();
        // The request arrived as a call, so its sender is parked on this
        // answer and the reply capability cannot block. That capability lives
        // only until the next receive, so the answer goes out first.
        if reply(&answer) < 0 {
            fail(b"answer");
        }
    }
}

/// Program `line` on `page`, or refuse with the register that disagreed.
fn bring_up(page: Page, line: Line) -> Option<Uart<Port>> {
    // The eight 16550 registers must sit inside the mapped page.
    if !matches!(line.reg_width, 1 | 4)
        || line.reg_shift > 8
        || (8usize << line.reg_shift) > PAGE as usize
    {
        debug_write(b"[uart16550-driver] line layout outside the page, refusing requests\n");
        return None;
    }
    // The clock is read only while a device is bound: with none, the driver
    // makes no root call between requests.
    let tick_hz = monotonic_frequency().unwrap_or_else(|_| fail(b"clock rate"));
    let port = Port {
        page,
        shift: line.reg_shift,
        width: line.reg_width,
    };
    match Uart::configure(port, line, tick_hz, &mut now) {
        Ok(uart) => {
            write_number(
                b"[uart16550-driver] ready clk_hz=",
                u64::from(line.clock_hz),
            );
            write_number(b" divisor=", u64::from(line.divisor));
            debug_write(b"\n");
            Some(uart)
        }
        Err(fault) => {
            debug_write(b"[uart16550-driver] line config mismatch reg=");
            debug_write(fault.reg.as_bytes());
            write_number(b" got=", u64::from(fault.got));
            debug_write(b", refusing requests\n");
            None
        }
    }
}

/// Answer one request: the protocol's identity, the operation, the length,
/// then the canonical encoding, each with its own status, then the device.
fn serve(uart: Option<&mut Uart<Port>>, bytes: &[u8]) -> WireSerialReply {
    let Some(request) = WireSerialRequest::decode(bytes)
        .filter(|request| request.magic == SERIAL_MAGIC && request.version == FORMAT_VERSION)
    else {
        return answer(STATUS_MALFORMED, 0, 0);
    };
    if request.op != OP_WRITE {
        return answer(STATUS_BAD_OP, 0, 0);
    }
    let length = request.length as usize;
    if !(1..=MAX_PAYLOAD).contains(&length) {
        return answer(STATUS_BAD_LENGTH, 0, 0);
    }
    if !valid_serial_request(&request) {
        return answer(STATUS_MALFORMED, 0, 0);
    }
    let Some(uart) = uart else {
        return answer(STATUS_NO_DEVICE, 0, 0);
    };
    match uart.transmit(&request.payload[..length], &mut now) {
        Ok(()) => answer(STATUS_OK, length as u16, uart.line_status()),
        Err(timeout) => answer(STATUS_TIMEOUT, timeout.written as u16, uart.line_status()),
    }
}

fn now() -> u64 {
    monotonic_read().unwrap_or_else(|_| fail(b"monotonic read"))
}

fn answer(status: i32, bytes_written: u16, detail: u32) -> WireSerialReply {
    WireSerialReply {
        magic: SERIAL_MAGIC,
        version: FORMAT_VERSION,
        bytes_written,
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
    debug_write(b"[uart16550-driver] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
