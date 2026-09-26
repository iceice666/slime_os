//! Runtime qualification of demand-backed private memory.
//!
//! Each scenario is compiled only into its own root role's image and runs
//! before any component is published, so a refusal it provokes cannot reach a
//! task. They drive the product paths: the same allocator, the same policy
//! ledger, the same retypes and the same kernel mappings a component's growth
//! would take.
//!
//! What differs from a component is the address space. A holder here is an
//! arena plus a window in the root's own VSpace, which is what lets the
//! scenario write a pattern into every committed page and read it back after
//! a failure, a revoke and a reuse. Window placement, admission and spawn
//! integration are the next stage's; nothing here publishes an adaptive
//! region to a component.

use boot_contracts::private_memory_policy::{
    self as policy, Entitlement, Header, Instance, Subject,
    ledger::{self, Incarnation, Ledger, Resources},
};

use super::{Region, Table, elastic};
use crate::child_vspace::GRANULE_SIZE;
use crate::child_vspace::LARGE_FRAME_PAGES;
use crate::object_allocator::{AllocError, ObjectAllocator, TaskArenaId};

/// First qualification window in the root's own address space.
///
/// Clear of the root image, which links below 16 MiB, and clear of the
/// metadata window at 2^37, which the bootstrap allocator owns. Every
/// reference architecture's user address space covers it: Sv39's 256 GiB is
/// the smallest, and the last holder's window ends far below it.
const WINDOW_BASE: usize = 1 << 36;
/// One window per holder, spaced so no two can ever meet.
const WINDOW_STRIDE: usize = 1 << 30;
const MAX_HOLDERS: usize = 4;
/// Address space every holder reserves, whether or not it grows into it.
const HOLDER_WINDOW_PAGES: usize = super::MAX_REGION_PAGES;

/// Bytes the scenario leaves in the elastic pool after its declared reserve.
///
/// Small on purpose: the evidence is about reaching the pool's exact boundary
/// and reconciling what is left, which a machine-sized pool would turn into a
/// long walk rather than a sharper observation. Everything above it is held
/// by the policy's operational reserve, whose serviceability the same
/// scenarios check.
const ELASTIC_POOL_BYTES: u64 = 32 * 1024 * 1024;
/// Pages the guaranteed entitlement promises.
const GUARANTEE_PAGES: u64 = 64;

const MAX_POLICY_BYTES: usize =
    policy::HEADER_BYTES + 2 * policy::ENTITLEMENT_BYTES + MAX_HOLDERS * policy::SUBJECT_BYTES;

/// Why a scenario could not run to its conclusion.
///
/// A qualification failure is never a soft result: every one of these means
/// the evidence this role exists to produce was not observed.
#[derive(Clone, Copy, Debug)]
pub enum QualificationError {
    Allocator(AllocError),
    Policy(ledger::Error),
    PolicyFormat(policy::Error),
    Growth(elastic::ElasticGrowError),
    /// An observation the scenario requires did not hold.
    Unmet(&'static str),
}

impl From<AllocError> for QualificationError {
    fn from(error: AllocError) -> Self {
        Self::Allocator(error)
    }
}

impl From<ledger::Error> for QualificationError {
    fn from(error: ledger::Error) -> Self {
        Self::Policy(error)
    }
}

impl From<policy::Error> for QualificationError {
    fn from(error: policy::Error) -> Self {
        Self::PolicyFormat(error)
    }
}

/// The entitlements every scenario's policy declares.
const GUARANTEED: &str = "qualification-guaranteed";
const ELASTIC: &str = "qualification-elastic";

/// One scenario's policy bytes, built in the root image from the inventory the
/// machine actually reported.
///
/// The policy is deterministic given an inventory: the same admitted RAM
/// produces the same reserve, the same entitlements and the same decisions.
/// It is a test-only composition, and no generation carries it.
struct PolicyBuffer {
    bytes: [u8; MAX_POLICY_BYTES],
    len: usize,
}

impl PolicyBuffer {
    const fn new() -> Self {
        Self {
            bytes: [0; MAX_POLICY_BYTES],
            len: 0,
        }
    }

    fn build(&mut self, available: Resources, subjects: &[(&str, &str)]) -> Result<(), ()> {
        // Everything the machine has beyond the scenario's pool is declared as
        // the operational reserve, so exhausting the elastic pool leaves the
        // reserve untouched and observably so.
        let reserve = Resources {
            bytes: available.bytes.saturating_sub(ELASTIC_POOL_BYTES),
            slots: 0,
            descriptors: 0,
            extents: 0,
            tables: 0,
        };
        // Only the entitlements this scenario's subjects join are declared: an
        // entitlement with no member is a promise nobody can redeem, and the
        // contract refuses it rather than admitting dead authorization.
        let guaranteed = subjects.iter().any(|(_, group)| *group == GUARANTEED);
        let elastic = subjects.iter().any(|(_, group)| *group == ELASTIC);
        let mut declared = [
            Entitlement {
                identity: policy::entitlement_identity(GUARANTEED),
                subtree_root: [0; 32],
                guarantee_pages: GUARANTEE_PAGES,
                maximum_pages: GUARANTEE_PAGES,
                maximum_mode: policy::FIXED,
                reserved: 0,
            },
            Entitlement {
                identity: policy::entitlement_identity(ELASTIC),
                subtree_root: [0; 32],
                guarantee_pages: 0,
                maximum_pages: 0,
                maximum_mode: policy::POOL,
                reserved: 0,
            },
        ];
        if !guaranteed {
            declared.swap(0, 1);
        }
        let entitlements = &mut declared[..usize::from(guaranteed) + usize::from(elastic)];
        entitlements.sort_unstable_by_key(|entry| entry.identity);
        let mut encoded = [Subject {
            identity: [0; 32],
            entitlement: [0; 32],
            maximum_pages: 0,
            maximum_mode: policy::POOL,
            reserved: 0,
        }; MAX_HOLDERS];
        if subjects.len() > MAX_HOLDERS {
            return Err(());
        }
        for (slot, (name, group)) in encoded.iter_mut().zip(subjects) {
            *slot = Subject {
                identity: policy::subject_identity(name),
                entitlement: policy::entitlement_identity(group),
                maximum_pages: if *group == GUARANTEED {
                    GUARANTEE_PAGES
                } else {
                    0
                },
                maximum_mode: if *group == GUARANTEED {
                    policy::FIXED
                } else {
                    policy::POOL
                },
                reserved: 0,
            };
        }
        let encoded = &mut encoded[..subjects.len()];
        encoded.sort_unstable_by_key(|entry| entry.identity);
        let header = Header {
            magic: policy::MAGIC,
            format_version: policy::FORMAT_VERSION,
            header_size: policy::HEADER_BYTES as u32,
            required_flags: 0,
            entitlement_count: entitlements.len() as u32,
            subject_count: encoded.len() as u32,
            total_len: (policy::HEADER_BYTES
                + entitlements.len() * policy::ENTITLEMENT_BYTES
                + encoded.len() * policy::SUBJECT_BYTES) as u32,
            reserved: 0,
            reserve_bytes: reserve.bytes,
            reserve_slots: reserve.slots,
            reserve_descriptors: reserve.descriptors,
            reserve_extents: reserve.extents,
            reserve_tables: reserve.tables,
        };
        self.len = 0;
        self.push(&header.encode());
        for entry in entitlements.iter() {
            self.push(&entry.encode());
        }
        for entry in encoded.iter() {
            self.push(&entry.encode());
        }
        Ok(())
    }

