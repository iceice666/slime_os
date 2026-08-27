//! Per-task root-mediated authority.
//!
//! Each [`crate::task::Task`] owns exactly one bounded table. There is no
//! generation-global task-id-to-authority index: a caller is first resolved in
//! [`crate::task::TaskTable`], then its own table is consulted.

use crate::directory::ScopeId;
use crate::ipc::IpcError;
use crate::shared_buffer::{BufferHandle, LoanHandle};
use crate::task::TaskId;
// B59: the capability-rights vocabulary is generated from
// `contracts/generation/v5/schema.zt`; these were local copies of the same
// bit numbering.
use boot_contracts::generation::{
    RIGHT_BLOCK_READ, RIGHT_BLOCK_WRITE, RIGHT_BUFFER_CREATE, RIGHT_BUFFER_LOAN, RIGHT_BUFFER_MAP,
    RIGHT_BUFFER_WRITE, RIGHT_DIRECTORY_DERIVE, RIGHT_DIRECTORY_LIST, RIGHT_DIRECTORY_READ,
    RIGHT_DIRECTORY_WRITE, RIGHT_EXEC, RIGHT_INPUT_READ, RIGHT_LIFECYCLE_RESTART,
    RIGHT_PARAMETER_READ, RIGHT_PARAMETER_WRITE, RIGHT_RECV, RIGHT_SCHEDULING_PROMOTE, RIGHT_SEND,
    RIGHT_SPAWN, RIGHT_SUPERVISE, RIGHT_TRANSFER,
};

/// Logical capability slots one task may hold.
pub const MAX_TASK_CAPS: usize = 64;

macro_rules! rights_type {
    ($name:ident, $valid:expr) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub struct $name(u64);

        impl $name {
            pub const VALID: u64 = $valid;

            pub const fn from_bits(bits: u64) -> Option<Self> {
                if bits != 0 && bits & !Self::VALID == 0 {
                    Some(Self(bits))
                } else {
                    None
                }
            }

            pub const fn bits(self) -> u64 {
                self.0
            }

            pub const fn allows(self, required: u64) -> bool {
                required != 0 && required & !Self::VALID == 0 && self.0 & required == required
            }

            pub const fn narrow(self, requested: u64) -> Option<Self> {
                if requested != 0 && requested & !self.0 == 0 {
                    Self::from_bits(requested)
                } else {
                    None
                }
            }
        }
    };
}

rights_type!(ExecutableRights, RIGHT_EXEC | RIGHT_SPAWN | RIGHT_TRANSFER);
rights_type!(BufferFactoryRights, RIGHT_BUFFER_CREATE | RIGHT_TRANSFER);
// C9.3: a supervision handle is also where promotion authority rides. The
// generation decides *which* handles carry the bit — the root sets it only when
// the scheduling-class policy declares an edge from this spawner to this child —
// so the right is on the capability the operation resolves rather than being a
// name the operation looks up separately.
rights_type!(
    SupervisionRights,
    RIGHT_SUPERVISE
        | RIGHT_SCHEDULING_PROMOTE
        | RIGHT_LIFECYCLE_RESTART
        | RIGHT_PARAMETER_READ
        | RIGHT_PARAMETER_WRITE
        | RIGHT_TRANSFER
);
rights_type!(BlockRights, RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE);
rights_type!(
    DirectoryRights,
    RIGHT_DIRECTORY_READ
        | RIGHT_DIRECTORY_WRITE
        | RIGHT_DIRECTORY_LIST
        | RIGHT_DIRECTORY_DERIVE
        | RIGHT_TRANSFER
);
rights_type!(InputRights, RIGHT_INPUT_READ);
rights_type!(NativeEndpointRights, RIGHT_SEND | RIGHT_RECV);
rights_type!(
    LoanRights,
    RIGHT_BUFFER_MAP | RIGHT_BUFFER_WRITE | RIGHT_TRANSFER
);

rights_type!(
    SharedBufferRights,
    RIGHT_BUFFER_WRITE | RIGHT_BUFFER_MAP | RIGHT_BUFFER_LOAN | RIGHT_TRANSFER
);

