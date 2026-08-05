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
//! what it sent. See [`ChannelTable::push`].
//!
//! # Slot numbering
//!
//! Two rules, because two kinds of component learn their slot numbers
//! differently:
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
use crate::graph::{self, GraphTables};
use crate::ipc::{Channel, ChannelKey, IpcError};
use crate::task::TaskId;

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
/// Sixteen is the same bound `graph::MAX_GRAPH_TASKS` uses, and one channel per
/// task pair is more than any declared seL4 generation needs.
pub const MAX_CHANNELS: usize = 16;

/// One logical channel: the two tasks holding it, and the directed queues
/// between them. A one-directional grant leaves `reverse` absent, so a task
/// cannot receive on a channel the generation only let it produce to.
struct Entry {
    key: ChannelKey,
    /// The end the grant made the producer; see the module doc's direction rule.
    producer: TaskId,
    /// The opposite end. Equal to `producer` for a loopback.
    consumer: TaskId,
    /// Queue carrying producer → consumer. Always present: every channel has at
    /// least one direction, whichever right named it.
    forward: Option<Channel>,
    /// Queue carrying consumer → producer, present only when the grant is
    /// bidirectional *and* the two ends are different tasks. A loopback has no
    /// reverse: both accessors resolve to `forward` for the one task holding it.
    reverse: Option<Channel>,
    /// Whether the generation declared this grant `transferable`.
    ///
    /// Recorded on the channel rather than folded into the ends' rights bits,
    /// because it is not authority over the *channel*: it does not widen what
    /// either end may send or receive. It is the generation's statement that
    /// this edge may carry delegated authority, and the only thing that reads
    /// it is the loan plane, which refuses to mint a loan over an edge the
    /// generation did not mark — see `main.rs::serve_buffer_loan`.
    transferable: bool,
}

impl Entry {
    /// The queue `task` may enqueue onto, if the grant gave it one.
    ///
    /// Note the order of the two comparisons here and in [`Self::recv_queue`]:
    /// when both ends are one task — a loopback — `producer` matches first in
    /// both, so sending and receiving resolve to the same queue. That is what a
    /// task sending to itself must mean, and [`ChannelTable::push`] allocates
    /// only that one queue for such a channel.
    fn send_queue(&self, task: TaskId) -> Option<&Channel> {
        if task == self.producer {
            self.forward.as_ref()
        } else if task == self.consumer {
            self.reverse.as_ref()
        } else {
            None
        }
    }

    fn send_queue_mut(&mut self, task: TaskId) -> Option<&mut Channel> {
        if task == self.producer {
            self.forward.as_mut()
        } else if task == self.consumer {
            self.reverse.as_mut()
        } else {
            None
        }
    }

    /// The queue `task` may dequeue from: the one its peer sends on.
    fn recv_queue(&self, task: TaskId) -> Option<&Channel> {
        if task == self.consumer {
            self.forward.as_ref()
        } else if task == self.producer {
            self.reverse.as_ref()
        } else {
            None
        }
    }

    fn recv_queue_mut(&mut self, task: TaskId) -> Option<&mut Channel> {
        if task == self.consumer {
            self.forward.as_mut()
        } else if task == self.producer {
            self.reverse.as_mut()
        } else {
            None
        }
    }

    fn queues_mut(&mut self) -> impl Iterator<Item = &mut Channel> {
        self.forward.iter_mut().chain(self.reverse.iter_mut())
    }

    fn involves(&self, task: TaskId) -> bool {
        self.producer == task || self.consumer == task
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
}

impl ChannelTable {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; MAX_CHANNELS],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Directed queues that still have a live peer. Reaching zero is what the
    /// service loop's termination condition reads.
    pub fn live_queues(&self) -> usize {
        self.entries
            .iter()
            .flatten()
            .flat_map(|entry| entry.forward.iter().chain(entry.reverse.iter()))
            .filter(|queue| queue.peer_alive())
            .count()
    }

    /// The queue `task` may send on over `key`.
    pub fn send_queue(&self, key: ChannelKey, task: TaskId) -> Option<&Channel> {
        self.entry(key)?.send_queue(task)
    }

    pub fn send_queue_mut(&mut self, key: ChannelKey, task: TaskId) -> Option<&mut Channel> {
        self.entry_mut(key)?.send_queue_mut(task)
    }

