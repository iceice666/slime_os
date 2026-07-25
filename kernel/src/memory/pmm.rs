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

/// Upper bound on frames per contiguous run. Bounds the fixed scratch array
/// in [`alloc_contiguous`]; both the DMA table and the shared-buffer table
/// cap their per-region page counts at or below this value.
pub const CONTIG_MAX_FRAMES: usize = 64;

/// Allocate `pages` physically contiguous frames and return the base (lowest)
/// address, or `None` if no contiguous run is available. Scans the free list
/// for a run of consecutive frame numbers; for the bounded QEMU vertical slice
/// this is sufficient and avoids a buddy allocator.
///
/// The free list is a stack, so a contiguous block is handed out in
/// *descending* address order; contiguity is therefore checked by span, not by
/// pop order. On a non-contiguous batch one frame is set aside to shift the
/// scan window, and every set-aside frame is returned before this call exits.
///
/// `pages` must be in `1..=CONTIG_MAX_FRAMES`; a larger request returns `None`.
pub fn alloc_contiguous(pages: usize) -> Option<PhysAddr> {
    if pages == 0 || pages > CONTIG_MAX_FRAMES {
        return None;
    }
    let mut alloc = FRAME_ALLOCATOR.lock();
    // Frames set aside to shift the scan window across retries. Bounded by the
    // retry budget so the scratch array stays fixed-size and stack-resident.
    const MAX_RETRIES: usize = CONTIG_MAX_FRAMES * 4;
    let mut aside = [PhysAddr(0); MAX_RETRIES];
    let mut aside_len = 0;
    let page = PAGE_SIZE as u64;

    let mut result = None;
    for _ in 0..MAX_RETRIES {
        let mut collected = [PhysAddr(0); CONTIG_MAX_FRAMES];
        let mut got = 0;
        while got < pages {
            match alloc.alloc() {
                Some(p) => {
                    collected[got] = p;
                    got += 1;
                }
                None => break,
            }
        }
        if got < pages {
            // Exhausted: return this partial batch and stop.
            for frame in collected.iter().take(got) {
                // SAFETY: each came from `alloc` and is unused.
                unsafe { alloc.dealloc(*frame) };
            }
            break;
        }
        // Contiguous iff `pages` distinct frames span exactly (pages-1) pages.
        let mut min = collected[0].0;
        let mut max = collected[0].0;
        for frame in collected.iter().take(pages).skip(1) {
            min = min.min(frame.0);
            max = max.max(frame.0);
        }
        if max - min == (pages as u64 - 1) * page {
            result = Some(PhysAddr(min));
            break;
        }
        // Not contiguous. Set one frame aside to shift the window, return the
        // rest, and retry. If the stash is full, give up (return everything).
        if aside_len == MAX_RETRIES {
            for frame in collected.iter().take(pages) {
                // SAFETY: each came from `alloc` and is unused.
                unsafe { alloc.dealloc(*frame) };
            }
            break;
        }
        aside[aside_len] = collected[0];
        aside_len += 1;
        for frame in collected.iter().take(pages).skip(1) {
            // SAFETY: each came from `alloc` and is unused.
            unsafe { alloc.dealloc(*frame) };
        }
    }

    // Return every set-aside frame to the free list.
    for frame in aside.iter().take(aside_len) {
        // SAFETY: each came from `alloc` and is unused (never part of `result`).
        unsafe { alloc.dealloc(*frame) };
    }
    result
}

/// Free a contiguous run previously returned by [`alloc_contiguous`].
///
/// # Safety
///
/// `base` must name a region of `pages` contiguous frames currently owned by
/// the caller and no longer in use.
pub unsafe fn free_contiguous(base: PhysAddr, pages: usize) {
    let mut alloc = FRAME_ALLOCATOR.lock();
    for i in 0..pages {
        // SAFETY: caller guarantees these frames are owned and unused.
        unsafe {
            alloc.dealloc(PhysAddr(base.0 + (i as u64) * PAGE_SIZE as u64));
        }
    }
}
