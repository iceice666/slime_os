//! Live seL4 backing for the shared-buffer state machine.
//!
//! [`SharedBufferTable`](crate::shared_buffer::SharedBufferTable) is pure: it
//! decides *what* must happen and hands the decision to a
//! [`SharedBufferAdapter`]. This module is the implementation that actually
//! performs it against the kernel.
//!
//! Three properties are structural rather than advisory:
//!
//! - **Rights are the mapping.** A [`MappingRights::ReadOnly`] page is mapped
//!   with `CapRights::read_only()`, so AArch64's `maskVMRights` narrows the
//!   frame's `VMReadWrite` to `VMReadOnly` and the page tables refuse a write.
//!   Nothing here records a "read-only" flag beside a writable mapping.
//! - **Frames are data.** Every mapping carries `EXECUTE_NEVER`; a shared
//!   buffer is never executable, whatever the holder does with its contents.
//! - **Intermediate tables are owned.** Page tables allocated to cover a shared
//!   mapping are recorded in [`BufferAdapter::tables`], not leaked, so the
//!   root can account for every object it retyped.
//!
//! Frame identity crosses the pure/live boundary as a
//! [`FrameCap`], which is a root CSlot index. That index is exactly what
//! [`sel4::init_thread::Slot::from_index`] reconstitutes, so the state machine
//! stores an unforgeable number and this module recovers the capability.

use crate::object_allocator::{AllocError, ObjectAllocator};
use crate::shared_buffer::{
    AdapterAction, AdapterError, FrameCap, MappingRights, PAGE_SIZE, SharedBufferAdapter, VSpaceCap,
};

/// Intermediate translation tables one adapter may own. A shared region spans
/// far less than this; the ceiling exists so a runaway mapping request fails
/// closed instead of exhausting CSlots silently.
pub const MAX_ADAPTER_TABLES: usize = 32;

/// Frame aliases the root may hold at once, across every adapter.
///
/// One per *page* of a second-or-later mapping of the same frame, which in
/// practice means one per loaned page: the lender's mapping uses the anchor and
/// the receiver's needs a copy. Sized against `MAX_MAPPINGS`, so a graph whose
/// every mapping were an alias still fits.
pub const MAX_FRAME_ALIASES: usize = crate::shared_buffer::MAX_MAPPINGS;

/// One frame capability minted so a frame could be mapped a second time.
///
/// Keyed by the exact mapping it backs — the anchor, the VSpace, and the
/// address — because that is what an unmap names, and unmapping through the
/// anchor when the alias holds the mapping would tear down the *other* holder's
/// view instead. See [`FrameAliases`].
#[derive(Clone, Copy)]
struct AliasRecord {
    frame: FrameCap,
    vspace: VSpaceCap,
    vaddr: usize,
    alias: sel4::cap::Granule,
}

/// Root-owned registry of frame aliases, outliving any one [`BufferAdapter`].
///
/// It has to outlive the adapter because the adapter is constructed fresh for
/// each operation: the mapping is installed by one and torn down by another,
/// and both must agree about which capability records it.
///
/// Nothing here owns memory. An alias is a copy of an anchor the shared-buffer
/// table already owns, and `AdapterAction::Revoke` on that anchor drops every
/// copy along with every mapping — so a record this registry loses costs a
/// CSlot, never a page.
pub struct FrameAliases {
    entries: [Option<AliasRecord>; MAX_FRAME_ALIASES],
    len: usize,
}

impl FrameAliases {
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_FRAME_ALIASES],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn record(
        &mut self,
        frame: FrameCap,
        vspace: VSpaceCap,
        vaddr: usize,
        alias: sel4::cap::Granule,
    ) -> Result<(), BufferAdapterError> {
        let slot = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(BufferAdapterError::TablesExhausted)?;
        *slot = Some(AliasRecord {
            frame,
            vspace,
            vaddr,
            alias,
        });
        self.len += 1;
        Ok(())
    }

    /// The capability holding the mapping of `frame` at `vaddr` in `vspace`, if
    /// an alias holds it. `None` means the anchor does.
    fn take(
        &mut self,
        frame: FrameCap,
        vspace: VSpaceCap,
        vaddr: usize,
    ) -> Option<sel4::cap::Granule> {
        let slot = self.entries.iter_mut().find(|entry| {
            entry.is_some_and(|record| {
                record.frame == frame && record.vspace == vspace && record.vaddr == vaddr
            })
        })?;
        let record = slot.take()?;
        self.len -= 1;
        Some(record.alias)
    }
}

