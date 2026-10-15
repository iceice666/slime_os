//! Slot tables over storage the caller supplies: the key aliases and the subscribers a session holds.
//!
//! A table is a run of equal slots in a byte slice, each `SLOT_OVERHEAD_BYTES` of bookkeeping and room for
//! a key of the configured size, which is the arithmetic `limits` states. A slot is found by its id with
//! open addressing from `id % slots`, so the ids a peer numbers in sequence land in their own slots and an
//! operation costs one probe; a table that is full or has been emptied around an id costs at most one pass.
//! The table owns no memory and keeps no count: the caller keeps how many slots are used.

use super::limits::SLOT_OVERHEAD_BYTES;

const EMPTY: u8 = 0;
const USED: u8 = 1;
const REMOVED: u8 = 2;
const STATE_AT: usize = 0;
const ID_AT: usize = 1;
const LEN_AT: usize = 5;
const KEY_AT: usize = SLOT_OVERHEAD_BYTES as usize;

/// Why a slot could not be taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertError {
    /// A slot already holds this id.
    Duplicate,
    /// Every slot is used.
    Full,
    /// The key is longer than a slot holds.
    KeyTooLong,
}

/// The bytes a table of `slots` slots for keys of `key_max` bytes occupies.
pub const fn table_bytes(slots: usize, key_max: usize) -> usize {
    slots * (key_max + KEY_AT)
}

/// A read view of a table.
#[derive(Clone, Copy, Debug)]
pub struct Table<'a> {
    bytes: &'a [u8],
    slots: usize,
    slot: usize,
}

/// A table that can be changed.
pub struct TableMut<'a> {
    bytes: &'a mut [u8],
    slots: usize,
    slot: usize,
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    let mut word = [0u8; 4];
    if let Some(src) = bytes.get(at..at + 4) {
        word.copy_from_slice(src);
    }
    u32::from_le_bytes(word)
}

fn read_u16(bytes: &[u8], at: usize) -> u16 {
    let mut word = [0u8; 2];
    if let Some(src) = bytes.get(at..at + 2) {
        word.copy_from_slice(src);
    }
    u16::from_le_bytes(word)
}

impl<'a> Table<'a> {
    /// A view over the first `table_bytes(slots, key_max)` bytes of `bytes`.
    pub fn new(bytes: &'a [u8], slots: usize, key_max: usize) -> Self {
        Self {
            bytes,
            slots,
            slot: key_max + KEY_AT,
        }
    }

    fn at(&self, index: usize) -> &'a [u8] {
        self.bytes
            .get(index * self.slot..(index + 1) * self.slot)
            .unwrap_or(&[])
    }

    fn state(&self, index: usize) -> u8 {
        self.at(index).get(STATE_AT).copied().unwrap_or(EMPTY)
    }

    /// The slot holding `id`.
    pub fn find(&self, id: u32) -> Option<usize> {
        if self.slots == 0 {
            return None;
        }
        let start = id as usize % self.slots;
        for step in 0..self.slots {
            let index = (start + step) % self.slots;
            match self.state(index) {
                EMPTY => return None,
                USED if read_u32(self.at(index), ID_AT) == id => return Some(index),
                _ => {}
            }
        }
        None
    }

    /// The id and key of a used slot.
    pub fn get(&self, index: usize) -> Option<(u32, &'a [u8])> {
        let slot = self.at(index);
        if slot.get(STATE_AT) != Some(&USED) {
            return None;
        }
        let len = usize::from(read_u16(slot, LEN_AT));
        Some((read_u32(slot, ID_AT), slot.get(KEY_AT..KEY_AT + len)?))
    }

    /// The first used slot, for a table that holds one.
    pub fn first(&self) -> Option<(u32, &'a [u8])> {
        (0..self.slots).find_map(|index| self.get(index))
    }
}

impl<'a> TableMut<'a> {
    /// A table over the first `table_bytes(slots, key_max)` bytes of `bytes`.
    pub fn new(bytes: &'a mut [u8], slots: usize, key_max: usize) -> Self {
        Self {
            bytes,
            slots,
            slot: key_max + KEY_AT,
        }
    }

    pub fn view(&self) -> Table<'_> {
        Table {
            bytes: self.bytes,
            slots: self.slots,
            slot: self.slot,
        }
    }

    /// Takes a slot for `id` and `key`.
    pub fn insert(&mut self, id: u32, key: &[u8]) -> Result<usize, InsertError> {
        if key.len() > self.slot - KEY_AT || self.slots == 0 {
            return Err(if self.slots == 0 {
                InsertError::Full
            } else {
                InsertError::KeyTooLong
            });
        }
        if self.view().find(id).is_some() {
            return Err(InsertError::Duplicate);
        }
        let start = id as usize % self.slots;
        let mut chosen = None;
        for step in 0..self.slots {
            let index = (start + step) % self.slots;
            if self.view().state(index) != USED {
                chosen = Some(index);
                break;
            }
        }
        let index = chosen.ok_or(InsertError::Full)?;
        let slot = self
            .bytes
            .get_mut(index * self.slot..(index + 1) * self.slot)
            .ok_or(InsertError::Full)?;
        slot.fill(0);
        slot[STATE_AT] = USED;
        slot[ID_AT..ID_AT + 4].copy_from_slice(&id.to_le_bytes());
        slot[LEN_AT..LEN_AT + 2].copy_from_slice(&(key.len() as u16).to_le_bytes());
        slot[KEY_AT..KEY_AT + key.len()].copy_from_slice(key);
        Ok(index)
    }

    /// Frees a used slot. Later lookups still pass over it.
    pub fn remove(&mut self, index: usize) {
        if let Some(slot) = self
            .bytes
            .get_mut(index * self.slot..(index + 1) * self.slot)
        {
            slot[STATE_AT] = REMOVED;
        }
    }

    /// Frees every slot.
    pub fn clear(&mut self) {
        let end = self.slots * self.slot;
        if let Some(all) = self.bytes.get_mut(..end) {
            all.fill(EMPTY);
        }
    }
}

#[cfg(test)]
#[path = "tables_tests.rs"]
mod tests;
