//! Generation-owned native seL4 Endpoint provisioning.

use crate::launched::LaunchedInstances;
use crate::object_allocator::{AllocError, ObjectAllocator, TaskArenaId};
use crate::task::{MAX_TASKS, TaskId, TaskTable};
use boot_contracts::generation::{
    CapabilityKind, Generation, GrantEndpoint, RIGHT_RECV, RIGHT_SEND, RIGHT_TRANSFER,
};

pub const MAX_PEER_ENDPOINTS: usize = 48;
pub const NATIVE_ENDPOINT_BASE: sel4::CPtrBits = crate::task::CHILD_SLOT_ENDPOINT_BASE;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PeerEndpointError {
    TableFull,
    Unknown,
    UnlaidSlot,
    UnlaunchedInstance,
    Alloc(AllocError),
    Mint(sel4::Error),
    SlotOutOfRange {
        slot: sel4::CPtrBits,
        limit: sel4::CPtrBits,
    },
}
impl From<AllocError> for PeerEndpointError {
    fn from(error: AllocError) -> Self {
        Self::Alloc(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    Producer,
    Consumer,
    Both,
}
impl Side {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Producer => "producer",
            Self::Consumer => "consumer",
            Self::Both => "both",
        }
    }
}

#[derive(Clone, Copy)]
struct Entry {
    object: sel4::cap::Endpoint,
    grant: usize,
    source: usize,
    target: usize,
    rights: u64,
}
impl Entry {
    fn side_for(self, instance: usize) -> Option<Side> {
        if self.source == self.target && self.source == instance {
            return Some(Side::Both);
        }
        if instance != self.source && instance != self.target {
            return None;
        }
        let carries = self.rights & (RIGHT_SEND | RIGHT_RECV);
        if carries == RIGHT_SEND | RIGHT_RECV {
            Some(Side::Both)
        } else if (carries == RIGHT_RECV && instance == self.source)
            || (carries == RIGHT_SEND && instance == self.target)
        {
            Some(Side::Producer)
        } else {
            Some(Side::Consumer)
        }
    }

    fn held_rights(self, side: Side) -> u64 {
        let carries = self.rights & (RIGHT_SEND | RIGHT_RECV);
        let directional = if carries == RIGHT_SEND | RIGHT_RECV {
            carries
        } else if side == Side::Producer {
            RIGHT_SEND
        } else {
            RIGHT_RECV
        };
        directional | (self.rights & RIGHT_TRANSFER)
    }

    fn cap_rights(self, side: Side) -> sel4::CapRights {
        let rights = self.held_rights(side);
        let can_send = rights & RIGHT_SEND != 0;
        sel4::CapRightsBuilder::none()
            .write(can_send)
            .read(rights & RIGHT_RECV != 0)
            .grant(can_send && rights & RIGHT_TRANSFER != 0)
            .grant_reply(can_send)
            .build()
    }
}
#[derive(Clone, Copy, Default)]
pub struct ProvisionReport {
    pub grants: usize,
    pub installed: usize,
}
pub struct PeerEndpointTable {
    entries: [Option<Entry>; MAX_PEER_ENDPOINTS],
    len: usize,
}

