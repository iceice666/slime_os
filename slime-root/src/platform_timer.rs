//! Real seL4 timer-IRQ backing for [`crate::timer::PlatformTimer`].
//!
//! # Register scheme
//!
//! `sel4/config/qemu-arm-virt.cmake` builds seL4 with
//! `KernelArmHypervisorSupport ON`, so the kernel itself runs at EL2 and uses
//! the EL2 hypervisor physical timer (`CNTHP_*`, PPI 26 on this platform —
//! `KERNEL_TIMER_IRQ` in the generated `devices_gen.h`) for its own non-MCS
//! scheduling tick; that IRQ is claimed by `setIRQState(IRQTimer, ...)` at
//! kernel boot (`deps/sel4/src/arch/arm/kernel/boot.c::init_irqs`) and is
//! never available to a root task. The EL1 virtual timer (`CNTV_*`, PPI 27,
//! `INTERRUPT_VTIMER_EVENT`) is *also* claimed unconditionally at the same
//! boot step — `setIRQState(IRQReserved, ...)` runs whenever
//! `CONFIG_ARM_HYPERVISOR_SUPPORT` is compiled in, independent of whether any
//! VCPU is ever created, because it is reserved for VCPU virtual-timer
//! maintenance. That leaves exactly one architected-timer PPI seL4 does not
//! claim for itself under this configuration: the EL1 **physical** timer
//! (`CNTP_*`), non-secure, PPI 30 (`<GIC_PPI 14 ...>` in the platform device
//! tree, offset by the PPI base of 16). [`TIMER_IRQ`] is that IRQ number.
//!
//! Reading and writing `CNTPCT_EL0`/`CNTFRQ_EL0`/`CNTP_CVAL_EL0`/
//! `CNTP_CTL_EL0` directly from this EL0 root task requires the kernel to
//! grant PL0 access explicitly; `sel4/config/qemu-arm-virt.cmake` enables
//! exactly the two config options this scheme needs —
//! `KernelArmExportPCNTUser` (physical counter + frequency) and
//! `KernelArmExportPTMRUser` (physical timer control + compare) — and leaves
//! `KernelArmExportVCNTUser`/`KernelArmExportVTMRUser` off, since nothing
//! here touches the virtual timer registers. The kernel applies both grants
//! once, globally, at boot (`armv_init_user_access` in
//! `deps/sel4/src/arch/arm/armv/armv8-a/64/user_access.c`), so no
//! capability invocation is needed at runtime to unlock the registers
//! themselves — only to claim the IRQ and bind it to a notification.
//!
//! # What this establishes, and what it does not
//!
//! Acquiring [`PhysicalTimerAdapter`] proves the root task holds the one
//! architected-timer IRQ seL4 leaves for userspace on this platform, binds it
//! to a notification the root can wait on, and can read/program/acknowledge
//! the EL1 physical timer directly. It does **not** make the control registers
//! root-exclusive: `KernelArmExportPTMRUser` applies globally, so hostile EL0
//! component code can disable or overwrite the same `CNTP_*` comparator and
//! disrupt the root-brokered service without possessing C9.1 timer authority.
//! Closing that integrity wall requires a kernel/platform change. It also does
//! not establish temporal isolation, a CPU reservation, or any deadline
//! guarantee: this is a plain non-MCS one-shot compare timer, and a scheduling
//! delay of arbitrary length can still separate the compare condition becoming
//! true from this task next running.

#[cfg(target_arch = "riscv64")]
use crate::device::MappedGranule;
use crate::event::MonotonicInstant;
use crate::object_allocator::{AllocError, ObjectAllocator};
use crate::timer::PlatformTimer;

/// Userspace timer interrupt for the selected QEMU `virt` platform.
#[cfg(target_arch = "aarch64")]
pub const TIMER_IRQ: sel4::Word = 30;
#[cfg(target_arch = "riscv64")]
pub const TIMER_IRQ: sel4::Word = 11;

/// Badge minted onto the notification copy bound to the IRQ handler. The
/// notification object's own (unbadged, full-rights) capability is used to
/// wait/poll; without a distinct nonzero badge on the *sender* side, a
/// woken/polled badge of `0` would be indistinguishable from "nothing
/// pending yet".
const SIGNAL_BADGE: sel4::Badge = 1;

#[cfg(target_arch = "aarch64")]
/// `CNTP_CTL_EL0.ENABLE`: the timer counts down to (and compares against)
/// `CNTP_CVAL_EL0` and can assert its interrupt line.
const CNTP_CTL_ENABLE: u64 = 1 << 0;

