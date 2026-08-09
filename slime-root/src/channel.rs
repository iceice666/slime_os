//! Logical channels materialized from the generation's declared grants.
//!
//! A Slime channel is a root-owned queue, not a seL4 endpoint. Components name
//! one by logical slot number — `send(slot, ..)`, `recv(slot, ..)` — and the
//! root resolves that slot through [`crate::graph::CapabilityTable`] to a
//! [`ChannelKey`] here. No child CSpace ever holds a channel capability, so a
//! component cannot forge, widen, or transfer one; every operation on a channel
//! is a table lookup the root performs against authority the generation stated.
//!
//! # Direction
//!
//! A grant's `rights` list describes what its **target** may do with the
//! channel, so `rights = ["send"]` makes the target the producer and
//! `rights = ["recv"]` makes it the consumer. The source holds the other end.
//!
//! That is what the retired kernel's manifest already means:
//! `contracts/generation/v1/fixtures/sel4.zti` declares `console-output` as
//! `source = "console"; target = "spawn-service"; rights = ["send"]`, and
//! `spawn-service` is indeed the one that writes to the console while `console`
//! is the one blocked in `recv`. Reading `target` as "the producer" regardless
//! of which right is named gets this exactly backwards for a `recv` grant, and
//! produces a graph that materializes cleanly and then deadlocks — both ends
//! waiting to receive.
//!
//! A grant naming **both** `send` and `recv` is bidirectional: one logical
//! channel carrying two directed queues, addressed by one slot at each end. The
//! retired kernel does the same — `spawn-service-rpc` is placed as the two
//! halves `dango-spawn`/`service-spawn`, both `RIGHT_SEND | RIGHT_RECV`, from a
//! single `ipc::channel()`. It has to be one slot number in both directions
//! because `spawn-service` does `recv(RPC_SLOT)` and then replies on that same
//! slot.
//!
//! A bidirectional grant whose two ends are the *same* component is a loopback,
//! and that is one queue rather than two: a task sending to itself must receive
//! what it sent. A second queue would have no distinct peer end that could
//! reach it.
//!
//! # Slot numbering
//!
//! Two numbering rules preserve the slot contracts the component binaries
//! compile against:
//!
//! - The **bootstrap component** takes its slots from the generation's
//!   boot-layout resource, the way `LayoutPlacer` does in the retired kernel.
//!   `init.rs` addresses every slot through a constant generated from that same
//!   layout, so the number it compiles against and the number filled here are
//!   one number by construction.
//! - Every **other** component takes its first channel at slot 0 and further
//!   channels above its executable grants. Slot 0 is what `console.rs`,
//!   `spawn-service.rs::RPC_SLOT`, and `launch_context::CONTEXT_SLOT` all
//!   already address, and executables keep the `1..=N` numbering
//!   `launch_component_graph` established in P5.2.
//!
//! # What this slice does not place, and why
//!
//! A grant naming the bootstrap component whose channel the layout does *not*
//! name is **skipped and counted**, not guessed at and not fatal. Two facts
//! make that the honest answer rather than a shortcut:
//!
//! - A grant's name and a layout's channel label are different namespaces. The
//!   layout labels the two *halves* of a channel — `dango-spawn` and
//!   `service-spawn` — while the generation names the *grant* that authorizes
//!   it, `spawn-service-rpc`. Nothing maps one onto the other; the retired
//!   kernel hardcodes the correspondence in `bootstrap.rs`, which is not data
//!   this root task can read.
//! - In the retired kernel `init` is a **broker**: it holds both halves of every
//!   channel it mints and hands one to each child in that child's spawn grant
//!   list, which is also what fixes the child's slot numbers.
//!
//! So a channel whose slot this slice cannot know is left unmaterialized and
//! reported in [`Materialized::unplaced`], rather than installed at a number the
//! component never compiled against. A component then finds either the channel
//! its generation declared or nothing at all — never someone else's.
//!
//! **P5.3.3 does not change that rule, and deliberately.** Spawn now exists, so
//! init *can* broker a half — but it brokers one it already holds, handed on as
//! a spawn grant that lands at a slot the child's own `0..n` numbering fixes.
//! Nothing about that needs the layout to have labelled the grant, so the
//! unplaced count stays what it was: a statement that this root cannot map a
//! grant name onto a layout channel label, which is still true.

use boot_contracts::boot_layout::{BootLayout, Role, channel_identity};
use boot_contracts::generation::Generation;

use crate::generation::{RIGHT_RECV, RIGHT_SEND};
use crate::graph::{self, GraphTables, Side};
use crate::ipc::{Channel, ChannelKey, IpcError};
use crate::task::TaskId;
use crate::transit::Transit;

/// Authority to observe a spawned child's termination; see
/// `main.rs::RIGHT_SUPERVISE`, which this must agree with.
const RIGHT_SUPERVISE: u64 = 1 << 18;