impl Default for FrameAliases {
    fn default() -> Self {
        Self::new()
    }
}

/// The root's frame-alias registry.
///
/// A static rather than a field, because it must outlive every
/// [`BufferAdapter`] — one adapter installs a mapping and a later one tears it
/// down, and both have to agree about which capability records it — while every
/// adapter is a short-lived local constructed per operation. Threading it
/// through the eight call sites that build one would put a parameter on each
/// only to reach the two places that read it.
///
/// The same shape, and for a related reason, as `main.rs`'s `CHANNELS`. Access
/// is sound because the root task is single-threaded and each borrow below ends
/// within its own statement, so no two references are ever live at once.
///
/// How many aliases are live is reported by [`live_frame_aliases`], which the
/// terminal accounting reads: an alias outstanding at teardown means a mapping
/// the root still believes exists.
static mut FRAME_ALIASES: FrameAliases = FrameAliases::new();

/// Frame aliases the root currently holds. Zero at teardown means every
/// second-holder mapping was torn down through the capability that recorded it.
pub fn live_frame_aliases() -> usize {
    // SAFETY: single-threaded; the borrow ends with this expression.
    unsafe { &*core::ptr::addr_of!(FRAME_ALIASES) }.len()
}

/// Typed failure for every live invocation this adapter performs. Nothing here
/// panics: an exhausted table, a bad capability, and a kernel error are all
/// values the caller can report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BufferAdapterError {
    /// Allocating a frame or an intermediate translation table failed.
    Alloc(AllocError),
    /// `seL4_ARM_Page_Map` failed for this exact address.
    Map { vaddr: usize, error: sel4::Error },
    /// `seL4_ARM_Page_Unmap` failed.
    Unmap { vaddr: usize, error: sel4::Error },
    /// Revoking every capability derived from a frame failed.
    Revoke { slot: usize, error: sel4::Error },
    /// Deleting the root's own frame capability failed.
    Release { slot: usize, error: sel4::Error },
    /// Mapping an intermediate translation table failed.
    TableMap { level: usize, error: sel4::Error },
    /// More intermediate tables than [`MAX_ADAPTER_TABLES`].
    TablesExhausted,
    /// A frame anchor did not name a usable root CSlot.
    BadFrameCap(FrameCap),
}

impl From<AllocError> for BufferAdapterError {
    fn from(error: AllocError) -> Self {
        Self::Alloc(error)
    }
}

impl BufferAdapterError {
    /// Collapse a live failure into the state machine's classification. The
    /// detailed error stays available on [`BufferAdapter::last_error`] for the
    /// marker the caller prints; the table only needs the category.
    const fn classify(self) -> AdapterError {
        match self {
            Self::Alloc(AllocError::UntypedExhausted { .. }) => AdapterError::OutOfMemory,
            Self::Alloc(_) | Self::TablesExhausted => AdapterError::Other,
            Self::Map { error, .. } | Self::TableMap { error, .. } => match error {
                sel4::Error::DeleteFirst => AdapterError::MapConflict,
                sel4::Error::FailedLookup => AdapterError::FailedLookup,
                sel4::Error::NotEnoughMemory => AdapterError::OutOfMemory,
                sel4::Error::InvalidCapability => AdapterError::InvalidCapability,
                _ => AdapterError::Other,
            },
            Self::Unmap { .. } => AdapterError::UnmapFailed,
            Self::Revoke { .. } => AdapterError::RevokeFailed,
            Self::Release { .. } => AdapterError::ReleaseFailed,
            Self::BadFrameCap(_) => AdapterError::InvalidCapability,
        }
    }
}

/// One intermediate translation table this adapter allocated and mapped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableRecord {
    pub slot: usize,
    pub level: usize,
    pub vaddr: usize,
}

/// The live adapter. It borrows the root's [`ObjectAllocator`] because every
/// frame and page table it creates is retyped from the same untyped pool and
/// occupies the same monotonic CSlot range the rest of startup accounts for.
pub struct BufferAdapter<'a> {
    allocator: &'a mut ObjectAllocator,
    tables: [Option<TableRecord>; MAX_ADAPTER_TABLES],
    table_len: usize,
    frames_allocated: usize,
    mapped: usize,
    unmapped: usize,
    revoked: usize,
    released: usize,
    aliased: usize,
    last_error: Option<BufferAdapterError>,
}

