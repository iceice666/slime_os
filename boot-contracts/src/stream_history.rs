//! Bounded KEEP_LAST history for one stream subscriber (C8.4).
//!
//! A subscriber's declared `history_depth` is the whole of its buffering: the
//! fabric holds at most that many undelivered samples for it, and admitting one
//! more evicts the oldest. The rule has to be exact rather than approximate —
//! "evicts the oldest sequence at the declared depth" is the milestone's check,
//! and a ring that dropped the newest, or dropped two, would still look healthy
//! from a transcript that only counts arrivals.
//!
//! Two properties this type exists to make true:
//!
//! 1. **Bounded by construction.** Capacity is fixed at [`MAX_HISTORY`] slots
//!    and the live depth is whatever the generation declared, so a stalled
//!    subscriber costs a fixed number of entries no matter how long it stalls.
//!    There is no growth path: `push` on a full ring evicts, never allocates.
//! 2. **Loss is counted, not silent.** Every eviction returns the evicted entry
//!    and bumps a saturating loss counter alongside the oldest lost sequence, so
//!    BEST_EFFORT delivery can report exactly what a subscriber missed without
//!    retaining the samples themselves.
//!
//! Deliberately in `boot-contracts` rather than the fabric component: the
//! eviction order is a contract the gate asserts, and here it is unit-testable
//! on the host without a boot.

use crate::fabric_graph::LIMIT_HISTORY_DEPTH;

/// Fixed slot capacity, matching the fabric-graph contract's history ceiling.
/// A declared depth above this cannot be honoured and is rejected by
/// [`StreamHistory::new`]; generation admission rejects it earlier still.
pub const MAX_HISTORY: usize = LIMIT_HISTORY_DEPTH as usize;

/// One queued sample, stored by reference to the payload the fabric owns
/// rather than by value: an entry names either inline bytes already copied into
/// the fabric's own frame, or the fabric-owned sealed buffer a large sample was
/// copied into exactly once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryEntry {
    /// Publisher-assigned sequence. Monotonic per publisher; the fabric does
    /// not renumber, so a subscriber sees the gap an eviction left.
    pub sequence: u64,
    /// Index of the publisher whose per-edge sequence this sample carries.
    pub publisher: u32,
    /// Index into the fabric's own sample storage.
    pub slot: u32,
    /// Whether this entry is carried inline or through a shared-buffer loan.
    pub inline: bool,
}

/// A bounded per-subscriber KEEP_LAST ring with loss accounting.
#[derive(Debug, Clone, Copy)]
pub struct StreamHistory {
    entries: [Option<HistoryEntry>; MAX_HISTORY],
    head: usize,
    len: usize,
    depth: usize,
    lost: u64,
    oldest_lost: u64,
}

impl StreamHistory {
    /// A ring holding at most `depth` samples. `depth` must be at least one and
    /// at most [`MAX_HISTORY`]; KEEP_LAST has no unbounded form, so a zero depth
    /// is a malformed policy rather than "keep nothing".
    pub const fn new(depth: usize) -> Option<Self> {
        if depth == 0 || depth > MAX_HISTORY {
            return None;
        }
        Some(Self {
            entries: [None; MAX_HISTORY],
            head: 0,
            len: 0,
            depth,
            lost: 0,
            oldest_lost: 0,
        })
    }

