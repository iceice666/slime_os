//! Zenoh Profile 0 FRAGMENT: the codec for a message larger than the batch the peer announced.
//!
//! A message that does not fit one batch is sent as reliable FRAGMENTs: a header, a sequence number and
//! a slice of the serialized network message of at most the batch less the two-byte stream length. Every
//! fragment but the last carries the "more" flag, and the sequence numbers are consecutive and wrap at 32
//! bits. This module is pure: [`Splitter`] cuts a message into bodies, [`decode_fragment`] reads one body,
//! and [`decode_put`] reads the message a run of fragments joins into. Joining the slices into storage,
//! the timeout, and the session's sequence number are [`super::reassembly`]'s and the session's.
//!
//! FRAGMENT is not in `contracts/zenoh-profile/v1`, which is the vocabulary of the first release; its
//! identifier and flags are those of the Zenoh transport specification, which this crate does not own.

use super::limits::leb_bytes;
use super::{
    ATTACHMENT_GID_LEN_AT, Attachment, EXT_ATTACHMENT, FLAG_BIT5, FLAG_BIT6, FLAG_Z, GID_BYTES,
    ID_MASK, ID_PUSH, ID_PUT, MAX_ATTACHMENT_BYTES, Reader, Refusal, STREAM_LENGTH_BYTES,
    key_expression, word,
};
use slime_proto::zenoh_profile as p;

#[cfg(test)]
#[path = "fragment_tests.rs"]
mod tests;

/// The transport message identifier of a FRAGMENT.
pub const ID_FRAGMENT: u8 = 0x06;
/// Reliable channel.
const FLAG_RELIABLE: u8 = FLAG_BIT5;
/// More fragments of this message follow.
const FLAG_MORE: u8 = FLAG_BIT6;
/// A batch must carry more than a fragment's own framing, or no message could make progress.
const MIN_BATCH: usize = STREAM_LENGTH_BYTES + 6;

/// Why a fragment, a run of them or the message they join is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FragmentError {
    /// The decoder's own classes.
    Refused(Refusal),
    /// A continuation whose sequence number is not the one after the fragment before it.
    FragmentSequence,
    /// A run whose first fragment is not the session's next expected sequence number.
    SequenceMismatch,
    /// A reassembly that outlived its declared timeout.
    ReassemblyTimeout,
    /// The storage the component supplied cannot hold the message.
    OutOfMemory,
    /// A Put for a key the subscriber did not declare.
    UndeclaredKey,
    /// The batch cannot carry a fragment's header and a slice.
    BatchTooSmall,
    /// The storage lent for a body is shorter than the body.
    OutputTooSmall,
}

impl From<Refusal> for FragmentError {
    fn from(refusal: Refusal) -> Self {
        Self::Refused(refusal)
    }
}

impl FragmentError {
    /// The class name as the exam spells it.
    pub const fn class(self) -> &'static str {
        match self {
            Self::Refused(refusal) => refusal.name(),
            Self::FragmentSequence => "fragment-sequence",
            Self::SequenceMismatch => "sequence-mismatch",
            Self::ReassemblyTimeout => "reassembly-timeout",
            Self::OutOfMemory => "out-of-memory",
            Self::UndeclaredKey => "undeclared-key",
            Self::BatchTooSmall => "batch-too-small",
            Self::OutputTooSmall => "output-too-small",
        }
    }
}

/// One decoded FRAGMENT body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fragment<'a> {
    /// More fragments of the message follow.
    pub more: bool,
    pub sn: u32,
    pub data: &'a [u8],
}

/// Whether a transport message header is a FRAGMENT's, whatever else it says. A link that cannot
/// reassemble refuses it on this alone.
pub const fn is_fragment(header: u8) -> bool {
    header & ID_MASK == ID_FRAGMENT
}