/// Failure to acquire and wire the timer IRQ. Every step names exactly which
/// seL4 invocation or allocator call failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformTimerSetupError {
    /// A CSlot or the notification object itself could not be allocated.
    Allocator(AllocError),
    /// Minting the badged sender copy of the notification failed.
    MintSignalCap(sel4::Error),
    /// `seL4_IRQControl_GetTrigger` for [`TIMER_IRQ`] failed — most likely
    /// because it is already active (claimed by the kernel or a prior boot
    /// attempt) rather than free for this root task.
    IrqAcquire(sel4::Error),
    /// `seL4_IRQHandler_SetNotification` failed to bind the acquired handler
    /// to the badged notification copy.
    BindNotification(sel4::Error),
}

/// Failure while operating the platform timer after acquisition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformTimerAckError {
    /// The RV64 adapter was used before its RTC register page was attached.
    RegistersUnavailable,
    /// A device-register offset was outside the mapped granule.
    RegisterAccess,
    /// seL4 refused the IRQ acknowledgement.
    Acknowledge(sel4::Error),
}

/// A live, IRQ-backed timer source this root task owns.
pub struct PhysicalTimerAdapter {
    irq_handler: sel4::cap::IrqHandler,
    /// The unbadged, full-rights notification capability.
    notification: sel4::cap::Notification,
    #[cfg(target_arch = "riscv64")]
    registers: Option<MappedGranule>,
}

impl PhysicalTimerAdapter {
    /// Claim [`TIMER_IRQ`], create and bind a notification, and return the
    /// live adapter. Every seL4 invocation and allocator call along the way
    /// returns a typed [`PlatformTimerSetupError`] instead of panicking.
    pub fn acquire(allocator: &mut ObjectAllocator) -> Result<Self, PlatformTimerSetupError> {
        let notification_slot = allocator
            .allocate_fixed::<sel4::cap_type::Notification>()
            .map_err(PlatformTimerSetupError::Allocator)?;
        let signal_slot = allocator
            .reserve_slot::<sel4::cap_type::Notification>()
            .map_err(PlatformTimerSetupError::Allocator)?;
        let irq_handler_slot = allocator
            .reserve_slot::<sel4::cap_type::IrqHandler>()
            .map_err(PlatformTimerSetupError::Allocator)?;

        let root_cnode = sel4::init_thread::slot::CNODE.cap();

        // A badged sender-only copy of the notification, bound to the IRQ
        // handler below. Signals through it accumulate a badge distinct from
        // the "nothing pending" `0` a `Poll`/`Wait` on the original
        // (unbadged) capability reports.
        root_cnode
            .absolute_cptr(signal_slot.cptr())
            .mint(
                &root_cnode.absolute_cptr(notification_slot.cptr()),
                sel4::CapRightsBuilder::none().write(true).build(),
                SIGNAL_BADGE,
            )
            .map_err(PlatformTimerSetupError::MintSignalCap)?;

        // Level-triggered: the architected timer's interrupt line stays
        // asserted for as long as the compare condition holds, exactly like
        // seL4's own generic-timer driver treats it
        // (`deps/sel4/include/drivers/timer/arm_generic.h`).
        sel4::init_thread::slot::IRQ_CONTROL
            .cap()
            .irq_control_get_trigger(
                TIMER_IRQ,
                false,
                &root_cnode.absolute_cptr(irq_handler_slot.cptr()),
            )
            .map_err(PlatformTimerSetupError::IrqAcquire)?;

        irq_handler_slot
            .cap()
            .irq_handler_set_notification(signal_slot.cap())
            .map_err(PlatformTimerSetupError::BindNotification)?;

        Ok(Self {
            irq_handler: irq_handler_slot.cap(),
            notification: notification_slot.cap(),
            #[cfg(target_arch = "riscv64")]
            registers: None,
        })
    }

    /// Attach the mapped Goldfish RTC register page used by RV64 QEMU.
    #[cfg(target_arch = "riscv64")]
    pub fn attach_registers(&mut self, registers: MappedGranule) {
        self.registers = Some(registers);
    }

    /// The capability a caller waits or polls on to observe delivery. Full
    /// rights, but signalling only ever happens through the badged copy
    /// bound to the IRQ handler in [`Self::acquire`], never through this one.
    pub const fn notification(&self) -> sel4::cap::Notification {
        self.notification
    }

