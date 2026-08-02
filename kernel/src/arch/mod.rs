//! Architecture boundary (P1).
//!
//! Each supported target profile supplies one architecture module implementing
//! the same mechanism surface: CPU control (halt, interrupt masking, debug
//! exit), the saved user register frame and its semantic syscall accessors,
//! page-table construction and TLB maintenance, privilege transitions, and the
//! early platform devices the vertical slice needs.
//!
//! Everything outside `arch` is architecture-neutral: capabilities, IPC, tasks,
//! syscall semantics, generations, storage, and component runtime. Neutral code
//! reaches a mechanism only through the names re-exported here, never through
//! an ISA-specific module path, register name, or instruction.
//!
//! The boundary is enforced two ways. `just x86_portability_check` rejects x86
//! mechanism appearing outside the admitted architecture and build files, and
//! builds the neutral kernel library for `aarch64-unknown-none` so a leak fails
//! to assemble rather than passing review.

pub mod boot_context;

#[cfg(target_arch = "aarch64")]
pub mod aarch64;
#[cfg(target_arch = "x86_64")]
pub mod x86_64;

#[cfg(target_arch = "aarch64")]
pub use aarch64 as target;
#[cfg(target_arch = "x86_64")]
pub use x86_64 as target;

pub use target::context;
pub use target::cpu;
pub use target::paging;
pub use target::trap;
