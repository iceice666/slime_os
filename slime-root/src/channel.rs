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

use boot_contracts::generation::{Generation, GrantEndpoint};

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
    forward: Option<Channel>,
    reverse: Option<Channel>,
    transferable: bool,
}

#[derive(Clone, Copy)]
struct DeclaredEndpoint {
    instance: usize,
    grant: usize,
    key: ChannelKey,
    side: Side,
    rights: u64,
    /// Set once the declared holder has run and exited.
    ///
    /// A declaration keeps its side alive so a peer's exit cannot tear down a
    /// channel whose other end is about to be installed — and so a service
    /// whose launcher exits keeps serving, which is what a `required`
    /// instance means. Neither reason survives the holder's own death: from
    /// then on the declaration would make the side permanently
    /// un-abandonable, so `mark_dead` would find nothing abandoned, wake
    /// nobody, and leave a peer blocked on a channel with no other end (B46).
    ///
    /// The descriptor stays either way, because a repeatable instance
    /// template installs from it again.
    holder_exited: bool,
    /// The task the end was installed into, so the death above can be
    /// attributed to the right declaration.
    installed: Option<TaskId>,
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
    /// A grant names an instance that was not launched.
    UnlaunchedEndpoint,
    /// A launched instance has no explicit binding for a grant it receives.
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

/// Every logical channel this generation declared, including endpoint halves
/// whose declared instance has not yet been spawned.
pub struct ChannelTable {
    entries: [Option<Entry>; MAX_CHANNELS],
    len: usize,
    next_key: ChannelKey,
    minted: usize,
    declared: [Option<DeclaredEndpoint>; MAX_CHANNELS * 2],
    declared_len: usize,
}

impl ChannelTable {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; MAX_CHANNELS],
            len: 0,
            next_key: 0,
            minted: 0,
            declared: [const { None }; MAX_CHANNELS * 2],
            declared_len: 0,
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
            .filter(|entry| {
                graph.holds_endpoint(entry.key)
                    || transit.holds_endpoint(entry.key)
                    || self.has_declared(entry.key)
            })
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

    fn declare_endpoint(&mut self, endpoint: DeclaredEndpoint) -> Result<(), ChannelError> {
        let slot = self
            .declared
            .get_mut(self.declared_len)
            .ok_or(ChannelError::TableFull)?;
        *slot = Some(endpoint);
        self.declared_len += 1;
        Ok(())
    }

    fn has_declared(&self, key: ChannelKey) -> bool {
        self.declared[..self.declared_len]
            .iter()
            .flatten()
            .any(|endpoint| endpoint.key == key)
    }

    fn has_declared_side(&self, key: ChannelKey, side: Side) -> bool {
        self.declared[..self.declared_len]
            .iter()
            .flatten()
            .any(|endpoint| endpoint.key == key && endpoint.side == side && !endpoint.holder_exited)
    }

