//! Capabilities in flight between a `send` and the `recv` that collects them.
//!
//! A Slime capability is a logical slot in a task's own table, so a slot number
//! means nothing outside the table it indexes. Moving one therefore cannot be
//! done by putting the sender's number in the message: the receiver's table
//! numbers its slots independently, and a number that resolved to a loan in one
//! would resolve to an executable — or to nothing — in the other.
//!
//! So a move is three steps, and the capability is root-held for the middle one:
//!
//! 1. `send` removes the capability from the sender's table and parks it here,
//!    putting the returned **token** in the message.
//! 2. The message sits in its queue. The capability belongs to no task: the
//!    sender can no longer name it, and the receiver cannot yet.
//! 3. `recv` takes the token back, installs the capability into the receiver's
//!    table at a slot that table chose, and reports that slot.
//!
//! A token is a fresh number every time, never a table index, so a token
//! observed in one message can never name a capability parked later. Nothing
//! outside the root ever sees one — the component reads only the slot number
//! step 3 assigns.
//!
//! # Why an entry records the sender and destination end
//!
//! A parked capability is owned by no task, which is exactly what makes it a
//! leak if the sender dies or the destination end disappears while it sits
//! here. The destination is a **channel end**, not a receiver task: since B25
//! more than one task may hold one end, and whoever dequeues the message is the
//! receiver. Binding its attached capabilities to the same queue end preserves
//! that one delivery decision instead of trying to predict a winning task at
//! send time.
//!
//! [`Transit::reclaim`] drops an entry when its sender dies or the last holder
//! of its destination end dies. Dropping the entry is all this table owes. The
//! underlying resource is reclaimed by whoever owns it —
//! [`SharedBufferTable::reclaim_holder`](crate::shared_buffer::SharedBufferTable::reclaim_holder)
//! settles the lender's loans — and a capability is a name for a resource, not
//! the resource.

use crate::graph::{Capability, GraphTables, Resource, Side};
use crate::ipc::{ChannelKey, IpcError, LogicalCap};
use crate::task::TaskId;

/// Capabilities that may be in flight across the whole graph at once.
///
/// Deliberately small. A capability parked here is one no task can name, so a
/// large ceiling would mostly buy the ability to strand more of them before
/// anything noticed; a bounded refusal at the send is the better failure.
///
/// Sixteen is `ipc::MAX_MESSAGE_CAPS` — one full message's worth — times the
/// four concurrent transfers any declared graph performs, so no graph this
/// cutover boots can reach it. A send past it is refused as an ordinary Slime
/// error rather than dropping a capability.
pub const MAX_TRANSIT: usize = 16;

struct Entry {
    token: LogicalCap,
    capability: Capability,
    /// The task that parked it. Its table no longer holds the capability.
    sender: TaskId,
    /// The queue end whose `recv` may collect it.
    channel: ChannelKey,
    receiving_side: Side,
}

/// Capabilities parked between their send and their receive.
pub struct Transit {
    entries: [Option<Entry>; MAX_TRANSIT],
    len: usize,
    /// Next token. Monotonic and never reused within an epoch, so a token from
    /// a settled transfer names nothing rather than something new.
    next: LogicalCap,
}