    fn push(&mut self, bytes: &[u8]) {
        self.bytes[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
    }

    fn policy(&self) -> Result<policy::Policy<'_>, policy::Error> {
        policy::Policy::decode(&self.bytes[..self.len])
    }
}

/// One elastic holder: its window, its arena and its ledger incarnation.
struct Holder {
    name: &'static str,
    arena: TaskArenaId,
    region: Region,
    token: Incarnation,
    committed: usize,
}

impl Holder {
    /// Reserve a holder's window and bind it to its subject, taking nothing
    /// from the pool but the upper tables the window itself needs.
    fn admit(
        allocator: &mut ObjectAllocator,
        ledger: &mut Ledger<'_>,
        name: &'static str,
        index: usize,
        maximum: usize,
    ) -> Result<Self, QualificationError> {
        let arena = allocator.begin_task_arena(16)?;
        allocator.mark_arena_elastic(arena)?;
        let base = WINDOW_BASE
            .checked_add(
                index
                    .checked_mul(WINDOW_STRIDE)
                    .ok_or(QualificationError::Unmet(
                        "holder window stride does not fit an address",
                    ))?,
            )
            .ok_or(QualificationError::Unmet(
                "holder window base does not fit an address",
            ))?;
        reserve_window(allocator, arena, base)?;
        let region = Region::reserve(allocator, base, HOLDER_WINDOW_PAGES, maximum, true)
            .map_err(|_| QualificationError::Unmet("window base is unrepresentable"))?;
        let token = ledger.bind(&policy::subject_identity(name))?;
        Ok(Self {
            name,
            arena,
            region,
            token,
            committed: 0,
        })
    }

    /// Ask for `pages` more, reporting what the growth actually cost.
    fn grow(
        &mut self,
        table: &mut Table,
        allocator: &mut ObjectAllocator,
        ledger: &mut Ledger<'_>,
        pages: usize,
    ) -> Result<usize, elastic::ElasticGrowError> {
        let previous = elastic::grow_native(
            table,
            allocator,
            ledger,
            self.token,
            self.arena,
            sel4::init_thread::slot::VSPACE.cap(),
            &mut self.region,
            pages,
            super::GrowthPlan::elastic(),
        )?;
        self.committed = self.region.pages();
        Ok(previous)
    }

    /// Write one word per page, derived from the page's own address, so a
    /// later read proves both that the page survived and that it is the page
    /// this holder mapped rather than another holder's.
    fn write_pattern(&self, from: usize, to: usize) {
        for page in from..to {
            let address = self.region.base() + page * GRANULE_SIZE;
            // SAFETY: every page in `from..to` is committed private backing
            // this holder mapped read-write into the root's own VSpace, and no
            // other owner may map inside a reserved window.
            unsafe { core::ptr::write_volatile(address as *mut usize, pattern(address)) };
        }
    }

    fn pattern_holds(&self, from: usize, to: usize) -> bool {
        (from..to).all(|page| {
            let address = self.region.base() + page * GRANULE_SIZE;
            // SAFETY: as in `write_pattern`; the range is committed backing.
            unsafe { core::ptr::read_volatile(address as *const usize) == pattern(address) }
        })
    }

    fn zeroed(&self, from: usize, to: usize) -> bool {
        (from..to).all(|page| {
            let address = self.region.base() + page * GRANULE_SIZE;
            // SAFETY: as in `write_pattern`; the range is committed backing.
            unsafe { core::ptr::read_volatile(address as *const usize) == 0 }
        })
    }

