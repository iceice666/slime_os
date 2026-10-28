//! The core over a link. A component owns a link, and the link owns the session, one send slot, retries
//! and keep-alives, so the core queues each declaration and each publication through the link.
//!
//! The link has one send slot. The core holds at most one waiting declaration or publication, and a call
//! that finds either taken, or finds the link's slot taken by a keep-alive, returns [`ServiceError::Busy`]
//! and changes nothing. A declaration counts as declared, and a publication's local copies reach the rings,
//! only after [`Core::settle`] sees the slot empty. That is the whole guarantee: the link handed the frame
//! to the byte stream. It is not that the peer received the frame. When the stream takes a frame and then
//! resets, `settle` drops the waiting copies and reports a closed link, and the peer may still have
//! received the frame. Profile 0 has no acknowledgement, so the core cannot know more.

use super::{Core, Peer, Pending, Published, ServiceError};
use crate::zenoh_link::{ByteLink, Error as LinkError, LinkHandle, LinkOf, Notice};
use crate::zenoh_profile0::encode;

/// What `settle` found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settled {
    /// Nothing was waiting.
    Idle,
    /// The frame is still in the link's send slot.
    Waiting,
    /// A declaration left. The subscription is now declared.
    Declared { id: u16 },
    /// A publication left, and `reached` holders took a copy.
    Published { reached: usize },
}

impl<'k, const H: usize, const N: usize, const P: usize, const R: usize> Core<'k, H, N, P, R> {
    /// Queues the next subscription through the link. The link's send slot must be free and nothing may
    /// be waiting. The subscription counts as declared only once [`settle`](Self::settle) sees it flushed.
    pub fn declare_next_over<
        L: ByteLink,
        const F: usize,
        const K: usize,
        const S: usize,
        const B: usize,
        const PP: usize,
        const QQ: usize,
        const E: usize,
    >(
        &mut self,
        link: &mut LinkOf<L, F, K, S, B, PP, QQ, E>,
        handle: LinkHandle,
    ) -> Result<bool, ServiceError> {
        self.check(link.session())?;
        if !matches!(self.pending, Pending::None) || link.send_pending() {
            return Err(ServiceError::Busy);
        }
        let queued = self.declared + 1;
        if self.declared == self.subscriptions {
            return Ok(false);
        }
        let row = self
            .rows
            .iter()
            .find(|row| row.subscription == queued)
            .ok_or(ServiceError::InvalidGrants)?;
        link.declare_subscriber(handle, u32::from(queued), row.key)
            .map_err(link_error)?;
        self.pending = Pending::Declare { id: queued };
        Ok(true)
    }

    /// Queues a publication through the link. The holder needs a row that grants the right to publish the
    /// key. Local copies wait for the flush. A key the peer did not declare is reported at once, because
    /// nothing is queued for it and the local copies are delivered immediately.
    #[allow(clippy::too_many_arguments)]
    pub fn publish_over<
        L: ByteLink,
        const F: usize,
        const K: usize,
        const S: usize,
        const B: usize,
        const PP: usize,
        const QQ: usize,
        const E: usize,
    >(
        &mut self,
        link: &mut LinkOf<L, F, K, S, B, PP, QQ, E>,
        handle: LinkHandle,
        holder: usize,
        key: &'k str,
        sequence: i64,
        timestamp: i64,
        payload: &[u8],
    ) -> Result<Option<Published>, ServiceError> {
        self.check(link.session())?;
        if !matches!(self.pending, Pending::None) || link.send_pending() {
            return Err(ServiceError::Busy);
        }
        if holder >= H {
            return Err(ServiceError::UnknownHolder);
        }
        let granted = self
            .rows
            .iter()
            .any(|row| usize::from(row.holder) == holder && row.publish && row.key == key);
        if !granted {
            return Err(ServiceError::NotGranted);
        }
        if payload.len() > P {
            return Err(ServiceError::TooLarge);
        }
        if !link.is_open() {
            return Err(ServiceError::Link(LinkError::SendAfterClose));
        }
        let attachment = encode::attachment(sequence, timestamp, &self.gid)
            .map_err(crate::zenoh_profile0::session::SessionError::from)?;
        match link.publish(handle, key, &attachment, payload) {
            Ok(()) => {
                let mut copy = [0u8; P];
                if let Some(dst) = copy.get_mut(..payload.len()) {
                    dst.copy_from_slice(payload);
                }
                self.pending = Pending::Publish {
                    key,
                    sequence,
                    timestamp,
                    len: payload.len() as u32,
                    payload: copy,
                };
                Ok(None)
            }
            Err(LinkError::Session(
                crate::zenoh_profile0::session::SessionError::UndeclaredKey,
            )) => {
                let reached = self.deliver_key(key, sequence, timestamp, payload);
                Ok(Some(Published {
                    reached,
                    peer: Peer::NotSubscribed,
                }))
            }
            Err(other) => Err(link_error(other)),
        }
    }

    /// Looks at the link's send slot. A frame that left completes what was waiting for it. A link that
    /// closed with a frame still waiting drops the waiting copies and says so.
    pub fn settle<
        L: ByteLink,
        const F: usize,
        const K: usize,
        const S: usize,
        const B: usize,
        const PP: usize,
        const QQ: usize,
        const E: usize,
    >(
        &mut self,
        link: &LinkOf<L, F, K, S, B, PP, QQ, E>,
    ) -> Result<Settled, ServiceError> {
        self.check(link.session())?;
        if matches!(self.pending, Pending::None) {
            return Ok(Settled::Idle);
        }
        // An empty slot on a closed link does not say the frame left: a later pump flushes the CLOSE the
        // link queued and empties the slot. A closed link therefore drops what was waiting.
        if link.is_closed() {
            self.pending = Pending::None;
            return Err(ServiceError::Link(LinkError::SendAfterClose));
        }
        if link.send_pending() {
            return Ok(Settled::Waiting);
        }
        match core::mem::replace(&mut self.pending, Pending::None) {
            Pending::None => Ok(Settled::Idle),
            Pending::Declare { id } => {
                self.declared = id;
                Ok(Settled::Declared { id })
            }
            Pending::Publish {
                key,
                sequence,
                timestamp,
                len,
                payload,
            } => {
                let body = payload.get(..len as usize).unwrap_or(&[]);
                let reached = self.deliver_key(key, sequence, timestamp, body);
                Ok(Settled::Published { reached })
            }
        }
    }

    /// Takes the link's notices and routes each sample to the holders subscribed to the subscription that
    /// matched. Returns how many samples it routed. A link whose session has other parameters than the core's
    /// is refused before any notice is taken.
    pub fn drain_notices<
        L: ByteLink,
        const F: usize,
        const K: usize,
        const S: usize,
        const B: usize,
        const QQ: usize,
        const E: usize,
    >(
        &mut self,
        link: &mut LinkOf<L, F, K, S, B, P, QQ, E>,
    ) -> Result<usize, ServiceError> {
        self.check(link.session())?;
        let mut routed = 0;
        while let Some(notice) = link.next_notice() {
            if let Notice::SampleDelivered(sample) = notice {
                self.deliver_subscription(
                    sample.subscription,
                    sample.sequence,
                    sample.timestamp,
                    sample.payload(),
                );
                routed += 1;
            }
        }
        Ok(routed)
    }
}

fn link_error(error: LinkError) -> ServiceError {
    match error {
        LinkError::SendBusy => ServiceError::Busy,
        other => ServiceError::Link(other),
    }
}
