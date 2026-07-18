//! Local APIC and its timer.
//!
//! Bring-up sequence ([`init`]):
//!   1. Mask both legacy 8259 PICs so no stray IRQ reaches an unhandled vector.
//!   2. Map the Local APIC MMIO page cache-disabled and enable the LAPIC.
//!   3. Calibrate the LAPIC timer against the PIT (a fixed-frequency reference).
//!   4. Program the LAPIC timer periodic at the requested frequency.
//!
//! The LAPIC is memory-mapped; we reach its registers through the HHDM, mapping
//! the page uncached because it is device memory, not RAM.

use crate::memory::vmm::{self, MapError, PTE_CACHE_DISABLE, PTE_NO_EXECUTE, PTE_WRITABLE};
use crate::memory::{PhysAddr, VirtAddr};

// --- Local APIC register offsets (bytes from the LAPIC base) ---
const REG_SPURIOUS: usize = 0xF0;
const REG_EOI: usize = 0xB0;
const REG_LVT_TIMER: usize = 0x320;
const REG_TIMER_INITIAL: usize = 0x380;
const REG_TIMER_CURRENT: usize = 0x390;
const REG_TIMER_DIVIDE: usize = 0x3E0;

/// SVR bit 8: software-enable the APIC.
const SVR_ENABLE: u32 = 1 << 8;
/// Spurious-interrupt vector. Must be handled but needs no EOI.
const SPURIOUS_VECTOR: u32 = 0xFF;
/// LVT timer bit 17: periodic mode.
const LVT_TIMER_PERIODIC: u32 = 1 << 17;
/// LVT bit 16: masked.
const LVT_MASKED: u32 = 1 << 16;
/// Timer divide configuration: divide by 16 (0b1010 across the split field).
const TIMER_DIVIDE_16: u32 = 0b1010;

/// `IA32_APIC_BASE` MSR: holds the LAPIC physical base and the global enable bit.
const IA32_APIC_BASE: u32 = 0x1B;
const APIC_BASE_GLOBAL_ENABLE: u64 = 1 << 11;

/// PIT input frequency in Hz (fixed by hardware).
const PIT_FREQUENCY: u32 = 1_193_182;

/// LAPIC MMIO base as a virtual (HHDM) address, set in [`init`].
static mut LAPIC_VIRT: u64 = 0;
/// Calibrated LAPIC timer count for one tick at the requested frequency.
static mut TIMER_COUNT: u32 = 0;

/// Read an LAPIC register.
fn lapic_read(reg: usize) -> u32 {
    // SAFETY: `LAPIC_VIRT` is a mapped MMIO page after `init`; register offsets
    // are 4-byte aligned and in range.
    unsafe { core::ptr::read_volatile((LAPIC_VIRT as usize + reg) as *const u32) }
}

/// Write an LAPIC register.
fn lapic_write(reg: usize, value: u32) {
    // SAFETY: as `lapic_read`; MMIO writes are volatile and in range.
    unsafe { core::ptr::write_volatile((LAPIC_VIRT as usize + reg) as *mut u32, value) }
}

/// Signal end-of-interrupt to the LAPIC. Call from every LAPIC interrupt
/// handler (except the spurious vector).
pub fn end_of_interrupt() {
    lapic_write(REG_EOI, 0);
}

/// Read a model-specific register.
fn read_msr(msr: u32) -> u64 {
    let (hi, lo): (u32, u32);
    // SAFETY: `rdmsr` is a privileged ring-0 read; `IA32_APIC_BASE` always exists.
    unsafe {
        core::arch::asm!("rdmsr", in("ecx") msr, out("edx") hi, out("eax") lo,
            options(nomem, nostack, preserves_flags));
    }
    ((hi as u64) << 32) | lo as u64
}

/// Write a model-specific register.
fn write_msr(msr: u32, value: u64) {
    let hi = (value >> 32) as u32;
    let lo = value as u32;
    // SAFETY: `wrmsr` is a privileged ring-0 write to a known-good MSR/value.
    unsafe {
        core::arch::asm!("wrmsr", in("ecx") msr, in("edx") hi, in("eax") lo,
            options(nomem, nostack, preserves_flags));
    }
}

/// Byte out to an I/O port.
fn outb(port: u16, val: u8) {
    // SAFETY: ring-0 port I/O to a fixed legacy port.
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") val,
            options(nomem, nostack, preserves_flags));
    }
}

/// Byte in from an I/O port.
fn inb(port: u16) -> u8 {
    let val: u8;
    // SAFETY: ring-0 port I/O from a fixed legacy port.
    unsafe {
        core::arch::asm!("in al, dx", out("al") val, in("dx") port,
            options(nomem, nostack, preserves_flags));
    }
    val
}