    pub fn depth(&self) -> usize {
        self.depth
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn is_full(&self) -> bool {
        self.len == self.depth
    }

    /// Samples dropped by eviction since this ring was created. Saturating: a
    /// counter that wrapped would under-report loss, which is worse than
    /// pinning at the ceiling.
    pub fn lost(&self) -> u64 {
        self.lost
    }

    /// Sequence of the oldest sample lost since the last [`Self::take_loss`],
    /// or zero when nothing was lost.
    pub fn oldest_lost(&self) -> u64 {
        self.oldest_lost
    }

    /// Consume the pending loss report, returning `(count, oldest_sequence)`
    /// and clearing it. The fabric calls this when delivery resumes, so one
    /// stall produces exactly one event rather than one per dropped sample.
    pub fn take_loss(&mut self) -> Option<(u64, u64)> {
        if self.lost == 0 {
            return None;
        }
        let report = (self.lost, self.oldest_lost);
        self.lost = 0;
        self.oldest_lost = 0;
        Some(report)
    }

    /// Attribute losses this ring did not itself evict.
    ///
    /// Exists for one caller: a restart salvaging a dead subscriber's backlog
    /// into a replacement's history. Samples still in the abandoned ring, and
    /// the dead record's own untaken report, would otherwise vanish with the
    /// record they lived on — leaving a truncated stream indistinguishable from
    /// a complete one. `inherited` is a previously taken `(count, oldest)`
    /// report being carried forward; `abandoned` is a count with no sequence of
    /// its own, so it only establishes `oldest_lost` when nothing older is
    /// already recorded.
    pub fn note_loss(&mut self, abandoned: u64, inherited: Option<(u64, u64)>) {
        if let Some((count, oldest)) = inherited {
            if self.lost == 0 || oldest < self.oldest_lost {
                self.oldest_lost = oldest;
            }
            self.lost = self.lost.saturating_add(count);
        }
        if abandoned == 0 {
            return;
        }
        if self.lost == 0 {
            // No sequence is known for a sample abandoned unread, so the newest
            // salvaged sequence is the closest true lower bound available.
            self.oldest_lost = if self.len == 0 {
                0
            } else {
                let newest = (self.head + self.len - 1) % self.depth;
                self.entries[newest].map_or(0, |entry| entry.sequence)
            };
        }
        self.lost = self.lost.saturating_add(abandoned);
    }

    /// Admit `entry`, evicting and returning the oldest sample when the ring is
    /// already at its declared depth. The evicted entry is returned so the
    /// caller can release whatever backing storage it named; the loss counter
    /// is bumped here so no caller can drop a sample without recording it.
    pub fn push(&mut self, entry: HistoryEntry) -> Option<HistoryEntry> {
        let evicted = if self.is_full() { self.pop() } else { None };
        if let Some(evicted) = evicted {
            if self.lost == 0 {
                self.oldest_lost = evicted.sequence;
            }
            self.lost = self.lost.saturating_add(1);
        }
        let index = (self.head + self.len) % self.depth;
        self.entries[index] = Some(entry);
        self.len += 1;
        evicted
    }

    /// Remove and return the oldest queued sample.
    pub fn pop(&mut self) -> Option<HistoryEntry> {
        if self.len == 0 {
            return None;
        }
        let entry = self.entries[self.head].take();
        self.head = (self.head + 1) % self.depth;
        self.len -= 1;
        entry
    }

    /// The oldest queued sample without removing it.
    pub fn peek(&self) -> Option<HistoryEntry> {
        self.entry_at(0)
    }

    /// The queued sample `offset` places behind the oldest, without removing
    /// it.
    ///
    /// Delivery needs this and not just [`Self::peek`]: a sample stays in the
    /// ring until its ack settles it, so a fabric that only ever looked at the
    /// head would re-send the same sequence for every free delivery slot. The
    /// caller passes its in-flight count, so `entry_at` names the first sample
    /// it has not sent yet.
    pub fn entry_at(&self, offset: usize) -> Option<HistoryEntry> {
        if offset >= self.len {
            return None;
        }
        self.entries[(self.head + offset) % self.depth]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(sequence: u64) -> HistoryEntry {
        HistoryEntry {
            sequence,
            publisher: 0,
            slot: sequence as u32,
            inline: true,
        }
    }

    #[test]
    fn zero_or_oversized_depth_is_rejected() {
        assert!(StreamHistory::new(0).is_none());
        assert!(StreamHistory::new(MAX_HISTORY + 1).is_none());
        assert!(StreamHistory::new(1).is_some());
        assert!(StreamHistory::new(MAX_HISTORY).is_some());
    }

    /// The exact oldest sequence is evicted, and only one per admission.
    #[test]
    fn keep_last_evicts_the_exact_oldest_sequence() {
        let mut history = StreamHistory::new(2).expect("depth");
        assert_eq!(history.push(entry(1)), None);
        assert_eq!(history.push(entry(2)), None);
        assert!(history.is_full());
        // At depth, admitting sequence 3 evicts sequence 1 — not 2, not both.
        assert_eq!(history.push(entry(3)), Some(entry(1)));
        assert_eq!(history.len(), 2);
        assert_eq!(history.pop(), Some(entry(2)));
        assert_eq!(history.pop(), Some(entry(3)));
        assert_eq!(history.pop(), None);
    }

    /// A stalled subscriber costs a fixed number of entries however long it
    /// stalls, and the loss report names the count and the oldest lost.
    #[test]
    fn a_stalled_subscriber_is_bounded_and_reports_its_loss() {
        let mut history = StreamHistory::new(3).expect("depth");
        for sequence in 1..=100 {
            history.push(entry(sequence));
            assert!(history.len() <= 3);
        }
        // 100 admitted, 3 retained, so 97 were evicted starting at sequence 1.
        assert_eq!(history.lost(), 97);
        assert_eq!(history.oldest_lost(), 1);
        assert_eq!(history.take_loss(), Some((97, 1)));
        // One stall produces one report: the counter clears on take.
        assert_eq!(history.take_loss(), None);
        // The retained window is the newest `depth` sequences.
        assert_eq!(history.pop(), Some(entry(98)));
        assert_eq!(history.pop(), Some(entry(99)));
        assert_eq!(history.pop(), Some(entry(100)));
    }

    /// A ring drained before it fills never reports loss, so a keeping-up
    /// subscriber cannot be told it missed something.
    #[test]
    fn draining_at_depth_never_reports_loss() {
        let mut history = StreamHistory::new(2).expect("depth");
        for sequence in 1..=50 {
            assert_eq!(history.push(entry(sequence)), None);
            assert_eq!(history.pop(), Some(entry(sequence)));
        }
        assert_eq!(history.lost(), 0);
        assert_eq!(history.take_loss(), None);
        assert!(history.is_empty());
    }

    /// The ring wraps without corrupting order: peek always names the oldest.
    #[test]
    fn wrapping_preserves_order() {
        let mut history = StreamHistory::new(3).expect("depth");
        history.push(entry(1));
        history.push(entry(2));
        assert_eq!(history.peek(), Some(entry(1)));
        assert_eq!(history.pop(), Some(entry(1)));
        history.push(entry(3));
        history.push(entry(4));
        assert_eq!(history.peek(), Some(entry(2)));
        assert_eq!(history.push(entry(5)), Some(entry(2)));
        assert_eq!(history.pop(), Some(entry(3)));
        assert_eq!(history.pop(), Some(entry(4)));
        assert_eq!(history.pop(), Some(entry(5)));
        assert!(history.is_empty());
    }

    /// A salvaged history reports the backlog it could not keep. Without this a
    /// restart that dropped samples would present a truncated stream as a
    /// complete one, which is the failure the salvage path exists to prevent.
    #[test]
    fn salvage_attributes_abandoned_samples_and_inherited_loss() {
        let mut history = StreamHistory::new(2).expect("depth");
        history.push(entry(7));
        history.push(entry(8));
        // Two samples the frame table had no room for, and nothing inherited.
        history.note_loss(2, None);
        // The newest salvaged sequence is the lower bound for an unread sample.
        assert_eq!(history.take_loss(), Some((2, 8)));

        // An inherited report keeps its own, older sequence.
        let mut history = StreamHistory::new(2).expect("depth");
        history.push(entry(7));
        history.note_loss(1, Some((5, 2)));
        assert_eq!(history.take_loss(), Some((6, 2)));
    }

    /// Salvaging a fully-kept backlog reports nothing: a restart that lost no
    /// sample must not manufacture a gap.
    #[test]
    fn salvage_without_abandonment_reports_no_loss() {
        let mut history = StreamHistory::new(2).expect("depth");
        history.push(entry(3));
        history.note_loss(0, None);
        assert_eq!(history.lost(), 0);
        assert_eq!(history.take_loss(), None);
    }
}