/// Logical channels one generation's graph may declare.
///
/// Each carries up to two [`Channel`]s, and each of those is a fixed-depth
/// queue of `CHANNEL_CAPACITY` × `MAX_MESSAGE_BYTES` messages — so this bound
/// multiplies out to a table measured in tens of kilobytes. That is why
/// `main.rs` holds the table in a `static` rather than constructing it on the
/// root task's stack: building a table this size in a stack frame overflows it
/// silently, which is exactly the failure backlog B3 records for the retired
/// kernel's `SharedBufferTable`.
///
/// **Not one per task pair.** That was the original reasoning, and P5.5.2's
/// stream plane disproved it: a channel is created per *edge*, and a userspace
/// broker mints edges the generation never declared. The stream graph's thirteen
/// declared grants become six control channels, and `fabric-service` then mints
/// two more per publisher (data, credit) and two per subscriber (data, ack) —
/// ten for a two-publisher, two-subscriber graph. Sixteen wedged it: the fabric
/// failed its eleventh `endpoint_create`, and every participant then failed
/// downstream of that, which reads as four broken components rather than one
/// exhausted table.
///
/// Thirty-two was `task::MAX_TASKS`, chosen because the growth is driven by
/// route roles rather than by task pairs. P5.4.9 disproved that too, in the
/// same direction P5.5.2 disproved the first bound: the C8.10 full-graph boot
/// runs every plane at once and needs **thirty-seven** live at its peak — 16
/// participant controls, 14 stream role channels, 3 call, and 4 operation — so
/// a bound tracking the task count is again tracking the wrong quantity.
///
/// Forty-eight, with headroom rather than exactly 37, for B28's reason: a bound
/// raised to the first passing number is a bound that moves again at the next
/// graph, and the failure it produces is not a clean refusal — the fabric's
/// eleventh `endpoint_create` fails and every participant downstream of it
/// fails too, which reads as four broken components rather than one exhausted
/// table.
///
/// At ~4.2 KiB per entry (two queues of `CHANNEL_CAPACITY` ×
/// `MAX_MESSAGE_BYTES` messages) this costs about 66 KiB of additional `.bss`
/// over the previous bound, against a root task whose `.bss` is already
/// measured in megabytes. It lives in a `static` for that reason: constructing
/// something this size in a stack frame overflows it silently, exactly the
/// failure backlog B3 records for the retired kernel's `SharedBufferTable`.
///
/// This bounds the channels **live at once**, not the channels a boot may ever
/// mint. [`sweep`] reclaims every entry no live holder can name, which is what
/// closes backlog **B22**: before it, `push` never freed and `key = self.len`
/// meant a long-running graph spent one of these permanently per
/// `endpoint_create`, however short-lived the pair.
pub const MAX_CHANNELS: usize = 48;

/// One logical channel: one or two directed queues.
///
/// Holders are deliberately absent. Each endpoint capability carries its
/// [`Side`], so any number of tables may name either end without changing this
/// entry. That is B25's copy semantics and matches x86's cloned endpoint.
struct Entry {
    key: ChannelKey,
    /// Queue carrying producer → consumer. Always present: every channel has at
    /// least one direction, whichever right named it.
    forward: Option<Channel>,
    /// Queue carrying consumer → producer, present only for a bidirectional
    /// non-loopback channel. A declared loopback uses `forward` both ways.
    reverse: Option<Channel>,
    /// Whether the generation declared this grant `transferable`.
    transferable: bool,
}

impl Entry {
    /// The queue `side` may enqueue onto, if the grant created that direction.
    fn send_queue(&self, side: Side) -> Option<&Channel> {
        match side {
            Side::Producer | Side::Loopback => self.forward.as_ref(),
            Side::Consumer => self.reverse.as_ref(),
        }
    }

    fn send_queue_mut(&mut self, side: Side) -> Option<&mut Channel> {
        match side {
            Side::Producer | Side::Loopback => self.forward.as_mut(),
            Side::Consumer => self.reverse.as_mut(),
        }
    }

    /// The queue `side` may dequeue from: the one its opposite sends on.
    fn recv_queue(&self, side: Side) -> Option<&Channel> {
        match side {
            Side::Consumer | Side::Loopback => self.forward.as_ref(),
            Side::Producer => self.reverse.as_ref(),
        }
    }

    fn recv_queue_mut(&mut self, side: Side) -> Option<&mut Channel> {
        match side {
            Side::Consumer | Side::Loopback => self.forward.as_mut(),
            Side::Producer => self.reverse.as_mut(),
        }
    }

    fn queues_mut(&mut self) -> impl Iterator<Item = &mut Channel> {
        self.forward.iter_mut().chain(self.reverse.iter_mut())
    }

    /// The read-only sibling of [`Self::queues_mut`], for the wedge diagnostic.
    fn queues(&self) -> impl Iterator<Item = &Channel> {
        self.forward.iter().chain(self.reverse.iter())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelError {
    /// More logical channels than [`MAX_CHANNELS`].
    TableFull,
    /// A grant names a component that was not launched.
    UnlaunchedEndpoint,
    /// A slot resolved once and then did not resolve the second time. A
    /// bookkeeping defect in this module, not a property of any generation.
    UnlaidSlot,
    /// The layout's rights for a slot are not the rights the grant declares.
    RightsMismatch { declared: u64, layout: u64 },
    /// Installing the channel into a task's capability table failed.
    Install(IpcError),
}

impl From<IpcError> for ChannelError {
    fn from(error: IpcError) -> Self {
        Self::Install(error)
    }
}

/// What materializing a generation's channel grants produced. Counters rather
/// than a log: the boot marker states them, so a graph that quietly declared
/// fewer edges than intended is visible in the transcript.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Materialized {
    /// Grants that carried a send or receive right between two launched tasks.
    pub grants: usize,
    /// Logical channels created.
    pub channels: usize,
    /// Directed queues created. Exceeds `channels` when a grant is
    /// bidirectional.
    pub queues: usize,
    /// Slots filled. Always twice `channels`, except for a self-edge.
    pub slots: usize,
    /// Grants this slice could not place a slot for — a channel touching the
    /// bootstrap component that the boot layout does not label. See the module
    /// doc: these are the ones `init` brokers through spawn in the retired
    /// kernel. P5.3.3 added spawn without changing this — a brokered half is
    /// one init already holds, handed on at a slot the child's own numbering
    /// fixes, which never needed the layout to have labelled the grant.
    /// Counted rather than silently dropped, so the boot transcript states what
    /// the graph did not get.
    pub unplaced: usize,
}

/// Every logical channel this generation declared, and who holds each end.
pub struct ChannelTable {
    entries: [Option<Entry>; MAX_CHANNELS],
    len: usize,
    /// Next [`ChannelKey`]. Monotonic and never reused, so a key from a
    /// reclaimed channel names nothing rather than something new.
    ///
    /// This was `self.len` until B22. That derivation is only unique while
    /// `len` never decreases: once [`sweep`] frees an entry, the next `push`
    /// would reissue a key some live capability already names, and
    /// `Resource::Endpoint { channel }` is the only handle a component holds —
    /// so an aliased key silently redirects one component's sends into
    /// another's queue. A confused deputy is strictly worse than the
    /// exhaustion the sweep exists to remove, which is why the counter is a
    /// precondition for the sweep rather than tidying beside it.
    next_key: ChannelKey,
    /// Channels ever minted, never decremented.
    ///
    /// Split from `len` for the reason `supervision::Terminations` splits
    /// `recorded`: once entries are reclaimed, `len` measures what is held now,
    /// and a boot's transcript needs what happened. A graph that minted forty
    /// channels and released them all ends at `len == 0`, indistinguishable
    /// from one that never minted any.
    minted: usize,
}

impl ChannelTable {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; MAX_CHANNELS],
            len: 0,
            next_key: 0,
            minted: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// How many channels this boot has created, including ones since reclaimed.
    pub const fn minted(&self) -> usize {
        self.minted
    }

