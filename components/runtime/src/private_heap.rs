//! A `GlobalAlloc` over the task's generation-declared private region (C10.3).
//!
//! C10.1 built the growth mechanism and C10.2 made a generation declare who may
//! use it and how much. Both are reachable only through
//! [`crate::private_memory_grow`], which hands back raw pages — so the declared
//! quota was live but unusable by ordinary Rust. This module is what makes
//! `Vec`, `Box`, and `String` work inside a component's own ceiling.
//!
//! Deliberately not the [`crate::BumpHeap`] shape. That allocator's premise is
//! that nothing outlives the component: a store component opens a partition,
//! indexes it, answers a bounded number of requests, and exits, so a free list
//! would only add a failure mode. A component living inside a *declared ceiling*
//! is the opposite case — its bound is a policy number a generation chose, small
//! and deliberate, and reuse is the only way to stay under it. So: a first-fit
//! free list ordered by address, coalescing on both boundaries when a block
//! comes back.
//!
//! The two allocators are mutually exclusive features rather than one
//! configurable allocator, because `#[global_allocator]` is a single symbol per
//! link and the choice is per component, not per allocation. `crate::heap`'s
//! comment records the rest of that constraint: Cargo unifies features across
//! every package in one invocation, so the builder groups a private-heap
//! component into its own `cargo build` exactly as it already does for the store
//! plane, and `just component_crate_split_check` pins the grouping.
//!
//! # Growth is batched, the ABI is not
//!
//! [`crate::private_memory_grow`] counts in target pages and stays that way: the
//! declared operation, the root's accounting, and the generation's quota are all
//! per page. Asking for one page per allocation would make every `Vec` push that
//! outgrew its capacity a syscall, so the *policy* of asking for
//! [`GROWTH_PAGES`] at a time lives here, in userspace, where changing it
//! changes no contract and no fixture.
//!
//! # Exhaustion is observable
//!
//! Reaching the ceiling returns null, which is the allocator contract's own
//! signal: `Vec::try_reserve` and the other `try_*` methods turn it into a
//! `TryReserveError` a component can match on and survive. A component that
//! instead uses `push`/`insert` gets `handle_alloc_error`, and the crate's panic
//! handler exits nonzero — visible either way, and never a silent truncation or
//! a hang.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::runtime::GRANULE;
use crate::syscall::private_memory_grow;

/// Pages requested per growth.
///
/// Four granules, 16 KiB on this profile. The number is a tradeoff between
/// syscalls and slack, and it is a *userspace* number: the operation's ABI
/// counts single pages, so raising or lowering this changes no contract, no
/// generation, and no fixture. Small enough that a component with a handful of
/// declared pages still crosses a batch boundary while its collections grow —
/// which is what makes a boot able to prove the batching works, rather than
/// leaving a whole quota served by one request.
pub const GROWTH_PAGES: usize = 4;

/// Bookkeeping written immediately below every live allocation.
///
/// `block` rather than a size alone: an allocation whose alignment forced
/// padding after the block start must return the *whole* original span on free,
/// or the padding leaks and the free list slowly stops coalescing. Recording
/// where the block began makes free the exact inverse of allocate, independent
/// of the `Layout` the caller passes back.
#[repr(C)]
struct Header {
    block: usize,
    span: usize,
}

/// A free block's own link record, stored in the block it describes.
#[repr(C)]
struct FreeBlock {
    span: usize,
    /// Address of the next free block, `0` for none. An address rather than a
    /// reference: the list lives in memory this allocator hands out, so a
    /// borrow would outlive every split and merge.
    next: usize,
}

const HEADER: usize = size_of::<Header>();
/// Every block start, span, and returned pointer is a multiple of this, so the
/// two bookkeeping records are always naturally aligned.
const ALIGN: usize = align_of::<Header>();
/// A block smaller than its own link record cannot go back on the list, so a
/// remainder this small stays with the allocation instead of being lost.
const MIN_BLOCK: usize = size_of::<FreeBlock>();

const _: () = assert!(ALIGN >= align_of::<FreeBlock>());
const _: () = assert!(MIN_BLOCK >= HEADER);

/// The private region as this allocator sees it, plus its free list.
struct Heap {
    /// Region base, or `0` for a component the generation declares no quota
    /// for. Never moves, which is why the free list can hold addresses.
    base: usize,
    /// Pages the root has backed so far.
    backed: usize,
    /// First free block, `0` when the list is empty.
    head: usize,
    /// Growth requests the root served. The evidence that a free list is being
    /// reused rather than papered over by more pages.
    growths: usize,
    /// Bytes currently handed out, counted as carved spans so it accounts for
    /// alignment padding and absorbed remainders too.
    live: usize,
    started: bool,
}