impl<'a> BufferAdapter<'a> {
    pub fn new(allocator: &'a mut ObjectAllocator) -> Self {
        Self {
            allocator,
            tables: [None; MAX_ADAPTER_TABLES],
            table_len: 0,
            frames_allocated: 0,
            mapped: 0,
            unmapped: 0,
            revoked: 0,
            released: 0,
            aliased: 0,
            last_error: None,
        }
    }

    /// Capabilities minted so one frame could be mapped into a second VSpace.
    pub const fn aliased(&self) -> usize {
        self.aliased
    }

    pub const fn frames_allocated(&self) -> usize {
        self.frames_allocated
    }

    pub const fn mapped(&self) -> usize {
        self.mapped
    }

    pub const fn unmapped(&self) -> usize {
        self.unmapped
    }

    pub const fn revoked(&self) -> usize {
        self.revoked
    }

    pub const fn released(&self) -> usize {
        self.released
    }

    pub const fn tables_mapped(&self) -> usize {
        self.table_len
    }

    /// The most recent live failure, with its kernel error intact.
    pub const fn last_error(&self) -> Option<BufferAdapterError> {
        self.last_error
    }

    /// Retype one 4 KiB frame and return the root CSlot anchoring it. The state
    /// machine records the returned [`FrameCap`]; this adapter recovers the
    /// capability from it later by CSlot index.
    pub fn allocate_frame(&mut self) -> Result<FrameCap, BufferAdapterError> {
        let slot = self
            .allocator
            .allocate_fixed::<sel4::cap_type::Granule>()
            .map_err(BufferAdapterError::Alloc)?;
        self.frames_allocated += 1;
        Ok(FrameCap(slot.index()))
    }

    /// Ensure every intermediate translation table covering `vaddr` exists in
    /// `vspace`, allocating and recording the ones that do not.
    ///
    /// A level whose table is already present answers `DeleteFirst`, which is
    /// success for this purpose: the mapping only needs the table to exist, not
    /// to have been created here. Any table this call does create is retained
    /// in [`Self::tables`] so it is owned rather than leaked.
    fn ensure_tables(
        &mut self,
        vspace: sel4::cap::VSpace,
        vaddr: usize,
    ) -> Result<(), BufferAdapterError> {
        for level in 1..sel4::vspace_levels::NUM_LEVELS {
            let Some(ty) = sel4::TranslationTableObjectType::from_level(level) else {
                continue;
            };
            let span_bits = sel4::vspace_levels::span_bits(level);
            let aligned = vaddr & !((1usize << span_bits) - 1);
            if self
                .tables
                .iter()
                .flatten()
                .any(|table| table.level == level && table.vaddr == aligned)
            {
                continue;
            }
            // Reserve the bookkeeping slot before spending anything. Checking
            // after the retype-and-map would consume a CSlot and link a live
            // table into the child's page-table tree that this adapter then
            // has no room to record — the opposite of failing closed.
            if self.table_len >= self.tables.len() {
                return Err(BufferAdapterError::TablesExhausted);
            }
            let slot = self
                .allocator
                .allocate(ty.blueprint())
                .map_err(BufferAdapterError::Alloc)?;
            match slot
                .cap()
                .cast::<sel4::cap_type::UnspecifiedIntermediateTranslationTable>()
                .generic_intermediate_translation_table_map(
                    ty,
                    vspace,
                    aligned,
                    sel4::VmAttributes::default(),
                ) {
                Ok(()) => {
                    let record = self
                        .tables
                        .get_mut(self.table_len)
                        .ok_or(BufferAdapterError::TablesExhausted)?;
                    *record = Some(TableRecord {
                        slot: slot.index(),
                        level,
                        vaddr: aligned,
                    });
                    self.table_len += 1;
                }
                // The kernel already has a table at this level for this range.
                // The freshly retyped object stays in its CSlot, owned by the
                // root's monotonic allocation record, and is simply unused.
                Err(sel4::Error::DeleteFirst) => {}
                Err(error) => return Err(BufferAdapterError::TableMap { level, error }),
            }
        }
        Ok(())
    }

