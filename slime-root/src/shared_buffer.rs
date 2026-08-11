//! Bounded shared-buffer accounting behind a live seL4 mapping adapter.
//!
//! The root task owns the frame capabilities named here. This module records
//! only unforgeable numeric identities and root-owned capability anchors; it
//! never grants ambient access to either. Authority is table-held: rights come
//! from the recorded region state and per-holder ceilings come from the quotas
//! a generation declared through [`SharedBufferTable::declare_quota`]. A
//! caller-supplied [`BufferHandle`] can only ever narrow what the table already
//! grants, and can never name a holder the table does not record as the owner.
//! Mutations preflight all logical checks before invoking
//! [`SharedBufferAdapter`], then commit only after every required seL4
//! operation succeeds.
//!
//! ## What the live adapter observes
//!
//! `slime-root/src/buffer_adapter.rs` implements [`SharedBufferAdapter`] over
//! real seL4 invocations, and `main` drives it against a running child task.
//! The boot record therefore observes, as mechanism rather than bookkeeping:
//!
//! - a 4 KiB frame retyped from a kernel untyped and mapped into a child
//!   VSpace at the exact virtual address a [`MappingRecord`] names;
//! - a child reading back the exact bytes the root wrote through that frame;
//! - a [`MappingRights::ReadOnly`] mapping refusing a child write, enforced by
//!   the AArch64 page tables (the kernel's `maskVMRights` narrows the frame
//!   cap's `VMReadWrite` to `VMReadOnly`), observed as an attributable VM fault
//!   rather than as a rejected bookkeeping flag;
//! - teardown returning every frame, mapping, and per-holder charge to zero.
//!
//! ## What is still not observed
//!
//! Loans ([`SharedBufferTable::loan`], [`SharedBufferTable::map_loan`]),
//! [`SharedBufferTable::seal`] remapping of live writable mappings, and
//! [`SharedBufferTable::advance_epoch`] are exercised only by the unit tests
//! below against a recording adapter; no boot marker covers them yet. Adapter
//! failure handling — rollback, orphan retention, and teardown retry — is
//! likewise unit-tested but has never been triggered by a real seL4 error in
//! the boot record.

use alloc::boxed::Box;
use core::fmt;

/// seL4's base-page size for the AArch64 configuration used by Slime.
pub const PAGE_SIZE: usize = 4096;
/// Hard ceiling on one shared buffer.
pub const MAX_BUFFER_PAGES: usize = 64;
/// Hard ceiling on pages retained by all live buffers.
pub const MAX_TOTAL_PAGES: usize = 256;
/// Hard ceiling on live shared-buffer objects.
pub const MAX_SHARED_BUFFERS: usize = 32;
/// Hard ceiling on exact live mappings.
pub const MAX_MAPPINGS: usize = 64;
/// Maximum page mappings admitted across committed mappings and rollback
/// orphans. This is also the exact worst-case alias-record state space.
pub const MAX_MAPPING_PAGES: usize = MAX_MAPPINGS * MAX_BUFFER_PAGES;
/// Hard ceiling on outstanding receiver-bound loans.
pub const MAX_LOANS: usize = 64;
/// One frame-cap anchor is retained for every page of every live buffer.
pub const MAX_FRAME_ANCHORS: usize = MAX_TOTAL_PAGES;
/// A teardown can unmap every mapping, revoke every frame, then release every
/// frame anchor. The fixed bound is deliberately independent of stack growth.
pub const MAX_TEARDOWN_ACTIONS: usize = MAX_MAPPING_PAGES + MAX_FRAME_ANCHORS * 2;
/// Pages whose unmap the adapter failed to complete and which are therefore
/// still live in some VSpace. Admission reserves this same per-page state
/// space across committed mappings and orphans before any adapter call.
pub const MAX_ORPHANS: usize = MAX_MAPPING_PAGES;
const MAX_CHARGE_HOLDERS: usize = MAX_SHARED_BUFFERS + MAX_MAPPINGS;
const AARCH64_USER_TOP: usize = 1usize << 40;

/// A supervision-subtree identity local to one generation epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct HolderId(pub u64);

/// Monotonic generation epoch. Every externally supplied handle is checked
/// against the table's current epoch before any adapter call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct GenerationEpoch(pub u64);

/// Kernel-created shared-buffer identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BufferId(pub u64);

/// Kernel-created, single-return loan identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LoanId(pub u64);

/// Root CSpace slot retaining the authoritative frame capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FrameCap(pub usize);

/// Root CSpace slot retaining a child VSpace capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct VSpaceCap(pub usize);

/// Generation-declared per-holder live ceilings. An absent holder receives
/// [`HolderQuota::DENY`]; authority is never ambient.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HolderQuota {
    pub byte_pages: u32,
    pub buffer_count: u32,
    pub mapping_count: u32,
    pub loan_count: u32,
}

impl HolderQuota {
    pub const DENY: Self = Self {
        byte_pages: 0,
        buffer_count: 0,
        mapping_count: 0,
        loan_count: 0,
    };
}

/// Rights that a holder may exercise through a particular buffer handle.
/// `READ` is implicit in `MAP`; write and loan authority remain separately
/// narrowable generation rights.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct BufferRights(u8);

impl BufferRights {
    pub const NONE: Self = Self(0);
    pub const MAP: Self = Self(1 << 0);
    pub const WRITE: Self = Self(1 << 1);
    pub const LOAN: Self = Self(1 << 2);
    pub const ALL: Self = Self(Self::MAP.0 | Self::WRITE.0 | Self::LOAN.0);

    pub const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }

    pub const fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

impl fmt::Debug for BufferRights {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BufferRights")
            .field("map", &self.contains(Self::MAP))
            .field("write", &self.contains(Self::WRITE))
            .field("loan", &self.contains(Self::LOAN))
            .finish()
    }
}

/// A holder-visible reference. Possession of an id alone is insufficient:
/// epoch, rights, and recorded ownership/loan bindings are checked separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferHandle {
    pub id: BufferId,
    pub epoch: GenerationEpoch,
    pub rights: BufferRights,
}

impl BufferHandle {
    /// Narrow a handle without creating authority.
    pub const fn derive(self, rights: BufferRights) -> Self {
        Self {
            rights: self.rights.intersect(rights),
            ..self
        }
    }
}

/// A receiver-bound reference to one exact sealed subrange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoanHandle {
    pub id: LoanId,
    pub buffer: BufferId,
    pub epoch: GenerationEpoch,
    pub receiver: HolderId,
}

/// Root-owned frame anchors supplied after the allocator has created real seL4
/// frame objects. The array is fixed even though only `len` entries are live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameAnchors {
    caps: [FrameCap; MAX_BUFFER_PAGES],
    len: usize,
}

impl FrameAnchors {
    pub const fn empty() -> Self {
        Self {
            caps: [FrameCap(0); MAX_BUFFER_PAGES],
            len: 0,
        }
    }

    pub fn from_slice(caps: &[FrameCap]) -> Result<Self, SharedBufferError> {
        if caps.is_empty() || caps.len() > MAX_BUFFER_PAGES {
            return Err(SharedBufferError::BadSize);
        }
        let mut anchors = Self::empty();
        for (index, cap) in caps.iter().copied().enumerate() {
            if cap.0 == 0 || anchors.caps[..index].contains(&cap) {
                return Err(SharedBufferError::BadFrameAnchors);
            }
            anchors.caps[index] = cap;
        }
        anchors.len = caps.len();
        Ok(anchors)
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, index: usize) -> Option<FrameCap> {
        (index < self.len).then(|| self.caps[index])
    }
}

/// Exact mapping protection committed in the logical table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingRights {
    ReadOnly,
    ReadWrite,
}

impl MappingRights {
    const fn writable(self) -> bool {
        matches!(self, Self::ReadWrite)
    }
}

/// Exact, externally inspectable mapping record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MappingRecord {
    pub holder: HolderId,
    pub buffer: BufferId,
    pub epoch: GenerationEpoch,
    pub vspace: VSpaceCap,
    pub base: usize,
    pub offset_pages: u16,
    pub page_count: u16,
    pub rights: MappingRights,
    pub loan: Option<LoanId>,
}

/// A deterministic adapter action. Reclamation plans are ordered by table slot,
/// then by ascending page index: unmap mappings first, revoke frame derivations
/// next, release root frame anchors last.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterAction {
    Unmap {
        frame: FrameCap,
        vspace: VSpaceCap,
        vaddr: usize,
    },
    Revoke {
        frame: FrameCap,
    },
    ReleaseFrame {
        frame: FrameCap,
    },
}

/// Fixed-capacity action batch produced before teardown side effects begin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionList {
    actions: [Option<AdapterAction>; MAX_TEARDOWN_ACTIONS],
    len: usize,
}

impl ActionList {
    pub const fn new() -> Self {
        Self {
            actions: [None; MAX_TEARDOWN_ACTIONS],
            len: 0,
        }
    }

    /// An empty list on the heap.
    ///
    /// The array is 144 KiB. Built in a stack frame it overflowed the root's
    /// 1 MiB stack during the stream plane's loan teardown — the frame is
    /// entered from an already-deep dispatch path, and every by-value return
    /// stacked a second copy in the caller. Exactly the failure `main.rs`
    /// records for the graph tables and B3 records for this table's
    /// predecessor.
    ///
    /// The bound itself is unchanged: it is still the fixed worst case of
    /// every mapping unmapped, every frame revoked, and every anchor released.
    /// What changed is where those bytes live.
    pub fn boxed() -> Box<Self> {
        // `vec![None; N]` fills the allocation in place; `Box::new(Self::new())`
        // would build the whole array in a stack frame first and then copy it,
        // which is the thing being avoided.
        // Written through the heap allocation rather than built in a frame.
        // `Box::new(Self::new())` would construct the whole 144 KiB array on
        // the stack and then copy it, which is the thing being avoided, and
        // zeroing is not an option: `None` is not the all-zero pattern for
        // `Option<AdapterAction>` here, as `an_empty_list_is_all_zero_bytes`
        // records.
        let layout = core::alloc::Layout::new::<Self>();
        // SAFETY: the layout is non-zero-sized, and every field is written
        // before the value is used as a `Self` — `len` directly, and each
        // action slot by the loop below, so no uninitialized byte is ever
        // read.
        unsafe {
            let raw = alloc::alloc::alloc(layout).cast::<Self>();
            if raw.is_null() {
                alloc::alloc::handle_alloc_error(layout);
            }
            let actions = core::ptr::addr_of_mut!((*raw).actions).cast::<Option<AdapterAction>>();
            for index in 0..MAX_TEARDOWN_ACTIONS {
                actions.add(index).write(None);
            }
            core::ptr::addr_of_mut!((*raw).len).write(0);
            Box::from_raw(raw)
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, index: usize) -> Option<AdapterAction> {
        if index < self.len {
            self.actions[index]
        } else {
            None
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = AdapterAction> + '_ {
        self.actions[..self.len].iter().copied().flatten()
    }

    fn push(&mut self, action: AdapterAction) -> Result<(), SharedBufferError> {
        if self.len == self.actions.len() {
            return Err(SharedBufferError::ActionListExhausted);
        }
        self.actions[self.len] = Some(action);
        self.len += 1;
        Ok(())
    }
}

impl Default for ActionList {
    fn default() -> Self {
        Self::new()
    }
}

/// Adapter error classification retained by the state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdapterError {
    MapConflict,
    OutOfMemory,
    InvalidCapability,
    FailedLookup,
    RevokeFailed,
    UnmapFailed,
    ReleaseFailed,
    Other,
}

/// Narrow adapter surface implemented by the live seL4 object/VSpace owner.
/// The state machine never assumes a failed invocation was harmless: map
/// rollback is explicit, while teardown failures leave state uncommitted so the
/// exact action list can be retried.
pub trait SharedBufferAdapter {
    fn map_frame(
        &mut self,
        frame: FrameCap,
        vspace: VSpaceCap,
        vaddr: usize,
        rights: MappingRights,
    ) -> Result<(), AdapterError>;