    /// Directed queues that are still usable: something can still name the
    /// channel, and its peer is alive. Reaching zero is what the service loop's
    /// termination condition reads.
    ///
    /// Nameability is part of the question since B25. An entry survives until a
    /// [`sweep`] frees it, and sweeping is lazy-on-full, so a boot that released
    /// every end can still hold entries no capability reaches — queues with no
    /// peer at all rather than a live one. Before B25 the entry cached a task
    /// per end and `mark_dead` killed those queues when that task died, which
    /// hid the difference; a copied end has holders rather than a holder, so the
    /// count has to be derived from the same predicate [`sweep`] uses.
    pub fn live_queues(&self, graph: &GraphTables, transit: &Transit) -> usize {
        self.entries
            .iter()
            .flatten()
            .filter(|entry| graph.holds_endpoint(entry.key) || transit.holds_endpoint(entry.key))
            .flat_map(|entry| entry.forward.iter().chain(entry.reverse.iter()))
            .filter(|queue| queue.peer_alive())
            .count()
    }

    /// The queue `side` may send on over `key`.
    pub fn send_queue(&self, key: ChannelKey, side: Side) -> Option<&Channel> {
        self.entry(key)?.send_queue(side)
    }

    pub fn send_queue_mut(&mut self, key: ChannelKey, side: Side) -> Option<&mut Channel> {
        self.entry_mut(key)?.send_queue_mut(side)
    }

    /// The queue `side` may receive from over `key`.
    pub fn recv_queue(&self, key: ChannelKey, side: Side) -> Option<&Channel> {
        self.entry(key)?.recv_queue(side)
    }

    pub fn recv_queue_mut(&mut self, key: ChannelKey, side: Side) -> Option<&mut Channel> {
        self.entry_mut(key)?.recv_queue_mut(side)
    }

    /// Whether the generation declared this channel's grant `transferable`.
    ///
    /// `None` for a key naming no channel. A component cannot influence this:
    /// it is decided once at materialization from the generation's own field.
    pub fn transferable(&self, key: ChannelKey) -> Option<bool> {
        self.entry(key).map(|entry| entry.transferable)
    }

    /// Mark queues whose last holder died as having a dead peer.
    ///
    /// An end can have more than one holder since B25. A task dying abandons a
    /// side only when no other live table reaches that side; killing the queue
    /// for the first co-holder would strand the survivor on a channel whose
    /// opposite end is still alive.
    ///
    /// The dying task's table is still installed, so every holder query excludes
    /// it explicitly.
    pub fn mark_dead(&mut self, graph: &GraphTables, task: TaskId, wakes: &mut DeathWakes) {
        for entry in self.entries.iter_mut().flatten() {
            let key = entry.key;
            let abandoned = [Side::Producer, Side::Consumer, Side::Loopback]
                .into_iter()
                .filter(|side| {
                    graph
                        .get(task)
                        .is_some_and(|table| table.reaches_endpoint(key, *side))
                })
                .any(|side| !graph.holds_endpoint_side(key, side, Some(task)));
            if !abandoned {
                continue;
            }
            for queue in entry.queues_mut() {
                let batch = queue.mark_peer_dead();
                for index in 0..batch.len() {
                    if let Some(wake) = batch.get(index) {
                        wakes.push(key, wake);
                    }
                }
            }
        }
    }

    fn entry(&self, key: ChannelKey) -> Option<&Entry> {
        self.entries.iter().flatten().find(|entry| entry.key == key)
    }

    fn entry_mut(&mut self, key: ChannelKey) -> Option<&mut Entry> {
        self.entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.key == key)
    }

    /// Mint a bidirectional runtime pair.
    ///
    /// Both queues exist immediately. The caller receives distinct Producer and
    /// Consumer capabilities; no holder transition is needed later to "split"
    /// the pair.
    pub fn mint(&mut self) -> Result<ChannelKey, ChannelError> {
        let (key, _) = self.push(RIGHT_SEND | RIGHT_RECV, true, false)?;
        Ok(key)
    }

    fn push(
        &mut self,
        rights: u64,
        transferable: bool,
        loopback: bool,
    ) -> Result<(ChannelKey, usize), ChannelError> {
        let key = self.next_key;
        // Refused rather than wrapped. A wrapped key would alias a live
        // channel, which is the failure the monotonic counter exists to
        // prevent; `TableFull` is the same bounded refusal the caller already
        // handles, and at one key per `endpoint_create` a `u32` is unreachable
        // for any graph this cutover boots.
        let next_key = key.checked_add(1).ok_or(ChannelError::TableFull)?;
        let slot = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(ChannelError::TableFull)?;
        // A one-directional grant has only producer → consumer. A bidirectional
        // non-loopback has one queue per direction. A declared self-edge uses
        // the forward queue both ways and would make the reverse unreachable.
        let bidirectional = rights == RIGHT_SEND | RIGHT_RECV;
        let forward = Some(Channel::new(key));
        let reverse = (bidirectional && !loopback).then(|| Channel::new(key));
        let queues = usize::from(forward.is_some()) + usize::from(reverse.is_some());
        *slot = Some(Entry {
            key,
            forward,
            reverse,
            transferable,
        });
        self.next_key = next_key;
        self.len += 1;
        self.minted += 1;
        Ok((key, queues))
    }
}