    fn map_frame_inner(
        &mut self,
        frame: FrameCap,
        vspace: VSpaceCap,
        vaddr: usize,
        rights: MappingRights,
    ) -> Result<(), BufferAdapterError> {
        if !vaddr.is_multiple_of(PAGE_SIZE) {
            return Err(BufferAdapterError::Map {
                vaddr,
                error: sel4::Error::AlignmentError,
            });
        }
        let vspace_cap = vspace_cap(vspace);
        self.ensure_tables(vspace_cap, vaddr)?;
        // Rights narrowing is real here: `read_only()` clears capAllowWrite, so
        // `maskVMRights` produces VMReadOnly and the page-table entry itself
        // refuses a store. A shared buffer is never executable, so every
        // mapping is EXECUTE_NEVER regardless of its read/write rights.
        let cap_rights = match rights {
            MappingRights::ReadOnly => sel4::CapRights::read_only(),
            MappingRights::ReadWrite => sel4::CapRights::read_write(),
        };
        // An seL4 frame capability records exactly one mapping. A loaned region
        // is mapped by two holders at once — that is what a loan *is* — so the
        // second mapping cannot go through the same capability: the kernel
        // answers `InvalidCapability` with "attempting to remap a frame that
        // does not belong to the passed address space", and the receiver's map
        // fails while the lender's is live.
        //
        // So a mapping that finds the anchor already mapped is installed through
        // a fresh copy of it. The copy is the same authority, not a widening:
        // rights are narrowed per mapping by `cap_rights` above, so the
        // receiver's read-only mapping stays read-only however the lender's was
        // made. The root's own anchor keeps naming the frame, and `Revoke` on it
        // drops every copy along with every mapping — which is what keeps
        // reclamation whole without the table having to know an alias exists.
        //
        // The same technique the transfer window already uses for the root's own
        // staging mapping (`child_vspace::transfer_window_alias`); this is the
        // per-mapping form of it.
        let mut cap = frame_cap(frame);
        let mut aliased = false;
        loop {
            match cap.frame_map(
                vspace_cap,
                vaddr,
                cap_rights.clone(),
                sel4::VmAttributes::default() | sel4::VmAttributes::EXECUTE_NEVER,
            ) {
                Ok(()) => break,
                // The anchor, or the alias tried, is spent on another mapping.
                // One retry through a fresh copy is enough: the copy is
                // unmapped by construction.
                Err(sel4::Error::InvalidCapability) if !aliased => {
                    aliased = true;
                    cap = self.alias_frame(frame, vaddr)?;
                }
                Err(error) => return Err(BufferAdapterError::Map { vaddr, error }),
            }
        }
        // Only now, with the mapping installed — see `record_alias`.
        if aliased {
            self.record_alias(frame, vspace, vaddr, cap)?;
        }
        self.mapped += 1;
        Ok(())
    }

    /// Mint a second capability to the frame `frame` anchors.
    ///
    /// From the same monotonic CSlot cursor as every other object, so the
    /// allocator's accounting still covers it. It is never deleted
    /// individually: `AdapterAction::Revoke` on the anchor drops every
    /// capability derived from it, which is exactly this set.
    fn alias_frame(
        &mut self,
        frame: FrameCap,
        vaddr: usize,
    ) -> Result<sel4::cap::Granule, BufferAdapterError> {
        let alias = self
            .allocator
            .reserve_slot::<sel4::cap_type::Granule>()
            .map_err(BufferAdapterError::Alloc)?
            .cap();
        let root_cnode = sel4::init_thread::slot::CNODE.cap();
        root_cnode
            .absolute_cptr(alias)
            .copy(
                &root_cnode.absolute_cptr(frame_cap(frame)),
                sel4::CapRights::read_write(),
            )
            .map_err(|error| BufferAdapterError::Map { vaddr, error })?;
        self.aliased += 1;
        Ok(alias)
    }

    /// Record that `alias` holds the mapping of `frame` at `vaddr` in `vspace`.
    ///
    /// Called only once the map has *succeeded*, which is the whole ordering
    /// question here. A record written before the attempt would be findable by
    /// nothing if the attempt then failed: [`FrameAliases::take`] runs from the
    /// `Unmap` arm alone, and an unmap is only ever emitted for a mapping that
    /// committed. The entry would sit in the registry for the rest of the boot
    /// and [`live_frame_aliases`] would never return to zero — which the loan
    /// gate's terminal `aliases=0` asserts.
    ///
    /// The capability itself is not lost by recording late: it is a copy of an
    /// anchor the shared-buffer table owns, and `AdapterAction::Revoke` on that
    /// anchor drops every copy. An unrecorded alias costs a CSlot, never a page.
    fn record_alias(
        &mut self,
        frame: FrameCap,
        vspace: VSpaceCap,
        vaddr: usize,
        alias: sel4::cap::Granule,
    ) -> Result<(), BufferAdapterError> {
        // SAFETY: the root task is single-threaded and every `BufferAdapter`
        // is a short-lived local, so no two references to the registry exist at
        // once. See `FRAME_ALIASES`.
        let registry = unsafe { &mut *core::ptr::addr_of_mut!(FRAME_ALIASES) };
        registry.record(frame, vspace, vaddr, alias)?;
        // Reported, because the terminal `aliases=0` cannot evidence this on
        // its own: a boot that never recorded an alias also ends at zero. The
        // failure that hides behind that is quiet and serious — an unrecorded
        // alias sends the unmap through the anchor, which holds the *other*
        // holder's mapping, so the wrong view is torn down and the right one
        // silently survives its own teardown.
        sel4::debug_println!(
            "SLIME_GRAPH frame aliased frame={} vaddr={vaddr:#x} live={}",
            frame.0,
            registry.len(),
        );
        Ok(())
    }

