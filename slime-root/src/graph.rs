//! Per-task logical capability tables: what a component's grants actually are.
//!
//! A Slime capability is a *logical slot number* the root task interprets, not a
//! seL4 capability in the child's CSpace. The whole operation surface takes
//! `u32` slots — `spawn(executable_slot, …)`, `endpoint_create(factory_slot)`,
//! `send(slot, …)` — and `ipc::decode_request` refuses a message carrying real
//! seL4 extra-caps outright. So a child CSpace holds only the capabilities
//! `task.rs` installs (null, service endpoint, own TCB, fault endpoint), and
//! every grant the generation declares lives here instead.
//!
//! That is why `init`'s boot layout can reach slot 18 while the child's CSpace
//! holds four capabilities: the layout numbers *logical* slots, and the root
//! resolves each one to the object it names. A component cannot forge a slot it
//! was not granted, because the resolution is a table lookup here rather than a
//! CSpace address the child controls.
//!
//! The CSpace itself is sized from the generation's admitted plan (B40), not
//! from a constant, so its size and this table's size are independent.
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
///
/// [`crate::task::MAX_TASKS`], so this is no longer a *second* ceiling on how
/// many tasks a graph may hold. It was 16 against that 32, which made this
/// table exhaustible before the task table — and `construct_child` reserves an
/// entry here only after the child's frames, CNode, and TCB are allocated, so
/// running out produces a mid-construction unwind rather than a bounded refusal
/// at the point of allocation.
///
/// P5.5.2's stream plane is what made the margin worth closing: it holds
/// **13** tables at peak — seven components the root launches from the
/// generation plus six children init spawns — against the old 16. That is
/// tighter than the `MAX_CHANNELS` margin which had already broken, and driven
/// by the same growth: route roles, not task pairs.
///
/// Costs ~48 KiB of additional stack in `launch_component_graph`, where
/// `GraphTables` is a local. That is affordable against the root's 1 MiB stack
/// and was checked rather than assumed — backlog B3 records a silent overflow
/// from exactly this kind of growth, which is why the *channel* table lives in
/// a `static` instead.
pub const MAX_GRAPH_TASKS: usize = crate::task::MAX_TASKS;

/// What a logical slot resolves to.
///
/// Deliberately not a seL4 capability: these are the objects `slime-root` owns
/// and mediates. A child never holds one, it names one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resource {
    /// An executable this task may spawn, named by generation executable index.
    Executable { executable: usize },
    /// Authority to allocate shared buffers.
    SharedBufferFactory,
    /// Authority over the block device (P5.4.2c).
    ///
    /// Singular: this cutover brings up at most one, so the resource names no
    /// index. What distinguishes read from write authority is the capability's
    /// *rights*, exactly as the generation declares them — `blockRead` and
    /// `blockWrite` are separate bits, and a grant carrying only the first
    /// cannot reach `OP_WRITE`.
    Block {
        /// Which brought-up device, in the root's stable physical-address
        /// order (P5.4.3).
        ///
        /// In the capability, not the request: M6.7 hands one component a
        /// source it may only read and a receiver it may write, and an index
        /// the *caller* supplied would let either reach the other.
        device: u8,
    },
    /// A supervision handle for a spawned child.
    Supervision { task: TaskId },
    /// A native seL4 endpoint capability imported from a peer.
    ///
    /// The actual kernel capability lives in the importer's CSpace; this
    /// logical record exists only so root-side accounting can name its kind and
    /// rights without reintroducing a mediated message path.
    NativeEndpoint,
    /// A shared buffer this task holds.
    ///
    /// The `BufferHandle` the table issues carries rights and a generation
    /// epoch, so it is authority rather than a name. It stays here, in root
    /// memory; the component receives only the slot number, and every operation
    /// on the region resolves back to this record. That is what stops a
    /// component from widening its own rights by editing a handle it holds.
    SharedBuffer {
        handle: crate::shared_buffer::BufferHandle,
    },
    /// One outstanding loan of a sealed subrange, held by its receiver.
    ///
    /// The only resource kind this cutover moves between tasks. A
    /// [`LoanHandle`](crate::shared_buffer::LoanHandle) names the receiver it
    /// was minted for, and the table refuses a claim from anyone else, so the
    /// move is a transfer of a name the recipient can already be checked
    /// against rather than a widening of authority.
    Loan {
        handle: crate::shared_buffer::LoanHandle,
    },
    /// A scoped view of one shared filesystem namespace (M6.3, P5.4.3).
    ///
    /// Two fields, and the split between them is the whole design. `namespace`
    /// names a root the tasks holding it *share* — committing through one is
    /// visible through every other — while `scope` is this capability's own
    /// view of it, a bounded relative path that derivation may only lengthen.
    ///
    /// The scope is what makes the authority narrow: a holder of `docs` cannot
    /// name `..`, cannot reach a sibling, and — because a commit requires an
    /// *unscoped* writer — cannot replace the namespace-wide root with a
    /// subtree snapshot. Rights (`directoryRead`, `directoryWrite`,
    /// `directoryList`, `directoryDerive`) narrow it further and independently.
    ///
    /// The root owns this because it is unforgeable shared state with an atomic
    /// transition, which is mechanism. What a directory *contains* — entries,
    /// names, object identities — is a filesystem component's business, built
    /// over the object store, and none of it is here.
    /// Authority to read decoded key events (M6.4, P5.4.3).
    ///
    /// Singular and indexless: this cutover has one scripted source. Mechanism
    /// rather than policy for the same reason the block device is — the events
    /// come from somewhere a component cannot reach — and just as thin: what a
    /// key *means* is Dango's business.
    Input,
    Directory {
        namespace: u32,
        /// An index into [`ScopeTable`], not the path itself.
        ///
        /// A `Resource` is copied into every capability slot, and there are
        /// `MAX_TASKS * MAX_CAPS` of them — so inlining a 128-byte path here
        /// grew the capability tables from ~96 KiB to ~432 KiB and cost the
        /// root its stack. Measured, not guessed: the loan plane started
        /// faulting on `init`'s exit path the moment the variant landed.
        ///
        /// Interning also makes the common case free. Almost every directory
        /// capability is unscoped, and every unscoped one shares
        /// [`ScopeTable::ROOT`].
        scope: ScopeId,
    },
}

