//! A bump allocator for components that need one (P5.4.2c).
//!
//! Most components never allocate: the syscall surface is fixed-size records
//! and the scenarios use stack arrays. Two do — `boot_contracts::gpt` reads a
//! device-sized GPT entry table into a `Vec`, and
//! `boot_contracts::object_store` builds its object index the same way — and
//! those two are the reason this exists.
//!
//! Bump, and no free. That is not a shortcut, it is the allocation shape:
//! a store component opens a partition, indexes it, answers a bounded number
//! of requests, and exits. Nothing outlives the component, so returning memory
//! to a free list would only add a failure mode.
//!
//! The whole module is behind the `heap` feature, which the store-plane build
//! turns on. That granularity is forced rather than chosen: `extern crate
//! alloc` anywhere in the dependency graph makes *every* binary in the build
//! require a `#[global_allocator]` symbol, and the store plane builds `init`
//! alongside the probe. So the allocator is registered here, once, for every
//! component in a build that needs one — a per-binary `declare_heap!` would
//! leave `init` without the symbol and fail the link.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr;
use core::sync::atomic::{AtomicUsize, Ordering};

/// A fixed region of `.bss` handed out in one direction.
///
/// `N` is the component's own bound. Exceeding it returns null, which the
/// `alloc` crate turns into `handle_alloc_error` and the panic handler turns
/// into a nonzero exit — a component that outgrew its heap fails visibly
/// rather than corrupting itself.
pub struct BumpHeap<const N: usize> {
    memory: UnsafeCell<[u8; N]>,
    next: AtomicUsize,
}

// A component is single-threaded: one seL4 TCB, no interrupts delivered to
// userspace. The `AtomicUsize` is what makes the interior mutability sound
// rather than a claim about concurrency.
unsafe impl<const N: usize> Sync for BumpHeap<N> {}

impl<const N: usize> Default for BumpHeap<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> BumpHeap<N> {
    pub const fn new() -> Self {
        Self {
            memory: UnsafeCell::new([0u8; N]),
            next: AtomicUsize::new(0),
        }
    }

    /// Bytes handed out so far. A component can print this to justify its
    /// declared bound with an observation rather than an estimate.
    pub fn used(&self) -> usize {
        self.next.load(Ordering::Relaxed)
    }

    /// The declared bound, for the same reason.
    pub const fn capacity(&self) -> usize {
        N
    }
}

unsafe impl<const N: usize> GlobalAlloc for BumpHeap<N> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let base = self.memory.get() as usize;
        let mut current = self.next.load(Ordering::Relaxed);
        loop {
            // Align relative to the real address, not the offset: the array's
            // own alignment is 1, so an offset-aligned pointer need not be.
            let Some(start) = (base + current).checked_next_multiple_of(layout.align()) else {
                return ptr::null_mut();
            };
            let offset = start - base;
            let Some(end) = offset.checked_add(layout.size()) else {
                return ptr::null_mut();
            };
            if end > N {
                return ptr::null_mut();
            }
            match self.next.compare_exchange_weak(
                current,
                end,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return start as *mut u8,
                Err(observed) => current = observed,
            }
        }
    }

    /// A no-op, deliberately. See the module comment: nothing here outlives the
    /// component, and the allocation pattern is open-index-answer-exit.
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

/// The heap bound shared by every component in a `heap`-enabled build.
///
/// Sized for the largest consumer, the store plane's probe: a 16 KiB GPT entry
/// table, an object index bounded by `MAX_OBJECTS`, and a `MAX_OBJECT_PAYLOAD`
/// (32 KiB) staging buffer, with room to open the store a second time to prove
/// a commit durable. 256 KiB leaves headroom for the bump allocator never
/// reusing the first open's memory, which is the cost of not having a free
/// list.
pub const HEAP_BYTES: usize = 256 * 1024;

#[global_allocator]
static HEAP: BumpHeap<HEAP_BYTES> = BumpHeap::new();

/// Bytes handed out so far, so a component can report its own footprint against
/// [`HEAP_BYTES`] rather than leaving the bound unjustified.
pub fn heap_used() -> usize {
    HEAP.used()
}