/// Decodes one FRAGMENT body, refusing anything but the reliable form. With `batch`, the batch this
/// side announced, a body longer than the batch less the stream length is refused: the peer was told
/// that size and must not send more.
pub fn decode_fragment(body: &[u8], batch: Option<usize>) -> Result<Fragment<'_>, FragmentError> {
    if let Some(batch) = batch
        && body.len() > batch.saturating_sub(STREAM_LENGTH_BYTES)
    {
        return Err(Refusal::LengthExceedsBatch.into());
    }
    let mut reader = Reader::new(body);
    let header = reader.u8().map_err(|_| Refusal::MalformedMessage)?;
    if !is_fragment(header) {
        return Err(Refusal::UnsupportedMessage.into());
    }
    if header & FLAG_RELIABLE == 0 {
        return Err(Refusal::UnsupportedMessage.into());
    }
    if header & FLAG_Z != 0 {
        return Err(Refusal::UnsupportedExtension.into());
    }
    let sn = reader.leb(p::LEB128_MAX_BITS)?;
    let data = reader.take(reader.left())?;
    if data.is_empty() {
        return Err(Refusal::MalformedMessage.into());
    }
    Ok(Fragment {
        more: header & FLAG_MORE != 0,
        sn,
        data,
    })
}

/// Writes `value` as a canonical LEB128 into `out` and returns its length.
fn write_leb(out: &mut [u8], value: u32) -> Result<usize, FragmentError> {
    let width = leb_bytes(u64::from(value));
    let mut rest = value;
    for (index, slot) in out
        .get_mut(..width)
        .ok_or(FragmentError::OutputTooSmall)?
        .iter_mut()
        .enumerate()
    {
        let low = (rest & 0x7F) as u8;
        rest >>= 7;
        *slot = if index + 1 == width { low } else { low | 0x80 };
    }
    Ok(width)
}

/// Cuts one serialized network message into FRAGMENT bodies for a peer whose batch is `batch`.
#[derive(Clone, Copy, Debug)]
pub struct Splitter<'a> {
    message: &'a [u8],
    at: usize,
    sn: u32,
    batch: usize,
}

impl<'a> Splitter<'a> {
    /// A splitter over `message`, numbering its fragments from `first_sn`.
    pub fn new(message: &'a [u8], batch: usize, first_sn: u32) -> Result<Self, FragmentError> {
        if batch <= MIN_BATCH {
            return Err(FragmentError::BatchTooSmall);
        }
        Ok(Self {
            message,
            at: 0,
            sn: first_sn,
            batch,
        })
    }

    /// The sequence number the next fragment carries; after the last it is the session's next.
    pub const fn next_sn(&self) -> u32 {
        self.sn
    }

    pub const fn done(&self) -> bool {
        self.at >= self.message.len()
    }

    /// Writes the next body into `out` and returns its length, or `None` when the message is done.
    pub fn next_body(&mut self, out: &mut [u8]) -> Result<Option<usize>, FragmentError> {
        if self.done() {
            return Ok(None);
        }
        let header_len = 1 + leb_bytes(u64::from(self.sn));
        let room = self.batch - STREAM_LENGTH_BYTES - header_len;
        let remaining = self.message.len() - self.at;
        let take = room.min(remaining);
        let more = take < remaining;
        let total = header_len + take;
        let slot = out.get_mut(..total).ok_or(FragmentError::OutputTooSmall)?;
        slot[0] = ID_FRAGMENT | FLAG_RELIABLE | if more { FLAG_MORE } else { 0 };
        write_leb(&mut slot[1..], self.sn)?;
        slot[header_len..].copy_from_slice(&self.message[self.at..self.at + take]);
        self.at += take;
        self.sn = self.sn.wrapping_add(1);
        Ok(Some(total))
    }
}

/// Whether a message of `message_len` bytes travels whole in one FRAME numbered `frame_sn`: the FRAME's
/// header byte and sequence number and the message must fit the batch less the stream length.
pub const fn fits_one_frame(message_len: usize, batch: usize, frame_sn: u32) -> bool {
    1 + leb_bytes(frame_sn as u64) + message_len <= batch.saturating_sub(STREAM_LENGTH_BYTES)
}

/// What the session knows when a reassembled message is read.
#[derive(Clone, Copy, Debug)]
pub struct PutRules<'a> {
    /// The longest key the configuration declares, in encoded bytes.
    pub max_key: usize,
    /// The largest payload the configuration declares.
    pub max_payload: u64,
    /// The alias the peer declared, if any. A peer-declared alias is in the sender's mapping.
    pub alias: Option<u32>,
    /// The key that alias stands for.
    pub alias_key: Option<&'a [u8]>,
    /// The key this side's subscriber declared.
    pub subscribed: Option<&'a [u8]>,
}

