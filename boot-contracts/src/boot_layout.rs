//! Boot capability layout resource object (B10).
//!
//! A generation resource object declaring which capability slot of the
//! bootstrap component holds which role, under which name, with which rights.
//! It is embedded as a `KIND_RESOURCE` object in a generation and authenticated
//! by the generation's existing per-object digest table, so decoding here
//! assumes integrity already verified and enforces only structural validity.
//!
//! An entry declares a *role*, not a concrete kernel object. The storage slot
//! resolves to a block device when the platform enumerates one and to an object
//! store when it does not, which is decided by PCI enumeration at boot and is
//! not knowable to the host builder. Declaring the role keeps hardware probing
//! in the kernel while still fixing which slot holds it.
//!
//! A name grants nothing on its own: the entry names *which* executable or
//! channel half occupies a slot, the rights field is the authority, and the
//! generation's capability grants remain the gate on delegation.

use crate::sha256::Sha256;

pub const MAGIC: [u8; 8] = *b"SLIMEBL\0";
include!("generated/boot_layout.rs");
pub const MAX_BYTES: usize = HEADER_BYTES + MAX_ENTRIES * ENTRY_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    UnknownRequiredFlags,
    BadBounds,
    /// Entries are not in ascending slot order, or two entries claim one slot.
    BadOrder,
    /// A role field names no role this format version defines.
    UnknownRole,
    /// A role that must name something carries an all-zero identity, or a role
    /// that names nothing carries one.
    BadIdentity,
}

/// What the kernel must place in one capability slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    EndpointFactory,
    SharedBufferFactory,
    Executable,
    EndpointClient,
    EndpointService,
    ObjectStore,
    DirectoryRoot,
    Input,
    GenerationControl,
    /// Resolves to a block device when one is enumerated, and to an object
    /// store when none is. The kernel decides; the layout only fixes the slot.
    StorageCapability,
    /// Occupies its slot only when the matching block device is present.
    TransferReceiver,
    TransferSource,
}

impl Role {
    fn from_wire(value: u32) -> Option<Self> {
        Some(match value {
            ROLE_ENDPOINT_FACTORY => Self::EndpointFactory,
            ROLE_SHARED_BUFFER_FACTORY => Self::SharedBufferFactory,
            ROLE_EXECUTABLE => Self::Executable,
            ROLE_ENDPOINT_CLIENT => Self::EndpointClient,
            ROLE_ENDPOINT_SERVICE => Self::EndpointService,
            ROLE_OBJECT_STORE => Self::ObjectStore,
            ROLE_DIRECTORY_ROOT => Self::DirectoryRoot,
            ROLE_INPUT => Self::Input,
            ROLE_GENERATION_CONTROL => Self::GenerationControl,
            ROLE_STORAGE_CAPABILITY => Self::StorageCapability,
            ROLE_TRANSFER_RECEIVER => Self::TransferReceiver,
            ROLE_TRANSFER_SOURCE => Self::TransferSource,
            _ => return None,
        })
    }

    /// Whether this role identifies a specific component or channel. The roles
    /// that answer `false` are singular — there is one endpoint factory, one
    /// input device — so a name would add nothing to distinguish them.
    pub fn is_named(self) -> bool {
        matches!(
            self,
            Self::Executable | Self::EndpointClient | Self::EndpointService
        )
    }
}

/// One slot's declared occupant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutEntry {
    /// Component identity for an executable, channel identity for an endpoint
    /// half, all-zero for a role that names nothing.
    pub name_identity: [u8; 32],
    pub slot: u32,
    pub role: Role,
    pub rights: u64,
}

/// A decoded, structurally validated boot layout. Entries are sorted by slot
/// and each slot appears at most once, so lookup is deterministic.
#[derive(Debug, Clone, Copy)]
pub struct BootLayout<'a> {
    bytes: &'a [u8],
    entry_count: usize,
    generation_number: u64,
}