    /// The queue `task` may receive from over `key`.
    pub fn recv_queue(&self, key: ChannelKey, task: TaskId) -> Option<&Channel> {
        self.entry(key)?.recv_queue(task)
    }

    pub fn recv_queue_mut(&mut self, key: ChannelKey, task: TaskId) -> Option<&mut Channel> {
        self.entry_mut(key)?.recv_queue_mut(task)
    }

    /// How many channels `task` holds an end of.
    pub fn held_by(&self, task: TaskId) -> usize {
        self.entries
            .iter()
            .flatten()
            .filter(|entry| entry.involves(task))
            .count()
    }

    /// Whether the generation declared this channel's grant `transferable`.
    ///
    /// `None` for a key naming no channel. A component cannot influence this:
    /// it is decided once at materialization from the generation's own field.
    pub fn transferable(&self, key: ChannelKey) -> Option<bool> {
        self.entry(key).map(|entry| entry.transferable)
    }

    /// The task at the other end of `key` from `task`.
    pub fn peer(&self, key: ChannelKey, task: TaskId) -> Option<TaskId> {
        let entry = self.entry(key)?;
        if entry.producer == task {
            Some(entry.consumer)
        } else if entry.consumer == task {
            Some(entry.producer)
        } else {
            None
        }
    }