/// The Put a run of fragments joined into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PutView<'a> {
    /// The scope of the key reference: 0 for a key in full, the alias for an aliased Put.
    pub scope: u32,
    /// The key in full, absent when the Put names an alias.
    pub key: Option<&'a [u8]>,
    pub attachment: Attachment<'a>,
    pub payload: &'a [u8],
}

/// Reads the PUSH with a Put that a run of fragments joined into. It is held to what a whole-frame
/// Put is held to, and to the configuration's key and payload sizes and the session's aliases.
pub fn decode_put<'a>(
    message: &'a [u8],
    rules: &PutRules<'_>,
) -> Result<PutView<'a>, FragmentError> {
    if message.is_empty() {
        return Err(Refusal::MalformedMessage.into());
    }
    let mut reader = Reader::new(message);
    let header = reader.u8()?;
    if header & ID_MASK != ID_PUSH {
        return Err(Refusal::UnsupportedMessage.into());
    }
    if header & FLAG_Z != 0 {
        return Err(Refusal::UnsupportedExtension.into());
    }
    let named = header & FLAG_BIT5 != 0;
    let sender = header & FLAG_BIT6 != 0;
    let scope = reader.leb(p::LEB128_SHORT_BITS)?;
    let key = if named {
        // The suffix is judged before the scope, as the whole-frame decoder judges it, so a wildcard
        // or an oversized key keeps its own class.
        let length = reader.leb(p::LEB128_SHORT_BITS)? as usize;
        if length > rules.max_key {
            return Err(Refusal::OverBound.into());
        }
        if length == 0 {
            return Err(Refusal::InvalidKeyexpr.into());
        }
        let raw = reader.take(length)?;
        let text = key_expression(raw)?;
        if scope != 0 {
            return Err(Refusal::InvalidKeyexpr.into());
        }
        if rules.subscribed.is_some_and(|held| held != text.as_bytes()) {
            return Err(FragmentError::UndeclaredKey);
        }
        Some(raw)
    } else {
        if scope == 0 {
            return Err(Refusal::InvalidKeyexpr.into());
        }
        if let Some(declared) = rules.alias
            && (!sender || scope != declared)
        {
            return Err(Refusal::InvalidKeyexpr.into());
        }
        if let (Some(stands_for), Some(held)) = (rules.alias_key, rules.subscribed)
            && stands_for != held
        {
            return Err(FragmentError::UndeclaredKey);
        }
        None
    };
    let body = reader.u8()?;
    if body & ID_MASK != ID_PUT || body & (FLAG_BIT5 | FLAG_BIT6) != 0 {
        return Err(Refusal::UnsupportedMessage.into());
    }
    if body & FLAG_Z == 0 {
        return Err(Refusal::MalformedMessage.into());
    }
    if reader.u8()? != EXT_ATTACHMENT {
        return Err(Refusal::UnsupportedExtension.into());
    }
    let length = reader.leb(p::LEB128_MAX_BITS)? as usize;
    if length > MAX_ATTACHMENT_BYTES {
        return Err(Refusal::OverBound.into());
    }
    if length < MAX_ATTACHMENT_BYTES {
        return Err(Refusal::MalformedMessage.into());
    }
    let raw = reader.take(length)?;
    if usize::from(raw[ATTACHMENT_GID_LEN_AT]) != GID_BYTES {
        return Err(Refusal::MalformedMessage.into());
    }
    let size = reader.leb(p::LEB128_MAX_BITS)?;
    if u64::from(size) > rules.max_payload {
        return Err(Refusal::OverBound.into());
    }
    let payload = reader.take(size as usize)?;
    reader.finish()?;
    Ok(PutView {
        scope,
        key,
        attachment: Attachment {
            raw,
            sequence: i64::from_le_bytes(word(raw, 0)),
            timestamp: i64::from_le_bytes(word(raw, 8)),
            gid: &raw[ATTACHMENT_GID_LEN_AT + 1..],
        },
        payload,
    })
}
