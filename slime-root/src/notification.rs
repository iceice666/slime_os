//! Notifications a component waits on (B46).
//!
//! The kernel object replacing the logical wait set. A component that must
//! park across several sources at once cannot do that with Endpoints — seL4
//! receives on exactly one — so each source is minted a badged signal
//! capability into one Notification, and the component waits there and reads
//! the badge to learn which fired.
//!
//! # Why the root still owns the table
//!
//! The *wait* does not go through the root: the child holds the Notification
//! and calls `seL4_Wait` directly, which is the whole point. What the root
//! keeps is the mapping from a source to its bit, because that is authority —
//! which sources may signal a component, and which bits it may legitimately
//! observe. A component that could mint its own badge could impersonate any
//! source its peer trusts.
//!
//! # Bits are per notification, not global
//!
//! Two components may both use bit 0 for their first source. The bit is only
//! meaningful against the notification it was minted into, which is why
//! [`Registration`] names both.

use crate::object_allocator::{AllocError, ObjectAllocator};
use crate::task::TaskId;

/// Notifications the root may create for children.
///
/// One per component that waits on several sources; a component with a single
/// source needs none, because receiving on its one endpoint already blocks.
pub const MAX_NOTIFICATIONS: usize = 16;

/// Sources that may signal one notification.
///
/// Matches `slime_rt::MAX_WAIT_SOURCES`: the badge is a word, so the kernel
/// bounds this far higher, but the component-side wait set is what the bits
/// have to describe. A component asking for more sources than it can name is
/// refused rather than silently given a bit it will never check.
pub const MAX_SOURCES_PER_NOTIFICATION: usize = crate::ipc::MAX_WAIT_SOURCES;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationError {
    /// Every notification slot is taken.
    TableFull,
    /// This notification already names as many sources as it may.
    SourcesExhausted,
    /// No notification with that index.
    Unknown,
    /// The badge asked for is outside what this notification declares.
    UnknownBit,
    Alloc(AllocError),
    /// The badged signal capability could not be minted.
    Mint(sel4::Error),
}

impl From<AllocError> for NotificationError {
    fn from(error: AllocError) -> Self {
        Self::Alloc(error)
    }
}

/// One source's right to signal one bit.
///
/// The `signal` capability is minted with `bit` as its badge, which is how the
/// kernel knows what to OR into the waiter's word — a badge is fixed at mint
/// time, not chosen per signal. That is also what makes this authority rather
/// than a convention: a holder of this capability can set exactly this bit of
/// exactly this notification, and nothing else.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Registration {
    pub notification: u32,
    pub bit: u64,
    /// The badged send capability. Root-held for now; handed to the source
    /// itself once senders stop going through the root.
    pub signal: sel4::cap::Notification,
}

struct Entry {
    /// The waitable capability, handed to the component.
    object: sel4::cap::Notification,
    /// Which task waits here. A notification belongs to one waiter: two
    /// components waiting on one object would each consume the other's wakes,
    /// since a wait clears the badge.
    owner: TaskId,
    /// Bits already handed out, so the next source gets a fresh one and a
    /// caller cannot be given a bit another source already signals.
    assigned: u64,
    /// How many sources are registered, bounded separately from `assigned`
    /// because a released source's bit is not reused — see `register`.
    sources: usize,
}

/// The bit the `sources`-th registration of one notification gets.
///
/// Bits are assigned in order and never reused within a notification's life. A
/// released source leaves its bit retired rather than handing it to the next
/// caller: a signal already in flight from the old source would otherwise
/// arrive as the new one, and the waiter cannot tell them apart.
///
/// Separated from `register` so the assignment rule is testable without a
/// kernel: minting needs real capabilities, but which bit a source gets is
/// arithmetic and is the part that would be wrong quietly.
fn next_bit(sources: usize) -> Result<u64, NotificationError> {
    if sources >= MAX_SOURCES_PER_NOTIFICATION {
        return Err(NotificationError::SourcesExhausted);
    }
    Ok(1u64 << sources)
}

/// Notifications the root has created, and who may signal them.
pub struct NotificationTable {
    entries: [Option<Entry>; MAX_NOTIFICATIONS],
    len: usize,
}

