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
//! different spaces, so both are tracked and each is checked against its own.
//!
//! The two are also learned differently, and that is forced rather than chosen.
//! Declared-space occupancy is root-mediated end to end: every install goes
//! through a root operation, so a credit is complete. Physical occupancy is not
//! — the receiving runtime moves a transferred Endpoint out of
//! `CHILD_SLOT_RECEIVE` into its own transfer region itself
//! (`receive_native`, `components/runtime/src/syscall/sel4_transport.rs`), and
//! the root mediates neither the arrival nor the move. So physical occupancy is
//! *observed*: [`census`] asks the kernel about every slot, which is the only
//! answer that includes what the child did to its own CSpace.

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

/// One child CSpace's occupancy in both spaces, each with its own high-water
/// mark.
///
/// Declared space is credited, because every install into it is a root
/// operation; physical space is observed, because it includes installs the root
/// does not mediate. Each space's peak is raised only by its own kind of
/// evidence: `declared_peak` by a credit the root performed, `physical_peak` by
/// a census the kernel answered. Crossing them would report a number nothing of
/// the right kind ever established.
///
/// The root owns both peaks rather than leaving them to a querying component,
/// because a component sees only the snapshots it asked for. Declared occupancy
/// moves on every capability install, drop, transfer, and retirement between two
/// queries, so a caller sampling twice cannot observe the run's real maximum —
/// the root, which performs every one of those mutations, can and does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CSpaceLedger {
    declared: u32,
    declared_peak: u32,
    populated: u32,
    physical_peak: u32,
}

impl CSpaceLedger {
    pub const EMPTY: Self = Self {
        declared: 0,
        declared_peak: 0,
        populated: 0,
        physical_peak: 0,
    };

    /// A ledger for a freshly constructed task, seeded from the construction
    /// audit's own kernel-observed physical count.
    ///
    /// That seed is an observation, not an estimate, so it sets the physical
    /// peak too: `audit_child_cspace` asked the kernel about every slot in the
    /// CNode before the child ever ran. Declared space starts empty because
    /// construction installs only the root's own fixtures — service, console,
    /// fault, and when self-managed the TCB and CNode root — none of which a
    /// generation grant names or `capabilitySlots` budgets.
    pub const fn seeded(populated: u32) -> Self {
        Self {
            declared: 0,
            declared_peak: 0,
            populated,
            physical_peak: populated,
        }
    }

    /// Slots populated in the space `capabilitySlots` bounds, excluding the
    /// task's own logical authority table, which the caller adds.
    ///
    /// Split that way because the table is the authoritative count of its own
    /// half and already maintains it ([`crate::graph::AuthorityTable::len`]);
    /// mirroring it here would be a second counter to keep in step.
    pub const fn declared_installs(&self) -> u32 {
        self.declared
    }

    /// The highest declared-install count this task ever reached — the natively
    /// installed half of declared space only, matching
    /// [`Self::declared_installs`]. The authority table tracks its own mark.
    pub const fn installs_peak(&self) -> u32 {
        self.declared_peak
    }

    /// The last physically observed slot count.
    pub const fn populated(&self) -> u32 {
        self.populated
    }

    /// The highest physical occupancy any census ever observed.
    pub const fn physical_peak(&self) -> u32 {
        self.physical_peak
    }

    /// Credit `count` newly installed declared capabilities, raising the
    /// declared high-water mark.
    ///
    /// Saturating rather than wrapping: an overcount pinned at `u32::MAX` is
    /// visibly wrong, where a wrapped one reads as an empty CSpace.
    pub const fn installed(&mut self, count: u32) {
        self.declared = self.declared.saturating_add(count);
        if self.declared > self.declared_peak {
            self.declared_peak = self.declared;
        }
    }

    /// Credit `count` released declared capabilities. The peak is a high-water
    /// mark, so releasing never lowers it.
    pub const fn freed(&mut self, count: u32) {
        self.declared = self.declared.saturating_sub(count);
    }

    /// Adopt a kernel-observed physical count as the live value and the peak
    /// candidate. Declared space is untouched: a census cannot tell which
    /// occupied slot a generation named.
    pub const fn observed(&mut self, populated: u32) {
        self.populated = populated;
        if populated > self.physical_peak {
            self.physical_peak = populated;
        }
    }
}

