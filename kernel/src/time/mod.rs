//! Timekeeping built on the Local APIC timer.
//!
//! [`init`] masks the legacy 8259 PICs, enables the Local APIC, calibrates its
//! timer against the PIT, and programs it to fire periodically at
//! [`TICK_HZ`]. Each tick runs [`on_tick`] from the interrupt handler,
//! advancing a monotonic counter that [`uptime_ms`] and [`sleep_ms`] read.
//!
//! This is the milestone's "APIC/timer support": a working interrupt-driven
//! monotonic clock, with the legacy PIC deliberately silenced so stray IRQs
//! cannot reach vectors we do not handle.

pub mod apic;

use core::sync::atomic::{AtomicU64, Ordering};

/// Timer interrupt frequency. 100 Hz → a 10 ms tick.
pub const TICK_HZ: u64 = 100;

/// Monotonic tick counter, incremented once per timer interrupt.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Advance the tick counter. Called from the timer interrupt handler only.
pub fn on_tick() {
    TICKS.fetch_add(1, Ordering::Relaxed);
}

/// Ticks elapsed since [`init`].
pub fn ticks() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

/// Milliseconds elapsed since [`init`].
pub fn uptime_ms() -> u64 {
    ticks() * 1000 / TICK_HZ
}

/// Busy-wait until at least `ms` milliseconds of ticks have elapsed.
///
/// Requires interrupts to be enabled (so the tick counter advances); halts
/// between checks to avoid spinning hot.
pub fn sleep_ms(ms: u64) {
    let start = ticks();
    let needed = ms * TICK_HZ / 1000;
    while ticks().wrapping_sub(start) < needed {
        // SAFETY: `hlt` waits for the next interrupt; the timer will wake us.
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}

/// Bring up the APIC timer and start ticking. Enables interrupts on return.
///
/// Call after [`crate::interrupts::init`] (the timer vector must be routed) and
/// after [`crate::memory::init`] (LAPIC access goes through the HHDM).
pub fn init() {
    apic::init(TICK_HZ);
    // Unmask interrupts now that a handler exists for the timer vector.
    // SAFETY: the IDT is loaded and the timer gate is installed.
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
    }
}
