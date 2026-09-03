//! Child address-space construction from a target-qualified ELF64 payload.
//!
//! The root task owns every frame and translation table created here. An image
//! is validated against the pinned target before a single object is allocated,
//! its loadable segments are copied through one root-owned scratch page, and the
//! child's IPC buffer is placed immediately above the image footprint. Nothing
//! in this module reuses the root's own VSpace authority for the child: the
//! child receives exactly the frames mapped into its own VSpace.

use core::ops::Range;
use core::ptr;

use object::read::elf::ElfFile64;
use object::{Architecture, Endianness, Object, ObjectSegment, ObjectSymbol, SegmentFlags};
use sel4::CapTypeForObjectOfFixedSize;

use crate::object_allocator::{AllocError, ArenaPlan, ObjectAllocator, TaskArenaId};

/// seL4 base page size for this configuration.
pub const GRANULE_SIZE: usize = sel4::FrameObjectType::GRANULE.bytes();
const _: () = assert!(GRANULE_SIZE == boot_contracts::component_runtime_abi::GRANULE_BYTES);

/// Pages one child image footprint may span, including the IPC buffer and
/// startup transfer-window pages. A larger payload fails closed rather than
/// silently truncating.
pub const MAX_CHILD_IMAGE_PAGES: usize = 512;

/// Highest child virtual address this root task will map. Both current seL4
/// profiles admit addresses below this conservative shared ceiling.
const CHILD_ADDRESS_CEILING: usize = 1usize << 38;

