//! Live child-CSpace occupancy, in both spaces a child's slots live in
//! (C8.13.3).
//!
//! [`crate::object_allocator::ArenaRecord`] tracks what the root *allocated*
//! building a task; neither it nor anything else answered how many of a live
//! child's slots hold a capability right now. This module does, and it has to
//! answer twice, because a child's slots are counted two different ways by two
//! different bounds:
//!
//! - **Declared space.** A generation names a child's authority in the
//!   component's own logical numbering from 0, and `capabilitySlots` is the
//!   ceiling on *that*: `scripts/build/build-generation.py` derives its required
//!   value as `FABRIC_FIRST_CONTROL_SLOT + control endpoints + buffers`, and
//!   `fabric_graph_is_satisfiable` validates it against
//!   [`crate::graph::MAX_TASK_CAPS`] (64), not against the CNode. Occupancy here
//!   is the declared native capabilities the root installed plus the entries in
//!   the task's own [`crate::graph::AuthorityTable`].
//! - **Physical space.** The CNode itself holds `1 << cnode_size_bits` (128)
//!   slots at fixed addresses — service, console, fault, TCB, CNode root, the
//!   native endpoint and notification regions, the authority mirrors, the
//!   receive slot (`crate::task`'s slot map). Its bound is the CNode's own
//!   capacity, and a logical index of 3 lives at physical 36.
//!
//! Reporting one number against the other bound would compare quantities from
//! different spaces, so both are answered and each is checked against its own.
//!
//! Only one of them is *retained*, and that asymmetry is forced rather than
//! chosen. Declared-space occupancy is root-mediated end to end — every install
//! goes through a root operation — so the root can accumulate it, and must, to
//! hold a high-water mark across mutations no single reader observes. Physical
//! occupancy is not root-mediated: the receiving runtime moves a transferred
//! Endpoint out of `CHILD_SLOT_RECEIVE` into its own transfer region itself
//! (`receive_native`, `components/runtime/src/syscall/sel4_transport.rs`). A
//! stored physical count could therefore only be stale, so [`census`] asks the
//! kernel on every read and nothing caches the answer.

/// Why a child CSpace could not be counted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CSpaceError {
    /// A slot answered neither "occupied" nor "empty". The slot is not
    /// addressable at the depth the task record carries, which is a layout
    /// defect rather than an occupancy answer.
    Unaddressable { slot: sel4::CPtrBits },
    /// The recorded CNode size is not one this root can enumerate.
    BadSize { size_bits: usize },
}

/// One child's declared-space slot occupancy, live and at its high-water mark.
///
/// Only declared space is retained. Physical occupancy is not stored, because
/// it is never answerable from stored state: the child fills physical slots the
/// root does not mediate, so every reader has to take a fresh [`census`] and
/// there is nothing a cached copy could add. Declared space is the opposite —
/// every install into it is a root operation, so the root can and must
/// accumulate it.
///
/// The peak is tracked here rather than sampled by a reader because declared
/// occupancy moves on every install, drop, transfer, and retirement. A caller
/// sampling twice sees the higher of two snapshots, not the run's maximum.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CSpaceLedger {
    installs: u32,
    installs_peak: u32,
}

impl CSpaceLedger {
    pub const EMPTY: Self = Self {
        installs: 0,
        installs_peak: 0,
    };

    /// Slots populated in the space `capabilitySlots` bounds, excluding the
    /// task's own logical authority table, which the caller adds.
    ///
    /// Split that way because the table is the authoritative count of its own
    /// half and already maintains it ([`crate::graph::AuthorityTable::len`]);
    /// mirroring it here would be a second counter to keep in step.
    pub const fn declared_installs(&self) -> u32 {
        self.installs
    }

    /// The highest declared-install count this task ever reached — the natively
    /// installed half of declared space only, matching
    /// [`Self::declared_installs`]. The authority table tracks its own mark.
    pub const fn installs_peak(&self) -> u32 {
        self.installs_peak
    }

    /// Credit `count` newly installed declared capabilities, raising the
    /// high-water mark.
    ///
    /// Saturating rather than wrapping: an overcount pinned at `u32::MAX` is
    /// visibly wrong, where a wrapped one reads as an empty CSpace.
    pub const fn installed(&mut self, count: u32) {
        self.installs = self.installs.saturating_add(count);
        if self.installs > self.installs_peak {
            self.installs_peak = self.installs;
        }
    }
}

