//! Generation-owned native seL4 Notification provisioning.

use crate::object_allocator::{AllocError, ObjectAllocator, TaskArenaId};
use crate::task::TaskId;
use boot_contracts::generation::{Generation, NotificationRole};

pub const MAX_NOTIFICATIONS: usize = 31;
pub const NATIVE_NOTIFICATION_BASE: sel4::CPtrBits = crate::task::CHILD_SLOT_NOTIFICATION_BASE;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationError {
    TableFull,
    Unknown,
    UnlaunchedInstance,
    UnlaidSlot,
    Alloc(AllocError),
    Mint(sel4::Error),
    SlotOutOfRange {
        slot: sel4::CPtrBits,
        limit: sel4::CPtrBits,
    },
}
impl From<AllocError> for NotificationError {
    fn from(error: AllocError) -> Self {
        Self::Alloc(error)
    }
}

#[derive(Clone, Copy)]
struct Entry {
    object: sel4::cap::Notification,
    grant: usize,
}
#[derive(Clone, Copy, Default)]
pub struct ProvisionReport {
    pub created: usize,
    pub bindings: usize,
}
pub struct NotificationTable {
    entries: [Option<Entry>; MAX_NOTIFICATIONS],
    len: usize,
}

impl NotificationTable {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; MAX_NOTIFICATIONS],
            len: 0,
        }
    }
    /// Notification objects this table has materialized.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether the generation declared no notification at all.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The root-held Notification named by a C9.1 timer authority entry. Only
    /// a grant whose declared target is `instance` resolves; the entry cannot
    /// redirect expiry to another component. The expiry badge itself is
    /// contract data: it is an additional root signaller on this object, not a
    /// generation NotificationBinding and therefore not derived from a slot.
    pub fn timer_target(
        &self,
        generation: &Generation<'_>,
        instance: usize,
        grant_identity: u64,
    ) -> Option<sel4::cap::Notification> {
        self.entries.iter().flatten().find_map(|entry| {
            let grant = generation.notification_grant(entry.grant).ok()?;
            (grant.target == instance
                && boot_contracts::clock_authority::notification_grant_identity(grant.name)
                    == grant_identity)
                .then_some(entry.object)
        })
    }

    /// The root-held Notification named by a C9.2 wait-set entry. Only a grant
    /// whose declared target is `instance` resolves, so an entry cannot
    /// attribute a badge to an object its waiter does not wait on.
    ///
    /// A sibling of [`Self::timer_target`] rather than a shared helper taking a
    /// hash function: the two resources deliberately use different identity
    /// domains, and folding them together would make the domain a parameter a
    /// caller could pass the wrong value for — which is exactly the replay the
    /// separate domains exist to prevent.
    pub fn wait_target(
        &self,
        generation: &Generation<'_>,
        instance: usize,
        grant_identity: u64,
    ) -> Option<sel4::cap::Notification> {
        self.entries.iter().flatten().find_map(|entry| {
            let grant = generation.notification_grant(entry.grant).ok()?;
            (grant.target == instance
                && boot_contracts::wait_set::notification_grant_identity(grant.name)
                    == grant_identity)
                .then_some(entry.object)
        })
    }
    pub fn native_slot(
        declared: sel4::CPtrBits,
        cnode_size_bits: usize,
    ) -> Result<sel4::CPtrBits, NotificationError> {
        if declared >= crate::task::CHILD_NATIVE_REGION_SLOTS {
            return Err(NotificationError::SlotOutOfRange {
                slot: declared,
                limit: crate::task::CHILD_NATIVE_REGION_SLOTS,
            });
        }
        let slot = NATIVE_NOTIFICATION_BASE + declared;
        let limit = 1u64 << cnode_size_bits;
        if slot >= limit {
            return Err(NotificationError::SlotOutOfRange { slot, limit });
        }
        Ok(slot)
    }

    pub fn materialize(
        &mut self,
        generation: &Generation<'_>,
        allocator: &mut ObjectAllocator,
    ) -> Result<ProvisionReport, NotificationError> {
        let mut report = ProvisionReport::default();
        for grant_index in 0..generation.notification_grant_count() {
            let grant = generation
                .notification_grant(grant_index)
                .map_err(|_| NotificationError::Unknown)?;
            let slot = self
                .entries
                .iter_mut()
                .find(|entry| entry.is_none())
                .ok_or(NotificationError::TableFull)?;
            *slot = Some(Entry {
                object: allocator
                    .allocate_fixed::<sel4::cap_type::Notification>()?
                    .cap(),
                grant: grant_index,
            });
            self.len += 1;
            report.created += 1;
            sel4::debug_println!(
                "SLIME_GRAPH notification grant={} source_instance={} target_instance={}",
                grant.name,
                grant.source,
                grant.target
            );
        }
        Ok(report)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn install_instance(
        &self,
        generation: &Generation<'_>,
        instance: usize,
        task: TaskId,
        allocator: &mut ObjectAllocator,
        arena: TaskArenaId,
        cnode: sel4::cap::CNode,
        cnode_size_bits: usize,
    ) -> Result<usize, NotificationError> {
        let mut installed = 0;
        for binding_index in 0..generation.notification_binding_count() {
            let binding = generation
                .notification_binding(binding_index)
                .map_err(|_| NotificationError::Unknown)?;
            if binding.holder != instance {
                continue;
            }
            let entry = self
                .entries
                .iter()
                .flatten()
                .find(|entry| entry.grant == binding.grant)
                .ok_or(NotificationError::Unknown)?;
            let destination = Self::native_slot(binding.slot as sel4::CPtrBits, cnode_size_bits)?;
            let minted = allocator
                .reserve_slot_in::<sel4::cap_type::Notification>(arena)?
                .cap();
            let root = sel4::init_thread::slot::CNODE.cap();
            let rights = || match binding.role {
                NotificationRole::Signal => sel4::CapRightsBuilder::none().write(true).build(),
                NotificationRole::Wait => sel4::CapRightsBuilder::none().read(true).build(),
            };
            let badge = match binding.role {
                NotificationRole::Signal => 1u64 << (binding.slot % 63),
                NotificationRole::Wait => 0,
            };
            root.absolute_cptr(minted)
                .mint(&root.absolute_cptr(entry.object), rights(), badge)
                .map_err(NotificationError::Mint)?;
            cnode
                .absolute_cptr_from_bits_with_depth(destination, cnode_size_bits)
                .copy(&root.absolute_cptr(minted), rights())
                .map_err(NotificationError::Mint)?;
            installed += 1;
            sel4::debug_println!(
                "SLIME_GRAPH notification binding task={} slot={} role={:?}",
                task.0,
                destination,
                binding.role
            );
        }
        Ok(installed)
    }
}
impl Default for NotificationTable {
    fn default() -> Self {
        Self::new()
    }
}