const FLAG_READ: u8 = 1 << 0;
const FLAG_WRITE: u8 = 1 << 1;
const FLAG_EXEC: u8 = 1 << 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageError {
    /// The payload is not a parseable ELF file.
    NotElf,
    /// The payload is not a little-endian ELF64 executable for this build's ISA.
    WrongTarget,
    /// The payload declares no loadable segment.
    NoLoadableSegment,
    /// A segment declares more file data than memory, or overflows the address
    /// space, or its data cannot be read.
    MalformedSegment,
    /// A segment uses a flag encoding this loader does not model.
    UnsupportedSegmentFlags,
    /// The image footprint exceeds [`MAX_CHILD_IMAGE_PAGES`].
    FootprintTooLarge { pages: usize, limit: usize },
    /// The image would be mapped above [`CHILD_ADDRESS_CEILING`].
    FootprintOutOfRange,
    /// Segment rights accumulated on one granule would make it writable and
    /// executable, even if no individual segment requested both.
    WritableExecutablePage,
    /// The entry point lies outside every executable segment.
    EntryNotExecutable { entry: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VSpaceError {
    /// The plan declares more threads than the runtime maps pages for.
    ThreadCount {
        requested: usize,
        limit: usize,
    },
    Image(ImageError),
    Alloc(AllocError),
    /// Assigning an ASID to the child VSpace failed.
    AsidAssign(sel4::Error),
    /// Mapping an intermediate translation table failed.
    TableMap {
        level: usize,
        error: sel4::Error,
    },
    /// Mapping a frame into the child VSpace failed.
    FrameMap {
        vaddr: usize,
        error: sel4::Error,
    },
    /// Mapping a frame into the root VSpace for loading failed.
    ScratchMap(sel4::Error),
    /// Releasing the root scratch mapping failed.
    ScratchUnmap(sel4::Error),
    /// Unifying the instruction cache over a loaded code page failed.
    UnifyInstruction(sel4::Error),
}

impl From<ImageError> for VSpaceError {
    fn from(error: ImageError) -> Self {
        Self::Image(error)
    }
}

impl From<AllocError> for VSpaceError {
    fn from(error: AllocError) -> Self {
        Self::Alloc(error)
    }
}

/// A validated native child image.
pub struct ChildImage<'a> {
    file: ElfFile64<'a, Endianness>,
    footprint: Range<usize>,
}

impl<'a> ChildImage<'a> {
    /// Parse and validate a payload against this root task's compiled ISA.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ImageError> {
        let file = ElfFile64::<Endianness>::parse(bytes).map_err(|_| ImageError::NotElf)?;
        #[cfg(target_arch = "aarch64")]
        let expected = Architecture::Aarch64;
        #[cfg(target_arch = "riscv64")]
        let expected = Architecture::Riscv64;
        #[cfg(target_arch = "x86_64")]
        let expected = Architecture::X86_64;
        if file.architecture() != expected || !file.is_little_endian() {
            return Err(ImageError::WrongTarget);
        }
        let footprint = footprint(&file)?;
        let pages = footprint.len() / GRANULE_SIZE + 2;
        if pages > MAX_CHILD_IMAGE_PAGES {
            return Err(ImageError::FootprintTooLarge {
                pages,
                limit: MAX_CHILD_IMAGE_PAGES,
            });
        }
        let image = Self { file, footprint };
        image.validate_entry()?;
        Ok(image)
    }

    pub fn entry(&self) -> u64 {
        self.file.entry()
    }

    pub fn footprint(&self) -> Range<usize> {
        self.footprint.clone()
    }

    /// The worker thread's entry point and stack, if the image declares them
    /// (B47).
    ///
    /// Resolved from the symbol table rather than a second ELF entry point,
    /// because ELF has exactly one. `slime_rt::entry!(main, worker = ...)`
    /// emits both symbols with `#[unsafe(no_mangle)]`; an image built without
    /// the worker form has neither, and returns `None` here.
    ///
    /// Both or neither: a stack with no entry point is unreachable, and an
    /// entry point with no stack would run on whatever the register held.
    pub fn worker(&self) -> Option<WorkerImage> {
        let entry = self.symbol(WORKER_ENTRY_SYMBOL)?;
        let (stack_base, stack_size) = self.symbol_with_size(WORKER_STACK_SYMBOL)?;
        let top = usize::try_from(stack_base + stack_size).ok()?;
        Some(WorkerImage {
            entry,
            // The kernel enters this thread by writing PC and SP, so what the
            // callee's ABI requires there is not the symbol's own alignment;
            // `thread_abi` owns that difference for every directly started
            // thread.
            stack_top: crate::thread_abi::initial_stack_pointer(top) as u64,
        })
    }

    fn symbol(&self, name: &str) -> Option<u64> {
        self.file
            .symbols()
            .find(|symbol| symbol.name() == Ok(name))
            .map(|symbol| symbol.address())
    }

    fn symbol_with_size(&self, name: &str) -> Option<(u64, u64)> {
        self.file
            .symbols()
            .find(|symbol| symbol.name() == Ok(name))
            .map(|symbol| (symbol.address(), symbol.size()))
    }

    /// Pages the image itself occupies, excluding per-thread runtime pages.
    pub fn image_pages(&self) -> usize {
        self.footprint.len() / GRANULE_SIZE
    }

    /// Exact kernel-memory plan for the VSpace portion of this image.
    pub fn vspace_arena_plan(&self, threads: usize) -> Result<ArenaPlan, ImageError> {
        let mapped = thread_mapped_span(&self.footprint, threads)?;
        let mut plan = ArenaPlan::new();
        plan.add(sel4::cap_type::VSpace::object_blueprint())
            .ok_or(ImageError::FootprintOutOfRange)?;
        for level in 1..sel4::vspace_levels::NUM_LEVELS {
            let span_bytes = 1usize << sel4::vspace_levels::span_bits(level);
            let coarse = coarsen(&mapped, span_bytes);
            let Some(ty) = sel4::TranslationTableObjectType::from_level(level) else {
                continue;
            };
            for _ in 0..(coarse.len() / span_bytes) {
                plan.add(ty.blueprint())
                    .ok_or(ImageError::FootprintOutOfRange)?;
            }
        }
        for _ in 0..(self.image_pages() + 2 * threads) {
            plan.add(sel4::cap_type::Granule::object_blueprint())
                .ok_or(ImageError::FootprintOutOfRange)?;
        }
        Ok(plan)
    }

    /// The entry point must fall inside a mapped executable segment; otherwise
    /// activation would immediately fault on its first instruction fetch.
    fn validate_entry(&self) -> Result<(), ImageError> {
        let entry = self.file.entry();
        for segment in self.file.segments() {
            let flags = segment_flags(segment.flags())?;
            if flags & FLAG_EXEC == 0 {
                continue;
            }
            let start = segment.address();
            let end = start
                .checked_add(segment.size())
                .ok_or(ImageError::MalformedSegment)?;
            if (start..end).contains(&entry) {
                return Ok(());
            }
        }
        Err(ImageError::EntryNotExecutable { entry })
    }
}

/// The symbol `slime_rt::entry!`'s worker form emits for the second thread's
/// entry point.
const WORKER_ENTRY_SYMBOL: &str = "__slime_rt_worker_entrypoint";

/// The symbol for the second thread's stack.
const WORKER_STACK_SYMBOL: &str = "__slime_rt_worker_stack";

/// Where a component's second thread starts, resolved from its image.
#[derive(Clone, Copy, Debug)]
pub struct WorkerImage {
    pub entry: u64,
    pub stack_top: u64,
}

/// Threads one child process may run (B47).
///
/// Matches `slime_rt::runtime::MAX_THREADS`: the runtime declares one worker
/// stack and one worker entry point, and this maps one buffer/window pair per
/// thread at addresses the runtime derives from the same arithmetic. Raising it
/// means raising both.
pub const MAX_CHILD_THREADS: usize = boot_contracts::component_runtime_abi::MAX_THREADS;