    /// Return this holder's capacity, reporting whether the revoke completed.
    fn retire(
        &mut self,
        table: &mut Table,
        allocator: &mut ObjectAllocator,
        ledger: &mut Ledger<'_>,
    ) -> Result<usize, QualificationError> {
        let revoked = allocator.release_task_arena(self.arena).is_ok();
        let pages = elastic::retire(
            table,
            allocator,
            ledger,
            self.token,
            &mut self.region,
            revoked,
        )?;
        self.committed = 0;
        Ok(pages)
    }
}

/// A page's expected content: its own address, mixed so neither a shifted
/// mapping nor a zeroed page can accidentally match.
const fn pattern(address: usize) -> usize {
    address ^ 0x0005_115e_0511_5e05_usize
}

/// Map the upper translation tables one window needs.
///
/// Leaf tables stay lazy — that is the mechanism under test — but every level
/// above the leaf must exist before the first mapping, exactly as a child
/// VSpace's construction establishes them.
///
/// A level whose table already covers the address is left alone: the root's
/// own image and an earlier holder's window can share an upper table, and the
/// kernel reports that by refusing the second map rather than by answering a
/// query. The table object allocated for such an address is carried to the
/// next one instead of being discarded, so a shared level costs nothing.
fn reserve_window(
    allocator: &mut ObjectAllocator,
    arena: TaskArenaId,
    base: usize,
) -> Result<(), AllocError> {
    let vspace = sel4::init_thread::slot::VSPACE.cap();
    let end = base + HOLDER_WINDOW_PAGES * GRANULE_SIZE;
    for level in 1..sel4::vspace_levels::NUM_LEVELS - 1 {
        let span = 1usize << sel4::vspace_levels::span_bits(level);
        let Some(ty) = sel4::TranslationTableObjectType::from_level(level) else {
            continue;
        };
        let mut address = base - base % span;
        let mut pending = None;
        while address < end {
            let table = match pending {
                Some(table) => table,
                None => allocator
                    .allocate_in(arena, ty.blueprint())?
                    .cap()
                    .cast::<sel4::cap_type::UnspecifiedIntermediateTranslationTable>(),
            };
            match table.generic_intermediate_translation_table_map(
                ty,
                vspace,
                address,
                sel4::VmAttributes::default(),
            ) {
                Ok(()) => pending = None,
                Err(sel4::Error::DeleteFirst) => pending = Some(table),
                Err(error) => {
                    return Err(AllocError::ArenaCleanup {
                        slot: address,
                        error,
                    });
                }
            }
            address += span;
        }
    }
    Ok(())
}

/// Capacity no holder owns: ordinary tails, retained prefixes and the extents
/// returned holders left behind, counted once each.
#[cfg(any(slime_private_conservation, slime_private_fragmentation))]
fn unallocated_bytes(allocator: &ObjectAllocator) -> usize {
    allocator.untyped_bytes_remaining()
        + allocator.preserved_bytes_remaining()
        + allocator.reusable_extent_bytes()
}

/// Admit one scenario's policy against the inventory the machine reported.
fn admit<'a>(
    allocator: &ObjectAllocator,
    buffer: &'a mut PolicyBuffer,
    subjects: &[(&str, &str)],
    instances: &'a mut [Instance; MAX_HOLDERS],
) -> Result<Ledger<'a>, QualificationError> {
    let available = allocator.elastic_inventory();
    // The inventory a policy was admitted against is part of its evidence: the
    // same policy on a machine with different resources is a different
    // decision, and a refusal here must name which resource was missing.
    sel4::debug_println!(
        "SLIME_MEM elastic available bytes={} slots={} descriptors={} extents={} tables={} subjects={}",
        available.bytes,
        available.slots,
        available.descriptors,
        available.extents,
        available.tables,
        subjects.len(),
    );
    buffer
        .build(available, subjects)
        .map_err(|()| QualificationError::Unmet("more subjects than the scenario declares"))?;
    let decoded = buffer.policy()?;
    for (slot, (name, _)) in instances.iter_mut().zip(subjects) {
        *slot = Instance {
            identity: policy::subject_identity(name),
            owner: None,
        };
    }
    let live = &instances[..subjects.len()];
    let declared = decoded.entitlement_count();
    let mut envelopes = [Resources::ZERO; 2];
    let guarantees = &mut envelopes[..declared];
    for (index, slot) in guarantees.iter_mut().enumerate() {
        let pages = decoded
            .entitlement(index)
            .ok_or(QualificationError::Unmet("entitlement lost in encoding"))?
            .guarantee_pages;
        if pages != 0 {
            // A conservative per-page envelope: every guaranteed page can cost
            // its own extent, descriptor, slot and table. The allocator, not
            // this envelope, decides what a page actually takes.
            *slot = Resources {
                bytes: pages * policy::PAGE_BYTES * 2,
                slots: pages * 2,
                descriptors: pages * 2,
                extents: pages,
                tables: pages,
            };
        }
    }
    Ok(Ledger::admit(decoded, live, available, guarantees)?)
}

/// Report one holder's position, separating what it holds from what it may ask
/// for and from what the pool still has.
fn report_holder(name: &str, region: &Region, ledger: &Ledger<'_>, allocator: &ObjectAllocator) {
    let free = ledger.available();
    sel4::debug_println!(
        "SLIME_MEM elastic holder subject={} committed={} maximum={} reserved_window={} pool_bytes={} pool_slots={} ordinary={} reusable={}",
        name,
        region.pages(),
        region.quota(),
        region.reservation(),
        free.bytes,
        free.slots,
        allocator.untyped_bytes_remaining(),
        allocator.reusable_private_extent_bytes(),
    );
}

#[cfg(slime_private_elastic)]
pub fn exercise_idle_and_guarantee(
    allocator: &mut ObjectAllocator,
) -> Result<(), QualificationError> {
    let mut buffer = PolicyBuffer::new();
    let mut instances = [Instance {
        identity: [0; 32],
        owner: None,
    }; MAX_HOLDERS];
    let subjects = [
        ("qualification-guaranteed-holder", GUARANTEED),
        ("qualification-idle", ELASTIC),
        ("qualification-bulk", ELASTIC),
    ];
    let mut ledger = admit(allocator, &mut buffer, &subjects, &mut instances)?;
    let mut table = Table::new();
    allocator.report_elastic_census("admitted");

    let pool = ledger.available();
    sel4::debug_println!(
        "SLIME_MEM elastic admitted pool_bytes={} guarantee_pages={} holders={}",
        pool.bytes,
        GUARANTEE_PAGES,
        subjects.len(),
    );

    // An idle holder authorized against the whole pool. Its window is
    // reserved; its maximum is not.
    let before = ledger.available();
    let mut idle = Holder::admit(
        allocator,
        &mut ledger,
        "qualification-idle",
        1,
        HOLDER_WINDOW_PAGES,
    )?;
    let after = ledger.available();
    if after.bytes != before.bytes {
        return Err(QualificationError::Unmet("an idle maximum took pool bytes"));
    }
    sel4::debug_println!(
        "SLIME_MEM elastic idle subject={} maximum={} reserved_bytes={} pool_before={} pool_after={}",
        idle.name,
        idle.region.quota(),
        after.bytes.abs_diff(before.bytes),
        before.bytes,
        after.bytes,
    );

    let mut bulk = Holder::admit(
        allocator,
        &mut ledger,
        "qualification-bulk",
        2,
        HOLDER_WINDOW_PAGES,
    )?;
    let mut guaranteed = Holder::admit(
        allocator,
        &mut ledger,
        "qualification-guaranteed-holder",
        0,
        GUARANTEE_PAGES as usize,
    )?;

    // The peer consumes the spare capacity the idle holder did not reserve.
    let mut refusal = None;
    while refusal.is_none() {
        let previous = bulk.region.pages();
        match bulk.grow(&mut table, allocator, &mut ledger, LARGE_FRAME_PAGES) {
            Ok(_) => bulk.write_pattern(previous, bulk.region.pages()),
            Err(error) => refusal = Some(error),
        }
    }
    let refusal = refusal.expect("loop exits only on a refusal");
    sel4::debug_println!(
        "SLIME_MEM elastic grow subject={} committed={} bytes={} pool_after={}",
        bulk.name,
        bulk.region.pages(),
        bulk.region.pages() * GRANULE_SIZE,
        ledger.available().bytes,
    );
    sel4::debug_println!(
        "SLIME_MEM elastic refused subject={} pages={} cause={} committed={} peer_committed={}",
        bulk.name,
        LARGE_FRAME_PAGES,
        refusal.cause(),
        bulk.region.pages(),
        idle.region.pages(),
    );
    if bulk.region.pages() == 0 {
        return Err(QualificationError::Unmet("no spare capacity was served"));
    }

    // The guarantee is still affordable with the elastic pool exhausted, and
    // the idle holder can still take what its peer could not.
    guaranteed
        .grow(&mut table, allocator, &mut ledger, GUARANTEE_PAGES as usize)
        .map_err(QualificationError::Growth)?;
    guaranteed.write_pattern(0, guaranteed.region.pages());
    sel4::debug_println!(
        "SLIME_MEM elastic guarantee subject={} committed={} promised={} served=1",
        guaranteed.name,
        guaranteed.region.pages(),
        GUARANTEE_PAGES,
    );

    let idle_served = idle.grow(&mut table, allocator, &mut ledger, 1).is_ok();
    if idle_served {
        idle.write_pattern(0, idle.region.pages());
    }
    sel4::debug_println!(
        "SLIME_MEM elastic idle_request subject={} served={} committed={}",
        idle.name,
        u8::from(idle_served),
        idle.region.pages(),
    );

    // Neither holder was damaged by the refusal.
    let intact = bulk.pattern_holds(0, bulk.region.pages())
        && guaranteed.pattern_holds(0, guaranteed.region.pages())
        && (!idle_served || idle.pattern_holds(0, idle.region.pages()));
    sel4::debug_println!(
        "SLIME_MEM elastic intact bulk={} guaranteed={} idle={} pages={}",
        u8::from(bulk.pattern_holds(0, bulk.region.pages())),
        u8::from(guaranteed.pattern_holds(0, guaranteed.region.pages())),
        u8::from(!idle_served || idle.pattern_holds(0, idle.region.pages())),
        bulk.region.pages() + guaranteed.region.pages() + idle.region.pages(),
    );
    if !intact {
        return Err(QualificationError::Unmet("a refusal damaged a live holder"));
    }
    report_holder("qualification-bulk", &bulk.region, &ledger, allocator);
    report_holder(
        "qualification-guaranteed-holder",
        &guaranteed.region,
        &ledger,
        allocator,
    );
    allocator.report_elastic_census("served");

    for holder in [&mut bulk, &mut guaranteed, &mut idle] {
        holder.retire(&mut table, allocator, &mut ledger)?;
    }
    allocator.report_elastic_census("retired");
    sel4::debug_println!(
        "SLIME_MEM elastic complete case=idle-and-guarantee holders={} granted={} reclaimed={}",
        subjects.len(),
        table.grown_pages(),
        table.reclaimed_pages(),
    );
    Ok(())
}

