//! The AArch64 architecture mechanisms.
//!
//! P2.1 established the EL1/MMU/PL011 boot path. P2.2 installs the exception
//! vector table, synchronous-fault decoding, `svc` entry, and DAIF/idle
//! mechanisms. EL0 task scheduling, GIC/timer delivery, and devices remain in
//! the later P2 slices.

pub mod boot;
pub mod context;
pub mod cpu;
pub mod paging;
pub mod trap;
pub mod uart;

/// The generic timer, named `apic` to match the boundary's timer slot until
/// P2.4 renames it on both architectures.
pub mod apic {
    /// Program the periodic timer at `hz`.
    pub fn init(_hz: u64) {
        unimplemented!("aarch64 generic timer: implemented by P2.4")
    }

    /// Acknowledge the current interrupt at the interrupt controller.
    pub fn end_of_interrupt() {
        unimplemented!("aarch64 GIC end-of-interrupt: implemented by P2.4")
    }

    /// The calibrated per-tick count, for diagnostics.
    pub fn timer_count() -> u32 {
        unimplemented!("aarch64 generic timer: implemented by P2.4")
    }
}

/// Exception vectors and privilege-level configuration.
pub mod interrupts {
    /// Vector numbers the neutral kernel names. IRQ routing is supplied by
    /// P2.4; synchronous exceptions dispatch from `ESR_EL1` instead.
    pub const TIMER_VECTOR: u8 = 0x20;
    pub const KEYBOARD_VECTOR: u8 = 0x21;
    pub const SYSCALL_VECTOR: u8 = 0x80;

    /// Install the architected EL1 vector table.
    pub fn init() {
        super::trap::install();
    }
}

/// Privileged-mode stack and descriptor setup. AArch64 has no GDT/TSS; the
/// kernel entry stack is selected by SPSel and `SP_EL1`. This module keeps the
/// same name until P2.3 gives the boundary an architecture-neutral one.
pub mod gdt {
    /// Selector values the saved user frame carries. AArch64 has no segment
    /// selectors; `SPSR_EL1` encodes the return exception level instead.
    pub const USER_CODE_SELECTOR: u16 = 0;
    pub const USER_DATA_SELECTOR: u16 = 0;

    pub fn init() {
        unimplemented!("aarch64 privileged stack setup: implemented by P2.3")
    }

    /// Set the stack pointer used when entering the kernel from userspace.
    pub fn set_rsp0(_sp: u64) {
        unimplemented!("aarch64 SP_EL1 setup: implemented by P2.3")
    }

    pub fn rsp0() -> u64 {
        unimplemented!("aarch64 SP_EL1 setup: implemented by P2.3")
    }
}
