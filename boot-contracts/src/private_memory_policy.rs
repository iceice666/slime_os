//! Versioned authorization; accepting its structure does not activate allocation.
//!
//! Fixed v1 budgets remain fully guaranteed. Version 2 uses the same magic so
//! v1 readers reject it rather than boot with silently absent authority.

use crate::sha256::Sha256;

include!("generated/private_memory_policy.rs");

pub mod ledger;

pub const MAX_BYTES: usize =
    HEADER_BYTES + MAX_ENTITLEMENTS * ENTITLEMENT_BYTES + MAX_SUBJECTS * SUBJECT_BYTES;
const _: () = assert!(MAX_PAGES == MAX_VALUE / PAGE_BYTES);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Truncated,
    Version,
    Flags,
    Bounds,
    Order,
    Reference,
    Maximum,
    Overflow,
    Topology,
}

#[derive(Clone, Copy, Debug)]
pub struct Policy<'a> {
    bytes: &'a [u8],
    header: Header,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Instance {
    pub identity: [u8; 32],
    pub owner: Option<[u8; 32]>,
}

fn valid_maximum(mode: u32, pages: u64) -> bool {
    (mode == FIXED && pages > 0 && pages <= MAX_PAGES) || (mode == POOL && pages == 0)
}

impl<'a> Policy<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, Error> {
        let header = Header::decode(bytes).ok_or(Error::Truncated)?;
        if header.magic != MAGIC || header.format_version != FORMAT_VERSION {
            return Err(Error::Version);
        }
        if header.header_size as usize != HEADER_BYTES
            || header.entitlement_count as usize > MAX_ENTITLEMENTS
            || header.subject_count as usize > MAX_SUBJECTS
            || bytes.len()
                != HEADER_BYTES
                    + header.entitlement_count as usize * ENTITLEMENT_BYTES
                    + header.subject_count as usize * SUBJECT_BYTES
            || header.total_len as usize != bytes.len()
        {
            return Err(Error::Bounds);
        }
        if header.required_flags != 0 || header.reserved != 0 {
            return Err(Error::Flags);
        }
        if [
            header.reserve_bytes,
            header.reserve_slots,
            header.reserve_descriptors,
            header.reserve_extents,
            header.reserve_tables,
        ]
        .into_iter()
        .any(|value| value > MAX_VALUE)
        {
            return Err(Error::Overflow);
        }
        let policy = Self { bytes, header };
        let mut previous = [0; 32];
        let mut guarantees = 0u64;
        for index in 0..policy.entitlement_count() {
            let entry = policy.entitlement(index).ok_or(Error::Bounds)?;
            if entry.identity <= previous {
                return Err(Error::Order);
            }
            previous = entry.identity;
            if entry.reserved != 0 {
                return Err(Error::Flags);
            }
            if !valid_maximum(entry.maximum_mode, entry.maximum_pages)
                || entry.guarantee_pages > MAX_PAGES
                || (entry.maximum_mode == FIXED && entry.guarantee_pages > entry.maximum_pages)
            {
                return Err(Error::Maximum);
            }
            guarantees = guarantees
                .checked_add(entry.guarantee_pages)
                .ok_or(Error::Overflow)?;
            if guarantees > MAX_PAGES {
                return Err(Error::Overflow);
            }
        }
        previous = [0; 32];
        for index in 0..policy.subject_count() {
            let entry = policy.subject(index).ok_or(Error::Bounds)?;
            if entry.identity <= previous {
                return Err(Error::Order);
            }
            previous = entry.identity;
            if entry.reserved != 0 {
                return Err(Error::Flags);
            }
            if !valid_maximum(entry.maximum_mode, entry.maximum_pages) {
                return Err(Error::Maximum);
            }
            if policy.entitlement_index(&entry.entitlement).is_none() {
                return Err(Error::Reference);
            }
        }
        for index in 0..policy.entitlement_count() {
            let entry = policy.entitlement(index).unwrap();
            let mut member_maximum = 0u64;
            let mut members = 0;
            for subject_index in 0..policy.subject_count() {
                let member = policy.subject(subject_index).unwrap();
                if member.entitlement == entry.identity {
                    members += 1;
                    member_maximum = member_maximum
                        .saturating_add(if member.maximum_mode == POOL {
                            MAX_PAGES
                        } else {
                            member.maximum_pages
                        })
                        .min(MAX_PAGES);
                }
            }
            if members == 0 || entry.guarantee_pages > member_maximum {
                return Err(Error::Maximum);
            }
        }
        Ok(policy)
    }

    pub fn entitlement_count(&self) -> usize {
        self.header.entitlement_count as usize
    }

    pub fn subject_count(&self) -> usize {
        self.header.subject_count as usize
    }

    pub fn entitlement(&self, index: usize) -> Option<Entitlement> {
        if index >= self.entitlement_count() {
            return None;
        }
        Entitlement::decode(&self.bytes[HEADER_BYTES + index * ENTITLEMENT_BYTES..])
    }

    pub fn subject(&self, index: usize) -> Option<Subject> {
        if index >= self.subject_count() {
            return None;
        }
        Subject::decode(
            &self.bytes[HEADER_BYTES
                + self.entitlement_count() * ENTITLEMENT_BYTES
                + index * SUBJECT_BYTES..],
        )
    }

    pub fn entitlement_index(&self, identity: &[u8; 32]) -> Option<usize> {
        (0..self.entitlement_count())
            .find(|&index| self.entitlement(index).unwrap().identity == *identity)
    }

    pub fn subject_index(&self, identity: &[u8; 32]) -> Option<usize> {
        (0..self.subject_count()).find(|&index| self.subject(index).unwrap().identity == *identity)
    }

    pub fn reserve(&self) -> ledger::Resources {
        ledger::Resources {
            bytes: self.header.reserve_bytes,
            slots: self.header.reserve_slots,
            descriptors: self.header.reserve_descriptors,
            extents: self.header.reserve_extents,
            tables: self.header.reserve_tables,
        }
    }

    /// Membership is explicit even in subtree mode. No descendants are added.
    pub fn validate_instances(&self, instances: &[Instance]) -> Result<(), Error> {
        for (index, instance) in instances.iter().enumerate() {
            if instance.identity == [0; 32]
                || instances[..index]
                    .iter()
                    .any(|prior| prior.identity == instance.identity)
            {
                return Err(Error::Topology);
            }
            let mut current = Some(instance.identity);
            for step in 0..=instances.len() {
                let Some(identity) = current else {
                    break;
                };
                if step == instances.len() {
                    return Err(Error::Topology);
                }
                current = instances
                    .iter()
                    .find(|entry| entry.identity == identity)
                    .ok_or(Error::Reference)?
                    .owner;
            }
        }
        for index in 0..self.subject_count() {
            let subject = self.subject(index).unwrap();
            let entitlement = self
                .entitlement(self.entitlement_index(&subject.entitlement).unwrap())
                .unwrap();
            if !instances
                .iter()
                .any(|entry| entry.identity == subject.identity)
            {
                return Err(Error::Reference);
            }
            if entitlement.subtree_root != [0; 32] {
                let mut current = Some(subject.identity);
                let mut found = false;
                while let Some(identity) = current {
                    if identity == entitlement.subtree_root {
                        found = true;
                        break;
                    }
                    current = instances
                        .iter()
                        .find(|entry| entry.identity == identity)
                        .ok_or(Error::Reference)?
                        .owner;
                }
                if !found {
                    return Err(Error::Topology);
                }
            }
        }
        Ok(())
    }
}

fn identity(domain: &[u8], name: &str) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(domain);
    hash.update(name.as_bytes());
    hash.finalize()
}

pub fn entitlement_identity(name: &str) -> [u8; 32] {
    identity(b"slime-private-memory-entitlement-v2\0", name)
}

pub fn subject_identity(name: &str) -> [u8; 32] {
    identity(b"slime-private-memory-subject-v2\0", name)
}

#[cfg(test)]
mod tests;
