//! Pre-effect funding for a serialized adaptive allocation attempt.
//!
//! Root source adoption is permanent even when the triggering holder rolls
//! back. Its allowance is independent of payload funding and cannot consume
//! either outstanding payload funding or the operational floor.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Refusal {
    Overflow,
    Reserve { required: u64, available: u64 },
    Root { required: u64, available: u64 },
    Payload,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Funding {
    floor: u64,
    payload: u64,
    root_remaining: u64,
    root_owned: u64,
}

impl Funding {
    /// Construction reserves the complete rounded arena before metadata can
    /// adopt any source. Metadata may consume only the remaining inventory.
    fn construction(inventory: u64, floor: u64, payload: u64) -> Result<Self, Refusal> {
        let mut funding = Self::new(inventory, floor, payload, 0)?;
        funding.root_remaining = inventory - floor - payload;
        Ok(funding)
    }

    pub fn new(
        inventory: u64,
        floor: u64,
        payload: u64,
        root_allowance: u64,
    ) -> Result<Self, Refusal> {
        let required = floor.checked_add(payload).ok_or(Refusal::Overflow)?;
        if required > inventory {
            return Err(Refusal::Reserve {
                required,
                available: inventory,
            });
        }
        Ok(Self {
            floor,
            payload,
            root_remaining: root_allowance,
            root_owned: 0,
        })
    }

    pub fn check_payload(&self, inventory: u64, bytes: u64) -> Result<(), Refusal> {
        if bytes > self.payload {
            return Err(Refusal::Payload);
        }
        self.check_floor(inventory, bytes)
    }

    pub fn take_payload(&mut self, bytes: u64) -> Result<(), Refusal> {
        self.payload = self.payload.checked_sub(bytes).ok_or(Refusal::Payload)?;
        Ok(())
    }

    pub fn check_floor(&self, inventory: u64, bytes: u64) -> Result<(), Refusal> {
        let required = self.floor.checked_add(bytes).ok_or(Refusal::Overflow)?;
        if required > inventory {
            return Err(Refusal::Reserve {
                required,
                available: inventory,
            });
        }
        Ok(())
    }

    pub fn check_root(&self, inventory: u64, bytes: u64) -> Result<(), Refusal> {
        if bytes > self.root_remaining {
            return Err(Refusal::Root {
                required: bytes,
                available: self.root_remaining,
            });
        }
        self.root_owned
            .checked_add(bytes)
            .ok_or(Refusal::Overflow)?;
        let protected = self.payload.checked_add(bytes).ok_or(Refusal::Overflow)?;
        self.check_floor(inventory, protected)
    }

    /// Publish only after the checked ownership transfer succeeded. No nested
    /// allocation may occur between the check and this publication.
    pub fn take_root(&mut self, bytes: u64) -> Result<(), Refusal> {
        let remaining = self
            .root_remaining
            .checked_sub(bytes)
            .ok_or(Refusal::Root {
                required: bytes,
                available: self.root_remaining,
            })?;
        let owned = self
            .root_owned
            .checked_add(bytes)
            .ok_or(Refusal::Overflow)?;
        self.root_remaining = remaining;
        self.root_owned = owned;
        Ok(())
    }

    pub const fn root_owned(&self) -> u64 {
        self.root_owned
    }
}

impl From<Refusal> for super::AllocError {
    fn from(value: Refusal) -> Self {
        let (resource, required, available) = match value {
            Refusal::Overflow => ("arithmetic", 0, 0),
            Refusal::Reserve {
                required,
                available,
            } => ("operational-reserve", required, available),
            Refusal::Root {
                required,
                available,
            } => ("root-funding", required, available),
            Refusal::Payload => ("payload-funding", 0, 0),
        };
        Self::AdaptiveFunding {
            resource,
            required,
            available,
        }
    }
}

