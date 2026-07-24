//! CPU exception handling via a hand-written Interrupt Descriptor Table.
//!
//! On x86-64 the CPU consults the IDT (an array of up to 256 16-byte gate
//! descriptors) whenever an exception or interrupt fires. We define the
//! descriptor layout ourselves instead of using the `x86_64` crate so the
//! binary representation of each entry is explicit and auditable.
//!
//! Design note: handlers here only *collect and report* the fault frame.
//! Any policy (kill a component, reflect to a user-space fault manager)
//! belongs elsewhere — this matches the README's "kernel owns mechanisms,
//! not policy" direction. Today the policy is "print and halt"; later it
//! will be "deliver a typed fault to the component manager over IPC".
//!
//! Limine sets up a GDT before jumping to `_start`: null at 0x00, 64-bit
//! code at 0x08, 64-bit data at 0x10. We reuse Limine's code segment
//! (0x08) as the selector in every gate. We do not need our own GDT until
//! we want an Interrupt Stack Table entry (needed for safe Double Fault
//! handling) — that is the next lesson.
use core::fmt;
use spin::LazyLock;

use crate::{println, serial_println};
/// Code segment selector as set up by Limine. Read from CS at init time
/// instead of hardcoded, because Limine's GDT layout is an implementation
/// detail — today CS=0x28, not the textbook 0x08, and that has changed
/// between Limine versions. Reading CS makes us robust to that.
static CODE_SELECTOR: spin::Once<u16> = spin::Once::new();

/// Gate flag byte: Present=1, DPL=0, type=64-bit interrupt gate.
/// Bits: `P DPL 0 type` = `1 00 0 1110` = `0x8E`.
/// Interrupt gates clear IF on entry (masks further interrupts); trap
/// gates (`0x8F`) would leave IF alone.
const FLAG_PRESENT: u8 = 1 << 7;
const FLAG_DPL0: u8 = 0 << 5;
const FLAG_INTERRUPT_GATE: u8 = 0b1110;
const FLAG_TRAP_GATE: u8 = 0b1111;
const INTERRUPT_GATE_FLAGS: u8 = FLAG_PRESENT | FLAG_DPL0 | FLAG_INTERRUPT_GATE;
const TRAP_GATE_FLAGS: u8 = FLAG_PRESENT | FLAG_DPL0 | FLAG_TRAP_GATE;

/// A 16-byte IDT gate descriptor, laid out exactly as the AMD64 manual
/// specifies. `packed` because fields are not naturally aligned
/// (`offset_low` is 2 bytes, immediately followed by a 2-byte selector,
/// then a 1-byte IST...). We only ever write fields by value, never by
/// reference, which keeps the packed-struct hazards away.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub(crate) struct GateDescriptor {
    offset_low: u16,
    selector: u16,
    ist: u8,
    flags: u8,
    offset_middle: u16,
    offset_high: u32,
    reserved: u32,
}

impl GateDescriptor {
    /// An empty (not-present) entry. The CPU skips these and moves on to
    /// the next vector; if a fault targets one, it will double-fault.
    const NULL: Self = Self {
        offset_low: 0,
        selector: 0,
        ist: 0,
        flags: 0,
        offset_middle: 0,
        offset_high: 0,
        reserved: 0,
    };

    /// Install a handler that takes no error code (the majority of
    /// exceptions: #DE, #BP, #OF, #BR, #UD, #NM, #MF, #XF, #MD, ...).
    fn set_interrupt_handler(
        &mut self,
        handler: unsafe extern "x86-interrupt" fn(InterruptStackFrame),
    ) {
        self.set_handler_raw(handler as usize, INTERRUPT_GATE_FLAGS);
    }

    /// Install a trap-gate handler (same shape, but does not clear IF on
    /// entry). Used for `int3` so breakpoints do not mask interrupts.
    fn set_trap_handler(&mut self, handler: unsafe extern "x86-interrupt" fn(InterruptStackFrame)) {
        self.set_handler_raw(handler as usize, TRAP_GATE_FLAGS);
    }

    pub(crate) fn set_handler_raw(&mut self, addr: usize, flags: u8) {
        self.set_handler_raw_ist(addr, flags, 0);
    }

