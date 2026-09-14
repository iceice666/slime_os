//! Initial register state for a thread the root starts directly.
//!
//! A thread the root creates does not reach its entry point through a call.
//! The root writes PC and SP into a fresh `UserContext` and resumes the TCB, so
//! the entry point begins executing with whatever stack pointer was written and
//! nothing pushed below it. Every supported ABI requires a particular alignment
//! at that first instruction, and they do not agree on what it is, so the value
//! is computed once here rather than at each thread-start site.
//!
//! AArch64 and RISC-V pass the return address in a link register, so a function
//! body begins with a 16-byte-aligned stack pointer and the aligned top of the
//! reserved region is already correct.
//!
//! SysV x86-64 instead specifies that `rsp + 8` is 16-byte aligned at a
//! function's first instruction, because a `call` has just pushed an 8-byte
//! return address. Compilers rely on this: they place 16-byte-aligned SSE
//! spills at `rsp`-relative offsets derived from it, so entering with a
//! 16-byte-aligned `rsp` misaligns every one of them and the first `movaps`
//! raises a general-protection fault. Reserving the extra 8 bytes reproduces
//! what the absent `call` would have pushed.
//!
//! The reserved word is never read. Every thread started this way has a
//! diverging entry point, so no return to that slot can occur.

/// The stack pointer to give a thread the kernel enters directly.
///
/// `top` is the exclusive upper bound of the reserved stack region; the result
/// is always at or below it, so a caller cannot overrun the reservation.
pub const fn initial_stack_pointer(top: usize) -> usize {
    let aligned = top & !0xf;
    #[cfg(target_arch = "x86_64")]
    {
        // Saturating rather than wrapping: a zero or tiny `top` is a caller
        // error, and underflowing to `usize::MAX` would turn it into a wild
        // stack pointer instead of an obviously unusable one.
        aligned.saturating_sub(8)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        aligned
    }
}

#[cfg(test)]
mod tests {
    use super::initial_stack_pointer;

    /// The result is what the callee's ABI requires at its first instruction,
    /// expressed as the condition the compiler's spill offsets assume rather
    /// than as the constant this platform happens to produce.
    #[test]
    fn entry_alignment_matches_the_callee_abi() {
        for top in [0x1000, 0x1008, 0x100f, 0x2000, 0xffff_0000] {
            let sp = initial_stack_pointer(top);
            if cfg!(target_arch = "x86_64") {
                assert_eq!(
                    (sp + 8) % 16,
                    0,
                    "SysV x86-64 requires the entry stack pointer plus 8 to be \
                     16-byte aligned"
                );
            } else {
                assert_eq!(
                    sp % 16,
                    0,
                    "AArch64 and RISC-V require a 16-byte aligned entry stack pointer"
                );
            }
        }
    }

    /// Never above the reservation: the stack grows down from here, so a
    /// pointer past the top would write memory the region does not own.
    #[test]
    fn stays_within_the_reserved_region() {
        for top in [0x1000, 0x1008, 0x100f, 0x2000] {
            assert!(initial_stack_pointer(top) <= top);
        }
    }

    /// A degenerate `top` must not wrap into a huge address.
    #[test]
    fn refuses_to_underflow() {
        assert!(initial_stack_pointer(0) <= 16);
    }
}