    /// Install every pre-created channel end declared for an instance. The
    /// descriptor remains reusable for repeatable instance templates.
    pub fn install_instance(
        &mut self,
        generation: &Generation<'_>,
        instance: usize,
        task: TaskId,
        graph: &mut GraphTables,
    ) -> Result<usize, ChannelError> {
        let mut installed = 0;
        for index in 0..self.declared_len {
            let Some(endpoint) = self.declared[index] else {
                continue;
            };
            if endpoint.instance != instance {
                continue;
            }
            let slot = binding_slot(generation, instance, endpoint.grant)?;
            graph
                .get_mut(task)
                .ok_or(ChannelError::UnlaunchedEndpoint)?
                .install(
                    slot,
                    graph::Capability {
                        resource: graph::Resource::Endpoint {
                            channel: endpoint.key,
                            side: endpoint.side,
                        },
                        rights: endpoint.rights,
                    },
                )?;
            // Where a declared end actually landed. The slot is the
            // generation's to choose and the component compiles against it, so
            // a mismatch is silent until the first send goes nowhere; naming
            // it here is what lets a gate assert the two agree.
            sel4::debug_println!(
                "SLIME_GRAPH channel end task={} slot={slot} key={} side={} rights={:#x}",
                task.0,
                endpoint.key,
                endpoint.side.name(),
                endpoint.rights,
            );
            if let Some(entry) = self.declared[index].as_mut() {
                entry.installed = Some(task);
            }
            installed += 1;
        }
        Ok(installed)
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
        // Retire this task's declarations before looking for abandoned sides:
        // while one stands, its side reads as held and nothing below can see
        // the channel as abandoned.
        for entry in self.declared[..self.declared_len].iter_mut().flatten() {
            if entry.installed == Some(task) {
                entry.holder_exited = true;
            }
        }
        for index in 0..MAX_CHANNELS {
            let Some(key) = self.entries[index].as_ref().map(|entry| entry.key) else {
                continue;
            };
            let abandoned = [Side::Producer, Side::Consumer, Side::Loopback]
                .into_iter()
                .filter(|side| {
                    graph
                        .get(task)
                        .is_some_and(|table| table.reaches_endpoint(key, *side))
                })
                .any(|side| {
                    !graph.holds_endpoint_side(key, side, Some(task))
                        && !self.has_declared_side(key, side)
                });
            if !abandoned {
                continue;
            }
            let Some(entry) = self.entries[index].as_mut() else {
                continue;
            };
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
    for index in 0..MAX_CHANNELS {
        let Some(key) = channels.entries[index].as_ref().map(|entry| entry.key) else {
            continue;
        };
        if graph.holds_endpoint(key) || transit.holds_endpoint(key) || channels.has_declared(key) {
            continue;
        }
        channels.entries[index] = None;
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

/// Which launched generation instance and executable each task represents.
pub struct LaunchedInstances {
    entries: [Option<LaunchedInstance>; MAX_CHANNELS],
    len: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LaunchedInstance {
    pub instance: usize,
    pub executable: usize,
    pub task: TaskId,
}

impl LaunchedInstances {
    pub const fn new() -> Self {
        Self {
            entries: [None; MAX_CHANNELS],
            len: 0,
        }
    }

    pub fn record(
        &mut self,
        instance: usize,
        executable: usize,
        task: TaskId,
    ) -> Result<(), ChannelError> {
        if self.task_for_instance(instance).is_some() {
            return Err(ChannelError::UnlaidSlot);
        }
        let slot = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(ChannelError::TableFull)?;
        *slot = Some(LaunchedInstance {
            instance,
            executable,
            task,
        });
        self.len += 1;
        Ok(())
    }

    pub fn task_for_instance(&self, instance: usize) -> Option<TaskId> {
        self.entries
            .iter()
            .flatten()
            .find(|launched| launched.instance == instance)
            .map(|launched| launched.task)
    }

    pub fn instance_for_task(&self, task: TaskId) -> Option<usize> {
        self.entries
            .iter()
            .flatten()
            .find(|launched| launched.task == task)
            .map(|launched| launched.instance)
    }

    pub fn release_by_task(&mut self, task: TaskId) -> Option<LaunchedInstance> {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_some_and(|launched| launched.task == task))?;
        let released = entry.take();
        if released.is_some() {
            self.len = self.len.saturating_sub(1);
        }
        released
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn iter(&self) -> impl Iterator<Item = LaunchedInstance> + '_ {
        self.entries.iter().flatten().copied()
    }
}

impl Default for LaunchedInstances {
    fn default() -> Self {
        Self::new()
    }
}

/// Create every declared channel exactly once. Ends belonging to already
/// launched instances are installed immediately; the rest remain as explicit
/// descriptors until that declared instance is spawned.
pub fn materialize(
    generation: &Generation<'_>,
    launched: &LaunchedInstances,
    channels: &mut ChannelTable,
    graph: &mut GraphTables,
) -> Result<Materialized, ChannelError> {
    let mut report = Materialized::default();
    for grant_index in 0..generation.grant_count() {
        let grant = generation
            .grant(grant_index)
            .map_err(|_| ChannelError::UnlaunchedEndpoint)?;
        let carries = grant.rights & (RIGHT_SEND | RIGHT_RECV);
        if carries == 0 {
            continue;
        }
        // A minted grant declares the edge but not its object: the source
        // creates the channel at runtime and hands the far half to the target
        // at spawn. Pre-creating one here would install a second endpoint at
        // the target's declared slot, shadowing the one it is actually given.
        if grant.minted {
            continue;
        }
        let (GrantEndpoint::Instance(source), GrantEndpoint::Instance(target)) =
            (grant.source, grant.target)
        else {
            continue;
        };
        let loopback = source == target;
        let (producer, consumer) = if carries == RIGHT_RECV {
            (source, target)
        } else {
            (target, source)
        };
        let (key, queues) = channels.push(carries, grant.transferable, loopback)?;
        report.grants += 1;
        report.channels += 1;
        report.queues += queues;
        let producer_side = if loopback {
            Side::Loopback
        } else {
            Side::Producer
        };

        channels.declare_endpoint(DeclaredEndpoint {
            instance: producer,
            grant: grant_index,
            key,
            side: producer_side,
            rights: held_rights(grant.rights, producer, producer),
            holder_exited: false,
            installed: None,
        })?;
        if !loopback {
            channels.declare_endpoint(DeclaredEndpoint {
                instance: consumer,
                grant: grant_index,
                key,
                side: Side::Consumer,
                rights: held_rights(grant.rights, consumer, producer),
                holder_exited: false,
                installed: None,
            })?;
        }
        sel4::debug_println!(
            "SLIME_GRAPH channel grant={} key={key} producer_instance={} consumer_instance={} queues={queues}",
            grant.name,
            producer,
            consumer,
        );
    }

    for launched in launched.iter() {
        report.slots +=
            channels.install_instance(generation, launched.instance, launched.task, graph)?;
    }
    Ok(report)
}

fn binding_slot(
    generation: &Generation<'_>,
    instance_index: usize,
    grant_index: usize,
) -> Result<u32, ChannelError> {
    let instance = generation
        .instance(instance_index)
        .map_err(|_| ChannelError::UnlaunchedEndpoint)?;
    (0..instance.binding_count())
        .filter_map(|index| generation.binding(instance, index).ok())
        .find(|binding| binding.grant == grant_index)
        .and_then(|binding| u32::try_from(binding.slot).ok())
        .ok_or(ChannelError::UnlaidSlot)
}

/// The rights one end actually holds. A grant states what its target may do;
/// the source holds the complementary end unless the grant is bidirectional.
fn held_rights(declared: u64, instance: usize, producer: usize) -> u64 {
    if declared & (RIGHT_SEND | RIGHT_RECV) == RIGHT_SEND | RIGHT_RECV {
        RIGHT_SEND | RIGHT_RECV
    } else if instance == producer {
        RIGHT_SEND
    } else {
        RIGHT_RECV
    }
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
    use super::{ChannelError, ChannelTable, DeathWakes, LaunchedInstances};
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
    fn a_live_declared_instance_cannot_be_recorded_twice() {
        let mut launched = LaunchedInstances::new();
        launched.record(3, 2, TaskId(7)).expect("first launch");
        assert_eq!(
            launched.record(3, 2, TaskId(8)),
            Err(ChannelError::UnlaidSlot)
        );
        assert_eq!(launched.task_for_instance(3), Some(TaskId(7)));
        assert_eq!(launched.len(), 1);
    }

    #[test]
    fn a_reclaimed_instance_can_be_recorded_again_without_multiplying_entries() {
        let mut launched = LaunchedInstances::new();
        launched.record(3, 2, TaskId(7)).expect("first launch");
        assert_eq!(
            launched.release_by_task(TaskId(7)).map(|v| v.instance),
            Some(3)
        );
        assert_eq!(launched.len(), 0);
        launched.record(3, 2, TaskId(8)).expect("respawn");
        assert_eq!(launched.task_for_instance(3), Some(TaskId(8)));
        assert_eq!(launched.len(), 1);
        assert!(launched.release_by_task(TaskId(7)).is_none());
        assert_eq!(launched.len(), 1);
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
}