impl NotificationTable {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; MAX_NOTIFICATIONS],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Create a notification for `owner` to wait on.
    ///
    /// Allocated from the task's own arena, so teardown reclaims it with
    /// everything else the task owns and the count still reaches zero.
    pub fn create(
        &mut self,
        allocator: &mut ObjectAllocator,
        arena: crate::object_allocator::TaskArenaId,
        owner: TaskId,
    ) -> Result<u32, NotificationError> {
        let index = self
            .entries
            .iter()
            .position(Option::is_none)
            .ok_or(NotificationError::TableFull)?;
        let object = allocator
            .allocate_fixed_in::<sel4::cap_type::Notification>(arena)?
            .cap();
        self.entries[index] = Some(Entry {
            object,
            owner,
            assigned: 0,
            sources: 0,
        });
        self.len += 1;
        Ok(index as u32)
    }

    /// Give a source the right to signal one bit of `notification`.
    ///
    /// Bits are never reused within a notification's life. A source that is
    /// released leaves its bit retired rather than handing it to the next
    /// caller: a signal already in flight from the old source would otherwise
    /// arrive as the new one, and the waiter has no way to tell them apart.
    pub fn register(
        &mut self,
        allocator: &mut ObjectAllocator,
        arena: crate::object_allocator::TaskArenaId,
        notification: u32,
    ) -> Result<Registration, NotificationError> {
        let entry = self
            .entries
            .get_mut(notification as usize)
            .and_then(Option::as_mut)
            .ok_or(NotificationError::Unknown)?;
        let bit = next_bit(entry.sources)?;
        let signal = allocator
            .reserve_slot_in::<sel4::cap_type::Notification>(arena)?
            .cap();
        let root_cnode = sel4::init_thread::slot::CNODE.cap();
        root_cnode
            .absolute_cptr(signal)
            .mint(
                &root_cnode.absolute_cptr(entry.object),
                // Signal only: a source may wake the waiter and must not
                // consume the wake itself, which `seL4_Wait` on the same
                // object would do.
                sel4::CapRightsBuilder::none().write(true).build(),
                bit,
            )
            .map_err(NotificationError::Mint)?;
        entry.assigned |= bit;
        entry.sources += 1;
        Ok(Registration {
            notification,
            bit,
            signal,
        })
    }

    /// The bits `notification` has handed out, which is the mask its waiter
    /// may legitimately observe.
    pub fn assigned_bits(&self, notification: u32) -> Result<u64, NotificationError> {
        self.entries
            .get(notification as usize)
            .and_then(Option::as_ref)
            .map(|entry| entry.assigned)
            .ok_or(NotificationError::Unknown)
    }

    /// The waitable capability, for installing into its owner's CSpace.
    pub fn object(&self, notification: u32) -> Result<sel4::cap::Notification, NotificationError> {
        self.entries
            .get(notification as usize)
            .and_then(Option::as_ref)
            .map(|entry| entry.object)
            .ok_or(NotificationError::Unknown)
    }

    /// Signal one bit, from the root, on behalf of a source.
    ///
    /// The root signals rather than the source itself while the logical
    /// channels remain: a sender still calls the root, and this is how that
    /// call reaches a waiter that has moved to a notification. Once senders
    /// hold badged capabilities directly this becomes the root's own use only.
    pub fn signal(&self, registration: Registration) -> Result<(), NotificationError> {
        let entry = self
            .entries
            .get(registration.notification as usize)
            .and_then(Option::as_ref)
            .ok_or(NotificationError::Unknown)?;
        if entry.assigned & registration.bit == 0 {
            return Err(NotificationError::UnknownBit);
        }
        // Through the source's own badged capability, so the bit the waiter
        // observes is the one the kernel put there rather than one the root
        // chose at call time. That distinction is what survives the cutover:
        // when the source holds this capability itself, nothing changes here.
        registration.signal.signal();
        Ok(())
    }

    /// Which task waits on `notification`.
    pub fn owner(&self, notification: u32) -> Result<TaskId, NotificationError> {
        self.entries
            .get(notification as usize)
            .and_then(Option::as_ref)
            .map(|entry| entry.owner)
            .ok_or(NotificationError::Unknown)
    }

    /// Drop every notification a task owned, as part of reclaiming it.
    ///
    /// The objects themselves are freed by the task's cleanup record, which
    /// revokes the whole arena; this only forgets the registrations.
    pub fn release_by_task(&mut self, task: TaskId) -> usize {
        let mut released = 0;
        for entry in self.entries.iter_mut() {
            if entry.as_ref().is_some_and(|entry| entry.owner == task) {
                *entry = None;
                released += 1;
            }
        }
        self.len -= released;
        released
    }
}

impl Default for NotificationTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_SOURCES_PER_NOTIFICATION, NotificationError, next_bit};

    #[test]
    fn bits_are_assigned_in_order_and_are_distinct() {
        // Two sources sharing a bit would make a waiter unable to tell which
        // fired, which is the whole reason the badge exists.
        let mut seen = 0u64;
        for source in 0..MAX_SOURCES_PER_NOTIFICATION {
            let bit = next_bit(source).expect("within the bound");
            assert_eq!(bit.count_ones(), 1, "each source gets exactly one bit");
            assert_eq!(seen & bit, 0, "source {source} reuses an assigned bit");
            seen |= bit;
        }
        assert_eq!(seen.count_ones() as usize, MAX_SOURCES_PER_NOTIFICATION);
    }

    #[test]
    fn a_source_past_the_bound_is_refused_rather_than_wrapping() {
        // `1 << 64` is undefined and `1 << 9` on a nine-source bound would
        // silently collide with nothing -- but the component's wait set can
        // only name `MAX_SOURCES_PER_NOTIFICATION`, so a tenth bit is one it
        // would never check.
        assert_eq!(
            next_bit(MAX_SOURCES_PER_NOTIFICATION),
            Err(NotificationError::SourcesExhausted)
        );
        assert_eq!(next_bit(64), Err(NotificationError::SourcesExhausted));
    }

    #[test]
    fn every_assigned_bit_fits_a_badge_word() {
        // The badge is a `u64` the kernel ORs into the waiter's word. A bound
        // that let the shift reach 64 would be undefined behaviour rather than
        // a refusal, so the ceiling has to stay well inside it.
        assert!(
            MAX_SOURCES_PER_NOTIFICATION < u64::BITS as usize,
            "a source's bit must be representable in the badge"
        );
        let highest = next_bit(MAX_SOURCES_PER_NOTIFICATION - 1).expect("the last source");
        assert!(highest.leading_zeros() > 0, "the top bit stays unused");
    }
}