impl Default for ChannelTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Reclaim every channel no live holder can name, returning how many were freed.
///
/// This closes backlog **B22**, and it is [`crate::supervision::sweep`]'s exact
/// shape applied to the second table that never freed. The reasoning carries
/// over unchanged, so only what differs is restated here.
///
/// # The predicate is capability-derived, never task-derived
///
/// A channel is observable exactly when some live
/// [`CapabilityTable`](crate::graph::CapabilityTable) holds a
/// `Resource::Endpoint` naming it, or one is parked in
/// [`Transit`](crate::transit::Transit) mid-transfer. Both halves are required,
/// for B16's reason: `serve_cap_transfer` removes the capability from the
/// sender's table *before* parking it, so between those two steps the end is
/// held by no table at all. A sweep reading only the graph would free the
/// channel a transfer is in the middle of moving, and the receiver would land
/// a capability resolving to no queue.
///
/// No task identity is stored in [`Entry`]. Since B25, each endpoint capability
/// carries its [`Side`], so reachability comes only from live capability tables
/// and in-flight transfers. A task-derived cache would be both redundant and
/// wrong once two tables may name the same end.
///
/// A dead peer is likewise not a reason to free. `mark_dead` marks both queues,
/// and [`ChannelTable::is_ready`] treats a dead peer as *ready* so a parked
/// receive returns `PeerDead` rather than hanging. Freeing an entry whose
/// holder still names it would turn that clean answer into an unresolvable
/// slot.
///
/// # Waits are not consulted
///
/// A registration lives on the queue, not in a table, and a waiter necessarily
/// resolved a held capability one syscall earlier to build its
/// [`WaitTarget`] — so a wait implies a holder rather than adding one. Any
/// registration on a channel this frees belongs to a task whose capability is
/// already gone, and [`ChannelTable::clear_waits`] runs before reclamation on
/// every path that removes one.
pub fn sweep(channels: &mut ChannelTable, graph: &GraphTables, transit: &Transit) -> usize {
    let mut freed = 0;
    for slot in channels.entries.iter_mut() {
        let Some(entry) = slot.as_ref() else {
            continue;
        };
        if graph.holds_endpoint(entry.key) || transit.holds_endpoint(entry.key) {
            continue;
        }
        *slot = None;
        freed += 1;
    }
    // `saturating_sub`, not `-=`, for `supervision::sweep`'s reason: `freed`
    // counts only entries that were `Some` and every one of those was counted
    // in `len`, so the two provably cannot disagree — but this is a `no_std`
    // root task where a wrap would not panic. It would make `len` enormous and
    // `is_empty` permanently false, turning a bookkeeping slip into a boot that
    // misreports its own teardown.
    channels.len = channels.len.saturating_sub(freed);
    freed
}

/// Wakes owed after a task died, each with the channel that produced it.
///
/// Bounded by construction: a task holds at most [`MAX_CHANNELS`] channels and
/// each queue wakes at most one receive waiter and one send waiter.
pub struct DeathWakes {
    entries: [Option<(ChannelKey, crate::ipc::WakeDecision)>; MAX_CHANNELS * 4],
    len: usize,
}

impl DeathWakes {
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_CHANNELS * 4],
            len: 0,
        }
    }

    fn push(&mut self, key: ChannelKey, wake: crate::ipc::WakeDecision) {
        if let Some(slot) = self.entries.get_mut(self.len) {
            *slot = Some((key, wake));
            self.len += 1;
        }
    }

    pub fn drain(&self) -> impl Iterator<Item = (ChannelKey, crate::ipc::WakeDecision)> + '_ {
        self.entries.iter().take(self.len).flatten().copied()
    }
}

impl Default for DeathWakes {
    fn default() -> Self {
        Self::new()
    }
}

/// Where each component's runtime-numbered slots start, and which it takes
/// next. See the module doc's slot rule.
///
/// One fixed-size table rather than a slice of per-task cursors, so the caller
/// records a task's executable count as it stages the task and hands the whole
/// table to [`materialize`] without an intermediate collection.
pub struct SlotCursors {
    entries: [Option<Cursor>; MAX_CHANNELS],
    len: usize,
}

#[derive(Clone, Copy)]
struct Cursor {
    task: TaskId,
    next: u32,
    used_slot_zero: bool,
}

impl SlotCursors {
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_CHANNELS],
            len: 0,
        }
    }

    /// Record that `task` already holds executable grants at slots
    /// `1..=executables`, so its channels are numbered clear of them.
    pub fn declare(&mut self, task: TaskId, executables: u32) -> Result<(), ChannelError> {
        let slot = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(ChannelError::TableFull)?;
        *slot = Some(Cursor {
            task,
            next: executables + 1,
            used_slot_zero: false,
        });
        self.len += 1;
        Ok(())
    }

    fn take(&mut self, task: TaskId) -> Option<u32> {
        let cursor = self
            .entries
            .iter_mut()
            .flatten()
            .find(|cursor| cursor.task == task)?;
        if !cursor.used_slot_zero {
            cursor.used_slot_zero = true;
            return Some(0);
        }
        let slot = cursor.next;
        cursor.next += 1;
        Some(slot)
    }
}

impl Default for SlotCursors {
    fn default() -> Self {
        Self::new()
    }
}

/// Which launched task each generation component index became.
pub struct LaunchedComponents {
    entries: [Option<(usize, TaskId)>; MAX_CHANNELS],
    len: usize,
}

impl LaunchedComponents {
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_CHANNELS],
            len: 0,
        }
    }

    pub fn record(&mut self, component: usize, task: TaskId) -> Result<(), ChannelError> {
        let slot = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(ChannelError::TableFull)?;
        *slot = Some((component, task));
        self.len += 1;
        Ok(())
    }

    pub fn task_for(&self, component: usize) -> Option<TaskId> {
        self.entries
            .iter()
            .flatten()
            .find(|(index, _)| *index == component)
            .map(|(_, task)| *task)
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Every launched component and the task it became, in launch order. The
    /// component index is what resolves back to the generation's own record, so
    /// a caller can reach the declared name rather than only the task id.
    pub fn iter(&self) -> impl Iterator<Item = (usize, TaskId)> + '_ {
        self.entries.iter().flatten().copied()
    }
}

impl Default for LaunchedComponents {
    fn default() -> Self {
        Self::new()
    }
}