    /// Execute one idempotent teardown action. Implementations must treat an
    /// already-unmapped frame, a frame with no remaining derivations, or an
    /// already-released anchor as success so a failed action batch can be
    /// retried from the beginning without losing reclamation progress.
    fn perform(&mut self, action: AdapterAction) -> Result<(), AdapterError>;
}

/// Typed failures for every observable logical and adapter failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedBufferError {
    BadSize,
    BadFrameAnchors,
    ObjectsExhausted,
    BytesExhausted,
    MappingsExhausted,
    LoansExhausted,
    ChargesExhausted,
    ActionListExhausted,
    QuotaExceeded,
    NotFound,
    WrongOwner,
    WrongReceiver,
    BadRange,
    RightsDenied,
    WriteDenied,
    NotSealed,
    EpochMismatch,
    IdentityExhausted,
    /// A frame cap already anchors another live region. Admitting it would
    /// alias one physical frame into two independently accounted regions.
    DuplicateFrameAnchor,
    /// The orphan table cannot record another page the adapter failed to
    /// unmap. Rollback still attempts every earlier page before reporting this
    /// aggregate failure.
    OrphansExhausted,
    Adapter(AdapterError),
    Rollback {
        cause: AdapterError,
        rollback: AdapterError,
    },
    /// Rollback unmaps failed. `orphans` pages were retained for exact retry;
    /// `unrecorded` is normally zero because admission reserves sufficient
    /// orphan space, but records defensive exhaustion without aborting cleanup.
    Orphaned {
        cause: AdapterError,
        rollback: AdapterError,
        orphans: usize,
        unrecorded: usize,
    },
}

