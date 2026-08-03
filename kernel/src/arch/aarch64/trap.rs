//! AArch64 exception entry, saved user state, and syscall dispatch.
//!
//! The vector stubs save `x0`–`x30`, `SP_EL0`, `ELR_EL1`, and `SPSR_EL1`
//! before calling Rust, then restore the possibly-mutated frame before `eret`.
//! Synchronous lower-EL `svc #0` exceptions enter the architecture-neutral
//! syscall dispatcher; every other synchronous exception is translated into
//! the shared [`UserFaultReason`] vocabulary.

use core::arch::global_asm;
use core::sync::atomic::{AtomicU8, Ordering};

use crate::arch::paging::{
    ENTRIES_PER_TABLE, PTE_INTERMEDIATE, PTE_LEAF, PTE_NO_EXECUTE, PTE_NO_EXECUTE_PRIVILEGED,
    PTE_NOT_GLOBAL, PTE_READ_ONLY, PTE_USER, active_root, flush_tlb_all, set_active_root,
    table_index,
};
use crate::memory::pmm::FRAME_ALLOCATOR;
use crate::memory::{PAGE_SIZE, PhysAddr, VirtAddr};
use crate::task::{self, TermReason, UserFaultReason};
use crate::{ipc, serial_println};

/// One past the highest user virtual address: the low half of a 48-bit
/// translation regime (`TTBR0_EL1`).
pub const USER_ADDRESS_TOP: u64 = 0x0000_8000_0000_0000;

/// Number of semantic syscall argument registers (`a0`..`a4`).
pub const SYSCALL_ARG_COUNT: usize = 5;

/// The user register state saved on exception entry and restored on `eret`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct UserFrame {
    /// `x0`–`x30`.
    pub x: [u64; 31],
    /// `SP_EL0`: the user stack pointer.
    pub sp: u64,
    /// `ELR_EL1`: the address execution resumes at.
    pub elr: u64,
    /// `SPSR_EL1`: saved processor state, including the return exception level.
    pub spsr: u64,
}

/// `SPSR_EL1.M[3:0]` selecting EL0 with `SP_EL0`.
const SPSR_MODE_EL0T: u64 = 0b0000;
/// `SPSR_EL1.M[3:0]` selecting EL1 with `SP_EL1`.
const SPSR_MODE_EL1H: u64 = 0b0101;
/// Mask over `SPSR_EL1.M[3:0]`.
const SPSR_MODE_MASK: u64 = 0b1111;
/// Mask every asynchronous exception while returning to privileged EL1 code.
const SPSR_DAIF_MASKED: u64 = 0b1111 << 6;

impl UserFrame {
    /// The requested syscall number (`x8` on this profile).
    pub fn syscall_number(&self) -> u64 {
        self.x[8]
    }

    /// Semantic syscall argument `index`, per the AArch64 calling convention
    /// `a0..a4 = x0..x4`.
    ///
    /// # Panics
    ///
    /// Panics if `index >= SYSCALL_ARG_COUNT`.
    pub fn arg(&self, index: usize) -> u64 {
        assert!(
            index < SYSCALL_ARG_COUNT,
            "syscall argument index out of range"
        );
        self.x[index]
    }

    /// Set the primary syscall return value (`x0`).
    pub fn set_return(&mut self, value: u64) {
        self.x[0] = value;
    }

    /// Set the auxiliary syscall return value (`x1`).
    pub fn set_aux_return(&mut self, value: u64) {
        self.x[1] = value;
    }

    /// The faulting or trapping user instruction address, for diagnostics.
    pub fn instruction_pointer(&self) -> u64 {
        self.elr
    }

    /// Whether the frame was saved while executing at EL0.
    pub fn from_user(&self) -> bool {
        self.spsr & SPSR_MODE_MASK == SPSR_MODE_EL0T
    }

    /// Build the initial frame for a task entering EL0 at `entry` with stack
    /// pointer `stack_top`.
    pub fn for_user_entry(entry: u64, stack_top: u64) -> Self {
        Self {
            elr: entry,
            sp: stack_top,
            // EL0t with all DAIF interrupt masks clear, so the timer preempts.
            spsr: SPSR_MODE_EL0T,
            ..Self::zeroed()
        }
    }