#[cfg(slime_private_fragmentation)]
pub fn exercise_mixed_fragmentation(
    allocator: &mut ObjectAllocator,
) -> Result<(), QualificationError> {
    let mut buffer = PolicyBuffer::new();
    let mut instances = [Instance {
        identity: [0; 32],
        owner: None,
    }; MAX_HOLDERS];
    let subjects = [
        ("qualification-small", ELASTIC),
        ("qualification-mixed", ELASTIC),
        ("qualification-bulk", ELASTIC),
    ];
    let mut ledger = admit(allocator, &mut buffer, &subjects, &mut instances)?;
    let mut table = Table::new();
    allocator.report_elastic_census("admitted");
    sel4::debug_println!(
        "SLIME_MEM elastic inventory pool_bytes={} largest_aligned={} retained={} reusable={}",
        ledger.available().bytes,
        allocator.largest_aligned_ordinary_block(),
        allocator.preserved_bytes_remaining(),
        allocator.reusable_private_extent_bytes(),
    );

    // One page. A request this small must cost one page and its table, not
    // the 2 MiB span a quota-shaped reservation would take.
    let mut small = Holder::admit(
        allocator,
        &mut ledger,
        "qualification-small",
        0,
        HOLDER_WINDOW_PAGES,
    )?;
    let before = ledger.available().bytes;
    small
        .grow(&mut table, allocator, &mut ledger, 1)
        .map_err(QualificationError::Growth)?;
    small.write_pattern(0, 1);
    let single_cost = before - ledger.available().bytes;
    sel4::debug_println!(
        "SLIME_MEM elastic request kind=single pages=1 committed={} charged={} large=0 base=1 tables=1",
        small.region.pages(),
        single_cost,
    );
    if single_cost != 2 * GRANULE_SIZE as u64 {
        return Err(QualificationError::Unmet("a single page charged a span"));
    }

    // A mixed request: one aligned large frame and a partial span behind it.
    let mut mixed = Holder::admit(
        allocator,
        &mut ledger,
        "qualification-mixed",
        1,
        HOLDER_WINDOW_PAGES,
    )?;
    let shape = mixed.region.shape(LARGE_FRAME_PAGES + 3);
    let before = ledger.available().bytes;
    mixed
        .grow(&mut table, allocator, &mut ledger, LARGE_FRAME_PAGES + 3)
        .map_err(QualificationError::Growth)?;
    mixed.write_pattern(0, mixed.region.pages());
    sel4::debug_println!(
        "SLIME_MEM elastic request kind=mixed pages={} committed={} charged={} large={} base={} tables={}",
        LARGE_FRAME_PAGES + 3,
        mixed.region.pages(),
        before - ledger.available().bytes,
        shape.large_frames,
        shape.base_pages,
        shape.tables,
    );

    // Now remove every aligned ordinary block and serve page-granular growth
    // from retained prefixes and returned extents alone.
    let watermarks = allocator.hold_ordinary_tails();
    let aligned = allocator.largest_aligned_ordinary_block();
    let fragmented_from = small.region.pages();
    let fragmented = small.grow(&mut table, allocator, &mut ledger, 1);
    if fragmented.is_ok() {
        small.write_pattern(fragmented_from, small.region.pages());
    }
    sel4::debug_println!(
        "SLIME_MEM elastic fragmented ordinary_aligned={} served={} committed={} retained={} reusable={}",
        aligned,
        u8::from(fragmented.is_ok()),
        small.region.pages(),
        allocator.preserved_bytes_remaining(),
        allocator.reusable_private_extent_bytes(),
    );
    // The same machine cannot serve an aligned span: its remaining capacity
    // is real but no longer contiguous, and the refusal must say so as a
    // resource rather than as an authorization answer.
    let spanned = mixed.grow(&mut table, allocator, &mut ledger, LARGE_FRAME_PAGES);
    let span_cause = spanned.err().map_or("none", |error| error.cause());
    sel4::debug_println!(
        "SLIME_MEM elastic fragmented_span served={} cause={} committed={}",
        u8::from(span_cause == "none"),
        span_cause,
        mixed.region.pages(),
    );
    allocator.restore_ordinary_tails(&watermarks);
    if fragmented.is_err() {
        return Err(QualificationError::Unmet(
            "page-granular growth demanded an aligned span",
        ));
    }
    if span_cause == "none" || span_cause == "policy" {
        return Err(QualificationError::Unmet(
            "a span refusal did not name a resource",
        ));
    }

    // Bulk, in whole spans, until the pool refuses. The per-page cost of each
    // path is reported rather than a single best case.
    let mut bulk = Holder::admit(
        allocator,
        &mut ledger,
        "qualification-bulk",
        2,
        HOLDER_WINDOW_PAGES,
    )?;
    let before = ledger.available().bytes;
    let mut refusal = None;
    while refusal.is_none() {
        match bulk.grow(&mut table, allocator, &mut ledger, LARGE_FRAME_PAGES) {
            Ok(_) => {}
            Err(error) => refusal = Some(error),
        }
    }
    let refusal = refusal.expect("the loop exits only on a refusal");
    let bulk_charged = before - ledger.available().bytes;
    if bulk.region.pages() == 0 {
        return Err(QualificationError::Unmet("no bulk capacity was served"));
    }
    sel4::debug_println!(
        "SLIME_MEM elastic request kind=bulk pages={} committed={} charged={} large={} base=0 tables=0",
        bulk.region.pages(),
        bulk.region.pages(),
        bulk_charged,
        bulk.region.pages() / LARGE_FRAME_PAGES,
    );
    sel4::debug_println!(
        "SLIME_MEM elastic cost small_bytes_per_page={} bulk_bytes_per_page={}",
        single_cost,
        bulk_charged / bulk.region.pages() as u64,
    );
    sel4::debug_println!(
        "SLIME_MEM elastic refusal kind=bulk resource={} pool_bytes={} ordinary={} retained={} reusable={}",
        refusal.cause(),
        ledger.available().bytes,
        allocator.untyped_bytes_remaining(),
        allocator.preserved_bytes_remaining(),
        allocator.reusable_private_extent_bytes(),
    );

    // A still-fitting request must succeed after the terminal refusal: the
    // pool ran out of what a span needs, not of everything.
    let fitting = small.grow(&mut table, allocator, &mut ledger, 1).is_ok();
    if fitting {
        small.write_pattern(1, small.region.pages());
    }
    sel4::debug_println!(
        "SLIME_MEM elastic next kind=fitting served={} committed={} pool_bytes={}",
        u8::from(fitting),
        small.region.pages(),
        ledger.available().bytes,
    );
    if !fitting {
        return Err(QualificationError::Unmet(
            "a fitting request was refused after a span refusal",
        ));
    }

    // A returned holder's extents are reusable capacity, and the next request
    // takes them instead of a fresh block.
    let returned = bulk.retire(&mut table, allocator, &mut ledger)?;
    let reused_before = allocator.extents_reused();
    let after_reuse = small.grow(&mut table, allocator, &mut ledger, LARGE_FRAME_PAGES);
    sel4::debug_println!(
        "SLIME_MEM elastic reuse returned_pages={} reused_extents={} served={} committed={}",
        returned,
        allocator.extents_reused() - reused_before,
        u8::from(after_reuse.is_ok()),
        small.region.pages(),
    );
    // Every page this holder was ever served, including the ones fragmented
    // backing supplied, still holds its own pattern.
    if !small.pattern_holds(0, fragmented_from + 1) {
        return Err(QualificationError::Unmet("fragmented service lost a page"));
    }

    for holder in [&mut small, &mut mixed] {
        holder.retire(&mut table, allocator, &mut ledger)?;
    }
    allocator.report_elastic_census("retired");
    sel4::debug_println!(
        "SLIME_MEM elastic complete case=mixed-fragmentation granted={} reclaimed={}",
        table.grown_pages(),
        table.reclaimed_pages(),
    );
    Ok(())
}

