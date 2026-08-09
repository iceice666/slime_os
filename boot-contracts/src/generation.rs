use crate::sha256::Sha256;

pub const MAGIC_V4: [u8; 8] = *b"SLIMEG4\0";
pub const MAGIC_V3: [u8; 8] = *b"SLIMEG3\0";
pub const MAGIC_V2: [u8; 8] = *b"SLIMEG2\0";
pub const MAGIC: [u8; 8] = MAGIC_V4;
pub const FORMAT_VERSION_V3: u32 = 3;
pub const FORMAT_VERSION_V2: u32 = 2;
include!("generated/generation.rs");

const LEGACY_HEADER_LEN: usize = 256;
const LEGACY_COMPONENT_LEN: usize = 32;
const LEGACY_DEPENDENCY_LEN: usize = 4;
const MAX_COMPONENTS_V2: usize = 32;
const MAX_TASK_CAPS: usize = 64;

pub const KIND_KERNEL: u32 = 1;
pub const KIND_BOOTSTRAP: u32 = 2;
pub const KIND_COMPONENT: u32 = 3;
pub const KIND_RESOURCE: u32 = 4;
pub const ROLE_INIT: u32 = 1;
pub type Rights = u64;
pub const RIGHT_TRANSFER: Rights = 1 << 2;
pub const RIGHT_EXEC: Rights = 1 << 3;
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
    BadBinding,
    BadOwner,
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
pub struct Executable<'a> {
    pub name: &'a str,
    pub object: usize,
    pub role: u32,
    pub spawn_budget: u16,
}

