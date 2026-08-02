//! The AArch64 architecture mechanisms.
//!
//! P1 establishes this module as the second implementation of the architecture
//! boundary so architecture-neutral kernel code can be built for a non-x86
//! target. The mechanisms below are declared with their real signatures and
//! panic when invoked: P2 (`just aarch64_qemu_check`) implements EL1/EL0 entry,
//! translation tables, GIC interrupt delivery, the generic timer, PL011
//! diagnostics, and `svc` syscall entry.
//!
//! Nothing here may be treated as evidence that AArch64 runs. Building this
//! module proves only that neutral code names no x86 mechanism; it does not
//! boot, schedule, or execute a component.

pub mod boot;
pub mod context;
pub mod cpu;
pub mod paging;
pub mod trap;
pub mod uart;

/// The generic timer, named `apic` to match the boundary's timer slot until P2
/// renames it on both architectures.
pub mod apic {
    /// Program the periodic timer at `hz`.
    pub fn init(_hz: u64) {
        unimplemented!("aarch64 generic timer: implemented by P2")
    }

    /// Acknowledge the current interrupt at the interrupt controller.
    pub fn end_of_interrupt() {
        unimplemented!("aarch64 GIC end-of-interrupt: implemented by P2")
    }

    /// The calibrated per-tick count, for diagnostics.
    pub fn timer_count() -> u32 {
        unimplemented!("aarch64 generic timer: implemented by P2")
    }
}

/// Exception vectors and privilege-level configuration.
pub mod interrupts {
    /// Vector numbers the neutral kernel names. AArch64 routes through
    /// exception vectors and GIC interrupt IDs rather than an IDT; P2 fixes the
    /// concrete numbering.
    pub const TIMER_VECTOR: u8 = 0x20;
    pub const KEYBOARD_VECTOR: u8 = 0x21;
    pub const SYSCALL_VECTOR: u8 = 0x80;

    /// Install the exception vector table.
    pub fn init() {
        unimplemented!("aarch64 exception vectors: implemented by P2")
    }
}

/// Privileged-mode stack and descriptor setup. AArch64 has no GDT/TSS; the
/// kernel entry stack is selected by SPSel and `SP_EL1`. This module keeps the
/// same name until P2 gives the boundary an architecture-neutral one.
pub mod gdt {
    /// Selector values the saved user frame carries. AArch64 has no segment
    /// selectors; `SPSR_EL1` encodes the return exception level instead.
    pub const USER_CODE_SELECTOR: u16 = 0;
    pub const USER_DATA_SELECTOR: u16 = 0;

    pub fn init() {
        unimplemented!("aarch64 privileged stack setup: implemented by P2")
    }

    /// Set the stack pointer used when entering the kernel from userspace.
    pub fn set_rsp0(_sp: u64) {
        unimplemented!("aarch64 SP_EL1 setup: implemented by P2")
    }

    pub fn rsp0() -> u64 {
        unimplemented!("aarch64 SP_EL1 setup: implemented by P2")
    }
}