impl<'a> BootLayout<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, DecodeError> {
        if bytes.len() < HEADER_BYTES || bytes.len() > MAX_BYTES {
            return Err(DecodeError::Truncated);
        }
        if bytes[..8] != MAGIC {
            return Err(DecodeError::BadMagic);
        }
        if u32_at(bytes, 8)? != FORMAT_VERSION || u32_at(bytes, 12)? as usize != HEADER_BYTES {
            return Err(DecodeError::UnsupportedVersion);
        }
        if u64_at(bytes, 16)? != 0 {
            return Err(DecodeError::UnknownRequiredFlags);
        }
        let generation_number = u64_at(bytes, 24)?;
        let entry_count = u32_at(bytes, 32)? as usize;
        let total_len = u32_at(bytes, 36)? as usize;
        if entry_count > MAX_ENTRIES
            || total_len != HEADER_BYTES + entry_count * ENTRY_BYTES
            || total_len != bytes.len()
        {
            return Err(DecodeError::BadBounds);
        }
        for index in 0..entry_count {
            let entry = decode_entry(bytes, index)?;
            // Ascending and unique: a slot claimed twice would make the layout
            // depend on which claim the kernel applied last, which is the
            // positional ambiguity this format exists to remove.
            if index > 0 && entry.slot <= decode_entry(bytes, index - 1)?.slot {
                return Err(DecodeError::BadOrder);
            }
            if entry.slot as usize >= MAX_ENTRIES {
                return Err(DecodeError::BadBounds);
            }
            if entry.role.is_named() == (entry.name_identity == [0; 32]) {
                return Err(DecodeError::BadIdentity);
            }
        }
        Ok(Self {
            bytes,
            entry_count,
            generation_number,
        })
    }

    /// The generation this layout was built for. The kernel checks it against
    /// the generation carrying it, so a builder that emitted one generation's
    /// layout into another fails closed at boot rather than launching init with
    /// a table its components do not expect.
    pub fn generation_number(&self) -> u64 {
        self.generation_number
    }

    pub fn entry_count(&self) -> usize {
        self.entry_count
    }

    pub fn entry(&self, index: usize) -> Option<LayoutEntry> {
        (index < self.entry_count)
            .then(|| decode_entry(self.bytes, index).expect("validated layout entry"))
    }

    /// The entry declared for `slot`, or `None` when the layout leaves it
    /// empty.
    pub fn slot(&self, slot: u32) -> Option<LayoutEntry> {
        (0..self.entry_count)
            .map(|index| decode_entry(self.bytes, index).expect("validated layout entry"))
            .find(|entry| entry.slot == slot)
    }
}

/// Stable identity for a component named in a layout entry.
pub fn component_identity(name: &str) -> [u8; 32] {
    identity(b"slime-boot-layout-component-v1:", name)
}

/// Stable identity for a channel half named in a layout entry. A separate
/// domain from [`component_identity`], so a component and a channel sharing a
/// name are still distinct identities.
pub fn channel_identity(name: &str) -> [u8; 32] {
    identity(b"slime-boot-layout-channel-v1:", name)
}

fn identity(domain: &[u8], name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.finalize()
}