impl Default for CSpaceLedger {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// Whether `occupancy` breaches a declared ceiling.
///
/// A zero ceiling is *no* ceiling rather than a ceiling of zero: a generation
/// that declares no fabric graph declares no slot budget, and refusing every
/// child of such a generation would make the absence of a graph an authority
/// failure.
///
/// A graph that *does* declare the field may still declare zero —
/// `boot_contracts::fabric_graph` bounds `capability_slots` only from above
/// (`validate_declared_limits`, `validate_against`), with no nonzero arm like
/// the one `limits.buffers` carries. Such a graph is therefore also treated as
/// declaring no ceiling. That is the conservative reading: this predicate feeds
/// a report, not a refusal, and inventing a ceiling of zero would name every
/// live child a breach for holding authority the root gave it.
pub const fn breaches_ceiling(occupancy: u32, declared: u32) -> bool {
    declared != 0 && occupancy > declared
}

/// Count the populated slots in one child CSpace, by asking the kernel.
///
/// Occupancy is probed with `seL4_CNode_Move` onto the slot itself, the same
/// read-only test `crate::task`'s construction audit uses: the kernel refuses a
/// move whose destination is occupied (`DeleteFirst`) and one whose source is
/// empty (`FailedLookup`), and moves nothing in either case. No scratch slot is
/// needed and no capability under audit can be destroyed.
///
/// Every slot is probed, including ones no plan named. That is the point: a
/// capability the child moved into its own transfer region was installed by
/// nobody the root can ask, so a census restricted to declared regions would
/// answer the question the root already knew the answer to.
pub fn census(cnode: sel4::cap::CNode, cnode_size_bits: usize) -> Result<u32, CSpaceError> {
    if cnode_size_bits == 0 || cnode_size_bits >= usize::BITS as usize {
        return Err(CSpaceError::BadSize {
            size_bits: cnode_size_bits,
        });
    }
    let capacity = 1u64 << cnode_size_bits;
    let mut populated = 0;
    for slot in 0..capacity {
        let slot = slot as sel4::CPtrBits;
        let cptr = cnode.absolute_cptr_from_bits_with_depth(slot, cnode_size_bits);
        match cptr.move_(&cptr) {
            Err(sel4::Error::DeleteFirst) => populated += 1,
            Err(sel4::Error::FailedLookup) => {}
            _ => return Err(CSpaceError::Unaddressable { slot }),
        }
    }
    Ok(populated)
}

/// How many slots a CNode of this size holds, saturated into the reply's
/// 16-bit field width rather than wrapped.
pub const fn capacity_of(cnode_size_bits: usize) -> u32 {
    if cnode_size_bits >= u32::BITS as usize {
        u32::MAX
    } else {
        1u32 << cnode_size_bits
    }
}

#[cfg(test)]
mod tests {
    use super::{CSpaceLedger, breaches_ceiling, capacity_of};

    /// The peak is the root's high-water mark over declared installs, tracked
    /// as they happen. It exists because no single reader sees every mutation:
    /// a caller sampling twice reports the higher of two snapshots, which is a
    /// smaller claim than the run's maximum.
    #[test]
    fn the_peak_is_a_high_water_mark_over_credits() {
        let mut ledger = CSpaceLedger::EMPTY;
        assert_eq!(ledger.declared_installs(), 0);
        assert_eq!(
            ledger.installs_peak(),
            0,
            "construction installs no declared capability"
        );

        ledger.installed(33);
        assert_eq!(ledger.declared_installs(), 33);
        assert_eq!(ledger.installs_peak(), 33);

        ledger.installed(1);
        assert_eq!(ledger.declared_installs(), 34);
        assert_eq!(ledger.installs_peak(), 34, "a credit raises the mark");
    }

    /// A credit may not wrap: a wrapped install reads as an empty CSpace, which
    /// is the one answer a bounded count must never give — it would turn a
    /// ceiling breach into a holder that appears to hold nothing.
    #[test]
    fn credits_saturate_rather_than_wrap() {
        let mut ledger = CSpaceLedger::EMPTY;
        ledger.installed(u32::MAX);
        ledger.installed(2);
        assert_eq!(ledger.declared_installs(), u32::MAX);
        assert_eq!(ledger.installs_peak(), u32::MAX);
    }

    /// A zero ceiling is no ceiling, whether it arrived from a generation with
    /// no fabric graph or from a graph that declared the field as zero —
    /// `boot_contracts::fabric_graph` bounds `capability_slots` only from
    /// above, so both reach this predicate as `0`. Treating either as a real
    /// ceiling would name every child a breach for holding authority the root
    /// installed.
    #[test]
    fn a_zero_ceiling_is_never_treated_as_a_ceiling() {
        assert!(!breaches_ceiling(128, 0));
        assert!(!breaches_ceiling(0, 0));
        assert!(!breaches_ceiling(48, 48), "at the ceiling is admissible");
        assert!(breaches_ceiling(49, 48));
        assert!(!breaches_ceiling(47, 48));
    }

    #[test]
    fn capacity_follows_the_cnode_size() {
        assert_eq!(capacity_of(7), 128, "the child CNode this root builds");
        assert_eq!(capacity_of(0), 1);
        assert_eq!(
            capacity_of(u32::BITS as usize),
            u32::MAX,
            "a size no CNode has still yields a total answer"
        );
    }
}
