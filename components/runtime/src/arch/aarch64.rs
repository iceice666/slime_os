//! AArch64 syscall entry.
//!
//! Trap: `svc #0`. Number in `x8`; arguments `a0..a4` in `x0`–`x4`; primary
//! return in `x0` as `i64` (negative = error), auxiliary return in `x1`. This
//! mirrors the kernel-side mapping in `kernel/src/arch/aarch64/trap.rs`.
//!
//! The instruction sequence is the real one, so this assembles and links for
//! `aarch64-unknown-none`; it becomes *live* only once P2 implements the
//! kernel's `svc` exception vector.

/// Issue a syscall returning one value.
///
/// # Safety
///
/// The caller must satisfy the invoked syscall's contract: pointer arguments
/// must name mapped user memory of the stated length.
#[inline(always)]
pub unsafe fn raw_syscall(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") nr,
            inlateout("x0") a0 => ret,
            in("x1") a1,
            in("x2") a2,
            in("x3") a3,
            in("x4") a4,
            options(nostack),
        );
    }
    ret
}

/// Issue a syscall returning a primary and an auxiliary value.
///
/// # Safety
///
/// As [`raw_syscall`].
#[inline(always)]
pub unsafe fn raw_syscall_pair(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> (i64, u64) {
    let ret: i64;
    let aux: u64;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x8") nr,
            inlateout("x0") a0 => ret,
            inlateout("x1") a1 => aux,
            in("x2") a2,
            in("x3") a3,
            in("x4") a4,
            options(nostack),
        );
    }
    (ret, aux)
}
