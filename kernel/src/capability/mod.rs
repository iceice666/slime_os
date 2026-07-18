use alloc::vec::Vec;

use crate::ipc::Endpoint;

pub const MAX_CAPS: usize = 64;
pub const RIGHT_SEND: u32 = 1;
pub const RIGHT_RECV: u32 = 2;
pub const RIGHT_TRANSFER: u32 = 4;

#[derive(Clone)]
pub struct Capability {
    pub object: KernelObject,
    pub rights: u32,
}

#[derive(Clone)]
pub enum KernelObject {
    Endpoint(Endpoint),
}

pub struct CapabilityTable {
    slots: [Option<Capability>; MAX_CAPS],
}

impl CapabilityTable {
    pub fn new() -> Self {
        Self {
            slots: core::array::from_fn(|_| None),
        }
    }

    pub fn insert(&mut self, cap: Capability) -> Result<u32, CapError> {
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(cap);
                return Ok(i as u32);
            }
        }
        Err(CapError::TableFull)
    }

    pub fn get(&self, slot: u32) -> Option<&Capability> {
        self.slots.get(slot as usize)?.as_ref()
    }

    pub fn take(&mut self, slot: u32) -> Option<Capability> {
        self.slots.get_mut(slot as usize)?.take()
    }
    pub fn put(&mut self, slot: u32, cap: Capability) -> Result<(), CapError> {
        let Some(dst) = self.slots.get_mut(slot as usize) else {
            return Err(CapError::BadSlot);
        };
        if dst.is_some() {
            return Err(CapError::BadSlot);
        }
        *dst = Some(cap);
        Ok(())
    }

    pub fn available_slots(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_none()).count()
    }

    pub fn drain(&mut self) -> Vec<Capability> {
        let mut caps = Vec::new();
        for slot in self.slots.iter_mut() {
            if let Some(cap) = slot.take() {
                caps.push(cap);
            }
        }
        caps
    }
}

impl Default for CapabilityTable {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapError {
    TableFull,
    BadSlot,
}