/// Snapshot of a serialized construction attempt, including failed attempts.
#[derive(Clone, Copy)]
pub(crate) struct ConstructionFunding {
    inventory: u64,
    reserve: u64,
    system: u64,
}

impl super::ObjectAllocator {
    /// Materialize the operational count floor after guarantees and before the
    /// ledger's residual inventory snapshot. Bytes are configured separately.
    /// Translation tables use the bounded construction arena, not a separate
    /// table pool; these counts do not guarantee arbitrary construction shapes.
    pub fn reserve_operational_resources(
        &mut self,
        resources: boot_contracts::private_memory_policy::ledger::Resources,
    ) -> Result<(), super::AllocError> {
        if self.adaptive_funding.is_some() || self.operational_lent {
            return Err(Refusal::Payload.into());
        }
        let slots = usize::try_from(resources.slots).map_err(|_| Refusal::Overflow)?;
        let descriptors = usize::try_from(resources.descriptors).map_err(|_| Refusal::Overflow)?;
        let extents = usize::try_from(resources.extents).map_err(|_| Refusal::Overflow)?;
        // Existing floors remain protected during growth and on failure.
        // Reconfiguration conservatively funds a fresh floor before replacing it.
        self.ensure_root_slots(slots)?;
        self.ensure_allocation_descriptors(descriptors)?;
        self.ensure_extent_descriptors(extents)?;
        self.operational_resources = boot_contracts::private_memory_policy::ledger::Resources {
            slots: resources.slots,
            descriptors: resources.descriptors,
            extents: resources.extents,
            ..boot_contracts::private_memory_policy::ledger::Resources::ZERO
        };
        Ok(())
    }

    /// The permanent floor is withheld from actual availability, never added
    /// back as fictional free capacity after construction consumes its loan.
    pub(super) fn operational_floor(&self, count: u64) -> usize {
        if self.operational_lent {
            0
        } else {
            count as usize
        }
    }

    /// Enable only after adaptive admission has funded the full reserve.
    /// Fixed-v1 allocators leave this disabled.
    pub fn enable_adaptive_reserve(&mut self, reserve: u64) {
        self.adaptive_reserve = Some(reserve);
    }

    pub(crate) fn begin_construction_funding(
        &mut self,
        arena_bits: usize,
        operational: bool,
    ) -> Result<Option<ConstructionFunding>, super::AllocError> {
        let Some(reserve) = self.adaptive_reserve else {
            return Ok(None);
        };
        let payload = u32::try_from(arena_bits)
            .ok()
            .and_then(|bits| 1u64.checked_shl(bits))
            .ok_or(Refusal::Overflow)?;
        let inventory = self.elastic_inventory().bytes;
        let floor = if operational { 0 } else { reserve };
        let funding = Funding::construction(inventory, floor, payload)?;
        if self.adaptive_funding.is_some() {
            return Err(Refusal::Payload.into());
        }
        let snapshot = ConstructionFunding {
            inventory,
            reserve,
            system: self.system_backing_bytes(),
        };
        self.adaptive_funding = Some(funding);
        self.operational_lent = operational;
        Ok(Some(snapshot))
    }

    /// Always finish, even when rollback retains an arena or adopted source.
    /// Ownership remains in the allocator's independent system/root census.
    pub(crate) fn finish_construction_funding(
        &mut self,
        snapshot: Option<ConstructionFunding>,
        success: bool,
    ) {
        self.operational_lent = false;
        let Some(snapshot) = snapshot else {
            return;
        };
        let root = self.finish_adaptive_funding();
        let inventory = self.elastic_inventory().bytes;
        let consumed = snapshot.inventory.saturating_sub(inventory);
        let spent = snapshot
            .reserve
            .saturating_sub(inventory)
            .saturating_sub(snapshot.reserve.saturating_sub(snapshot.inventory));
        let system = self.system_backing_bytes().saturating_sub(snapshot.system);
        sel4::debug_println!(
            "SLIME_MEM construction funding success={} before={} after={} consumed={} system={} root={} reserve={} reserve_spent={}",
            success as u8,
            snapshot.inventory,
            inventory,
            consumed,
            system,
            root,
            snapshot.reserve,
            spent,
        );
    }

