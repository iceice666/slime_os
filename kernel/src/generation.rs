use alloc::vec::Vec;

use crate::sha256::{self, Sha256};

const MAGIC: &[u8; 8] = b"SLIMEGEN";
const HEADER_SIZE: usize = 68;
const OBJECT_SIZE: usize = 48;
const COMPONENT_SIZE: usize = 12;
const GRANT_SIZE: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    BadHash,
    BadIndex,
    BadUtf8,
    BadObjectHash,
    DuplicateName,
    BadBootstrap,
}

#[derive(Clone, Copy)]
pub struct Object<'a> {
    pub kind: u32,
    pub bytes: &'a [u8],
}

pub struct Component<'a> {
    pub name: &'a str,
    pub role: u8,
    pub object: usize,
}

pub struct Grant<'a> {
    pub name: &'a str,
    pub source: usize,
    pub target: usize,
    pub rights: u8,
}

pub struct Generation<'a> {
    pub number: u64,
    pub bootstrap: usize,
    pub objects: Vec<Object<'a>>,
    pub components: Vec<Component<'a>>,
    pub grants: Vec<Grant<'a>>,
}

impl<'a> Generation<'a> {
    pub fn component(&self, name: &str) -> Option<&Component<'a>> {
        self.components
            .iter()
            .find(|component| component.name == name)
    }

    pub fn grant(&self, name: &str) -> Option<&Grant<'a>> {
        self.grants.iter().find(|grant| grant.name == name)
    }

    pub fn component_bytes(&self, name: &str) -> Option<&'a [u8]> {
        let component = self.component(name)?;
        self.objects
            .get(component.object)
            .map(|object| object.bytes)
    }
}

pub fn decode(bytes: &[u8]) -> Result<Generation<'_>, DecodeError> {
    if bytes.len() < HEADER_SIZE {
        return Err(DecodeError::Truncated);
    }
    if &bytes[..8] != MAGIC {
        return Err(DecodeError::BadMagic);
    }
    if read_u32(bytes, 8)? != 1 || read_u32(bytes, 12)? as usize != HEADER_SIZE {
        return Err(DecodeError::UnsupportedVersion);
    }

    let mut hasher = Sha256::new();
    hasher.update(&bytes[..24]);
    hasher.update(&[0; 32]);
    hasher.update(&bytes[56..]);
    if hasher.finalize() != bytes[24..56] {
        return Err(DecodeError::BadHash);
    }
    let number = read_u64(bytes, 16)?;
    let object_count = read_u16(bytes, 56)? as usize;
    let component_count = read_u16(bytes, 58)? as usize;
    let grant_count = read_u16(bytes, 60)? as usize;
    let bootstrap = read_u16(bytes, 62)? as usize;
    if bootstrap >= component_count {
        return Err(DecodeError::BadBootstrap);
    }

    let object_start = HEADER_SIZE;
    let component_start = checked_add(object_start, checked_mul(object_count, OBJECT_SIZE)?)?;
    let grant_start = checked_add(
        component_start,
        checked_mul(component_count, COMPONENT_SIZE)?,
    )?;
    let strings_start = checked_add(grant_start, checked_mul(grant_count, GRANT_SIZE)?)?;
    if strings_start > bytes.len() {
        return Err(DecodeError::Truncated);
    }

    let mut string_end = 0usize;
    for index in 0..component_count {
        let offset = read_u32(bytes, component_start + index * COMPONENT_SIZE)? as usize;
        string_end = string_end.max(string_extent(bytes, strings_start, offset)?);
    }
    for index in 0..grant_count {
        let offset = read_u32(bytes, grant_start + index * GRANT_SIZE)? as usize;
        string_end = string_end.max(string_extent(bytes, strings_start, offset)?);
    }
    let blobs_start = checked_add(strings_start, string_end)?;

    let mut objects = Vec::with_capacity(object_count);
    for index in 0..object_count {
        let offset = object_start + index * OBJECT_SIZE;
        let digest: [u8; 32] = bytes[offset..offset + 32]
            .try_into()
            .map_err(|_| DecodeError::Truncated)?;
        let blob_offset = read_u64(bytes, offset + 32)? as usize;
        let blob_len = read_u32(bytes, offset + 40)? as usize;
        let kind = read_u32(bytes, offset + 44)?;
        let start = checked_add(blobs_start, blob_offset)?;
        let end = checked_add(start, blob_len)?;
        let blob = bytes.get(start..end).ok_or(DecodeError::Truncated)?;
        if sha256::digest(blob) != digest {
            return Err(DecodeError::BadObjectHash);
        }
        objects.push(Object { kind, bytes: blob });
    }

    let mut components = Vec::with_capacity(component_count);
    for index in 0..component_count {
        let offset = component_start + index * COMPONENT_SIZE;
        let name_offset = read_u32(bytes, offset)? as usize;
        let object = read_u32(bytes, offset + 4)? as usize;
        if object >= object_count {
            return Err(DecodeError::BadIndex);
        }
        let role = bytes[offset + 8];
        let name = read_string(bytes, strings_start, name_offset)?;
        if components
            .iter()
            .any(|component: &Component<'_>| component.name == name)
        {
            return Err(DecodeError::DuplicateName);
        }
        components.push(Component { name, role, object });
    }
    if components[bootstrap].role != 1 {
        return Err(DecodeError::BadBootstrap);
    }

    let mut grants = Vec::with_capacity(grant_count);
    for index in 0..grant_count {
        let offset = grant_start + index * GRANT_SIZE;
        let name_offset = read_u32(bytes, offset)? as usize;
        let source = read_u32(bytes, offset + 4)? as usize;
        let target = read_u32(bytes, offset + 8)? as usize;
        if source >= component_count || target >= component_count {
            return Err(DecodeError::BadIndex);
        }
        let rights = bytes[offset + 12];
        let name = read_string(bytes, strings_start, name_offset)?;
        if grants.iter().any(|grant: &Grant<'_>| grant.name == name) {
            return Err(DecodeError::DuplicateName);
        }
        grants.push(Grant {
            name,
            source,
            target,
            rights,
        });
    }

    Ok(Generation {
        number,
        bootstrap,
        objects,
        components,
        grants,
    })
}

fn checked_add(left: usize, right: usize) -> Result<usize, DecodeError> {
    left.checked_add(right).ok_or(DecodeError::Truncated)
}

fn checked_mul(left: usize, right: usize) -> Result<usize, DecodeError> {
    left.checked_mul(right).ok_or(DecodeError::Truncated)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, DecodeError> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(DecodeError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, DecodeError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(DecodeError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, DecodeError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(DecodeError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}

fn string_extent(bytes: &[u8], start: usize, offset: usize) -> Result<usize, DecodeError> {
    let absolute = checked_add(start, offset)?;
    let len = read_u16(bytes, absolute)? as usize;
    checked_add(offset, checked_add(2, len)?)
}

fn read_string(bytes: &[u8], start: usize, offset: usize) -> Result<&str, DecodeError> {
    let absolute = checked_add(start, offset)?;
    let len = read_u16(bytes, absolute)? as usize;
    let value = bytes
        .get(absolute + 2..absolute + 2 + len)
        .ok_or(DecodeError::Truncated)?;
    core::str::from_utf8(value).map_err(|_| DecodeError::BadUtf8)
}
