//! Reassembly of one fragmented message into storage the component supplies.
//!
//! A heap link is built over a slice of memory the owning component provides from its own allocator;
//! this library takes no allocator of its own. [`Reassembly`] joins the slices of a run of FRAGMENTs
//! into the front of that slice, one message at a time. It owns no memory and keeps no clock: the
//! caller passes the time of each fragment, so the deadline is measured from the first fragment's
//! arrival and a continuation never refreshes it.
//!
//! Every way a run can end releases the storage, so the next message starts from empty: it completes,
//! it is refused, or it outlives its timeout. A refusal leaves the run as it was only in the sense that
//! the run is gone; the session's own sequence number is committed by the caller after a whole message
//! is accepted, never by a refused one.

use super::fragment::{Fragment, FragmentError};

/// Where a reassembly stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    /// More fragments are expected.
    Open,
    /// The final fragment arrived: the message is `message()` long.
    Complete(usize),
}

/// One message being joined from a run of fragments.
#[derive(Clone, Copy, Debug)]
pub struct Reassembly {
    timeout_ms: u64,
    /// The declared limit on one message, so a message over it is refused though storage would hold it.
    limit: usize,
    /// The sequence number the next fragment must carry; `None` while no run is open.
    next_sn: Option<u32>,
    /// When the first fragment of the open run arrived.
    started_ms: u64,
    len: usize,
    /// Whether the last fragment seen said more follow.
    more: bool,
}

impl Reassembly {
    /// A reassembly whose runs may take `timeout_ms` from their first fragment, and whose messages may
    /// not exceed `limit` bytes.
    pub const fn new(timeout_ms: u64, limit: usize) -> Self {
        Self {
            timeout_ms,
            limit,
            next_sn: None,
            started_ms: 0,
            len: 0,
            more: false,
        }
    }

    /// Whether a run is open.
    pub const fn is_open(&self) -> bool {
        self.next_sn.is_some()
    }

    /// The bytes joined so far.
    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The sequence number the next fragment must carry, while a run is open.
    pub const fn next_sn(&self) -> Option<u32> {
        self.next_sn
    }

    /// The time after which an open run is refused, `None` while none is open. The last fragment may
    /// arrive exactly at it; one millisecond later is refused.
    pub const fn deadline(&self) -> Option<u64> {
        if self.next_sn.is_some() {
            Some(self.started_ms.saturating_add(self.timeout_ms))
        } else {
            None
        }
    }

    /// Drops the open run and frees the storage for the next message.
    pub fn release(&mut self) {
        self.next_sn = None;
        self.len = 0;
        self.more = false;
    }

    /// Whether an open run has outlived its timeout at `now_ms`. The caller releases it.
    pub fn expired(&self, now_ms: u64) -> bool {
        self.deadline().is_some_and(|deadline| now_ms > deadline)
    }

    /// Takes one fragment into `storage`. `expected_sn` is the session's next expected sequence number,
    /// which the first fragment of a run must carry exactly; later fragments must follow their
    /// predecessor. Any refusal ends the run and frees the storage.
    pub fn accept(
        &mut self,
        fragment: &Fragment<'_>,
        storage: &mut [u8],
        expected_sn: u32,
        now_ms: u64,
    ) -> Result<Progress, FragmentError> {
        let result = self.take(fragment, storage, expected_sn, now_ms);
        if result.is_err() {
            self.release();
        }
        result
    }

    fn take(
        &mut self,
        fragment: &Fragment<'_>,
        storage: &mut [u8],
        expected_sn: u32,
        now_ms: u64,
    ) -> Result<Progress, FragmentError> {
        match self.next_sn {
            None => {
                if fragment.sn != expected_sn {
                    return Err(FragmentError::SequenceMismatch);
                }
                self.started_ms = now_ms;
            }
            Some(next) => {
                if self.expired(now_ms) {
                    return Err(FragmentError::ReassemblyTimeout);
                }
                if !self.more {
                    return Err(FragmentError::FragmentSequence);
                }
                if fragment.sn != next {
                    return Err(FragmentError::FragmentSequence);
                }
            }
        }
        let end = self
            .len
            .checked_add(fragment.data.len())
            .ok_or(FragmentError::OutOfMemory)?;
        if end > self.limit {
            return Err(crate::zenoh_profile0::Refusal::OverBound.into());
        }
        let slot = storage
            .get_mut(self.len..end)
            .ok_or(FragmentError::OutOfMemory)?;
        slot.copy_from_slice(fragment.data);
        self.len = end;
        self.next_sn = Some(fragment.sn.wrapping_add(1));
        self.more = fragment.more;
        if fragment.more {
            Ok(Progress::Open)
        } else {
            Ok(Progress::Complete(self.len))
        }
    }

    /// The message of a completed run, borrowed from `storage`, and the sequence number the session
    /// expects after it. The run is released, so the storage is free for the next message once the
    /// caller is done reading.
    pub fn finish<'s>(&mut self, storage: &'s [u8]) -> Result<(&'s [u8], u32), FragmentError> {
        let next = self.next_sn.ok_or(FragmentError::FragmentSequence)?;
        if self.more {
            return Err(FragmentError::FragmentSequence);
        }
        let message = storage.get(..self.len).ok_or(FragmentError::OutOfMemory)?;
        self.release();
        Ok((message, next))
    }
}

#[cfg(test)]
#[path = "reassembly_tests.rs"]
mod tests;