impl Heap {
    const fn new() -> Self {
        Self {
            base: 0,
            backed: 0,
            head: 0,
            growths: 0,
            live: 0,
            started: false,
        }
    }

    /// Learn the region's base and current size, once.
    ///
    /// A size query allocates nothing and needs no quota, so it is safe on
    /// first allocation and answers for a denied component too: base `0` means
    /// the generation named no quota, and every later request fails without
    /// another syscall.
    fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let Ok(region) = private_memory_grow(0) else {
            return;
        };
        self.base = region.base;
        // Normally zero. If a region arrived already backed, that span is
        // usable memory and belongs on the list rather than being stranded.
        if region.base != 0 && region.pages != 0 {
            self.backed = region.pages;
            self.release(region.base, region.pages * GRANULE);
        }
    }

    /// Total free bytes, walked rather than counted incrementally so a caller
    /// reporting its own footprint reports the list's real state.
    fn free_bytes(&self) -> usize {
        let mut total = 0;
        let mut cursor = self.head;
        while cursor != 0 {
            // SAFETY: every address on the list is a block this allocator
            // owns, inside the region, holding a live `FreeBlock`.
            let block = unsafe { &*(cursor as *const FreeBlock) };
            total += block.span;
            cursor = block.next;
        }
        total
    }

    /// Return `[block, block + span)` to the list, merging with an adjacent
    /// neighbour on either side.
    ///
    /// Coalescing on both boundaries is what keeps a long-lived component from
    /// starving inside a fixed ceiling: without it, a sequence of allocations
    /// and frees leaves the total free but no single block large enough, and the
    /// component grows instead — straight past a quota it never needed to use.
    fn release(&mut self, block: usize, span: usize) {
        let mut previous = 0usize;
        let mut cursor = self.head;
        // Address order, so an adjacency is always with the immediate
        // neighbours and a single pass can see both.
        while cursor != 0 && cursor < block {
            previous = cursor;
            // SAFETY: `cursor` is a nonzero address taken from the list, so it
            // is a block start this allocator carved: `ALIGN`-aligned, at least
            // `MIN_BLOCK` bytes long, and currently free — so it holds a live
            // `FreeBlock`.
            cursor = unsafe { (*(cursor as *const FreeBlock)).next };
        }

        let start = block;
        let mut length = span;

        // Merge forward first: the successor's link is needed either way, and
        // absorbing it here means the backward merge sees one block.
        if cursor != 0 {
            // SAFETY: `cursor` came from the list and is nonzero, so it holds a
            // live `FreeBlock` — read before the adjacency test, which needs
            // only its address, because both branches want its span and link.
            let next = unsafe { &*(cursor as *const FreeBlock) };
            let (next_span, following) = (next.span, next.next);
            if start + length == cursor {
                length += next_span;
                cursor = following;
            }
        }

        if previous != 0 {
            // SAFETY: `previous` is a nonzero list address, so it holds a live
            // `FreeBlock`. Exclusive here because the lock is held and `next`
            // above was dropped with the read of its two fields.
            let before = unsafe { &mut *(previous as *mut FreeBlock) };
            if previous + before.span == start {
                before.span += length;
                before.next = cursor;
                return;
            }
        }

        // SAFETY: `start` is `ALIGN >= align_of::<FreeBlock>()`-aligned, because
        // every caller passes either a `Header::block` recorded by `take` from a
        // list address or a growth's `previous.pages * GRANULE` offset from the
        // granule-aligned base. And `length >= MIN_BLOCK`: `take` folds any
        // remainder below that into the allocation it records, and a growth's
        // span is a whole granule. So the record fits inside the block.
        unsafe {
            (start as *mut FreeBlock).write(FreeBlock {
                span: length,
                next: cursor,
            })
        }
        if previous == 0 {
            self.head = start;
        } else {
            // SAFETY: `previous` is a nonzero list address holding a live
            // `FreeBlock`, and `start` is now initialized as its successor.
            unsafe { (*(previous as *mut FreeBlock)).next = start }
        }
    }

    /// First-fit: the first block whose aligned interior can hold the request.
    ///
    /// First-fit rather than best-fit on purpose. Best-fit needs the whole list
    /// walked for every allocation and, at these sizes, mostly buys the
    /// difference between two small remainders — while first-fit over an
    /// address-ordered list keeps allocations low in the region, which is
    /// exactly where coalescing wants them.
    fn take(&mut self, size: usize, align: usize) -> *mut u8 {
        let mut previous = 0usize;
        let mut cursor = self.head;
        while cursor != 0 {
            // SAFETY: `cursor` is a nonzero list address, so it is a block start
            // this allocator carved — `ALIGN`-aligned and at least `MIN_BLOCK`
            // bytes long — holding a live `FreeBlock`.
            let block = unsafe { &*(cursor as *const FreeBlock) };
            let (span, next) = (block.span, block.next);
            let user = match (cursor + HEADER).checked_next_multiple_of(align) {
                Some(user) => user,
                None => return ptr::null_mut(),
            };
            let Some(end) = user.checked_add(size) else {
                return ptr::null_mut();
            };
            // Rounded up so the *next* block start stays aligned; a block whose
            // span left an unaligned boundary would put the two bookkeeping
            // records at unaligned addresses.
            let Some(carved) = end.checked_next_multiple_of(ALIGN) else {
                return ptr::null_mut();
            };
            if carved > cursor + span {
                previous = cursor;
                cursor = next;
                continue;
            }

            // A remainder too small to hold a link record stays with the
            // allocation. Recording that in the header is what lets `dealloc`
            // give it back — dropping it instead would lose a few words per
            // allocation, permanently.
            let remainder = cursor + span - carved;
            let taken = if remainder >= MIN_BLOCK {
                carved - cursor
            } else {
                span
            };
            let successor = if remainder >= MIN_BLOCK {
                // SAFETY: `carved` is `ALIGN`-aligned, inside the block, and
                // has at least `MIN_BLOCK` bytes ahead of it.
                unsafe {
                    (carved as *mut FreeBlock).write(FreeBlock {
                        span: remainder,
                        next,
                    })
                }
                carved
            } else {
                next
            };
            if previous == 0 {
                self.head = successor;
            } else {
                // SAFETY: `previous` is a nonzero list address holding a live
                // `FreeBlock`, and `successor` is either zero, an existing list
                // address, or the `carved` record just written above.
                unsafe { (*(previous as *mut FreeBlock)).next = successor }
            }

            // SAFETY: `user - HEADER >= cursor` because `user >= cursor +
            // HEADER`, and `user` is at least `ALIGN`-aligned, so the record is
            // aligned and inside the carved span.
            unsafe {
                ((user - HEADER) as *mut Header).write(Header {
                    block: cursor,
                    span: taken,
                })
            }
            self.live += taken;
            return user as *mut u8;
        }
        ptr::null_mut()
    }

    /// Ask the root for at least `pages` more, in batches.
    ///
    /// Two attempts, not one: a batch refused by the declared quota does not
    /// mean the quota is exhausted — a component with three pages left is
    /// refused a four-page batch and would otherwise fail an allocation its
    /// ceiling could still serve. The retry asks for exactly what the request
    /// needs, so batching stays an optimization rather than a lowered ceiling.
    fn grow(&mut self, pages: usize) -> bool {
        if self.base == 0 {
            return false;
        }
        let batch = pages.max(GROWTH_PAGES);
        // The *root's* count of what was backed before this growth, not this
        // allocator's. `private_memory_grow` is a public component API and
        // `private_memory_probe` already calls it directly, so a component may
        // legitimately grow its own region outside the allocator. Deriving the
        // appended span from a private counter would then name an address range
        // that is already backed and in use, put it on the free list, and hand
        // a caller a pointer into live memory. Taking the answer the root just
        // gave makes that divergence unrepresentable rather than detected: any
        // page grown behind the allocator's back simply never joins its list.
        let (previous, granted) = match private_memory_grow(batch) {
            Ok(region) => (region, batch),
            Err(_) if batch > pages => match private_memory_grow(pages) {
                Ok(region) => (region, pages),
                Err(_) => return false,
            },
            Err(_) => return false,
        };
        // The region grows contiguously from a base that never moves, so the
        // new span begins exactly where the backed pages ended. `release`
        // therefore merges it with the trailing free block whenever there is
        // one, which is what keeps a batched growth from fragmenting the tail.
        let appended = previous.base + previous.pages * GRANULE;
        self.backed = previous.pages + granted;
        self.growths += 1;
        self.release(appended, granted * GRANULE);
        true
    }
}

