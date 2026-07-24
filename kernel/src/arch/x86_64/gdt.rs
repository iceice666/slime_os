//! Global Descriptor Table and Task State Segment.
//!
//! Limine hands us a working GDT, but not a Task State Segment — and without a
//! TSS we cannot install an Interrupt Stack Table (IST) entry. The IST matters
//! for one specific failure: a fault that occurs while the current stack is
//! unusable (a kernel stack overflow, say) would fault again while the CPU
//! tries to push the exception frame, escalating straight to a triple fault and
//! a silent QEMU reset. Pointing the Double Fault handler at a known-good IST
//! stack turns that silent reset into a reported `#DF` — the CPU half of the
//! milestone's "faults are reported deterministically" exit condition.
//!
//! We build our own GDT (null, kernel code, kernel data, TSS) so we control the
//! selectors, load it, reload the segment registers, and `ltr` the TSS.
//!
//! State lives in `static mut`s initialized once in [`init`] during
//! single-threaded bring-up. Access goes through raw pointers (`addr_of_mut!`)
//! so we never form an aliasing reference to the mutable statics.

use core::mem::size_of;
use core::ptr::addr_of_mut;

/// IST slot (1..=7) reserved for the Double Fault handler's stack.
pub const DOUBLE_FAULT_IST_INDEX: u8 = 1;

/// GDT byte-offset selectors. RPL/TI bits are zero (ring 0, GDT).
pub const KERNEL_CODE_SELECTOR: u16 = 0x08;
pub const KERNEL_DATA_SELECTOR: u16 = 0x10;
pub const TSS_SELECTOR: u16 = 0x18;
pub const USER_CODE_SELECTOR: u16 = 0x28;
pub const USER_DATA_SELECTOR: u16 = 0x30;

/// Size of the dedicated Double Fault stack: 5 pages. Never used for anything
/// else, so it stays intact even when the normal kernel stack is corrupt.
const DF_STACK_SIZE: usize = 5 * 4096;

/// The Double Fault stack.
static mut DF_STACK: [u8; DF_STACK_SIZE] = [0; DF_STACK_SIZE];

/// The 64-bit Task State Segment. Only the IST pointer fields matter to us; the
/// rest stay zero. `packed` because the manual layout is not naturally aligned.
#[repr(C, packed)]
struct TaskStateSegment {
    reserved_0: u32,
    privilege_stack_table: [u64; 3],
    reserved_1: u64,
    interrupt_stack_table: [u64; 7],
    reserved_2: u64,
    reserved_3: u16,
    iomap_base: u16,
}

impl TaskStateSegment {
    const fn new() -> Self {
        Self {
            reserved_0: 0,
            privilege_stack_table: [0; 3],
            reserved_1: 0,
            interrupt_stack_table: [0; 7],
            reserved_2: 0,
            reserved_3: 0,
            // No I/O permission bitmap: point past the TSS limit.
            iomap_base: size_of::<TaskStateSegment>() as u16,
        }
    }
}

/// GDT: null + kernel code + kernel data + a 16-byte TSS descriptor, plus
/// ring-3 code/data descriptors as raw `u64`s laid out
/// `[null, kcode, kdata, tss_low, tss_high, ucode, udata]`.
#[repr(C, align(16))]
struct GlobalDescriptorTable {
    entries: [u64; 7],
}

static mut TSS: TaskStateSegment = TaskStateSegment::new();
static mut GDT: GlobalDescriptorTable = GlobalDescriptorTable {
    entries: [0, KERNEL_CODE, KERNEL_DATA, 0, 0, USER_CODE, USER_DATA],
};

// Access-byte / flag constants for the 64-bit code and data descriptors.
const ACCESS_PRESENT: u64 = 1 << 47;
const ACCESS_DPL0: u64 = 0 << 45;
const ACCESS_DPL3: u64 = 3 << 45;
const ACCESS_TYPE_SEGMENT: u64 = 1 << 44; // S=1 (code/data, not system)
const ACCESS_EXECUTABLE: u64 = 1 << 43; // code segment
const ACCESS_READ_WRITE: u64 = 1 << 41; // readable code / writable data
const FLAG_LONG_MODE: u64 = 1 << 53; // L=1: 64-bit code

/// Kernel 64-bit code segment descriptor.
const KERNEL_CODE: u64 = ACCESS_PRESENT
    | ACCESS_DPL0
    | ACCESS_TYPE_SEGMENT
    | ACCESS_EXECUTABLE
    | ACCESS_READ_WRITE
    | FLAG_LONG_MODE;

/// Kernel data segment descriptor.
const KERNEL_DATA: u64 = ACCESS_PRESENT | ACCESS_DPL0 | ACCESS_TYPE_SEGMENT | ACCESS_READ_WRITE;

/// User 64-bit code segment descriptor.
const USER_CODE: u64 = ACCESS_PRESENT
    | ACCESS_DPL3
    | ACCESS_TYPE_SEGMENT
    | ACCESS_EXECUTABLE
    | ACCESS_READ_WRITE
    | FLAG_LONG_MODE;