    pub(crate) fn report_construction_cleanup(&self, before: u64, system_before: u64) {
        if let Some(reserve) = self.adaptive_reserve {
            let after = self.elastic_inventory().bytes;
            sel4::debug_println!(
                "SLIME_MEM construction cleanup before={} after={} returned={} system_returned={} reserve={}",
                before,
                after,
                after.saturating_sub(before),
                system_before.saturating_sub(self.system_backing_bytes()),
                reserve,
            );
        }
    }

    /// Rounded backing remains charged until the complete arena cleanup
    /// publishes it common-free, including partially revoked extents.
    pub fn system_backing_bytes(&self) -> u64 {
        self.extents
            .iter()
            .flatten()
            .filter(|extent| {
                extent.active
                    && !extent.split
                    && extent.origin == super::guarantee_vault::RESERVATION_NONE
                    && matches!(
                        extent.kind,
                        super::ExtentKind::Static | super::ExtentKind::MappingTables
                    )
            })
            .map(|extent| 1u64 << extent.size_bits)
            .sum::<u64>()
            + self.retained_shared_bytes() as u64
            + self.global_object_backing_bytes
    }

    pub(crate) fn begin_adaptive_funding(
        &mut self,
        floor: u64,
        payload: u64,
        root_allowance: u64,
    ) -> Result<(), super::AllocError> {
        if self.adaptive_funding.is_some() {
            return Err(Refusal::Payload.into());
        }
        self.adaptive_funding = Some(Funding::new(
            self.elastic_inventory().bytes,
            floor,
            payload,
            root_allowance,
        )?);
        Ok(())
    }

    pub(crate) fn finish_adaptive_funding(&mut self) -> u64 {
        self.operational_lent = false;
        self.adaptive_funding
            .take()
            .map_or(0, |funding| funding.root_owned())
    }

    pub(super) fn check_common_funding(&self, bytes: usize) -> Result<(), super::AllocError> {
        if let Some(funding) = self.adaptive_funding {
            funding.check_payload(self.elastic_inventory().bytes, bytes as u64)?;
        } else if let Some(floor) = self.adaptive_reserve {
            Funding::new(self.elastic_inventory().bytes, floor, bytes as u64, 0)?;
        }
        Ok(())
    }

    pub(super) fn consume_payload_funding(
        &mut self,
        bytes: usize,
    ) -> Result<(), super::AllocError> {
        if let Some(funding) = self.adaptive_funding.as_mut() {
            funding.take_payload(bytes as u64)?;
        }
        Ok(())
    }

    pub(super) fn check_root_funding(&self, bytes: usize) -> Result<(), super::AllocError> {
        if let Some(funding) = self.adaptive_funding {
            funding.check_root(self.elastic_inventory().bytes, bytes as u64)?;
        } else if let Some(floor) = self.adaptive_reserve {
            Funding::new(self.elastic_inventory().bytes, floor, bytes as u64, 0)?;
        }
        Ok(())
    }