impl Transit {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; MAX_TRANSIT],
            len: 0,
            next: 1,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Park `capability` on its way from `sender` to one channel end, returning
    /// the token the message carries.
    ///
    /// The caller must already have removed it from `sender`'s table: this
    /// takes ownership of a capability no table holds, and a caller that parked
    /// one it had left installed would have duplicated it.
    pub fn depart(
        &mut self,
        capability: Capability,
        sender: TaskId,
        channel: ChannelKey,
        receiving_side: Side,
    ) -> Result<LogicalCap, IpcError> {
        let token = self.next;
        let next = self
            .next
            .checked_add(1)
            .ok_or(IpcError::DestinationSlotsExhausted)?;
        let slot = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or(IpcError::DestinationSlotsExhausted)?;
        *slot = Some(Entry {
            token,
            capability,
            sender,
            channel,
            receiving_side,
        });
        self.next = next;
        self.len += 1;
        Ok(token)
    }

    /// Whether any parked capability is a supervision handle naming `task`.
    ///
    /// The second half of [`crate::supervision::sweep`]'s predicate, and the
    /// half whose absence would be invisible. A capability in transit is held
    /// by no table by construction — that is what this module owns — so a sweep
    /// consulting only [`crate::graph::GraphTables`] would find no holder for a
    /// handle mid-transfer, free the record, and leave the receiver's
    /// `supervision_status` answering `WouldBlock` forever. That is backlog B16
    /// exactly, reintroduced by its own fix.
    ///
    /// Keyed by resource rather than by token, unlike every other method here:
    /// the sweep asks what is in flight, not which transfer is which.
    pub fn holds_supervision(&self, task: TaskId) -> bool {
        self.entries
            .iter()
            .flatten()
            .any(|entry| entry.capability.resource == Resource::Supervision { task })
    }

    /// Whether any parked capability is an endpoint end naming `channel`.
    ///
    /// The in-flight half of [`crate::channel::sweep`]'s predicate, and the
    /// half whose absence would be invisible — for exactly the reason
    /// [`Self::holds_supervision`] gives. `serve_cap_transfer` drops the
    /// capability from the sender's table and only then parks it here, so a
    /// sweep firing in that window and reading only
    /// [`GraphTables`](crate::graph::GraphTables) would free the channel the
    /// transfer is moving. The receiver would then collect a capability
    /// resolving to no queue and park on it forever.
    ///
    /// This also covers an endpoint riding as a *message* payload: a queued
    /// message carries the transit token, and the entry behind it stands until
    /// [`Self::arrive`] takes it, so a scan here sees the end while it sits in
    /// some third channel's queue.
    pub fn holds_endpoint(&self, channel: u32) -> bool {
        self.entries.iter().flatten().any(|entry| {
            matches!(entry.capability.resource, Resource::Endpoint { channel: key, .. } if key == channel)
        })
    }

    /// Take the capability `token` names if it arrived on the end it was sent
    /// to.
    ///
    /// The check follows the queue, not a preselected task. Two holders of one
    /// receive end already race to dequeue a message; the task that wins that
    /// single race receives both the bytes and their capabilities. Supplying a
    /// different channel or side cannot redirect a leaked or requeued token.
    pub fn arrive(
        &mut self,
        token: LogicalCap,
        channel: ChannelKey,
        receiving_side: Side,
    ) -> Option<Capability> {
        let slot = self.entries.iter_mut().find(|entry| {
            entry.as_ref().is_some_and(|entry| {
                entry.token == token
                    && entry.channel == channel
                    && entry.receiving_side.reaches(receiving_side)
            })
        })?;
        let entry = slot.take()?;
        self.len -= 1;
        Some(entry.capability)
    }

    /// Take a capability back to the task that parked it, by token.
    ///
    /// Used when a send fails after its capabilities were parked: the sender
    /// still holds them as far as it knows, so they are returned to its table
    /// rather than stranded here.
    pub fn recall(&mut self, token: LogicalCap, sender: TaskId) -> Option<Capability> {
        let slot = self.entries.iter_mut().find(|entry| {
            entry
                .as_ref()
                .is_some_and(|entry| entry.token == token && entry.sender == sender)
        })?;
        let entry = slot.take()?;
        self.len -= 1;
        Some(entry.capability)
    }

    /// Drop every entry whose sender is `task` or whose destination end loses
    /// its last holder, reporting how many.
    ///
    /// The dying task's table is still installed, so the holder query excludes
    /// it explicitly. A co-holder keeps the entry collectible.
    pub fn reclaim(&mut self, graph: &GraphTables, task: TaskId) -> usize {
        let mut dropped = 0;
        for slot in self.entries.iter_mut() {
            let should_drop = slot.as_ref().is_some_and(|entry| {
                entry.sender == task
                    || (graph.get(task).is_some_and(|table| {
                        table.reaches_endpoint(entry.channel, entry.receiving_side)
                    }) && !graph.holds_endpoint_side(
                        entry.channel,
                        entry.receiving_side,
                        Some(task),
                    ))
            });
            if should_drop {
                *slot = None;
                self.len -= 1;
                dropped += 1;
            }
        }
        dropped
    }
}