    /// Like [`Self::set_handler_raw`] but selects an Interrupt Stack Table
    /// entry (1..=7). `ist == 0` means "use the current stack".
    fn set_handler_raw_ist(&mut self, addr: usize, flags: u8, ist: u8) {
        self.offset_low = (addr & 0xffff) as u16;
        self.offset_middle = ((addr >> 16) & 0xffff) as u16;
        self.offset_high = ((addr >> 32) & 0xffff_ffff) as u32;
        self.selector = *CODE_SELECTOR
            .get()
            .expect("CODE_SELECTOR must be set in init() before installing handlers");
        self.ist = ist & 0b111;
        self.flags = flags;
        self.reserved = 0;
    }

    /// Install an error-code handler that runs on IST stack `ist` (1..=7).
    /// Used for the Double Fault handler so it survives a corrupt kernel stack.
    fn set_interrupt_handler_err_ist(
        &mut self,
        handler: unsafe extern "x86-interrupt" fn(InterruptStackFrame, u64) -> !,
        ist: u8,
    ) {
        self.set_handler_raw_ist(handler as usize, INTERRUPT_GATE_FLAGS, ist);
    }
}

/// The stack frame the CPU pushes before entering any exception handler
/// (no error code). Order in memory, low address first:
/// `RIP, CS(padded), RFLAGS, RSP, SS(padded)`.
///
/// `repr(C)` (not packed) — this is what the handler receives a reference
/// to, and the layout on the stack is naturally 8-byte aligned.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct InterruptStackFrame {
    pub instruction_pointer: u64,
    pub code_segment: u64,
    pub cpu_flags: u64,
    pub stack_pointer: u64,
    pub stack_segment: u64,
}

impl fmt::Display for InterruptStackFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "RIP={:#018x} CS={:#06x} RFLAGS={:#018x} RSP={:#018x} SS={:#06x}",
            self.instruction_pointer,
            self.code_segment,
            self.cpu_flags,
            self.stack_pointer,
            self.stack_segment,
        )
    }
}

/// The 256-entry Interrupt Descriptor Table.
///
/// 256 entries × 16 bytes = 4096 bytes, exactly one page. The `'static`
/// lifetime in `load()` is enforced by the caller holding a `&'static`
/// reference (the `IDT` static below).
pub struct InterruptDescriptorTable {
    entries: [GateDescriptor; 256],
}

impl InterruptDescriptorTable {
    /// A table with every entry cleared. Callers fill in the vectors they
    /// care about, then `load()`.
    fn new_empty() -> Self {
        Self {
            entries: [GateDescriptor::NULL; 256],
        }
    }

    fn load(&'static self) {
        // The 10-byte IDTR: 16-bit limit (size-1) + 64-bit base address.
        // `repr(C, packed)` because limit is 2 bytes immediately followed
        // by the 8-byte base — not naturally aligned.
        #[repr(C, packed)]
        struct Idtr {
            limit: u16,
            base: u64,
        }

        let idtr = Idtr {
            limit: (core::mem::size_of::<Self>() - 1) as u16,
            base: self.entries.as_ptr() as u64,
        };

        // SAFETY: `lidt` reads 10 bytes from `[idtr]` and stores them in
        // the hidden IDTR register. The pointer is valid for the duration
        // of the load. The IDT it points at (`self`) lives for `'static`
        // because the caller bound `&'static self`, so the CPU's later
        // reads of the IDT are also in-bounds.
        unsafe {
            core::arch::asm!(
                "lidt [{idtr}]",
                idtr = in(reg) core::ptr::addr_of!(idtr),
                options(nostack, preserves_flags, readonly),
            );
        }
    }

    // --- entry accessors by vector, for init code ---
    pub(crate) fn entry(&mut self, vector: u8) -> &mut GateDescriptor {
        &mut self.entries[vector as usize]
    }
}

// ---- handlers ----

/// Breakpoint exception (#BP, vector 3). Triggered by `int3`.
///
/// A trap, not a fault: RIP points *past* the `int3`, so `iretq` resumes
/// normally. We use a trap gate so we do not mask other interrupts.
fn breakpoint_handler(frame: &InterruptStackFrame) {
    serial_println!("[#BP] breakpoint\n  {}", frame);
    // Note: framebuffer println would also work, but serial is more
    // reliable in early bring-up.
}

/// Double fault (#DF, vector 8). Raised when a fault occurs while the CPU is
/// trying to deliver an earlier fault (e.g. a fault on an unusable stack).
/// Runs on the dedicated IST stack (see [`crate::gdt`]) so it reports rather
/// than escalating to a triple fault. A #DF is unrecoverable: the error code
/// is always zero and there is no defined way to resume, so we halt.
fn double_fault_handler(frame: &InterruptStackFrame, error_code: u64) -> ! {
    serial_println!("[#DF] double fault (err={:#x})\n  {}", error_code, frame,);
    println!("[#DF] double fault at {:#018x}", frame.instruction_pointer,);
    crate::hlt_loop()
}