/// A kernel that performs every operation for real except the one it was
/// asked to fail.
///
/// Retype and mapping failures need no product-code injection: the growth
/// path already takes its kernel through a trait, so a scenario can fail the
/// exact invocation it wants to observe the rollback of.
#[cfg(slime_private_rollback)]
struct FailingKernel {
    native: super::NativePrivateMemoryKernel,
    operation: Operation,
    remaining: usize,
}

#[cfg(slime_private_rollback)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Operation {
    Retype,
    MapFrame,
    MapLeaf,
    Unmap,
}

#[cfg(slime_private_rollback)]
impl FailingKernel {
    fn new(region: &Region, delta: usize, operation: Operation, after: usize) -> Self {
        Self {
            native: super::NativePrivateMemoryKernel::for_growth(region, delta),
            operation,
            remaining: after,
        }
    }

    fn refuse(&mut self, operation: Operation) -> bool {
        if self.operation != operation {
            return false;
        }
        if self.remaining == 0 {
            return true;
        }
        self.remaining -= 1;
        false
    }
}

#[cfg(slime_private_rollback)]
impl super::PrivateMemoryKernel for FailingKernel {
    fn revoke(&mut self, parent: sel4::cap::Untyped) -> Result<(), sel4::Error> {
        self.native.revoke(parent)
    }

    fn retype(
        &mut self,
        parent: sel4::cap::Untyped,
        blueprint: &sel4::ObjectBlueprint,
        slot: usize,
    ) -> Result<(), sel4::Error> {
        if self.refuse(Operation::Retype) {
            return Err(sel4::Error::NotEnoughMemory);
        }
        self.native.retype(parent, blueprint, slot)
    }

    fn map_frame(
        &mut self,
        frame: sel4::cap::UnspecifiedPage,
        vspace: sel4::cap::VSpace,
        vaddr: usize,
        rights: sel4::CapRights,
        attrs: sel4::VmAttributes,
    ) -> Result<(), sel4::Error> {
        if self.refuse(Operation::MapFrame) {
            return Err(sel4::Error::FailedLookup);
        }
        self.native.map_frame(frame, vspace, vaddr, rights, attrs)
    }

    fn map_leaf(
        &mut self,
        table: sel4::cap::UnspecifiedIntermediateTranslationTable,
        ty: sel4::TranslationTableObjectType,
        vspace: sel4::cap::VSpace,
        vaddr: usize,
        attrs: sel4::VmAttributes,
    ) -> Result<(), sel4::Error> {
        if self.refuse(Operation::MapLeaf) {
            return Err(sel4::Error::DeleteFirst);
        }
        self.native.map_leaf(table, ty, vspace, vaddr, attrs)
    }