/// Retained only for decoding v2/v3 rollback generations.
#[derive(Debug, Clone, Copy)]
pub struct LegacyComponent<'a> {
    pub name: &'a str,
    pub object: usize,
    pub role: u32,
    pub spawn_budget: u16,
    dependency_start: usize,
    dependency_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceOwner {
    Root,
    Instance(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceHealth {
    Optional,
    Required,
}

#[derive(Debug, Clone, Copy)]
pub struct Instance<'a> {
    pub name: &'a str,
    pub executable: usize,
    pub owner: InstanceOwner,
    pub autostart: bool,
    pub health: InstanceHealth,
    dependency_start: usize,
    dependency_count: usize,
    binding_start: usize,
    binding_count: usize,
}

impl Instance<'_> {
    pub const fn dependency_count(self) -> usize {
        self.dependency_count
    }
    pub const fn binding_count(self) -> usize {
        self.binding_count
    }
    pub const fn is_root_autostart(self) -> bool {
        matches!(self.owner, InstanceOwner::Root) && self.autostart
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceBinding {
    pub grant: usize,
    pub slot: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantEndpoint {
    Executable(usize),
    Instance(usize),
}

#[derive(Debug, Clone, Copy)]
pub struct Grant<'a> {
    pub name: &'a str,
    pub source: GrantEndpoint,
    pub target: GrantEndpoint,
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
    pub version: u32,
    pub identity: [u8; 32],
    pub number: u64,
    pub parent: Option<[u8; 32]>,
    pub target: &'a str,
    /// Only meaningful for retained v2/v3 generations.
    pub kernel_object: usize,
    /// v4 instance index; for v2/v3 this is the legacy bootstrap component index.
    pub bootstrap_instance: usize,
    pub boot_attempts: u32,
    object_count: usize,
    executable_count: usize,
    instance_count: usize,
    dependency_count: usize,
    binding_count: usize,
    grant_count: usize,
    state_count: usize,
    health_count: usize,
    object_offset: usize,
    executable_offset: usize,
    instance_offset: usize,
    dependency_offset: usize,
    binding_offset: usize,
    grant_offset: usize,
    state_offset: usize,
    health_offset: usize,
    string_offset: usize,
    string_len: usize,
}

impl<'a> Generation<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, DecodeError> {
        if bytes.len() < HEADER_LEN {
            return Err(DecodeError::Truncated);
        }
        let magic: [u8; 8] = bytes[..8].try_into().unwrap();
        let encoded_version = u32_at(bytes, 8)?;
        let version = match (magic, encoded_version) {
            (MAGIC_V4, FORMAT_VERSION) => FORMAT_VERSION,
            (MAGIC_V3, FORMAT_VERSION_V3) => FORMAT_VERSION_V3,
            (MAGIC_V2, FORMAT_VERSION_V2) => FORMAT_VERSION_V2,
            (MAGIC_V4, _) | (MAGIC_V3, _) | (MAGIC_V2, _) => {
                return Err(DecodeError::UnsupportedVersion);
            }
            _ => return Err(DecodeError::BadMagic),
        };
        if u32_at(bytes, 12)? as usize != HEADER_LEN || HEADER_LEN != LEGACY_HEADER_LEN {
            return Err(DecodeError::BadHeader);
        }
        if u64_at(bytes, 16)? != 0 {
            return Err(DecodeError::UnknownRequiredFlags);
        }
        let reserved_start = if version == FORMAT_VERSION { 240 } else { 216 };
        if bytes[reserved_start..HEADER_LEN]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(DecodeError::NonZeroReserved);
        }
        let total_len_offset = if version == FORMAT_VERSION { 232 } else { 208 };
        let total_len = u64_at(bytes, total_len_offset)? as usize;
        if total_len != bytes.len() || total_len > MAX_GENERATION_BYTES {
            return Err(DecodeError::BadBounds);
        }
        let identity: [u8; 32] = bytes[IDENTITY_OFFSET..IDENTITY_END].try_into().unwrap();
        if generation_identity(bytes) != identity {
            return Err(DecodeError::BadIdentity);
        }
        let parent_bytes: [u8; 32] = bytes[64..96].try_into().unwrap();

        let (
            kernel_object,
            bootstrap_instance,
            object_count,
            executable_count,
            instance_count,
            dependency_count,
            binding_count,
            grant_count,
            state_count,
            health_count,
            object_offset,
            executable_offset,
            instance_offset,
            dependency_offset,
            binding_offset,
            grant_offset,
            state_offset,
            health_offset,
            string_offset,
            string_len,
            payload_offset,
        ) = if version == FORMAT_VERSION {
            if u32_at(bytes, 100)? != 0 {
                return Err(DecodeError::NonZeroReserved);
            }
            (
                usize::MAX,
                u32_at(bytes, 104)? as usize,
                bounded_count(u32_at(bytes, 112)? as usize, 1, MAX_OBJECTS)?,
                bounded_count(u32_at(bytes, 116)? as usize, 1, MAX_EXECUTABLES)?,
                bounded_count(u32_at(bytes, 120)? as usize, 1, MAX_INSTANCES)?,
                bounded_count(u32_at(bytes, 124)? as usize, 0, MAX_DEPENDENCIES)?,
                bounded_count(u32_at(bytes, 128)? as usize, 0, MAX_BINDINGS)?,
                bounded_count(u32_at(bytes, 132)? as usize, 0, MAX_GRANTS)?,
                bounded_count(u32_at(bytes, 136)? as usize, 0, MAX_STATES)?,
                bounded_count(u32_at(bytes, 140)? as usize, 0, MAX_HEALTH_INSTANCES)?,
                u64_at(bytes, 144)? as usize,
                u64_at(bytes, 152)? as usize,
                u64_at(bytes, 160)? as usize,
                u64_at(bytes, 168)? as usize,
                u64_at(bytes, 176)? as usize,
                u64_at(bytes, 184)? as usize,
                u64_at(bytes, 192)? as usize,
                u64_at(bytes, 200)? as usize,
                u64_at(bytes, 208)? as usize,
                u64_at(bytes, 216)? as usize,
                u64_at(bytes, 224)? as usize,
            )
        } else {
            let component_limit = if version == FORMAT_VERSION_V2 {
                MAX_COMPONENTS_V2
            } else {
                48
            };
            (
                u32_at(bytes, 100)? as usize,
                u32_at(bytes, 104)? as usize,
                bounded_count(u32_at(bytes, 112)? as usize, 1, MAX_OBJECTS)?,
                bounded_count(u32_at(bytes, 116)? as usize, 1, component_limit)?,
                0,
                bounded_count(u32_at(bytes, 120)? as usize, 0, MAX_DEPENDENCIES)?,
                0,
                bounded_count(u32_at(bytes, 124)? as usize, 0, MAX_GRANTS)?,
                bounded_count(u32_at(bytes, 128)? as usize, 0, MAX_STATES)?,
                bounded_count(u32_at(bytes, 132)? as usize, 0, 32)?,
                u64_at(bytes, 136)? as usize,
                u64_at(bytes, 144)? as usize,
                0,
                u64_at(bytes, 152)? as usize,
                0,
                u64_at(bytes, 160)? as usize,
                u64_at(bytes, 168)? as usize,
                u64_at(bytes, 176)? as usize,
                u64_at(bytes, 184)? as usize,
                u64_at(bytes, 192)? as usize,
                u64_at(bytes, 200)? as usize,
            )
        };
        if string_len > MAX_STRING_TABLE_BYTES {
            return Err(DecodeError::BadBounds);
        }
        if version == FORMAT_VERSION {
            check_section(object_offset, object_count, OBJECT_LEN, executable_offset)?;
            check_section(
                executable_offset,
                executable_count,
                EXECUTABLE_LEN,
                instance_offset,
            )?;
            check_section(
                instance_offset,
                instance_count,
                INSTANCE_LEN,
                dependency_offset,
            )?;
            check_section(
                dependency_offset,
                dependency_count,
                DEPENDENCY_LEN,
                binding_offset,
            )?;
            check_section(binding_offset, binding_count, BINDING_LEN, grant_offset)?;
        } else {
            check_section(object_offset, object_count, OBJECT_LEN, executable_offset)?;
            check_section(
                executable_offset,
                executable_count,
                LEGACY_COMPONENT_LEN,
                dependency_offset,
            )?;
            check_section(
                dependency_offset,
                dependency_count,
                LEGACY_DEPENDENCY_LEN,
                grant_offset,
            )?;
        }
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
            kernel_object,
            bootstrap_instance,
            boot_attempts: u32_at(bytes, 108)?,
            object_count,
            executable_count,
            instance_count,
            dependency_count,
            binding_count,
            grant_count,
            state_count,
            health_count,
            object_offset,
            executable_offset,
            instance_offset,
            dependency_offset,
            binding_offset,
            grant_offset,
            state_offset,
            health_offset,
            string_offset,
            string_len,
        };
        generation.validate(payload_offset)?;
        Ok(generation)
    }

    pub const fn is_v4(&self) -> bool {
        self.version == FORMAT_VERSION
    }
    pub const fn object_count(&self) -> usize {
        self.object_count
    }
    pub const fn executable_count(&self) -> usize {
        self.executable_count
    }
    pub const fn instance_count(&self) -> usize {
        self.instance_count
    }
    pub const fn grant_count(&self) -> usize {
        self.grant_count
    }
    pub const fn state_count(&self) -> usize {
        self.state_count
    }
    pub const fn health_count(&self) -> usize {
        self.health_count
    }
    pub const fn bootstrap(&self) -> usize {
        self.bootstrap_instance
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
        let digest = self.bytes[offset + 24..offset + 56].try_into().unwrap();
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

    pub fn executable(&self, index: usize) -> Result<Executable<'a>, DecodeError> {
        if !self.is_v4() || index >= self.executable_count {
            return Err(DecodeError::BadIndex);
        }
        let offset = self.executable_offset + index * EXECUTABLE_LEN;
        if self.bytes[offset + 16..offset + EXECUTABLE_LEN]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(DecodeError::NonZeroReserved);
        }
        Ok(Executable {
            name: self.string(u32_at(self.bytes, offset)? as usize)?,
            object: u32_at(self.bytes, offset + 4)? as usize,
            role: u32_at(self.bytes, offset + 8)?,
            spawn_budget: u32_at(self.bytes, offset + 12)?
                .try_into()
                .map_err(|_| DecodeError::BadBounds)?,
        })
    }

    pub fn executable_named(&self, name: &str) -> Option<Executable<'a>> {
        (0..self.executable_count).find_map(|i| self.executable(i).ok().filter(|e| e.name == name))
    }
    pub fn executable_bytes(&self, name: &str) -> Option<&'a [u8]> {
        let executable = self.executable_named(name)?;
        self.object(executable.object)
            .ok()
            .map(|object| object.bytes)
    }

    pub fn instance(&self, index: usize) -> Result<Instance<'a>, DecodeError> {
        if !self.is_v4() || index >= self.instance_count {
            return Err(DecodeError::BadIndex);
        }
        let offset = self.instance_offset + index * INSTANCE_LEN;
        if self.bytes[offset + 40..offset + INSTANCE_LEN]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(DecodeError::NonZeroReserved);
        }
        let owner = match u32_at(self.bytes, offset + 8)? {
            0 => {
                if u32_at(self.bytes, offset + 12)? != 0 {
                    return Err(DecodeError::BadOwner);
                }
                InstanceOwner::Root
            }
            1 => InstanceOwner::Instance(u32_at(self.bytes, offset + 12)? as usize),
            _ => return Err(DecodeError::UnknownEnum),
        };
        Ok(Instance {
            name: self.string(u32_at(self.bytes, offset)? as usize)?,
            executable: u32_at(self.bytes, offset + 4)? as usize,
            owner,
            autostart: bool_at(self.bytes, offset + 16)?,
            dependency_start: u32_at(self.bytes, offset + 20)? as usize,
            dependency_count: u32_at(self.bytes, offset + 24)? as usize,
            binding_start: u32_at(self.bytes, offset + 28)? as usize,
            binding_count: u32_at(self.bytes, offset + 32)? as usize,
            health: match u32_at(self.bytes, offset + 36)? {
                0 => InstanceHealth::Optional,
                1 => InstanceHealth::Required,
                _ => return Err(DecodeError::UnknownEnum),
            },
        })
    }
    pub fn instance_named(&self, name: &str) -> Option<Instance<'a>> {
        (0..self.instance_count).find_map(|i| self.instance(i).ok().filter(|v| v.name == name))
    }
    pub fn dependency(
        &self,
        instance: Instance<'a>,
        index: usize,
    ) -> Result<Instance<'a>, DecodeError> {
        if index >= instance.dependency_count {
            return Err(DecodeError::BadIndex);
        }
        let absolute = instance
            .dependency_start
            .checked_add(index)
            .ok_or(DecodeError::BadIndex)?;
        if absolute >= self.dependency_count {
            return Err(DecodeError::BadIndex);
        }
        self.instance(u32_at(
            self.bytes,
            self.dependency_offset + absolute * DEPENDENCY_LEN,
        )? as usize)
    }
    pub fn binding(
        &self,
        instance: Instance<'a>,
        index: usize,
    ) -> Result<InstanceBinding, DecodeError> {
        if index >= instance.binding_count {
            return Err(DecodeError::BadIndex);
        }
        let absolute = instance
            .binding_start
            .checked_add(index)
            .ok_or(DecodeError::BadIndex)?;
        if absolute >= self.binding_count {
            return Err(DecodeError::BadIndex);
        }
        let offset = self.binding_offset + absolute * BINDING_LEN;
        Ok(InstanceBinding {
            grant: u32_at(self.bytes, offset)? as usize,
            slot: u32_at(self.bytes, offset + 4)? as usize,
        })
    }

    /// Retained v2/v3 accessor; v4 never synthesizes instances from this table.
    pub fn component(&self, index: usize) -> Result<LegacyComponent<'a>, DecodeError> {
        if self.is_v4() || index >= self.executable_count {
            return Err(DecodeError::BadIndex);
        }
        let offset = self.executable_offset + index * LEGACY_COMPONENT_LEN;
        Ok(LegacyComponent {
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
    pub const fn component_count(&self) -> usize {
        if self.version == FORMAT_VERSION {
            0
        } else {
            self.executable_count
        }
    }
    pub fn legacy_dependency(
        &self,
        component: LegacyComponent<'a>,
        index: usize,
    ) -> Result<LegacyComponent<'a>, DecodeError> {
        if index >= component.dependency_count {
            return Err(DecodeError::BadIndex);
        }
        let absolute = component
            .dependency_start
            .checked_add(index)
            .ok_or(DecodeError::BadIndex)?;
        self.component(u32_at(
            self.bytes,
            self.dependency_offset + absolute * LEGACY_DEPENDENCY_LEN,
        )? as usize)
    }

    pub fn grant(&self, index: usize) -> Result<Grant<'a>, DecodeError> {
        if index >= self.grant_count {
            return Err(DecodeError::BadIndex);
        }
        let offset = self.grant_offset + index * GRANT_LEN;
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
        let source = u32_at(self.bytes, offset + 4)? as usize;
        let target = u32_at(self.bytes, offset + 8)? as usize;
        Ok(Grant {
            name: self.string(u32_at(self.bytes, offset)? as usize)?,
            source: GrantEndpoint::Instance(source),
            target: if self.is_v4() && rights & RIGHT_EXEC != 0 {
                GrantEndpoint::Executable(target)
            } else {
                GrantEndpoint::Instance(target)
            },
            rights,
            transferable: bool_at(self.bytes, transferable_offset)?,
        })
    }
    pub fn grant_named(&self, name: &str) -> Option<Grant<'a>> {
        (0..self.grant_count).find_map(|i| self.grant(i).ok().filter(|g| g.name == name))
    }
    pub fn authority_manifest_identity(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"slime-authority-manifest-v1");
        for index in 0..self.grant_count {
            let grant = self.grant(index).expect("validated grant");
            update_bounded_string(&mut hasher, grant.name);
            let endpoint_name = |endpoint| match endpoint {
                GrantEndpoint::Executable(i) => {
                    self.executable(i).expect("validated executable").name
                }
                GrantEndpoint::Instance(i) if self.is_v4() => {
                    self.instance(i).expect("validated instance").name
                }
                GrantEndpoint::Instance(i) => {
                    self.component(i).expect("validated legacy component").name
                }
            };
            update_bounded_string(&mut hasher, endpoint_name(grant.source));
            update_bounded_string(&mut hasher, endpoint_name(grant.target));
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
    pub fn health_instance(&self, index: usize) -> Result<Instance<'a>, DecodeError> {
        if !self.is_v4() || index >= self.health_count {
            return Err(DecodeError::BadIndex);
        }
        self.instance(u32_at(self.bytes, self.health_offset + index * HEALTH_LEN)? as usize)
    }

    fn string(&self, offset: usize) -> Result<&'a str, DecodeError> {
        read_string(self.bytes, self.string_offset, self.string_len, offset)
    }

    fn validate(&self, payload_offset: usize) -> Result<(), DecodeError> {
        let mut previous_id = None;
        let mut previous_payload_end = payload_offset;
        for index in 0..self.object_count {
            let object = self.object(index)?;
            if !matches!(
                object.kind,
                KIND_KERNEL | KIND_BOOTSTRAP | KIND_COMPONENT | KIND_RESOURCE
            ) {
                return Err(DecodeError::UnknownEnum);
            }
            if previous_id.is_some_and(|p| p >= object.id) {
                return Err(DecodeError::BadOrder);
            }
            let start = u64_at(self.bytes, self.object_offset + index * OBJECT_LEN + 8)? as usize;
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
        if self.boot_attempts == 0 {
            return Err(DecodeError::BadHealth);
        }
        if self.is_v4() {
            self.validate_v4()
        } else {
            self.validate_legacy()
        }
    }

    fn validate_v4(&self) -> Result<(), DecodeError> {
        let mut previous_name = None;
        for index in 0..self.executable_count {
            let executable = self.executable(index)?;
            if executable.object >= self.object_count
                || !matches!(executable.role, 1..=4)
                || executable.spawn_budget > MAX_SPAWN_BUDGET
            {
                return Err(DecodeError::BadIndex);
            }
            if previous_name.is_some_and(|p| p >= executable.name) {
                return Err(DecodeError::BadOrder);
            }
            previous_name = Some(executable.name);
        }
        previous_name = None;
        let mut expected_dependency_start = 0;
        let mut expected_binding_start = 0;
        for index in 0..self.instance_count {
            let instance = self.instance(index)?;
            if instance.executable >= self.executable_count {
                return Err(DecodeError::BadIndex);
            }
            if previous_name.is_some_and(|p| p >= instance.name) {
                return Err(DecodeError::BadOrder);
            }
            if matches!(instance.owner, InstanceOwner::Instance(owner) if owner >= self.instance_count || owner == index)
            {
                return Err(DecodeError::BadOwner);
            }
            if instance.dependency_start != expected_dependency_start
                || instance.binding_start != expected_binding_start
                || instance
                    .dependency_start
                    .checked_add(instance.dependency_count)
                    .is_none_or(|end| end > self.dependency_count)
                || instance
                    .binding_start
                    .checked_add(instance.binding_count)
                    .is_none_or(|end| end > self.binding_count)
            {
                return Err(DecodeError::BadBounds);
            }
            expected_dependency_start += instance.dependency_count;
            expected_binding_start += instance.binding_count;
            if instance.health == InstanceHealth::Required
                && matches!(instance.owner, InstanceOwner::Instance(owner) if self.instance(owner)?.health != InstanceHealth::Required)
            {
                return Err(DecodeError::BadHealth);
            }
            let mut previous_dependency = None;
            for at in 0..instance.dependency_count {
                let dependency = self.dependency(instance, at)?;
                let dependency_index = self
                    .instance_index(dependency.name)
                    .ok_or(DecodeError::BadDependency)?;
                if dependency_index == index
                    || previous_dependency.is_some_and(|p| p >= dependency_index)
                {
                    return Err(DecodeError::BadDependency);
                }
                if instance.is_root_autostart() && !dependency.is_root_autostart() {
                    return Err(DecodeError::BadDependency);
                }
                if instance.health == InstanceHealth::Required
                    && dependency.health != InstanceHealth::Required
                {
                    return Err(DecodeError::BadHealth);
                }
                previous_dependency = Some(dependency_index);
            }
            let mut previous_slot = None;
            for at in 0..instance.binding_count {
                let binding = self.binding(instance, at)?;
                if binding.grant >= self.grant_count
                    || binding.slot >= MAX_TASK_CAPS
                    || previous_slot.is_some_and(|p| p >= binding.slot)
                {
                    return Err(DecodeError::BadBinding);
                }
                if !self.grant_applies_to_instance(self.grant(binding.grant)?, index) {
                    return Err(DecodeError::BadBinding);
                }
                previous_slot = Some(binding.slot);
            }
            for grant_index in 0..self.grant_count {
                if self.grant_requires_instance_binding(self.grant(grant_index)?, index) {
                    let mut found = 0;
                    for at in 0..instance.binding_count {
                        found += usize::from(self.binding(instance, at)?.grant == grant_index);
                    }
                    if found != 1 {
                        return Err(DecodeError::BadBinding);
                    }
                }
            }
            previous_name = Some(instance.name);
        }
        validate_acyclic(
            self.instance_count,
            |node, edge| {
                let instance = self.instance(node)?;
                if edge >= instance.dependency_count {
                    return Ok(None);
                }
                let dependency = self.dependency(instance, edge)?;
                self.instance_index(dependency.name)
                    .map(Some)
                    .ok_or(DecodeError::BadDependency)
            },
            DecodeError::BadDependency,
        )?;
        validate_acyclic(
            self.instance_count,
            |node, edge| {
                if edge != 0 {
                    return Ok(None);
                }
                Ok(match self.instance(node)?.owner {
                    InstanceOwner::Root => None,
                    InstanceOwner::Instance(owner) => Some(owner),
                })
            },
            DecodeError::BadOwner,
        )?;
        for index in 0..self.instance_count {
            let instance = self.instance(index)?;
            if instance.autostart
                && let InstanceOwner::Instance(owner) = instance.owner
                && !self.instance(owner)?.autostart
            {
                return Err(DecodeError::BadOwner);
            }
        }
        if expected_dependency_start != self.dependency_count
            || expected_binding_start != self.binding_count
        {
            return Err(DecodeError::BadBounds);
        }
        if self.bootstrap_instance >= self.instance_count {
            return Err(DecodeError::BadBootstrap);
        }
        let bootstrap = self.instance(self.bootstrap_instance)?;
        let executable = self.executable(bootstrap.executable)?;
        if !bootstrap.is_root_autostart()
            || executable.role != ROLE_INIT
            || self.object(executable.object)?.kind != KIND_BOOTSTRAP
        {
            return Err(DecodeError::BadBootstrap);
        }
        self.validate_grants_states_health()
    }

    fn validate_legacy(&self) -> Result<(), DecodeError> {
        if self.kernel_object >= self.object_count
            || self.bootstrap_instance >= self.executable_count
        {
            return Err(DecodeError::BadIndex);
        }
        if self.object(self.kernel_object)?.kind != KIND_KERNEL {
            return Err(DecodeError::BadKernel);
        }
        let mut previous_name = None;
        for index in 0..self.executable_count {
            let component = self.component(index)?;
            if component.object >= self.object_count
                || !matches!(component.role, 1..=4)
                || component.spawn_budget > MAX_SPAWN_BUDGET
            {
                return Err(DecodeError::BadIndex);
            }
            if previous_name.is_some_and(|p| p >= component.name) {
                return Err(DecodeError::BadOrder);
            }
            if component
                .dependency_start
                .checked_add(component.dependency_count)
                .is_none_or(|end| end > self.dependency_count)
            {
                return Err(DecodeError::BadDependency);
            }
            previous_name = Some(component.name);
        }
        let bootstrap = self.component(self.bootstrap_instance)?;
        if bootstrap.role != ROLE_INIT || self.object(bootstrap.object)?.kind != KIND_BOOTSTRAP {
            return Err(DecodeError::BadBootstrap);
        }
        self.validate_grants_states_health()
    }

    fn validate_grants_states_health(&self) -> Result<(), DecodeError> {
        let rights_mask = if self.version == FORMAT_VERSION_V2 {
            RIGHT_ALL_V2
        } else {
            RIGHT_ALL
        };
        let mut previous_grant = None;
        for index in 0..self.grant_count {
            let grant = self.grant(index)?;
            let valid_endpoint = |endpoint| match endpoint {
                GrantEndpoint::Executable(i) => self.is_v4() && i < self.executable_count,
                GrantEndpoint::Instance(i) => {
                    i < if self.is_v4() {
                        self.instance_count
                    } else {
                        self.executable_count
                    }
                }
            };
            if !valid_endpoint(grant.source)
                || !valid_endpoint(grant.target)
                || !matches!(grant.source, GrantEndpoint::Instance(_))
                || grant.rights == 0
                || grant.rights & !rights_mask != 0
                || (grant.rights & RIGHT_TRANSFER != 0) != grant.transferable
            {
                return Err(DecodeError::BadIndex);
            }
            let key = (
                grant.name,
                endpoint_key(grant.source),
                endpoint_key(grant.target),
            );
            if previous_grant.is_some_and(|p| p >= key) {
                return Err(DecodeError::BadOrder);
            }
            previous_grant = Some(key);
        }
        let owner_limit = if self.is_v4() {
            self.instance_count
        } else {
            self.executable_count
        };
        let mut previous_state = None;
        for index in 0..self.state_count {
            let state = self.state(index)?;
            if state.owner >= owner_limit
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
            if previous_state.is_some_and(|p| p >= state.name) {
                return Err(DecodeError::BadOrder);
            }
            previous_state = Some(state.name);
        }
        if self.is_v4() {
            let mut previous_health = None;
            let mut required = 0;
            for index in 0..self.instance_count {
                required += usize::from(self.instance(index)?.health == InstanceHealth::Required);
            }
            if required != self.health_count {
                return Err(DecodeError::BadHealth);
            }
            for index in 0..self.health_count {
                let instance = self.health_instance(index)?;
                let instance_index = self
                    .instance_index(instance.name)
                    .ok_or(DecodeError::BadHealth)?;
                if instance.health != InstanceHealth::Required
                    || previous_health.is_some_and(|p| p >= instance_index)
                {
                    return Err(DecodeError::BadHealth);
                }
                previous_health = Some(instance_index);
            }
        }
        Ok(())
    }

    fn grant_requires_instance_binding(&self, grant: Grant<'_>, instance: usize) -> bool {
        if grant.rights & RIGHT_EXEC != 0 {
            grant.source == GrantEndpoint::Instance(instance)
        } else if grant.rights & 0b11 != 0 {
            grant.source == GrantEndpoint::Instance(instance)
                || grant.target == GrantEndpoint::Instance(instance)
        } else {
            grant.target == GrantEndpoint::Instance(instance)
        }
    }

    pub fn grant_applies_to_instance(&self, grant: Grant<'_>, instance: usize) -> bool {
        if self.grant_requires_instance_binding(grant, instance) {
            return true;
        }
        let Ok(record) = self.instance(instance) else {
            return false;
        };
        let child_copy = match record.owner {
            InstanceOwner::Root => false,
            InstanceOwner::Instance(owner) => {
                grant.source == GrantEndpoint::Instance(owner)
                    && if grant.rights & RIGHT_EXEC != 0 {
                        (0..self.instance_count).any(|child| {
                            self.instance(child).is_ok_and(|child_record| {
                                child_record.owner == InstanceOwner::Instance(instance)
                                    && grant.target
                                        == GrantEndpoint::Executable(child_record.executable)
                            })
                        })
                    } else if grant.rights & 0b11 == 0 {
                        grant.target == GrantEndpoint::Instance(instance)
                            || grant.target == GrantEndpoint::Instance(owner)
                    } else {
                        grant.target == GrantEndpoint::Instance(instance)
                    }
            }
        };
        child_copy
            || (0..self.instance_count).any(|child| {
                let Ok(child_record) = self.instance(child) else {
                    return false;
                };
                child_record.owner == InstanceOwner::Instance(instance)
                    && grant.source == GrantEndpoint::Instance(instance)
                    && if grant.rights & RIGHT_EXEC != 0 {
                        grant.target == GrantEndpoint::Executable(child_record.executable)
                    } else {
                        grant.target == GrantEndpoint::Instance(child)
                    }
            })
    }
    fn instance_index(&self, name: &str) -> Option<usize> {
        (0..self.instance_count).find(|i| self.instance(*i).is_ok_and(|v| v.name == name))
    }
}