/// A handle naming a path in the [`ScopeTable`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeId(u16);

/// The interned directory scopes.
///
/// Append-only for the life of a graph, like every other table here. Scopes are
/// created by derivation and a derivation is a deliberate act, so the table
/// grows with composition rather than with traffic; exhausting it refuses the
/// derive rather than dropping a path.
pub struct ScopeTable {
    paths: [DirectoryScope; MAX_SCOPES],
    len: usize,
}

/// Distinct scopes one boot may name. Generous against the deepest composition
/// any plane builds, and bounded because the root allocates nothing.
pub const MAX_SCOPES: usize = 64;

impl Default for ScopeTable {
    fn default() -> Self {
        Self::new()
    }
}

impl ScopeTable {
    /// The unscoped view, interned at index 0 so it needs no lookup and every
    /// namespace-root capability shares it.
    pub const ROOT: ScopeId = ScopeId(0);

    pub const fn new() -> Self {
        Self {
            paths: [DirectoryScope::root(); MAX_SCOPES],
            len: 1,
        }
    }

    pub fn path(&self, id: ScopeId) -> &[u8] {
        self.paths
            .get(id.0 as usize)
            .map_or(&[][..], DirectoryScope::path)
    }

    pub fn is_root(&self, id: ScopeId) -> bool {
        id == Self::ROOT || self.path(id).is_empty()
    }

    /// Intern the result of extending `base` by `relative`.
    ///
    /// `None` when the joined path is not valid or the table is full. An
    /// existing identical scope is reused, so a plane that derives the same
    /// view twice consumes one entry.
    pub fn derive(&mut self, base: ScopeId, relative: &[u8]) -> Option<ScopeId> {
        let derived = self.paths.get(base.0 as usize)?.derive(relative)?;
        if let Some(index) = self.paths[..self.len]
            .iter()
            .position(|existing| *existing == derived)
        {
            return Some(ScopeId(index as u16));
        }
        if self.len >= MAX_SCOPES {
            return None;
        }
        let index = self.len;
        self.paths[index] = derived;
        self.len = index + 1;
        Some(ScopeId(index as u16))
    }
}

/// A bounded relative path naming a capability's view into a namespace.
///
/// Inline rather than heap: a capability is copied on every spawn grant and
/// derivation, and the root has no allocator. `MAX_DIRECTORY_PATH` matches the
/// oracle's bound so a component's buffer sizing is the same on both.
/// One interned path. Held only by the [`ScopeTable`]; capabilities carry a
/// [`ScopeId`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryScope {
    bytes: [u8; MAX_DIRECTORY_PATH],
    len: u8,
}

