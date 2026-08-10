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

/// Which end of a logical channel a capability names.
///
/// The side lives in the *capability* rather than in the channel entry, and that
/// placement is the whole point: it is what lets an endpoint grant be an
/// ordinary copy. `ChannelTable` used to carry `producer`/`consumer` fields
/// naming the holding tasks, and every queue lookup compared a `TaskId` against
/// them — so a capability alone did not say which queue it reached, a second
/// holder was unrepresentable, and handing an end to a child had to *move* the
/// record. With the side carried here, two tasks may hold the same end and each
/// resolves to the same queue, which is what the retired kernel gets for free by
/// cloning an `Arc<Endpoint>` (`kernel/src/ipc/mod.rs::Clone for Endpoint`).
///
/// Closes backlog **B25**.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    /// Sends on the forward queue, receives on the reverse.
    Producer,
    /// Sends on the reverse queue, receives on the forward.
    Consumer,
    /// Both ends at one slot, for a declared self-edge.
    ///
    /// Not a third end: it resolves to the forward queue in *both* directions,
    /// which is what a task sending to itself must mean, and it is why
    /// `ChannelTable::push` allocates a single queue for such a channel. Only
    /// `materialize` ever creates one — a *minted* pair gets a real side per slot,
    /// so its two halves are distinguishable and separately grantable.
    Loopback,
}

impl Side {
    /// The end facing this one across the channel.
    ///
    /// A loopback faces itself, which is what makes a self-edge deliver what it
    /// sent.
    pub const fn opposite(self) -> Self {
        match self {
            Self::Producer => Self::Consumer,
            Self::Consumer => Self::Producer,
            Self::Loopback => Self::Loopback,
        }
    }

    /// Whether a capability naming `self` reaches `other`.
    ///
    /// A loopback names both real sides. A real side does not reach loopback:
    /// loopback is a holder representation, not a third physical end.
    pub const fn reaches(self, other: Self) -> bool {
        matches!(
            (self, other),
            (Self::Producer, Self::Producer)
                | (Self::Consumer, Self::Consumer)
                | (Self::Loopback, _)
        )
    }

    /// A short name for markers.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Producer => "producer",
            Self::Consumer => "consumer",
            Self::Loopback => "loopback",
        }
    }
}

