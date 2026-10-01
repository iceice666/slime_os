//! Bounded assembly of spawn/v2 arguments before any launch side effect.

use crate::spawn::{self, WireSpawnFrame, WireSpawnRequest};

#[derive(Clone, Copy)]
pub struct SpawnArguments {
    bytes: [u8; spawn::MAX_ARGUMENT_BYTES],
    lengths: [u16; spawn::MAX_ARGUMENTS],
    count: usize,
    total: usize,
    received: usize,
    sequence: u16,
    complete: bool,
}

impl SpawnArguments {
    pub fn new(request: &WireSpawnRequest) -> Option<Self> {
        if !crate::valid_spawn_request(request) {
            return None;
        }
        Some(Self {
            bytes: [0; spawn::MAX_ARGUMENT_BYTES],
            lengths: argument_lengths(request),
            count: usize::from(request.argument_count),
            total: usize::from(request.argument_bytes),
            received: 0,
            sequence: 0,
            complete: request.argument_count == 0,
        })
    }

    /// Refuse replay, gaps, altered totals and completion not at the declared
    /// end. Even zero-byte arguments require an explicit final frame.
    pub fn push(&mut self, frame: &WireSpawnFrame) -> Result<bool, ArgumentError> {
        let len = usize::from(frame.length);
        let end = self.received.checked_add(len).ok_or(ArgumentError)?;
        let final_frame = frame.flags == spawn::FRAME_FLAG_FINAL;
        if self.complete
            || frame.magic != spawn::FRAME_MAGIC
            || frame.version != spawn::FORMAT_VERSION
            || frame.sequence != self.sequence
            || usize::from(frame.offset) != self.received
            || len > frame.payload.len()
            || end > self.total
            || frame.flags & !spawn::FRAME_FLAG_FINAL != 0
            || frame.payload[len..].iter().any(|byte| *byte != 0)
            || (!final_frame && (len == 0 || end == self.total))
            || (final_frame && end != self.total)
        {
            return Err(ArgumentError);
        }
        self.bytes[self.received..end].copy_from_slice(&frame.payload[..len]);
        self.received = end;
        self.sequence = self.sequence.checked_add(1).ok_or(ArgumentError)?;
        self.complete = final_frame;
        Ok(self.complete)
    }

    pub const fn is_complete(&self) -> bool {
        self.complete
    }

    pub fn argument(&self, index: usize) -> Option<&[u8]> {
        if !self.complete || index >= self.count {
            return None;
        }
        let start: usize = self.lengths[..index]
            .iter()
            .map(|len| usize::from(*len))
            .sum();
        self.bytes
            .get(start..start + usize::from(self.lengths[index]))
    }

    /// Canonical frames for forwarding a validated launch context.
    pub fn frame(&self, sequence: usize) -> Option<WireSpawnFrame> {
        if !self.complete || self.count == 0 {
            return None;
        }
        let offset = sequence.checked_mul(spawn::FRAME_PAYLOAD_BYTES)?;
        if (self.total == 0 && sequence != 0) || (self.total != 0 && offset >= self.total) {
            return None;
        }
        let end = (offset + spawn::FRAME_PAYLOAD_BYTES).min(self.total);
        let mut payload = [0; spawn::FRAME_PAYLOAD_BYTES];
        payload[..end - offset].copy_from_slice(&self.bytes[offset..end]);
        Some(WireSpawnFrame {
            magic: spawn::FRAME_MAGIC,
            version: spawn::FORMAT_VERSION,
            sequence: sequence as u16,
            offset: offset as u16,
            length: (end - offset) as u16,
            flags: if end == self.total {
                spawn::FRAME_FLAG_FINAL
            } else {
                0
            },
            payload,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArgumentError;

pub const fn argument_lengths(request: &WireSpawnRequest) -> [u16; spawn::MAX_ARGUMENTS] {
    [
        request.argument_len0,
        request.argument_len1,
        request.argument_len2,
        request.argument_len3,
    ]
}