/// The private-region allocator.
///
/// One per component, registered below. The lock is a real spin lock rather
/// than the `AtomicUsize` [`crate::BumpHeap`] gets away with: a bump allocator's
/// whole state is one offset, so a compare-exchange loop *is* the allocator,
/// while a free list mutates several links per operation and a second thread
/// observing a half-relinked list would read a torn block. A component may run
/// a worker thread (`MAX_THREADS` is 2), so that case is reachable rather than
/// hypothetical.
pub struct PrivateHeap {
    locked: AtomicBool,
    heap: UnsafeCell<Heap>,
}

// SAFETY: every access goes through `with`, which holds `locked` for the whole
// critical section.
unsafe impl Sync for PrivateHeap {}

impl Default for PrivateHeap {
    fn default() -> Self {
        Self::new()
    }
}

impl PrivateHeap {
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
            heap: UnsafeCell::new(Heap::new()),
        }
    }

    fn with<R>(&self, body: impl FnOnce(&mut Heap) -> R) -> R {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // Yield rather than spin. The holder blocks in a root IPC inside
            // this critical section, the plane runs one core, and
            // `slime-root/src/task.rs` admits a per-thread worker priority
            // independent of the main thread's — so a worker the generation gave
            // the higher priority would spin forever while the runnable holder
            // is never scheduled. That is the hang this module's header promises
            // cannot happen, and a bump allocator avoids it only because its
            // critical section is a CAS with no syscall in it.
            crate::syscall::yield_now();
        }
        // SAFETY: the lock is held, so this is the only live reference.
        let heap = unsafe { &mut *self.heap.get() };
        heap.start();
        let outcome = body(heap);
        self.locked.store(false, Ordering::Release);
        outcome
    }
}

