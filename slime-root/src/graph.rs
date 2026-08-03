//! Per-task logical capability tables: what a component's grants actually are.
//!
//! A Slime capability is a *logical slot number* the root task interprets, not a
//! seL4 capability in the child's CSpace. The whole operation surface takes
//! `u32` slots — `spawn(executable_slot, …)`, `endpoint_create(factory_slot)`,
//! `send(slot, …)` — and `ipc::decode_request` refuses a message carrying real
//! seL4 extra-caps outright. So a child CSpace holds only the four capabilities
//! `task.rs` installs (null, service endpoint, own TCB, fault endpoint), and
//! every grant the generation declares lives here instead.
//!
//! That is what keeps `CHILD_CNODE_SIZE_BITS = 2` correct while `init`'s boot
//! layout reaches slot 18: the layout numbers *logical* slots, and the root
//! resolves each one to the object it names. A component cannot forge a slot it
//! was not granted, because the resolution is a table lookup here rather than a
//! CSpace address the child controls.
//!
//! Slot numbers come from the generation's boot-layout resource, so the
//! component that uses a slot and the root that fills it read one source (B10).

use crate::ipc::IpcError;
use crate::task::TaskId;

/// Logical capability slots one task may hold. Matches the retired kernel's
/// `MAX_CAPS`, which is the bound `init`'s layout and the spawn grant array
/// were already written against.
pub const MAX_TASK_CAPS: usize = 64;

/// Tasks one generation's graph may declare capability tables for.
pub const MAX_GRAPH_TASKS: usize = 16;

/// What a logical slot resolves to.
///
/// Deliberately not a seL4 capability: these are the objects `slime-root` owns
/// and mediates. A child never holds one, it names one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resource {
    /// An executable this task may spawn, named by generation component index.
    Executable { component: usize },
    /// One end of a logical channel, by channel key.
    Endpoint { channel: u32 },
    /// Authority to mint channel pairs.
    EndpointFactory,
    /// Authority to allocate shared buffers.
    SharedBufferFactory,
    /// A supervision handle for a spawned child.
    Supervision { task: TaskId },
}

/// One logical capability: what it names, and the rights held over it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capability {
    pub resource: Resource,
    pub rights: u64,
}

impl Capability {
    /// Whether this capability carries every right in `required`.
    pub const fn allows(&self, required: u64) -> bool {
        self.rights & required == required
    }
}

/// One task's logical capability table.
pub struct CapabilityTable {
    slots: [Option<Capability>; MAX_TASK_CAPS],
    len: usize,
}