/// Create every logical channel the generation's send/recv grants declare, and
/// install both ends into the holding tasks' capability tables.
///
/// Ordering is the generation's grant order, which the builder sorts by
/// `(name, source, target)`, so channel keys are deterministic across boots of
/// one generation.
pub fn materialize(
    generation: &Generation<'_>,
    layout: Option<&BootLayout<'_>>,
    bootstrap: Option<TaskId>,
    launched: &LaunchedComponents,
    channels: &mut ChannelTable,
    graph: &mut GraphTables,
    cursors: &mut SlotCursors,
) -> Result<Materialized, ChannelError> {
    let mut report = Materialized::default();
    for index in 0..generation.grant_count() {
        let Ok(grant) = generation.grant(index) else {
            continue;
        };
        let carries = grant.rights & (RIGHT_SEND | RIGHT_RECV);
        if carries == 0 {
            continue;
        }
        // A grant between components this boot did not launch declares nothing
        // about this graph. Not a defect: a generation may name a component
        // whose payload this root task cannot load, and admission already
        // reports those separately.
        // The rights describe the target, so a `send` grant makes the target
        // the producer and a `recv` grant makes it the consumer. A grant naming
        // both is bidirectional and the assignment is only a labelling of which
        // end is which.
        let (Some(target), Some(source)) = (
            launched.task_for(grant.target),
            launched.task_for(grant.source),
        ) else {
            continue;
        };
        let (producer, consumer) = if carries == RIGHT_RECV {
            (source, target)
        } else {
            (target, source)
        };
        report.grants += 1;

        // One slot per holder. A grant naming one component as both endpoints
        // takes a single slot: installing twice would be refused as a layout
        // defect, and a loopback is not one.
        //
        // A grant this slice cannot place is skipped whole, so placeability is
        // decided before any slot is handed out. Only the bootstrap lookup can
        // fail, and it is a pure table read; a cursor `take` is not, so taking
        // one for the first endpoint and then abandoning the grant would push
        // that component's *next* channel off slot 0 — which is the one slot
        // number `console` and `spawn-service` compile against.
        let second = (consumer != producer).then_some(consumer);
        let mut unplaceable = false;
        for task in core::iter::once(producer).chain(second) {
            // Checked against the rights this end will actually be installed
            // with, not the grant's literal bits. They differ — see `held_rights`
            // — and validating the grant's while installing the derived ones
            // would let the containment check pass for authority the layout
            // never declared, which is exactly the case it exists to catch.
            if bootstrap == Some(task)
                && bootstrap_slot(
                    layout,
                    grant.name,
                    held_rights(grant.rights, task, producer),
                )?
                .is_none()
            {
                unplaceable = true;
            }
        }
        if unplaceable {
            report.unplaced += 1;
            sel4::debug_println!(
                "SLIME_GRAPH channel unplaced grant={} reason=no-layout-slot",
                grant.name,
            );
            continue;
        }

        let loopback = consumer == producer;
        let mut slots = [None; 2];
        for (destination, (task, side)) in slots.iter_mut().zip(
            core::iter::once((
                producer,
                if loopback {
                    Side::Loopback
                } else {
                    Side::Producer
                },
            ))
            .chain(second.map(|task| (task, Side::Consumer))),
        ) {
            let held = held_rights(grant.rights, task, producer);
            let slot = channel_slot(layout, bootstrap, cursors, task, grant.name, held)?
                .ok_or(ChannelError::UnlaidSlot)?;
            *destination = Some((task, slot, held, side));
        }

        let (key, queues) = channels.push(carries, grant.transferable, loopback)?;
        report.channels += 1;
        report.queues += queues;
        sel4::debug_println!(
            "SLIME_GRAPH channel grant={} key={key} producer={} consumer={} queues={queues}",
            grant.name,
            producer.0,
            consumer.0,
        );
        for (task, slot, held, side) in slots.into_iter().flatten() {
            let table = graph
                .get_mut(task)
                .ok_or(ChannelError::UnlaunchedEndpoint)?;
            table.install(
                slot,
                graph::Capability {
                    resource: graph::Resource::Endpoint { channel: key, side },
                    rights: held,
                },
            )?;
            sel4::debug_println!(
                "SLIME_GRAPH channel end task={} slot={slot} key={key} side={} rights={held:#x}",
                task.0,
                side.name(),
            );
            report.slots += 1;
        }
    }
    Ok(report)
}

/// The slot this task addresses this grant's channel by.
/// The rights one end of a channel actually holds.
///
/// Not the grant's literal bits. The grant states what its *target* may do; the
/// other end necessarily holds the complement, and a bidirectional grant gives
/// both ends both. Installing the grant's bits verbatim on both ends would leave
/// the producer of a `recv` grant unable to send on the queue the generation
/// created for exactly that.
fn held_rights(declared: u64, task: TaskId, producer: TaskId) -> u64 {
    if declared & (RIGHT_SEND | RIGHT_RECV) == RIGHT_SEND | RIGHT_RECV {
        RIGHT_SEND | RIGHT_RECV
    } else if task == producer {
        RIGHT_SEND
    } else {
        RIGHT_RECV
    }
}

/// `None` means "this slice cannot know the number", which is a skip rather
/// than a failure; see the module doc.
fn channel_slot(
    layout: Option<&BootLayout<'_>>,
    bootstrap: Option<TaskId>,
    cursors: &mut SlotCursors,
    task: TaskId,
    name: &str,
    rights: u64,
) -> Result<Option<u32>, ChannelError> {
    if bootstrap == Some(task) {
        return bootstrap_slot(layout, name, rights);
    }
    cursors
        .take(task)
        .map(Some)
        .ok_or(ChannelError::UnlaunchedEndpoint)
}

/// The bootstrap component's slot for a singular role, from the boot layout.
///
/// The endpoint and shared-buffer factories carry no name — there is one of
/// each — so they are addressed by role rather than by identity, exactly as
/// `LayoutPlacer::role` does in the retired kernel. `init.rs` reads them
/// through the generated `ENDPOINT_FACTORY_SLOT` and
/// `SHARED_BUFFER_FACTORY_SLOT`, so as with every other slot the number it
/// compiles against and the number filled here are one number.
pub fn bootstrap_role_slot(layout: Option<&BootLayout<'_>>, role: Role) -> Option<u32> {
    let layout = layout?;
    (0..layout.entry_count())
        .filter_map(|index| layout.entry(index))
        .find(|entry| entry.role == role && !entry.role.is_named())
        .map(|entry| entry.slot)
}

