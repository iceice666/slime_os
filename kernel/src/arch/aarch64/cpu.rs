//! AArch64 CPU control mechanisms.
//!
//! Signatures mirror `arch::x86_64::cpu`, which is the surface neutral kernel
//! code is allowed to use. P2 implements the bodies (`DAIF` masking, `WFI`,
//! and the profile's debug-exit path).

/// Whether maskable interrupts are currently unmasked (`DAIF.I` clear).
pub fn interrupts_enabled() -> bool {
    unimplemented!("aarch64 DAIF read: implemented by P2")
}

/// Mask maskable interrupts.
///
/// # Safety
///
/// Leaving interrupts masked stalls timer preemption and device wakes; the
/// caller must re-enable them on every path out.
pub unsafe fn disable_interrupts() {
    unimplemented!("aarch64 interrupt masking: implemented by P2")
}

/// Unmask maskable interrupts.
///
/// # Safety
///
/// The exception vector table must already be installed.
pub unsafe fn enable_interrupts() {
    unimplemented!("aarch64 interrupt masking: implemented by P2")
}

/// Run `f` with maskable interrupts masked, restoring the previous mask state.
pub fn without_interrupts<T>(f: impl FnOnce() -> T) -> T {
    let enabled = interrupts_enabled();
    // SAFETY: interrupts are restored to their entry state before returning.
    unsafe { disable_interrupts() };
    let result = f();
    if enabled {
        // SAFETY: interrupts were enabled on entry.
        unsafe { enable_interrupts() };
    }
    result
}

/// Unmask interrupts and idle until the next one arrives, without a window in
/// which a wake can be lost.
///
/// # Safety
///
/// As [`enable_interrupts`].
pub unsafe fn enable_interrupts_and_wait() {
    unimplemented!("aarch64 WFI park: implemented by P2")
}

/// Idle until the next interrupt without changing the interrupt mask.
pub fn wait_for_interrupt() {
    unimplemented!("aarch64 WFI: implemented by P2")
}

/// Raise a debug breakpoint trap and return.
///
/// # Safety
///
/// A breakpoint handler that returns must be installed.
pub unsafe fn breakpoint() {
    unimplemented!("aarch64 BRK: implemented by P2")
}

/// Terminate the emulator with `code`, if the profile has a debug-exit path.
pub fn debug_exit(_code: u32) {
    unimplemented!("aarch64 debug exit: implemented by P2")
}

/// Establish the SIMD/floating-point baseline the compiler assumes.
///
/// # Safety
///
/// Must run exactly once at kernel entry, before any code that may use SIMD.
pub unsafe fn init_simd_baseline() {
    unimplemented!("aarch64 CPACR_EL1 FP/SIMD enable: implemented by P2")
}