impl CapabilityTable {
    pub const fn new() -> Self {
        Self {
            slots: [None; MAX_TASK_CAPS],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Install `capability` at exactly `slot`.
    ///
    /// The slot is chosen by the generation's boot layout, not allocated here,
    /// so the number a component compiles against and the number the root fills
    /// are the same number by construction. Overwriting a filled slot is
    /// refused: two grants resolving to one slot is a layout defect, and
    /// silently keeping the last would hand a component authority it was not
    /// declared.
    pub fn install(&mut self, slot: u32, capability: Capability) -> Result<(), IpcError> {
        let Some(entry) = self.slots.get_mut(slot as usize) else {
            return Err(IpcError::InvalidOperation);
        };
        if entry.is_some() {
            return Err(IpcError::WaiterConflict);
        }
        *entry = Some(capability);
        self.len += 1;
        Ok(())
    }

    /// The capability at `slot`, if the task holds one.
    pub fn get(&self, slot: u32) -> Option<Capability> {
        self.slots.get(slot as usize).copied().flatten()
    }

    /// Resolve `slot`, requiring every right in `required`.
    ///
    /// A slot the task does not hold and a slot whose rights are too narrow are
    /// both `BadCap`-shaped failures, deliberately: which one it was is not a
    /// component's business, and distinguishing them would let a component probe
    /// the table for slots it was never granted.
    pub fn resolve(&self, slot: u32, required: u64) -> Result<Capability, IpcError> {
        self.get(slot)
            .filter(|capability| capability.allows(required))
            .ok_or(IpcError::InvalidOperation)
    }

    /// Drop the capability at `slot`, reporting whether one was there.
    pub fn drop_slot(&mut self, slot: u32) -> bool {
        let Some(entry) = self.slots.get_mut(slot as usize) else {
            return false;
        };
        if entry.take().is_some() {
            self.len -= 1;
            return true;
        }
        false
    }

    /// The first free slot at or above `from`, for authority minted at runtime
    /// (a spawn's supervision handle, a freshly created channel end) that no
    /// layout could have numbered in advance.
    pub fn free_slot_from(&self, from: u32) -> Option<u32> {
        (from as usize..MAX_TASK_CAPS)
            .find(|index| self.slots[*index].is_none())
            .map(|index| index as u32)
    }
}

impl Default for CapabilityTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Every task's capability table, keyed by logical task id.
pub struct GraphTables {
    tables: [Option<(TaskId, CapabilityTable)>; MAX_GRAPH_TASKS],
    len: usize,
}

impl GraphTables {
    pub const fn new() -> Self {
        Self {
            tables: [const { None }; MAX_GRAPH_TASKS],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Start an empty table for `task`.
    pub fn create(&mut self, task: TaskId) -> Result<&mut CapabilityTable, IpcError> {
        if self.tables.iter().flatten().any(|(id, _)| *id == task) {
            return Err(IpcError::WaiterConflict);
        }
        let Some(slot) = self.tables.iter_mut().find(|entry| entry.is_none()) else {
            return Err(IpcError::WaitSetFull);
        };
        *slot = Some((task, CapabilityTable::new()));
        self.len += 1;
        Ok(&mut slot.as_mut().expect("just installed").1)
    }

    pub fn get(&self, task: TaskId) -> Option<&CapabilityTable> {
        self.tables
            .iter()
            .flatten()
            .find(|(id, _)| *id == task)
            .map(|(_, table)| table)
    }

    pub fn get_mut(&mut self, task: TaskId) -> Option<&mut CapabilityTable> {
        self.tables
            .iter_mut()
            .flatten()
            .find(|(id, _)| *id == task)
            .map(|(_, table)| table)
    }

    /// Drop a task's whole table as part of reclaiming it.
    pub fn release(&mut self, task: TaskId) -> bool {
        let Some(slot) = self
            .tables
            .iter_mut()
            .find(|entry| entry.as_ref().is_some_and(|(id, _)| *id == task))
        else {
            return false;
        };
        *slot = None;
        self.len -= 1;
        true
    }
}

impl Default for GraphTables {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{Capability, CapabilityTable, GraphTables, MAX_TASK_CAPS, Resource};
    use crate::ipc::IpcError;
    use crate::task::TaskId;

    const RIGHT_SEND: u64 = 1;
    const RIGHT_RECV: u64 = 1 << 1;
    const RIGHT_EXEC: u64 = 1 << 3;

    fn endpoint(rights: u64) -> Capability {
        Capability {
            resource: Resource::Endpoint { channel: 7 },
            rights,
        }
    }

    #[test]
    fn a_slot_resolves_only_with_the_rights_it_was_granted() {
        let mut table = CapabilityTable::new();
        table.install(4, endpoint(RIGHT_SEND)).unwrap();

        assert!(table.resolve(4, RIGHT_SEND).is_ok());
        assert_eq!(
            table.resolve(4, RIGHT_SEND | RIGHT_RECV),
            Err(IpcError::InvalidOperation),
            "a narrower grant cannot satisfy a wider requirement"
        );
    }

    #[test]
    fn an_ungranted_slot_is_indistinguishable_from_an_underpowered_one() {
        let mut table = CapabilityTable::new();
        table.install(4, endpoint(RIGHT_SEND)).unwrap();
        // Both are the same error, so a component cannot probe the table to
        // discover which slots exist.
        assert_eq!(
            table.resolve(9, RIGHT_SEND),
            table.resolve(4, RIGHT_EXEC),
            "an absent slot and an insufficient one report identically"
        );
    }

    #[test]
    fn a_layout_cannot_fill_one_slot_twice() {
        let mut table = CapabilityTable::new();
        table.install(2, endpoint(RIGHT_SEND)).unwrap();
        assert_eq!(
            table.install(2, endpoint(RIGHT_RECV)),
            Err(IpcError::WaiterConflict),
            "two grants at one slot is a layout defect, not a last-wins merge"
        );
        // The original grant is intact.
        assert_eq!(table.get(2), Some(endpoint(RIGHT_SEND)));
    }

    #[test]
    fn a_slot_past_the_table_is_refused_rather_than_wrapping() {
        let mut table = CapabilityTable::new();
        assert_eq!(
            table.install(MAX_TASK_CAPS as u32, endpoint(RIGHT_SEND)),
            Err(IpcError::InvalidOperation)
        );
        assert_eq!(table.get(MAX_TASK_CAPS as u32), None);
    }

    #[test]
    fn dropping_a_slot_frees_it_for_runtime_minted_authority() {
        let mut table = CapabilityTable::new();
        table.install(0, endpoint(RIGHT_SEND)).unwrap();
        table.install(1, endpoint(RIGHT_SEND)).unwrap();
        assert_eq!(table.free_slot_from(0), Some(2));

        assert!(table.drop_slot(1));
        assert_eq!(table.len(), 1);
        assert_eq!(table.free_slot_from(0), Some(1));
        assert!(!table.drop_slot(1), "dropping twice is not a drop");
    }

    #[test]
    fn one_task_cannot_see_anothers_capabilities() {
        let mut graph = GraphTables::new();
        graph
            .create(TaskId(0))
            .unwrap()
            .install(3, endpoint(RIGHT_SEND))
            .unwrap();
        graph.create(TaskId(1)).unwrap();

        assert!(graph.get(TaskId(0)).unwrap().resolve(3, RIGHT_SEND).is_ok());
        assert_eq!(
            graph.get(TaskId(1)).unwrap().resolve(3, RIGHT_SEND),
            Err(IpcError::InvalidOperation),
            "slot 3 means nothing to a task that was not granted it"
        );
    }

    #[test]
    fn a_reclaimed_task_keeps_no_authority() {
        let mut graph = GraphTables::new();
        graph
            .create(TaskId(0))
            .unwrap()
            .install(3, endpoint(RIGHT_SEND))
            .unwrap();
        assert!(graph.release(TaskId(0)));
        assert!(graph.get(TaskId(0)).is_none());
        assert!(!graph.release(TaskId(0)));
        assert_eq!(graph.len(), 0);
    }
}
