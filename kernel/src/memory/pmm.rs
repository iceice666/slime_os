//! Physical frame allocator.
//!
//! A free-list stack: each free frame stores the physical address of the next
//! free frame in its first 8 bytes (reached through the HHDM). This needs no
//! bootstrap storage of its own — the bookkeeping lives inside the free frames
//! — and gives O(1) `alloc`/`dealloc`.
//!
//! Physical frame 0 is never handed out: address 0 doubles as the list's null
//! terminator. Firmware reserves low memory anyway, so no usable frame is lost.

use boot_contracts::handoff::MEMORY_USABLE;
use spin::Mutex;

use super::{PAGE_SIZE, PhysAddr, align_down, align_up};

/// The single kernel frame allocator.
pub static FRAME_ALLOCATOR: Mutex<FrameAllocator> = Mutex::new(FrameAllocator::empty());

pub struct FrameAllocator {
    /// Physical address of the top free frame, or `None` when empty.
    head: Option<PhysAddr>,
    /// Frames currently free.
    free: usize,
    /// Frames ever managed (constant after [`init`]).
    total: usize,
}

impl FrameAllocator {
    const fn empty() -> Self {
        Self {
            head: None,
            free: 0,
            total: 0,
        }
    }

    /// Push a frame onto the free list.
    ///
    /// # Safety
    ///
    /// `frame` must be a page-aligned physical frame that is currently unused
    /// and covered by the HHDM, and must not be physical frame 0.
    unsafe fn push(&mut self, frame: PhysAddr) {
        let slot = frame.to_virt().as_mut_ptr::<u64>();
        // Store the previous head inside the frame; 0 marks end-of-list.
        unsafe { slot.write(self.head.map_or(0, |p| p.0)) };
        self.head = Some(frame);
        self.free += 1;
    }

    /// Allocate one physical frame, or `None` if exhausted.
    ///
    /// The returned frame's contents are unspecified; callers that need zeroed
    /// memory must clear it themselves.
    pub fn alloc(&mut self) -> Option<PhysAddr> {
        let frame = self.head?;
        let slot = frame.to_virt().as_mut_ptr::<u64>();
        // SAFETY: `frame` came from the free list, so its first word holds the
        // next-free-frame pointer we wrote in `push`.
        let next = unsafe { slot.read() };
        self.head = (next != 0).then_some(PhysAddr(next));
        self.free -= 1;
        Some(frame)
    }

    /// Return a previously allocated frame to the free list.
    ///
    /// # Safety
    ///
    /// `frame` must have come from [`Self::alloc`] and must no longer be in use.
    pub unsafe fn dealloc(&mut self, frame: PhysAddr) {
        unsafe { self.push(frame) };
    }

    /// Frames currently free.
    pub fn free_frames(&self) -> usize {
        self.free
    }

    /// Frames managed in total (constant after init).
    pub fn total_frames(&self) -> usize {
        self.total
    }
}

/// Seed the frame allocator from the boot handoff memory map.
pub fn init(entries: &[crate::boot::MemoryEntry]) {
    let mut fa = FRAME_ALLOCATOR.lock();
    let page = PAGE_SIZE as u64;

    for entry in entries {
        if entry.kind != MEMORY_USABLE {
            continue;
        }
        let start = align_up(entry.base, page);
        let end = align_down(entry.base + entry.length, page);
        let mut addr = start;
        while addr + page <= end {
            if addr != 0 {
                unsafe { fa.push(PhysAddr(addr)) };
            }
            addr += page;
        }
    }

    fa.total = fa.free;
}