/// User data segment descriptor.
const USER_DATA: u64 = ACCESS_PRESENT | ACCESS_DPL3 | ACCESS_TYPE_SEGMENT | ACCESS_READ_WRITE;

/// Build the two 64-bit halves of a TSS system descriptor for the TSS at `base`.
fn tss_descriptor(base: u64) -> (u64, u64) {
    let limit = (size_of::<TaskStateSegment>() - 1) as u64;

    // Low half: limit[0:16], base[0:24], type/present, limit[16:20], base[24:32].
    let mut low: u64 = 0;
    low |= limit & 0xffff;
    low |= (base & 0xff_ffff) << 16;
    low |= 0b1001u64 << 40; // type: 64-bit TSS (available)
    low |= ACCESS_PRESENT;
    low |= ((limit >> 16) & 0xf) << 48;
    low |= ((base >> 24) & 0xff) << 56;

    // High half: base[32:64].
    (low, base >> 32)
}

/// The 10-byte GDTR operand for `lgdt`.
#[repr(C, packed)]
struct Gdtr {
    limit: u16,
    base: u64,
}

/// Initialize the GDT and TSS: install the Double Fault IST stack, load the
/// GDT, reload CS/data segments, and load the task register.
///
/// Must run before [`crate::interrupts::init`] so the IDT gates pick up our
/// code selector, and before any Double Fault can occur.
pub fn init() {
    // IST1 points at the *top* of the DF stack (stacks grow down).
    // SAFETY note: `addr_of_mut!` on a whole static forms no reference.
    let df_stack_top = addr_of_mut!(DF_STACK) as u64 + DF_STACK_SIZE as u64;
    // SAFETY: single-threaded bring-up; no other access to TSS is live.
    unsafe {
        addr_of_mut!(TSS.interrupt_stack_table[(DOUBLE_FAULT_IST_INDEX - 1) as usize])
            .write(df_stack_top);
    }
    crate::println!("[gdt] TSS stack set");

    let tss_base = addr_of_mut!(TSS) as u64;
    let (low, high) = tss_descriptor(tss_base);
    // SAFETY: single-threaded bring-up; no other access to GDT is live.
    unsafe {
        addr_of_mut!(GDT.entries[3]).write(low);
        addr_of_mut!(GDT.entries[4]).write(high);
        addr_of_mut!(GDT.entries[5]).write(USER_CODE);
        addr_of_mut!(GDT.entries[6]).write(USER_DATA);
    }
    crate::println!("[gdt] descriptors built");

    // SAFETY: taking the address of the mutable static; no reference formed.
    let gdt_base = unsafe { addr_of_mut!(GDT.entries) as u64 };
    let gdtr = Gdtr {
        limit: (size_of::<GlobalDescriptorTable>() - 1) as u16,
        base: gdt_base,
    };
    crate::println!("[gdt] GDTR ready");

    // SAFETY: `gdtr` describes the valid, live GDT we just built. We reload CS
    // via a far return and the data segments directly, then load the task
    // register with the TSS selector — all indexing descriptors we installed.
    unsafe {
        core::arch::asm!(
            "lgdt [{gdtr}]",
            gdtr = in(reg) core::ptr::addr_of!(gdtr),
            options(readonly, nostack, preserves_flags),
        );
        crate::println!("[gdt] lgdt loaded");
        // Reload CS with a far return: push new selector + return target.
        core::arch::asm!(
            "push {sel}",
            "lea {tmp}, [rip + 55f]",
            "push {tmp}",
            "retfq",
            "55:",
            sel = in(reg) KERNEL_CODE_SELECTOR as u64,
            tmp = lateout(reg) _,
            options(preserves_flags),
        );
        crate::println!("[gdt] CS reloaded");
        // Reload data segment registers.
        core::arch::asm!(
            "mov ds, {sel:x}",
            "mov es, {sel:x}",
            "mov ss, {sel:x}",
            "mov fs, {sel:x}",
            "mov gs, {sel:x}",
            sel = in(reg) KERNEL_DATA_SELECTOR,
            options(nostack, preserves_flags),
        );
        crate::println!("[gdt] data segments reloaded");
        // Load the task register with the TSS selector.
        core::arch::asm!(
            "ltr {sel:x}",
            sel = in(reg) TSS_SELECTOR,
            options(nostack, preserves_flags),
        );
        crate::println!("[gdt] TSS loaded");
    }
}

/// Set the ring-0 stack pointer used when the CPU enters the kernel from ring 3.
pub fn set_rsp0(sp: u64) {
    // SAFETY: task switching is serialized by the scheduler; this writes the
    // TSS RSP0 slot without forming a reference to the mutable static.
    unsafe {
        addr_of_mut!(TSS.privilege_stack_table[0]).write(sp);
    }
}

pub fn rsp0() -> u64 {
    unsafe { core::ptr::addr_of!(TSS.privilege_stack_table[0]).read_unaligned() }
}
