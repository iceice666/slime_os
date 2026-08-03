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
            last_error: None,
        }
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
        frame_cap(frame)
            .frame_map(
                vspace_cap,
                vaddr,
                cap_rights,
                sel4::VmAttributes::default() | sel4::VmAttributes::EXECUTE_NEVER,
            )
            .map_err(|error| BufferAdapterError::Map { vaddr, error })?;
        self.mapped += 1;
        Ok(())
    }

    fn perform_inner(&mut self, action: AdapterAction) -> Result<(), BufferAdapterError> {
        match action {
            // `seL4_ARM_Page_Unmap` on an already-unmapped frame is a no-op that
            // returns success, which is exactly the idempotence the trait
            // requires for a retryable teardown batch.
            AdapterAction::Unmap { frame, vaddr, .. } => {
                frame_cap(frame)
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
