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
use super::tables::Keys;
use super::{
    ATTACHMENT_GID_LEN_AT, Attachment, EXT_ATTACHMENT, FLAG_BIT5, FLAG_BIT6, FLAG_Z, GID_BYTES,
    ID_MASK, ID_PUSH, ID_PUT, MAX_ATTACHMENT_BYTES, Reader, Refusal, STREAM_LENGTH_BYTES,
    key_expression, word,
};
use slime_proto::zenoh_profile as p;

#[cfg(test)]
#[path = "fragment_tests.rs"]
pub(crate) mod tests;

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

/// The notices one received batch can raise, judged from its first byte before the batch is decoded:
/// how many at most, and whether one may be a sample. A FRAME can carry several events; the last
/// fragment of a run, one sample; any other fragment, none.
pub const fn notice_need(header: u8, frame_events: usize) -> (usize, bool) {
    if header & ID_MASK == super::ID_FRAME {
        (frame_events, true)
    } else if is_fragment(header) {
        if header & FLAG_MORE == 0 {
            (1, true)
        } else {
            (0, false)
        }
    } else {
        (1, false)
    }
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

/// A serialized network message read in ranges, so one far larger than any buffer is never held whole.
pub trait Source {
    fn len(&self) -> usize;
    /// Fills `out` with the bytes from `at`; the range lies inside the message.
    fn read(&self, at: usize, out: &mut [u8]);

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Source for &[u8] {
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }

    fn read(&self, at: usize, out: &mut [u8]) {
        if let Some(src) = self.get(at..at + out.len()) {
            out.copy_from_slice(src);
        }
    }
}

/// The widest variable-length integer is five bytes.
const LEB_MAX: usize = 5;

/// Our own PUSH with a Put, in the sender mapping: its parts are borrowed and its bytes are produced on
/// demand, so a payload of any size is carried without a copy.
#[derive(Clone, Copy, Debug)]
pub struct PutMessage<'a> {
    head: [u8; 2 + LEB_MAX],
    head_len: usize,
    key: &'a [u8],
    mid: [u8; 2 + LEB_MAX],
    mid_len: usize,
    attachment: &'a [u8],
    size: [u8; LEB_MAX],
    size_len: usize,
    payload: &'a [u8],
}

fn leb_into(out: &mut [u8], value: u64) -> usize {
    let width = leb_bytes(value);
    let mut rest = value;
    for (index, slot) in out.iter_mut().take(width).enumerate() {
        let low = (rest & 0x7F) as u8;
        rest >>= 7;
        *slot = if index + 1 == width { low } else { low | 0x80 };
    }
    width
}

impl<'a> PutMessage<'a> {
    pub fn new(key: &'a [u8], attachment: &'a [u8], payload: &'a [u8]) -> Self {
        let mut head = [0u8; 2 + LEB_MAX];
        head[0] = ID_PUSH | FLAG_BIT5 | FLAG_BIT6;
        head[1] = 0;
        let head_len = 2 + leb_into(&mut head[2..], key.len() as u64);
        let mut mid = [0u8; 2 + LEB_MAX];
        mid[0] = ID_PUT | FLAG_Z;
        mid[1] = EXT_ATTACHMENT;
        let mid_len = 2 + leb_into(&mut mid[2..], attachment.len() as u64);
        let mut size = [0u8; LEB_MAX];
        let size_len = leb_into(&mut size, payload.len() as u64);
        Self {
            head,
            head_len,
            key,
            mid,
            mid_len,
            attachment,
            size,
            size_len,
            payload,
        }
    }

    fn parts(&self) -> [&[u8]; 6] {
        [
            &self.head[..self.head_len],
            self.key,
            &self.mid[..self.mid_len],
            self.attachment,
            &self.size[..self.size_len],
            self.payload,
        ]
    }
}

impl Source for PutMessage<'_> {
    fn len(&self) -> usize {
        self.parts().iter().map(|part| part.len()).sum()
    }

    fn read(&self, at: usize, out: &mut [u8]) {
        let mut start = 0;
        let mut filled = 0;
        for part in self.parts() {
            let end = start + part.len();
            let want = at + filled;
            if want < end && filled < out.len() {
                let from = want - start;
                let take = (part.len() - from).min(out.len() - filled);
                out[filled..filled + take].copy_from_slice(&part[from..from + take]);
                filled += take;
            }
            start = end;
        }
    }
}

/// Cuts one serialized network message into FRAGMENT bodies for a peer whose batch is `batch`.
#[derive(Clone, Copy, Debug)]
pub struct Splitter<S: Source> {
    message: S,
    at: usize,
    sn: u32,
    batch: usize,
}

impl<S: Source> Splitter<S> {
    /// A splitter over `message`, numbering its fragments from `first_sn`.
    pub fn new(message: S, batch: usize, first_sn: u32) -> Result<Self, FragmentError> {
        Self::resume(message, batch, 0, first_sn)
    }

    /// A splitter that carries on from byte `at` of `message`, `sn` the number of the next fragment.
    pub fn resume(message: S, batch: usize, at: usize, sn: u32) -> Result<Self, FragmentError> {
        if batch <= MIN_BATCH {
            return Err(FragmentError::BatchTooSmall);
        }
        Ok(Self {
            message,
            at,
            sn,
            batch,
        })
    }

    /// The byte of the message the next fragment starts at.
    pub const fn position(&self) -> usize {
        self.at
    }

    /// The sequence number the next fragment carries; after the last it is the session's next.
    pub const fn next_sn(&self) -> u32 {
        self.sn
    }

    pub fn done(&self) -> bool {
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
        self.message.read(self.at, &mut slot[header_len..]);
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
#[derive(Clone, Copy)]
pub struct PutRules<'a> {
    /// The longest key the configuration declares, in encoded bytes.
    pub max_key: usize,
    /// The largest payload the configuration declares.
    pub max_payload: u64,
    /// The aliases the peer declared. A peer-declared alias is in the sender's mapping.
    pub aliases: Option<&'a dyn Keys>,
    /// Whether a key is one this side subscribed to; a Put for any other key is refused.
    pub subscribed: &'a dyn Fn(&[u8]) -> bool,
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
        if !(rules.subscribed)(text.as_bytes()) {
            return Err(FragmentError::UndeclaredKey);
        }
        Some(raw)
    } else {
        if scope == 0 {
            return Err(Refusal::InvalidKeyexpr.into());
        }
        // A session passes the aliases its peer declared: a reference must then be sender-mapped, name a
        // declared alias, and stand for a key this side subscribed to. With no table the codec reads the
        // Put as it is, which is how a captured stock Put is read.
        if let Some(table) = rules.aliases {
            if !sender {
                return Err(Refusal::InvalidKeyexpr.into());
            }
            let stands_for = table
                .key_of(u64::from(scope))
                .ok_or(Refusal::InvalidKeyexpr)?;
            if !(rules.subscribed)(stands_for) {
                return Err(FragmentError::UndeclaredKey);
            }
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
