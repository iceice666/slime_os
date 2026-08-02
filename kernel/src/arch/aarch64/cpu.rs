//! AArch64 CPU control mechanisms.
//!
//! Signatures mirror `arch::x86_64::cpu`, which is the surface neutral kernel
//! code is allowed to use. Interrupt masking is `DAIF`, idling is `WFI`, and
//! the debug exit uses the semihosting call QEMU implements for this profile.
//!
//! P2.2 adds the breakpoint path once exception vectors exist; taking a `BRK`
//! before then would escalate rather than return.

/// `DAIF.I`: the IRQ mask bit, as read through `DAIF`.
const DAIF_IRQ_MASK: u64 = 1 << 7;

/// Whether maskable interrupts are currently unmasked (`DAIF.I` clear).
pub fn interrupts_enabled() -> bool {
    let daif: u64;
    // SAFETY: reading DAIF is an unprivileged, side-effect-free register read.
    unsafe {
        core::arch::asm!("mrs {}, daif", out(reg) daif, options(nomem, nostack, preserves_flags));
    }
    daif & DAIF_IRQ_MASK == 0
}

/// Mask maskable interrupts.
///
/// # Safety
///
/// Leaving interrupts masked stalls timer preemption and device wakes; the
/// caller must re-enable them on every path out.
pub unsafe fn disable_interrupts() {
    unsafe {
        core::arch::asm!("msr daifset, #2", options(nomem, nostack, preserves_flags));
    }
}

/// Unmask maskable interrupts.
///
/// # Safety
///
/// The exception vector table must already be installed (P2.2); unmasking
/// before then delivers an interrupt to an absent vector.
pub unsafe fn enable_interrupts() {
    unsafe {
        core::arch::asm!("msr daifclr, #2", options(nomem, nostack, preserves_flags));
    }
}

/// Run `f` with maskable interrupts masked, restoring the previous mask state.
///
/// Interrupts are re-enabled only if they were enabled on entry, so nesting
/// this inside an exception handler does not silently unmask.
pub fn without_interrupts<T>(f: impl FnOnce() -> T) -> T {
    let enabled = interrupts_enabled();
    // SAFETY: interrupts are restored to their entry state before returning.
    unsafe { disable_interrupts() };
    let result = f();
    if enabled {
        // SAFETY: interrupts were enabled on entry, so re-enabling restores the
        // caller's state rather than unmasking under a handler.
        unsafe { enable_interrupts() };
    }
    result
}

/// Unmask interrupts and idle until the next one arrives.
///
/// Unlike x86's `sti; hlt`, AArch64 needs no deferred-unmask trick: `WFI` wakes
/// on a pending interrupt even while it is masked, so unmasking after the wake
/// cannot lose one.
///
/// # Safety
///
/// As [`enable_interrupts`].
pub unsafe fn enable_interrupts_and_wait() {
    unsafe {
        core::arch::asm!(
            "msr daifclr, #2",
            "wfi",
            options(nomem, nostack, preserves_flags)
        );
    }
}

/// Idle until the next interrupt without changing the interrupt mask.
pub fn wait_for_interrupt() {
    // SAFETY: `wfi` only parks the CPU; it is always valid at EL1.
    unsafe {
        core::arch::asm!("wfi", options(nomem, nostack, preserves_flags));
    }
}

/// Raise a debug breakpoint trap and return.
///
/// # Safety
///
/// A breakpoint handler that returns must be installed. P2.2 installs the
/// exception vectors; calling this before then escalates instead of resuming.
pub unsafe fn breakpoint() {
    unsafe {
        core::arch::asm!("brk #0", options(nostack, preserves_flags));
    }
}

/// Semihosting operation `SYS_EXIT`.
const SEMIHOSTING_SYS_EXIT: u64 = 0x18;
/// `ADP_Stopped_ApplicationExit`: the reason code an ordinary exit reports.
const ADP_STOPPED_APPLICATION_EXIT: u64 = 0x2002_0026;