/// Vectors used by the Local APIC timer, i8042 keyboard, and userspace traps.
pub const TIMER_VECTOR: u8 = 0x20;
pub const KEYBOARD_VECTOR: u8 = 0x21;
pub const SYSCALL_VECTOR: u8 = 0x80;

/// Local APIC timer interrupt. Advances the monotonic tick counter and
/// acknowledges the interrupt so the APIC can deliver the next one.
fn timer_handler(_frame: &InterruptStackFrame) {
    crate::time::on_tick();
    crate::time::apic::end_of_interrupt();
}

/// i8042 keyboard interrupt. Drain one scan code, then acknowledge the LAPIC.
fn keyboard_handler(_frame: &InterruptStackFrame) {
    crate::input::on_interrupt();
    crate::time::apic::end_of_interrupt();
}

/// Spurious-interrupt vector. The LAPIC raises this when an interrupt is
/// withdrawn before it can be dispatched; it needs no EOI. Must be handled so
/// the CPU does not fault on an absent gate.
pub const SPURIOUS_VECTOR: u8 = 0xFF;

/// Spurious LAPIC interrupt. Deliberately does nothing (no EOI).
fn spurious_handler(_frame: &InterruptStackFrame) {}

// Mark handlers with the x86-interrupt ABI.
//
// The frame is passed BY VALUE: the `x86-interrupt` ABI aliases the by-value
// parameter onto the frame the CPU pushed on the stack. Taking it by reference
// instead would make the handler treat the pushed RIP as a pointer and
// dereference into garbage — so each wrapper receives the frame by value and
// lends `&frame` to the reporting handler.
//
// SAFETY: each handler is only ever entered by the CPU through its IDT entry;
// calling them from Rust would violate their calling convention.
unsafe extern "x86-interrupt" fn breakpoint(frame: InterruptStackFrame) {
    breakpoint_handler(&frame)
}
unsafe extern "x86-interrupt" fn double_fault(frame: InterruptStackFrame, error_code: u64) -> ! {
    double_fault_handler(&frame, error_code)
}
unsafe extern "x86-interrupt" fn timer(frame: InterruptStackFrame) {
    timer_handler(&frame)
}
unsafe extern "x86-interrupt" fn keyboard(frame: InterruptStackFrame) {
    keyboard_handler(&frame)
}
unsafe extern "x86-interrupt" fn spurious(frame: InterruptStackFrame) {
    spurious_handler(&frame)
}

/// The single kernel IDT. `LazyLock` initializes it on first access
/// (during `init()`), filling in only the vectors we actually handle.
static IDT: LazyLock<InterruptDescriptorTable> = LazyLock::new(|| {
    let mut idt = InterruptDescriptorTable::new_empty();

    // User/kernel trap stubs for #DE, #UD, #GP, #PF and int 0x80.
    crate::trap::install(&mut idt);
    // #BP — breakpoint. Trap gate: do not mask interrupts.
    idt.entry(3).set_trap_handler(breakpoint);
    // #DF — double fault. Runs on the IST stack so a corrupt kernel stack
    // still delivers a reported fault instead of a triple fault.
    idt.entry(8)
        .set_interrupt_handler_err_ist(double_fault, crate::gdt::DOUBLE_FAULT_IST_INDEX);
    // LAPIC timer.
    idt.entry(TIMER_VECTOR).set_interrupt_handler(timer);
    // i8042 keyboard routed through the ACPI-described I/O APIC.
    idt.entry(KEYBOARD_VECTOR).set_interrupt_handler(keyboard);
    // LAPIC spurious interrupt.
    idt.entry(SPURIOUS_VECTOR).set_interrupt_handler(spurious);

    idt
});

/// Load the IDT into the CPU. After this returns, exceptions route to
/// our handlers instead of triple-faulting QEMU into a reset loop.
pub fn init() {
    // Capture the current code segment selector *before* the LazyLock
    // initializer runs, since every gate descriptor needs it. SAFETY:
    // reading CS is a non-privileged, side-effect-free register read.
    let cs: u16;
    unsafe {
        core::arch::asm!(
            "mov {0:x}, cs",
            out(reg) cs,
            options(nomem, nostack, preserves_flags),
        );
    }
    CODE_SELECTOR.call_once(|| cs);

    IDT.load();
}
