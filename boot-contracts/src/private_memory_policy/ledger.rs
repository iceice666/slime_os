//! Pure admission and ownership accounting. The allocator must supply a complete
//! placement plan; these transitions neither allocate objects nor prove a kernel
//! operation succeeded. A transaction remains charged until its owner settles it.

use super::{FIXED, MAX_ENTITLEMENTS, MAX_PAGES, MAX_SUBJECTS, PAGE_BYTES, Policy};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Resources {
    pub bytes: u64,
    pub slots: u64,
    pub descriptors: u64,
    pub extents: u64,
    pub tables: u64,
}

impl Resources {
    pub const ZERO: Self = Self {
        bytes: 0,
        slots: 0,
        descriptors: 0,
        extents: 0,
        tables: 0,
    };
    pub fn checked_add(self, other: Self) -> Result<Self, Error> {
        Ok(Self {
            bytes: self.bytes.checked_add(other.bytes).ok_or(Error::Overflow)?,
            slots: self.slots.checked_add(other.slots).ok_or(Error::Overflow)?,
            descriptors: self
                .descriptors
                .checked_add(other.descriptors)
                .ok_or(Error::Overflow)?,
            extents: self
                .extents
                .checked_add(other.extents)
                .ok_or(Error::Overflow)?,
            tables: self
                .tables
                .checked_add(other.tables)
                .ok_or(Error::Overflow)?,
        })
    }
    pub fn subtract(self, other: Self) -> Result<Self, Error> {
        Ok(Self {
            bytes: self
                .bytes
                .checked_sub(other.bytes)
                .ok_or(Error::Unavailable)?,
            slots: self
                .slots
                .checked_sub(other.slots)
                .ok_or(Error::Unavailable)?,
            descriptors: self
                .descriptors
                .checked_sub(other.descriptors)
                .ok_or(Error::Unavailable)?,
            extents: self
                .extents
                .checked_sub(other.extents)
                .ok_or(Error::Unavailable)?,
            tables: self
                .tables
                .checked_sub(other.tables)
                .ok_or(Error::Unavailable)?,
        })
    }
    fn page_share(self, pages: u64, total_pages: u64) -> Self {
        if total_pages == 0 {
            return Self::ZERO;
        }
        Self {
            bytes: self.bytes / total_pages * pages,
            slots: self.slots / total_pages * pages,
            descriptors: self.descriptors / total_pages * pages,
            extents: self.extents / total_pages * pages,
            tables: self.tables / total_pages * pages,
        }
    }
    fn minimum(self, other: Self) -> Self {
        Self {
            bytes: self.bytes.min(other.bytes),
            slots: self.slots.min(other.slots),
            descriptors: self.descriptors.min(other.descriptors),
            extents: self.extents.min(other.extents),
            tables: self.tables.min(other.tables),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Overflow,
    Inventory,
    Placement,
    Unavailable,
    Guarantee,
    Denied,
    Maximum,
    Incarnation,
    Transaction,
    Cleanup,
}

/// Ordinary physical ownership classes are disjoint; boot-excluded bytes are
/// deliberately not an input and cannot be subtracted a second time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Class {
    OrdinaryTail,
    PreservedLeaf,
    Reusable,
    StaticTasks,
    KernelObjects,
    Metadata,
    PageTables,
    SharedDma,
    Guaranteed,
    Elastic,
    Quarantined,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Range {
    pub start: u64,
    pub bytes: u64,
    pub class: Class,
}

impl Range {
    fn end(self) -> Result<u64, Error> {
        self.start.checked_add(self.bytes).ok_or(Error::Overflow)
    }
    fn free(self) -> bool {
        matches!(
            self.class,
            Class::OrdinaryTail | Class::PreservedLeaf | Class::Reusable
        )
    }
}

/// Check that a disjoint partition exactly covers the admitted ordinary ranges.
/// Device ranges must never be supplied as ordinary inventory by the caller.
pub fn inventory_available(ordinary: &[Range], partition: &[Range]) -> Result<u64, Error> {
    fn disjoint(ranges: &[Range]) -> Result<(), Error> {
        for (index, range) in ranges.iter().enumerate() {
            let end = range.end()?;
            if range.bytes == 0
                || ranges[..index]
                    .iter()
                    .any(|other| range.start < other.end().unwrap_or(u64::MAX) && other.start < end)
            {
                return Err(Error::Inventory);
            }
        }
        Ok(())
    }
    disjoint(ordinary)?;
    disjoint(partition)?;
    let mut total = 0u64;
    let mut available = 0u64;
    for range in partition {
        if !ordinary
            .iter()
            .any(|parent| range.start >= parent.start && range.end().ok() <= parent.end().ok())
        {
            return Err(Error::Inventory);
        }
        total = total.checked_add(range.bytes).ok_or(Error::Overflow)?;
        if range.free() {
            available = available.checked_add(range.bytes).ok_or(Error::Overflow)?;
        }
    }
    let admitted = ordinary.iter().try_fold(0u64, |sum, range| {
        sum.checked_add(range.bytes).ok_or(Error::Overflow)
    })?;
    if total != admitted {
        return Err(Error::Inventory);
    }
    Ok(available)
}

/// An allocator's exact physical placements for one request. Sizes are powers
/// of two and starts must satisfy seL4 retype alignment. No best-case byte sum
/// substitutes for fitting every extent in the current free partition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Placement {
    pub start: u64,
    pub size_bits: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Plan {
    resources: Resources,
}

impl Plan {
    pub fn validate(
        free: &[Range],
        placements: &[Placement],
        resources: Resources,
    ) -> Result<Self, Error> {
        let mut bytes = 0u64;
        for (index, placement) in placements.iter().enumerate() {
            let size = 1u64
                .checked_shl(placement.size_bits.into())
                .ok_or(Error::Overflow)?;
            let end = placement.start.checked_add(size).ok_or(Error::Overflow)?;
            if !placement.start.is_multiple_of(size)
                || !free.iter().any(|range| {
                    range.free()
                        && placement.start >= range.start
                        && end <= range.end().unwrap_or(0)
                })
                || placements[..index].iter().any(|prior| {
                    let prior_end = prior.start + (1u64 << prior.size_bits);
                    placement.start < prior_end && prior.start < end
                })
            {
                return Err(Error::Placement);
            }
            bytes = bytes.checked_add(size).ok_or(Error::Overflow)?;
        }
        if bytes != resources.bytes {
            return Err(Error::Placement);
        }
        Ok(Self { resources })
    }
    pub fn resources(self) -> Resources {
        self.resources
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Member {
    epoch: u64,
    live: bool,
    quarantined: bool,
    pages: u64,
    held_payload_pages: u64,
    guarantee_pages: u64,
    guaranteed: Resources,
    elastic: Resources,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Incarnation {
    subject: usize,
    epoch: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Pending {
    member: Incarnation,
    pages: u64,
    guarantee_pages: u64,
    guaranteed: Resources,
    elastic: Resources,
}

/// One service-order transaction at a time, matching root's serialized grow
/// boundary. Incarnation tokens prevent a late cleanup from charging a restart.
pub struct Ledger<'a> {
    policy: Policy<'a>,
    free: Resources,
    pool_pages: u64,
    guarantee_total: [Resources; MAX_ENTITLEMENTS],
    guarantee_free: [Resources; MAX_ENTITLEMENTS],
    members: [Member; MAX_SUBJECTS],
    pending: Option<Pending>,
}

impl<'a> Ledger<'a> {
    /// Guarantees use a conservative per-page resource envelope. Every page's
    /// complete cost must fit its share, including tables and extent records;
    /// the allocator certifies physical fit before publication. This model does
    /// not infer a worst-case mapping cost from a best-case large-frame plan.
    pub fn admit(
        policy: Policy<'a>,
        instances: &[super::Instance],
        available: Resources,
        guarantees: &[Resources],
    ) -> Result<Self, Error> {
        policy
            .validate_instances(instances)
            .map_err(|_| Error::Denied)?;
        if guarantees.len() != policy.entitlement_count() {
            return Err(Error::Guarantee);
        }
        let mut free = available.subtract(policy.reserve())?;
        let pool_pages = (free.bytes / PAGE_BYTES).min(MAX_PAGES);
        let mut total = [Resources::ZERO; MAX_ENTITLEMENTS];
        for (index, resources) in guarantees.iter().copied().enumerate() {
            let entitlement = policy.entitlement(index).unwrap();
            if resources.bytes
                < entitlement
                    .guarantee_pages
                    .checked_mul(PAGE_BYTES)
                    .ok_or(Error::Overflow)?
                || resources.slots < entitlement.guarantee_pages
                || resources.descriptors < entitlement.guarantee_pages
                || resources.extents < entitlement.guarantee_pages
                || resources.tables < entitlement.guarantee_pages
                || (entitlement.guarantee_pages == 0 && resources != Resources::ZERO)
            {
                return Err(Error::Guarantee);
            }
            free = free.subtract(resources)?;
            total[index] = resources;
        }
        Ok(Self {
            policy,
            free,
            pool_pages,
            guarantee_total: total,
            guarantee_free: total,
            members: [Member::default(); MAX_SUBJECTS],
            pending: None,
        })
    }

    pub fn available(&self) -> Resources {
        self.free
    }
    pub fn guaranteed_available(&self, identity: &[u8; 32]) -> Result<Resources, Error> {
        Ok(self.guarantee_free[self
            .policy
            .entitlement_index(identity)
            .ok_or(Error::Denied)?])
    }
    pub fn bind(&mut self, subject: &[u8; 32]) -> Result<Incarnation, Error> {
        let index = self.policy.subject_index(subject).ok_or(Error::Denied)?;
        let member = &mut self.members[index];
        if member.live {
            return Err(Error::Incarnation);
        }
        member.epoch = member.epoch.checked_add(1).ok_or(Error::Overflow)?;
        member.live = true;
        Ok(Incarnation {
            subject: index,
            epoch: member.epoch,
        })
    }
    fn member(&self, token: Incarnation) -> Result<Member, Error> {
        self.members
            .get(token.subject)
            .copied()
            .filter(|member| member.live && member.epoch == token.epoch)
            .ok_or(Error::Incarnation)
    }
    fn entitlement(&self, token: Incarnation) -> usize {
        self.policy
            .entitlement_index(&self.policy.subject(token.subject).unwrap().entitlement)
            .unwrap()
    }
    pub fn pages(&self, token: Incarnation) -> Result<u64, Error> {
        Ok(self.member(token)?.pages)
    }

    pub fn begin(&mut self, token: Incarnation, pages: u64, plan: Plan) -> Result<(), Error> {
        if self.pending.is_some() {
            return Err(Error::Transaction);
        }
        let member = self.member(token)?;
        if member.quarantined {
            return Err(Error::Cleanup);
        }
        let subject = self.policy.subject(token.subject).unwrap();
        let index = self.entitlement(token);
        let entitlement = self.policy.entitlement(index).unwrap();
        let requested = member
            .pages
            .checked_add(member.held_payload_pages)
            .and_then(|held| held.checked_add(pages))
            .ok_or(Error::Overflow)?;
        let subject_limit = if subject.maximum_mode == FIXED {
            subject.maximum_pages
        } else {
            self.pool_pages
        };
        let entitlement_limit = if entitlement.maximum_mode == FIXED {
            entitlement.maximum_pages
        } else {
            self.pool_pages
        };
        let mut total = pages;
        let mut redeemed = 0u64;
        for position in 0..self.policy.subject_count() {
            if self.policy.subject(position).unwrap().entitlement == entitlement.identity {
                total = total
                    .checked_add(self.members[position].pages)
                    .and_then(|held| held.checked_add(self.members[position].held_payload_pages))
                    .ok_or(Error::Overflow)?;
                redeemed = redeemed
                    .checked_add(self.members[position].guarantee_pages)
                    .ok_or(Error::Overflow)?;
            }
        }
        if requested > subject_limit || total > entitlement_limit {
            return Err(Error::Maximum);
        }
        if plan.resources.bytes < pages.checked_mul(PAGE_BYTES).ok_or(Error::Overflow)? {
            return Err(Error::Placement);
        }
        if pages == 0 && plan.resources != Resources::ZERO {
            return Err(Error::Placement);
        }
        let guarantee_pages = pages.min(entitlement.guarantee_pages.saturating_sub(redeemed));
        let guaranteed = plan
            .resources
            .minimum(
                self.guarantee_total[index]
                    .page_share(guarantee_pages, entitlement.guarantee_pages),
            )
            .minimum(self.guarantee_free[index]);
        let elastic = plan.resources.subtract(guaranteed)?;
        let free = self.free.subtract(elastic)?;
        let guarantee_free = self.guarantee_free[index].subtract(guaranteed)?;
        // Preflight the later settlement arithmetic before reserving resources.
        member.guaranteed.checked_add(guaranteed)?;
        member.elastic.checked_add(elastic)?;
        self.free = free;
        self.guarantee_free[index] = guarantee_free;
        self.pending = Some(Pending {
            member: token,
            pages,
            guarantee_pages,
            guaranteed,
            elastic,
        });
        Ok(())
    }

    pub fn commit(&mut self, token: Incarnation) -> Result<(), Error> {
        let pending = self
            .pending
            .filter(|pending| pending.member == token)
            .ok_or(Error::Transaction)?;
        let old = self.member(token)?;
        let guaranteed = old.guaranteed.checked_add(pending.guaranteed)?;
        let elastic = old.elastic.checked_add(pending.elastic)?;
        let pages = old
            .pages
            .checked_add(pending.pages)
            .ok_or(Error::Overflow)?;
        let guarantee_pages = old
            .guarantee_pages
            .checked_add(pending.guarantee_pages)
            .ok_or(Error::Overflow)?;
        let member = &mut self.members[token.subject];
        member.guaranteed = guaranteed;
        member.elastic = elastic;
        member.pages = pages;
        member.guarantee_pages = guarantee_pages;
        self.pending = None;
        Ok(())
    }

    /// Successful rollback may retain only elastic-funded table resources.
    /// An unrefunded guarantee remains pending until cleanup succeeds or is
    /// explicitly quarantined; failure never advertises that guarantee as free.
    pub fn abort(
        &mut self,
        token: Incarnation,
        retained: Resources,
        cleanup_succeeded: bool,
    ) -> Result<(), Error> {
        let pending = self
            .pending
            .filter(|pending| pending.member == token)
            .ok_or(Error::Transaction)?;
        let old = self.member(token)?;
        let (kept_guarantee, kept_elastic) = if cleanup_succeeded {
            if retained.extents != 0
                || retained.bytes
                    != retained
                        .tables
                        .checked_mul(PAGE_BYTES)
                        .ok_or(Error::Overflow)?
                || retained.slots > retained.tables
                || retained.descriptors > retained.tables
            {
                return Err(Error::Cleanup);
            }
            let table_bound = old
                .pages
                .checked_add(pending.pages)
                .ok_or(Error::Overflow)?;
            let retained_tables = old
                .guaranteed
                .tables
                .checked_add(old.elastic.tables)
                .and_then(|tables| tables.checked_add(retained.tables))
                .ok_or(Error::Overflow)?;
            if retained_tables > table_bound {
                return Err(Error::Cleanup);
            }
            pending
                .elastic
                .subtract(retained)
                .map_err(|_| Error::Cleanup)?;
            (Resources::ZERO, retained)
        } else {
            (pending.guaranteed, pending.elastic)
        };
        let index = self.entitlement(token);
        let guarantee_free =
            self.guarantee_free[index].checked_add(pending.guaranteed.subtract(kept_guarantee)?)?;
        let free = self
            .free
            .checked_add(pending.elastic.subtract(kept_elastic)?)?;
        let guaranteed = old.guaranteed.checked_add(kept_guarantee)?;
        let elastic = old.elastic.checked_add(kept_elastic)?;
        let held_payload_pages = old
            .held_payload_pages
            .checked_add(if cleanup_succeeded { 0 } else { pending.pages })
            .ok_or(Error::Overflow)?;
        let guarantee_pages = old
            .guarantee_pages
            .checked_add(if cleanup_succeeded {
                0
            } else {
                pending.guarantee_pages
            })
            .ok_or(Error::Overflow)?;
        self.guarantee_free[index] = guarantee_free;
        self.free = free;
        self.members[token.subject].guaranteed = guaranteed;
        self.members[token.subject].elastic = elastic;
        self.members[token.subject].quarantined = !cleanup_succeeded;
        self.members[token.subject].held_payload_pages = held_payload_pages;
        self.members[token.subject].guarantee_pages = guarantee_pages;
        self.pending = None;
        Ok(())
    }

    pub fn retire(&mut self, token: Incarnation, revoked: bool) -> Result<(), Error> {
        let old = self.member(token)?;
        if self.pending.is_some_and(|pending| pending.member == token) {
            return Err(Error::Transaction);
        }
        if !revoked {
            self.members[token.subject].quarantined = true;
            return Err(Error::Cleanup);
        }
        let index = self.entitlement(token);
        let guaranteed = self.guarantee_free[index].checked_add(old.guaranteed)?;
        self.guarantee_total[index].subtract(guaranteed)?;
        let free = self.free.checked_add(old.elastic)?;
        self.guarantee_free[index] = guaranteed;
        self.free = free;
        self.members[token.subject] = Member {
            epoch: old.epoch,
            ..Member::default()
        };
        Ok(())
    }
}