/// The IPC buffer and transfer window belonging to one thread (B47).
///
/// One pair per thread, because each is thread-private by definition: the
/// kernel writes message registers into the buffer of whichever thread made the
/// call, and each thread stages its own payloads through its own window.
#[derive(Clone, Copy, Debug)]
pub struct ThreadPages {
    pub ipc_buffer_addr: usize,
    pub ipc_buffer: sel4::cap::Granule,
    /// Where this thread's transfer window is mapped. The seL4 transport stages
    /// payloads too large for the fast registers here; see
    /// [`crate::transfer_window`].
    pub transfer_window_addr: usize,
    pub transfer_window: sel4::cap::Granule,
    /// A root-held second capability to the window frame, for the root's own
    /// transient staging mapping. See [`crate::transfer_window::Window::alias`].
    pub transfer_window_alias: sel4::cap::Granule,
}

const EMPTY_THREAD_PAGES: ThreadPages = ThreadPages {
    ipc_buffer_addr: 0,
    ipc_buffer: sel4::cap::Granule::from_bits(0),
    transfer_window_addr: 0,
    transfer_window: sel4::cap::Granule::from_bits(0),
    transfer_window_alias: sel4::cap::Granule::from_bits(0),
};

/// A constructed child address space. Every capability named here is held in
/// root CSpace; the child never receives any of them.
#[derive(Clone, Copy, Debug)]
pub struct ChildVSpace {
    pub vspace: sel4::cap::VSpace,
    /// One entry per thread this child runs, in thread order. `threads` says
    /// how many are live; the rest are zeroed.
    pub pages: [ThreadPages; MAX_CHILD_THREADS],
    pub threads: usize,
    /// Base of the task-private memory window this VSpace reserves (C10.1).
    ///
    /// Address space only: its translation tables are mapped here, its leaf
    /// frames are allocated on demand by `crate::private_memory`. Carried on
    /// the VSpace rather than recomputed by the grow path, so exactly one
    /// arithmetic decides where the window is.
    pub private_base: usize,
    pub frames_mapped: usize,
    pub tables_mapped: usize,
}

impl ChildVSpace {
    /// The main thread's pages. Every child has a thread 0, so this is the
    /// accessor for the paths that predate multi-threading.
    pub fn main(&self) -> &ThreadPages {
        &self.pages[0]
    }
}

#[derive(Clone, Copy)]
struct PageEntry {
    cap: sel4::cap::Granule,
    flags: u8,
}

const EMPTY_PAGE: PageEntry = PageEntry {
    cap: sel4::cap::Granule::from_bits(0),
    flags: 0,
};

/// A root-owned page whose virtual address is reused to load child frames.
///
/// The root task's own image frame for `addr` is unmapped once, so the address
/// becomes a scratch window the loader can bind to any child frame in turn.
pub struct ScratchPage {
    addr: usize,
}

impl ScratchPage {
    /// Claim `addr` (which must be a granule-aligned address inside the root
    /// task's own image) as the loader's scratch window.
    pub fn claim(bootinfo: &sel4::BootInfo, addr: usize) -> Result<Self, VSpaceError> {
        user_image_frame(bootinfo, addr)
            .cap()
            .frame_unmap()
            .map_err(VSpaceError::ScratchUnmap)?;
        Ok(Self { addr })
    }

    pub fn addr(&self) -> usize {
        self.addr
    }
}

/// The root task's own frame capability for `addr`, which must be a
/// granule-aligned address inside the root image.
///
/// The frame is already mapped at `addr` in the root's VSpace, which is what
/// makes a root-image page usable as a second root thread's IPC buffer without
/// allocating and mapping anything (B41).
pub fn image_frame(bootinfo: &sel4::BootInfo, addr: usize) -> sel4::cap::Granule {
    user_image_frame(bootinfo, addr).cap()
}

/// The CSpace guard for the root task's own CNode.
///
/// `tcb_configure` needs this for any thread sharing the root's CSpace: a CPtr
/// resolves to `WORD_SIZE` bits and the root CNode holds only
/// `initThreadCNodeSizeBits` of them, so the remainder is guard. A zero guard
/// faults every lookup.
pub fn root_cspace_guard(bootinfo: &sel4::BootInfo) -> sel4::CNodeCapData {
    let size_bits = bootinfo.inner().initThreadCNodeSizeBits as usize;
    sel4::CNodeCapData::new(0, sel4::WORD_SIZE - size_bits)
}

fn user_image_frame(
    bootinfo: &sel4::BootInfo,
    addr: usize,
) -> sel4::init_thread::Slot<sel4::cap_type::Granule> {
    unsafe extern "C" {
        static __executable_start: usize;
    }
    let image_start = ptr::addr_of!(__executable_start) as usize;
    bootinfo
        .user_image_frames()
        .index(addr / GRANULE_SIZE - image_start / GRANULE_SIZE)
}

