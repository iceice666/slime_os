//! The legacy i8042 PS/2 controller reached through port I/O.
//!
//! This is platform mechanism for the `x86_64-qemu-virtio` profile: bounded
//! controller commands and raw scan-code bytes. Scan-code decoding, the event
//! queue, waiter registration, and scripted input stay in the neutral input
//! driver, which is the part a different platform's keyboard transport reuses.

use super::cpu;

const STATUS_PORT: u16 = 0x64;
const DATA_PORT: u16 = 0x60;
const STATUS_OUTPUT_FULL: u8 = 1 << 0;
const STATUS_INPUT_FULL: u8 = 1 << 1;
const CONTROLLER_SPINS: usize = 100_000;

/// The controller stopped responding within its bounded spin budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControllerTimeout;

/// Read the status register.
pub fn status() -> u8 {
    // SAFETY: reading the i8042 status port has no device side effect.
    unsafe { cpu::inb(STATUS_PORT) }
}

/// Whether a byte is waiting in the output buffer.
pub fn output_full() -> bool {
    status() & STATUS_OUTPUT_FULL != 0
}

/// Read the data port unconditionally. Only call once [`output_full`] holds.
pub fn read_data_port() -> u8 {
    // SAFETY: consuming the output buffer is this driver's own transaction.
    unsafe { cpu::inb(DATA_PORT) }
}

/// Send a controller command byte, waiting for the input buffer to drain.
pub fn command(value: u8) -> Result<(), ControllerTimeout> {
    wait_input_empty()?;
    // SAFETY: the input buffer is empty; this is a bounded controller command.
    unsafe { cpu::outb(STATUS_PORT, value) };
    Ok(())
}

/// Send a command byte without waiting. Used for the initial port-disable
/// sequence, which must run before the controller is known to be responsive.
pub fn command_immediate(value: u8) {
    // SAFETY: fixed legacy controller port owned by this driver.
    unsafe { cpu::outb(STATUS_PORT, value) };
}

/// Write a data byte, waiting for the input buffer to drain.
pub fn write_data(value: u8) -> Result<(), ControllerTimeout> {
    wait_input_empty()?;
    // SAFETY: the input buffer is empty; this is a bounded device write.
    unsafe { cpu::outb(DATA_PORT, value) };
    Ok(())
}

/// Read one data byte, waiting a bounded time for the output buffer to fill.
pub fn read_data() -> Result<u8, ControllerTimeout> {
    for _ in 0..CONTROLLER_SPINS {
        if output_full() {
            return Ok(read_data_port());
        }
    }
    Err(ControllerTimeout)
}

/// Discard any bytes already sitting in the output buffer.
pub fn drain_output() {
    for _ in 0..64 {
        if !output_full() {
            return;
        }
        let _ = read_data_port();
    }
}

fn wait_input_empty() -> Result<(), ControllerTimeout> {
    for _ in 0..CONTROLLER_SPINS {
        if status() & STATUS_INPUT_FULL == 0 {
            return Ok(());
        }
    }
    Err(ControllerTimeout)
}