impl Default for Transit {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GraphTables, Resource, Side};
    use crate::shared_buffer::{BufferId, GenerationEpoch, HolderId, LoanHandle, LoanId};

    const SENDER: TaskId = TaskId(1);
    const RECEIVER: TaskId = TaskId(2);
    const COHOLDER: TaskId = TaskId(3);
    const CHANNEL: ChannelKey = 7;

    fn loan(id: u64) -> Capability {
        Capability {
            resource: Resource::Loan {
                handle: LoanHandle {
                    id: LoanId(id),
                    buffer: BufferId(1),
                    epoch: GenerationEpoch(1),
                    receiver: HolderId(u64::from(RECEIVER.0)),
                },
            },
            rights: 0,
        }
    }

    fn hold(graph: &mut GraphTables, task: TaskId, side: Side) {
        if graph.get(task).is_none() {
            graph.create(task).expect("create");
        }
        let table = graph.get_mut(task).expect("table");
        let slot = table.free_slot_from(0).expect("slot");
        table
            .install(
                slot,
                Capability {
                    resource: Resource::Endpoint {
                        channel: CHANNEL,
                        side,
                    },
                    rights: 0,
                },
            )
            .expect("install");
    }

    #[test]
    fn a_parked_capability_reaches_the_destination_end() {
        let mut transit = Transit::new();
        let token = transit
            .depart(loan(7), SENDER, CHANNEL, Side::Consumer)
            .unwrap();
        assert_eq!(transit.len(), 1);
        assert_eq!(
            transit.arrive(token, CHANNEL, Side::Consumer),
            Some(loan(7))
        );
        assert_eq!(transit.len(), 0, "collecting empties the entry");
        assert_eq!(transit.arrive(token, CHANNEL, Side::Consumer), None);
    }

    #[test]
    fn the_wrong_channel_or_side_cannot_collect_a_capability() {
        let mut transit = Transit::new();
        let token = transit
            .depart(loan(7), SENDER, CHANNEL, Side::Consumer)
            .unwrap();
        assert_eq!(transit.arrive(token, CHANNEL + 1, Side::Consumer), None);
        assert_eq!(transit.arrive(token, CHANNEL, Side::Producer), None);
        assert_eq!(transit.len(), 1, "refusal leaves the capability parked");
        assert_eq!(
            transit.arrive(token, CHANNEL, Side::Consumer),
            Some(loan(7))
        );
    }

    #[test]
    fn either_coholder_can_collect_from_the_same_destination_end() {
        let mut transit = Transit::new();
        let token = transit
            .depart(loan(7), SENDER, CHANNEL, Side::Consumer)
            .unwrap();
        let mut graph = GraphTables::new();
        hold(&mut graph, RECEIVER, Side::Consumer);
        hold(&mut graph, COHOLDER, Side::Consumer);
        assert!(graph.holds_endpoint_side(CHANNEL, Side::Consumer, None));
        assert_eq!(
            transit.arrive(token, CHANNEL, Side::Consumer),
            Some(loan(7)),
            "delivery follows the queue end, not a task chosen at send time"
        );
    }

    #[test]
    fn tokens_are_never_reused() {
        let mut transit = Transit::new();
        let first = transit
            .depart(loan(1), SENDER, CHANNEL, Side::Consumer)
            .unwrap();
        transit
            .arrive(first, CHANNEL, Side::Consumer)
            .expect("arrive");
        let second = transit
            .depart(loan(2), SENDER, CHANNEL, Side::Consumer)
            .unwrap();
        assert_ne!(first, second, "a settled token never names a fresh entry");
        assert_eq!(transit.arrive(first, CHANNEL, Side::Consumer), None);
    }

    #[test]
    fn a_failed_send_recalls_to_its_sender_only() {
        let mut transit = Transit::new();
        let token = transit
            .depart(loan(7), SENDER, CHANNEL, Side::Consumer)
            .unwrap();
        assert_eq!(transit.recall(token, RECEIVER), None, "recall is by sender");
        assert_eq!(transit.recall(token, SENDER), Some(loan(7)));
        assert_eq!(transit.len(), 0);
    }

    #[test]
    fn destination_death_reclaims_only_after_the_last_holder_dies() {
        let mut graph = GraphTables::new();
        hold(&mut graph, RECEIVER, Side::Consumer);
        hold(&mut graph, COHOLDER, Side::Consumer);
        let mut transit = Transit::new();
        transit
            .depart(loan(1), SENDER, CHANNEL, Side::Consumer)
            .unwrap();
        assert_eq!(
            transit.reclaim(&graph, RECEIVER),
            0,
            "co-holder keeps it collectible"
        );
        graph.release(RECEIVER);
        assert_eq!(
            transit.reclaim(&graph, COHOLDER),
            1,
            "last holder removes destination"
        );
        assert!(transit.is_empty());
    }

    #[test]
    fn sender_death_reclaims_the_entry() {
        let mut transit = Transit::new();
        transit
            .depart(loan(3), SENDER, CHANNEL, Side::Consumer)
            .unwrap();
        assert_eq!(transit.reclaim(&GraphTables::new(), SENDER), 1);
        assert!(transit.is_empty());
    }

    #[test]
    fn the_table_refuses_rather_than_dropping_a_capability() {
        let mut transit = Transit::new();
        for index in 0..MAX_TRANSIT {
            transit
                .depart(loan(index as u64), SENDER, CHANNEL, Side::Consumer)
                .unwrap();
        }
        assert_eq!(
            transit.depart(loan(99), SENDER, CHANNEL, Side::Consumer),
            Err(IpcError::DestinationSlotsExhausted),
        );
        assert_eq!(transit.len(), MAX_TRANSIT);
    }
}
