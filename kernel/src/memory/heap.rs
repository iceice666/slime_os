//! Kernel heap and the global allocator.
//!
//! [`init`] reserves a fixed virtual range, backs every page of it with frames
//! from the [`pmm`](super::pmm), maps them writable/no-execute via the
//! [`vmm`](super::vmm), and hands the range to a free-list allocator that
//! becomes the program-wide `#[global_allocator]`. After it runs, `alloc`,
//! `Box`, `Vec`, and friends work.
//!
//! The allocator is a first-fit free list kept sorted by address, with
//! boundary coalescing on free — enough for a kernel that allocates rarely and
//! never in a hot loop, and simple enough to audit. Out-of-memory is reported
//! by returning null from `GlobalAlloc::alloc`, which surfaces as a
//! deterministic allocation-error panic rather than a silent hang.
//!
//! The list is walked through raw pointers rather than chained `&mut`
//! references: each free block begins with a [`FreeNode`] header, and traversal
//! threads `*mut FreeNode` links. This keeps the coalescing logic linear and
//! explicit.

use core::alloc::{GlobalAlloc, Layout};
use core::mem::{align_of, size_of};
use core::ptr;

use spin::Mutex;

use super::vmm::{self, MapError, PTE_NO_EXECUTE, PTE_WRITABLE};
use super::{PAGE_SIZE, VirtAddr, align_up};

/// Virtual base of the kernel heap. Canonical higher-half, clear of Limine's
/// HHDM (based near `0xffff_8000_…`) and the kernel image (`0xffffffff8000_…`).
const HEAP_START: u64 = 0xffff_e000_0000_0000;
/// Heap size covers one bounded 16 MiB generation during recovery scrub,
/// component stacks, and the object-store staging buffer without overcommit.
const HEAP_SIZE: usize = 24 * 1024 * 1024;

/// Header at the front of every free block. Its size is the minimum block size.
struct FreeNode {
    size: usize,
    next: *mut FreeNode,
}

/// Minimum block size: large enough to hold a `FreeNode` when freed.
const fn min_block() -> usize {
    size_of::<FreeNode>()
}

/// Alignment every block starts on, so a `FreeNode` can always live there.
const fn block_align() -> usize {
    align_of::<FreeNode>()
}

/// A sorted (by address) singly linked free list.
struct FreeList {
    head: *mut FreeNode,
}

// SAFETY: the list is only ever touched behind `LockedHeap`'s `Mutex`; the raw
// pointers name heap memory owned by this allocator.
unsafe impl Send for FreeList {}

impl FreeList {
    const fn new() -> Self {
        Self {
            head: ptr::null_mut(),
        }
    }

    /// The block size and alignment needed to satisfy `layout`.
    fn size_align(layout: Layout) -> (usize, usize) {
        let align = layout.align().max(block_align());
        let size = align_up(layout.size() as u64, align as u64) as usize;
        (size.max(min_block()), align)
    }

    /// Insert `[addr, addr+size)` into the list in address order, then coalesce
    /// it with any physically adjacent neighbors.
    ///
    /// # Safety
    ///
    /// The region must be unused, writable, reachable, aligned to
    /// [`block_align`], and at least [`min_block`] bytes; it must outlive every
    /// allocation served from it.
    unsafe fn add_region(&mut self, addr: usize, size: usize) {
        debug_assert_eq!(addr % block_align(), 0);
        debug_assert!(size >= min_block());
        let node = addr as *mut FreeNode;

        // Find the last node before `addr` (`prev`), and the first after it.
        let mut prev: *mut FreeNode = ptr::null_mut();
        let mut cur = self.head;
        while !cur.is_null() && (cur as usize) < addr {
            prev = cur;
            // SAFETY: `cur` is a live list node.
            cur = unsafe { (*cur).next };
        }

        // Link the new node in between `prev` and `cur`.
        // SAFETY: `node` names the fresh writable region.
        unsafe {
            (*node).size = size;
            (*node).next = cur;
        }
        if prev.is_null() {
            self.head = node;
        } else {
            // SAFETY: `prev` is a live list node.
            unsafe { (*prev).next = node };
        }

        // Coalesce forward (node + next) then backward (prev + node).
        // SAFETY: all three pointers, when non-null, name live list nodes.
        unsafe {
            Self::try_merge(node);
            if !prev.is_null() {
                Self::try_merge(prev);
            }
        }
    }