/// Mask every line on both 8259 PICs so no legacy IRQ is delivered. We drive
/// interrupts entirely through the APIC, so the PICs must stay silent.
fn mask_pics() {
    // ICW1: begin init, expect ICW4. ICW2: vector offsets (0x20 / 0x28) so any
    // spurious IRQ lands on a distinct, benign vector rather than an exception
    // vector. ICW3: master/slave cascade on IRQ2. ICW4: 8086 mode.
    outb(0x20, 0x11);
    outb(0xA0, 0x11);
    outb(0x21, 0x20);
    outb(0xA1, 0x28);
    outb(0x21, 0x04);
    outb(0xA1, 0x02);
    outb(0x21, 0x01);
    outb(0xA1, 0x01);
    // Mask all lines.
    outb(0x21, 0xFF);
    outb(0xA1, 0xFF);
}

/// Busy-wait `us` microseconds using PIT channel 2 in one-shot (mode 0).
fn pit_wait_us(us: u32) {
    let count = ((PIT_FREQUENCY as u64 * us as u64) / 1_000_000) as u16;

    // Enable channel-2 gate (bit0) without driving the speaker (bit1 clear).
    let port61 = (inb(0x61) & 0xFC) | 0x01;
    outb(0x61, port61);

    // Channel 2, access lo/hi, mode 0 (interrupt on terminal count), binary.
    outb(0x43, 0b1011_0000);
    outb(0x42, count as u8);
    outb(0x42, (count >> 8) as u8);

    // Retrigger the gate so the counter reloads.
    let p = inb(0x61) & 0xFE;
    outb(0x61, p);
    outb(0x61, p | 0x01);

    // Poll OUT2 (port 0x61 bit 5): set when the count reaches terminal count.
    while inb(0x61) & 0x20 == 0 {
        core::hint::spin_loop();
    }
}

/// Bring up the Local APIC and program its timer to fire `hz` times per second.
pub fn init(hz: u64) {
    mask_pics();

    // Locate and enable the LAPIC. The base's low 12 bits are flags; the frame
    // address is bits 12.. .
    let base_msr = read_msr(IA32_APIC_BASE);
    let lapic_phys = PhysAddr(base_msr & 0x000f_ffff_ffff_f000);
    write_msr(IA32_APIC_BASE, base_msr | APIC_BASE_GLOBAL_ENABLE);

    // Map the LAPIC MMIO page uncached at its HHDM address. Limine may already
    // map this region; treat an existing mapping as success.
    let lapic_virt = lapic_phys.to_virt();
    // SAFETY: mapping device MMIO uncached and non-executable at its HHDM VA.
    let mapped = unsafe {
        vmm::map_page(
            lapic_virt,
            lapic_phys,
            PTE_WRITABLE | PTE_CACHE_DISABLE | PTE_NO_EXECUTE,
        )
    };
    match mapped {
        Ok(()) | Err(MapError::AlreadyMapped) => {}
        Err(e) => panic!("apic: failed to map LAPIC MMIO: {e:?}"),
    }
    // SAFETY: single-threaded bring-up; `LAPIC_VIRT` is set once here.
    unsafe { store_lapic_virt(lapic_virt) };

    // Software-enable the APIC and route spurious interrupts to a benign vector.
    lapic_write(REG_SPURIOUS, SVR_ENABLE | SPURIOUS_VECTOR);

    // Calibrate: run the timer flat-out for a known interval, see how far it
    // counts, then scale to the requested frequency.
    lapic_write(REG_TIMER_DIVIDE, TIMER_DIVIDE_16);
    lapic_write(REG_LVT_TIMER, LVT_MASKED); // masked during calibration
    lapic_write(REG_TIMER_INITIAL, u32::MAX);

    pit_wait_us(10_000); // 10 ms

    let elapsed = u32::MAX - lapic_read(REG_TIMER_CURRENT);
    // Counts per second = elapsed / 0.01s; per tick = that / hz.
    let per_second = elapsed as u64 * 100;
    let count = (per_second / hz).max(1) as u32;
    // SAFETY: single-threaded bring-up; `TIMER_COUNT` is set once here.
    unsafe { store_timer_count(count) };

    // Program periodic mode on the timer vector and start counting.
    lapic_write(
        REG_LVT_TIMER,
        crate::interrupts::TIMER_VECTOR as u32 | LVT_TIMER_PERIODIC,
    );
    lapic_write(REG_TIMER_DIVIDE, TIMER_DIVIDE_16);
    lapic_write(REG_TIMER_INITIAL, count);
}

/// The calibrated per-tick count (for diagnostics/tests). Zero before [`init`].
pub fn timer_count() -> u32 {
    // SAFETY: plain read of a value only written once during bring-up.
    unsafe { core::ptr::addr_of!(TIMER_COUNT).read() }
}

/// # Safety
/// Single-threaded bring-up only.
unsafe fn store_lapic_virt(v: VirtAddr) {
    unsafe { core::ptr::addr_of_mut!(LAPIC_VIRT).write(v.as_u64()) };
}

/// # Safety
/// Single-threaded bring-up only.
unsafe fn store_timer_count(c: u32) {
    unsafe { core::ptr::addr_of_mut!(TIMER_COUNT).write(c) };
}