    fn perform_inner(&mut self, action: AdapterAction) -> Result<(), BufferAdapterError> {
        match action {
            // `seL4_ARM_Page_Unmap` on an already-unmapped frame is a no-op that
            // returns success, which is exactly the idempotence the trait
            // requires for a retryable teardown batch.
            AdapterAction::Unmap {
                frame,
                vspace,
                vaddr,
            } => {
                // Through whichever capability holds *this* mapping. When a
                // frame is mapped by two holders the anchor holds one and an
                // alias the other, so unmapping through the anchor
                // unconditionally would tear down the wrong holder's view and
                // leave this one live.
                // SAFETY: as in `alias_frame` — single-threaded, and the
                // borrow ends before this statement does.
                unsafe { &mut *core::ptr::addr_of_mut!(FRAME_ALIASES) }
                    .take(frame, vspace, vaddr)
                    .unwrap_or_else(|| frame_cap(frame))
                    .frame_unmap()
                    .map_err(|error| BufferAdapterError::Unmap { vaddr, error })?;
                self.unmapped += 1;
                Ok(())
            }
            // Revoke drops every capability *derived* from the root's frame
            // cap, including any mapping still installed in a child VSpace. A
            // frame with no derivations revokes successfully, so this is safe
            // to repeat.
            AdapterAction::Revoke { frame } => {
                root_cptr(frame)
                    .revoke()
                    .map_err(|error| BufferAdapterError::Revoke {
                        slot: frame.0,
                        error,
                    })?;
                self.revoked += 1;
                Ok(())
            }
            // Delete the root's own capability, emptying the CSlot. Deleting an
            // already-empty slot succeeds, keeping the batch retryable.
            AdapterAction::ReleaseFrame { frame } => {
                root_cptr(frame)
                    .delete()
                    .map_err(|error| BufferAdapterError::Release {
                        slot: frame.0,
                        error,
                    })?;
                self.released += 1;
                Ok(())
            }
        }
    }

    fn fail(&mut self, error: BufferAdapterError) -> AdapterError {
        self.last_error = Some(error);
        error.classify()
    }
}

impl SharedBufferAdapter for BufferAdapter<'_> {
    fn map_frame(
        &mut self,
        frame: FrameCap,
        vspace: VSpaceCap,
        vaddr: usize,
        rights: MappingRights,
    ) -> Result<(), AdapterError> {
        self.map_frame_inner(frame, vspace, vaddr, rights)
            .map_err(|error| self.fail(error))
    }

    fn perform(&mut self, action: AdapterAction) -> Result<(), AdapterError> {
        self.perform_inner(action).map_err(|error| self.fail(error))
    }
}

/// Recover the root-owned frame capability a [`FrameCap`] anchors. The value is
/// a root CSlot index produced by [`BufferAdapter::allocate_frame`], never a
/// caller-supplied pointer.
fn frame_cap(frame: FrameCap) -> sel4::cap::Granule {
    sel4::init_thread::Slot::<sel4::cap_type::Granule>::from_index(frame.0).cap()
}

fn vspace_cap(vspace: VSpaceCap) -> sel4::cap::VSpace {
    sel4::init_thread::Slot::<sel4::cap_type::VSpace>::from_index(vspace.0).cap()
}

/// The absolute path to a frame's slot in the root CNode, for revoke/delete.
fn root_cptr(frame: FrameCap) -> sel4::AbsoluteCPtr {
    sel4::init_thread::slot::CNODE
        .cap()
        .absolute_cptr(sel4::CPtr::from_bits(frame.0 as sel4::CPtrBits))
}
