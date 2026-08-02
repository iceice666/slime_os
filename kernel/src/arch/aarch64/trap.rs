//! AArch64 saved user state and the semantic syscall calling convention.
//!
//! The frame holds `x0`–`x30` plus the exception-return state (`ELR_EL1`,
//! `SPSR_EL1`, `SP_EL0`). The accessors implement the AArch64 half of
//! `docs/syscall-abi.md`: syscall number in `x8`, arguments `a0..a4` in
//! `x0`–`x4`, primary return in `x0`, auxiliary return in `x1`. Those
//! assignments are what P2 must implement in the `svc` entry stub; the
//! semantic table, error model, bounds, and rights checks are shared.

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

/// `SPSR_EL1.M[3:0]` selecting EL0 with `SP_EL0` — the state a user task
/// returns to.
const SPSR_MODE_EL0T: u64 = 0b0000;
/// Mask over `SPSR_EL1.M[3:0]`.
const SPSR_MODE_MASK: u64 = 0b1111;

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

/// Byte size of a saved [`UserFrame`], as the context-switch stubs assume.
pub const USER_FRAME_BYTES: usize = core::mem::size_of::<UserFrame>();
