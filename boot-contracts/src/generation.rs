use crate::sha256::Sha256;

/// Format 3 magic (current). Grant rights are a 64-bit little-endian value.
pub const MAGIC_V3: [u8; 8] = *b"SLIMEG3\0";
/// Format 2 magic, retained for the bounded rollback window. Grant rights are
/// a 32-bit value; decoding widens them to `u64` without changing meaning.
pub const MAGIC_V2: [u8; 8] = *b"SLIMEG2\0";
/// The magic emitted for newly built generations.
pub const MAGIC: [u8; 8] = MAGIC_V3;
/// Retained format-2 version number. `FORMAT_VERSION` (3) is the current one.
pub const FORMAT_VERSION_V2: u32 = 2;
const MAX_COMPONENTS_V2: usize = 32;
include!("generated/generation.rs");
pub const KIND_KERNEL: u32 = 1;
pub const KIND_BOOTSTRAP: u32 = 2;
pub const KIND_COMPONENT: u32 = 3;
pub const KIND_RESOURCE: u32 = 4;
pub const ROLE_INIT: u32 = 1;
/// A capability rights bitset. Flat `u64` as of generation format v3.
pub type Rights = u64;
pub const RIGHT_TRANSFER: Rights = 1 << 2;
pub const RIGHT_ALL_V2: Rights = (1 << 24) - 1;
pub const RIGHT_ALL: Rights = (1 << 26) - 1;
pub const MAX_SPAWN_BUDGET: u16 = 32;
pub const POLICY_IMMUTABLE: u32 = 1;
pub const POLICY_EPHEMERAL: u32 = 2;
pub const POLICY_PRESERVE: u32 = 3;
pub const POLICY_SNAPSHOT_BEFORE_UPGRADE: u32 = 4;
pub const POLICY_DISCARD_ON_ROLLBACK: u32 = 5;

const IDENTITY_OFFSET: usize = 24;
const IDENTITY_END: usize = 56;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    UnknownRequiredFlags,
    BadHeader,
    BadIdentity,
    BadBounds,
    BadIndex,
    BadUtf8,
    BadOrder,
    DuplicateName,
    BadObjectHash,
    BadKernel,
    BadBootstrap,
    BadDependency,
    BadState,
    BadHealth,
    UnknownEnum,
    NonZeroReserved,
}

#[derive(Debug, Clone, Copy)]
pub struct Object<'a> {
    pub id: &'a str,
    pub kind: u32,
    pub digest: [u8; 32],
    pub bytes: &'a [u8],
}

#[derive(Debug, Clone, Copy)]
pub struct Component<'a> {
    pub name: &'a str,
    pub object: usize,
    pub role: u32,
    pub spawn_budget: u16,
    dependency_start: usize,
    dependency_count: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct Grant<'a> {
    pub name: &'a str,
    pub source: usize,
    pub target: usize,
    pub rights: Rights,
    pub transferable: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct StateBinding<'a> {
    pub name: &'a str,
    pub owner: usize,
    pub schema_version: u32,
    pub policy: u32,
}

pub struct Generation<'a> {
    bytes: &'a [u8],
    /// Decoded format version: `FORMAT_VERSION` (3) or `FORMAT_VERSION_V2` (2).
    /// Selects the capability-grant rights layout.
    pub version: u32,
    pub identity: [u8; 32],
    pub number: u64,
    pub parent: Option<[u8; 32]>,
    pub target: &'a str,
    pub kernel_object: usize,
    pub bootstrap: usize,
    pub boot_attempts: u32,
    object_count: usize,
    component_count: usize,
    dependency_count: usize,
    grant_count: usize,
    state_count: usize,
    health_count: usize,
    object_offset: usize,
    component_offset: usize,
    dependency_offset: usize,
    grant_offset: usize,
    state_offset: usize,
    health_offset: usize,
    string_offset: usize,
}