    pub(super) fn consume_root_funding(&mut self, bytes: usize) -> Result<(), super::AllocError> {
        if let Some(funding) = self.adaptive_funding.as_mut() {
            funding.take_root(bytes as u64)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    extern crate std;

    fn with_allocator(test: impl FnOnce(&mut super::super::ObjectAllocator) + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(move || {
                let mut allocator = super::super::ObjectAllocator::empty();
                allocator.slots.initialize(100..110).unwrap();
                allocator.untypeds[0] = Some(super::super::UntypedRegion {
                    cap: sel4::cap::Untyped::from_bits(50),
                    paddr: 0,
                    size_bits: 16,
                    watermark: 0,
                });
                allocator.untyped_len = 1;
                test(&mut allocator);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    fn counts() -> boot_contracts::private_memory_policy::ledger::Resources {
        boot_contracts::private_memory_policy::ledger::Resources {
            bytes: 4096,
            slots: 4,
            descriptors: 3,
            extents: 2,
            tables: 1,
        }
    }

    #[test]
    fn operational_counts_are_materialized_and_withheld() {
        with_allocator(|allocator| {
            allocator.reserve_operational_resources(counts()).unwrap();
            assert_eq!(allocator.free_slots(), 6);
            assert_eq!(
                allocator.allocation_descriptors_free(),
                allocator.allocations.len() - 3
            );
            assert_eq!(
                allocator.extent_descriptors_free(),
                allocator.extents.len() - 2
            );
            assert_eq!(allocator.operational_resources.bytes, 0);
            assert_eq!(allocator.operational_resources.tables, 0);
            assert_eq!(allocator.adaptive_reserve, None);
            for _ in 0..6 {
                allocator.take_slot().unwrap();
            }
            assert!(allocator.take_slot().is_err());
            assert!(allocator.ensure_contiguous_root_slots(1).is_err());
            assert_eq!(allocator.slots.free(), 4);
        });
    }

    #[test]
    fn operational_loan_restores_floor_without_recreating_consumed_capacity() {
        with_allocator(|allocator| {
            allocator.reserve_operational_resources(counts()).unwrap();
            allocator.enable_adaptive_reserve(4096);
            for _ in 0..6 {
                allocator.take_slot().unwrap();
            }
            let snapshot = allocator.begin_construction_funding(12, true).unwrap();
            assert_eq!(allocator.free_slots(), 4);
            assert_eq!(
                allocator.allocation_descriptors_free(),
                allocator.allocations.len()
            );
            assert_eq!(allocator.extent_descriptors_free(), allocator.extents.len());
            allocator.take_slot().unwrap();
            assert!(snapshot.is_some());
            allocator.finish_adaptive_funding();
            assert!(!allocator.operational_lent);
            assert_eq!(allocator.slots.free(), 3);
            assert_eq!(allocator.free_slots(), 0);
            assert!(allocator.take_slot().is_err());
            let snapshot = allocator.begin_construction_funding(12, true).unwrap();
            assert_eq!(allocator.free_slots(), 3);
            assert!(snapshot.is_some());
            allocator.finish_adaptive_funding();
            assert_eq!(allocator.free_slots(), 0);
        });
    }

    #[test]
    fn only_successful_operational_scope_lends_counts() {
        with_allocator(|allocator| {
            allocator.reserve_operational_resources(counts()).unwrap();
            assert!(
                allocator
                    .begin_construction_funding(12, true)
                    .unwrap()
                    .is_none()
            );
            assert!(!allocator.operational_lent);
            allocator.enable_adaptive_reserve(4096);
            assert!(allocator.begin_construction_funding(17, true).is_err());
            assert!(!allocator.operational_lent);
            let snapshot = allocator.begin_construction_funding(12, false).unwrap();
            assert_eq!(allocator.free_slots(), 6);
            assert!(allocator.begin_construction_funding(12, true).is_err());
            assert!(!allocator.operational_lent);
            assert!(allocator.reserve_operational_resources(counts()).is_err());
            assert!(snapshot.is_some());
            allocator.finish_adaptive_funding();
            assert_eq!(allocator.free_slots(), 6);
        });
    }

    #[test]
    fn failed_materialization_does_not_publish_a_floor() {
        with_allocator(|allocator| {
            let mut resources = counts();
            resources.slots = 11;
            assert!(allocator.reserve_operational_resources(resources).is_err());
            assert_eq!(
                allocator.operational_resources,
                boot_contracts::private_memory_policy::ledger::Resources::ZERO
            );
            assert_eq!(allocator.free_slots(), 10);
        });
    }

    #[test]
    fn ordinary_construction_preserves_the_full_floor() {
        let funding = Funding::construction(100, 16, 64).unwrap();
        assert_eq!(funding.root_remaining, 20);
        assert_eq!(funding.check_root(100, 20), Ok(()));
        assert!(funding.check_root(100, 21).is_err());
        assert!(Funding::construction(79, 16, 64).is_err());
    }

    #[test]
    fn operational_construction_can_spend_reserve_but_not_unfunded_bytes() {
        let mut funding = Funding::construction(80, 0, 64).unwrap();
        assert_eq!(funding.root_remaining, 16);
        assert!(funding.check_root(80, 17).is_err());
        funding.check_root(80, 16).unwrap();
        funding.take_root(16).unwrap();
        funding.check_payload(64, 64).unwrap();
        funding.take_payload(64).unwrap();
        assert!(funding.check_payload(0, 1).is_err());
        assert!(funding.check_root(0, 1).is_err());
        assert!(Funding::construction(63, 0, 64).is_err());
    }

    #[test]
    fn failed_construction_retains_root_charge_without_reusing_allowance() {
        let mut funding = Funding::construction(100, 0, 64).unwrap();
        funding.check_root(100, 36).unwrap();
        funding.take_root(36).unwrap();
        // Returning the arena cannot refund a root-lifetime adopted source.
        funding.check_payload(64, 64).unwrap();
        funding.take_payload(64).unwrap();
        assert_eq!(funding.root_owned(), 36);
        assert!(funding.check_root(64, 1).is_err());
        assert!(funding.check_payload(64, 64).is_err());
    }

    #[test]
    fn root_adoption_protects_both_reserve_and_unacquired_payload() {
        let funding = Funding::new(100, 16, 64, 40).unwrap();
        assert_eq!(funding.check_root(100, 20), Ok(()));
        assert_eq!(
            funding.check_root(100, 21),
            Err(Refusal::Reserve {
                required: 101,
                available: 100
            })
        );
    }

    #[test]
    fn whole_source_cost_not_requested_page_decides_adoption() {
        let funding = Funding::new(100, 16, 64, 10).unwrap();
        assert_eq!(funding.check_root(100, 4), Ok(()));
        assert_eq!(
            funding.check_root(100, 20),
            Err(Refusal::Root {
                required: 20,
                available: 10
            })
        );
    }

    #[test]
    fn acquired_payload_does_not_remain_reserved_twice() {
        let mut funding = Funding::new(100, 16, 64, 20).unwrap();
        funding.check_payload(100, 32).unwrap();
        funding.take_payload(32).unwrap();
        assert_eq!(funding.check_root(68, 20), Ok(()));
        funding.take_root(20).unwrap();
        funding.check_payload(48, 32).unwrap();
        funding.take_payload(32).unwrap();
        assert_eq!(funding.check_root(16, 0), Ok(()));
        assert_eq!(funding.root_owned(), 20);
    }

    #[test]
    fn failed_funding_is_atomic_and_retained_root_cost_survives() {
        let mut funding = Funding::new(100, 16, 64, 20).unwrap();
        funding.take_root(10).unwrap();
        let before = funding;
        assert!(funding.take_root(11).is_err());
        assert_eq!(funding, before);
        assert!(funding.take_payload(65).is_err());
        assert_eq!(funding, before);
        assert_eq!(funding.root_owned(), 10);
    }

    #[test]
    fn overflow_cannot_wrap_a_protected_floor() {
        assert_eq!(
            Funding::new(u64::MAX, u64::MAX, 1, 0),
            Err(Refusal::Overflow)
        );
        let funding = Funding::new(u64::MAX, 1, 1, u64::MAX).unwrap();
        assert_eq!(
            funding.check_root(u64::MAX, u64::MAX),
            Err(Refusal::Overflow)
        );
    }
}