/// Build a child VSpace containing the image and its IPC buffer.
pub fn create_child_vspace(
    allocator: &mut ObjectAllocator,
    arena: TaskArenaId,
    image: &ChildImage<'_>,
    caller_vspace: sel4::cap::VSpace,
    scratch: &ScratchPage,
    asid_pool: sel4::cap::AsidPool,
    threads: usize,
) -> Result<ChildVSpace, VSpaceError> {
    admit_thread_count(threads)?;
    let footprint = image.footprint();
    let vspace = allocator
        .allocate_fixed_in::<sel4::cap_type::VSpace>(arena)?
        .cap();
    asid_pool
        .asid_pool_assign(vspace)
        .map_err(VSpaceError::AsidAssign)?;

    // Each thread owns an IPC buffer/window pair above the image, so the
    // translation tables must cover every pair rather than only thread 0's.
    // The arena planner uses this exact helper too: mapping a wider range than
    // it plans would make construction depend on power-of-two arena slack.
    let mapped = thread_mapped_span(&footprint, threads).map_err(VSpaceError::Image)?;
    let tables_mapped = map_intermediate_tables(allocator, arena, vspace, &mapped)?;

    let mut pages = [EMPTY_PAGE; MAX_CHILD_IMAGE_PAGES];
    let page_count = image.image_pages();
    for entry in
        pages
            .get_mut(..page_count)
            .ok_or(VSpaceError::Image(ImageError::FootprintTooLarge {
                pages: page_count,
                limit: MAX_CHILD_IMAGE_PAGES,
            }))?
    {
        entry.cap = allocator
            .allocate_fixed_in::<sel4::cap_type::Granule>(arena)?
            .cap();
    }

    accumulate_rights(image, &footprint, &mut pages)?;
    load_segments(image, &footprint, &pages, caller_vspace, scratch)?;

    for (index, entry) in pages.iter().take(page_count).enumerate() {
        let vaddr = footprint.start + index * GRANULE_SIZE;
        entry
            .cap
            .frame_map(
                vspace,
                vaddr,
                page_rights(entry.flags),
                page_attributes(entry.flags),
            )
            .map_err(|error| VSpaceError::FrameMap { vaddr, error })?;
    }
    // Only now, with each frame mapped into the child VSpace, can the flush
    // run: `seL4_ARM_Page_Unify_Instruction` resolves the range through the
    // frame's own mapping and rejects an unmapped frame with
    // `IllegalOperation`.
    unify_instruction_cache(&pages, page_count)?;

    let mut thread_pages = [EMPTY_THREAD_PAGES; MAX_CHILD_THREADS];
    for (index, slot) in thread_pages.iter_mut().enumerate().take(threads) {
        *slot = map_thread_pages(allocator, arena, vspace, footprint.end, index)?;
    }

    Ok(ChildVSpace {
        vspace,
        pages: thread_pages,
        threads,
        // The window's tables were mapped above, inside `mapped`: the span
        // `thread_mapped_span` returned already covers it, so a growth
        // allocates leaf frames only and can never need a table.
        private_base: private_window(&footprint, threads)
            .map_err(VSpaceError::Image)?
            .start,
        frames_mapped: page_count + 2 * threads,
        tables_mapped,
    })
}

/// Maps thread `index`'s IPC buffer and transfer window.
///
/// The pairs sit above the image in thread order — buffer at `base`, window one
/// granule up, then the next thread's pair — which is the arithmetic
/// `slime_rt::runtime::thread_ipc_buffer_addr` performs from its own `_end`.
/// Neither image holds a table the other could disagree with.
fn map_thread_pages(
    allocator: &mut ObjectAllocator,
    arena: TaskArenaId,
    vspace: sel4::cap::VSpace,
    base: usize,
    index: usize,
) -> Result<ThreadPages, VSpaceError> {
    let ipc_buffer_addr = base + index * 2 * GRANULE_SIZE;
    let ipc_buffer = allocator
        .allocate_fixed_in::<sel4::cap_type::Granule>(arena)?
        .cap();
    ipc_buffer
        .frame_map(
            vspace,
            ipc_buffer_addr,
            sel4::CapRights::read_write(),
            crate::vm_attributes::data(),
        )
        .map_err(|error| VSpaceError::FrameMap {
            vaddr: ipc_buffer_addr,
            error,
        })?;

    // The startup transfer window, one granule above the IPC buffer. It is
    // mapped by the root rather than allocated by the child on purpose: a
    // component the generation grants no `SharedBufferFactory` — `console` and
    // every spawned application — could not create one for itself, yet still
    // needs `recv` to work. Placing it by construction keeps the window a
    // property of the address space the root built, not an authority the child
    // had to be given.
    let transfer_window_addr = ipc_buffer_addr + GRANULE_SIZE;
    let transfer_window = allocator
        .allocate_fixed_in::<sel4::cap_type::Granule>(arena)?
        .cap();
    transfer_window
        .frame_map(
            vspace,
            transfer_window_addr,
            sel4::CapRights::read_write(),
            crate::vm_attributes::data(),
        )
        .map_err(|error| VSpaceError::FrameMap {
            vaddr: transfer_window_addr,
            error,
        })?;
    // A second capability to that same frame, kept in root CSpace so the root
    // can map the window at its scratch address to stage a payload through it.
    // A frame capability records exactly one mapping, and the one above is the
    // child's; without a copy the root could only read the window by first
    // unmapping a live child's own view of it.
    //
    // Allocated from the same cursor as every other object this task owns, so
    // the task's cleanup record already covers it and teardown still reaches
    // zero.
    let transfer_window_alias = allocator
        .reserve_slot_in::<sel4::cap_type::Granule>(arena)?
        .cap();
    let root_cnode = sel4::init_thread::slot::CNODE.cap();
    root_cnode
        .absolute_cptr(transfer_window_alias)
        .copy(
            &root_cnode.absolute_cptr(transfer_window),
            sel4::CapRights::read_write(),
        )
        .map_err(|error| VSpaceError::FrameMap {
            vaddr: transfer_window_addr,
            error,
        })?;

    Ok(ThreadPages {
        ipc_buffer_addr,
        ipc_buffer,
        transfer_window_addr,
        transfer_window,
        transfer_window_alias,
    })
}