    /// An all-zero frame, for a task that has no saved user state yet.
    pub const fn zeroed() -> Self {
        Self {
            x: [0; 31],
            sp: 0,
            elr: 0,
            spsr: 0,
        }
    }
}

/// Byte size of a saved [`UserFrame`], as the entry stubs assume.
pub const USER_FRAME_BYTES: usize = core::mem::size_of::<UserFrame>();

// Hand-written assembly addresses this frame by byte offset. Pin every field
// boundary so a Rust-only layout change cannot silently corrupt exception
// return state.
const _: () = {
    assert!(USER_FRAME_BYTES == 272);
    assert!(core::mem::offset_of!(UserFrame, x) == 0);
    assert!(core::mem::offset_of!(UserFrame, sp) == 248);
    assert!(core::mem::offset_of!(UserFrame, elr) == 256);
    assert!(core::mem::offset_of!(UserFrame, spsr) == 264);
};

global_asm!(
    r#"
    .section .text.aarch64_vectors,"ax"
    .balign 2048
    .global __aarch64_vector_table
__aarch64_vector_table:
    .macro VECTOR target
        b \target
        .space 124
    .endm

    VECTOR __aarch64_current_sp0_sync
    VECTOR __aarch64_current_sp0_irq
    VECTOR __aarch64_current_sp0_fiq
    VECTOR __aarch64_current_sp0_serror
    VECTOR __aarch64_current_spx_sync
    VECTOR __aarch64_current_spx_irq
    VECTOR __aarch64_current_spx_fiq
    VECTOR __aarch64_current_spx_serror
    VECTOR __aarch64_lower_a64_sync
    VECTOR __aarch64_lower_a64_irq
    VECTOR __aarch64_lower_a64_fiq
    VECTOR __aarch64_lower_a64_serror
    VECTOR __aarch64_lower_a32_sync
    VECTOR __aarch64_lower_a32_irq
    VECTOR __aarch64_lower_a32_fiq
    VECTOR __aarch64_lower_a32_serror

    .macro EXCEPTION_ENTRY name, slot
    .global \name
\name:
        sub sp, sp, #272
        stp x0, x1, [sp, #0]
        stp x2, x3, [sp, #16]
        stp x4, x5, [sp, #32]
        stp x6, x7, [sp, #48]
        stp x8, x9, [sp, #64]
        stp x10, x11, [sp, #80]
        stp x12, x13, [sp, #96]
        stp x14, x15, [sp, #112]
        stp x16, x17, [sp, #128]
        stp x18, x19, [sp, #144]
        stp x20, x21, [sp, #160]
        stp x22, x23, [sp, #176]
        stp x24, x25, [sp, #192]
        stp x26, x27, [sp, #208]
        stp x28, x29, [sp, #224]
        str x30, [sp, #240]
        mrs x9, sp_el0
        str x9, [sp, #248]
        mrs x9, elr_el1
        str x9, [sp, #256]
        mrs x9, spsr_el1
        str x9, [sp, #264]

        mov x0, #\slot
        mov x1, sp
        mrs x2, esr_el1
        mrs x3, far_el1
        bl {trap_dispatch}

        ldr x9, [sp, #248]
        msr sp_el0, x9
        ldr x9, [sp, #256]
        msr elr_el1, x9
        ldr x9, [sp, #264]
        msr spsr_el1, x9
        ldp x0, x1, [sp, #0]
        ldp x2, x3, [sp, #16]
        ldp x4, x5, [sp, #32]
        ldp x6, x7, [sp, #48]
        ldp x8, x9, [sp, #64]
        ldp x10, x11, [sp, #80]
        ldp x12, x13, [sp, #96]
        ldp x14, x15, [sp, #112]
        ldp x16, x17, [sp, #128]
        ldp x18, x19, [sp, #144]
        ldp x20, x21, [sp, #160]
        ldp x22, x23, [sp, #176]
        ldp x24, x25, [sp, #192]
        ldp x26, x27, [sp, #208]
        ldp x28, x29, [sp, #224]
        ldr x30, [sp, #240]
        add sp, sp, #272
        eret
    .endm

    EXCEPTION_ENTRY __aarch64_current_sp0_sync, 0
    EXCEPTION_ENTRY __aarch64_current_sp0_irq, 1
    EXCEPTION_ENTRY __aarch64_current_sp0_fiq, 2
    EXCEPTION_ENTRY __aarch64_current_sp0_serror, 3
    EXCEPTION_ENTRY __aarch64_current_spx_sync, 4
    EXCEPTION_ENTRY __aarch64_current_spx_irq, 5
    EXCEPTION_ENTRY __aarch64_current_spx_fiq, 6
    EXCEPTION_ENTRY __aarch64_current_spx_serror, 7
    EXCEPTION_ENTRY __aarch64_lower_a64_sync, 8
    EXCEPTION_ENTRY __aarch64_lower_a64_irq, 9
    EXCEPTION_ENTRY __aarch64_lower_a64_fiq, 10
    EXCEPTION_ENTRY __aarch64_lower_a64_serror, 11
    EXCEPTION_ENTRY __aarch64_lower_a32_sync, 12
    EXCEPTION_ENTRY __aarch64_lower_a32_irq, 13
    EXCEPTION_ENTRY __aarch64_lower_a32_fiq, 14
    EXCEPTION_ENTRY __aarch64_lower_a32_serror, 15

    .section .text.aarch64_probe,"ax"
    .global __aarch64_enter_user_probe
__aarch64_enter_user_probe:
        sub sp, sp, #96
        stp x19, x20, [sp, #0]
        stp x21, x22, [sp, #16]
        stp x23, x24, [sp, #32]
        stp x25, x26, [sp, #48]
        stp x27, x28, [sp, #64]
        stp x29, x30, [sp, #80]

        mov x18, x0
        ldr x9, [x18, #248]
        msr sp_el0, x9
        ldr x9, [x18, #256]
        msr elr_el1, x9
        ldr x9, [x18, #264]
        msr spsr_el1, x9
        ldp x0, x1, [x18, #0]
        ldp x2, x3, [x18, #16]
        ldp x4, x5, [x18, #32]
        ldp x6, x7, [x18, #48]
        ldp x8, x9, [x18, #64]
        ldp x10, x11, [x18, #80]
        ldp x12, x13, [x18, #96]
        ldp x14, x15, [x18, #112]
        ldp x16, x17, [x18, #128]
        ldp x19, x20, [x18, #152]
        ldp x21, x22, [x18, #168]
        ldp x23, x24, [x18, #184]
        ldp x25, x26, [x18, #200]
        ldp x27, x28, [x18, #216]
        ldp x29, x30, [x18, #232]
        ldr x18, [x18, #144]
        eret

    .global __aarch64_probe_return
__aarch64_probe_return:
        ldp x19, x20, [sp, #0]
        ldp x21, x22, [sp, #16]
        ldp x23, x24, [sp, #32]
        ldp x25, x26, [sp, #48]
        ldp x27, x28, [sp, #64]
        ldp x29, x30, [sp, #80]
        add sp, sp, #96
        ret

    .global __aarch64_user_probe_start
__aarch64_user_probe_start:
        svc #0
        brk #0
        b .
    .global __aarch64_user_probe_end
__aarch64_user_probe_end:
    "#,
    trap_dispatch = sym trap_dispatch,
);

unsafe extern "C" {
    static __aarch64_vector_table: u8;
    static __aarch64_probe_return: u8;
    static __aarch64_user_probe_start: u8;
    static __aarch64_user_probe_end: u8;
    fn __aarch64_enter_user_probe(frame: *const UserFrame);
}

const EC_UNKNOWN: u8 = 0x00;
const EC_SVC64: u8 = 0x15;
const EC_INSTRUCTION_ABORT_LOWER: u8 = 0x20;
const EC_INSTRUCTION_ABORT_CURRENT: u8 = 0x21;
const EC_PC_ALIGNMENT: u8 = 0x22;
const EC_DATA_ABORT_LOWER: u8 = 0x24;
const EC_DATA_ABORT_CURRENT: u8 = 0x25;
const EC_SP_ALIGNMENT: u8 = 0x26;
const EC_BRK64: u8 = 0x3c;

/// Translate `ESR_EL1.EC` into the architecture-neutral task-fault vocabulary.
pub fn decode_sync_fault(esr: u64) -> UserFaultReason {
    let ec = ((esr >> 26) & 0x3f) as u8;
    match ec {
        EC_UNKNOWN | EC_BRK64 => UserFaultReason::UndefinedOp,
        EC_INSTRUCTION_ABORT_LOWER
        | EC_INSTRUCTION_ABORT_CURRENT
        | EC_DATA_ABORT_LOWER
        | EC_DATA_ABORT_CURRENT => UserFaultReason::PageFault,
        EC_PC_ALIGNMENT | EC_SP_ALIGNMENT => UserFaultReason::GeneralProt,
        _ => UserFaultReason::Unknown(ec),
    }
}

fn exception_class(esr: u64) -> u8 {
    ((esr >> 26) & 0x3f) as u8
}

fn is_sync_slot(slot: u64) -> bool {
    matches!(slot, 0 | 4 | 8 | 12)
}

extern "C" fn trap_dispatch(slot: u64, frame: *mut UserFrame, esr: u64, far: u64) {
    // SAFETY: every vector entry passes the aligned frame it allocated on the
    // current privileged stack; the borrow ends before the assembly restores it.
    let frame = unsafe { &mut *frame };
    let ec = exception_class(esr);

    if slot == 4 && ec == EC_BRK64 && PROBE_STAGE.load(Ordering::Relaxed) == PROBE_EXPECT_EL1_BRK {
        let reason = decode_sync_fault(esr);
        serial_println!(
            "[aarch64-trap] el1 sync ec={:#x} reason={:?} elr={:#x}",
            ec,
            reason,
            frame.elr,
        );
        PROBE_STAGE.store(PROBE_IDLE, Ordering::Relaxed);
        // BRK reports its own address. Skip the instruction when returning.
        frame.elr = frame.elr.wrapping_add(4);
        return;
    }

    if slot == 8 && ec == EC_SVC64 && esr & 0xffff == 0 {
        if PROBE_STAGE.load(Ordering::Relaxed) == PROBE_EXPECT_SVC {
            dispatch_probe_syscall(frame);
        } else {
            crate::syscall::dispatch(frame);
        }
        return;
    }

    if slot == 8
        && ec == EC_BRK64
        && matches!(
            PROBE_STAGE.load(Ordering::Relaxed),
            PROBE_EXPECT_BRK | PROBE_FAILED
        )
    {
        complete_user_probe(frame, esr);
        return;
    }

    if frame.from_user() && is_sync_slot(slot) {
        let reason = decode_sync_fault(esr);
        if PROBE_STAGE.load(Ordering::Relaxed) != PROBE_IDLE {
            fail_probe("unexpected EL0 synchronous fault");
            complete_user_probe(frame, esr);
            return;
        }
        serial_println!(
            "[fault] {:?} elr={:#x} esr={:#x} far={:#x}",
            reason,
            frame.elr,
            esr,
            far,
        );
        // A scheduled EL0 task always establishes a current scheduler entry.
        task::terminate(frame, TermReason::Fault(reason));
        return;
    }

    let reason = is_sync_slot(slot).then(|| decode_sync_fault(esr));
    serial_println!(
        "[kernel fault] aarch64 slot={} reason={:?} elr={:#x} esr={:#x} far={:#x}",
        slot,
        reason,
        frame.elr,
        esr,
        far,
    );
    crate::hlt_loop();
}

/// Install the architected 16-slot EL1 vector table.
pub fn install() {
    let base = core::ptr::addr_of!(__aarch64_vector_table) as u64;
    assert_eq!(base & 0x7ff, 0, "vector table must be 2048-byte aligned");
    // SAFETY: the symbol names the 2048-byte-aligned static table above and
    // remains mapped executable for the kernel's lifetime.
    unsafe {
        core::arch::asm!(
            "msr vbar_el1, {base}",
            "isb",
            base = in(reg) base,
            options(nostack, preserves_flags),
        );
    }
    serial_println!("[aarch64-trap] vectors installed vbar={:#x}", base);
}

const PROBE_IDLE: u8 = 0;
const PROBE_EXPECT_SVC: u8 = 1;
const PROBE_EXPECT_BRK: u8 = 2;
const PROBE_COMPLETE: u8 = 3;
const PROBE_EXPECT_EL1_BRK: u8 = 4;
const PROBE_FAILED: u8 = 0xff;
static PROBE_STAGE: AtomicU8 = AtomicU8::new(PROBE_IDLE);

const USER_PROBE_CODE: u64 = 0x0040_0000;
const USER_PROBE_STACK: u64 = USER_PROBE_CODE + 2 * PAGE_SIZE as u64;
const HANDLER_MUTATION: u64 = 0x4d55_5441_5445_4432;

fn register_seed(index: usize) -> u64 {
    0xa500_0000_0000_0000 | index as u64
}

fn expected_input(index: usize) -> u64 {
    match index {
        0 => 0x1111_1111_1111_1111,
        1 => 0x2222_2222_2222_2222,
        2 => 0x3333_3333_3333_3333,
        3 => 0x4444_4444_4444_4444,
        4 => (crate::ipc::MAX_CAPS_PER_MSG + 1) as u64,
        8 => crate::syscall::SYS_SEND,
        // Exceeding this bound returns before pointer/capability lookup. That
        // lets P2.2 observe the real shared dispatcher before P2.3 establishes
        // a scheduled current task and its address-space/capability context.
        _ => register_seed(index),
    }
}

fn fail_probe(message: &str) {
    PROBE_STAGE.store(PROBE_FAILED, Ordering::Relaxed);
    serial_println!("[aarch64-trap] failed: {}", message);
}

fn dispatch_probe_syscall(frame: &mut UserFrame) {
    for index in 0..31 {
        if frame.x[index] != expected_input(index) {
            fail_probe("svc entry register mismatch");
            return;
        }
    }
    if frame.sp != USER_PROBE_STACK || !frame.from_user() || frame.elr != USER_PROBE_CODE + 4 {
        fail_probe("svc entry return state mismatch");
        return;
    }

    crate::syscall::dispatch(frame);
    if frame.x[0] as i64 != ipc::ERR_INVALID_ARG {
        fail_probe("shared syscall dispatcher returned the wrong bounded error");
        return;
    }
    for index in 1..31 {
        if frame.x[index] != expected_input(index) {
            fail_probe("shared syscall dispatcher clobbered a preserved register");
            return;
        }
    }

    // Deliberately mutate one otherwise-preserved register. The following BRK
    // observes it only if exception return restored this mutable frame.
    frame.x[20] = HANDLER_MUTATION;
    PROBE_STAGE.store(PROBE_EXPECT_BRK, Ordering::Relaxed);
    serial_println!(
        "[aarch64-trap] svc nr={} args={:#x},{:#x},{:#x},{:#x},{} result={}",
        frame.x[8],
        expected_input(0),
        frame.x[1],
        frame.x[2],
        frame.x[3],
        frame.x[4],
        frame.x[0] as i64,
    );
}

fn complete_user_probe(frame: &mut UserFrame, esr: u64) {
    let stage = PROBE_STAGE.load(Ordering::Relaxed);
    let reason = decode_sync_fault(esr);
    serial_println!(
        "[aarch64-trap] el0 sync ec={:#x} reason={:?} elr={:#x}",
        exception_class(esr),
        reason,
        frame.elr,
    );

    if stage == PROBE_EXPECT_BRK {
        for index in 0..31 {
            let expected = match index {
                0 => ipc::ERR_INVALID_ARG as u64,
                20 => HANDLER_MUTATION,
                _ => expected_input(index),
            };
            if frame.x[index] != expected {
                fail_probe("eret did not restore the complete mutable frame");
                break;
            }
        }
        if PROBE_STAGE.load(Ordering::Relaxed) != PROBE_FAILED
            && (frame.sp != USER_PROBE_STACK || !frame.from_user())
        {
            fail_probe("eret did not restore SP_EL0 or SPSR_EL1");
        }
        if PROBE_STAGE.load(Ordering::Relaxed) != PROBE_FAILED {
            PROBE_STAGE.store(PROBE_COMPLETE, Ordering::Relaxed);
            serial_println!(
                "[aarch64-trap] frame restored gprs=31 sp={:#x} handler_mutation={:#x}",
                frame.sp,
                frame.x[20],
            );
        }
    }

    // Return from the lower-EL exception to the privileged continuation that
    // restores the Rust caller's callee-saved registers and stack frame.
    frame.elr = core::ptr::addr_of!(__aarch64_probe_return) as u64;
    frame.spsr = SPSR_MODE_EL1H | SPSR_DAIF_MASKED;
}

/// Exercise the deliberate EL1 `brk #0` trap without swallowing compiler
/// trap instructions outside this exact probe window.
pub fn run_el1_breakpoint_probe() {
    PROBE_STAGE.store(PROBE_EXPECT_EL1_BRK, Ordering::Relaxed);
    // SAFETY: the vector table is installed and this exact probe stage makes
    // the EL1 BRK handler advance ELR_EL1 before returning.
    unsafe { crate::arch::cpu::breakpoint() };
    assert_eq!(
        PROBE_STAGE.load(Ordering::Relaxed),
        PROBE_IDLE,
        "EL1 breakpoint probe did not return through the vector handler",
    );
}

/// Run P2.2's live frame/syscall fixture. The pages exist only for this
/// bounded bring-up probe and are released after TTBR0 is restored.
pub fn run_user_probe() -> bool {
    let Some(mappings) = ProbeMappings::new() else {
        fail_probe("could not allocate probe mappings");
        return false;
    };
    let previous_root = active_root();

    let mut frame = UserFrame::for_user_entry(USER_PROBE_CODE, USER_PROBE_STACK);
    for index in 0..31 {
        frame.x[index] = expected_input(index);
    }

    PROBE_STAGE.store(PROBE_EXPECT_SVC, Ordering::Relaxed);
    // SAFETY: `mappings.root()` owns complete low-half tables containing only
    // the probe's executable and stack pages. TTBR1 keeps the kernel mapped.
    unsafe { set_active_root(mappings.root()) };
    flush_tlb_all();
    // SAFETY: the frame targets the two live user mappings and the exception
    // vectors return to the privileged continuation before this call returns.
    unsafe { __aarch64_enter_user_probe(core::ptr::addr_of!(frame)) };
    // SAFETY: restore the loader-provided low-half root before releasing every
    // frame in `mappings`.
    unsafe { set_active_root(previous_root) };
    flush_tlb_all();

    let complete = PROBE_STAGE.load(Ordering::Relaxed) == PROBE_COMPLETE;
    PROBE_STAGE.store(PROBE_IDLE, Ordering::Relaxed);
    drop(mappings);
    complete
}

struct ProbeMappings {
    frames: [PhysAddr; 6],
}

impl ProbeMappings {
    fn new() -> Option<Self> {
        let mut frames = [PhysAddr(0); 6];
        let mut allocated = 0;
        while allocated < frames.len() {
            let Some(frame) = FRAME_ALLOCATOR.lock().alloc() else {
                let mut allocator = FRAME_ALLOCATOR.lock();
                for frame in frames.iter().take(allocated) {
                    // SAFETY: these frames were allocated above and never linked
                    // into an active root because construction is incomplete.
                    unsafe { allocator.dealloc(*frame) };
                }
                return None;
            };
            frames[allocated] = frame;
            allocated += 1;
        }

        for frame in frames {
            // SAFETY: every frame is freshly allocated and direct-map reachable.
            unsafe { core::ptr::write_bytes(frame.to_virt().as_mut_ptr::<u8>(), 0, PAGE_SIZE) };
        }

        let code = VirtAddr(USER_PROBE_CODE);
        let stack = VirtAddr(USER_PROBE_CODE + PAGE_SIZE as u64);
        if table_index(code, 4) != table_index(stack, 4)
            || table_index(code, 3) != table_index(stack, 3)
            || table_index(code, 2) != table_index(stack, 2)
        {
            let mut allocator = FRAME_ALLOCATOR.lock();
            for frame in frames {
                // SAFETY: construction has not installed the root.
                unsafe { allocator.dealloc(frame) };
            }
            return None;
        }

        let start = core::ptr::addr_of!(__aarch64_user_probe_start) as usize;
        let end = core::ptr::addr_of!(__aarch64_user_probe_end) as usize;
        let Some(len) = end.checked_sub(start).filter(|len| *len <= PAGE_SIZE) else {
            deallocate_probe_frames(frames);
            return None;
        };

        write_entry(
            frames[0],
            table_index(code, 4),
            frames[1].0 | PTE_INTERMEDIATE,
        );
        write_entry(
            frames[1],
            table_index(code, 3),
            frames[2].0 | PTE_INTERMEDIATE,
        );
        write_entry(
            frames[2],
            table_index(code, 2),
            frames[3].0 | PTE_INTERMEDIATE,
        );
        write_entry(
            frames[3],
            table_index(code, 1),
            frames[4].0
                | PTE_LEAF
                | PTE_USER
                | PTE_NOT_GLOBAL
                | PTE_READ_ONLY
                | PTE_NO_EXECUTE_PRIVILEGED,
        );
        write_entry(
            frames[3],
            table_index(stack, 1),
            frames[5].0
                | PTE_LEAF
                | PTE_USER
                | PTE_NOT_GLOBAL
                | PTE_NO_EXECUTE
                | PTE_NO_EXECUTE_PRIVILEGED,
        );
        // SAFETY: the source is the linked probe byte range and the destination
        // is the fresh code frame, both valid for `len` non-overlapping bytes.
        unsafe {
            core::ptr::copy_nonoverlapping(
                start as *const u8,
                frames[4].to_virt().as_mut_ptr::<u8>(),
                len,
            );
        }
        synchronize_instruction_bytes(frames[4].to_virt().as_u64());
        Some(Self { frames })
    }

    fn root(&self) -> PhysAddr {
        self.frames[0]
    }
}

fn deallocate_probe_frames(frames: [PhysAddr; 6]) {
    let mut allocator = FRAME_ALLOCATOR.lock();
    for frame in frames {
        // SAFETY: the root was never activated on these construction failures.
        unsafe { allocator.dealloc(frame) };
    }
}

impl Drop for ProbeMappings {
    fn drop(&mut self) {
        let mut allocator = FRAME_ALLOCATOR.lock();
        for frame in self.frames {
            // SAFETY: TTBR0 was restored before drop; no translation or borrow
            // still references any probe table or leaf frame.
            unsafe { allocator.dealloc(frame) };
        }
    }
}

fn write_entry(table: PhysAddr, index: usize, value: u64) {
    assert!(index < ENTRIES_PER_TABLE, "page-table index out of range");
    let entry = table.to_virt().as_mut_ptr::<u64>().wrapping_add(index);
    // SAFETY: `table` is a fresh page-table frame and `index` is in its fixed
    // 512-entry range. Construction is single-threaded before the root is live.
    unsafe { entry.write(value) };
}

fn synchronize_instruction_bytes(address: u64) {
    let ctr: u64;
    // SAFETY: CTR_EL0 is a side-effect-free identification register read.
    unsafe {
        core::arch::asm!("mrs {}, ctr_el0", out(reg) ctr, options(nomem, nostack, preserves_flags));
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
    }
    let line_bytes = 4u64 << ((ctr >> 16) & 0xf);
    let mut line = address & !(line_bytes - 1);
    let end = address + PAGE_SIZE as u64;
    while line < end {
        // SAFETY: each address lies in the direct-mapped code frame. Cleaning
        // to PoU makes the copied instructions visible before the I-cache flush.
        unsafe {
            core::arch::asm!("dc cvau, {line}", line = in(reg) line, options(nostack, preserves_flags));
        }
        line += line_bytes;
    }
    // SAFETY: global I-cache invalidation is bounded to this single-core
    // bring-up phase and ordered after the data-cache clean.
    unsafe {
        core::arch::asm!(
            "dsb ish",
            "ic iallu",
            "dsb ish",
            "isb",
            options(nostack, preserves_flags),
        );
    }
}