/// The longest scope a capability may carry, matching the oracle's
/// `capability::MAX_DIRECTORY_PATH`.
pub const MAX_DIRECTORY_PATH: usize = 128;
/// The deepest path a scope may name, matching the oracle's
/// `capability::MAX_DIRECTORY_DEPTH`.
pub const MAX_DIRECTORY_DEPTH: usize = 8;

impl DirectoryScope {
    /// The unscoped view: the namespace root itself, and the only view a commit
    /// may be made through.
    pub const fn root() -> Self {
        Self {
            bytes: [0; MAX_DIRECTORY_PATH],
            len: 0,
        }
    }

    pub fn path(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    pub const fn is_root(&self) -> bool {
        self.len == 0
    }

    /// Extend this scope by a relative path.
    ///
    /// Lengthen only: there is no operation that shortens a scope, which is why
    /// derivation cannot widen authority. The joined result is re-validated as
    /// a whole rather than only the suffix, so a pair of individually valid
    /// halves cannot compose into an over-long or over-deep path.
    pub fn derive(&self, relative: &[u8]) -> Option<Self> {
        if !valid_directory_path(relative, true) {
            return None;
        }
        let current = self.len as usize;
        let separator = usize::from(current != 0 && !relative.is_empty());
        let total = current
            .checked_add(separator)?
            .checked_add(relative.len())?;
        if total > MAX_DIRECTORY_PATH {
            return None;
        }
        let mut bytes = [0u8; MAX_DIRECTORY_PATH];
        bytes[..current].copy_from_slice(&self.bytes[..current]);
        if separator != 0 {
            bytes[current] = b'/';
        }
        bytes[current + separator..total].copy_from_slice(relative);
        if !valid_directory_path(&bytes[..total], true) {
            return None;
        }
        Some(Self {
            bytes,
            len: total as u8,
        })
    }
}

/// Whether `path` is a legal relative directory path.
///
/// The same rule the oracle's `capability::valid_directory_path` applies, and
/// for the same reason: a path is validated *before* it reaches a filesystem
/// component, so no component has to defend itself against `..`, an absolute
/// path, an empty segment, or an unbounded depth.
pub fn valid_directory_path(path: &[u8], allow_empty: bool) -> bool {
    if path.is_empty() {
        return allow_empty;
    }
    if path.len() > MAX_DIRECTORY_PATH || path[0] == b'/' || path[path.len() - 1] == b'/' {
        return false;
    }
    let mut depth = 0;
    for segment in path.split(|byte| *byte == b'/') {
        if segment.is_empty()
            || segment == b"."
            || segment == b".."
            || !segment
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-'))
        {
            return false;
        }
        depth += 1;
        if depth > MAX_DIRECTORY_DEPTH {
            return false;
        }
    }
    true
}

impl Resource {
    /// A stable one-word name for serial evidence. Not a wire format: it names
    /// the kind for a reader, and carries none of the resource's identity.
    pub const fn kind_name(self) -> &'static str {
        match self {
            Self::Executable { .. } => "executable",
            Self::SharedBufferFactory => "shared-buffer-factory",
            Self::Block { .. } => "block",
            Self::Supervision { .. } => "supervision",
            Self::NativeEndpoint => "endpoint",
            Self::SharedBuffer { .. } => "shared-buffer",
            Self::Loan { .. } => "loan",
            Self::Input => "input",
            Self::Directory { .. } => "directory",
        }
    }

    /// Whether a root-mediated capability may cross a capability-update ticket.
    pub const fn is_transferable(&self) -> bool {
        matches!(self, Self::Loan { .. } | Self::Directory { .. })
    }

