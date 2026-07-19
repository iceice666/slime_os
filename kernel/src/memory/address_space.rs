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
            core::ptr::copy_nonoverlapping(src.add(256), dst.add(256), 255);
            dst.add(511).write(src.add(511).read());
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
        // SAFETY: this address space owns its PML4 frame. Intermediate user-half
        // tables intentionally leak for the small M2 isolation test.
        unsafe {
            FRAME_ALLOCATOR.lock().dealloc(self.pml4);
        }
    }
}
