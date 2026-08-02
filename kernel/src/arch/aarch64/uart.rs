//! The PL011 UART used for AArch64 diagnostics.
//!
//! The `aarch64-qemu-virt` profile places PL011 at a fixed physical base, so
//! P2.1 reaches it there directly. The device is memory-mapped rather than
//! port-mapped, and it is written through the direct map once the MMU is on —
//! but the first bring-up markers must appear *before* memory management
//! initializes, so writes go to the physical address until the direct-map
//! offset is published, then through it.
//!
//! P2.5 replaces the fixed base with device-tree discovery, which is also what
//! `aarch64-rpi5` will require; until then this base is part of the pinned
//! machine profile.

use core::sync::atomic::{AtomicU64, Ordering};

/// PL011 base on `qemu-system-aarch64 -machine virt`. Fixed by the machine.
const PL011_PHYS_BASE: u64 = 0x0900_0000;

/// Data register: writing transmits a byte.
const UARTDR: usize = 0x00;
/// Flag register.
const UARTFR: usize = 0x18;
/// Integer baud-rate divisor.
const UARTIBRD: usize = 0x24;
/// Fractional baud-rate divisor.
const UARTFBRD: usize = 0x28;
/// Line control register.
const UARTLCR_H: usize = 0x2c;
/// Control register.
const UARTCR: usize = 0x30;
/// Interrupt mask set/clear.
const UARTIMSC: usize = 0x38;
/// Interrupt clear register.
const UARTICR: usize = 0x44;

/// `UARTFR.TXFF`: the transmit FIFO is full.
const FR_TXFF: u32 = 1 << 5;
/// `UARTLCR_H.WLEN` = 8 bits, plus FIFO enable.
const LCR_H_8BIT_FIFO: u32 = (0b11 << 5) | (1 << 4);
/// `UARTCR`: UART enable, transmit enable, receive enable.
const CR_ENABLE: u32 = (1 << 0) | (1 << 8) | (1 << 9);

/// Bounded spin budget waiting for FIFO space, so a wedged or absent UART
/// drops a byte instead of hanging bring-up.
const SPINS: usize = 100_000;

/// Virtual address the UART is reached at. Starts as the physical base, which
/// the identity map makes valid, and is rebased onto the direct map once that
/// offset is known.
static UART_BASE: AtomicU64 = AtomicU64::new(PL011_PHYS_BASE);

/// Configure the UART: 115200 baud 8N1 with FIFOs, interrupts masked.
///
/// The divisors assume the QEMU `virt` 24 MHz reference clock, which is part of
/// the pinned machine profile.
pub fn init() {
    write_register(UARTCR, 0);
    write_register(UARTICR, 0x7ff);
    // 24 MHz / (16 * 115200) = 13.02: integer 13, fractional round(0.02*64) = 1.
    write_register(UARTIBRD, 13);
    write_register(UARTFBRD, 1);
    write_register(UARTLCR_H, LCR_H_8BIT_FIFO);
    write_register(UARTIMSC, 0);
    write_register(UARTCR, CR_ENABLE);
}

/// Rebase the UART onto the direct map.
///
/// Call once memory management has published the direct-map offset. Before
/// this, writes go to the physical address, which the stage-0 identity map
/// keeps valid.
pub fn use_direct_map(offset: u64) {
    UART_BASE.store(PL011_PHYS_BASE.wrapping_add(offset), Ordering::Relaxed);
}

/// Transmit one byte, dropping it if the FIFO never drains.
pub fn write_byte(byte: u8) {
    for _ in 0..SPINS {
        if read_register(UARTFR) & FR_TXFF == 0 {
            write_register(UARTDR, byte as u32);
            return;
        }
        core::hint::spin_loop();
    }
}

fn read_register(offset: usize) -> u32 {
    let address = UART_BASE.load(Ordering::Relaxed) as usize + offset;
    // SAFETY: the pinned machine profile places PL011 at this base, mapped
    // either identically or through the direct map; offsets are in-range and
    // 4-byte aligned. MMIO reads must be volatile.
    unsafe { core::ptr::read_volatile(address as *const u32) }
}

fn write_register(offset: usize, value: u32) {
    let address = UART_BASE.load(Ordering::Relaxed) as usize + offset;
    // SAFETY: as `read_register`; MMIO writes must be volatile.
    unsafe { core::ptr::write_volatile(address as *mut u32, value) }
}
