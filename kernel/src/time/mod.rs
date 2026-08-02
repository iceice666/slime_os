//! Timekeeping built on the architecture's periodic interrupt source.
//!
//! [`init`] asks the architecture to program a periodic timer at [`TICK_HZ`]
//! and unmasks interrupts. Each tick runs [`on_tick`] from the handler,
//! advancing a monotonic counter that [`uptime_ms`] and [`sleep_ms`] read.
//!
//! The counter, tick rate, and sleep policy are architecture-neutral; the timer
//! device (Local APIC here, generic timer on AArch64) lives behind
//! `arch::target`.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::cpu;
/// The architecture's timer implementation, re-exported so existing
/// `time::apic::…` diagnostic callers keep one path to the timer device.
pub use crate::arch::target::apic;

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
        // Park until the next interrupt; the timer tick wakes us.
        cpu::wait_for_interrupt();
    }
}

/// Bring up the periodic timer and start ticking. Enables interrupts on return.
///
/// Call after [`crate::interrupts::init`] (the timer vector must be routed) and
/// after [`crate::memory::init`] (timer MMIO goes through the direct map).
pub fn init() {
    apic::init(TICK_HZ);
    // Unmask interrupts now that a handler exists for the timer vector.
    // SAFETY: the interrupt table is loaded and the timer gate is installed.
    unsafe { cpu::enable_interrupts() };
}
