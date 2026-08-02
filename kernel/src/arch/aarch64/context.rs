//! AArch64 privilege transitions: entering and resuming EL0.
//!
//! P2 implements the `eret` path — restoring `x0`–`x30`, `SP_EL0`, `ELR_EL1`,
//! and `SPSR_EL1`, and installing the incoming `TTBR0_EL1` — behind this
//! signature.

use super::trap::UserFrame;

/// Install `root` as the address-space root and resume `frame` at EL0. Does
/// not return.
///
/// # Safety
///
/// `root` must name a live top-level table whose kernel half maps this kernel,
/// and `frame` must be a saved user frame for a task in it.
pub unsafe fn switch_address_space_and_user(_root: u64, _frame: *const UserFrame) -> ! {
    unimplemented!("aarch64 eret to EL0: implemented by P2")
}
