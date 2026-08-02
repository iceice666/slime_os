//! x86-64 CPU control mechanisms: port I/O, interrupt masking, idle, and the
//! QEMU debug-exit device.
//!
//! These are the ISA primitives neutral kernel code needs. Every `in`/`out`,
//! `cli`/`sti`, and `hlt` in the kernel funnels through this module so no
//! architecture-neutral file names an x86 instruction or register.

/// Write a byte to an I/O port.
///
/// # Safety
///
/// Port I/O is a privileged side effect on whatever device decodes `port`; the
/// caller must own that device and the write must be valid for it.
pub unsafe fn outb(port: u16, value: u8) {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

/// Write a 16-bit word to an I/O port.
///
/// # Safety
///
/// As [`outb`].
pub unsafe fn outw(port: u16, value: u16) {
    unsafe {
        core::arch::asm!(
            "out dx, ax",
            in("dx") port,
            in("ax") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

/// Write a 32-bit dword to an I/O port.
///
/// # Safety
///
/// As [`outb`].
pub unsafe fn outl(port: u16, value: u32) {
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") port,
            in("eax") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

/// Read a byte from an I/O port.
///
/// # Safety
///
/// As [`outb`]; a read can also have device side effects (draining a FIFO).
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!(
            "in al, dx",
            out("al") value,
            in("dx") port,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}

/// Read a machine-specific register.
///
/// # Safety
///
/// `msr` must name an MSR that exists on this CPU; reading an absent MSR
/// raises `#GP`.
pub unsafe fn read_msr(msr: u32) -> u64 {
    let (low, high): (u32, u32);
    unsafe {
        core::arch::asm!("rdmsr", in("ecx") msr, out("edx") high, out("eax") low,
             options(nomem, nostack, preserves_flags));
    }
    ((high as u64) << 32) | low as u64
}

/// Write a machine-specific register.
///
/// # Safety
///
/// `msr` must exist and `value` must be a legal setting for it; an illegal
/// write raises `#GP` or silently reconfigures the CPU.
pub unsafe fn write_msr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    unsafe {
        core::arch::asm!("wrmsr", in("ecx") msr, in("edx") high, in("eax") low,
             options(nostack, preserves_flags));
    }
}

/// Whether maskable interrupts are currently enabled (RFLAGS.IF).
pub fn interrupts_enabled() -> bool {
    let flags: u64;
    // SAFETY: reading RFLAGS through the stack is side-effect free. `pushfq`
    // and `pop` touch the stack, so `nostack` must not be set here.
    unsafe {
        core::arch::asm!("pushfq", "pop {}", out(reg) flags, options(nomem, preserves_flags));
    }
    flags & (1 << 9) != 0
}

/// Mask maskable interrupts.
///
/// # Safety
///
/// Leaving interrupts masked stalls timer preemption and device wakes; the
/// caller must re-enable them on every path out.
pub unsafe fn disable_interrupts() {
    unsafe {
        core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
    }
}

/// Unmask maskable interrupts.
///
/// # Safety
///
/// The interrupt table must already be installed, and the caller must not be
/// holding a lock an interrupt handler takes.
pub unsafe fn enable_interrupts() {
    unsafe {
        core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
    }
}

/// Run `f` with maskable interrupts masked, restoring the previous mask state.
///
/// Interrupts are re-enabled only if they were enabled on entry, so nesting
/// this inside an interrupt handler (which already runs with IF clear through
/// an interrupt gate) does not silently unmask.
pub fn without_interrupts<T>(f: impl FnOnce() -> T) -> T {
    let enabled = interrupts_enabled();
    // SAFETY: interrupts are restored to their entry state before returning.
    unsafe { disable_interrupts() };
    let result = f();
    if enabled {
        // SAFETY: interrupts were enabled on entry, so re-enabling restores
        // the caller's state rather than unmasking under a handler.
        unsafe { enable_interrupts() };
    }
    result
}

/// Enable interrupts and idle until the next one arrives, as one step.
///
/// `sti` defers its effect by one instruction on x86, so a pending wake cannot
/// slip between unmasking and halting. Callers rely on that to park without a
/// lost-wakeup race.
///
/// # Safety
///
/// As [`enable_interrupts`]: the interrupt table must be installed.
pub unsafe fn enable_interrupts_and_wait() {
    unsafe {
        core::arch::asm!("sti; hlt", options(nomem, nostack, preserves_flags));
    }
}

/// Idle until the next interrupt without changing the interrupt mask.
pub fn wait_for_interrupt() {
    // SAFETY: `hlt` only parks the CPU; it is always valid in ring 0.
    unsafe {
        core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
    }
}

/// Raise a debug breakpoint trap and return.
///
/// # Safety
///
/// The interrupt table must install a `#BP` handler that returns; otherwise
/// this escalates instead of resuming.
pub unsafe fn breakpoint() {
    unsafe {
        core::arch::asm!("int3", options(nostack, preserves_flags));
    }
}

/// I/O port of the QEMU `isa-debug-exit` device this profile launches with.
const DEBUG_EXIT_PORT: u16 = 0xf4;

/// Terminate the emulator with `code`, if a debug-exit device is present.
pub fn debug_exit(code: u32) {
    // SAFETY: the launch profile wires `isa-debug-exit` at this port; on
    // hardware without it the write is discarded.
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") DEBUG_EXIT_PORT,
            in("eax") code,
            options(nomem, nostack, preserves_flags),
        );
    }
}

/// Establish the SIMD/floating-point baseline the compiler assumes.
///
/// UEFI may hand off with `CR0.EM` set and `CR4.OSFXSR` clear. Rust code for
/// this target emits SSE2, so this must run before any ordinary Rust call in
/// the kernel entry path.
///
/// # Safety
///
/// Must run exactly once at kernel entry, before any code that may use SSE.
pub unsafe fn init_simd_baseline() {
    unsafe {
        core::arch::asm!(
            "mov rax, cr0",
            "and rax, {clear_em}",
            "or rax, {set_mp}",
            "mov cr0, rax",
            "mov rax, cr4",
            "or rax, {set_osfxsr}",
            "mov cr4, rax",
            clear_em = const !(1u64 << 2),
            set_mp = const 1u64 << 1,
            set_osfxsr = const (1u64 << 9) | (1u64 << 10),
            out("rax") _,
            options(nostack),
        );
    }
}