    /// A short name for markers, so a refusal states which kind was named
    /// without leaking the handle behind it.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Executable { .. } => "executable",
            Self::SharedBufferFactory => "shared-buffer-factory",
            Self::Block { .. } => "block",
            Self::Supervision { .. } => "supervision",
            Self::NativeEndpoint => "endpoint",
            Self::SharedBuffer { .. } => "shared-buffer",
            Self::Loan { .. } => "loan",
            Self::Directory { .. } => "directory",
            Self::Input => "input",
        }
    }
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

    /// Every slot in numbering order, filled or not.
    ///
    /// For the boot-layout dump (B10): a layout is a statement about *which*
    /// numbers hold what, so the empty ones are part of the shape and an
    /// iterator that skipped them would report a different table.
    pub fn slots(&self) -> impl Iterator<Item = (u32, Option<&Capability>)> {
        self.slots
            .iter()
            .enumerate()
            .map(|(slot, entry)| (slot as u32, entry.as_ref()))
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
            return Err(IpcError::DestinationSlotsExhausted);
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

    /// Whether any live table holds a supervision handle naming `task`.
    ///
    /// The live half of the predicate
    /// [`crate::supervision::sweep`] uses to decide whether a termination
    /// record can still be observed. It scans rather than consulting an index
    /// because a supervision handle moves — it is granted at spawn, may be
    /// transferred, and is dropped when its outcome is collected — so any index
    /// would be one more thing to keep correct across all three paths.
    ///
    /// Bounded by `MAX_GRAPH_TASKS * MAX_TASK_CAPS`, and run only when
    /// `Terminations` is full, so the cost is paid once per record reclaimed
    /// rather than per spawn.
    pub fn holds_supervision(&self, task: TaskId) -> bool {
        self.tables.iter().flatten().any(|(_, table)| {
            table
                .slots
                .iter()
                .flatten()
                .any(|capability| capability.resource == Resource::Supervision { task })
        })
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

    const RIGHT_READ: u64 = 1;
    const RIGHT_WRITE: u64 = 1 << 1;
    const RIGHT_EXEC: u64 = 1 << 3;

    fn capability(rights: u64) -> Capability {
        Capability {
            resource: Resource::Block { device: 0 },
            rights,
        }
    }

    #[test]
    fn a_slot_resolves_only_with_the_rights_it_was_granted() {
        let mut table = CapabilityTable::new();
        table.install(4, capability(RIGHT_READ)).unwrap();

        assert!(table.resolve(4, RIGHT_READ).is_ok());
        assert_eq!(
            table.resolve(4, RIGHT_READ | RIGHT_WRITE),
            Err(IpcError::InvalidOperation),
            "a narrower grant cannot satisfy a wider requirement"
        );
    }

    #[test]
    fn an_ungranted_slot_is_indistinguishable_from_an_underpowered_one() {
        let mut table = CapabilityTable::new();
        table.install(4, capability(RIGHT_READ)).unwrap();
        // Both are the same error, so a component cannot probe the table.
        assert_eq!(
            table.resolve(9, RIGHT_READ),
            table.resolve(4, RIGHT_EXEC),
            "an absent slot and an insufficient one report identically"
        );
    }

    #[test]
    fn a_layout_cannot_fill_one_slot_twice() {
        let mut table = CapabilityTable::new();
        table.install(2, capability(RIGHT_READ)).unwrap();
        assert_eq!(
            table.install(2, capability(RIGHT_WRITE)),
            Err(IpcError::WaiterConflict),
            "two grants at one slot is a layout defect, not a last-wins merge"
        );
        // The original grant is intact.
        assert_eq!(table.get(2), Some(capability(RIGHT_READ)));
    }

    #[test]
    fn a_slot_past_the_table_is_refused_rather_than_wrapping() {
        let mut table = CapabilityTable::new();
        assert_eq!(
            table.install(MAX_TASK_CAPS as u32, capability(RIGHT_READ)),
            Err(IpcError::InvalidOperation)
        );
        assert_eq!(table.get(MAX_TASK_CAPS as u32), None);
    }

    #[test]
    fn dropping_a_slot_frees_it_for_runtime_minted_authority() {
        let mut table = CapabilityTable::new();
        table.install(0, capability(RIGHT_READ)).unwrap();
        table.install(1, capability(RIGHT_READ)).unwrap();
        assert_eq!(table.free_slot_from(0), Some(2));

        assert!(table.drop_slot(1));
        assert_eq!(table.len(), 1);
        assert_eq!(table.free_slot_from(0), Some(1));
        assert!(!table.drop_slot(1), "dropping twice is not a drop");
    }

    #[test]
    fn one_task_cannot_see_another_tasks_capabilities() {
        let mut graph = GraphTables::new();
        graph
            .create(TaskId(0))
            .unwrap()
            .install(3, capability(RIGHT_READ))
            .unwrap();
        graph.create(TaskId(1)).unwrap();

        assert!(graph.get(TaskId(0)).unwrap().resolve(3, RIGHT_READ).is_ok());
        assert_eq!(
            graph.get(TaskId(1)).unwrap().resolve(3, RIGHT_READ),
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
            .install(3, capability(RIGHT_READ))
            .unwrap();
        assert!(graph.release(TaskId(0)));
        assert!(graph.get(TaskId(0)).is_none());
        assert!(!graph.release(TaskId(0)));
        assert_eq!(graph.len(), 0);
    }
}
