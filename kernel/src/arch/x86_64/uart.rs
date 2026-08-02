//! The 16550 COM1 UART reached through legacy port I/O.
//!
//! This is the diagnostic transport for the `x86_64-qemu-virtio` profile. The
//! neutral console in [`crate::drivers::serial`] owns the lock, line endings,
//! and formatting; only the port mechanism lives here.

use super::cpu;

const COM1: u16 = 0x3F8;
const INTERRUPT_ENABLE: u16 = COM1 + 1;
const FIFO_CONTROL: u16 = COM1 + 2;
const LINE_CONTROL: u16 = COM1 + 3;
const MODEM_CONTROL: u16 = COM1 + 4;
const LINE_STATUS: u16 = COM1 + 5;
const TRANSMITTER_EMPTY: u8 = 1 << 5;
const DLAB: u8 = 1 << 7;
const SPINS: usize = 100_000;

/// Configure the UART: 38400 baud, 8N1, FIFOs on.
///
/// Every wait is bounded so a machine with no legacy COM1 cannot hang
/// bring-up.
pub fn init() {
    // SAFETY: fixed legacy UART ports owned by the kernel diagnostic path.
    unsafe {
        cpu::outb(INTERRUPT_ENABLE, 0x00);
        cpu::outb(LINE_CONTROL, DLAB);
        cpu::outb(COM1, 0x03);
        cpu::outb(INTERRUPT_ENABLE, 0x00);
        cpu::outb(LINE_CONTROL, 0x03);
        cpu::outb(FIFO_CONTROL, 0xC7);
        cpu::outb(MODEM_CONTROL, 0x0B);
    }
}

/// Transmit one byte, dropping it if the transmitter never drains.
pub fn write_byte(byte: u8) {
    if wait_transmitter_empty() {
        // SAFETY: the transmitter holding register is empty and owned here.
        unsafe { cpu::outb(COM1, byte) };
    }
}

fn wait_transmitter_empty() -> bool {
    for _ in 0..SPINS {
        // SAFETY: reading line status has no device side effect.
        if unsafe { cpu::inb(LINE_STATUS) } & TRANSMITTER_EMPTY != 0 {
            return true;
        }
    }
    false
}