impl PeerEndpointTable {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; MAX_PEER_ENDPOINTS],
            len: 0,
        }
    }
    /// Declared channel objects this table has materialized.
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether the generation declared no peer endpoint at all.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn native_slot(
        declared: sel4::CPtrBits,
        cnode_size_bits: usize,
    ) -> Result<sel4::CPtrBits, PeerEndpointError> {
        if declared >= crate::task::CHILD_NATIVE_REGION_SLOTS {
            return Err(PeerEndpointError::SlotOutOfRange {
                slot: declared,
                limit: crate::task::CHILD_NATIVE_REGION_SLOTS,
            });
        }
        let slot = NATIVE_ENDPOINT_BASE + declared;
        let limit = 1u64 << cnode_size_bits;
        if slot >= limit {
            return Err(PeerEndpointError::SlotOutOfRange { slot, limit });
        }
        Ok(slot)
    }

    pub fn materialize(
        &mut self,
        generation: &Generation<'_>,
        launched: &LaunchedInstances,
        allocator: &mut ObjectAllocator,
        tasks: &mut TaskTable<MAX_TASKS>,
    ) -> Result<ProvisionReport, PeerEndpointError> {
        let mut report = ProvisionReport::default();
        for grant_index in 0..generation.grant_count() {
            let grant = generation
                .grant(grant_index)
                .map_err(|_| PeerEndpointError::Unknown)?;
            if grant.capability_kind != CapabilityKind::Endpoint {
                continue;
            }
            let (GrantEndpoint::Instance(source), GrantEndpoint::Instance(target)) =
                (grant.source, grant.target)
            else {
                continue;
            };
            let slot = self
                .entries
                .iter_mut()
                .find(|entry| entry.is_none())
                .ok_or(PeerEndpointError::TableFull)?;
            let object = allocator
                .allocate_fixed::<sel4::cap_type::Endpoint>()?
                .cap();
            let carries = grant.rights & (RIGHT_SEND | RIGHT_RECV);
            let (producer, consumer) = if carries == RIGHT_RECV {
                (source, target)
            } else {
                (target, source)
            };
            *slot = Some(Entry {
                object,
                grant: grant_index,
                source,
                target,
                rights: grant.rights,
            });
            self.len += 1;
            report.grants += 1;
            sel4::debug_println!(
                "SLIME_GRAPH endpoint grant={} producer_instance={} consumer_instance={}",
                grant.name,
                producer,
                consumer
            );
        }
        for launched in launched.iter() {
            let (arena, cnode, cnode_size_bits) = {
                let task = tasks
                    .get(launched.task)
                    .ok_or(PeerEndpointError::UnlaunchedInstance)?;
                (task.cleanup.arena, task.cnode, task.cnode_size_bits)
            };
            let installed = self.install_instance(
                generation,
                launched.instance,
                launched.task,
                allocator,
                arena,
                cnode,
                cnode_size_bits,
            )?;
            report.installed += installed;
            // C8.13.3: each install filled a slot the generation declared, so
            // it belongs to the holder's declared-space count -- the space
            // `capabilitySlots` budgets.
            if let Some(task) = tasks.get_mut(launched.task) {
                task.cspace.installed(installed as u32);
            }
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
    ) -> Result<usize, PeerEndpointError> {
        let record = generation
            .instance(instance)
            .map_err(|_| PeerEndpointError::UnlaunchedInstance)?;
        let mut installed = 0;
        for entry in self.entries.iter().flatten() {
            let Some(side) = entry.side_for(instance) else {
                continue;
            };
            let declared = (0..record.binding_count())
                .filter_map(|i| generation.binding(record, i).ok())
                .find(|binding| binding.grant == entry.grant)
                .and_then(|binding| u32::try_from(binding.slot).ok())
                .ok_or(PeerEndpointError::UnlaidSlot)?;
            let native = Self::native_slot(declared as sel4::CPtrBits, cnode_size_bits)?;
            let minted = allocator
                .reserve_slot_in::<sel4::cap_type::Endpoint>(arena)?
                .cap();
            let root = sel4::init_thread::slot::CNODE.cap();
            root.absolute_cptr(minted)
                .mint(&root.absolute_cptr(entry.object), entry.cap_rights(side), 0)
                .map_err(PeerEndpointError::Mint)?;
            cnode
                .absolute_cptr_from_bits_with_depth(native, cnode_size_bits)
                .copy(&root.absolute_cptr(minted), entry.cap_rights(side))
                .map_err(PeerEndpointError::Mint)?;
            installed += 1;
            sel4::debug_println!(
                "SLIME_GRAPH native endpoint task={} slot={} side={}",
                task.0,
                native,
                side.name()
            );
        }
        Ok(installed)
    }

    pub fn endpoint_for(
        &self,
        generation: &Generation<'_>,
        holder_instance: usize,
        declared_slot: u32,
    ) -> Option<(sel4::cap::Endpoint, Side, bool)> {
        let instance = generation.instance(holder_instance).ok()?;
        let binding = (0..instance.binding_count())
            .filter_map(|index| generation.binding(instance, index).ok())
            .find(|binding| binding.slot == declared_slot as usize)?;
        let entry = self
            .entries
            .iter()
            .flatten()
            .find(|entry| entry.grant == binding.grant)?;
        let side = entry.side_for(holder_instance)?;
        Some((entry.object, side, entry.rights & RIGHT_TRANSFER != 0))
    }

    /// The peer this holder's control edge names, and whether the grant
    /// carries `RIGHT_TRANSFER`.
    ///
    /// `launched` alone cannot answer this for a dynamically spawned pair:
    /// `LaunchedInstances` is populated only by the boot-time root-autostart
    /// staging pass (`main.rs`'s one `launched_instances.record` call site),
    /// so a task that reached this edge via `spawn()` — as every C9.6
    /// participant does — is invisible to it on both ends. `tasks` is
    /// authoritative for every live task regardless of how it was
    /// constructed: `construct_child` records `Some(plan.instance)` on the
    /// spawn path exactly as the boot path does, so a live-task scan finds
    /// what `launched` cannot. Tried second, not instead: the boot-time table
    /// answers a root-autostart peer without walking the task table, so the
    /// common case pays no extra cost.
    pub fn receiver_for(
        &self,
        generation: &Generation<'_>,
        sender_instance: usize,
        declared_slot: u32,
        launched: &LaunchedInstances,
        tasks: &TaskTable<MAX_TASKS>,
    ) -> Option<(TaskId, bool)> {
        let instance = generation.instance(sender_instance).ok()?;
        let binding = (0..instance.binding_count())
            .filter_map(|i| generation.binding(instance, i).ok())
            .find(|binding| binding.slot == declared_slot as usize)?;
        let entry = self
            .entries
            .iter()
            .flatten()
            .find(|entry| entry.grant == binding.grant)?;
        let receiver = if sender_instance == entry.source {
            entry.target
        } else if sender_instance == entry.target {
            entry.source
        } else {
            return None;
        };
        let task = launched.task_for_instance(receiver).or_else(|| {
            tasks
                .tasks()
                .find(|task| task.instance == Some(receiver))
                .map(|task| task.id)
        })?;
        Some((task, entry.rights & RIGHT_TRANSFER != 0))
    }
}
impl Default for PeerEndpointTable {
    fn default() -> Self {
        Self::new()
    }
}