/// What a logical slot resolves to.
///
/// Deliberately not a seL4 capability: these are the objects `slime-root` owns
/// and mediates. A child never holds one, it names one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resource {
    /// An executable this task may spawn, named by generation executable index.
    Executable { executable: usize },
    /// One end of a logical channel: which channel, and which end of it.
    Endpoint { channel: u32, side: Side },
    /// Authority to mint channel pairs.
    EndpointFactory,
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
    /// Whether this cutover can move the resource between capability tables
    /// **over a channel**, as a `send` attachment.
    ///
    /// Kind, not rights, is what decides *here*: this answers only whether the
    /// root has a mechanism for the move, and today it has one only for a loan.
    ///
    /// Whether the generation *delegated* the move is a separate question,
    /// answered at the mint rather than at the send — `main.rs::serve_buffer_loan`
    /// refuses to create a loan over a channel the generation did not declare
    /// `transferable`, so an undelegated edge carries nothing to test here.
    ///
    /// # Not the same question as a spawn grant
    ///
    /// P5.3.3 hands a child capabilities at construction, and those are of
    /// every kind this returns `false` for — an endpoint end, a factory, a
    /// supervision handle. That is not a contradiction, because the two are
    /// authorized by different things and neither can stand in for the other:
    ///
    /// - a **spawn grant** is a derived copy the parent makes at the moment it
    ///   constructs the child, bounded by `preflight_spawn_grants` to rights
    ///   the parent already holds. The parent is the child's whole reason for
    ///   existing, and the generation authorized the pair by granting the
    ///   parent the executable. Nothing reaches a task the generation did not
    ///   connect to the parent, because the child had no table to reach at all
    ///   until the parent made one.
    /// - a **send attachment** moves a capability to a task that already
    ///   exists, chosen at runtime by whoever is at the other end of a channel.
    ///   That is redistribution of a declared graph, which is why it is
    ///   narrowed to the one kind whose handle names its own recipient.
    ///
    /// So this bound is about the *send* path specifically. Widening it to
    /// endpoints would let a component pass its channel ends around at runtime;
    /// spawn cannot, because a spawn grant's destination is a task that does
    /// not exist yet.
    pub const fn is_transferable(&self) -> bool {
        // A directory joins the loan since P5.4.3, and for the same reason the
        // loan qualified: the move is checkable against the *recipient*.
        //
        // A loan's handle names the receiver it was minted for. A directory
        // carries its own scope and rights, and the root narrows both on every
        // derivation — so a view that arrives over a channel grants exactly
        // what the sender held and no more, whoever the sender chose. That is
        // what M6.3's filesystem service needs: a client hands the service its
        // view with each request, precisely so the service acts with the
        // *client's* authority rather than its own.
        //
        // An endpoint end joins them for M6.4 (P5.4.3), and the earlier
        // reasoning here was wrong rather than merely narrow.
        //
        // The claim was that nothing bounds where an endpoint lands. But
        // nothing bounds where a *loan* lands either — the handle names its
        // receiver, and the send path checks it, which is a check rather than a
        // property of the kind. What actually bounds every move on this path is
        // the same thing: the sender must hold `RIGHT_TRANSFER` on the
        // capability, which the generation grants or a parent narrows at spawn.
        // The oracle's `sys_send` gates on exactly that bit and no kind
        // predicate, and this port refusing endpoints meant a shell could not
        // give a child its stdin.
        //
        // What still cannot move is an executable or a factory, and for a
        // reason that is not a policy choice: `contracts/capability-transfer`
        // defines no descriptor for either, so there is nothing to send.
        matches!(
            self,
            Self::Loan { .. } | Self::Directory { .. } | Self::Endpoint { .. }
        )
    }

    /// A short name for markers, so a refusal states which kind was named
    /// without leaking the handle behind it.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Executable { .. } => "executable",
            Self::Endpoint { .. } => "endpoint",
            Self::EndpointFactory => "endpoint-factory",
            Self::SharedBufferFactory => "shared-buffer-factory",
            Self::Block { .. } => "block",
            Self::Supervision { .. } => "supervision",
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

    /// Whether this table names any end of `channel`, at any side.
    ///
    /// Side-agnostic on purpose: reclamation asks whether the channel is still
    /// reachable at all, not from which direction.
    pub fn names_endpoint(&self, channel: u32) -> bool {
        self.slots.iter().flatten().any(|capability| {
            matches!(capability.resource, Resource::Endpoint { channel: key, .. } if key == channel)
        })
    }

    /// Whether this table holds a capability that reaches `side` of `channel`.
    ///
    /// See [`Side::reaches`]: a loopback slot satisfies a query for either real
    /// side, because it names both ends.
    pub fn reaches_endpoint(&self, channel: u32, side: Side) -> bool {
        self.slots.iter().flatten().any(|capability| {
            matches!(
                capability.resource,
                Resource::Endpoint { channel: key, side: held }
                    if key == channel && held.reaches(side)
            )
        })
    }

    /// How many distinct channels this table names an end of.
    ///
    /// Feeds the `peer death task=N channels=M` marker, which counted
    /// `ChannelTable` entries by holder before B25 removed the holder fields. A
    /// task holding *both* ends of one channel — a minted pair it has not granted
    /// away — counts once, matching what the old per-entry filter reported.
    ///
    /// Counts a key at the first slot naming it, so duplicates are skipped without
    /// a set: quadratic over 64 slots on a death path, against one more fixed-size
    /// bound to keep in step with `MAX_TASK_CAPS`.
    pub fn endpoints_held(&self) -> usize {
        let channel_at = |index: usize| match self.slots.get(index)?.as_ref()?.resource {
            Resource::Endpoint { channel, .. } => Some(channel),
            _ => None,
        };
        (0..MAX_TASK_CAPS)
            .filter(|index| {
                let Some(channel) = channel_at(*index) else {
                    return false;
                };
                !(0..*index).any(|earlier| channel_at(earlier) == Some(channel))
            })
            .count()
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

    /// Whether any live table holds an endpoint capability naming `channel`.
    ///
    /// The live half of the predicate [`crate::channel::sweep`] uses to decide
    /// whether a channel entry can still be named. The sibling of
    /// [`Self::holds_supervision`], and a scan for the same reason: a channel
    /// end is placed at materialization, copied at spawn, moved by
    /// `cap_transfer`, and dropped by `CapDrop` — four paths an index would
    /// have to stay correct across.
    ///
    /// Bounded by `MAX_GRAPH_TASKS * MAX_TASK_CAPS`, and run only when
    /// `ChannelTable` is full, so the cost is paid once per channel reclaimed
    /// rather than per mint.
    pub fn holds_endpoint(&self, channel: u32) -> bool {
        self.tables
            .iter()
            .flatten()
            .any(|(_, table)| table.names_endpoint(channel))
    }

    /// Whether any live table other than `except` reaches one particular side
    /// of `channel`.
    ///
    /// The exclusion is what a death path needs: the dying task's table is
    /// deliberately still installed while its queues and parked transfers are
    /// reclaimed, so counting it would make every end look live until too late.
    pub fn holds_endpoint_side(&self, channel: u32, side: Side, except: Option<TaskId>) -> bool {
        self.tables
            .iter()
            .flatten()
            .any(|(id, table)| Some(*id) != except && table.reaches_endpoint(channel, side))
    }

    /// The unique live task holding the requested side, excluding `except`.
    ///
    /// Used where the object being minted records a concrete task identity, as
    /// a `LoanHandle` does. A shared channel end names a queue, not one of its
    /// competing receivers, so ambiguity is refused rather than resolved by
    /// table order.
    pub fn unique_holder_of_endpoint_side(
        &self,
        channel: u32,
        side: Side,
        except: Option<TaskId>,
    ) -> Option<TaskId> {
        let mut holders = self.tables.iter().flatten().filter_map(|(id, table)| {
            (Some(*id) != except && table.reaches_endpoint(channel, side)).then_some(*id)
        });
        let holder = holders.next()?;
        holders.next().is_none().then_some(holder)
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
    use super::{Capability, CapabilityTable, GraphTables, MAX_TASK_CAPS, Resource, Side};
    use crate::ipc::IpcError;
    use crate::task::TaskId;

    const RIGHT_SEND: u64 = 1;
    const RIGHT_RECV: u64 = 1 << 1;
    const RIGHT_EXEC: u64 = 1 << 3;

    fn endpoint(rights: u64) -> Capability {
        Capability {
            resource: Resource::Endpoint {
                channel: 7,
                side: Side::Producer,
            },
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
    fn one_task_cannot_see_another_tasks_capabilities() {
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
