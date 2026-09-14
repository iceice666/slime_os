//! Frame VM attributes for data and executable mappings.
//!
//! `sel4::VmAttributes` is not the same kind of value on every admitted
//! architecture, so the root states the two mappings it actually wants once
//! here rather than open-coding an architecture conditional at each of the
//! seven `frame_map` call sites.
//!
//! On AArch64 and RISC-V the attribute word carries `EXECUTE_NEVER`, so a data
//! or stack page is mapped non-executable and only a `PF_X` segment stays
//! executable. On x86-64 seL4's `seL4_X86_VMAttributes` is a cache-policy
//! selector with no execute bit at all: the kernel's `makeUserPTE` derives
//! present/writable/user from the capability rights and the cache policy from
//! this word, and never sets the NX page-table bit. Execute permission on that
//! architecture is therefore not expressible in a frame mapping, and asking for
//! it would be a silent no-op rather than an enforced restriction.
//!
//! This is a real weakening of the x86-64 profile relative to the other two,
//! not an equivalence: W^X on child data pages is enforced by the page tables
//! on AArch64 and RISC-V and is unenforced on x86-64. It is recorded here so a
//! caller cannot read `data()` as a portable guarantee.

/// Attributes for a mapping that must not be executable.
pub fn data() -> sel4::VmAttributes {
    #[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
    {
        sel4::VmAttributes::DEFAULT | sel4::VmAttributes::EXECUTE_NEVER
    }
    // No execute-disable attribute exists on x86-64; see the module comment.
    #[cfg(target_arch = "x86_64")]
    {
        sel4::VmAttributes::DEFAULT
    }
}

/// Attributes for a mapping that may be executed.
pub fn executable() -> sel4::VmAttributes {
    sel4::VmAttributes::DEFAULT
}