fn validate_acyclic<F>(
    node_count: usize,
    mut edge: F,
    cycle_error: DecodeError,
) -> Result<(), DecodeError>
where
    F: FnMut(usize, usize) -> Result<Option<usize>, DecodeError>,
{
    let mut colors = [0u8; MAX_INSTANCES];
    let mut nodes = [0usize; MAX_INSTANCES];
    let mut edges = [0usize; MAX_INSTANCES];
    for root in 0..node_count {
        if colors[root] != 0 {
            continue;
        }
        let mut depth = 1;
        nodes[0] = root;
        edges[0] = 0;
        colors[root] = 1;
        while depth != 0 {
            let frame = depth - 1;
            let node = nodes[frame];
            let edge_index = edges[frame];
            match edge(node, edge_index)? {
                Some(next) => {
                    if next >= node_count {
                        return Err(cycle_error);
                    }
                    edges[frame] += 1;
                    match colors[next] {
                        0 => {
                            if depth >= node_count {
                                return Err(cycle_error);
                            }
                            nodes[depth] = next;
                            edges[depth] = 0;
                            colors[next] = 1;
                            depth += 1;
                        }
                        1 => return Err(cycle_error),
                        _ => {}
                    }
                }
                None => {
                    colors[node] = 2;
                    depth -= 1;
                }
            }
        }
    }
    Ok(())
}