// SAFETY: `alloc` returns either null or a pointer to a span carved out of this
// component's private region, aligned to at least the requested alignment, sized
// at least the requested size, and owned by the caller until it comes back
// through `dealloc` — no span is ever handed out twice, because `take` unlinks
// the block it carves from the free list before returning. The allocator is
// itself allocation-free, so it cannot re-enter, and every access is serialized
// by `with`'s lock.
unsafe impl GlobalAlloc for PrivateHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(1);
        // At least `ALIGN`, so the header below the returned pointer is aligned
        // for its own reads. Over-aligning an allocation is always sound.
        let align = layout.align().max(ALIGN);
        self.with(|heap| {
            let first = heap.take(size, align);
            if !first.is_null() {
                return first;
            }
            // The free list could not serve it, which is the only thing that
            // justifies spending quota: growth is a consequence of exhaustion,
            // never a policy applied per allocation.
            let Some(needed) = HEADER
                .checked_add(align)
                .and_then(|head| head.checked_add(size))
            else {
                return ptr::null_mut();
            };
            if !heap.grow(needed.div_ceil(GRANULE)) {
                return ptr::null_mut();
            }
            // One retry suffices: the growth appended at least `needed` bytes as
            // a single span, so a second failure would mean the request cannot
            // be served at any size.
            heap.take(size, align)
        })
    }

    unsafe fn dealloc(&self, pointer: *mut u8, _layout: Layout) {
        if pointer.is_null() {
            return;
        }
        // SAFETY: the caller guarantees this pointer came from `alloc`, which
        // wrote a `Header` immediately below it.
        let header = unsafe { ((pointer as usize - HEADER) as *const Header).read() };
        self.with(|heap| {
            heap.live -= header.span;
            heap.release(header.block, header.span);
        });
    }
}

/// What a component's private heap is currently doing.
///
/// `growths` is the load-bearing field: it is how a component — or a gate
/// reading its report — can tell reuse from consumption. A workload that frees
/// and reallocates without this number moving is one whose free list is working.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PrivateHeapStats {
    /// Region base, `0` for a component with no declared quota.
    pub base: usize,
    /// Pages the root has backed.
    pub pages: usize,
    /// Growth requests the root served.
    pub growths: usize,
    /// Bytes currently handed out.
    pub live: usize,
    /// Bytes on the free list.
    pub free: usize,
}

/// Read the private heap's own accounting, so a component can justify its
/// declared quota with an observation rather than an estimate.
pub fn private_heap_stats() -> PrivateHeapStats {
    PRIVATE_HEAP.with(|heap| PrivateHeapStats {
        base: heap.base,
        pages: heap.backed,
        growths: heap.growths,
        live: heap.live,
        free: heap.free_bytes(),
    })
}

#[global_allocator]
static PRIVATE_HEAP: PrivateHeap = PrivateHeap::new();