fn map_intermediate_tables(
    allocator: &mut ObjectAllocator,
    arena: TaskArenaId,
    vspace: sel4::cap::VSpace,
    footprint: &Range<usize>,
) -> Result<usize, VSpaceError> {
    let mut mapped = 0;
    for level in 1..sel4::vspace_levels::NUM_LEVELS {
        let span_bytes = 1usize << sel4::vspace_levels::span_bits(level);
        let coarse = coarsen(footprint, span_bytes);
        let Some(ty) = sel4::TranslationTableObjectType::from_level(level) else {
            continue;
        };
        for index in 0..(coarse.len() / span_bytes) {
            let addr = coarse.start + index * span_bytes;
            allocator
                .allocate_in(arena, ty.blueprint())?
                .cap()
                .cast::<sel4::cap_type::UnspecifiedIntermediateTranslationTable>()
                .generic_intermediate_translation_table_map(
                    ty,
                    vspace,
                    addr,
                    sel4::VmAttributes::default(),
                )
                .map_err(|error| VSpaceError::TableMap { level, error })?;
            mapped += 1;
        }
    }
    Ok(mapped)
}

/// Union each segment's access rights into every page it spans, so a page
/// shared by two segments ends up with exactly the rights both need.
fn accumulate_rights(
    image: &ChildImage<'_>,
    footprint: &Range<usize>,
    pages: &mut [PageEntry],
) -> Result<(), ImageError> {
    for segment in image.file.segments() {
        let flags = segment_flags(segment.flags())?;
        let start = usize::try_from(segment.address()).map_err(|_| ImageError::MalformedSegment)?;
        let size = usize::try_from(segment.size()).map_err(|_| ImageError::MalformedSegment)?;
        let end = start
            .checked_add(size)
            .ok_or(ImageError::MalformedSegment)?;
        let span = coarsen(&(start..end), GRANULE_SIZE);
        let first = span
            .start
            .checked_sub(footprint.start)
            .ok_or(ImageError::MalformedSegment)?
            / GRANULE_SIZE;
        let count = span.len() / GRANULE_SIZE;
        let entries = pages
            .get_mut(first..first + count)
            .ok_or(ImageError::MalformedSegment)?;
        for entry in entries {
            entry.flags |= flags;
        }
    }
    reject_writable_executable(pages.iter().map(|entry| entry.flags))?;
    Ok(())
}

/// Copy each segment's file data into the frames backing it, one granule at a
/// time, through the single root-owned scratch mapping. Frames arrive zeroed
/// from `untyped_retype`, so `.bss` needs no explicit clearing.
fn load_segments(
    image: &ChildImage<'_>,
    footprint: &Range<usize>,
    pages: &[PageEntry],
    caller_vspace: sel4::cap::VSpace,
    scratch: &ScratchPage,
) -> Result<(), VSpaceError> {
    for segment in image.file.segments() {
        let mut vaddr =
            usize::try_from(segment.address()).map_err(|_| ImageError::MalformedSegment)?;
        let mem_size = usize::try_from(segment.size()).map_err(|_| ImageError::MalformedSegment)?;
        let mut data = segment.data().map_err(|_| ImageError::MalformedSegment)?;
        if data.len() > mem_size {
            return Err(ImageError::MalformedSegment.into());
        }
        while !data.is_empty() {
            let page_base = round_down(vaddr, GRANULE_SIZE);
            let index = page_base
                .checked_sub(footprint.start)
                .ok_or(ImageError::MalformedSegment)?
                / GRANULE_SIZE;
            let entry = pages.get(index).ok_or(ImageError::MalformedSegment)?;
            let offset = vaddr - page_base;
            let len = (GRANULE_SIZE - offset).min(data.len());
            entry
                .cap
                .frame_map(
                    caller_vspace,
                    scratch.addr(),
                    sel4::CapRights::read_write(),
                    sel4::VmAttributes::default(),
                )
                .map_err(VSpaceError::ScratchMap)?;
            // SAFETY: `scratch.addr()` names a granule-aligned page that is
            // mapped read-write into this VSpace for the duration of the copy
            // and is not aliased by any live Rust reference, and `offset + len`
            // is bounded by `GRANULE_SIZE`.
            unsafe {
                ptr::copy_nonoverlapping(data.as_ptr(), (scratch.addr() + offset) as *mut u8, len);
            }
            entry.cap.frame_unmap().map_err(VSpaceError::ScratchUnmap)?;
            vaddr += len;
            data = data.get(len..).ok_or(ImageError::MalformedSegment)?;
        }
    }
    Ok(())
}