    fn unmap_frame(&mut self, frame: sel4::cap::UnspecifiedPage) -> Result<(), sel4::Error> {
        if self.refuse(Operation::Unmap) {
            return Err(sel4::Error::IllegalOperation);
        }
        self.native.unmap_frame(frame)
    }
}

#[cfg(slime_private_rollback)]
impl Holder {
    /// Grow through a kernel of the scenario's choosing.
    fn grow_with<K: super::PrivateMemoryKernel>(
        &mut self,
        table: &mut Table,
        allocator: &mut ObjectAllocator,
        ledger: &mut Ledger<'_>,
        pages: usize,
        kernel: &mut K,
    ) -> Result<usize, elastic::ElasticGrowError> {
        let previous = elastic::grow(
            table,
            allocator,
            ledger,
            self.token,
            self.arena,
            sel4::init_thread::slot::VSPACE.cap(),
            &mut self.region,
            pages,
            super::GrowthPlan::elastic(),
            kernel,
        )?;
        self.committed = self.region.pages();
        Ok(previous)
    }
}

#[cfg(slime_private_rollback)]
pub fn exercise_failure_rollback(
    allocator: &mut ObjectAllocator,
) -> Result<(), QualificationError> {
    use crate::object_allocator::elastic::inject;

    let mut buffer = PolicyBuffer::new();
    let mut instances = [Instance {
        identity: [0; 32],
        owner: None,
    }; MAX_HOLDERS];
    let subjects = [
        ("qualification-injected", ELASTIC),
        ("qualification-peer", ELASTIC),
    ];
    let mut ledger = admit(allocator, &mut buffer, &subjects, &mut instances)?;
    let mut table = Table::new();
    allocator.report_elastic_census("admitted");

    let mut peer = Holder::admit(
        allocator,
        &mut ledger,
        "qualification-peer",
        1,
        HOLDER_WINDOW_PAGES,
    )?;
    peer.grow(&mut table, allocator, &mut ledger, 8)
        .map_err(QualificationError::Growth)?;
    peer.write_pattern(0, peer.region.pages());

    let mut holder = Holder::admit(
        allocator,
        &mut ledger,
        "qualification-injected",
        0,
        HOLDER_WINDOW_PAGES,
    )?;
    holder
        .grow(&mut table, allocator, &mut ledger, 4)
        .map_err(QualificationError::Growth)?;
    holder.write_pattern(0, holder.region.pages());
    let sentinel_pages = holder.region.pages();
    let clean_charge = {
        let before = ledger.available().bytes;
        holder
            .grow(&mut table, allocator, &mut ledger, 4)
            .map_err(QualificationError::Growth)?;
        holder.write_pattern(sentinel_pages, holder.region.pages());
        before - ledger.available().bytes
    };
    let committed = holder.region.pages();
    sel4::debug_println!(
        "SLIME_MEM elastic baseline committed={} charged={} pool_bytes={} peer={}",
        committed,
        clean_charge,
        ledger.available().bytes,
        peer.region.pages(),
    );

    // Each stage fails after a different amount of the transaction exists, so
    // each needs a growth whose shape reaches that stage: a request that takes
    // one extent cannot fail on its second, and one inside a span that already
    // has a leaf table never maps one.
    let crossing = LARGE_FRAME_PAGES - committed + 1;
    let mut stages = 0;
    for stage in [
        "extent",
        "descriptors",
        "retype",
        "table-map",
        "frame-map",
        "near-limit-descriptors",
    ] {
        let pool_before = ledger.available().bytes;
        let ordinary_before = allocator.untyped_bytes_remaining();
        let tables_before = holder.region.leaf_tables();
        let outcome = match stage {
            // Fails the second of the three extents a span-crossing request
            // takes, so a partial acquisition is what has to be returned.
            "extent" => {
                inject::fail_extent(1);
                holder.grow(&mut table, allocator, &mut ledger, crossing)
            }
            "descriptors" => {
                inject::fail_descriptors();
                holder.grow(&mut table, allocator, &mut ledger, 4)
            }
            "retype" => {
                let mut kernel = FailingKernel::new(&holder.region, 4, Operation::Retype, 1);
                holder.grow_with(&mut table, allocator, &mut ledger, 4, &mut kernel)
            }
            // The first leaf table this holder needs is the one the crossing
            // page requires; refusing its map is refusing the whole span.
            "table-map" => {
                let mut kernel =
                    FailingKernel::new(&holder.region, crossing, Operation::MapLeaf, 0);
                holder.grow_with(&mut table, allocator, &mut ledger, crossing, &mut kernel)
            }
            // Fails the page *after* the new span's table was mapped, which is
            // the case whose table must stay charged and bound to that span.
            "frame-map" => {
                let mut kernel =
                    FailingKernel::new(&holder.region, crossing, Operation::MapFrame, crossing - 1);
                holder.grow_with(&mut table, allocator, &mut ledger, crossing, &mut kernel)
            }
            // Descriptor storage at its limit *and* unable to grow: the stock
            // and its funding both have to be gone for this to be the
            // near-limit case rather than one more growth of the pool.
            _ => {
                let reserved = allocator.reserve_stress_descriptors(1);
                let window = allocator.hold_metadata_window();
                let result = holder.grow(&mut table, allocator, &mut ledger, 4);
                allocator.restore_metadata_window(window);
                allocator.release_stress_descriptors();
                let _ = reserved;
                result
            }
        };
        let Err(error) = outcome else {
            return Err(QualificationError::Unmet("an injected stage did not fail"));
        };
        if inject::armed() {
            return Err(QualificationError::Unmet("an injection was never consumed"));
        }
        let intact =
            holder.pattern_holds(0, committed) && peer.pattern_holds(0, peer.region.pages());
        sel4::debug_println!(
            "SLIME_MEM elastic injected stage={} cause={} committed={} pool_before={} pool_after={} ordinary_before={} ordinary_after={} retained_tables={} sentinels={} quarantined={}",
            stage,
            error.cause(),
            holder.region.pages(),
            pool_before,
            ledger.available().bytes,
            ordinary_before,
            allocator.untyped_bytes_remaining(),
            holder.region.leaf_tables() - tables_before,
            u8::from(intact),
            u8::from(allocator.elastic_quarantined(holder.arena)),
        );
        if holder.region.pages() != committed || !intact {
            return Err(QualificationError::Unmet(
                "a failed growth changed committed state",
            ));
        }
        stages += 1;
    }

    // A retry after every injected failure must cost exactly what a clean
    // growth costs: nothing was charged twice and nothing leaked.
    let before = ledger.available().bytes;
    holder
        .grow(&mut table, allocator, &mut ledger, 4)
        .map_err(QualificationError::Growth)?;
    holder.write_pattern(committed, holder.region.pages());
    let retry_charge = before - ledger.available().bytes;
    sel4::debug_println!(
        "SLIME_MEM elastic retry stage=all committed={} charged={} clean={} doubled={}",
        holder.region.pages(),
        retry_charge,
        clean_charge,
        u8::from(retry_charge > clean_charge),
    );
    if retry_charge > clean_charge {
        return Err(QualificationError::Unmet("a retry charged twice"));
    }

    // A cleanup that does not complete keeps ownership, refuses the next
    // request, and returns the resources exactly once when it is retried.
    inject::fail_revoke();
    let mut kernel = FailingKernel::new(&holder.region, 4, Operation::MapFrame, 1);
    let Err(quarantine) = holder.grow_with(&mut table, allocator, &mut ledger, 4, &mut kernel)
    else {
        return Err(QualificationError::Unmet("the cleanup stage did not fail"));
    };
    let owned = allocator.elastic_quarantined(holder.arena);
    let refused = holder.grow(&mut table, allocator, &mut ledger, 1).is_err();
    let pool_held = ledger.available().bytes;
    let released = allocator.retry_elastic_quarantine(holder.arena);
    sel4::debug_println!(
        "SLIME_MEM elastic quarantine stage=cleanup cause={} owned={} refused={} released={} remaining={} pool_held={} pool_after={}",
        quarantine.cause(),
        u8::from(owned),
        u8::from(refused),
        u8::from(released.is_ok()),
        u8::from(allocator.elastic_quarantined(holder.arena)),
        pool_held,
        ledger.available().bytes,
    );
    if !owned || !refused || released.is_err() || allocator.elastic_quarantined(holder.arena) {
        return Err(QualificationError::Unmet(
            "a failed cleanup was not retryable exactly once",
        ));
    }
    if !holder.pattern_holds(0, committed) || !peer.pattern_holds(0, peer.region.pages()) {
        return Err(QualificationError::Unmet("quarantine damaged live pages"));
    }

    for holder in [&mut holder, &mut peer] {
        holder.retire(&mut table, allocator, &mut ledger)?;
    }

    // Each acquisition failure needs a fresh incarnation: a quarantined
    // incarnation cannot grow again even after its backing has been returned.
    let mut peer = Holder::admit(
        allocator,
        &mut ledger,
        "qualification-peer",
        1,
        HOLDER_WINDOW_PAGES,
    )?;
    peer.grow(&mut table, allocator, &mut ledger, 8)
        .map_err(QualificationError::Growth)?;
    peer.write_pattern(0, 8);
    for stage in ["extent-revoke", "descriptors-revoke"] {
        let mut holder = Holder::admit(
            allocator,
            &mut ledger,
            "qualification-injected",
            0,
            HOLDER_WINDOW_PAGES,
        )?;
        let common_baseline = ledger.common_held();
        holder
            .grow(&mut table, allocator, &mut ledger, 4)
            .map_err(QualificationError::Growth)?;
        holder.write_pattern(0, 4);
        let common_before = ledger.common_held();
        let pages_before = table.total_pages();
        if stage == "extent-revoke" {
            inject::fail_extent(1);
        } else {
            inject::fail_descriptors();
        }
        inject::fail_revoke();
        let delta = LARGE_FRAME_PAGES - holder.region.pages() + 1;
        let outcome = holder.grow(&mut table, allocator, &mut ledger, delta);
        let retained = allocator.elastic_quarantine_resources(holder.arena);
        if !matches!(outcome, Err(elastic::ElasticGrowError::Quarantined { .. }))
            || inject::armed()
            || !allocator.elastic_quarantined(holder.arena)
            || retained.bytes == 0
            || ledger.common_held().subtract(common_before)? != retained
            || holder.region.pages() != 4
            || ledger.pages(holder.token)? != 4
            || peer.region.pages() != 8
            || ledger.pages(peer.token)? != 8
            || table.total_pages() != pages_before
            || !holder.pattern_holds(0, 4)
            || !peer.pattern_holds(0, 8)
        {
            return Err(QualificationError::Unmet(
                "acquisition quarantine lost ownership or committed state",
            ));
        }
        let held = ledger.available();
        let common_held = ledger.common_held();
        let root_owned = ledger.root_owned();
        allocator.retry_elastic_quarantine(holder.arena)?;
        if allocator.elastic_quarantined(holder.arena)
            || ledger.available() != held
            || ledger.common_held() != common_held
            || ledger.root_owned() != root_owned
        {
            return Err(QualificationError::Unmet(
                "quarantine retry refunded ownership",
            ));
        }
        let inventory = allocator.elastic_inventory();
        let reused = allocator.extents_reused();
        let refused = holder.grow(&mut table, allocator, &mut ledger, 1);
        if refused != Err(elastic::ElasticGrowError::Policy(ledger::Error::Cleanup))
            || allocator.elastic_inventory() != inventory
            || allocator.extents_reused() != reused
            || ledger.available() != held
            || ledger.common_held() != common_held
            || ledger.root_owned() != root_owned
            || holder.region.pages() != 4
            || ledger.pages(holder.token)? != 4
            || peer.region.pages() != 8
            || ledger.pages(peer.token)? != 8
            || table.total_pages() != pages_before
            || !holder.pattern_holds(0, 4)
            || !peer.pattern_holds(0, 8)
        {
            return Err(QualificationError::Unmet(
                "quarantined incarnation reached acquisition after retry",
            ));
        }
        let refund = common_held.subtract(common_baseline)?;
        if holder.retire(&mut table, allocator, &mut ledger)? != 4
            || ledger.available() != held.checked_add(refund)?
            || ledger.common_held() != common_baseline
            || ledger.root_owned() != root_owned
        {
            return Err(QualificationError::Unmet(
                "retirement refunded the wrong ownership",
            ));
        }
        let refunded = ledger.available();
        if ledger.retire(holder.token, true) != Err(ledger::Error::Incarnation)
            || ledger.available() != refunded
            || ledger.root_owned() != root_owned
            || !peer.pattern_holds(0, 8)
        {
            return Err(QualificationError::Unmet(
                "retirement refunded twice or damaged peer",
            ));
        }
        sel4::debug_println!(
            "SLIME_MEM elastic acquisition_quarantine stage={} committed=4 peer=8 sentinels=1 retained={} charged={} retry_refund=0 pre_acquire_refused=1 refund={} pool_held={} pool_after={} root_before={} root_after={} retired_once=1",
            stage,
            retained.bytes,
            common_held.subtract(common_before)?.bytes,
            refund.bytes,
            held.bytes,
            refunded.bytes,
            root_owned.bytes,
            ledger.root_owned().bytes,
        );
    }
    peer.retire(&mut table, allocator, &mut ledger)?;
    allocator.report_elastic_census("retired");
    sel4::debug_println!(
        "SLIME_MEM elastic complete case=failure-rollback stages={} granted={} reclaimed={}",
        stages + 1,
        table.grown_pages(),
        table.reclaimed_pages(),
    );
    Ok(())
}