    /// The badge a genuine timer signal carries, as opposed to `0` for "no
    /// signal yet" on a [`seL4_Poll`]-style non-blocking check.
    pub const fn signal_badge(&self) -> sel4::Badge {
        SIGNAL_BADGE
    }

    /// Bind the timer IRQ's notification to the root service thread.
    ///
    /// With a bound Notification, a blocking `seL4_Recv` on the root service
    /// endpoint returns a badge-only wake when the timer fires. Requests and
    /// timer delivery therefore share one blocking point without a polling
    /// thread or lost-wake window.
    pub fn bind_to(&self, tcb: sel4::cap::Tcb) -> Result<(), sel4::Error> {
        tcb.tcb_bind_notification(self.notification)
    }

    /// Timer ticks per second for this platform's monotonic source.
    pub fn frequency_hz(&self) -> u64 {
        #[cfg(target_arch = "aarch64")]
        {
            read_cntfrq()
        }
        #[cfg(target_arch = "riscv64")]
        {
            1_000_000_000
        }
    }
}

impl PlatformTimer for PhysicalTimerAdapter {
    type Error = PlatformTimerAckError;

    fn monotonic_now(&mut self) -> Result<MonotonicInstant, Self::Error> {
        #[cfg(target_arch = "aarch64")]
        {
            Ok(MonotonicInstant(read_cntpct()))
        }
        #[cfg(target_arch = "riscv64")]
        {
            let registers = self
                .registers
                .ok_or(PlatformTimerAckError::RegistersUnavailable)?;
            Ok(MonotonicInstant(
                read_rtc(registers).ok_or(PlatformTimerAckError::RegisterAccess)?,
            ))
        }
    }

    fn program_deadline(&mut self, deadline: MonotonicInstant) -> Result<(), Self::Error> {
        #[cfg(target_arch = "aarch64")]
        {
            write_cntp_cval(deadline.0);
            write_cntp_ctl(CNTP_CTL_ENABLE);
        }
        #[cfg(target_arch = "riscv64")]
        program_rtc(
            self.registers
                .ok_or(PlatformTimerAckError::RegistersUnavailable)?,
            deadline.0,
        )
        .then_some(())
        .ok_or(PlatformTimerAckError::RegisterAccess)?;
        Ok(())
    }

    fn disarm_timer(&mut self) -> Result<(), Self::Error> {
        #[cfg(target_arch = "aarch64")]
        write_cntp_ctl(0);
        #[cfg(target_arch = "riscv64")]
        disarm_rtc(
            self.registers
                .ok_or(PlatformTimerAckError::RegistersUnavailable)?,
        )
        .then_some(())
        .ok_or(PlatformTimerAckError::RegisterAccess)?;
        Ok(())
    }

    fn acknowledge_timer_irq(&mut self) -> Result<(), Self::Error> {
        // `service_timer_source` calls this only after the next deadline
        // state (`program_deadline` or `disarm_timer`, both above) has
        // already been installed, so the device-level condition that raised
        // the interrupt is already cleared or superseded. The barrier below
        // makes that register write's effect on the timer hardware
        // observable before the interrupt controller is told the line is
        // clear; acknowledging first could let the kernel unmask a line the
        // device had not actually deasserted yet, which — being
        // level-triggered — would refire it immediately.
        #[cfg(target_arch = "aarch64")]
        isb();
        #[cfg(target_arch = "riscv64")]
        clear_rtc_interrupt(
            self.registers
                .ok_or(PlatformTimerAckError::RegistersUnavailable)?,
        )
        .then_some(())
        .ok_or(PlatformTimerAckError::RegisterAccess)?;
        self.irq_handler
            .irq_handler_ack()
            .map_err(PlatformTimerAckError::Acknowledge)
    }
}
#[cfg(target_arch = "riscv64")]
const RTC_TIME_LOW: usize = 0x00;
#[cfg(target_arch = "riscv64")]
const RTC_TIME_HIGH: usize = 0x04;
#[cfg(target_arch = "riscv64")]
const RTC_ALARM_LOW: usize = 0x08;
#[cfg(target_arch = "riscv64")]
const RTC_ALARM_HIGH: usize = 0x0c;
#[cfg(target_arch = "riscv64")]
const RTC_IRQ_ENABLED: usize = 0x10;
#[cfg(target_arch = "riscv64")]
const RTC_CLEAR_ALARM: usize = 0x14;
#[cfg(target_arch = "riscv64")]
const RTC_CLEAR_INTERRUPT: usize = 0x1c;