    /// Merge `node` with its successor if they are physically contiguous.
    ///
    /// # Safety
    ///
    /// `node` must name a live list node.
    unsafe fn try_merge(node: *mut FreeNode) {
        // SAFETY: caller guarantees `node` is live.
        let next = unsafe { (*node).next };
        if next.is_null() {
            return;
        }
        // SAFETY: `node` is live; `size` is its byte length.
        let node_end = node as usize + unsafe { (*node).size };
        if node_end == next as usize {
            // SAFETY: `next` is live and immediately follows `node`.
            unsafe {
                (*node).size += (*next).size;
                (*node).next = (*next).next;
            }
        }
    }

    /// First-fit allocation. Returns a pointer, or null on OOM.
    fn alloc(&mut self, layout: Layout) -> *mut u8 {
        let (size, align) = Self::size_align(layout);

        let mut prev: *mut FreeNode = ptr::null_mut();
        let mut cur = self.head;
        while !cur.is_null() {
            let region_addr = cur as usize;
            // SAFETY: `cur` is a live list node.
            let region_size = unsafe { (*cur).size };
            // SAFETY: `cur` is a live list node.
            let next = unsafe { (*cur).next };

            let alloc_start = align_up(region_addr as u64, align as u64) as usize;
            let front_pad = alloc_start - region_addr;
            let needed = front_pad + size;

            if region_size >= needed {
                // Detach `cur` from the list.
                if prev.is_null() {
                    self.head = next;
                } else {
                    // SAFETY: `prev` is a live list node.
                    unsafe { (*prev).next = next };
                }

                let leftover = region_size - needed;
                // Front padding from alignment goes back on the list; padding
                // smaller than a node is folded into the allocation.
                if front_pad >= min_block() {
                    // SAFETY: the region is ours; carve a free node from its front.
                    unsafe { self.add_region(region_addr, front_pad) };
                }
                // Trailing remainder goes back on the list.
                if leftover >= min_block() {
                    // SAFETY: the tail of the block is unused and writable.
                    unsafe { self.add_region(alloc_start + size, leftover) };
                }
                return alloc_start as *mut u8;
            }

            prev = cur;
            cur = next;
        }
        ptr::null_mut()
    }

    /// Return an allocation to the free list.
    ///
    /// # Safety
    ///
    /// `ptr`/`layout` must come from a prior [`Self::alloc`] on this list.
    unsafe fn dealloc(&mut self, ptr: *mut u8, layout: Layout) {
        let (size, _) = Self::size_align(layout);
        // SAFETY: caller guarantees the region is a live allocation from us.
        unsafe { self.add_region(ptr as usize, size) };
    }
}

/// Lock wrapper so the free list can be a `#[global_allocator]`.
pub struct LockedHeap(Mutex<FreeList>);

impl LockedHeap {
    const fn new() -> Self {
        Self(Mutex::new(FreeList::new()))
    }
}

// SAFETY: all state is behind a `Mutex`; pointers returned come from the mapped
// heap region and honor the requested layout.
unsafe impl GlobalAlloc for LockedHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        self.0.lock().alloc(layout)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarded from the global allocator contract.
        unsafe { self.0.lock().dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::new();

/// Map the heap region and hand it to the allocator. Call once, after
/// [`pmm::init`](super::pmm::init).
pub fn init() -> Result<(), MapError> {
    let flags = PTE_WRITABLE | PTE_NO_EXECUTE;
    let mut virt = HEAP_START;
    let end = HEAP_START + HEAP_SIZE as u64;

    while virt < end {
        let frame = super::pmm::FRAME_ALLOCATOR
            .lock()
            .alloc()
            .ok_or(MapError::OutOfFrames)?;
        // SAFETY: `frame` is a fresh PMM frame exposed only at this heap VA,
        // writable and non-executable — safe kernel data backing.
        unsafe { vmm::map_page(VirtAddr(virt), frame, flags)? };
        virt += PAGE_SIZE as u64;
    }

    // SAFETY: the whole range is now mapped writable and otherwise unused.
    unsafe {
        ALLOCATOR
            .0
            .lock()
            .add_region(HEAP_START as usize, HEAP_SIZE);
    }
    Ok(())
}
