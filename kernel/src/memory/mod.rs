//! Physical and virtual memory management.
//!
//! Three layers, brought up in order by [`init`]:
//!   - [`pmm`]  — a physical frame allocator seeded from the Limine memory map;
//!   - [`vmm`]  — a page-table mapper that walks the active tables via the HHDM;
//!   - [`heap`] — a kernel heap backed by frames from the PMM, mapped by the VMM.
//!
//! Physical memory is reached through Limine's Higher-Half Direct Map (HHDM):
//! every usable physical address `pa` is already mapped at virtual address
//! `pa + HHDM_OFFSET`. We never touch a physical address directly; we go
//! through [`PhysAddr::to_virt`].

pub mod address_space;
pub mod heap;
pub mod pmm;
pub mod vmm;

use core::sync::atomic::{AtomicU64, Ordering};

/// Size of a 4 KiB page/frame. The only page size the mapper produces.
pub const PAGE_SIZE: usize = 4096;

/// HHDM offset published by Limine, captured once in [`init`]. Zero until then.
static HHDM_OFFSET: AtomicU64 = AtomicU64::new(0);

/// A physical address. Never dereference directly — go through [`Self::to_virt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct PhysAddr(pub u64);

/// A virtual address in the kernel address space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct VirtAddr(pub u64);

impl PhysAddr {
    /// The HHDM virtual address that maps this physical address.
    pub fn to_virt(self) -> VirtAddr {
        VirtAddr(self.0.wrapping_add(hhdm_offset()))
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl VirtAddr {
    pub fn as_u64(self) -> u64 {
        self.0
    }

    pub fn as_mut_ptr<T>(self) -> *mut T {
        self.0 as *mut T
    }
}

/// The HHDM offset. Panics conceptually only if read before [`init`] (returns 0).
pub fn hhdm_offset() -> u64 {
    HHDM_OFFSET.load(Ordering::Relaxed)
}

/// Round `addr` up to the next multiple of `align` (a power of two).
pub const fn align_up(addr: u64, align: u64) -> u64 {
    (addr + align - 1) & !(align - 1)
}

/// Round `addr` down to the previous multiple of `align` (a power of two).
pub const fn align_down(addr: u64, align: u64) -> u64 {
    addr & !(align - 1)
}

/// Bring up physical, virtual, and heap memory management.
///
/// Must be called exactly once, after the IDT is loaded (so a bad mapping
/// surfaces as a reported page fault rather than a triple fault) and before
/// any heap allocation.
pub fn init() {
    HHDM_OFFSET.store(crate::boot::direct_map_offset(), Ordering::Relaxed);
    pmm::init(crate::boot::memory_map());
    heap::init().expect("kernel heap init failed");
}