fn decode_entry(bytes: &[u8], index: usize) -> Result<LayoutEntry, DecodeError> {
    let offset = HEADER_BYTES + index * ENTRY_BYTES;
    let entry = bytes
        .get(offset..offset + ENTRY_BYTES)
        .ok_or(DecodeError::Truncated)?;
    Ok(LayoutEntry {
        name_identity: entry[..32].try_into().unwrap(),
        slot: u32_at(entry, 32)?,
        role: Role::from_wire(u32_at(entry, 36)?).ok_or(DecodeError::UnknownRole)?,
        rights: u64_at(entry, 40)?,
    })
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, DecodeError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(DecodeError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, DecodeError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(DecodeError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;

    fn build(number: u64, entries: &[LayoutEntry]) -> alloc::vec::Vec<u8> {
        let total_len = HEADER_BYTES + entries.len() * ENTRY_BYTES;
        let mut bytes = alloc::vec![0u8; total_len];
        bytes[..8].copy_from_slice(&MAGIC);
        bytes[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes[24..32].copy_from_slice(&number.to_le_bytes());
        bytes[32..36].copy_from_slice(&(entries.len() as u32).to_le_bytes());
        bytes[36..40].copy_from_slice(&(total_len as u32).to_le_bytes());
        for (index, entry) in entries.iter().enumerate() {
            let offset = HEADER_BYTES + index * ENTRY_BYTES;
            bytes[offset..offset + 32].copy_from_slice(&entry.name_identity);
            bytes[offset + 32..offset + 36].copy_from_slice(&entry.slot.to_le_bytes());
            bytes[offset + 36..offset + 40].copy_from_slice(&wire_role(entry.role).to_le_bytes());
            bytes[offset + 40..offset + 48].copy_from_slice(&entry.rights.to_le_bytes());
        }
        bytes
    }

    fn wire_role(role: Role) -> u32 {
        match role {
            Role::EndpointFactory => ROLE_ENDPOINT_FACTORY,
            Role::SharedBufferFactory => ROLE_SHARED_BUFFER_FACTORY,
            Role::Executable => ROLE_EXECUTABLE,
            Role::EndpointClient => ROLE_ENDPOINT_CLIENT,
            Role::EndpointService => ROLE_ENDPOINT_SERVICE,
            Role::ObjectStore => ROLE_OBJECT_STORE,
            Role::DirectoryRoot => ROLE_DIRECTORY_ROOT,
            Role::Input => ROLE_INPUT,
            Role::GenerationControl => ROLE_GENERATION_CONTROL,
            Role::StorageCapability => ROLE_STORAGE_CAPABILITY,
            Role::TransferReceiver => ROLE_TRANSFER_RECEIVER,
            Role::TransferSource => ROLE_TRANSFER_SOURCE,
        }
    }

    fn named(slot: u32, name: &str, rights: u64) -> LayoutEntry {
        LayoutEntry {
            name_identity: component_identity(name),
            slot,
            role: Role::Executable,
            rights,
        }
    }

    fn anonymous(slot: u32, role: Role) -> LayoutEntry {
        LayoutEntry {
            name_identity: [0; 32],
            slot,
            role,
            rights: 0x2_0000,
        }
    }

    #[test]
    fn decodes_and_looks_up_by_slot() {
        let factory = anonymous(0, Role::EndpointFactory);
        let console = named(1, "console", 0x1_0008);
        let bytes = build(12, &[factory, console]);
        let layout = BootLayout::decode(&bytes).expect("decodes");
        assert_eq!(layout.generation_number(), 12);
        assert_eq!(layout.entry_count(), 2);
        assert_eq!(layout.slot(0), Some(factory));
        assert_eq!(layout.slot(1), Some(console));
        // A slot the layout does not declare is empty, not defaulted.
        assert_eq!(layout.slot(2), None);
    }

    /// Component and channel namespaces are distinct, so the same text in each
    /// is a different identity. Without this, a channel labelled `console`
    /// would resolve to the `console` executable.
    #[test]
    fn component_and_channel_identities_do_not_collide() {
        assert_ne!(component_identity("console"), channel_identity("console"));
    }

    #[test]
    fn unsorted_or_duplicate_slots_fail_closed() {
        let bytes = build(1, &[named(2, "a", 0), named(1, "b", 0)]);
        assert!(matches!(
            BootLayout::decode(&bytes),
            Err(DecodeError::BadOrder)
        ));
        let bytes = build(1, &[named(1, "a", 0), named(1, "b", 0)]);
        assert!(matches!(
            BootLayout::decode(&bytes),
            Err(DecodeError::BadOrder)
        ));
    }

    /// A named role without a name, or an unnamed role carrying one, is a
    /// builder bug that would otherwise resolve to the wrong object silently.
    #[test]
    fn identity_must_match_the_role() {
        let mut entry = named(1, "console", 0);
        entry.name_identity = [0; 32];
        let bytes = build(1, &[entry]);
        assert!(matches!(
            BootLayout::decode(&bytes),
            Err(DecodeError::BadIdentity)
        ));

        let mut entry = anonymous(1, Role::Input);
        entry.name_identity = component_identity("input");
        let bytes = build(1, &[entry]);
        assert!(matches!(
            BootLayout::decode(&bytes),
            Err(DecodeError::BadIdentity)
        ));
    }

    #[test]
    fn unknown_role_fails_closed() {
        let mut bytes = build(1, &[named(1, "console", 0)]);
        bytes[HEADER_BYTES + 36..HEADER_BYTES + 40].copy_from_slice(&9999u32.to_le_bytes());
        assert!(matches!(
            BootLayout::decode(&bytes),
            Err(DecodeError::UnknownRole)
        ));
    }

    #[test]
    fn slot_beyond_the_capability_table_fails_closed() {
        let bytes = build(1, &[named(MAX_ENTRIES as u32, "console", 0)]);
        assert!(matches!(
            BootLayout::decode(&bytes),
            Err(DecodeError::BadBounds)
        ));
    }

    #[test]
    fn wrong_magic_and_version_fail_closed() {
        let mut bytes = build(1, &[named(1, "console", 0)]);
        bytes[0] = b'X';
        assert!(matches!(
            BootLayout::decode(&bytes),
            Err(DecodeError::BadMagic)
        ));
        let mut bytes = build(1, &[named(1, "console", 0)]);
        bytes[8..12].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        assert!(matches!(
            BootLayout::decode(&bytes),
            Err(DecodeError::UnsupportedVersion)
        ));
    }

    #[test]
    fn truncated_and_bad_count_fail_closed() {
        let bytes = build(1, &[named(1, "console", 0)]);
        assert!(matches!(
            BootLayout::decode(&bytes[..HEADER_BYTES - 1]),
            Err(DecodeError::Truncated)
        ));
        let mut bytes = build(1, &[named(1, "console", 0)]);
        // Claim two entries but supply one entry of bytes.
        bytes[32..36].copy_from_slice(&2u32.to_le_bytes());
        assert!(matches!(
            BootLayout::decode(&bytes),
            Err(DecodeError::BadBounds)
        ));
    }

    /// Rights are 64-bit, matching the generation's v3 grant width, so a layout
    /// can carry every kernel-checked operation the capability model defines.
    #[test]
    fn rights_carry_the_full_sixty_four_bit_width() {
        let entry = named(1, "console", 1 << 40);
        let bytes = build(1, &[entry]);
        let layout = BootLayout::decode(&bytes).expect("decodes");
        assert_eq!(layout.slot(1).expect("entry").rights, 1 << 40);
    }
}
