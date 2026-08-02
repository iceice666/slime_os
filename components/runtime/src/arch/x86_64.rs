//! x86-64 syscall entry.
//!
//! Trap: `int 0x80`. Number in `rax`; arguments `a0..a4` in
//! `rdi, rsi, rdx, r10, r8`; primary return in `rax` as `i64` (negative =
//! error), auxiliary return in `rdx`. The kernel's trap stub saves and restores
//! every general-purpose register across the trap, so no register beyond the
//! returns needs to be marked clobbered here.

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
            "int 0x80",
            inlateout("rax") nr => ret,
            in("rdi") a0,
            in("rsi") a1,
            in("rdx") a2,
            in("r10") a3,
            in("r8") a4,
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
            "int 0x80",
            inlateout("rax") nr => ret,
            in("rdi") a0,
            in("rsi") a1,
            inlateout("rdx") a2 => aux,
            in("r10") a3,
            in("r8") a4,
            options(nostack),
        );
    }
    (ret, aux)
}