impl Default for CSpaceLedger {
    fn default() -> Self {
        Self::EMPTY
    }
}

/// Whether `populated` breaches a declared ceiling.
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
pub const fn breaches_ceiling(populated: u32, declared: u32) -> bool {
    declared != 0 && populated > declared
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

    /// Each space's peak is raised only by its own kind of evidence: a census
    /// for the physical mark, a credit for the declared one. Crossing them
    /// would report a number nothing of the right kind established.
    #[test]
    fn each_space_peaks_only_on_its_own_evidence() {
        let mut ledger = CSpaceLedger::seeded(5);
        assert_eq!(ledger.populated(), 5);
        assert_eq!(
            ledger.physical_peak(),
            5,
            "the construction audit is an observation"
        );
        assert_eq!(
            ledger.installs_peak(),
            0,
            "construction installs no declared capability"
        );

        ledger.installed(3);
        assert_eq!(ledger.installs_peak(), 3);
        assert_eq!(
            ledger.physical_peak(),
            5,
            "a credit says nothing about physical slots"
        );

        ledger.observed(9);
        assert_eq!(ledger.populated(), 9);
        assert_eq!(ledger.physical_peak(), 9);
        assert_eq!(
            ledger.installs_peak(),
            3,
            "a census says nothing about declared slots"
        );

        ledger.observed(4);
        assert_eq!(ledger.populated(), 4);
        assert_eq!(
            ledger.physical_peak(),
            9,
            "the physical peak is a high-water mark"
        );
    }

    /// A release lowers the live declared count but never its peak: a run that
    /// held 33 declared slots and came back to 29 still held 33, and that is
    /// the number a ceiling report must not lose.
    #[test]
    fn releasing_lowers_the_live_count_but_not_the_peak() {
        let mut ledger = CSpaceLedger::seeded(0);
        ledger.installed(33);
        assert_eq!(ledger.declared_installs(), 33);
        assert_eq!(ledger.installs_peak(), 33);

        ledger.freed(4);
        assert_eq!(ledger.declared_installs(), 29);
        assert_eq!(
            ledger.installs_peak(),
            33,
            "the peak is what the run reached"
        );

        ledger.installed(1);
        assert_eq!(ledger.declared_installs(), 30);
        assert_eq!(ledger.installs_peak(), 33, "still below the earlier mark");
    }

    /// The two spaces are counted independently and neither overwrites the
    /// other. A census cannot tell which occupied slot a generation named, so
    /// it must not disturb the declared count; a credited install is a logical
    /// fact and says nothing about how many physical slots are full.
    #[test]
    fn the_two_spaces_do_not_overwrite_each_other() {
        let mut ledger = CSpaceLedger::seeded(3);
        ledger.installed(4);
        assert_eq!(ledger.declared_installs(), 4);
        assert_eq!(ledger.populated(), 3, "a credit is not a physical count");

        // The child also accepted two native transfers and moved both out of
        // its receive slot. The root mediated neither, so only a census sees
        // them, and seeing them says nothing about declared space.
        ledger.observed(9);
        assert_eq!(ledger.populated(), 9);
        assert_eq!(
            ledger.declared_installs(),
            4,
            "a census must not clobber the credited count"
        );
    }

    /// Neither credit direction may wrap: a wrapped free reads as a full
    /// CSpace and a wrapped install reads as an empty one, and both would be
    /// reported as occupancy rather than as the accounting bug they are.
    #[test]
    fn credits_saturate_rather_than_wrap() {
        let mut ledger = CSpaceLedger::seeded(0);
        ledger.freed(1);
        assert_eq!(
            ledger.declared_installs(),
            0,
            "freeing an empty CSpace stays empty"
        );

        ledger.installed(u32::MAX);
        ledger.installed(2);
        assert_eq!(ledger.declared_installs(), u32::MAX);

        let mut ledger = CSpaceLedger::seeded(0);
        ledger.installed(6);
        ledger.freed(4);
        assert_eq!(ledger.declared_installs(), 2);
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