    /// Mark every queue `task` held an end of as having a dead peer, so a
    /// parked receive returns `PeerDead` instead of waiting forever.
    ///
    /// Returns the wakes the caller must deliver, paired with the channel they
    /// came from. Both queues of a bidirectional channel die together: the peer
    /// is gone in both directions.
    pub fn mark_dead(&mut self, task: TaskId, wakes: &mut DeathWakes) {
        for entry in self.entries.iter_mut().flatten() {
            if !entry.involves(task) {
                continue;
            }
            let key = entry.key;
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

    /// Move the end `from` holds on `key` to `to`.
    ///
    /// A channel's queues are resolved by *which task* holds each end — see
    /// [`Entry::send_queue`] — rather than by anything carried in the
    /// capability. So handing a child a channel end at spawn is not complete
    /// until the table agrees the child is the holder: a capability alone would
    /// resolve to no queue at all, because the child matches neither
    /// `producer` nor `consumer`.
    ///
    /// That makes this a **move**, where the retired kernel's spawn grant is a
    /// non-consuming copy. The difference is real but narrow, and it falls on
    /// the side of less authority: there, parent and child would both hold a
    /// working end; here the parent gives its end up. Every x86 caller already
    /// behaves that way — `launch_sample_plane` grants each half to exactly one
    /// child, and `launch_fabric_graph`'s comment states outright that init
    /// "releases the control endpoint as soon as the spawn that needed them
    /// returns". A parent that tried to keep a granted end would find it gone
    /// rather than find it silently shared.
    ///
    /// `false` when `from` holds no end of this channel, which the caller turns
    /// into a refused spawn rather than a partially distributed graph.
    pub fn reassign(&mut self, key: ChannelKey, from: TaskId, to: TaskId) -> bool {
        let Some(entry) = self.entry_mut(key) else {
            return false;
        };
        // A loopback is the case [`Self::mint`] creates: both ends start with
        // the minting task, and handing one to a child is the whole point.
        // Splitting it makes the pair real — the consumer end moves, the task
        // keeps the producer end, and the reverse queue the two-task shape
        // needs is allocated here rather than at mint, because until now there
        // was only one task to carry it.
        if entry.producer == entry.consumer {
            if entry.producer != from || from == to {
                return false;
            }
            entry.consumer = to;
            if entry.reverse.is_none() {
                entry.reverse = Some(Channel::new(entry.key));
            }
            return true;
        }
        if entry.producer == from {
            entry.producer = to;
            true
        } else if entry.consumer == from {
            entry.consumer = to;
            true
        } else {
            false
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

    /// Mint a bidirectional channel between `first` and `second` at runtime.
    ///
    /// The generation declares the graph's *standing* edges; this is how a
    /// component that holds an `EndpointFactory` makes one the generation could
    /// not have named — a per-request context channel, whose two ends exist
    /// only for as long as the request does. `spawn-service` does exactly this
    /// on every x86 boot (`endpoint_create(3)` then `send_context`), which is
    /// why the operation must exist before any component can hand a child its
    /// launch context.
    ///
    /// Marked `transferable`, matching the retired kernel's
    /// `sys_endpoint_create`: a freshly minted pair carries `RIGHT_TRANSFER` on
    /// both ends because handing one half away is the only reason to mint one.
    /// That is not a widening of the generation's authority — the authority to
    /// mint at all came from a declared `endpointCreate` grant.
    pub fn mint(&mut self, first: TaskId, second: TaskId) -> Result<ChannelKey, ChannelError> {
        let (key, _) = self.push(first, second, RIGHT_SEND | RIGHT_RECV, true)?;
        Ok(key)
    }

    fn push(
        &mut self,
        producer: TaskId,
        consumer: TaskId,
        rights: u64,
        transferable: bool,
    ) -> Result<(ChannelKey, usize), ChannelError> {
        let key = self.len as ChannelKey;
        let slot = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(ChannelError::TableFull)?;
        // `rights` here has already been resolved against the producer/consumer
        // assignment: a one-directional grant always yields the producer's
        // forward queue, whichever right named it, and only a bidirectional one
        // adds the reverse.
        //
        // A loopback is the exception. When both ends are one task, that task
        // matches `producer` in every accessor and so reaches `forward` for both
        // sending and receiving — which is right, since a task sending to itself
        // must receive what it sent. Allocating a reverse queue there would
        // build one nothing can ever name, and would make the boot marker report
        // two queues where the graph has one.
        let bidirectional = rights == RIGHT_SEND | RIGHT_RECV;
        let forward = Some(Channel::new(key));
        let reverse = (bidirectional && producer != consumer).then(|| Channel::new(key));
        let queues = usize::from(forward.is_some()) + usize::from(reverse.is_some());
        *slot = Some(Entry {
            key,
            producer,
            consumer,
            forward,
            reverse,
            transferable,
        });
        self.len += 1;
        Ok((key, queues))
    }
}

impl Default for ChannelTable {
    fn default() -> Self {
        Self::new()
    }
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

        let mut slots = [None; 2];
        for (destination, task) in slots
            .iter_mut()
            .zip(core::iter::once(producer).chain(second))
        {
            let held = held_rights(grant.rights, task, producer);
            let slot = channel_slot(layout, bootstrap, cursors, task, grant.name, held)?
                .ok_or(ChannelError::UnlaidSlot)?;
            *destination = Some((task, slot, held));
        }

        let (key, queues) = channels.push(producer, consumer, carries, grant.transferable)?;
        report.channels += 1;
        report.queues += queues;
        sel4::debug_println!(
            "SLIME_GRAPH channel grant={} key={key} producer={} consumer={} queues={queues}",
            grant.name,
            producer.0,
            consumer.0,
        );
        for (task, slot, held) in slots.into_iter().flatten() {
            let table = graph
                .get_mut(task)
                .ok_or(ChannelError::UnlaunchedEndpoint)?;
            table.install(
                slot,
                graph::Capability {
                    resource: graph::Resource::Endpoint { channel: key },
                    rights: held,
                },
            )?;
            sel4::debug_println!(
                "SLIME_GRAPH channel end task={} slot={slot} key={key} rights={held:#x}",
                task.0,
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
    /// Ready when the task's receive queue on this channel has a message, or
    /// its peer has died.
    Receive(ChannelKey),
    /// Ready when the task's send queue on this channel has room, or its peer
    /// has died.
    SendCapacity(ChannelKey),
    /// Ready when the named child has terminated.
    ///
    /// Not a queue, unlike the two above: the readiness event is a task dying,
    /// which no channel observes. `main.rs` holds the registration and the
    /// termination record, and this only carries which child was named.
    Supervision(TaskId),
    /// A source this cutover has no mechanism for. Never ready, and never
    /// registered — a task waiting only on one of these would block forever, so
    /// the dispatcher refuses the wait rather than parking on it.
    Unmediated,
}

/// Decode one wait-source record and resolve it against `table`.
pub fn resolve_wait_source(
    record: u64,
    table: &graph::CapabilityTable,
) -> Result<WaitTarget, IpcError> {
    let kind = record >> 32;
    let slot = (record & 0xffff_ffff) as u32;
    let channel = |required: u64| -> Result<ChannelKey, IpcError> {
        match table.resolve(slot, required)?.resource {
            graph::Resource::Endpoint { channel } => Ok(channel),
            _ => Err(IpcError::InvalidOperation),
        }
    };
    Ok(match kind {
        WAIT_KIND_ENDPOINT => WaitTarget::Receive(channel(RIGHT_RECV)?),
        WAIT_KIND_SEND_CAPACITY => WaitTarget::SendCapacity(channel(RIGHT_SEND)?),
        // Resolved through the caller's own table with the right the query
        // itself requires, so a task can only wait on a child it may also ask
        // about. Before P5.3.3 this was `Unmediated`, because no spawn existed
        // to mint a handle for it to name.
        WAIT_KIND_SUPERVISION => match table.resolve(slot, RIGHT_SUPERVISE)?.resource {
            graph::Resource::Supervision { task } => WaitTarget::Supervision(task),
            _ => return Err(IpcError::InvalidOperation),
        },
        WAIT_KIND_INPUT => WaitTarget::Unmediated,
        _ => return Err(IpcError::InvalidOperation),
    })
}

impl ChannelTable {
    /// Whether `task` would find `target` ready right now.
    ///
    /// A dead peer counts as ready for both directions: the operation the task
    /// retries after waking returns `PeerDead`, which is an answer. Parking on
    /// a channel whose peer is gone would be a hang.
    pub fn is_ready(&self, task: TaskId, target: WaitTarget) -> bool {
        match target {
            WaitTarget::Receive(key) => self
                .recv_queue(key, task)
                .is_some_and(Channel::receive_ready),
            WaitTarget::SendCapacity(key) => {
                self.send_queue(key, task).is_some_and(Channel::send_ready)
            }
            // Never ready *here*: a child's death is not a queue event, so
            // the dispatcher tests it against the termination record instead.
            // Returning `false` is what makes it fall through to registration.
            WaitTarget::Supervision(_) | WaitTarget::Unmediated => false,
        }
    }

    /// Register `task` as waiting on `target`, so a peer's send or death wakes
    /// it.
    pub fn register_wait(&mut self, task: TaskId, target: WaitTarget) -> Result<(), IpcError> {
        match target {
            WaitTarget::Receive(key) => self
                .recv_queue_mut(key, task)
                .ok_or(IpcError::InvalidOperation)?
                .register_receive_waiter(task.0),
            WaitTarget::SendCapacity(key) => self
                .send_queue_mut(key, task)
                .ok_or(IpcError::InvalidOperation)?
                .register_send_waiter(task.0),
            // Registered in `main.rs::SupervisionWaits` rather than on a
            // queue, because no queue observes a task dying.
            WaitTarget::Supervision(_) | WaitTarget::Unmediated => Ok(()),
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
}

#[cfg(test)]
mod tests {
    use super::{ChannelTable, DeathWakes, SlotCursors, WaitTarget};
    use crate::generation::{RIGHT_RECV, RIGHT_SEND};
    /// Authority to observe a spawned child's termination; see
    /// `main.rs::RIGHT_SUPERVISE`, which this must agree with.
    const RIGHT_SUPERVISE: u64 = 1 << 18;
    use crate::task::TaskId;

    const PRODUCER: TaskId = TaskId(1);
    const CONSUMER: TaskId = TaskId(2);

    #[test]
    fn a_one_directional_grant_gives_the_consumer_no_way_to_reply() {
        let mut channels = ChannelTable::new();
        let (key, queues) = channels.push(PRODUCER, CONSUMER, RIGHT_SEND).expect("push");
        assert_eq!(queues, 1);

        assert!(channels.send_queue(key, PRODUCER).is_some());
        assert!(channels.recv_queue(key, CONSUMER).is_some());
        assert!(
            channels.send_queue(key, CONSUMER).is_none(),
            "a one-directional grant does not let the consumer produce"
        );
        assert!(
            channels.recv_queue(key, PRODUCER).is_none(),
            "and gives the producer nothing to dequeue"
        );

        // A `recv` grant is the same shape with the ends labelled the other way
        // round -- one queue, from whichever end the generation made the
        // producer -- not a channel that runs backwards.
        let (recv_key, recv_queues) = channels.push(PRODUCER, CONSUMER, RIGHT_RECV).expect("push");
        assert_eq!(recv_queues, 1);
        assert!(channels.send_queue(recv_key, PRODUCER).is_some());
        assert!(channels.recv_queue(recv_key, CONSUMER).is_some());
    }

    #[test]
    fn a_bidirectional_grant_is_one_channel_carrying_two_queues() {
        let mut channels = ChannelTable::new();
        let (key, queues) = channels
            .push(PRODUCER, CONSUMER, RIGHT_SEND | RIGHT_RECV)
            .expect("push");
        assert_eq!(queues, 2);
        assert_eq!(channels.len(), 1, "one slot number at each end, not two");

        for task in [PRODUCER, CONSUMER] {
            assert!(channels.send_queue(key, task).is_some());
            assert!(channels.recv_queue(key, task).is_some());
        }
        // The two directions are distinct queues: filling one leaves the other
        // empty, which is what makes a request/reply pair work over one slot.
        let forward = channels.send_queue_mut(key, PRODUCER).expect("forward");
        let plan = forward.preflight_send().expect("capacity");
        forward
            .commit_send(plan, crate::ipc::Message::default())
            .expect("send");
        assert_eq!(channels.recv_queue(key, CONSUMER).expect("queue").len(), 1);
        assert_eq!(channels.recv_queue(key, PRODUCER).expect("queue").len(), 0);
    }

    #[test]
    fn a_task_holding_neither_end_resolves_to_nothing() {
        let mut channels = ChannelTable::new();
        let (key, _) = channels
            .push(PRODUCER, CONSUMER, RIGHT_SEND | RIGHT_RECV)
            .expect("push");
        assert!(channels.send_queue(key, TaskId(9)).is_none());
        assert!(channels.recv_queue(key, TaskId(9)).is_none());
        assert_eq!(channels.peer(key, TaskId(9)), None);
        assert_eq!(channels.peer(key, PRODUCER), Some(CONSUMER));
        assert_eq!(channels.peer(key, CONSUMER), Some(PRODUCER));
    }

    #[test]
    fn keys_are_dense_and_deterministic_in_declaration_order() {
        let mut channels = ChannelTable::new();
        let (first, _) = channels.push(PRODUCER, CONSUMER, RIGHT_SEND).expect("push");
        let (second, _) = channels
            .push(CONSUMER, TaskId(3), RIGHT_SEND)
            .expect("push");
        assert_eq!((first, second), (0, 1));
        assert_eq!(channels.len(), 2);
    }

    /// Peer death has to reach *both* directions and *both* channels a task
    /// holds, or a parked receive on the untouched one waits forever.
    #[test]
    fn a_dead_task_kills_every_queue_it_held_an_end_of() {
        let mut channels = ChannelTable::new();
        let (rpc, _) = channels
            .push(PRODUCER, CONSUMER, RIGHT_SEND | RIGHT_RECV)
            .expect("push");
        let (other, _) = channels
            .push(TaskId(3), TaskId(4), RIGHT_SEND)
            .expect("push");
        assert_eq!(channels.live_queues(), 3);

        channels.mark_dead(PRODUCER, &mut DeathWakes::new());
        assert_eq!(channels.live_queues(), 1, "only the unrelated queue lives");
        assert!(
            !channels
                .recv_queue(rpc, CONSUMER)
                .expect("queue")
                .peer_alive()
        );
        assert!(
            channels
                .recv_queue(other, TaskId(4))
                .expect("queue")
                .peer_alive()
        );
    }

    #[test]
    fn a_parked_receiver_is_woken_by_its_peers_death() {
        let mut channels = ChannelTable::new();
        let (key, _) = channels.push(PRODUCER, CONSUMER, RIGHT_SEND).expect("push");
        channels
            .recv_queue_mut(key, CONSUMER)
            .expect("queue")
            .register_receive_waiter(CONSUMER.0)
            .expect("register");

        let mut wakes = DeathWakes::new();
        channels.mark_dead(PRODUCER, &mut wakes);
        let woken: usize = wakes.drain().count();
        assert_eq!(
            woken, 1,
            "the parked receiver must be told, not left blocked"
        );
        assert!(wakes.drain().all(|(channel, wake)| channel == key
            && wake.task == CONSUMER.0
            && matches!(wake.cause, crate::ipc::WakeCause::PeerDeath(_))));
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