#[cfg(slime_private_conservation)]
pub fn exercise_cross_holder_conservation(
    allocator: &mut ObjectAllocator,
) -> Result<(), QualificationError> {
    let mut buffer = PolicyBuffer::new();
    let mut instances = [Instance {
        identity: [0; 32],
        owner: None,
    }; MAX_HOLDERS];
    let subjects = [
        ("qualification-first", ELASTIC),
        ("qualification-second", ELASTIC),
        ("qualification-peer", ELASTIC),
        ("qualification-stuck", ELASTIC),
    ];
    let mut ledger = admit(allocator, &mut buffer, &subjects, &mut instances)?;
    let mut table = Table::new();
    allocator.report_elastic_census("admitted");

    let baseline_pool = ledger.available().bytes;
    // Ownership, not one owner: capacity moves between the ordinary tails, the
    // retained alignment prefixes and the returned extents as holders come and
    // go, so only their sum is a baseline a workload can be reconciled against.
    let baseline_owned = unallocated_bytes(allocator);
    let baseline_slots = allocator.free_slots();

    // A peer that keeps its pattern across everything the others do.
    let mut peer = Holder::admit(
        allocator,
        &mut ledger,
        "qualification-peer",
        2,
        HOLDER_WINDOW_PAGES,
    )?;
    peer.grow(&mut table, allocator, &mut ledger, LARGE_FRAME_PAGES)
        .map_err(QualificationError::Growth)?;
    peer.write_pattern(0, peer.region.pages());

    let workload = LARGE_FRAME_PAGES;
    let mut first = Holder::admit(
        allocator,
        &mut ledger,
        "qualification-first",
        0,
        HOLDER_WINDOW_PAGES,
    )?;
    first
        .grow(&mut table, allocator, &mut ledger, workload)
        .map_err(QualificationError::Growth)?;
    first.write_pattern(0, first.region.pages());
    sel4::debug_println!(
        "SLIME_MEM elastic holder subject={} committed={} pattern={} pool_bytes={}",
        first.name,
        first.region.pages(),
        u8::from(first.pattern_holds(0, first.region.pages())),
        ledger.available().bytes,
    );

    let held = ledger.available().bytes;
    let returned = first.retire(&mut table, allocator, &mut ledger)?;
    sel4::debug_println!(
        "SLIME_MEM elastic retire subject={} revoked=1 returned_pages={} pool_before={} pool_after={} reusable={}",
        first.name,
        returned,
        held,
        ledger.available().bytes,
        allocator.reusable_private_extent_bytes(),
    );
    if ledger.available().bytes <= held {
        return Err(QualificationError::Unmet("a revoke returned no capacity"));
    }

    // A different holder takes the recovered capacity. Its pages must be
    // zero, not the previous holder's pattern.
    let mut second = Holder::admit(
        allocator,
        &mut ledger,
        "qualification-second",
        1,
        HOLDER_WINDOW_PAGES,
    )?;
    let reused_before = allocator.extents_reused();
    second
        .grow(&mut table, allocator, &mut ledger, workload)
        .map_err(QualificationError::Growth)?;
    let zeroed = second.zeroed(0, second.region.pages());
    second.write_pattern(0, second.region.pages());
    sel4::debug_println!(
        "SLIME_MEM elastic reuse subject={} committed={} reused_extents={} zeroed={} peer_pattern={}",
        second.name,
        second.region.pages(),
        allocator.extents_reused() - reused_before,
        u8::from(zeroed),
        u8::from(peer.pattern_holds(0, peer.region.pages())),
    );
    if !zeroed {
        return Err(QualificationError::Unmet("recovered pages were not zeroed"));
    }

    // A reclamation whose revoke fails keeps the original ownership, and the
    // capacity is returned only when the retry completes.
    let mut stuck = Holder::admit(
        allocator,
        &mut ledger,
        "qualification-stuck",
        3,
        HOLDER_WINDOW_PAGES,
    )?;
    stuck
        .grow(&mut table, allocator, &mut ledger, 8)
        .map_err(QualificationError::Growth)?;
    stuck.write_pattern(0, stuck.region.pages());
    crate::object_allocator::elastic::arm_arena_revoke_failure();
    let before = ledger.available().bytes;
    let failed = stuck.retire(&mut table, allocator, &mut ledger);
    let held_after_failure = ledger.available().bytes;
    let retried = stuck.retire(&mut table, allocator, &mut ledger);
    sel4::debug_println!(
        "SLIME_MEM elastic revoke_failure subject={} first_attempt={} retained={} retry={} pool_before={} pool_held={} pool_after={}",
        stuck.name,
        u8::from(failed.is_ok()),
        u8::from(held_after_failure == before),
        u8::from(retried.is_ok()),
        before,
        held_after_failure,
        ledger.available().bytes,
    );
    if failed.is_ok() || held_after_failure != before {
        return Err(QualificationError::Unmet(
            "a failed revoke advertised capacity",
        ));
    }

    // Everything returns, and the machine's own accounting comes back to its
    // pre-workload baseline without drift.
    for holder in [&mut second, &mut peer] {
        holder.retire(&mut table, allocator, &mut ledger)?;
    }
    let final_owned = unallocated_bytes(allocator);
    allocator.report_elastic_census("retired");
    sel4::debug_println!(
        "SLIME_MEM elastic baseline phase=final owned_before={} owned_after={} slots_before={} slots_after={} pool_before={} pool_after={} granted={} reclaimed={}",
        baseline_owned,
        final_owned,
        baseline_slots,
        allocator.free_slots(),
        baseline_pool,
        ledger.available().bytes,
        table.grown_pages(),
        table.reclaimed_pages(),
    );
    if final_owned < baseline_owned {
        return Err(QualificationError::Unmet("owned capacity drifted downward"));
    }
    sel4::debug_println!(
        "SLIME_MEM elastic complete case=cross-holder-conservation holders={} granted={} reclaimed={}",
        subjects.len(),
        table.grown_pages(),
        table.reclaimed_pages(),
    );
    Ok(())
}