/// The bootstrap component's slot for the executable named `component`, from
/// the boot layout.
///
/// The same rule the channel halves follow, applied to the other kind of thing
/// a layout numbers. `init.rs` addresses every executable it spawns through a
/// constant generated from this table — `CONSOLE_SLOT`, `SYSINFO_SLOT` — so the
/// number it compiles against and the number the root fills must be one number.
/// Numbering init's executables `1..=N` from a cursor instead is what P5.2 did,
/// and it happened to agree only because `sel4.zti` grants init no executable at
/// all; the moment one is granted, a cursor puts `sysinfo` at 2 while `init.rs`
/// reads 4, and the spawn resolves to whatever else landed there. That is
/// precisely the positional coupling B10 exists to remove.
///
/// `None` when the layout names no executable for this component, which the
/// caller reports as unplaced rather than guessing a number.
pub fn bootstrap_executable_slot(
    layout: Option<&BootLayout<'_>>,
    component: &str,
    rights: u64,
) -> Result<Option<u32>, ChannelError> {
    let Some(layout) = layout else {
        return Ok(None);
    };
    let identity = boot_contracts::boot_layout::component_identity(component);
    let Some(entry) = (0..layout.entry_count())
        .filter_map(|index| layout.entry(index))
        .filter(|entry| entry.role == Role::Executable)
        .find(|entry| entry.name_identity == identity)
    else {
        return Ok(None);
    };
    // Containment, for the reason `bootstrap_slot` documents below: the layout
    // states what the slot may carry and the grant states what the generation
    // confers, and a grant may not exceed the layout.
    if rights & !entry.rights != 0 {
        return Err(ChannelError::RightsMismatch {
            declared: rights,
            layout: entry.rights,
        });
    }
    Ok(Some(entry.slot))
}

/// The bootstrap component's slot for the channel grant `name` authorizes, from
/// the boot layout.
///
/// `None` when the layout labels no such channel — the grant names a channel
/// `init` brokers through spawn, which this slice does not have. That is not
/// the same as a wrong number, so it is reported as absent rather than as an
/// error.
///
/// When the layout *does* name it, the layout's rights bound what this end may
/// do, and the grant may not exceed them.
///
/// Containment rather than equality, because the two describe different things.
/// A layout entry states the authority the slot carries — and it carries
/// `RIGHT_TRANSFER` for every channel half the retired kernel's `init` brokers,
/// because init hands that half on to a child. A grant states the authority the
/// channel confers on its endpoints. Requiring the two to be equal would demand
/// that every generation restate the layout's delegation bit on a right it is
/// not about, which is how the first version of this check rejected a
/// well-formed graph.
///
/// What must hold is that the generation cannot grant an end more than the
/// layout gives that slot: a grant naming `send | recv` on a slot the layout
/// declares receive-only is a real disagreement between the two readers, and it
/// fails here rather than silently widening init's authority.
fn bootstrap_slot(
    layout: Option<&BootLayout<'_>>,
    name: &str,
    rights: u64,
) -> Result<Option<u32>, ChannelError> {
    let Some(layout) = layout else {
        return Ok(None);
    };
    let identity = channel_identity(name);
    let Some(entry) = (0..layout.entry_count())
        .filter_map(|index| layout.entry(index))
        .filter(|entry| matches!(entry.role, Role::EndpointClient | Role::EndpointService))
        .find(|entry| entry.name_identity == identity)
    else {
        return Ok(None);
    };
    if rights & !entry.rights != 0 {
        return Err(ChannelError::RightsMismatch {
            declared: rights,
            layout: entry.rights,
        });
    }
    Ok(Some(entry.slot))
}

/// A component's `wait` source set, resolved against one task's capability
/// table and this channel table.
///
/// `slime_rt::WaitSource` names a *logical slot*; the root resolves each to the
/// channel it holds and the direction it may use, so a wait naming a slot the
/// task was not granted registers nothing rather than parking on someone else's
/// queue. The kinds are `components/runtime/src/syscall.rs::WaitSource`'s.
pub const WAIT_KIND_ENDPOINT: u64 = 0;
pub const WAIT_KIND_INPUT: u64 = 1;
pub const WAIT_KIND_SUPERVISION: u64 = 2;
pub const WAIT_KIND_SEND_CAPACITY: u64 = 3;

/// Bytes one encoded wait-source record occupies in the transfer window.
pub const WAIT_RECORD_BYTES: usize = 8;

/// One resolved wait source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitTarget {
    /// Ready when this receive end has a message, or its peer has died.
    Receive(ChannelKey, Side),
    /// Ready when this send end has room, or its peer has died.
    SendCapacity(ChannelKey, Side),
    /// Ready when the named child has terminated.
    Supervision(TaskId),
    /// Always ready (M6.4): the key source is a script the root reads
    /// synchronously, so there is nothing to park for.
    Input,
    /// A source this cutover has no mechanism for.
    Unmediated,
}