#[cfg(target_arch = "riscv64")]
fn read_rtc(registers: MappedGranule) -> Option<u64> {
    let low = registers.read32(RTC_TIME_LOW)? as u64;
    let high = registers.read32(RTC_TIME_HIGH)? as u64;
    Some(low | (high << 32))
}

#[cfg(target_arch = "riscv64")]
fn program_rtc(registers: MappedGranule, deadline: u64) -> bool {
    registers.write32(RTC_ALARM_HIGH, (deadline >> 32) as u32)
        && registers.write32(RTC_ALARM_LOW, deadline as u32)
        && registers.write32(RTC_IRQ_ENABLED, 1)
}

#[cfg(target_arch = "riscv64")]
fn disarm_rtc(registers: MappedGranule) -> bool {
    registers.write32(RTC_IRQ_ENABLED, 0)
        && registers.write32(RTC_CLEAR_ALARM, 1)
        && registers.write32(RTC_CLEAR_INTERRUPT, 1)
}

#[cfg(target_arch = "riscv64")]
fn clear_rtc_interrupt(registers: MappedGranule) -> bool {
    registers.write32(RTC_CLEAR_INTERRUPT, 1)
}

/// Reads the EL1 physical counter (`CNTPCT_EL0`), the free-running clock the
/// EL1 physical timer's compare register is measured against.
#[cfg(target_arch = "aarch64")]
#[inline]
fn read_cntpct() -> u64 {
    let value: u64;
    // SAFETY: `mrs` from `cntpct_el0` is a pure register read with no memory
    // or control-flow side effects. It is legal at EL0 because
    // `KernelArmExportPCNTUser` (set in `sel4/config/qemu-arm-virt.cmake`)
    // makes the kernel set `CNTKCTL_EL1.EL0PCTEN` for every core once at
    // boot, before any thread runs; without that bit this instruction traps
    // instead of completing.
    unsafe {
        core::arch::asm!(
            "mrs {value}, cntpct_el0",
            value = out(reg) value,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}
/// Reads `CNTFRQ_EL0`, the counter frequency firmware programmed for
/// [`read_cntpct`]'s domain.
#[cfg(target_arch = "aarch64")]
#[inline]
fn read_cntfrq() -> u64 {
    let value: u64;
    // SAFETY: `mrs` from `cntfrq_el0` is a pure register read with no memory
    // or control-flow side effects; EL0 access is granted by the same
    // `EL0PCTEN` bit as `cntpct_el0` — `KernelArmExportPCNTUser`'s own
    // description covers both registers.
    unsafe {
        core::arch::asm!(
            "mrs {value}, cntfrq_el0",
            value = out(reg) value,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}
/// Writes the EL1 physical timer's compare value (`CNTP_CVAL_EL0`).
#[cfg(target_arch = "aarch64")]
#[inline]
fn write_cntp_cval(value: u64) {
    // SAFETY: `msr` to `cntp_cval_el0` only changes this timer's own compare
    // register; it has no effect on memory this crate's Rust code observes
    // and cannot fault once `KernelArmExportPTMRUser` has granted
    // `CNTKCTL_EL1.EL0PTEN` (set once at boot before any thread runs — see
    // `sel4/config/qemu-arm-virt.cmake`).
    unsafe {
        core::arch::asm!(
            "msr cntp_cval_el0, {value}",
            value = in(reg) value,
            options(nomem, nostack, preserves_flags),
        );
    }
}
/// Writes the EL1 physical timer's control register (`CNTP_CTL_EL0`).
#[cfg(target_arch = "aarch64")]
#[inline]
fn write_cntp_ctl(value: u64) {
    // SAFETY: `msr` to `cntp_ctl_el0` only toggles this timer's own
    // ENABLE/IMASK bits; legal at EL0 under the same `EL0PTEN` grant as
    // `write_cntp_cval`, and has no effect on memory this crate's Rust code
    // observes.
    unsafe {
        core::arch::asm!(
            "msr cntp_ctl_el0, {value}",
            value = in(reg) value,
            options(nomem, nostack, preserves_flags),
        );
    }
}
/// Instruction Synchronization Barrier: forces every instruction before it
/// (in particular, the `msr` writes above) to complete and become visible to
/// the timer hardware before anything after it executes.
#[cfg(target_arch = "aarch64")]
#[inline]
fn isb() {
    // SAFETY: `isb` reads and writes no memory; it only orders instruction
    // execution and completion, which is exactly the ordering
    // `acknowledge_timer_irq` documents needing against the preceding
    // `program_deadline`/`disarm_timer` register write.
    unsafe {
        core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
    }
}
