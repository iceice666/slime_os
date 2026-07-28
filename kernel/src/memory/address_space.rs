use super::pmm::FRAME_ALLOCATOR;
use super::vmm::{self, MapError, PTE_PRESENT, PTE_USER, PTE_WRITABLE, map_page_in};
use super::{PAGE_SIZE, PhysAddr, VirtAddr};

pub struct AddressSpace {
    pml4: PhysAddr,
}

impl AddressSpace {
    pub fn new() -> Result<Self, MapError> {
        let frame = FRAME_ALLOCATOR
            .lock()
            .alloc()
            .ok_or(MapError::OutOfFrames)?;

        // SAFETY: `frame` is freshly allocated and reachable through HHDM.
        unsafe {
            core::ptr::write_bytes(frame.to_virt().as_mut_ptr::<u8>(), 0, PAGE_SIZE);
        }

        let cur = vmm::active_pml4();
        // SAFETY: both frames are live PML4-sized pages reached through HHDM.
        unsafe {
            let dst = frame.to_virt().as_mut_ptr::<u64>();
            let src = cur.to_virt().as_mut_ptr::<u64>();
            core::ptr::copy_nonoverlapping(src.add(256), dst.add(256), 256);
        }

        Ok(Self { pml4: frame })
    }

    pub fn map_user(&mut self, virt: VirtAddr, phys: PhysAddr, flags: u64) -> Result<(), MapError> {
        // SAFETY: callers provide an owned frame and user mapping flags.
        unsafe { map_page_in(self.pml4, virt, phys, flags | PTE_USER | PTE_PRESENT) }
    }
    pub fn user_range_mapped(&self, addr: u64, len: usize, writable: bool) -> bool {
        let Some(end) = addr.checked_add(len as u64) else {
            return false;
        };
        if len == 0 {
            return true;
        }

        let mut page = addr & !(PAGE_SIZE as u64 - 1);
        let last = (end - 1) & !(PAGE_SIZE as u64 - 1);
        loop {
            let Some(flags) = vmm::page_flags_in(self.pml4, VirtAddr(page)) else {
                return false;
            };
            if writable && flags & PTE_WRITABLE == 0 {
                return false;
            }
            if page == last {
                return true;
            }
            page += PAGE_SIZE as u64;
        }
    }

    /// Translate one mapped user address in this address space. Returns `None`
    /// for an absent or non-user leaf. Used by shared-buffer lifecycle checks
    /// to prove an admitted subrange maps the exact buffer frame.
    pub fn user_translation(&self, addr: u64) -> Option<PhysAddr> {
        let virt = VirtAddr(addr);
        vmm::page_flags_in(self.pml4, virt)?;
        vmm::translate_in(self.pml4, virt)
    }

    pub fn switch(&self) {
        // SAFETY: `self.pml4` is a live PML4 frame for this address space.
        unsafe {
            core::arch::asm!("mov cr3, {}", in(reg) self.pml4.0, options(nostack, preserves_flags));
        }
    }

    pub fn pml4(&self) -> PhysAddr {
        self.pml4
    }
}

impl Drop for AddressSpace {
    fn drop(&mut self) {
        // Release the whole user half — leaf pages, then the tables that held
        // them — before the PML4 itself (B9). Every frame `spawn_with_caps_for`
        // mapped for image segments and the stack is reachable from here, and
        // nothing else releases them: without this, each spawn permanently
        // consumed its image plus stack pages.
        //
        // The kernel half is left alone on purpose. Entries 256..512 are
        // aliases of the one kernel hierarchy copied in by `new`, shared with
        // every other address space, so freeing them would unmap the kernel out
        // from under the whole system.
        //
        // SAFETY: an `AddressSpace` is dropped only once its task has been
        // reaped, so no task is running in it and no live borrow of its user
        // frames remains.
        unsafe {
            vmm::free_user_half(self.pml4);
            FRAME_ALLOCATOR.lock().dealloc(self.pml4);
        }
    }
}