/// Decode one wait-source record and resolve it against `table`.
pub fn resolve_wait_source(
    record: u64,
    table: &graph::CapabilityTable,
) -> Result<WaitTarget, IpcError> {
    let kind = record >> 32;
    let slot = (record & 0xffff_ffff) as u32;
    let endpoint = |required: u64| -> Result<(ChannelKey, Side), IpcError> {
        match table.resolve(slot, required)?.resource {
            graph::Resource::Endpoint { channel, side } => Ok((channel, side)),
            _ => Err(IpcError::InvalidOperation),
        }
    };
    Ok(match kind {
        WAIT_KIND_ENDPOINT => {
            let (channel, side) = endpoint(RIGHT_RECV)?;
            WaitTarget::Receive(channel, side)
        }
        WAIT_KIND_SEND_CAPACITY => {
            let (channel, side) = endpoint(RIGHT_SEND)?;
            WaitTarget::SendCapacity(channel, side)
        }
        // Resolved through the caller's own table with the right the query
        // itself requires, so a task can only wait on a child it may also ask
        // about. Before P5.3.3 this was `Unmediated`, because no spawn existed
        // to mint a handle for it to name.
        WAIT_KIND_SUPERVISION => match table.resolve(slot, RIGHT_SUPERVISE)?.resource {
            graph::Resource::Supervision { task } => WaitTarget::Supervision(task),
            _ => return Err(IpcError::InvalidOperation),
        },
        // M6.4 (P5.4.3). Always ready, because the source is a script the root
        // reads synchronously: there is no interrupt to park for, and the next
        // `InputRead` answers immediately — with an event, or `WouldBlock` when
        // the script is spent.
        //
        // Not `Unmediated`, which is never ready: a Dango session waiting on
        // input would have parked forever, and the mistake would have looked
        // like a hung component rather than an unhandled wait kind.
        WAIT_KIND_INPUT => WaitTarget::Input,
        _ => return Err(IpcError::InvalidOperation),
    })
}

impl ChannelTable {
    /// Whether `target` is ready right now.
    pub fn is_ready(&self, target: WaitTarget) -> bool {
        match target {
            WaitTarget::Receive(key, side) => self
                .recv_queue(key, side)
                .is_some_and(Channel::receive_ready),
            WaitTarget::SendCapacity(key, side) => {
                self.send_queue(key, side).is_some_and(Channel::send_ready)
            }
            // Always ready: `InputRead` answers immediately, with an event or
            // with `WouldBlock` when the script is spent. A source that
            // reported "not ready" once exhausted would park its reader
            // forever — which is exactly what the root-launched copy of a
            // console component does, since it drains its own cursor and then
            // waits. Readiness here means "asking is worthwhile", and asking
            // always terminates.
            WaitTarget::Input => true,
            WaitTarget::Supervision(_) | WaitTarget::Unmediated => false,
        }
    }

    /// Register `task` as waiting on `target`, so a peer's send or death wakes
    /// it.
    pub fn register_wait(&mut self, task: TaskId, target: WaitTarget) -> Result<(), IpcError> {
        match target {
            WaitTarget::Receive(key, side) => self
                .recv_queue_mut(key, side)
                .ok_or(IpcError::InvalidOperation)?
                .register_receive_waiter(task.0),
            WaitTarget::SendCapacity(key, side) => self
                .send_queue_mut(key, side)
                .ok_or(IpcError::InvalidOperation)?
                .register_send_waiter(task.0),
            // Input needs no registration: it is always ready, so `arm` never
            // reaches this for it after the readiness probe.
            WaitTarget::Input | WaitTarget::Supervision(_) | WaitTarget::Unmediated => Ok(()),
        }
    }

    /// Drop every registration `task` holds, on every queue.
    ///
    /// Called after a wake, because `WaitSet::arm` registers on every source
    /// and only one of them fires. A stale registration would make the next
    /// wake on that queue name a task that is no longer waiting.
    pub fn clear_waits(&mut self, task: TaskId) {
        for entry in self.entries.iter_mut().flatten() {
            for queue in entry.queues_mut() {
                queue.clear_waiter(task.0);
            }
        }
    }

    /// Every channel key `task` is registered to wake on, with the direction.
    ///
    /// Diagnostic only, and deliberately a scan: a wedge is already fatal, so
    /// the cost is paid once on a boot that is ending. It exists because the
    /// root could name *which* tasks were stuck (`wedged waiter task=N`) but not
    /// what each was waiting for, which is the difference between reporting a
    /// deadlock and explaining it.
    pub fn registered_waits(&self, task: TaskId) -> impl Iterator<Item = (ChannelKey, bool)> + '_ {
        self.entries.iter().flatten().flat_map(move |entry| {
            let key = entry.key;
            entry
                .queues()
                .filter_map(move |queue| queue.waits_for(task.0).map(|receive| (key, receive)))
        })
    }
}

/// Test-only conveniences.
///
#[cfg(test)]
impl ChannelTable {
    fn push_undelegated(
        &mut self,
        rights: u64,
        loopback: bool,
    ) -> Result<(ChannelKey, usize), ChannelError> {
        self.push(rights, false, loopback)
    }
}

#[cfg(test)]
mod tests {
    use super::{ChannelTable, DeathWakes, SlotCursors};
    use crate::generation::{RIGHT_RECV, RIGHT_SEND};
    use crate::graph::{Capability, GraphTables, Resource, Side};
    use crate::task::TaskId;
    use crate::transit::Transit;

    const PRODUCER: TaskId = TaskId(1);
    const CONSUMER: TaskId = TaskId(2);

    fn hold(graph: &mut GraphTables, task: TaskId, channel: u32, side: Side) {
        if graph.get(task).is_none() {
            graph.create(task).expect("create");
        }
        let table = graph.get_mut(task).expect("table");
        let slot = table.free_slot_from(0).expect("slot");
        table
            .install(
                slot,
                Capability {
                    resource: Resource::Endpoint { channel, side },
                    rights: RIGHT_SEND | RIGHT_RECV,
                },
            )
            .expect("install");
    }

    #[test]
    fn a_one_directional_grant_gives_the_consumer_no_way_to_reply() {
        let mut channels = ChannelTable::new();
        let (key, queues) = channels.push_undelegated(RIGHT_SEND, false).expect("push");
        assert_eq!(queues, 1);
        assert!(channels.send_queue(key, Side::Producer).is_some());
        assert!(channels.recv_queue(key, Side::Consumer).is_some());
        assert!(channels.send_queue(key, Side::Consumer).is_none());
        assert!(channels.recv_queue(key, Side::Producer).is_none());

        let (recv_key, recv_queues) = channels.push_undelegated(RIGHT_RECV, false).expect("push");
        assert_eq!(recv_queues, 1);
        assert!(channels.send_queue(recv_key, Side::Producer).is_some());
        assert!(channels.recv_queue(recv_key, Side::Consumer).is_some());
    }