/// Make loaded code visible to the child's instruction fetch.
fn unify_instruction_cache(pages: &[PageEntry], page_count: usize) -> Result<(), VSpaceError> {
    #[cfg(target_arch = "aarch64")]
    for entry in pages.iter().take(page_count) {
        if entry.flags & FLAG_EXEC == 0 {
            continue;
        }
        let error = sel4::with_ipc_buffer_mut(|ipc_buffer| {
            ipc_buffer.inner_mut().seL4_ARM_Page_Unify_Instruction(
                entry.cap.cptr().bits(),
                0,
                GRANULE_SIZE as sel4::Word,
            )
        });
        if let Some(error) = sel4::Error::from_sys(error) {
            return Err(VSpaceError::UnifyInstruction(error));
        }
    }
    #[cfg(target_arch = "riscv64")]
    {
        let _ = (pages, page_count);
        // The root wrote the executable bytes itself; order those stores before
        // instruction fetch on this hart. RISC-V seL4 exposes no page-cache
        // maintenance invocation because the architecture is coherent here.
        unsafe {
            core::arch::asm!("fence.i", options(nostack));
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        let _ = (pages, page_count);
        // x86-64 keeps its instruction cache coherent with stores in hardware
        // and seL4 exposes no page-cache maintenance invocation here, so the
        // writes this root already performed are visible to the child's first
        // fetch. The child has not run yet and its TCB is configured after
        // this point, so no speculative fetch of the old contents can be
        // outstanding either.
    }
    Ok(())
}

fn footprint(file: &ElfFile64<'_, Endianness>) -> Result<Range<usize>, ImageError> {
    let mut start = usize::MAX;
    let mut end = 0usize;
    let mut found = false;
    for segment in file.segments() {
        let seg_start =
            usize::try_from(segment.address()).map_err(|_| ImageError::FootprintOutOfRange)?;
        let seg_size =
            usize::try_from(segment.size()).map_err(|_| ImageError::FootprintOutOfRange)?;
        let seg_end = seg_start
            .checked_add(seg_size)
            .ok_or(ImageError::FootprintOutOfRange)?;
        start = start.min(seg_start);
        end = end.max(seg_end);
        found = true;
    }
    if !found {
        return Err(ImageError::NoLoadableSegment);
    }
    let span = coarsen(&(start..end), GRANULE_SIZE);
    validate_footprint_span(&span)?;
    Ok(span)
}

fn validate_footprint_span(span: &Range<usize>) -> Result<(), ImageError> {
    thread_mapped_span(span, 1).map(|_| ())
}

pub(crate) fn admit_thread_count(threads: usize) -> Result<(), VSpaceError> {
    if threads == 0 || threads > MAX_CHILD_THREADS {
        Err(VSpaceError::ThreadCount {
            requested: threads,
            limit: MAX_CHILD_THREADS,
        })
    } else {
        Ok(())
    }
}

/// Bytes of address space one child's private-memory window reserves (C10.1).
pub const PRIVATE_WINDOW_BYTES: usize = crate::private_memory::MAX_REGION_PAGES * GRANULE_SIZE;

/// Where a child's private-memory window sits, above its image and thread
/// pages (C10.1).
///
/// Two properties are load-bearing and neither is incidental:
///
/// * **A guard granule below.** One granule of unmapped address space separates
///   the window from the last thread page, so a write running off the end of
///   either faults rather than landing in the other. Above the window nothing
///   is ever mapped, which is the upper guard.
/// * **Aligned to its own span.** The base is rounded to a whole
///   [`PRIVATE_WINDOW_BYTES`], which on this profile is exactly one leaf
///   translation-table span, so the reservation costs one table rather than
///   straddling two.
///
/// The window is address space only: its tables are mapped when the VSpace is
/// built (it is inside the range [`thread_mapped_span`] returns, so
/// [`map_intermediate_tables`] and [`ChildImage::vspace_arena_plan`] both cover
/// it already), while its leaf frames are allocated on demand by
/// [`crate::private_memory`]. Planning frames here would charge every component
/// for memory it may never ask for.
pub(crate) fn private_window(
    span: &Range<usize>,
    threads: usize,
) -> Result<Range<usize>, ImageError> {
    let base = thread_pages_end(span, threads)?
        .checked_add(GRANULE_SIZE)
        .and_then(|addr| addr.checked_next_multiple_of(PRIVATE_WINDOW_BYTES))
        .ok_or(ImageError::FootprintOutOfRange)?;
    let end = base
        .checked_add(PRIVATE_WINDOW_BYTES)
        .ok_or(ImageError::FootprintOutOfRange)?;
    Ok(base..end)
}

/// Where the per-thread IPC-buffer and transfer-window pairs end.
fn thread_pages_end(span: &Range<usize>, threads: usize) -> Result<usize, ImageError> {
    let thread_bytes = threads
        .checked_mul(2 * GRANULE_SIZE)
        .ok_or(ImageError::FootprintOutOfRange)?;
    span.end
        .checked_add(thread_bytes)
        .ok_or(ImageError::FootprintOutOfRange)
}

/// The whole address range this root maps into a child: image, thread pages,
/// and the private-memory window's reservation.
///
/// One range rather than three, because the arena planner and the table mapper
/// must agree exactly — planning a narrower span than is mapped makes
/// construction depend on power-of-two arena slack, and mapping a narrower span
/// than is planned leaves a growth needing a table nothing allocated.
fn thread_mapped_span(span: &Range<usize>, threads: usize) -> Result<Range<usize>, ImageError> {
    let mapped_end = private_window(span, threads)?.end;
    if mapped_end > CHILD_ADDRESS_CEILING || span.start == 0 {
        Err(ImageError::FootprintOutOfRange)
    } else {
        Ok(span.start..mapped_end)
    }
}

fn reject_writable_executable(flags: impl IntoIterator<Item = u8>) -> Result<(), ImageError> {
    if flags
        .into_iter()
        .any(|flags| flags & (FLAG_WRITE | FLAG_EXEC) == (FLAG_WRITE | FLAG_EXEC))
    {
        Err(ImageError::WritableExecutablePage)
    } else {
        Ok(())
    }
}

fn segment_flags(flags: SegmentFlags) -> Result<u8, ImageError> {
    match flags {
        SegmentFlags::Elf { p_flags } => {
            let mut mapped = 0;
            if p_flags & object::elf::PF_R != 0 {
                mapped |= FLAG_READ;
            }
            if p_flags & object::elf::PF_W != 0 {
                mapped |= FLAG_WRITE;
            }
            if p_flags & object::elf::PF_X != 0 {
                mapped |= FLAG_EXEC;
            }
            Ok(mapped)
        }
        _ => Err(ImageError::UnsupportedSegmentFlags),
    }
}

/// seL4 frame rights for a page. Both current architecture implementations
/// narrow frame access from the read/write capability bits; `grant` is endpoint
/// authority and has no effect on a frame mapping.
fn page_rights(flags: u8) -> sel4::CapRights {
    sel4::CapRightsBuilder::none()
        .read(flags & FLAG_READ != 0)
        .write(flags & FLAG_WRITE != 0)
        .build()
}

/// Executability follows the segment's `PF_X` flag. Whether a non-executable
/// mapping is enforced by the page tables is architecture-dependent; see
/// `crate::vm_attributes`.
fn page_attributes(flags: u8) -> sel4::VmAttributes {
    if flags & FLAG_EXEC != 0 {
        crate::vm_attributes::executable()
    } else {
        crate::vm_attributes::data()
    }
}

fn coarsen(range: &Range<usize>, granularity: usize) -> Range<usize> {
    round_down(range.start, granularity)..range.end.next_multiple_of(granularity)
}

const fn round_down(value: usize, granularity: usize) -> usize {
    value - value % granularity
}

#[cfg(test)]
mod tests {
    use sel4::CapTypeForObjectOfFixedSize;

    use super::{
        CHILD_ADDRESS_CEILING, FLAG_EXEC, FLAG_READ, FLAG_WRITE, GRANULE_SIZE, ImageError,
        MAX_CHILD_IMAGE_PAGES, PRIVATE_WINDOW_BYTES, coarsen, private_window,
        reject_writable_executable, round_down, thread_mapped_span, validate_footprint_span,
    };

    #[test]
    fn coarsening_covers_partial_pages_at_both_ends() {
        assert_eq!(coarsen(&(0x1001..0x2001), 0x1000), 0x1000..0x3000);
        assert_eq!(coarsen(&(0x2000..0x3000), 0x1000), 0x2000..0x3000);
    }

    #[test]
    fn round_down_is_exact_on_boundaries() {
        assert_eq!(round_down(0x2000, 0x1000), 0x2000);
        assert_eq!(round_down(0x2fff, 0x1000), 0x2000);
    }

    #[test]
    fn flags_are_distinct_bits() {
        assert_eq!(FLAG_READ | FLAG_WRITE | FLAG_EXEC, 0b111);
    }

    #[test]
    fn accumulated_writable_executable_page_is_rejected() {
        assert_eq!(
            reject_writable_executable([FLAG_READ | FLAG_EXEC, FLAG_WRITE | FLAG_EXEC]),
            Err(ImageError::WritableExecutablePage)
        );
        assert_eq!(
            reject_writable_executable([FLAG_READ | FLAG_EXEC, FLAG_READ | FLAG_WRITE]),
            Ok(())
        );
    }

    #[test]
    fn loader_headroom_reserves_the_thread_pair_and_the_private_window() {
        // The mapped span is image + thread pages + a guard granule + the
        // private-memory window, aligned up to the window's own span (C10.1),
        // so the headroom below the ceiling is that whole tail rather than the
        // two granules it was before. An image ending inside the last window
        // span has nowhere to put the window and is refused.
        let highest_valid = 0x1000..CHILD_ADDRESS_CEILING - 2 * PRIVATE_WINDOW_BYTES;
        assert_eq!(validate_footprint_span(&highest_valid), Ok(()));

        let no_room_for_the_window = 0x1000..CHILD_ADDRESS_CEILING - GRANULE_SIZE;
        assert_eq!(
            validate_footprint_span(&no_room_for_the_window),
            Err(ImageError::FootprintOutOfRange)
        );
    }

    #[test]
    fn loader_headroom_covers_every_thread_pair() {
        let two_threads_fit = 0x1000..CHILD_ADDRESS_CEILING - 2 * PRIVATE_WINDOW_BYTES;
        assert!(thread_mapped_span(&two_threads_fit, 2).is_ok());

        // One granule short of a whole window span: the second thread's pair
        // pushes the guard past the alignment boundary, so the window would
        // start in the final span and end past the ceiling.
        let one_pair_short = 0x1000..CHILD_ADDRESS_CEILING - PRIVATE_WINDOW_BYTES;
        assert_eq!(
            thread_mapped_span(&one_pair_short, 2),
            Err(ImageError::FootprintOutOfRange)
        );
    }

    #[test]
    fn the_private_window_clears_the_thread_pages_by_at_least_one_guard_granule() {
        let footprint = 0x1000..0x1fe000;
        for threads in 1..=super::MAX_CHILD_THREADS {
            let window = private_window(&footprint, threads).unwrap();
            let thread_pages_end = footprint.end + threads * 2 * GRANULE_SIZE;
            assert!(
                window.start >= thread_pages_end + GRANULE_SIZE,
                "threads={threads}: the window must not abut the last thread page"
            );
            // Span-aligned, so the reservation costs one leaf table rather
            // than straddling two.
            assert_eq!(window.start % PRIVATE_WINDOW_BYTES, 0);
            assert_eq!(window.end - window.start, PRIVATE_WINDOW_BYTES);
            // And the mapped span covers it, which is what makes a growth
            // need leaf frames only.
            assert_eq!(
                thread_mapped_span(&footprint, threads).unwrap().end,
                window.end
            );
        }
    }

    #[test]
    fn worker_pair_expands_translation_table_plan() {
        // Chosen so the second thread's pair is what crosses the boundary: with
        // one thread the guarded window base lands exactly on a span, with two
        // it is pushed into the next one. A footprint whose two counts happened
        // to align identically would assert nothing (C10.1 made that possible,
        // because the window is itself a whole span wide).
        let footprint = 0x1000..0x1fd000;
        let table_span = 2 * 1024 * 1024;
        assert_eq!(PRIVATE_WINDOW_BYTES, table_span);
        let one_thread = thread_mapped_span(&footprint, 1).unwrap();
        let two_threads = thread_mapped_span(&footprint, 2).unwrap();

        assert_eq!(coarsen(&one_thread, table_span), 0..2 * table_span);
        assert_eq!(coarsen(&two_threads, table_span), 0..3 * table_span);
    }

    #[test]
    fn maximum_image_arena_plan_is_bounded_and_deterministic() {
        let mut plan = crate::object_allocator::ArenaPlan::new();
        plan.add_size_bits(sel4::cap_type::VSpace::object_blueprint().physical_size_bits())
            .unwrap();
        for _ in 0..(MAX_CHILD_IMAGE_PAGES + 3) {
            plan.add_size_bits(12).unwrap();
        }
        let first = plan.required_size_bits().unwrap();
        assert!(
            first <= 22,
            "accepted image arena unexpectedly exceeds 4 MiB"
        );
        assert_eq!(plan.required_size_bits(), Some(first));
    }
}
