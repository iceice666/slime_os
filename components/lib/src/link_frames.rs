//! Frame-slot bookkeeping for a `LinkDevice` client that lends the driver a
//! fixed set of one-page frame buffers (IO3's eight payload loans).
//!
//! Receive and transmit halves are tracked separately so transmit pressure
//! can never starve receive replenishment: a slot belongs to one direction
//! for the life of the attachment. The tracker owns no memory and makes no
//! syscall; it only says which slot is in which state, so a host test can
//! drive every transition.

/// One receive slot: its page is either lent to the device, holding a frame
/// the device delivered, or free to be lent again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxSlot {
    Free,
    Provisioned {
        request_id: u64,
    },
    /// `sequence` is the delivery order: frames must be read in the order the
    /// device delivered them, not in slot order, or a stream reorders.
    Ready {
        len: usize,
        sequence: u64,
    },
}

/// One transmit slot: its page is either free or retained by the device until
/// the transmit completes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TxSlot {
    Free,
    Retained { request_id: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotError {
    /// No slot carries that request id in the state the transition needs.
    Unknown,
    /// The slot is not in the state the transition needs.
    State,
}

#[derive(Clone, Copy, Debug)]
pub struct RxSlots<const N: usize> {
    slots: [RxSlot; N],
    deliveries: u64,
}

impl<const N: usize> Default for RxSlots<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> RxSlots<N> {
    pub const fn new() -> Self {
        Self {
            slots: [RxSlot::Free; N],
            deliveries: 0,
        }
    }
    /// Lend the first free slot to the device under `request_id`.
    pub fn provide(&mut self, request_id: u64) -> Option<usize> {
        let slot = self.slots.iter().position(|slot| *slot == RxSlot::Free)?;
        self.slots[slot] = RxSlot::Provisioned { request_id };
        Some(slot)
    }
    /// The device delivered `len` bytes into the slot lent under `request_id`.
    pub fn delivered(&mut self, request_id: u64, len: usize) -> Result<usize, SlotError> {
        let slot = self.provisioned(request_id)?;
        self.deliveries += 1;
        self.slots[slot] = RxSlot::Ready {
            len,
            sequence: self.deliveries,
        };
        Ok(slot)
    }
    /// The device gave the slot back without a frame (reset, error).
    pub fn returned(&mut self, request_id: u64) -> Result<usize, SlotError> {
        let slot = self.provisioned(request_id)?;
        self.slots[slot] = RxSlot::Free;
        Ok(slot)
    }
    /// The earliest delivered frame still unread, and the slot goes back to
    /// free: the caller reads the page and lends it again. Delivery order, not
    /// slot order: a slot freed and lent again holds a newer frame than its
    /// higher-numbered neighbours still waiting to be read.
    pub fn take_ready(&mut self) -> Option<(usize, usize)> {
        let (slot, len, _) = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| match *slot {
                RxSlot::Ready { len, sequence } => Some((index, len, sequence)),
                _ => None,
            })
            .min_by_key(|(_, _, sequence)| *sequence)?;
        self.slots[slot] = RxSlot::Free;
        Some((slot, len))
    }
    pub fn has_ready(&self) -> bool {
        self.slots
            .iter()
            .any(|slot| matches!(slot, RxSlot::Ready { .. }))
    }
    pub fn provisioned_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| matches!(slot, RxSlot::Provisioned { .. }))
            .count()
    }
    pub fn free_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| **slot == RxSlot::Free)
            .count()
    }
    fn provisioned(&self, request_id: u64) -> Result<usize, SlotError> {
        self.slots
            .iter()
            .position(|slot| *slot == RxSlot::Provisioned { request_id })
            .ok_or(SlotError::Unknown)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct TxSlots<const N: usize> {
    slots: [TxSlot; N],
}

impl<const N: usize> Default for TxSlots<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> TxSlots<N> {
    pub const fn new() -> Self {
        Self {
            slots: [TxSlot::Free; N],
        }
    }
    /// A free slot, if any. Nothing changes until [`TxSlots::retain`].
    pub fn free_slot(&self) -> Option<usize> {
        self.slots.iter().position(|slot| *slot == TxSlot::Free)
    }
    /// The frame in `slot` was submitted under `request_id`; the device owns
    /// the page until the completion.
    pub fn retain(&mut self, slot: usize, request_id: u64) -> Result<(), SlotError> {
        match self.slots.get(slot) {
            Some(TxSlot::Free) => {
                self.slots[slot] = TxSlot::Retained { request_id };
                Ok(())
            }
            _ => Err(SlotError::State),
        }
    }
    /// The transmit under `request_id` completed, however it ended.
    pub fn release(&mut self, request_id: u64) -> Result<usize, SlotError> {
        let slot = self
            .slots
            .iter()
            .position(|slot| *slot == TxSlot::Retained { request_id })
            .ok_or(SlotError::Unknown)?;
        self.slots[slot] = TxSlot::Free;
        Ok(slot)
    }
    pub fn retained_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| matches!(slot, TxSlot::Retained { .. }))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receive_slots_cycle_through_provision_delivery_and_reuse() {
        let mut rx = RxSlots::<2>::new();
        assert_eq!(rx.provide(10), Some(0));
        assert_eq!(rx.provide(11), Some(1));
        assert_eq!(rx.provide(12), None);
        assert_eq!(rx.provisioned_count(), 2);
        assert_eq!(rx.delivered(11, 60), Ok(1));
        assert_eq!(rx.delivered(11, 60), Err(SlotError::Unknown));
        assert!(rx.has_ready());
        assert_eq!(rx.take_ready(), Some((1, 60)));
        assert_eq!(rx.take_ready(), None);
        assert_eq!(rx.free_count(), 1);
        assert_eq!(rx.provide(13), Some(1));
        assert_eq!(rx.returned(10), Ok(0));
        assert_eq!(rx.returned(99), Err(SlotError::Unknown));
        assert_eq!(rx.free_count(), 1);
    }

    #[test]
    fn receive_frames_are_taken_in_delivery_order_not_slot_order() {
        let mut rx = RxSlots::<3>::new();
        for id in 10..13 {
            rx.provide(id);
        }
        // Three frames land in slot order; the first is read and its slot is
        // lent again, and a fourth frame lands there before slots 1 and 2 are
        // read.
        for id in 10..13 {
            rx.delivered(id, 60 + id as usize).unwrap();
        }
        assert_eq!(rx.take_ready(), Some((0, 70)));
        assert_eq!(rx.provide(13), Some(0));
        assert_eq!(rx.delivered(13, 99), Ok(0));
        assert_eq!(rx.take_ready(), Some((1, 71)));
        assert_eq!(rx.take_ready(), Some((2, 72)));
        assert_eq!(rx.take_ready(), Some((0, 99)));
        assert_eq!(rx.take_ready(), None);
    }

    #[test]
    fn transmit_slots_are_retained_until_their_completion() {
        let mut tx = TxSlots::<2>::new();
        assert_eq!(tx.free_slot(), Some(0));
        assert_eq!(tx.retain(0, 20), Ok(()));
        assert_eq!(tx.retain(0, 21), Err(SlotError::State));
        assert_eq!(tx.retain(5, 21), Err(SlotError::State));
        assert_eq!(tx.free_slot(), Some(1));
        assert_eq!(tx.retain(1, 21), Ok(()));
        assert_eq!(tx.free_slot(), None);
        assert_eq!(tx.retained_count(), 2);
        assert_eq!(tx.release(20), Ok(0));
        assert_eq!(tx.release(20), Err(SlotError::Unknown));
        assert_eq!(tx.free_slot(), Some(0));
    }

    #[test]
    fn halves_never_share_a_slot() {
        let mut rx = RxSlots::<1>::new();
        let mut tx = TxSlots::<1>::new();
        assert_eq!(rx.provide(1), Some(0));
        assert_eq!(tx.retain(0, 2), Ok(()));
        assert_eq!(rx.provide(3), None);
        assert_eq!(tx.free_slot(), None);
    }
}