    #[test]
    fn a_bidirectional_grant_is_one_channel_carrying_two_queues() {
        let mut channels = ChannelTable::new();
        let (key, queues) = channels
            .push_undelegated(RIGHT_SEND | RIGHT_RECV, false)
            .expect("push");
        assert_eq!(queues, 2);
        for side in [Side::Producer, Side::Consumer] {
            assert!(channels.send_queue(key, side).is_some());
            assert!(channels.recv_queue(key, side).is_some());
        }
        let forward = channels
            .send_queue_mut(key, Side::Producer)
            .expect("forward");
        let plan = forward.preflight_send().expect("capacity");
        forward
            .commit_send(plan, crate::ipc::Message::default())
            .expect("send");
        assert_eq!(
            channels
                .recv_queue(key, Side::Consumer)
                .expect("queue")
                .len(),
            1
        );
        assert_eq!(
            channels
                .recv_queue(key, Side::Producer)
                .expect("queue")
                .len(),
            0
        );
    }

    #[test]
    fn a_declared_self_edge_is_its_own_peer() {
        let mut channels = ChannelTable::new();
        let (key, queues) = channels
            .push_undelegated(RIGHT_SEND | RIGHT_RECV, true)
            .expect("push");
        assert_eq!(queues, 1);
        assert!(channels.send_queue(key, Side::Loopback).is_some());
        assert!(channels.recv_queue(key, Side::Loopback).is_some());
    }

    #[test]
    fn keys_are_dense_and_deterministic_in_declaration_order() {
        let mut channels = ChannelTable::new();
        let (first, _) = channels.push_undelegated(RIGHT_SEND, false).expect("push");
        let (second, _) = channels.push_undelegated(RIGHT_SEND, false).expect("push");
        assert_eq!((first, second), (0, 1));
    }

    #[test]
    fn a_dead_task_kills_every_queue_it_was_the_last_holder_of() {
        let mut channels = ChannelTable::new();
        let (rpc, _) = channels
            .push_undelegated(RIGHT_SEND | RIGHT_RECV, false)
            .expect("push");
        let (other, _) = channels.push_undelegated(RIGHT_SEND, false).expect("push");
        let mut graph = GraphTables::new();
        hold(&mut graph, PRODUCER, rpc, Side::Producer);
        hold(&mut graph, CONSUMER, rpc, Side::Consumer);
        hold(&mut graph, TaskId(3), other, Side::Producer);
        hold(&mut graph, TaskId(4), other, Side::Consumer);
        channels.mark_dead(&graph, PRODUCER, &mut DeathWakes::new());
        let transit = Transit::new();
        assert_eq!(
            channels.live_queues(&graph, &transit),
            1,
            "only the unrelated queue lives"
        );
        assert!(
            !channels
                .recv_queue(rpc, Side::Consumer)
                .expect("queue")
                .peer_alive()
        );
        assert!(
            channels
                .recv_queue(other, Side::Consumer)
                .expect("queue")
                .peer_alive()
        );
    }

    #[test]
    fn an_end_with_a_second_holder_survives_the_first_ones_death() {
        let mut channels = ChannelTable::new();
        let key = channels.mint().expect("mint");
        let mut graph = GraphTables::new();
        hold(&mut graph, PRODUCER, key, Side::Producer);
        hold(&mut graph, TaskId(7), key, Side::Producer);
        hold(&mut graph, CONSUMER, key, Side::Consumer);
        channels.mark_dead(&graph, PRODUCER, &mut DeathWakes::new());
        let transit = Transit::new();
        assert_eq!(
            channels.live_queues(&graph, &transit),
            2,
            "co-holder keeps both directions live"
        );
    }

    #[test]
    fn an_entry_no_table_names_counts_no_live_queue() {
        // The sweep is lazy-on-full, so a boot that released every end still
        // holds the entry until the table next fills. Its queues have no peer
        // rather than a live one, and counting them would report a graph that
        // never finished tearing down.
        let mut channels = ChannelTable::new();
        let key = channels.mint().expect("mint");
        let mut graph = GraphTables::new();
        hold(&mut graph, PRODUCER, key, Side::Producer);
        hold(&mut graph, CONSUMER, key, Side::Consumer);
        let transit = Transit::new();
        assert_eq!(channels.live_queues(&graph, &transit), 2, "both held");
        graph.release(PRODUCER);
        graph.release(CONSUMER);
        assert_eq!(
            channels.live_queues(&graph, &transit),
            0,
            "an unnameable entry is not a live queue"
        );
    }

    #[test]
    fn a_parked_receiver_is_woken_by_its_peers_death() {
        let mut channels = ChannelTable::new();
        let (key, _) = channels.push_undelegated(RIGHT_SEND, false).expect("push");
        let mut graph = GraphTables::new();
        hold(&mut graph, PRODUCER, key, Side::Producer);
        hold(&mut graph, CONSUMER, key, Side::Consumer);
        channels
            .recv_queue_mut(key, Side::Consumer)
            .expect("queue")
            .register_receive_waiter(CONSUMER.0)
            .expect("register");
        let mut wakes = DeathWakes::new();
        channels.mark_dead(&graph, PRODUCER, &mut wakes);
        assert_eq!(wakes.drain().count(), 1);
    }

    /// Slot 0 is what `console.rs`, `spawn-service.rs`, and `launch_context`
    /// all address, so a component's first channel must land there whatever its
    /// executable count is.
    #[test]
    fn the_first_channel_is_slot_zero_and_later_ones_clear_the_executables() {
        let mut cursors = SlotCursors::new();
        cursors.declare(TaskId(0), 2).expect("declare");
        cursors.declare(TaskId(1), 0).expect("declare");

        assert_eq!(cursors.take(TaskId(0)), Some(0));
        assert_eq!(
            cursors.take(TaskId(0)),
            Some(3),
            "slots 1 and 2 hold executable grants"
        );
        assert_eq!(cursors.take(TaskId(0)), Some(4));

        // One task's numbering says nothing about another's.
        assert_eq!(cursors.take(TaskId(1)), Some(0));
        assert_eq!(cursors.take(TaskId(1)), Some(1));

        assert_eq!(
            cursors.take(TaskId(9)),
            None,
            "a task that was never staged has no slots to hand out"
        );
    }
}