impl From<AdapterError> for SharedBufferError {
    fn from(error: AdapterError) -> Self {
        Self::Adapter(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Region {
    id: BufferId,
    epoch: GenerationEpoch,
    owner: HolderId,
    anchors: FrameAnchors,
    /// The authority this region actually confers on its owner. A caller's
    /// [`BufferHandle`] is intersected with this; it can never widen it.
    rights: BufferRights,
    created_writable: bool,
    sealed: bool,
    released: bool,
}

/// One page whose unmap the adapter failed to complete. The page is still live
/// in `vspace` at `vaddr`, so its frame must not be revoked or released until
/// the unmap succeeds on retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Orphan {
    frame: FrameCap,
    vspace: VSpaceCap,
    vaddr: usize,
}

/// A generation-declared ceiling bound to one holder identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QuotaEntry {
    holder: HolderId,
    quota: HolderQuota,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Loan {
    id: LoanId,
    epoch: GenerationEpoch,
    buffer: BufferId,
    lender: HolderId,
    receiver: HolderId,
    offset_pages: u16,
    page_count: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Charge {
    holder: HolderId,
    pages: u32,
    buffers: u32,
    mappings: u32,
    loans: u32,
}

impl Charge {
    const fn is_empty(self) -> bool {
        self.pages == 0 && self.buffers == 0 && self.mappings == 0 && self.loans == 0
    }
}

/// Prepared allocation admission. It contains root-owned frame-cap anchors but
/// changes no table state until [`SharedBufferTable::commit_create`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreatePlan {
    epoch: GenerationEpoch,
    owner: HolderId,
    anchors: FrameAnchors,
    writable: bool,
    slot: usize,
    id: BufferId,
    next_id: u64,
}

impl CreatePlan {
    pub const fn buffer_id(&self) -> BufferId {
        self.id
    }

    pub const fn anchors(&self) -> FrameAnchors {
        self.anchors
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MappingPlan {
    slot: usize,
    record: MappingRecord,
    anchors: FrameAnchors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TeardownPlan {
    remove_mappings: [bool; MAX_MAPPINGS],
    remove_loans: [bool; MAX_LOANS],
    remove_regions: [bool; MAX_SHARED_BUFFERS],
}

impl TeardownPlan {
    const fn new() -> Self {
        Self {
            remove_mappings: [false; MAX_MAPPINGS],
            remove_loans: [false; MAX_LOANS],
            remove_regions: [false; MAX_SHARED_BUFFERS],
        }
    }
}

/// Pure, fixed-capacity shared-buffer state owned by `slime-root`.
pub struct SharedBufferTable {
    epoch: GenerationEpoch,
    regions: [Option<Region>; MAX_SHARED_BUFFERS],
    mappings: [Option<MappingRecord>; MAX_MAPPINGS],
    loans: [Option<Loan>; MAX_LOANS],
    charges: [Option<Charge>; MAX_CHARGE_HOLDERS],
    quotas: [Option<QuotaEntry>; MAX_CHARGE_HOLDERS],
    orphans: [Option<Orphan>; MAX_ORPHANS],
    total_pages: usize,
    next_buffer_id: u64,
    next_loan_id: u64,
}

impl SharedBufferTable {
    pub const fn new(epoch: GenerationEpoch) -> Self {
        Self {
            epoch,
            regions: [None; MAX_SHARED_BUFFERS],
            mappings: [None; MAX_MAPPINGS],
            loans: [None; MAX_LOANS],
            charges: [None; MAX_CHARGE_HOLDERS],
            quotas: [None; MAX_CHARGE_HOLDERS],
            orphans: [None; MAX_ORPHANS],
            total_pages: 0,
            next_buffer_id: 1,
            next_loan_id: 1,
        }
    }

    pub const fn epoch(&self) -> GenerationEpoch {
        self.epoch
    }

    pub const fn total_pages(&self) -> usize {
        self.total_pages
    }

    pub fn live_count(&self) -> usize {
        self.regions.iter().flatten().count()
    }

    pub fn mapping_count(&self) -> usize {
        self.mappings.iter().flatten().count()
    }

    pub fn loan_count(&self) -> usize {
        self.loans.iter().flatten().count()
    }

    pub fn holder_pages(&self, holder: HolderId) -> u32 {
        self.charge(holder).map_or(0, |charge| charge.pages)
    }

    pub fn holder_buffers(&self, holder: HolderId) -> u32 {
        self.charge(holder).map_or(0, |charge| charge.buffers)
    }

    pub fn holder_mappings(&self, holder: HolderId) -> u32 {
        self.charge(holder).map_or(0, |charge| charge.mappings)
    }

    pub fn holder_loans(&self, holder: HolderId) -> u32 {
        self.charge(holder).map_or(0, |charge| charge.loans)
    }

    pub fn mapping(&self, index: usize) -> Option<MappingRecord> {
        self.mappings.get(index).copied().flatten()
    }

    /// Pages the adapter failed to unmap and which are therefore still live.
    pub fn orphan_count(&self) -> usize {
        self.orphans.iter().flatten().count()
    }

    /// Bind a generation-declared ceiling to one holder. Every later authority
    /// decision for that holder reads this table entry, never a caller
    /// argument, so a caller cannot raise its own ceiling by passing a wider
    /// [`HolderQuota`]. Re-declaring replaces the previous ceiling.
    pub fn declare_quota(
        &mut self,
        holder: HolderId,
        quota: HolderQuota,
    ) -> Result<(), SharedBufferError> {
        let slot = self
            .quotas
            .iter()
            .position(|entry| entry.is_some_and(|entry| entry.holder == holder))
            .or_else(|| self.quotas.iter().position(Option::is_none))
            .ok_or(SharedBufferError::ChargesExhausted)?;
        self.quotas[slot] = Some(QuotaEntry { holder, quota });
        Ok(())
    }

    /// Drop the ceiling bound to `holder`, reporting whether one was held.
    ///
    /// This closes backlog **B24**. `quotas` had no free path at all, and its
    /// key is a `HolderId` derived from a task id that `TaskTable::next_id`
    /// never rewinds — so a graph that spawned and reaped repeatedly presented
    /// a fresh holder every time, `declare_quota`'s same-holder reuse never
    /// fired, and `MAX_CHARGE_HOLDERS` bounded the holders a boot could *ever*
    /// construct rather than those live at once.
    ///
    /// Unlike [`crate::channel::sweep`] and [`crate::supervision::sweep`] this
    /// is a direct release rather than a derived predicate, because a quota has
    /// exactly one holder and that holder is a task: when the task is gone the
    /// entry is unreachable, with no second place it could still be named from.
    /// Channels and termination records both needed a sweep precisely because
    /// they can be named by a capability that outlives, or travels
    /// independently of, the task they concern. A quota cannot.
    ///
    /// Called from `reclaim_dead_task` beside the charge settlement, so the
    /// ceiling outlives every charge made against it and is dropped only once
    /// nothing can be charged again.
    pub fn release_quota(&mut self, holder: HolderId) -> bool {
        let Some(slot) = self
            .quotas
            .iter()
            .position(|entry| entry.is_some_and(|entry| entry.holder == holder))
        else {
            return false;
        };
        self.quotas[slot] = None;
        true
    }

    /// Ceilings currently bound. Reported at teardown so a leak is visible.
    pub fn quota_count(&self) -> usize {
        self.quotas.iter().flatten().count()
    }

    /// The ceiling a generation declared for `holder`. An undeclared holder
    /// receives [`HolderQuota::DENY`]: authority is never ambient.
    pub fn quota(&self, holder: HolderId) -> HolderQuota {
        self.quotas
            .iter()
            .flatten()
            .find(|entry| entry.holder == holder)
            .map_or(HolderQuota::DENY, |entry| entry.quota)
    }

    /// Admit root-owned frame anchors as one new buffer. Frame allocation is
    /// deliberately outside this pure state transition. If admission rejects,
    /// the caller still owns every supplied anchor and must release it.
    pub fn create(
        &mut self,
        owner: HolderId,
        anchors: FrameAnchors,
        writable: bool,
    ) -> Result<BufferHandle, SharedBufferError> {
        let plan = self.preflight_create(owner, anchors, writable)?;
        self.commit_create(plan)
    }

    /// Preflight allocation admission without changing accounting or consuming
    /// root-owned frame anchors. This is the allocation adapter transaction's
    /// prepare phase.
    pub fn preflight_create(
        &self,
        owner: HolderId,
        anchors: FrameAnchors,
        writable: bool,
    ) -> Result<CreatePlan, SharedBufferError> {
        let pages = anchors.len();
        if pages == 0 || pages > MAX_BUFFER_PAGES {
            return Err(SharedBufferError::BadSize);
        }
        self.preflight_anchor_uniqueness(&anchors)?;
        self.preflight_buffer_charge(owner, pages)?;
        let slot = self
            .regions
            .iter()
            .position(Option::is_none)
            .ok_or(SharedBufferError::ObjectsExhausted)?;
        self.preflight_charge_slot(owner)?;
        let id = BufferId(self.next_buffer_id);
        let next_id = self
            .next_buffer_id
            .checked_add(1)
            .ok_or(SharedBufferError::IdentityExhausted)?;
        Ok(CreatePlan {
            epoch: self.epoch,
            owner,
            anchors,
            writable,
            slot,
            id,
            next_id,
        })
    }

    /// Reject a frame cap that already anchors a live region, in this or any
    /// other holder's region. Uniqueness within one `FrameAnchors` is checked
    /// at construction; this is the global check that stops one physical frame
    /// from being aliased into two independently accounted regions.
    fn preflight_anchor_uniqueness(&self, anchors: &FrameAnchors) -> Result<(), SharedBufferError> {
        for index in 0..anchors.len() {
            let cap = anchors
                .get(index)
                .ok_or(SharedBufferError::BadFrameAnchors)?;
            if self.anchor_is_live(cap) {
                return Err(SharedBufferError::DuplicateFrameAnchor);
            }
        }
        Ok(())
    }

    fn anchor_is_live(&self, cap: FrameCap) -> bool {
        self.regions.iter().flatten().any(|region| {
            (0..region.anchors.len()).any(|index| region.anchors.get(index) == Some(cap))
        })
    }

    /// Commit a previously preflighted allocation after the allocator adapter
    /// has successfully produced the exact root frame-cap anchors.
    pub fn commit_create(&mut self, plan: CreatePlan) -> Result<BufferHandle, SharedBufferError> {
        if plan.epoch != self.epoch {
            return Err(SharedBufferError::EpochMismatch);
        }
        if self.next_buffer_id != plan.id.0
            || self.regions.get(plan.slot).is_none_or(Option::is_some)
        {
            return Err(SharedBufferError::NotFound);
        }
        let pages = plan.anchors.len();
        self.preflight_anchor_uniqueness(&plan.anchors)?;
        self.preflight_buffer_charge(plan.owner, pages)?;
        self.preflight_charge_slot(plan.owner)?;
        // The region's own rights are the authority of record. A handle handed
        // back to a caller is a copy of these bits, and every later decision
        // re-reads the region rather than trusting the copy.
        let rights = if plan.writable {
            BufferRights::ALL
        } else {
            BufferRights::MAP
        };
        self.regions[plan.slot] = Some(Region {
            id: plan.id,
            epoch: plan.epoch,
            owner: plan.owner,
            anchors: plan.anchors,
            rights,
            created_writable: plan.writable,
            sealed: false,
            released: false,
        });
        self.next_buffer_id = plan.next_id;
        self.total_pages += pages;
        self.charge_positive(plan.owner, pages as u32, 1, 0, 0)?;
        Ok(BufferHandle {
            id: plan.id,
            epoch: plan.epoch,
            rights,
        })
    }

    /// Resolve the authority a handle actually carries.
    ///
    /// `holder` is an authenticated caller claim — the root derives it from the
    /// arriving badge — and it must match the owner the table recorded. The
    /// effective rights are the table's bits intersected with the caller's
    /// copy, so a narrowed handle stays narrowed and a forged one cannot widen.
    /// A mismatch is always a typed error; there is no fallback to the claim.
    fn authorize(
        &self,
        holder: HolderId,
        handle: BufferHandle,
        required: BufferRights,
    ) -> Result<(Region, BufferRights), SharedBufferError> {
        self.check_epoch(handle.epoch)?;
        let region = self.live_region(handle.id)?;
        if region.owner != holder {
            return Err(SharedBufferError::WrongOwner);
        }
        if region.epoch != handle.epoch {
            return Err(SharedBufferError::EpochMismatch);
        }
        let effective = region.rights.intersect(handle.rights);
        if !effective.contains(required) {
            return Err(SharedBufferError::RightsDenied);
        }
        Ok((region, effective))
    }

    /// Install an exact direct buffer mapping. All range, authority, epoch,
    /// quota, and table-capacity checks complete before the first adapter call.
    #[allow(clippy::too_many_arguments)]
    pub fn map<A: SharedBufferAdapter>(
        &mut self,
        adapter: &mut A,
        holder: HolderId,
        handle: BufferHandle,
        vspace: VSpaceCap,
        base: usize,
        offset: usize,
        length: usize,
        rights: MappingRights,
    ) -> Result<(), SharedBufferError> {
        let required = if rights.writable() {
            BufferRights(BufferRights::MAP.0 | BufferRights::WRITE.0)
        } else {
            BufferRights::MAP
        };
        let (region, _) = self.authorize(holder, handle, required)?;
        let (offset_pages, page_count) =
            Self::validate_mapping_range(region, base, offset, length)?;
        if rights.writable() && (!region.created_writable || region.sealed) {
            return Err(SharedBufferError::WriteDenied);
        }
        let plan = self.preflight_mapping(
            holder,
            region,
            vspace,
            base,
            offset_pages,
            page_count,
            rights,
            None,
        )?;
        self.execute_mapping(adapter, plan)
    }

    /// Install a read-only mapping relative to one receiver-bound loan.
    #[allow(clippy::too_many_arguments)]
    pub fn map_loan<A: SharedBufferAdapter>(
        &mut self,
        adapter: &mut A,
        receiver: HolderId,
        handle: LoanHandle,
        vspace: VSpaceCap,
        base: usize,
        relative_offset: usize,
        length: usize,
    ) -> Result<(), SharedBufferError> {
        // A loan handle names its receiver, but the recorded loan is what
        // decides: both the handle's claim and the caller's claim must match it.
        let loan = self.authorize_loan(receiver, handle)?;
        let relative_pages =
            Self::validate_page_range(relative_offset, length, loan.page_count as usize)?;
        let absolute_offset_pages = (loan.offset_pages as usize)
            .checked_add(relative_pages.0)
            .ok_or(SharedBufferError::BadRange)?;
        // Direct owner authority is gone after `release`, but an outstanding
        // loan deliberately retains the region and remains independently
        // usable by its receiver. The live loan above is the authority check;
        // requiring an unreleased owner entry here would make release revoke
        // the loan implicitly, unlike the x86 table and `release`'s contract.
        let region = self.live_region_any(loan.buffer)?;
        let absolute_offset = absolute_offset_pages
            .checked_mul(PAGE_SIZE)
            .ok_or(SharedBufferError::BadRange)?;
        let (_, page_count) = Self::validate_mapping_range(region, base, absolute_offset, length)?;
        let plan = self.preflight_mapping(
            receiver,
            region,
            vspace,
            base,
            absolute_offset_pages,
            page_count,
            MappingRights::ReadOnly,
            Some(loan.id),
        )?;
        self.execute_mapping(adapter, plan)
    }

    /// Resolve a loan handle against the recorded loan, as its receiver.
    fn authorize_loan(
        &self,
        receiver: HolderId,
        handle: LoanHandle,
    ) -> Result<Loan, SharedBufferError> {
        self.check_epoch(handle.epoch)?;
        if handle.receiver != receiver {
            return Err(SharedBufferError::WrongReceiver);
        }
        let loan = self.live_loan(handle.id)?;
        if loan.epoch != handle.epoch || loan.buffer != handle.buffer {
            return Err(SharedBufferError::NotFound);
        }
        if loan.receiver != receiver {
            return Err(SharedBufferError::WrongReceiver);
        }
        Ok(loan)
    }

    /// Remove one exact mapping. State commits only after the adapter completes
    /// the required unmaps, so a failed teardown remains retryable.
    pub fn unmap<A: SharedBufferAdapter>(
        &mut self,
        adapter: &mut A,
        holder: HolderId,
        handle: BufferHandle,
        vspace: VSpaceCap,
        base: usize,
    ) -> Result<(), SharedBufferError> {
        self.unmap_authorized(adapter, holder, handle, vspace, base, false)
    }

    /// Remove a mapping a *loan receiver* installed through [`Self::map_loan`].
    ///
    /// Split from [`Self::unmap`] because the two authorize differently, and
    /// only one of them can. `unmap` requires the caller to own the region;
    /// a loan receiver never does — the region belongs to the lender, and the
    /// receiver holds a loan naming it. Running the owner check here would
    /// refuse every borrower unmapping something it legitimately mapped.
    ///
    /// What authorizes instead is **the mapping record itself**. `map_loan`
    /// stamps each mapping with the receiver as `holder` and the loan as
    /// `loan`, so a record matching this caller, this vspace, and this base is
    /// proof the caller installed it. That is the same thing the retired
    /// kernel's `SharedBufferTable::unmap` relies on — it takes no rights
    /// argument at all and matches on `mapping.owner` — so this is the oracle's
    /// authorization restated rather than a weaker one.
    ///
    /// The caller must still have resolved a live loan capability from its own
    /// table to get here, which is what bounds *which* buffer it may name.
    pub fn unmap_loan<A: SharedBufferAdapter>(
        &mut self,
        adapter: &mut A,
        receiver: HolderId,
        handle: LoanHandle,
        vspace: VSpaceCap,
        base: usize,
    ) -> Result<(), SharedBufferError> {
        // The loan must still be live and still name this receiver. Without
        // this a revoked loan's stale slot would keep unmapping rights.
        let loan = self.authorize_loan(receiver, handle)?;
        self.unmap_authorized(
            adapter,
            receiver,
            BufferHandle {
                id: loan.buffer,
                epoch: loan.epoch,
                rights: BufferRights::MAP,
            },
            vspace,
            base,
            true,
        )
    }

    fn unmap_authorized<A: SharedBufferAdapter>(
        &mut self,
        adapter: &mut A,
        holder: HolderId,
        handle: BufferHandle,
        vspace: VSpaceCap,
        base: usize,
        through_loan: bool,
    ) -> Result<(), SharedBufferError> {
        if !through_loan {
            self.authorize(holder, handle, BufferRights::MAP)?;
        }
        let slot = self
            .mappings
            .iter()
            .position(|mapping| {
                mapping.is_some_and(|mapping| {
                    mapping.holder == holder
                        && mapping.buffer == handle.id
                        && mapping.epoch == handle.epoch
                        && mapping.vspace == vspace
                        && mapping.base == base
                        // A loan unmap removes only a mapping *made through a
                        // loan*. Otherwise a receiver that also happened to own
                        // a direct mapping of the same region at the same base
                        // could tear that one down instead.
                        && mapping.loan.is_some() == through_loan
                })
            })
            .ok_or(SharedBufferError::NotFound)?;
        let mut plan = TeardownPlan::new();
        let mut actions = ActionList::boxed();
        plan.remove_mappings[slot] = true;
        self.append_mapping_actions(&mut actions, slot)?;
        self.execute_teardown(adapter, actions, plan).map(|_| ())
    }

    /// Irreversibly seal a writable region. Every extant writable mapping is
    /// revoked and reinstalled read-only before the logical seal commits. A
    /// failed remap is rolled back to its prior writable state; if rollback
    /// itself fails the caller receives an explicit [`SharedBufferError::Rollback`]
    /// and the logical state remains unsealed.
    pub fn seal<A: SharedBufferAdapter>(
        &mut self,
        adapter: &mut A,
        owner: HolderId,
        handle: BufferHandle,
    ) -> Result<(), SharedBufferError> {
        let (region, _) = self.authorize(owner, handle, BufferRights::WRITE)?;
        let region_slot = self.live_region_slot(handle.id)?;
        if region.sealed {
            return Ok(());
        }

        let mut writable_slots = [usize::MAX; MAX_MAPPINGS];
        let mut writable_count = 0usize;
        for (slot, mapping) in self.mappings.iter().copied().enumerate() {
            if mapping.is_some_and(|mapping| {
                mapping.buffer == handle.id && mapping.rights == MappingRights::ReadWrite
            }) {
                writable_slots[writable_count] = slot;
                writable_count += 1;
            }
        }

        for (converted, slot) in writable_slots[..writable_count].iter().copied().enumerate() {
            let mapping = self.mappings[slot].ok_or(SharedBufferError::NotFound)?;
            if let Err(cause) = self.remap_mapping(
                adapter,
                region,
                mapping,
                MappingRights::ReadOnly,
                mapping.rights,
            ) {
                let rollback = self.rollback_seal(adapter, region, &writable_slots[..converted]);
                return match rollback {
                    Ok(()) => Err(cause),
                    Err(rollback) => Err(SharedBufferError::Rollback {
                        cause: Self::adapter_cause(cause),
                        rollback,
                    }),
                };
            }
        }

        for slot in writable_slots[..writable_count].iter().copied() {
            self.mappings[slot]
                .as_mut()
                .ok_or(SharedBufferError::NotFound)?
                .rights = MappingRights::ReadOnly;
        }
        self.regions[region_slot]
            .as_mut()
            .ok_or(SharedBufferError::NotFound)?
            .sealed = true;
        Ok(())
    }

    /// Create one single-return loan over an exact sealed range.
    pub fn loan(
        &mut self,
        lender: HolderId,
        receiver: HolderId,
        handle: BufferHandle,
        offset: usize,
        length: usize,
    ) -> Result<LoanHandle, SharedBufferError> {
        let (region, _) = self.authorize(lender, handle, BufferRights::LOAN)?;
        if !region.sealed {
            return Err(SharedBufferError::NotSealed);
        }
        let (offset_pages, page_count) =
            Self::validate_page_range(offset, length, region.anchors.len())?;
        let quota = self.quota(lender);
        if self
            .holder_loans(lender)
            .checked_add(1)
            .is_none_or(|value| value > quota.loan_count)
        {
            return Err(SharedBufferError::QuotaExceeded);
        }
        let slot = self
            .loans
            .iter()
            .position(Option::is_none)
            .ok_or(SharedBufferError::LoansExhausted)?;
        self.preflight_charge_slot(lender)?;
        let id = LoanId(self.next_loan_id);
        let next_id = self
            .next_loan_id
            .checked_add(1)
            .ok_or(SharedBufferError::IdentityExhausted)?;

        self.loans[slot] = Some(Loan {
            id,
            epoch: self.epoch,
            buffer: handle.id,
            lender,
            receiver,
            offset_pages: u16::try_from(offset_pages).map_err(|_| SharedBufferError::BadRange)?,
            page_count: u16::try_from(page_count).map_err(|_| SharedBufferError::BadRange)?,
        });
        self.next_loan_id = next_id;
        self.charge_positive(lender, 0, 0, 0, 1)?;
        Ok(LoanHandle {
            id,
            buffer: handle.id,
            epoch: self.epoch,
            receiver,
        })
    }

    /// Return one exact loan. A duplicate return finds no live identity and has
    /// no side effects. Loan mappings are unmapped before the record is consumed.
    pub fn return_loan<A: SharedBufferAdapter>(
        &mut self,
        adapter: &mut A,
        receiver: HolderId,
        handle: LoanHandle,
    ) -> Result<(), SharedBufferError> {
        let loan = self.authorize_loan(receiver, handle)?;
        let plan = self.plan_settle_loans(|candidate| candidate.id == loan.id)?;
        let actions = self.build_actions(&plan)?;
        self.execute_teardown(adapter, actions, plan).map(|_| ())
    }

    /// Explicitly revoke one loan as its lender. The recorded loan decides who
    /// the lender is; the caller's claim must match it.
    pub fn revoke_loan<A: SharedBufferAdapter>(
        &mut self,
        adapter: &mut A,
        lender: HolderId,
        handle: LoanHandle,
    ) -> Result<(), SharedBufferError> {
        self.check_epoch(handle.epoch)?;
        let loan = self.live_loan(handle.id)?;
        if loan.buffer != handle.buffer {
            return Err(SharedBufferError::NotFound);
        }
        if loan.lender != lender {
            return Err(SharedBufferError::WrongOwner);
        }
        let plan = self.plan_settle_loans(|candidate| candidate.id == loan.id)?;
        let actions = self.build_actions(&plan)?;
        self.execute_teardown(adapter, actions, plan).map(|_| ())
    }

    /// Drop direct ownership. Outstanding loans retain the root frame anchors
    /// and original page/buffer charge until the last loan settles.
    pub fn release<A: SharedBufferAdapter>(
        &mut self,
        adapter: &mut A,
        owner: HolderId,
        handle: BufferHandle,
    ) -> Result<(), SharedBufferError> {
        let (region, _) = self.authorize(owner, handle, BufferRights::MAP)?;
        let slot = self.live_region_slot(handle.id)?;
        let has_loans = self.has_region_loans(region.id);
        let mut plan = TeardownPlan::new();
        let mut actions = ActionList::boxed();
        for mapping_slot in 0..self.mappings.len() {
            if self.mappings[mapping_slot].is_some_and(|mapping| {
                mapping.buffer == region.id && (mapping.loan.is_none() || !has_loans)
            }) {
                plan.remove_mappings[mapping_slot] = true;
                self.append_mapping_actions(&mut actions, mapping_slot)?;
            }
        }
        if !has_loans {
            plan.remove_regions[slot] = true;
        }
        self.append_region_reclamation(&plan, &mut actions)?;
        self.run_actions(adapter, &actions)?;
        self.commit_teardown(plan)?;
        if has_loans {
            self.regions[slot]
                .as_mut()
                .ok_or(SharedBufferError::NotFound)?
                .released = true;
        }
        Ok(())
    }

    /// Settle all loans involving a dead peer, tear down all of its mappings,
    /// and reclaim all buffers it owned. The returned action list is the exact
    /// deterministic sequence successfully executed by the adapter.
    pub fn reclaim_holder<A: SharedBufferAdapter>(
        &mut self,
        adapter: &mut A,
        holder: HolderId,
    ) -> Result<Box<ActionList>, SharedBufferError> {
        let mut plan =
            self.plan_settle_loans(|loan| loan.lender == holder || loan.receiver == holder)?;
        for slot in 0..self.mappings.len() {
            if self.mappings[slot].is_some_and(|mapping| mapping.holder == holder) {
                plan.remove_mappings[slot] = true;
            }
        }
        for slot in 0..self.regions.len() {
            if self.regions[slot].is_some_and(|region| region.owner == holder) {
                plan.remove_regions[slot] = true;
            }
        }
        self.complete_region_removals(&mut plan)?;
        let actions = self.build_actions(&plan)?;
        self.execute_teardown(adapter, actions, plan)
    }

    /// Retire the whole epoch in deterministic order. On success the table is
    /// empty and accepts no stale handle because the epoch advances.
    pub fn advance_epoch<A: SharedBufferAdapter>(
        &mut self,
        adapter: &mut A,
        next: GenerationEpoch,
    ) -> Result<Box<ActionList>, SharedBufferError> {
        if next.0 <= self.epoch.0 {
            return Err(SharedBufferError::EpochMismatch);
        }
        let mut plan = TeardownPlan::new();
        let mut actions = ActionList::boxed();
        for slot in 0..self.mappings.len() {
            if self.mappings[slot].is_some() {
                plan.remove_mappings[slot] = true;
                self.append_mapping_actions(&mut actions, slot)?;
            }
        }
        for slot in 0..self.loans.len() {
            plan.remove_loans[slot] = self.loans[slot].is_some();
        }
        for slot in 0..self.regions.len() {
            plan.remove_regions[slot] = self.regions[slot].is_some();
        }
        self.append_region_reclamation(&plan, &mut actions)?;
        let actions = self.execute_teardown(adapter, actions, plan)?;
        self.epoch = next;
        Ok(actions)
    }

    fn preflight_buffer_charge(
        &self,
        owner: HolderId,
        pages: usize,
    ) -> Result<(), SharedBufferError> {
        let quota = self.quota(owner);
        let pages_u32 = u32::try_from(pages).map_err(|_| SharedBufferError::BadSize)?;
        if self
            .holder_pages(owner)
            .checked_add(pages_u32)
            .is_none_or(|value| value > quota.byte_pages)
            || self
                .holder_buffers(owner)
                .checked_add(1)
                .is_none_or(|value| value > quota.buffer_count)
        {
            return Err(SharedBufferError::QuotaExceeded);
        }
        if self
            .total_pages
            .checked_add(pages)
            .is_none_or(|value| value > MAX_TOTAL_PAGES)
        {
            return Err(SharedBufferError::BytesExhausted);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn preflight_mapping(
        &self,
        holder: HolderId,
        region: Region,
        vspace: VSpaceCap,
        base: usize,
        offset_pages: usize,
        page_count: usize,
        rights: MappingRights,
        loan: Option<LoanId>,
    ) -> Result<MappingPlan, SharedBufferError> {
        let quota = self.quota(holder);
        if self
            .holder_mappings(holder)
            .checked_add(1)
            .is_none_or(|value| value > quota.mapping_count)
        {
            return Err(SharedBufferError::QuotaExceeded);
        }
        let slot = self
            .mappings
            .iter()
            .position(Option::is_none)
            .ok_or(SharedBufferError::MappingsExhausted)?;
        let admitted_pages = self
            .mappings
            .iter()
            .flatten()
            .try_fold(self.orphan_count(), |total, mapping| {
                total.checked_add(mapping.page_count as usize)
            })
            .ok_or(SharedBufferError::MappingsExhausted)?;
        if admitted_pages
            .checked_add(page_count)
            .is_none_or(|total| total > MAX_MAPPING_PAGES)
        {
            return Err(SharedBufferError::MappingsExhausted);
        }
        self.preflight_charge_slot(holder)?;
        Ok(MappingPlan {
            slot,
            record: MappingRecord {
                holder,
                buffer: region.id,
                epoch: region.epoch,
                vspace,
                base,
                offset_pages: u16::try_from(offset_pages)
                    .map_err(|_| SharedBufferError::BadRange)?,
                page_count: u16::try_from(page_count).map_err(|_| SharedBufferError::BadRange)?,
                rights,
                loan,
            },
            anchors: region.anchors,
        })
    }

    fn execute_mapping<A: SharedBufferAdapter>(
        &mut self,
        adapter: &mut A,
        plan: MappingPlan,
    ) -> Result<(), SharedBufferError> {
        for page in 0..plan.record.page_count as usize {
            let frame_index = plan.record.offset_pages as usize + page;
            let frame = plan
                .anchors
                .get(frame_index)
                .ok_or(SharedBufferError::BadRange)?;
            let vaddr = plan
                .record
                .base
                .checked_add(page * PAGE_SIZE)
                .ok_or(SharedBufferError::BadRange)?;
            if let Err(cause) =
                adapter.map_frame(frame, plan.record.vspace, vaddr, plan.record.rights)
            {
                // Undo the pages already mapped. A rollback unmap that itself
                // fails leaves a page live in the child's VSpace: record it as
                // an orphan so its frame is never revoked or released while
                // still mapped, and so reclamation can retry the exact page.
                // Dropping it here would strand real memory with nothing left
                // in the table that names it.
                let mut rollback_error = None;
                let mut orphaned = 0usize;
                let mut unrecorded = 0usize;
                for rollback_page in (0..page).rev() {
                    let rollback_frame = plan
                        .anchors
                        .get(plan.record.offset_pages as usize + rollback_page)
                        .ok_or(SharedBufferError::BadRange)?;
                    let rollback_vaddr = plan.record.base + rollback_page * PAGE_SIZE;
                    if let Err(rollback) = adapter.perform(AdapterAction::Unmap {
                        frame: rollback_frame,
                        vspace: plan.record.vspace,
                        vaddr: rollback_vaddr,
                    }) {
                        rollback_error.get_or_insert(rollback);
                        match self.record_orphan(Orphan {
                            frame: rollback_frame,
                            vspace: plan.record.vspace,
                            vaddr: rollback_vaddr,
                        }) {
                            Ok(()) => orphaned += 1,
                            Err(SharedBufferError::OrphansExhausted) => unrecorded += 1,
                            Err(error) => return Err(error),
                        }
                    }
                }
                if let Some(rollback) = rollback_error {
                    return Err(SharedBufferError::Orphaned {
                        cause,
                        rollback,
                        orphans: orphaned,
                        unrecorded,
                    });
                }
                return Err(Self::map_adapter_error(cause));
            }
        }
        self.mappings[plan.slot] = Some(plan.record);
        self.charge_positive(plan.record.holder, 0, 0, 1, 0)?;
        Ok(())
    }

    fn remap_mapping<A: SharedBufferAdapter>(
        &self,
        adapter: &mut A,
        region: Region,
        mapping: MappingRecord,
        target_rights: MappingRights,
        rollback_rights: MappingRights,
    ) -> Result<(), SharedBufferError> {
        for page in 0..mapping.page_count as usize {
            let frame = region
                .anchors
                .get(mapping.offset_pages as usize + page)
                .ok_or(SharedBufferError::BadRange)?;
            let vaddr = mapping.base + page * PAGE_SIZE;
            if let Err(cause) = adapter.perform(AdapterAction::Unmap {
                frame,
                vspace: mapping.vspace,
                vaddr,
            }) {
                for restore in 0..page {
                    let restore_frame = region
                        .anchors
                        .get(mapping.offset_pages as usize + restore)
                        .ok_or(SharedBufferError::BadRange)?;
                    let restore_vaddr = mapping.base + restore * PAGE_SIZE;
                    if let Err(rollback) = adapter.map_frame(
                        restore_frame,
                        mapping.vspace,
                        restore_vaddr,
                        rollback_rights,
                    ) {
                        return Err(SharedBufferError::Rollback { cause, rollback });
                    }
                }
                return Err(SharedBufferError::Adapter(cause));
            }
        }

        for page in 0..mapping.page_count as usize {
            let frame = region
                .anchors
                .get(mapping.offset_pages as usize + page)
                .ok_or(SharedBufferError::BadRange)?;
            let vaddr = mapping.base + page * PAGE_SIZE;
            if let Err(cause) = adapter.map_frame(frame, mapping.vspace, vaddr, target_rights) {
                for remove in 0..page {
                    let remove_frame = region
                        .anchors
                        .get(mapping.offset_pages as usize + remove)
                        .ok_or(SharedBufferError::BadRange)?;
                    let remove_vaddr = mapping.base + remove * PAGE_SIZE;
                    if let Err(rollback) = adapter.perform(AdapterAction::Unmap {
                        frame: remove_frame,
                        vspace: mapping.vspace,
                        vaddr: remove_vaddr,
                    }) {
                        return Err(SharedBufferError::Rollback { cause, rollback });
                    }
                }
                for restore in 0..mapping.page_count as usize {
                    let restore_frame = region
                        .anchors
                        .get(mapping.offset_pages as usize + restore)
                        .ok_or(SharedBufferError::BadRange)?;
                    let restore_vaddr = mapping.base + restore * PAGE_SIZE;
                    if let Err(rollback) = adapter.map_frame(
                        restore_frame,
                        mapping.vspace,
                        restore_vaddr,
                        rollback_rights,
                    ) {
                        return Err(SharedBufferError::Rollback { cause, rollback });
                    }
                }
                return Err(Self::map_adapter_error(cause));
            }
        }
        Ok(())
    }

    fn rollback_seal<A: SharedBufferAdapter>(
        &self,
        adapter: &mut A,
        region: Region,
        slots: &[usize],
    ) -> Result<(), AdapterError> {
        for slot in slots.iter().copied().rev() {
            let mapping = self.mappings[slot].ok_or(AdapterError::Other)?;
            self.remap_mapping(
                adapter,
                region,
                mapping,
                MappingRights::ReadWrite,
                MappingRights::ReadOnly,
            )
            .map_err(Self::adapter_cause)?;
        }
        Ok(())
    }

    fn plan_settle_loans(
        &self,
        mut predicate: impl FnMut(Loan) -> bool,
    ) -> Result<TeardownPlan, SharedBufferError> {
        let mut plan = TeardownPlan::new();
        for slot in 0..self.loans.len() {
            if self.loans[slot].is_some_and(&mut predicate) {
                plan.remove_loans[slot] = true;
            }
        }
        for slot in 0..self.mappings.len() {
            if self.mappings[slot].is_some_and(|mapping| {
                mapping.loan.is_some_and(|loan_id| {
                    self.loans.iter().enumerate().any(|(loan_slot, loan)| {
                        plan.remove_loans[loan_slot] && loan.is_some_and(|loan| loan.id == loan_id)
                    })
                })
            }) {
                plan.remove_mappings[slot] = true;
            }
        }
        self.complete_region_removals(&mut plan)?;
        Ok(plan)
    }

    /// Close a plan over regions whose last loan is settling, and over every
    /// mapping of a region being removed.
    fn complete_region_removals(&self, plan: &mut TeardownPlan) -> Result<(), SharedBufferError> {
        for slot in 0..self.regions.len() {
            let Some(region) = self.regions[slot] else {
                continue;
            };
            let loans_remain = self.loans.iter().enumerate().any(|(loan_slot, loan)| {
                !plan.remove_loans[loan_slot] && loan.is_some_and(|loan| loan.buffer == region.id)
            });
            if (region.released && !loans_remain) || plan.remove_regions[slot] {
                plan.remove_regions[slot] = true;
                for mapping_slot in 0..self.mappings.len() {
                    if self.mappings[mapping_slot]
                        .is_some_and(|mapping| mapping.buffer == region.id)
                    {
                        plan.remove_mappings[mapping_slot] = true;
                    }
                }
            }
        }
        Ok(())
    }

    /// Render one finished plan as the exact adapter sequence, in the order the
    /// module contract states: every mapping unmapped first, then every frame
    /// revoked across all regions, then every anchor released. Building the
    /// list in one pass from the closed plan is what makes that ordering
    /// structural rather than dependent on the order callers marked slots.
    fn build_actions(&self, plan: &TeardownPlan) -> Result<Box<ActionList>, SharedBufferError> {
        let mut actions = ActionList::boxed();
        for slot in 0..self.mappings.len() {
            if plan.remove_mappings[slot] {
                self.append_mapping_actions(&mut actions, slot)?;
            }
        }
        self.append_region_reclamation(plan, &mut actions)?;
        Ok(actions)
    }

    fn append_mapping_actions(
        &self,
        actions: &mut ActionList,
        mapping_slot: usize,
    ) -> Result<(), SharedBufferError> {
        let mapping = self.mappings[mapping_slot].ok_or(SharedBufferError::NotFound)?;
        let region = self.live_region_any(mapping.buffer)?;
        for page in 0..mapping.page_count as usize {
            actions.push(AdapterAction::Unmap {
                frame: region
                    .anchors
                    .get(mapping.offset_pages as usize + page)
                    .ok_or(SharedBufferError::BadRange)?,
                vspace: mapping.vspace,
                vaddr: mapping.base + page * PAGE_SIZE,
            })?;
        }
        Ok(())
    }

    /// Append every frame reclamation for the whole plan, globally ordered:
    /// all revokes across all regions first, then all releases. Interleaving
    /// per region would release one region's anchor while another region's
    /// derived mapping is still revocable, which is exactly the ordering the
    /// module contract forbids.
    fn append_region_reclamation(
        &self,
        plan: &TeardownPlan,
        actions: &mut ActionList,
    ) -> Result<(), SharedBufferError> {
        for slot in 0..self.regions.len() {
            if !plan.remove_regions[slot] {
                continue;
            }
            let region = self.regions[slot].ok_or(SharedBufferError::NotFound)?;
            for page in 0..region.anchors.len() {
                let frame = region
                    .anchors
                    .get(page)
                    .ok_or(SharedBufferError::BadFrameAnchors)?;
                actions.push(AdapterAction::Revoke { frame })?;
            }
        }
        for slot in 0..self.regions.len() {
            if !plan.remove_regions[slot] {
                continue;
            }
            let region = self.regions[slot].ok_or(SharedBufferError::NotFound)?;
            for page in 0..region.anchors.len() {
                let frame = region
                    .anchors
                    .get(page)
                    .ok_or(SharedBufferError::BadFrameAnchors)?;
                actions.push(AdapterAction::ReleaseFrame { frame })?;
            }
        }
        Ok(())
    }

    /// Retain one page the adapter failed to unmap. A duplicate is not an
    /// error: retrying a batch that orphaned the same page twice must converge.
    fn record_orphan(&mut self, orphan: Orphan) -> Result<(), SharedBufferError> {
        if self.orphans.iter().flatten().any(|held| *held == orphan) {
            return Ok(());
        }
        let slot = self
            .orphans
            .iter()
            .position(Option::is_none)
            .ok_or(SharedBufferError::OrphansExhausted)?;
        self.orphans[slot] = Some(orphan);
        Ok(())
    }

    /// Retry every retained orphan. A page that unmaps successfully is dropped
    /// from the table; one that fails again stays recorded. Returns how many
    /// remain live, so a caller can report reclamation as incomplete rather
    /// than assume it finished.
    pub fn retry_orphans<A: SharedBufferAdapter>(&mut self, adapter: &mut A) -> usize {
        for slot in 0..self.orphans.len() {
            let Some(orphan) = self.orphans[slot] else {
                continue;
            };
            let unmapped = adapter
                .perform(AdapterAction::Unmap {
                    frame: orphan.frame,
                    vspace: orphan.vspace,
                    vaddr: orphan.vaddr,
                })
                .is_ok();
            if unmapped {
                self.orphans[slot] = None;
            }
        }
        self.orphan_count()
    }

    /// Run the batch, commit the plan, and hand the batch back.
    ///
    /// Takes the box by value rather than by reference so the caller's
    /// `Ok(actions)` moves a pointer. Returning the list itself would copy
    /// 144 KiB through the caller's frame, which is what overflowed the root's
    /// stack.
    fn execute_teardown<A: SharedBufferAdapter>(
        &mut self,
        adapter: &mut A,
        actions: Box<ActionList>,
        plan: TeardownPlan,
    ) -> Result<Box<ActionList>, SharedBufferError> {
        self.run_actions(adapter, &actions)?;
        self.commit_teardown(plan)?;
        Ok(actions)
    }

    fn run_actions<A: SharedBufferAdapter>(
        &self,
        adapter: &mut A,
        actions: &ActionList,
    ) -> Result<(), SharedBufferError> {
        for action in actions.iter() {
            adapter.perform(action)?;
        }
        Ok(())
    }

    fn commit_teardown(&mut self, plan: TeardownPlan) -> Result<(), SharedBufferError> {
        for slot in 0..self.mappings.len() {
            if plan.remove_mappings[slot] {
                let mapping = self.mappings[slot]
                    .take()
                    .ok_or(SharedBufferError::NotFound)?;
                self.uncharge(mapping.holder, 0, 0, 1, 0)?;
            }
        }
        for slot in 0..self.loans.len() {
            if plan.remove_loans[slot] {
                let loan = self.loans[slot].take().ok_or(SharedBufferError::NotFound)?;
                self.uncharge(loan.lender, 0, 0, 0, 1)?;
            }
        }
        for slot in 0..self.regions.len() {
            if plan.remove_regions[slot] {
                let region = self.regions[slot]
                    .take()
                    .ok_or(SharedBufferError::NotFound)?;
                self.total_pages = self
                    .total_pages
                    .checked_sub(region.anchors.len())
                    .ok_or(SharedBufferError::BadSize)?;
                self.uncharge(region.owner, region.anchors.len() as u32, 1, 0, 0)?;
            }
        }
        Ok(())
    }

    fn validate_mapping_range(
        region: Region,
        base: usize,
        offset: usize,
        length: usize,
    ) -> Result<(usize, usize), SharedBufferError> {
        if !base.is_multiple_of(PAGE_SIZE) {
            return Err(SharedBufferError::BadRange);
        }
        let end = base
            .checked_add(length)
            .ok_or(SharedBufferError::BadRange)?;
        if base >= AARCH64_USER_TOP || end > AARCH64_USER_TOP {
            return Err(SharedBufferError::BadRange);
        }
        Self::validate_page_range(offset, length, region.anchors.len())
    }

    fn validate_page_range(
        offset: usize,
        length: usize,
        total_pages: usize,
    ) -> Result<(usize, usize), SharedBufferError> {
        if length == 0 || !offset.is_multiple_of(PAGE_SIZE) || !length.is_multiple_of(PAGE_SIZE) {
            return Err(SharedBufferError::BadRange);
        }
        let total_bytes = total_pages
            .checked_mul(PAGE_SIZE)
            .ok_or(SharedBufferError::BadRange)?;
        let end = offset
            .checked_add(length)
            .ok_or(SharedBufferError::BadRange)?;
        if end > total_bytes {
            return Err(SharedBufferError::BadRange);
        }
        Ok((offset / PAGE_SIZE, length / PAGE_SIZE))
    }

    fn check_epoch(&self, epoch: GenerationEpoch) -> Result<(), SharedBufferError> {
        if epoch == self.epoch {
            Ok(())
        } else {
            Err(SharedBufferError::EpochMismatch)
        }
    }

    fn live_region_slot(&self, id: BufferId) -> Result<usize, SharedBufferError> {
        self.regions
            .iter()
            .position(|region| region.is_some_and(|region| region.id == id && !region.released))
            .ok_or(SharedBufferError::NotFound)
    }

    fn live_region(&self, id: BufferId) -> Result<Region, SharedBufferError> {
        let region = self.live_region_any(id)?;
        if region.released {
            Err(SharedBufferError::NotFound)
        } else {
            Ok(region)
        }
    }

    fn live_region_any(&self, id: BufferId) -> Result<Region, SharedBufferError> {
        self.regions
            .iter()
            .flatten()
            .copied()
            .find(|region| region.id == id && region.epoch == self.epoch)
            .ok_or(SharedBufferError::NotFound)
    }

    fn live_loan(&self, id: LoanId) -> Result<Loan, SharedBufferError> {
        self.loans
            .iter()
            .flatten()
            .copied()
            .find(|loan| loan.id == id && loan.epoch == self.epoch)
            .ok_or(SharedBufferError::NotFound)
    }

    fn has_region_loans(&self, id: BufferId) -> bool {
        self.loans.iter().flatten().any(|loan| loan.buffer == id)
    }

    fn charge(&self, holder: HolderId) -> Option<Charge> {
        self.charges
            .iter()
            .flatten()
            .copied()
            .find(|charge| charge.holder == holder)
    }

    fn charge_slot(&self, holder: HolderId) -> Option<usize> {
        self.charges
            .iter()
            .position(|charge| charge.is_some_and(|charge| charge.holder == holder))
    }

    fn preflight_charge_slot(&self, holder: HolderId) -> Result<(), SharedBufferError> {
        if self.charge_slot(holder).is_some() || self.charges.iter().any(Option::is_none) {
            Ok(())
        } else {
            Err(SharedBufferError::ChargesExhausted)
        }
    }

    fn charge_positive(
        &mut self,
        holder: HolderId,
        pages: u32,
        buffers: u32,
        mappings: u32,
        loans: u32,
    ) -> Result<(), SharedBufferError> {
        let slot = self
            .charge_slot(holder)
            .or_else(|| self.charges.iter().position(Option::is_none))
            .ok_or(SharedBufferError::ChargesExhausted)?;
        let charge = self.charges[slot].get_or_insert(Charge {
            holder,
            pages: 0,
            buffers: 0,
            mappings: 0,
            loans: 0,
        });
        charge.pages += pages;
        charge.buffers += buffers;
        charge.mappings += mappings;
        charge.loans += loans;
        Ok(())
    }

    fn uncharge(
        &mut self,
        holder: HolderId,
        pages: u32,
        buffers: u32,
        mappings: u32,
        loans: u32,
    ) -> Result<(), SharedBufferError> {
        let slot = self
            .charge_slot(holder)
            .ok_or(SharedBufferError::NotFound)?;
        let charge = self.charges[slot]
            .as_mut()
            .ok_or(SharedBufferError::NotFound)?;
        charge.pages = charge
            .pages
            .checked_sub(pages)
            .ok_or(SharedBufferError::QuotaExceeded)?;
        charge.buffers = charge
            .buffers
            .checked_sub(buffers)
            .ok_or(SharedBufferError::QuotaExceeded)?;
        charge.mappings = charge
            .mappings
            .checked_sub(mappings)
            .ok_or(SharedBufferError::QuotaExceeded)?;
        charge.loans = charge
            .loans
            .checked_sub(loans)
            .ok_or(SharedBufferError::QuotaExceeded)?;
        if charge.is_empty() {
            self.charges[slot] = None;
        }
        Ok(())
    }

    const fn map_adapter_error(error: AdapterError) -> SharedBufferError {
        SharedBufferError::Adapter(error)
    }

    const fn adapter_cause(error: SharedBufferError) -> AdapterError {
        match error {
            SharedBufferError::Adapter(error) => error,
            SharedBufferError::Rollback { cause, .. } => cause,
            _ => AdapterError::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ActionList::boxed` writes every slot through a raw allocation rather
    /// than building the 144 KiB array in a stack frame. A missed slot would
    /// leave uninitialized memory that reads as a bogus action, so this walks
    /// the whole array rather than spot-checking the ends.
    ///
    /// The first attempt at `boxed` used `alloc_zeroed`, on the assumption
    /// that `None` is the all-zero pattern. It is not, and this test is what
    /// caught it.
    #[test]
    fn a_boxed_list_is_empty_in_every_slot() {
        let boxed = ActionList::boxed();
        assert_eq!(boxed.len(), 0);
        assert!(boxed.is_empty());
        for index in 0..MAX_TEARDOWN_ACTIONS {
            assert!(
                boxed.actions[index].is_none(),
                "slot {index} of a fresh list is not empty"
            );
            assert!(boxed.get(index).is_none());
        }
    }

    const OWNER: HolderId = HolderId(1);
    const RECEIVER: HolderId = HolderId(2);
    const EPOCH: GenerationEpoch = GenerationEpoch(7);
    const QUOTA: HolderQuota = HolderQuota {
        byte_pages: 8,
        buffer_count: 4,
        mapping_count: 4,
        loan_count: 4,
    };

    struct RecordingAdapter {
        actions: [Option<AdapterAction>; MAX_TEARDOWN_ACTIONS],
        len: usize,
        calls: usize,
        fail_at: Option<usize>,
        fail_from: Option<usize>,
    }

    impl RecordingAdapter {
        const fn new() -> Self {
            Self {
                actions: [None; MAX_TEARDOWN_ACTIONS],
                len: 0,
                calls: 0,
                fail_at: None,
                fail_from: None,
            }
        }

        fn failing_at(call: usize) -> Self {
            Self {
                fail_at: Some(call),
                ..Self::new()
            }
        }

        /// Fail this call and every call after it, so a rollback triggered by
        /// the first failure also fails.
        fn failing_from(call: usize) -> Self {
            Self {
                fail_from: Some(call),
                ..Self::new()
            }
        }

        fn record(&mut self, action: AdapterAction) -> Result<(), AdapterError> {
            let call = self.calls;
            self.calls += 1;
            if self.fail_at == Some(call) || self.fail_from.is_some_and(|first| call >= first) {
                return Err(AdapterError::MapConflict);
            }
            self.actions[self.len] = Some(action);
            self.len += 1;
            Ok(())
        }
    }

    impl SharedBufferAdapter for RecordingAdapter {
        fn map_frame(
            &mut self,
            frame: FrameCap,
            vspace: VSpaceCap,
            vaddr: usize,
            _rights: MappingRights,
        ) -> Result<(), AdapterError> {
            self.record(AdapterAction::Unmap {
                frame,
                vspace,
                vaddr,
            })
        }

        fn perform(&mut self, action: AdapterAction) -> Result<(), AdapterError> {
            self.record(action)
        }
    }

    fn anchors(first: usize, count: usize) -> FrameAnchors {
        let caps = core::array::from_fn::<_, MAX_BUFFER_PAGES, _>(|index| FrameCap(first + index));
        FrameAnchors::from_slice(&caps[..count]).expect("valid anchors")
    }

    /// A table whose generation has declared [`QUOTA`] for both test holders.
    /// An undeclared holder is denied outright, so every test that expects to
    /// allocate must first receive a ceiling from the table, never from a
    /// caller argument.
    fn table() -> SharedBufferTable {
        let mut table = SharedBufferTable::new(EPOCH);
        table.declare_quota(OWNER, QUOTA).expect("owner quota");
        table
            .declare_quota(RECEIVER, QUOTA)
            .expect("receiver quota");
        table
    }

    #[test]
    fn create_preflight_does_not_consume_anchors_or_accounting() {
        let mut table = table();
        let plan = table
            .preflight_create(OWNER, anchors(10, 2), true)
            .expect("preflight");
        assert_eq!(table.total_pages(), 0);
        assert_eq!(table.holder_buffers(OWNER), 0);
        assert_eq!(plan.anchors().len(), 2);
        let handle = table.commit_create(plan).expect("commit");
        assert_eq!(handle.id, plan.buffer_id());
        assert_eq!(table.total_pages(), 2);
        assert_eq!(table.holder_buffers(OWNER), 1);
        assert_eq!(table.commit_create(plan), Err(SharedBufferError::NotFound));
    }

    #[test]
    fn failed_partial_map_rolls_back_without_accounting() {
        let mut table = table();
        let handle = table.create(OWNER, anchors(10, 3), true).expect("create");
        let mut adapter = RecordingAdapter::failing_at(1);
        assert_eq!(
            table.map(
                &mut adapter,
                OWNER,
                handle,
                VSpaceCap(40),
                0x20_000,
                0,
                PAGE_SIZE * 3,
                MappingRights::ReadWrite,
            ),
            Err(SharedBufferError::Adapter(AdapterError::MapConflict))
        );
        assert_eq!(table.mapping_count(), 0);
        assert_eq!(table.holder_mappings(OWNER), 0);
        assert_eq!(adapter.calls, 3);
    }

    #[test]
    fn sealing_commits_only_after_read_only_remap() {
        let mut table = table();
        let handle = table.create(OWNER, anchors(10, 2), true).expect("create");
        let mut adapter = RecordingAdapter::new();
        table
            .map(
                &mut adapter,
                OWNER,
                handle,
                VSpaceCap(40),
                0x20_000,
                0,
                PAGE_SIZE * 2,
                MappingRights::ReadWrite,
            )
            .expect("map");
        table.seal(&mut adapter, OWNER, handle).expect("seal");
        assert_eq!(
            table.mapping(0).expect("mapping").rights,
            MappingRights::ReadOnly
        );
        assert_eq!(
            table.map(
                &mut adapter,
                OWNER,
                handle,
                VSpaceCap(40),
                0x30_000,
                0,
                PAGE_SIZE,
                MappingRights::ReadWrite,
            ),
            Err(SharedBufferError::WriteDenied)
        );
    }

    #[test]
    fn loan_is_receiver_bound_single_return_and_retains_release() {
        let mut table = table();
        let handle = table.create(OWNER, anchors(10, 2), true).expect("create");
        let mut adapter = RecordingAdapter::new();
        table.seal(&mut adapter, OWNER, handle).expect("seal");
        let loan = table
            .loan(OWNER, RECEIVER, handle, 0, PAGE_SIZE * 2)
            .expect("loan");
        table.release(&mut adapter, OWNER, handle).expect("release");
        assert_eq!(table.total_pages(), 2);
        table
            .map_loan(
                &mut adapter,
                RECEIVER,
                loan,
                VSpaceCap(7),
                PAGE_SIZE * 4,
                0,
                PAGE_SIZE * 2,
            )
            .expect("released owner region remains mappable through live loan");
        assert_eq!(
            table.return_loan(&mut adapter, HolderId(99), loan),
            Err(SharedBufferError::WrongReceiver)
        );
        table
            .return_loan(&mut adapter, RECEIVER, loan)
            .expect("return");
        assert_eq!(table.total_pages(), 0);
        assert_eq!(
            table.return_loan(&mut adapter, RECEIVER, loan),
            Err(SharedBufferError::NotFound)
        );
    }

    #[test]
    fn failed_seal_restores_writable_mapping_and_state() {
        let mut table = table();
        let handle = table.create(OWNER, anchors(10, 2), true).expect("create");
        let mut adapter = RecordingAdapter::new();
        table
            .map(
                &mut adapter,
                OWNER,
                handle,
                VSpaceCap(40),
                0x20_000,
                0,
                PAGE_SIZE * 2,
                MappingRights::ReadWrite,
            )
            .expect("map");
        let mut failing = RecordingAdapter::failing_at(3);
        assert_eq!(
            table.seal(&mut failing, OWNER, handle),
            Err(SharedBufferError::Adapter(AdapterError::MapConflict))
        );
        assert_eq!(
            table.mapping(0).expect("mapping").rights,
            MappingRights::ReadWrite
        );
        let mut retry = RecordingAdapter::new();
        table.seal(&mut retry, OWNER, handle).expect("seal retry");
    }

    #[test]
    fn receiver_death_settles_loan_without_reclaiming_live_lender() {
        let mut table = table();
        let handle = table.create(OWNER, anchors(10, 2), true).expect("create");
        let mut adapter = RecordingAdapter::new();
        table.seal(&mut adapter, OWNER, handle).expect("seal");
        let loan = table
            .loan(OWNER, RECEIVER, handle, 0, PAGE_SIZE)
            .expect("loan");
        table
            .map_loan(
                &mut adapter,
                RECEIVER,
                loan,
                VSpaceCap(41),
                0x30_000,
                0,
                PAGE_SIZE,
            )
            .expect("loan map");
        let actions = table
            .reclaim_holder(&mut adapter, RECEIVER)
            .expect("receiver cleanup");
        assert_eq!(actions.len(), 1);
        assert_eq!(table.loan_count(), 0);
        assert_eq!(table.mapping_count(), 0);
        assert_eq!(table.total_pages(), 2);
        assert_eq!(table.holder_buffers(OWNER), 1);
        assert_eq!(table.holder_loans(OWNER), 0);
    }

    #[test]
    fn teardown_order_is_mapping_then_revoke_then_release() {
        let mut table = table();
        let handle = table.create(OWNER, anchors(10, 2), true).expect("create");
        let mut adapter = RecordingAdapter::new();
        table
            .map(
                &mut adapter,
                OWNER,
                handle,
                VSpaceCap(40),
                0x20_000,
                0,
                PAGE_SIZE * 2,
                MappingRights::ReadOnly,
            )
            .expect("map");
        adapter.len = 0;
        adapter.calls = 0;
        let actions = table.reclaim_holder(&mut adapter, OWNER).expect("reclaim");
        assert_eq!(actions.len(), 6);
        assert!(matches!(actions.get(0), Some(AdapterAction::Unmap { .. })));
        assert!(matches!(actions.get(1), Some(AdapterAction::Unmap { .. })));
        assert_eq!(
            actions.get(2),
            Some(AdapterAction::Revoke {
                frame: FrameCap(10)
            })
        );
        assert_eq!(
            actions.get(3),
            Some(AdapterAction::Revoke {
                frame: FrameCap(11)
            })
        );
        assert_eq!(
            actions.get(4),
            Some(AdapterAction::ReleaseFrame {
                frame: FrameCap(10)
            })
        );
        assert_eq!(
            actions.get(5),
            Some(AdapterAction::ReleaseFrame {
                frame: FrameCap(11)
            })
        );
        assert_eq!(table.total_pages(), 0);
    }

    #[test]
    fn stale_epoch_fails_before_adapter_calls() {
        let mut table = table();
        let handle = table.create(OWNER, anchors(10, 1), true).expect("create");
        let mut stale = handle;
        stale.epoch = GenerationEpoch(EPOCH.0 - 1);
        let mut adapter = RecordingAdapter::new();
        assert_eq!(
            table.map(
                &mut adapter,
                OWNER,
                stale,
                VSpaceCap(40),
                0x20_000,
                0,
                PAGE_SIZE,
                MappingRights::ReadOnly,
            ),
            Err(SharedBufferError::EpochMismatch)
        );
        assert_eq!(adapter.calls, 0);
    }

    /// Finding 1: rights are table-held. A caller that hands back a handle with
    /// wider bits than the region grants must not get a writable mapping.
    #[test]
    fn caller_cannot_widen_rights_by_forging_a_handle() {
        let mut table = table();
        // A read-only region: `created_writable` is false, so the table records
        // MAP only, without WRITE or unreachable LOAN authority.
        let handle = table.create(OWNER, anchors(10, 1), false).expect("create");
        assert!(!handle.rights.contains(BufferRights::WRITE));

        let mut forged = handle;
        forged.rights = BufferRights::ALL;

        let mut adapter = RecordingAdapter::new();
        assert_eq!(
            table.map(
                &mut adapter,
                OWNER,
                forged,
                VSpaceCap(40),
                0x20_000,
                0,
                PAGE_SIZE,
                MappingRights::ReadWrite,
            ),
            Err(SharedBufferError::RightsDenied)
        );
        // Rejected before any invocation: the adapter never saw a map call.
        assert_eq!(adapter.calls, 0);
        assert_eq!(table.mapping_count(), 0);
    }

    /// Finding 1: a narrowed handle stays narrowed. Intersection with the
    /// table's bits must not restore authority the holder gave up.
    #[test]
    fn narrowed_handle_cannot_recover_write_authority() {
        let mut table = table();
        let handle = table.create(OWNER, anchors(10, 1), true).expect("create");
        let narrowed = handle.derive(BufferRights::MAP);
        let mut adapter = RecordingAdapter::new();
        assert_eq!(
            table.map(
                &mut adapter,
                OWNER,
                narrowed,
                VSpaceCap(40),
                0x20_000,
                0,
                PAGE_SIZE,
                MappingRights::ReadWrite,
            ),
            Err(SharedBufferError::RightsDenied)
        );
        assert_eq!(adapter.calls, 0);
    }

    /// Finding 1: the holder claim is verified against the recorded owner, so
    /// one holder cannot charge a mapping against another holder's quota.
    #[test]
    fn caller_cannot_charge_another_holders_quota() {
        let mut table = table();
        let handle = table.create(OWNER, anchors(10, 1), true).expect("create");
        let mut adapter = RecordingAdapter::new();
        assert_eq!(
            table.map(
                &mut adapter,
                RECEIVER,
                handle,
                VSpaceCap(40),
                0x20_000,
                0,
                PAGE_SIZE,
                MappingRights::ReadWrite,
            ),
            Err(SharedBufferError::WrongOwner)
        );
        assert_eq!(adapter.calls, 0);
        assert_eq!(table.holder_mappings(RECEIVER), 0);
        assert_eq!(table.holder_mappings(OWNER), 0);
    }

    /// Finding 1: quotas are table-held. An undeclared holder is denied even
    /// though no caller argument says so.
    #[test]
    fn undeclared_holder_is_denied_without_a_caller_supplied_quota() {
        let mut table = SharedBufferTable::new(EPOCH);
        assert_eq!(table.quota(OWNER), HolderQuota::DENY);
        assert_eq!(
            table.create(OWNER, anchors(10, 1), true),
            Err(SharedBufferError::QuotaExceeded)
        );
        table.declare_quota(OWNER, QUOTA).expect("declare");
        table.create(OWNER, anchors(10, 1), true).expect("create");
    }

    /// Finding 2: one frame cap must not anchor two live regions, even across
    /// different holders — that would alias one physical frame into two
    /// independently accounted buffers.
    #[test]
    fn frame_anchor_cannot_alias_a_second_live_region() {
        let mut table = table();
        table.create(OWNER, anchors(10, 2), true).expect("first");
        // Overlaps FrameCap(11) with the live region above.
        assert_eq!(
            table.create(RECEIVER, anchors(11, 2), true),
            Err(SharedBufferError::DuplicateFrameAnchor)
        );
        assert_eq!(table.live_count(), 1);
        assert_eq!(table.total_pages(), 2);
        assert_eq!(table.holder_pages(RECEIVER), 0);
    }

    /// Finding 2: the check is against *live* regions only. Once a region is
    /// reclaimed its frames are free to anchor a new one.
    #[test]
    fn reclaimed_frame_anchor_can_be_reused() {
        let mut table = table();
        let handle = table.create(OWNER, anchors(10, 2), true).expect("create");
        let mut adapter = RecordingAdapter::new();
        table.release(&mut adapter, OWNER, handle).expect("release");
        assert_eq!(table.live_count(), 0);
        table.create(RECEIVER, anchors(10, 2), true).expect("reuse");
    }

    /// Finding 3: a rollback unmap that itself fails leaves a page live in the
    /// target VSpace. It must be retained as an orphan, not silently dropped,
    /// or nothing in the table could ever reclaim that page.
    #[test]
    fn failed_rollback_retains_the_orphaned_page() {
        let mut table = table();
        let handle = table.create(OWNER, anchors(10, 3), true).expect("create");
        // Call 0 maps page 0; call 1 (page 1) fails, triggering rollback; the
        // rollback unmap of page 0 is call 2, which also fails.
        let mut adapter = RecordingAdapter::failing_from(1);
        let error = table
            .map(
                &mut adapter,
                OWNER,
                handle,
                VSpaceCap(40),
                0x20_000,
                0,
                PAGE_SIZE * 2,
                MappingRights::ReadWrite,
            )
            .expect_err("map fails");
        assert!(
            matches!(
                error,
                SharedBufferError::Orphaned {
                    cause: AdapterError::MapConflict,
                    rollback: AdapterError::MapConflict,
                    orphans: 1,
                    unrecorded: 0,
                }
            ),
            "expected an Orphaned error, got {error:?}"
        );
        // The still-mapped page is accounted for, and the mapping itself never
        // committed, so no charge was taken for a mapping that does not exist.
        assert_eq!(table.orphan_count(), 1);
        assert_eq!(table.mapping_count(), 0);
        assert_eq!(table.holder_mappings(OWNER), 0);

        // A working adapter drains the orphan; the exact page is retried.
        let mut retry = RecordingAdapter::new();
        assert_eq!(table.retry_orphans(&mut retry), 0);
        assert_eq!(
            retry.actions[0],
            Some(AdapterAction::Unmap {
                frame: FrameCap(10),
                vspace: VSpaceCap(40),
                vaddr: 0x20_000,
            })
        );
        assert_eq!(table.orphan_count(), 0);
    }

    /// Finding 3: an orphan that fails again on retry stays recorded, so
    /// reclamation reports itself incomplete rather than losing the page.
    #[test]
    fn orphan_survives_a_failed_retry() {
        let mut table = table();
        let handle = table.create(OWNER, anchors(10, 3), true).expect("create");
        let mut adapter = RecordingAdapter::failing_from(1);
        table
            .map(
                &mut adapter,
                OWNER,
                handle,
                VSpaceCap(40),
                0x20_000,
                0,
                PAGE_SIZE * 2,
                MappingRights::ReadWrite,
            )
            .expect_err("map fails");
        assert_eq!(table.orphan_count(), 1);
        let mut still_failing = RecordingAdapter::failing_from(0);
        assert_eq!(table.retry_orphans(&mut still_failing), 1);
        assert_eq!(table.orphan_count(), 1);
    }

    /// Read-only creation grants only the authority its lifecycle can use:
    /// mapping, without an unreachable loan bit that can never be sealed.
    #[test]
    fn read_only_regions_do_not_advertise_unreachable_loan_authority() {
        let mut table = table();
        let handle = table
            .create(OWNER, anchors(10, 1), false)
            .expect("create read-only");
        assert_eq!(handle.rights, BufferRights::MAP);
        assert_eq!(
            table.loan(OWNER, RECEIVER, handle, 0, PAGE_SIZE),
            Err(SharedBufferError::RightsDenied)
        );
    }

    #[test]
    fn mapping_page_admission_accepts_exact_capacity_and_reopens_after_retry() {
        let mut table = table();
        let handle = table.create(OWNER, anchors(10, 1), true).expect("create");
        for index in 0..MAX_MAPPING_PAGES - 1 {
            table.orphans[index] = Some(Orphan {
                frame: FrameCap(100 + index),
                vspace: VSpaceCap(200),
                vaddr: index * PAGE_SIZE,
            });
        }
        let mut adapter = RecordingAdapter::new();
        table
            .map(
                &mut adapter,
                OWNER,
                handle,
                VSpaceCap(40),
                0x20_000,
                0,
                PAGE_SIZE,
                MappingRights::ReadWrite,
            )
            .expect("exact capacity is admitted");
        assert_eq!(table.mapping_count(), 1);

        let mut overflow = RecordingAdapter::new();
        assert_eq!(
            table.map(
                &mut overflow,
                OWNER,
                handle,
                VSpaceCap(41),
                0x30_000,
                0,
                PAGE_SIZE,
                MappingRights::ReadWrite,
            ),
            Err(SharedBufferError::MappingsExhausted)
        );
        assert_eq!(overflow.calls, 0);

        let mut retry = RecordingAdapter::new();
        assert_eq!(table.retry_orphans(&mut retry), 0);
        let mut after_retry = RecordingAdapter::new();
        table
            .map(
                &mut after_retry,
                OWNER,
                handle,
                VSpaceCap(41),
                0x30_000,
                0,
                PAGE_SIZE,
                MappingRights::ReadWrite,
            )
            .expect("released capacity is reusable");
    }

    #[test]
    fn rollback_continues_after_orphan_table_exhaustion() {
        let mut table = table();
        let handle = table.create(OWNER, anchors(10, 3), true).expect("create");
        let region = table.live_region(handle.id).expect("region");
        let plan = table
            .preflight_mapping(
                OWNER,
                region,
                VSpaceCap(40),
                0x20_000,
                0,
                3,
                MappingRights::ReadWrite,
                None,
            )
            .expect("preflight before exhaustion");
        for index in 0..MAX_ORPHANS {
            table.orphans[index] = Some(Orphan {
                frame: FrameCap(100 + index),
                vspace: VSpaceCap(200),
                vaddr: index * PAGE_SIZE,
            });
        }
        // Two maps succeed, the third fails, then both rollback unmaps fail.
        let mut adapter = RecordingAdapter::failing_from(2);
        assert_eq!(
            table.execute_mapping(&mut adapter, plan),
            Err(SharedBufferError::Orphaned {
                cause: AdapterError::MapConflict,
                rollback: AdapterError::MapConflict,
                orphans: 0,
                unrecorded: 2,
            })
        );
        assert_eq!(adapter.calls, 5);
        assert_eq!(table.mapping_count(), 0);
        assert_eq!(table.orphan_count(), MAX_ORPHANS);
    }

    /// every release. With two regions torn down together, a per-region loop
    /// would emit Revoke(A) Release(A) Revoke(B) Release(B); the contract
    /// requires Revoke(A) Revoke(B) Release(A) Release(B).
    #[test]
    fn teardown_revokes_every_region_before_releasing_any() {
        let mut table = table();
        table.create(OWNER, anchors(10, 1), true).expect("first");
        table.create(OWNER, anchors(20, 1), true).expect("second");
        let mut adapter = RecordingAdapter::new();
        let actions = table
            .reclaim_holder(&mut adapter, OWNER)
            .expect("reclaim both regions");
        assert_eq!(actions.len(), 4);
        assert_eq!(
            actions.get(0),
            Some(AdapterAction::Revoke {
                frame: FrameCap(10)
            })
        );
        assert_eq!(
            actions.get(1),
            Some(AdapterAction::Revoke {
                frame: FrameCap(20)
            })
        );
        assert_eq!(
            actions.get(2),
            Some(AdapterAction::ReleaseFrame {
                frame: FrameCap(10)
            })
        );
        assert_eq!(
            actions.get(3),
            Some(AdapterAction::ReleaseFrame {
                frame: FrameCap(20)
            })
        );
        assert_eq!(table.total_pages(), 0);
        assert_eq!(table.holder_pages(OWNER), 0);
    }
}