/// Terminate the emulator with `code`.
///
/// This profile has no `isa-debug-exit` device — that is x86-only — so the exit
/// goes through Arm semihosting, which QEMU implements when launched with
/// `-semihosting`. The exit status QEMU reports is the second field, so the
/// launcher maps it back to the kernel's own code.
pub fn debug_exit(code: u32) {
    let block = [ADP_STOPPED_APPLICATION_EXIT, code as u64];
    // SAFETY: `hlt #0xf000` is the AArch64 semihosting call. With semihosting
    // disabled it raises an exception rather than corrupting state, and the
    // gate always enables it.
    unsafe {
        core::arch::asm!(
            "hlt #0xf000",
            in("x0") SEMIHOSTING_SYS_EXIT,
            in("x1") block.as_ptr(),
            options(nostack),
        );
    }
}

/// The exception level the CPU is currently executing at.
///
/// Stage-0 is entered by UEFI at EL1 on the admitted machines and does not drop
/// levels, so EL1 is expected. Reporting it makes that an observation rather
/// than an assumption: a kernel that came up at EL2 would appear to work until
/// the first EL0 transition.
pub fn exception_level() -> u64 {
    let current_el: u64;
    // SAFETY: `CurrentEL` is a side-effect-free read available at every level.
    unsafe {
        core::arch::asm!("mrs {}, currentel", out(reg) current_el,
             options(nomem, nostack, preserves_flags));
    }
    // CurrentEL holds the level in bits 2..4.
    (current_el >> 2) & 0b11
}

/// The live translation configuration: MMU, data cache, and instruction cache
/// enables from `SCTLR_EL1`, and the two address-space sizes from `TCR_EL1`.
///
/// Read back from the CPU rather than recomputed from what stage-0 intended, so
/// a boot confirms the configuration the hardware actually accepted.
pub fn translation_config() -> TranslationConfig {
    let (sctlr, tcr): (u64, u64);
    // SAFETY: both are side-effect-free EL1 system-register reads.
    unsafe {
        core::arch::asm!("mrs {}, sctlr_el1", out(reg) sctlr,
             options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, tcr_el1", out(reg) tcr,
             options(nomem, nostack, preserves_flags));
    }
    TranslationConfig {
        // SCTLR_EL1: M (bit 0), C (bit 2), I (bit 12).
        mmu_enabled: sctlr & 1 != 0,
        data_cache_enabled: (sctlr >> 2) & 1 != 0,
        instruction_cache_enabled: (sctlr >> 12) & 1 != 0,
        low_address_size: (tcr & 0x3f) as u8,
        high_address_size: ((tcr >> 16) & 0x3f) as u8,
    }
}

/// The live translation configuration reported by [`translation_config`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranslationConfig {
    pub mmu_enabled: bool,
    pub data_cache_enabled: bool,
    pub instruction_cache_enabled: bool,
    /// `TCR_EL1.T0SZ`: the low half's address-size shift.
    pub low_address_size: u8,
    /// `TCR_EL1.T1SZ`: the high half's address-size shift.
    pub high_address_size: u8,
}

/// `CPACR_EL1.FPEN`: do not trap FP/SIMD at EL0 or EL1.
const CPACR_FPEN_NO_TRAP: u64 = 0b11 << 20;

/// Establish the SIMD/floating-point baseline the compiler assumes.
///
/// Firmware may hand off with FP/SIMD trapping at EL1. Rust code for this
/// target emits SIMD, so this must run before any ordinary Rust call in the
/// kernel entry path.
///
/// # Safety
///
/// Must run exactly once at kernel entry, before any code that may use SIMD.
pub unsafe fn init_simd_baseline() {
    unsafe {
        core::arch::asm!(
            "mrs {tmp}, cpacr_el1",
            "orr {tmp}, {tmp}, {fpen}",
            "msr cpacr_el1, {tmp}",
            "isb",
            tmp = out(reg) _,
            fpen = in(reg) CPACR_FPEN_NO_TRAP,
            options(nomem, nostack, preserves_flags),
        );
    }
}