impl SharedBufferRights {
    pub const fn from_created_handle(handle: BufferHandle) -> Self {
        let mut bits = RIGHT_BUFFER_MAP | RIGHT_BUFFER_LOAN | RIGHT_TRANSFER;
        if handle
            .rights
            .contains(crate::shared_buffer::BufferRights::WRITE)
        {
            bits |= RIGHT_BUFFER_WRITE;
        }
        Self(bits)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutableCapability {
    pub executable: usize,
    pub rights: ExecutableRights,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferFactoryCapability {
    pub rights: BufferFactoryRights,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SupervisionCapability {
    pub task: TaskId,
    pub rights: SupervisionRights,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockCapability {
    pub device: u8,
    pub rights: BlockRights,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectoryCapability {
    pub namespace: u32,
    pub scope: ScopeId,
    pub rights: DirectoryRights,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputCapability {
    pub rights: InputRights,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeEndpointCapability {
    pub rights: NativeEndpointRights,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SharedBufferCapability {
    pub handle: BufferHandle,
    pub rights: SharedBufferRights,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoanCapability {
    pub handle: LoanHandle,
    pub rights: LoanRights,
}

/// One typed entry in a task-owned authority table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityEntry {
    Executable(ExecutableCapability),
    BufferFactory(BufferFactoryCapability),
    Supervision(SupervisionCapability),
    Block(BlockCapability),
    Directory(DirectoryCapability),
    Input(InputCapability),
    NativeEndpoint(NativeEndpointCapability),
    SharedBuffer(SharedBufferCapability),
    Loan(LoanCapability),
}

impl CapabilityEntry {
    pub const fn executable(executable: usize, rights: u64) -> Option<Self> {
        match ExecutableRights::from_bits(rights) {
            Some(rights) => Some(Self::Executable(ExecutableCapability {
                executable,
                rights,
            })),
            None => None,
        }
    }

    pub const fn buffer_factory(rights: u64) -> Option<Self> {
        match BufferFactoryRights::from_bits(rights) {
            Some(rights) => Some(Self::BufferFactory(BufferFactoryCapability { rights })),
            None => None,
        }
    }

    pub const fn supervision(task: TaskId, rights: u64) -> Option<Self> {
        match SupervisionRights::from_bits(rights) {
            Some(rights) => Some(Self::Supervision(SupervisionCapability { task, rights })),
            None => None,
        }
    }

    pub const fn block(device: u8, rights: u64) -> Option<Self> {
        match BlockRights::from_bits(rights) {
            Some(rights) => Some(Self::Block(BlockCapability { device, rights })),
            None => None,
        }
    }

    pub const fn directory(namespace: u32, scope: ScopeId, rights: u64) -> Option<Self> {
        match DirectoryRights::from_bits(rights) {
            Some(rights) => Some(Self::Directory(DirectoryCapability {
                namespace,
                scope,
                rights,
            })),
            None => None,
        }
    }

    pub const fn input(rights: u64) -> Option<Self> {
        match InputRights::from_bits(rights) {
            Some(rights) => Some(Self::Input(InputCapability { rights })),
            None => None,
        }
    }

    pub const fn native_endpoint(rights: u64) -> Option<Self> {
        match NativeEndpointRights::from_bits(rights) {
            Some(rights) => Some(Self::NativeEndpoint(NativeEndpointCapability { rights })),
            None => None,
        }
    }

    pub const fn shared_buffer(handle: BufferHandle, rights: u64) -> Option<Self> {
        match SharedBufferRights::from_bits(rights) {
            Some(rights) => Some(Self::SharedBuffer(SharedBufferCapability {
                handle,
                rights,
            })),
            None => None,
        }
    }

    pub const fn loan(handle: LoanHandle, rights: u64) -> Option<Self> {
        match LoanRights::from_bits(rights) {
            Some(rights) => Some(Self::Loan(LoanCapability { handle, rights })),
            None => None,
        }
    }

    pub const fn kind_name(self) -> &'static str {
        match self {
            Self::Executable(_) => "executable",
            Self::BufferFactory(_) => "shared-buffer-factory",
            Self::Supervision(_) => "supervision",
            Self::Block(_) => "block",
            Self::Directory(_) => "directory",
            Self::Input(_) => "input",
            Self::NativeEndpoint(_) => "endpoint",
            Self::SharedBuffer(_) => "shared-buffer",
            Self::Loan(_) => "loan",
        }
    }

    pub const fn rights_bits(self) -> u64 {
        match self {
            Self::Executable(cap) => cap.rights.bits(),
            Self::BufferFactory(cap) => cap.rights.bits(),
            Self::Supervision(cap) => cap.rights.bits(),
            Self::Block(cap) => cap.rights.bits(),
            Self::Directory(cap) => cap.rights.bits(),
            Self::Input(cap) => cap.rights.bits(),
            Self::NativeEndpoint(cap) => cap.rights.bits(),
            Self::SharedBuffer(cap) => cap.rights.bits(),
            Self::Loan(cap) => cap.rights.bits(),
        }
    }

    pub const fn allows(self, required: u64) -> bool {
        match self {
            Self::Executable(cap) => cap.rights.allows(required),
            Self::BufferFactory(cap) => cap.rights.allows(required),
            Self::Supervision(cap) => cap.rights.allows(required),
            Self::Block(cap) => cap.rights.allows(required),
            Self::Directory(cap) => cap.rights.allows(required),
            Self::Input(cap) => cap.rights.allows(required),
            Self::NativeEndpoint(cap) => cap.rights.allows(required),
            Self::Loan(cap) => cap.rights.allows(required),
            Self::SharedBuffer(cap) => cap.rights.allows(required),
        }
    }

    pub const fn narrow(self, requested: u64) -> Option<Self> {
        match self {
            Self::Executable(mut cap) => match cap.rights.narrow(requested) {
                Some(rights) => {
                    cap.rights = rights;
                    Some(Self::Executable(cap))
                }
                None => None,
            },
            Self::BufferFactory(mut cap) => match cap.rights.narrow(requested) {
                Some(rights) => {
                    cap.rights = rights;
                    Some(Self::BufferFactory(cap))
                }
                None => None,
            },
            Self::Supervision(mut cap) => match cap.rights.narrow(requested) {
                Some(rights) => {
                    cap.rights = rights;
                    Some(Self::Supervision(cap))
                }
                None => None,
            },
            Self::Block(mut cap) => match cap.rights.narrow(requested) {
                Some(rights) => {
                    cap.rights = rights;
                    Some(Self::Block(cap))
                }
                None => None,
            },
            Self::Directory(mut cap) => match cap.rights.narrow(requested) {
                Some(rights) => {
                    cap.rights = rights;
                    Some(Self::Directory(cap))
                }
                None => None,
            },
            Self::Input(mut cap) => match cap.rights.narrow(requested) {
                Some(rights) => {
                    cap.rights = rights;
                    Some(Self::Input(cap))
                }
                None => None,
            },
            Self::NativeEndpoint(mut cap) => match cap.rights.narrow(requested) {
                Some(rights) => {
                    cap.rights = rights;
                    Some(Self::NativeEndpoint(cap))
                }
                None => None,
            },
            Self::SharedBuffer(mut cap) => match cap.rights.narrow(requested) {
                Some(rights) => {
                    cap.rights = rights;
                    Some(Self::SharedBuffer(cap))
                }
                None => None,
            },
            Self::Loan(mut cap) => match cap.rights.narrow(requested) {
                Some(rights) => {
                    cap.rights = rights;
                    Some(Self::Loan(cap))
                }
                None => None,
            },
        }
    }

    pub const fn is_transferable(self) -> bool {
        self.rights_bits() & RIGHT_TRANSFER != 0
    }
}

/// One task's bounded logical authority, stored directly in its task record.
#[derive(Clone, Copy, Debug)]
pub struct AuthorityTable {
    slots: [Option<CapabilityEntry>; MAX_TASK_CAPS],
    len: usize,
    /// The most entries this table ever held (C8.13.3).
    ///
    /// Tracked here rather than derived at read time because a table this size
    /// is mutated on many paths between any two reads — every install, drop,
    /// transfer, and retirement — so a sample taken when a component asks
    /// cannot see the run's real maximum. It is a count of *this* half of
    /// declared-space occupancy; `crate::cspace::CSpaceLedger` folds it in with
    /// the natively installed half.
    peak: usize,
}

impl AuthorityTable {
    pub const fn new() -> Self {
        Self {
            slots: [None; MAX_TASK_CAPS],
            len: 0,
            peak: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The most entries this table ever held.
    pub const fn peak(&self) -> usize {
        self.peak
    }

    pub fn slots(&self) -> impl Iterator<Item = (u32, Option<&CapabilityEntry>)> {
        self.slots
            .iter()
            .enumerate()
            .map(|(slot, entry)| (slot as u32, entry.as_ref()))
    }

    pub fn install(&mut self, slot: u32, capability: CapabilityEntry) -> Result<(), IpcError> {
        let Some(entry) = self.slots.get_mut(slot as usize) else {
            return Err(IpcError::InvalidOperation);
        };
        if entry.is_some() {
            return Err(IpcError::WaiterConflict);
        }
        *entry = Some(capability);
        self.len += 1;
        if self.len > self.peak {
            self.peak = self.len;
        }
        Ok(())
    }

    pub fn get(&self, slot: u32) -> Option<CapabilityEntry> {
        self.slots.get(slot as usize).copied().flatten()
    }

    pub fn drop_slot(&mut self, slot: u32) -> bool {
        let Some(entry) = self.slots.get_mut(slot as usize) else {
            return false;
        };
        if entry.take().is_some() {
            self.len -= 1;
            true
        } else {
            false
        }
    }

    pub fn free_slot_from(&self, from: u32) -> Option<u32> {
        (from as usize..MAX_TASK_CAPS)
            .find(|index| self.slots[*index].is_none())
            .map(|index| index as u32)
    }

    pub fn resolve_executable(
        &self,
        slot: u32,
        required: u64,
    ) -> Result<ExecutableCapability, IpcError> {
        match self.get(slot) {
            Some(CapabilityEntry::Executable(cap)) if cap.rights.allows(required) => Ok(cap),
            _ => Err(IpcError::InvalidOperation),
        }
    }

    pub fn resolve_supervision(
        &self,
        slot: u32,
        required: u64,
    ) -> Result<SupervisionCapability, IpcError> {
        match self.get(slot) {
            Some(CapabilityEntry::Supervision(cap)) if cap.rights.allows(required) => Ok(cap),
            _ => Err(IpcError::InvalidOperation),
        }
    }

    pub fn resolve_block(&self, slot: u32, required: u64) -> Result<BlockCapability, IpcError> {
        match self.get(slot) {
            Some(CapabilityEntry::Block(cap)) if cap.rights.allows(required) => Ok(cap),
            _ => Err(IpcError::InvalidOperation),
        }
    }

    pub fn resolve_directory(
        &self,
        slot: u32,
        required: u64,
    ) -> Result<DirectoryCapability, IpcError> {
        match self.get(slot) {
            Some(CapabilityEntry::Directory(cap)) if cap.rights.allows(required) => Ok(cap),
            _ => Err(IpcError::InvalidOperation),
        }
    }

    pub fn resolve_input(&self, slot: u32) -> Result<InputCapability, IpcError> {
        match self.get(slot) {
            Some(CapabilityEntry::Input(cap)) if cap.rights.allows(RIGHT_INPUT_READ) => Ok(cap),
            _ => Err(IpcError::InvalidOperation),
        }
    }
}

impl Default for AuthorityTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthorityTable, CapabilityEntry, MAX_TASK_CAPS, RIGHT_BLOCK_READ, RIGHT_BLOCK_WRITE,
        RIGHT_LIFECYCLE_RESTART, RIGHT_PARAMETER_READ, RIGHT_PARAMETER_WRITE,
        RIGHT_SCHEDULING_PROMOTE, RIGHT_SUPERVISE,
    };
    use crate::ipc::IpcError;
    use crate::task::TaskId;
    use boot_contracts::generation::{
        RIGHT_BOOT_UPDATE, RIGHT_DMA_PIN, RIGHT_DMA_RELEASE, RIGHT_HEALTH_CONFIRM, RIGHT_IRQ_ACK,
        RIGHT_MAP_MMIO, RIGHT_STORE_READ, RIGHT_STORE_WRITE,
    };

    #[test]
    fn typed_rights_refuse_cross_kind_bits() {
        assert!(CapabilityEntry::block(0, RIGHT_BLOCK_READ).is_some());
        assert!(CapabilityEntry::block(0, RIGHT_BLOCK_READ | (1 << 23)).is_none());
    }

    #[test]
    fn supervision_carries_the_root_only_rights_no_manifest_can_declare() {
        let root_only = RIGHT_SUPERVISE
            | RIGHT_SCHEDULING_PROMOTE
            | RIGHT_LIFECYCLE_RESTART
            | RIGHT_PARAMETER_READ
            | RIGHT_PARAMETER_WRITE;
        assert!(CapabilityEntry::supervision(TaskId(0), root_only).is_some());

        for dead in [
            RIGHT_MAP_MMIO,
            RIGHT_DMA_PIN,
            RIGHT_DMA_RELEASE,
            RIGHT_IRQ_ACK,
            RIGHT_STORE_READ,
            RIGHT_STORE_WRITE,
            RIGHT_HEALTH_CONFIRM,
            RIGHT_BOOT_UPDATE,
        ] {
            assert!(CapabilityEntry::supervision(TaskId(0), RIGHT_SUPERVISE | dead).is_none());
            assert!(CapabilityEntry::block(0, RIGHT_BLOCK_READ | dead).is_none());
        }
    }

    #[test]
    fn a_slot_resolves_only_as_its_declared_kind() {
        let mut table = AuthorityTable::new();
        table
            .install(4, CapabilityEntry::block(0, RIGHT_BLOCK_READ).unwrap())
            .unwrap();
        assert!(table.resolve_block(4, RIGHT_BLOCK_READ).is_ok());
        assert_eq!(
            table.resolve_block(4, RIGHT_BLOCK_WRITE),
            Err(IpcError::InvalidOperation)
        );
        assert_eq!(table.resolve_input(4), Err(IpcError::InvalidOperation));
    }

    #[test]
    fn allocation_stays_bounded_and_non_overwriting() {
        let mut table = AuthorityTable::new();
        let cap = CapabilityEntry::block(0, RIGHT_BLOCK_READ).unwrap();
        table.install(2, cap).unwrap();
        assert_eq!(table.install(2, cap), Err(IpcError::WaiterConflict));
        assert_eq!(
            table.install(MAX_TASK_CAPS as u32, cap),
            Err(IpcError::InvalidOperation)
        );
        assert!(table.drop_slot(2));
        assert_eq!(table.free_slot_from(1), Some(1));
    }
}
