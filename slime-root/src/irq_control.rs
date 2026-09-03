//! Trigger-mode-aware IRQ handler acquisition.
//!
//! `seL4_IRQControl_Get` exists on every architecture but cannot state trigger
//! mode, and the invocation that can is architecture-specific: AArch64 and
//! RISC-V have `seL4_IRQControl_GetTrigger`, taking the interrupt number and an
//! edge-triggered flag, while x86 pc99 has `seL4_IRQControl_GetIOAPIC`, taking
//! an IOAPIC index, a pin, level and polarity flags, and a user-interrupt
//! index.
//!
//! Both callers in this crate want the same thing — "give me a handler for this
//! interrupt, level-triggered or not" — so the translation from that intent to
//! the local invocation lives here once.

/// Largest interrupt this root task can ask x86 pc99 for.
///
/// `irq_user_max - irq_user_min` from `deps/sel4/include/plat/pc99/plat/machine.h`:
/// `int_irq_user_min` is `IRQ_INT_OFFSET + PIC_IRQ_LINES` (0x20 + 16) and
/// `int_irq_user_max` is 155, both then offset by `IRQ_INT_OFFSET`. The kernel
/// range-checks the request against this bound before allocating a vector
/// (`deps/sel4/src/arch/x86/object/interrupt.c`), so exceeding it is a
/// `seL4_RangeError` rather than a silent misroute.
#[cfg(target_arch = "x86_64")]
pub const MAX_USER_IRQ: sel4::Word = 155 - (0x20 + 16);

/// Claim an IRQ handler capability for `irq` into `destination`.
///
/// `level_triggered` is the interrupt's own behavior: a virtio-mmio device and
/// the architected timer both hold their line asserted until the condition is
/// cleared, so acknowledging the handler before clearing that condition
/// re-fires immediately. The caller owns that ordering.
pub fn acquire_handler(
    irq: sel4::Word,
    level_triggered: bool,
    destination: &sel4::AbsoluteCPtr,
) -> Result<(), sel4::Error> {
    let irq_control = sel4::init_thread::slot::IRQ_CONTROL.cap();
    #[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
    {
        irq_control.irq_control_get_trigger(irq, !level_triggered, destination)
    }
    #[cfg(target_arch = "x86_64")]
    {
        if irq > MAX_USER_IRQ {
            return Err(sel4::Error::RangeError);
        }
        // pc99 addresses an interrupt by (IOAPIC, pin) and allocates the CPU
        // vector itself: the invocation's last argument is a *0-based index
        // into the user-interrupt range*, which the kernel offsets by
        // `irq_user_min` and then by `IRQ_INT_OFFSET` to obtain the vector
        // (`invokeIssueIRQHandlerIOAPIC` in
        // `deps/sel4/src/arch/x86/object/interrupt.c`). libsel4's XML names
        // that argument `vector`, which describes what the kernel derives from
        // it rather than what a caller supplies.
        //
        // Using `irq` for both the pin and that index keeps the mapping
        // injective, so two interrupts can never share a vector.
        //
        // IOAPIC 0 is the only one this profile admits: the pinned kernel
        // configuration builds with `MAX_NUM_IOAPIC = 1` and
        // `KERNEL_IRQ_CONTROLLER = IOAPIC`. A machine whose ACPI tables
        // describe more than one is a different platform profile, and H1 owns
        // discovering the real topology rather than this root task guessing it.
        //
        // Polarity 0 is active-high, matching the ISA-compatible pins QEMU q35
        // routes.
        const IOAPIC: sel4::Word = 0;
        const ACTIVE_HIGH: sel4::Word = 0;
        irq_control.irq_control_get_ioapic(
            IOAPIC,
            irq,
            sel4::Word::from(level_triggered),
            ACTIVE_HIGH,
            irq,
            destination,
        )
    }
}