fn endpoint_key(endpoint: GrantEndpoint) -> (u8, usize) {
    match endpoint {
        GrantEndpoint::Instance(i) => (0, i),
        GrantEndpoint::Executable(i) => (1, i),
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
fn bool_at(bytes: &[u8], offset: usize) -> Result<bool, DecodeError> {
    match u32_at(bytes, offset)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(DecodeError::UnknownEnum),
    }
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

    fn seal(bytes: &mut [u8]) {
        bytes[IDENTITY_OFFSET..IDENTITY_END].fill(0);
        let identity = generation_identity(bytes);
        bytes[IDENTITY_OFFSET..IDENTITY_END].copy_from_slice(&identity);
    }

    fn minimal() -> alloc::vec::Vec<u8> {
        let strings = b"\x01\0t\x04\0boot\x06\0worker\x04\0init";
        let object_offset = HEADER_LEN;
        let executable_offset = object_offset + OBJECT_LEN;
        let instance_offset = executable_offset + 2 * EXECUTABLE_LEN;
        let dependency_offset = instance_offset + INSTANCE_LEN;
        let binding_offset = dependency_offset;
        let grant_offset = binding_offset;
        let state_offset = grant_offset;
        let health_offset = state_offset;
        let string_offset = health_offset + HEALTH_LEN;
        let payload_offset = string_offset + strings.len();
        let total_len = payload_offset + 1;
        let mut bytes = alloc::vec![0; total_len];
        bytes[..8].copy_from_slice(&MAGIC_V4);
        bytes[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(HEADER_LEN as u32).to_le_bytes());
        bytes[56..64].copy_from_slice(&1u64.to_le_bytes());
        bytes[96..100].copy_from_slice(&0u32.to_le_bytes());
        bytes[104..108].copy_from_slice(&0u32.to_le_bytes());
        bytes[108..112].copy_from_slice(&1u32.to_le_bytes());
        for (offset, value) in [(112, 1u32), (116, 2), (120, 1), (140, 1)] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (offset, value) in [
            (144, object_offset),
            (152, executable_offset),
            (160, instance_offset),
            (168, dependency_offset),
            (176, binding_offset),
            (184, grant_offset),
            (192, state_offset),
            (200, health_offset),
            (208, string_offset),
            (216, strings.len()),
            (224, payload_offset),
            (232, total_len),
        ] {
            bytes[offset..offset + 8].copy_from_slice(&(value as u64).to_le_bytes());
        }
        let mut hasher = Sha256::new();
        hasher.update(b"I");
        bytes[object_offset..object_offset + 4].copy_from_slice(&3u32.to_le_bytes());
        bytes[object_offset + 4..object_offset + 8].copy_from_slice(&KIND_BOOTSTRAP.to_le_bytes());
        bytes[object_offset + 8..object_offset + 16]
            .copy_from_slice(&(payload_offset as u64).to_le_bytes());
        bytes[object_offset + 16..object_offset + 24].copy_from_slice(&1u64.to_le_bytes());
        for (index, (name, role)) in [(17u32, ROLE_INIT), (9u32, 2)].into_iter().enumerate() {
            let offset = executable_offset + index * EXECUTABLE_LEN;
            bytes[offset..offset + 4].copy_from_slice(&name.to_le_bytes());
            bytes[offset + 8..offset + 12].copy_from_slice(&role.to_le_bytes());
        }
        bytes[instance_offset..instance_offset + 4].copy_from_slice(&17u32.to_le_bytes());
        bytes[instance_offset + 16..instance_offset + 20].copy_from_slice(&1u32.to_le_bytes());
        bytes[instance_offset + 36..instance_offset + 40].copy_from_slice(&1u32.to_le_bytes());
        bytes[string_offset..payload_offset].copy_from_slice(strings);
        bytes[payload_offset] = b'I';
        let mut hasher = Sha256::new();
        hasher.update(&bytes[payload_offset..payload_offset + 1]);
        bytes[object_offset + 24..object_offset + 56].copy_from_slice(&hasher.finalize());
        seal(&mut bytes);
        bytes
    }

    #[test]
    fn v4_accepts_catalogue_only_executable() {
        let bytes = minimal();
        let generation = Generation::decode(&bytes).expect("valid v4");
        assert_eq!(generation.executable_count(), 2);
        assert_eq!(generation.instance_count(), 1);
        assert!(generation.executable_named("worker").is_some());
        assert!(generation.instance_named("worker").is_none());
    }

    #[test]
    fn v4_rejects_malformed_owner() {
        let mut bytes = minimal();
        let instance = u64_at(&bytes, 160).unwrap() as usize;
        bytes[instance + 8..instance + 12].copy_from_slice(&1u32.to_le_bytes());
        seal(&mut bytes);
        assert!(matches!(
            Generation::decode(&bytes),
            Err(DecodeError::BadOwner)
        ));
    }
    #[test]
    fn dependency_cycle_reachable_beyond_probe_is_rejected() {
        let edges = [Some(1usize), Some(2), Some(1)];
        assert_eq!(
            validate_acyclic(
                edges.len(),
                |node, edge| Ok((edge == 0).then_some(edges[node]).flatten()),
                DecodeError::BadDependency,
            ),
            Err(DecodeError::BadDependency)
        );
    }

    #[test]
    fn owner_cycle_reachable_beyond_probe_is_rejected() {
        let owners = [Some(1usize), Some(2), Some(1)];
        assert_eq!(
            validate_acyclic(
                owners.len(),
                |node, edge| Ok((edge == 0).then_some(owners[node]).flatten()),
                DecodeError::BadOwner,
            ),
            Err(DecodeError::BadOwner)
        );
    }
}