impl<'a> Generation<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, DecodeError> {
        if bytes.len() < HEADER_LEN {
            return Err(DecodeError::Truncated);
        }
        let version = match (bytes[..8].try_into().unwrap(), u32_at(bytes, 8)?) {
            (MAGIC_V3, FORMAT_VERSION) => FORMAT_VERSION,
            (MAGIC_V2, FORMAT_VERSION_V2) => FORMAT_VERSION_V2,
            (MAGIC_V3, _) | (MAGIC_V2, _) => return Err(DecodeError::UnsupportedVersion),
            _ => return Err(DecodeError::BadMagic),
        };
        if u32_at(bytes, 12)? as usize != HEADER_LEN {
            return Err(DecodeError::BadHeader);
        }
        if u64_at(bytes, 16)? != 0 {
            return Err(DecodeError::UnknownRequiredFlags);
        }
        if bytes[216..HEADER_LEN].iter().any(|byte| *byte != 0) {
            return Err(DecodeError::NonZeroReserved);
        }
        let total_len = u64_at(bytes, 208)? as usize;
        if total_len != bytes.len() || total_len > MAX_GENERATION_BYTES {
            return Err(DecodeError::BadBounds);
        }
        let identity: [u8; 32] = bytes[IDENTITY_OFFSET..IDENTITY_END].try_into().unwrap();
        if generation_identity(bytes) != identity {
            return Err(DecodeError::BadIdentity);
        }
        let parent_bytes: [u8; 32] = bytes[64..96].try_into().unwrap();
        let object_count = bounded_count(u32_at(bytes, 112)? as usize, 1, MAX_OBJECTS)?;
        let component_limit = if version == FORMAT_VERSION_V2 {
            MAX_COMPONENTS_V2
        } else {
            MAX_COMPONENTS
        };
        let component_count = bounded_count(u32_at(bytes, 116)? as usize, 1, component_limit)?;
        let dependency_count = bounded_count(u32_at(bytes, 120)? as usize, 0, MAX_DEPENDENCIES)?;
        let grant_count = bounded_count(u32_at(bytes, 124)? as usize, 0, MAX_GRANTS)?;
        let state_count = bounded_count(u32_at(bytes, 128)? as usize, 0, MAX_STATES)?;
        let health_count = bounded_count(u32_at(bytes, 132)? as usize, 0, MAX_HEALTH_COMPONENTS)?;
        let object_offset = u64_at(bytes, 136)? as usize;
        let component_offset = u64_at(bytes, 144)? as usize;
        let dependency_offset = u64_at(bytes, 152)? as usize;
        let grant_offset = u64_at(bytes, 160)? as usize;
        let state_offset = u64_at(bytes, 168)? as usize;
        let health_offset = u64_at(bytes, 176)? as usize;
        let string_offset = u64_at(bytes, 184)? as usize;
        let string_len = u64_at(bytes, 192)? as usize;
        let payload_offset = u64_at(bytes, 200)? as usize;
        if string_len > MAX_STRING_TABLE_BYTES {
            return Err(DecodeError::BadBounds);
        }
        check_section(object_offset, object_count, OBJECT_LEN, component_offset)?;
        check_section(
            component_offset,
            component_count,
            COMPONENT_LEN,
            dependency_offset,
        )?;
        check_section(
            dependency_offset,
            dependency_count,
            DEPENDENCY_LEN,
            grant_offset,
        )?;
        check_section(grant_offset, grant_count, GRANT_LEN, state_offset)?;
        check_section(state_offset, state_count, STATE_LEN, health_offset)?;
        check_section(health_offset, health_count, HEALTH_LEN, string_offset)?;
        if object_offset != HEADER_LEN
            || string_offset.checked_add(string_len) != Some(payload_offset)
            || payload_offset > bytes.len()
        {
            return Err(DecodeError::BadBounds);
        }
        let target = read_string(
            bytes,
            string_offset,
            string_len,
            u32_at(bytes, 96)? as usize,
        )?;
        let generation = Self {
            bytes,
            version,
            identity,
            number: u64_at(bytes, 56)?,
            parent: (parent_bytes != [0; 32]).then_some(parent_bytes),
            target,
            kernel_object: u32_at(bytes, 100)? as usize,
            bootstrap: u32_at(bytes, 104)? as usize,
            boot_attempts: u32_at(bytes, 108)?,
            object_count,
            component_count,
            dependency_count,
            grant_count,
            state_count,
            health_count,
            object_offset,
            component_offset,
            dependency_offset,
            grant_offset,
            state_offset,
            health_offset,
            string_offset,
        };
        generation.validate(payload_offset, string_len)?;
        Ok(generation)
    }

    pub fn object_count(&self) -> usize {
        self.object_count
    }
    pub fn component_count(&self) -> usize {
        self.component_count
    }
    pub fn grant_count(&self) -> usize {
        self.grant_count
    }
    pub fn state_count(&self) -> usize {
        self.state_count
    }
    pub fn health_count(&self) -> usize {
        self.health_count
    }

    pub fn object(&self, index: usize) -> Result<Object<'a>, DecodeError> {
        if index >= self.object_count {
            return Err(DecodeError::BadIndex);
        }
        let offset = self.object_offset + index * OBJECT_LEN;
        let id = self.string(u32_at(self.bytes, offset)? as usize)?;
        let kind = u32_at(self.bytes, offset + 4)?;
        let payload_offset = u64_at(self.bytes, offset + 8)? as usize;
        let payload_len = u64_at(self.bytes, offset + 16)? as usize;
        let digest: [u8; 32] = self.bytes[offset + 24..offset + 56].try_into().unwrap();
        if self.bytes[offset + 56..offset + OBJECT_LEN]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(DecodeError::NonZeroReserved);
        }
        if payload_len > MAX_OBJECT_PAYLOAD_BYTES {
            return Err(DecodeError::BadBounds);
        }
        let end = payload_offset
            .checked_add(payload_len)
            .ok_or(DecodeError::BadBounds)?;
        let payload = self
            .bytes
            .get(payload_offset..end)
            .ok_or(DecodeError::BadBounds)?;
        Ok(Object {
            id,
            kind,
            digest,
            bytes: payload,
        })
    }

    pub fn component(&self, index: usize) -> Result<Component<'a>, DecodeError> {
        if index >= self.component_count {
            return Err(DecodeError::BadIndex);
        }
        let offset = self.component_offset + index * COMPONENT_LEN;
        if self.bytes[offset + 24..offset + COMPONENT_LEN]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(DecodeError::NonZeroReserved);
        }
        Ok(Component {
            name: self.string(u32_at(self.bytes, offset)? as usize)?,
            object: u32_at(self.bytes, offset + 4)? as usize,
            role: u32_at(self.bytes, offset + 8)?,
            dependency_start: u32_at(self.bytes, offset + 12)? as usize,
            dependency_count: u32_at(self.bytes, offset + 16)? as usize,
            spawn_budget: u32_at(self.bytes, offset + 20)?
                .try_into()
                .map_err(|_| DecodeError::BadBounds)?,
        })
    }

    pub fn component_named(&self, name: &str) -> Option<Component<'a>> {
        (0..self.component_count).find_map(|index| {
            self.component(index)
                .ok()
                .filter(|component| component.name == name)
        })
    }

    pub fn component_bytes(&self, name: &str) -> Option<&'a [u8]> {
        let component = self.component_named(name)?;
        self.object(component.object)
            .ok()
            .map(|object| object.bytes)
    }

    pub fn dependency(
        &self,
        component: Component<'a>,
        index: usize,
    ) -> Result<Component<'a>, DecodeError> {
        if index >= component.dependency_count {
            return Err(DecodeError::BadIndex);
        }
        let absolute = component
            .dependency_start
            .checked_add(index)
            .ok_or(DecodeError::BadIndex)?;
        if absolute >= self.dependency_count {
            return Err(DecodeError::BadIndex);
        }
        self.component(u32_at(
            self.bytes,
            self.dependency_offset + absolute * DEPENDENCY_LEN,
        )? as usize)
    }

    pub fn grant(&self, index: usize) -> Result<Grant<'a>, DecodeError> {
        if index >= self.grant_count {
            return Err(DecodeError::BadIndex);
        }
        let offset = self.grant_offset + index * GRANT_LEN;
        // Grant record layout differs by format version: v3 carries a 64-bit
        // rights field at +12 with `transferable` at +20; v2 carries a 32-bit
        // rights field at +12 with `transferable` at +16. Both records are
        // GRANT_LEN (32) bytes; the trailing bytes past `transferable` are
        // reserved and must be zero.
        let (rights, transferable_offset) = if self.version == FORMAT_VERSION_V2 {
            (u64::from(u32_at(self.bytes, offset + 12)?), offset + 16)
        } else {
            (u64_at(self.bytes, offset + 12)?, offset + 20)
        };
        if self.bytes[transferable_offset + 4..offset + GRANT_LEN]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(DecodeError::NonZeroReserved);
        }
        Ok(Grant {
            name: self.string(u32_at(self.bytes, offset)? as usize)?,
            source: u32_at(self.bytes, offset + 4)? as usize,
            target: u32_at(self.bytes, offset + 8)? as usize,
            rights,
            transferable: match u32_at(self.bytes, transferable_offset)? {
                0 => false,
                1 => true,
                _ => return Err(DecodeError::UnknownEnum),
            },
        })
    }

    pub fn grant_named(&self, name: &str) -> Option<Grant<'a>> {
        (0..self.grant_count)
            .find_map(|index| self.grant(index).ok().filter(|grant| grant.name == name))
    }

    pub fn authority_manifest_identity(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"slime-authority-manifest-v1");
        for index in 0..self.grant_count {
            let grant = self.grant(index).expect("validated generation grant");
            let source = self
                .component(grant.source)
                .expect("validated grant source");
            let target = self
                .component(grant.target)
                .expect("validated grant target");
            update_bounded_string(&mut hasher, grant.name);
            update_bounded_string(&mut hasher, source.name);
            update_bounded_string(&mut hasher, target.name);
            // v2 releases hashed a 32-bit rights field; retain that width so a
            // known-good v2 generation still matches its signed release during
            // the rollback window. v3 hashes the full 64-bit rights value.
            if self.version == FORMAT_VERSION_V2 {
                hasher.update(&(grant.rights as u32).to_le_bytes());
            } else {
                hasher.update(&grant.rights.to_le_bytes());
            }
            hasher.update(&u32::from(grant.transferable).to_le_bytes());
        }
        hasher.finalize()
    }

    pub fn state(&self, index: usize) -> Result<StateBinding<'a>, DecodeError> {
        if index >= self.state_count {
            return Err(DecodeError::BadIndex);
        }
        let offset = self.state_offset + index * STATE_LEN;
        if self.bytes[offset + 16..offset + STATE_LEN]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(DecodeError::NonZeroReserved);
        }
        Ok(StateBinding {
            name: self.string(u32_at(self.bytes, offset)? as usize)?,
            owner: u32_at(self.bytes, offset + 4)? as usize,
            schema_version: u32_at(self.bytes, offset + 8)?,
            policy: u32_at(self.bytes, offset + 12)?,
        })
    }

    pub fn health_component(&self, index: usize) -> Result<Component<'a>, DecodeError> {
        if index >= self.health_count {
            return Err(DecodeError::BadIndex);
        }
        self.component(u32_at(self.bytes, self.health_offset + index * HEALTH_LEN)? as usize)
    }

    fn string(&self, offset: usize) -> Result<&'a str, DecodeError> {
        let string_len = (u64_at(self.bytes, 200)? as usize)
            .checked_sub(self.string_offset)
            .ok_or(DecodeError::BadBounds)?;
        read_string(self.bytes, self.string_offset, string_len, offset)
    }

    fn validate(&self, payload_offset: usize, _string_len: usize) -> Result<(), DecodeError> {
        if self.kernel_object >= self.object_count || self.bootstrap >= self.component_count {
            return Err(DecodeError::BadIndex);
        }
        let mut previous_id: Option<&str> = None;
        let mut previous_payload_end = payload_offset;
        for index in 0..self.object_count {
            let object = self.object(index)?;
            if !matches!(
                object.kind,
                KIND_KERNEL | KIND_BOOTSTRAP | KIND_COMPONENT | KIND_RESOURCE
            ) {
                return Err(DecodeError::UnknownEnum);
            }
            if previous_id.is_some_and(|previous| previous >= object.id) {
                return Err(DecodeError::BadOrder);
            }
            let record = self.object_offset + index * OBJECT_LEN;
            let start = u64_at(self.bytes, record + 8)? as usize;
            if start != previous_payload_end {
                return Err(DecodeError::BadBounds);
            }
            previous_payload_end = start
                .checked_add(object.bytes.len())
                .ok_or(DecodeError::BadBounds)?;
            let mut hasher = Sha256::new();
            hasher.update(object.bytes);
            if hasher.finalize() != object.digest {
                return Err(DecodeError::BadObjectHash);
            }
            previous_id = Some(object.id);
        }
        if previous_payload_end != self.bytes.len() {
            return Err(DecodeError::BadBounds);
        }
        if self.object(self.kernel_object)?.kind != KIND_KERNEL {
            return Err(DecodeError::BadKernel);
        }
        let mut previous_name: Option<&str> = None;
        for index in 0..self.component_count {
            let component = self.component(index)?;
            if component.object >= self.object_count
                || !matches!(component.role, 1..=4)
                || component.spawn_budget > MAX_SPAWN_BUDGET
            {
                return Err(DecodeError::BadIndex);
            }
            if previous_name.is_some_and(|previous| previous >= component.name) {
                return Err(DecodeError::BadOrder);
            }
            if component
                .dependency_start
                .checked_add(component.dependency_count)
                .is_none_or(|end| end > self.dependency_count)
            {
                return Err(DecodeError::BadDependency);
            }
            let mut previous_dependency = None;
            for dependency_index in 0..component.dependency_count {
                let dependency = self.dependency(component, dependency_index)?;
                let dep_index = self
                    .component_index(dependency.name)
                    .ok_or(DecodeError::BadDependency)?;
                if dep_index == index
                    || previous_dependency.is_some_and(|previous| previous >= dep_index)
                {
                    return Err(DecodeError::BadDependency);
                }
                previous_dependency = Some(dep_index);
            }
            previous_name = Some(component.name);
        }
        let bootstrap = self.component(self.bootstrap)?;
        if bootstrap.role != ROLE_INIT || self.object(bootstrap.object)?.kind != KIND_BOOTSTRAP {
            return Err(DecodeError::BadBootstrap);
        }
        let rights_mask = if self.version == FORMAT_VERSION_V2 {
            RIGHT_ALL_V2
        } else {
            RIGHT_ALL
        };
        let mut previous_grant: Option<(&str, usize, usize)> = None;
        for index in 0..self.grant_count {
            let grant = self.grant(index)?;
            if grant.source >= self.component_count
                || grant.target >= self.component_count
                || grant.rights == 0
                || grant.rights & !rights_mask != 0
                || (grant.rights & RIGHT_TRANSFER != 0) != grant.transferable
            {
                return Err(DecodeError::BadIndex);
            }
            let key = (grant.name, grant.source, grant.target);
            if previous_grant.is_some_and(|previous| previous >= key) {
                return Err(DecodeError::BadOrder);
            }
            previous_grant = Some(key);
        }
        let mut previous_state = None;
        for index in 0..self.state_count {
            let state = self.state(index)?;
            if state.owner >= self.component_count
                || state.schema_version == 0
                || !matches!(
                    state.policy,
                    POLICY_IMMUTABLE
                        | POLICY_EPHEMERAL
                        | POLICY_PRESERVE
                        | POLICY_SNAPSHOT_BEFORE_UPGRADE
                        | POLICY_DISCARD_ON_ROLLBACK
                )
            {
                return Err(DecodeError::BadState);
            }
            if previous_state.is_some_and(|previous| previous >= state.name) {
                return Err(DecodeError::BadOrder);
            }
            previous_state = Some(state.name);
        }
        if self.boot_attempts == 0 {
            return Err(DecodeError::BadHealth);
        }
        let mut previous_health = None;
        for index in 0..self.health_count {
            let component = self.health_component(index)?;
            let component_index = self
                .component_index(component.name)
                .ok_or(DecodeError::BadHealth)?;
            if previous_health.is_some_and(|previous| previous >= component_index) {
                return Err(DecodeError::BadHealth);
            }
            previous_health = Some(component_index);
        }
        Ok(())
    }

    fn component_index(&self, name: &str) -> Option<usize> {
        (0..self.component_count).find(|index| {
            self.component(*index)
                .is_ok_and(|component| component.name == name)
        })
    }
}

