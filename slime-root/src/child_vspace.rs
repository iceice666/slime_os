//! Child address-space construction from an AArch64 ELF64 payload.
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
use object::{Architecture, Endianness, Object, ObjectSegment, SegmentFlags};

use crate::object_allocator::{AllocError, ObjectAllocator};

/// seL4 base page size for this configuration.
pub const GRANULE_SIZE: usize = sel4::FrameObjectType::GRANULE.bytes();

/// Pages one child image footprint may span, including its IPC buffer page.
/// A larger payload fails closed rather than silently truncating.
pub const MAX_CHILD_IMAGE_PAGES: usize = 512;

/// Highest child virtual address this root task will map. AArch64 user VAs are
/// 48-bit; the bound keeps footprint arithmetic inside a single `usize`.
const CHILD_ADDRESS_CEILING: usize = 1usize << 40;

const FLAG_READ: u8 = 1 << 0;
const FLAG_WRITE: u8 = 1 << 1;
const FLAG_EXEC: u8 = 1 << 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageError {
    /// The payload is not a parseable ELF file.
    NotElf,
    /// The payload is not an AArch64 little-endian ELF64 executable.
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
    /// The entry point lies outside every executable segment.
    EntryNotExecutable { entry: u64 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VSpaceError {
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

/// A validated AArch64 child image.
pub struct ChildImage<'a> {
    file: ElfFile64<'a, Endianness>,
    footprint: Range<usize>,
}

impl<'a> ChildImage<'a> {
    /// Parse and validate a payload against the pinned AArch64 target.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, ImageError> {
        let file = ElfFile64::<Endianness>::parse(bytes).map_err(|_| ImageError::NotElf)?;
        if file.architecture() != Architecture::Aarch64 || !file.is_little_endian() {
            return Err(ImageError::WrongTarget);
        }
        let footprint = footprint(&file)?;
        let pages = footprint.len() / GRANULE_SIZE + 1;
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

    /// Pages the image itself occupies, excluding the IPC buffer page.
    pub fn image_pages(&self) -> usize {
        self.footprint.len() / GRANULE_SIZE
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

/// A constructed child address space. Every capability named here is held in
/// root CSpace; the child never receives any of them.
#[derive(Clone, Copy, Debug)]
pub struct ChildVSpace {
    pub vspace: sel4::cap::VSpace,
    pub ipc_buffer_addr: usize,
    pub ipc_buffer: sel4::cap::Granule,
    /// Where this child's startup transfer window is mapped. The seL4 transport
    /// stages payloads too large for the fast registers here; see
    /// [`crate::transfer_window`].
    pub transfer_window_addr: usize,
    pub transfer_window: sel4::cap::Granule,
    pub frames_mapped: usize,
    pub tables_mapped: usize,
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
    image: &ChildImage<'_>,
    caller_vspace: sel4::cap::VSpace,
    scratch: &ScratchPage,
    asid_pool: sel4::cap::AsidPool,
) -> Result<ChildVSpace, VSpaceError> {
    let footprint = image.footprint();
    let vspace = allocator.allocate_fixed::<sel4::cap_type::VSpace>()?.cap();
    asid_pool
        .asid_pool_assign(vspace)
        .map_err(VSpaceError::AsidAssign)?;

    // The IPC buffer sits in the granule directly above the image, and the
    // startup transfer window in the granule above that, so the translation
    // tables must cover two pages more than the image footprint.
    let mapped = footprint.start..(footprint.end + 2 * GRANULE_SIZE);
    let tables_mapped = map_intermediate_tables(allocator, vspace, &mapped)?;

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
        entry.cap = allocator.allocate_fixed::<sel4::cap_type::Granule>()?.cap();
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

    let ipc_buffer_addr = footprint.end;
    let ipc_buffer = allocator.allocate_fixed::<sel4::cap_type::Granule>()?.cap();
    ipc_buffer
        .frame_map(
            vspace,
            ipc_buffer_addr,
            sel4::CapRights::read_write(),
            sel4::VmAttributes::DEFAULT | sel4::VmAttributes::EXECUTE_NEVER,
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
    let transfer_window = allocator.allocate_fixed::<sel4::cap_type::Granule>()?.cap();
    transfer_window
        .frame_map(
            vspace,
            transfer_window_addr,
            sel4::CapRights::read_write(),
            sel4::VmAttributes::DEFAULT | sel4::VmAttributes::EXECUTE_NEVER,
        )
        .map_err(|error| VSpaceError::FrameMap {
            vaddr: transfer_window_addr,
            error,
        })?;

    Ok(ChildVSpace {
        vspace,
        ipc_buffer_addr,
        ipc_buffer,
        transfer_window_addr,
        transfer_window,
        frames_mapped: page_count + 2,
        tables_mapped,
    })
}

fn map_intermediate_tables(
    allocator: &mut ObjectAllocator,
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
                .allocate(ty.blueprint())?
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
///
/// `load_segments` writes through a data-cached scratch mapping in the *root's*
/// VSpace. On AArch64 the D-cache and I-cache are not coherent, so without a
/// clean-to-PoU plus I-cache invalidate the child can fetch stale instructions
/// from a line that was never written back. `seL4_ARM_Page_Unify_Instruction`
/// performs exactly that pair. It is invoked after the frame is mapped into
/// the child VSpace — the kernel resolves the flush range through that mapping
/// and rejects an unmapped frame — and its offsets are frame-relative.
fn unify_instruction_cache(pages: &[PageEntry], page_count: usize) -> Result<(), VSpaceError> {
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
    if span.end + GRANULE_SIZE > CHILD_ADDRESS_CEILING || span.start == 0 {
        return Err(ImageError::FootprintOutOfRange);
    }
    Ok(span)
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

/// seL4 frame rights for a page. AArch64 `maskVMRights`
/// (`deps/sel4/src/arch/arm/64/kernel/vspace.c`) reads only `capAllowRead` and
/// `capAllowWrite`; `grant` is endpoint authority and has no effect on a frame
/// mapping, so executability is NOT expressed here — see [`page_attributes`].
fn page_rights(flags: u8) -> sel4::CapRights {
    sel4::CapRightsBuilder::none()
        .read(flags & FLAG_READ != 0)
        .write(flags & FLAG_WRITE != 0)
        .build()
}

/// Executability is the `seL4_ARM_ExecuteNever` attribute, and
/// `VmAttributes::DEFAULT` does not set it. Without this every child page
/// would map executable, so a data or stack page is explicitly marked
/// execute-never and only a `PF_X` segment stays executable.
fn page_attributes(flags: u8) -> sel4::VmAttributes {
    if flags & FLAG_EXEC != 0 {
        sel4::VmAttributes::DEFAULT
    } else {
        sel4::VmAttributes::DEFAULT | sel4::VmAttributes::EXECUTE_NEVER
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
    use super::{FLAG_EXEC, FLAG_READ, FLAG_WRITE, coarsen, round_down};

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
}