fn update_bounded_string(hasher: &mut Sha256, value: &str) {
    hasher.update(&(value.len() as u16).to_le_bytes());
    hasher.update(value.as_bytes());
}

pub fn generation_identity(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    if bytes.len() < IDENTITY_END {
        return [0; 32];
    }
    hasher.update(&bytes[..IDENTITY_OFFSET]);
    hasher.update(&[0u8; 32]);
    hasher.update(&bytes[IDENTITY_END..]);
    hasher.finalize()
}

fn bounded_count(value: usize, min: usize, max: usize) -> Result<usize, DecodeError> {
    if (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(DecodeError::BadBounds)
    }
}

fn check_section(start: usize, count: usize, size: usize, next: usize) -> Result<(), DecodeError> {
    if start.checked_add(count.checked_mul(size).ok_or(DecodeError::BadBounds)?) == Some(next) {
        Ok(())
    } else {
        Err(DecodeError::BadBounds)
    }
}

fn read_string(
    bytes: &[u8],
    base: usize,
    table_len: usize,
    offset: usize,
) -> Result<&str, DecodeError> {
    if offset >= table_len {
        return Err(DecodeError::BadBounds);
    }
    let absolute = base.checked_add(offset).ok_or(DecodeError::BadBounds)?;
    let length = u16_at(bytes, absolute)? as usize;
    if length > MAX_STRING_BYTES
        || offset
            .checked_add(2 + length)
            .is_none_or(|end| end > table_len)
    {
        return Err(DecodeError::BadBounds);
    }
    core::str::from_utf8(
        bytes
            .get(absolute + 2..absolute + 2 + length)
            .ok_or(DecodeError::Truncated)?,
    )
    .map_err(|_| DecodeError::BadUtf8)
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, DecodeError> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(DecodeError::Truncated)?
            .try_into()
            .unwrap(),
    ))
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

    /// Build a minimal valid generation with one self-grant, encoded in the
    /// requested format version. v2 packs grant rights as 4 bytes at +12 with
    /// `transferable` at +16; v3 packs rights as 8 bytes at +12 with
    /// `transferable` at +20. Every other record is identical.
    fn build(version: u32, rights: u64) -> alloc::vec::Vec<u8> {
        let magic = if version == FORMAT_VERSION_V2 {
            MAGIC_V2
        } else {
            MAGIC_V3
        };
        // String table: (u16 len, bytes) entries; offsets are relative.
        let mut strings = alloc::vec::Vec::new();
        let mut push_str = |s: &str| -> u32 {
            let offset = strings.len() as u32;
            strings.extend_from_slice(&(s.len() as u16).to_le_bytes());
            strings.extend_from_slice(s.as_bytes());
            offset
        };
        let target_off = push_str("t");
        let obj_a_off = push_str("a");
        let obj_b_off = push_str("b");
        let init_off = push_str("init");
        let grant_off = push_str("g");

        let object_offset = HEADER_LEN;
        let component_offset = object_offset + 2 * OBJECT_LEN;
        let dependency_offset = component_offset + COMPONENT_LEN;
        let grant_offset = dependency_offset;
        let state_offset = grant_offset + GRANT_LEN;
        let health_offset = state_offset;
        let string_offset = health_offset;
        let payload_offset = string_offset + strings.len();
        let total_len = payload_offset + 2;

        let kernel_digest = {
            let mut h = Sha256::new();
            h.update(b"K");
            h.finalize()
        };
        let bootstrap_digest = {
            let mut h = Sha256::new();
            h.update(b"B");
            h.finalize()
        };

        let mut bytes = alloc::vec![0u8; total_len];
        bytes[..8].copy_from_slice(&magic);
        bytes[8..12].copy_from_slice(&version.to_le_bytes());
        bytes[12..16].copy_from_slice(&(HEADER_LEN as u32).to_le_bytes());
        bytes[56..64].copy_from_slice(&1u64.to_le_bytes()); // generation_number
        bytes[96..100].copy_from_slice(&target_off.to_le_bytes());
        bytes[100..104].copy_from_slice(&0u32.to_le_bytes()); // kernel_object
        bytes[104..108].copy_from_slice(&0u32.to_le_bytes()); // bootstrap_component
        bytes[108..112].copy_from_slice(&1u32.to_le_bytes()); // boot_attempts
        bytes[112..116].copy_from_slice(&2u32.to_le_bytes()); // object_count
        bytes[116..120].copy_from_slice(&1u32.to_le_bytes()); // component_count
        bytes[124..128].copy_from_slice(&1u32.to_le_bytes()); // grant_count
        bytes[136..144].copy_from_slice(&(object_offset as u64).to_le_bytes());
        bytes[144..152].copy_from_slice(&(component_offset as u64).to_le_bytes());
        bytes[152..160].copy_from_slice(&(dependency_offset as u64).to_le_bytes());
        bytes[160..168].copy_from_slice(&(grant_offset as u64).to_le_bytes());
        bytes[168..176].copy_from_slice(&(state_offset as u64).to_le_bytes());
        bytes[176..184].copy_from_slice(&(health_offset as u64).to_le_bytes());
        bytes[184..192].copy_from_slice(&(string_offset as u64).to_le_bytes());
        bytes[192..200].copy_from_slice(&(strings.len() as u64).to_le_bytes());
        bytes[200..208].copy_from_slice(&(payload_offset as u64).to_le_bytes());
        bytes[208..216].copy_from_slice(&(total_len as u64).to_le_bytes());

        // Object 0 (id "a", kernel), object 1 (id "b", bootstrap).
        let obj0 = object_offset;
        bytes[obj0..obj0 + 4].copy_from_slice(&obj_a_off.to_le_bytes());
        bytes[obj0 + 4..obj0 + 8].copy_from_slice(&KIND_KERNEL.to_le_bytes());
        bytes[obj0 + 8..obj0 + 16].copy_from_slice(&(payload_offset as u64).to_le_bytes());
        bytes[obj0 + 16..obj0 + 24].copy_from_slice(&1u64.to_le_bytes());
        bytes[obj0 + 24..obj0 + 56].copy_from_slice(&kernel_digest);
        let obj1 = object_offset + OBJECT_LEN;
        bytes[obj1..obj1 + 4].copy_from_slice(&obj_b_off.to_le_bytes());
        bytes[obj1 + 4..obj1 + 8].copy_from_slice(&KIND_BOOTSTRAP.to_le_bytes());
        bytes[obj1 + 8..obj1 + 16].copy_from_slice(&((payload_offset + 1) as u64).to_le_bytes());
        bytes[obj1 + 16..obj1 + 24].copy_from_slice(&1u64.to_le_bytes());
        bytes[obj1 + 24..obj1 + 56].copy_from_slice(&bootstrap_digest);

        // Component 0 ("init", object 1, role init).
        let comp = component_offset;
        bytes[comp..comp + 4].copy_from_slice(&init_off.to_le_bytes());
        bytes[comp + 4..comp + 8].copy_from_slice(&1u32.to_le_bytes()); // object_index
        bytes[comp + 8..comp + 12].copy_from_slice(&ROLE_INIT.to_le_bytes());

        // Grant 0 ("g", self, rights).
        let grant = grant_offset;
        bytes[grant..grant + 4].copy_from_slice(&grant_off.to_le_bytes());
        bytes[grant + 4..grant + 8].copy_from_slice(&0u32.to_le_bytes()); // source
        bytes[grant + 8..grant + 12].copy_from_slice(&0u32.to_le_bytes()); // target
        let transferable = u32::from(rights & RIGHT_TRANSFER != 0);
        if version == FORMAT_VERSION_V2 {
            bytes[grant + 12..grant + 16].copy_from_slice(&(rights as u32).to_le_bytes());
            bytes[grant + 16..grant + 20].copy_from_slice(&transferable.to_le_bytes());
        } else {
            bytes[grant + 12..grant + 20].copy_from_slice(&rights.to_le_bytes());
            bytes[grant + 20..grant + 24].copy_from_slice(&transferable.to_le_bytes());
        }

        // String table and payloads.
        bytes[string_offset..string_offset + strings.len()].copy_from_slice(&strings);
        bytes[payload_offset] = b'K';
        bytes[payload_offset + 1] = b'B';

        let identity = generation_identity(&bytes);
        bytes[IDENTITY_OFFSET..IDENTITY_END].copy_from_slice(&identity);
        bytes
    }

    #[test]
    fn v3_generation_decodes_with_u64_rights() {
        let rights = RIGHT_ALL;
        let bytes = build(FORMAT_VERSION, rights);
        let generation = Generation::decode(&bytes).expect("v3 decodes");
        assert_eq!(generation.version, FORMAT_VERSION);
        assert_eq!(generation.grant(0).unwrap().rights, rights);
    }

    #[test]
    fn retained_v2_generation_still_decodes() {
        // v2 rights are 24-bit; widen to u64 without changing meaning.
        let rights = RIGHT_TRANSFER | 1;
        let bytes = build(FORMAT_VERSION_V2, rights);
        let generation = Generation::decode(&bytes).expect("retained v2 decodes");
        assert_eq!(generation.version, FORMAT_VERSION_V2);
        assert_eq!(generation.grant(0).unwrap().rights, rights);
        assert!(generation.grant(0).unwrap().transferable);
    }

    #[test]
    fn retained_v2_keeps_its_component_ceiling() {
        let mut bytes = build(FORMAT_VERSION_V2, RIGHT_TRANSFER | 1);
        bytes[116..120].copy_from_slice(&33u32.to_le_bytes());
        let identity = generation_identity(&bytes);
        bytes[IDENTITY_OFFSET..IDENTITY_END].copy_from_slice(&identity);
        assert!(matches!(
            Generation::decode(&bytes),
            Err(DecodeError::BadBounds)
        ));
    }

    #[test]
    fn retained_v2_rejects_v3_only_rights() {
        let bytes = build(FORMAT_VERSION_V2, 1 << 24);
        assert!(matches!(
            Generation::decode(&bytes),
            Err(DecodeError::BadIndex)
        ));
    }

    /// The stage-0 admission chain a retained v2 generation actually traverses
    /// on a rollback boot: the identity seal is verified, the kernel object is
    /// located, and its own kernel image is what would be loaded.
    ///
    /// Each generation embeds the kernel it boots (`stage0::verify_kernel`
    /// resolves `generation.kernel_object`), so a v2 rollback runs its v2-era
    /// kernel rather than the current one. That is why the current tree's
    /// v3-only rights cannot break the rollback window, and it is the property
    /// worth pinning: an unmodified v2 artifact stays admissible end to end.
    #[test]
    fn retained_v2_generation_passes_stage0_admission() {
        let bytes = build(FORMAT_VERSION_V2, RIGHT_TRANSFER | 1);

        // Stage 0 verifies the recorded identity before decoding anything.
        let identity = generation_identity(&bytes);
        assert_eq!(&bytes[IDENTITY_OFFSET..IDENTITY_END], &identity[..]);

        let generation = Generation::decode(&bytes).expect("retained v2 decodes");
        assert_eq!(generation.version, FORMAT_VERSION_V2);

        // The generation names its own kernel object, so rollback boots the
        // kernel that shipped with it.
        let kernel = generation
            .object(generation.kernel_object)
            .expect("kernel object present");
        assert_eq!(kernel.kind, KIND_KERNEL);
        assert!(!kernel.bytes.is_empty());

        // A bootstrap component is present and reachable, so stage 0 has an
        // init to launch.
        let bootstrap = generation
            .component(generation.bootstrap)
            .expect("bootstrap component present");
        assert_eq!(bootstrap.role, ROLE_INIT);

        // Any byte flipped in a retained artifact fails the identity seal
        // rather than being admitted.
        let mut tampered = bytes.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        assert_ne!(generation_identity(&tampered), identity);
    }

    /// A v2 generation's authority manifest keeps hashing rights at 32 bits, so
    /// its signed release still matches during the rollback window. Widening to
    /// `u64` in C7.1 must not have moved this value — if it had, every retained
    /// v2 release would fail authorization and the rollback window would be
    /// closed in practice while appearing open.
    #[test]
    fn retained_v2_authority_manifest_is_width_stable() {
        let rights = RIGHT_TRANSFER | 1;
        let v2_bytes = build(FORMAT_VERSION_V2, rights);
        let v3_bytes = build(FORMAT_VERSION, rights);
        let v2 = Generation::decode(&v2_bytes).expect("v2 decodes");
        let v3 = Generation::decode(&v3_bytes).expect("v3 decodes");

        // Same logical grant, different encoded widths: the hashes must differ,
        // because each version hashes its own on-disk width. A v2 release is
        // therefore only ever valid against a v2 generation.
        assert_ne!(
            v2.authority_manifest_identity(),
            v3.authority_manifest_identity()
        );

        // And the v2 value is stable across decodes of the same bytes.
        let again_bytes = build(FORMAT_VERSION_V2, rights);
        let again = Generation::decode(&again_bytes).expect("v2 decodes");
        assert_eq!(
            v2.authority_manifest_identity(),
            again.authority_manifest_identity()
        );
    }

    #[test]
    fn unsupported_version_fails_closed() {
        let mut bytes = build(FORMAT_VERSION, RIGHT_ALL);
        bytes[8..12].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        // Re-seal identity so the version, not the hash, is what fails.
        let identity = generation_identity(&bytes);
        bytes[IDENTITY_OFFSET..IDENTITY_END].copy_from_slice(&identity);
        assert!(matches!(
            Generation::decode(&bytes),
            Err(DecodeError::UnsupportedVersion)
        ));
    }

    #[test]
    fn wrong_magic_fails_closed() {
        let mut bytes = build(FORMAT_VERSION, RIGHT_ALL);
        bytes[0] = b'X';
        let identity = generation_identity(&bytes);
        bytes[IDENTITY_OFFSET..IDENTITY_END].copy_from_slice(&identity);
        assert!(matches!(
            Generation::decode(&bytes),
            Err(DecodeError::BadMagic)
        ));
    }
}
