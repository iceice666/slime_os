use core::str;

#[cfg(feature = "release-crypto")]
use crate::generation::Generation;
#[cfg(feature = "release-crypto")]
use ed25519_dalek::{Signature, VerifyingKey};

pub const RELEASE_MAGIC: [u8; 8] = *b"SLIMERL\0";
include!("generated/release.rs");

pub const ROTATION_MAGIC: [u8; 8] = *b"SLIMERT\0";
pub const ROTATION_VERSION: u32 = 1;
pub const ROTATION_BYTES: usize = 1024;
pub const ROTATION_HEADER_BYTES: usize = 64;
pub const MAX_TRUST_KEYS: usize = 4;
pub const SIGN_NAMESPACE: &[u8] = b"slime-release";

pub const INITIAL_TRUST_ROOT: TrustRoot = TrustRoot {
    version: 1,
    threshold: 2,
    key_count: 3,
    keys: [
        hex32(*b"4b2b337e3762e1867c6c004f534156b6cae1eeec17bcb74a03b187bb0a053cbe"),
        hex32(*b"3f8ad44d5423e1443113b4d71a576e62293387d011808a3706d743b89df2b0ce"),
        hex32(*b"af5f0d3a5f47127874aab49d1c53508ddcacde17f25358afd32588a50e0d3934"),
        [0; 32],
    ],
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustRoot {
    pub version: u32,
    pub threshold: u32,
    pub key_count: u32,
    pub keys: [[u8; 32]; MAX_TRUST_KEYS],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Release<'a> {
    bytes: &'a [u8; RELEASE_BYTES],
    pub generation: [u8; 32],
    pub parent: Option<[u8; 32]>,
    pub sequence: u64,
    pub target: &'a str,
    pub trust_root_version: u32,
    pub kernel: [u8; 32],
    pub authority_manifest: [u8; 32],
    signature_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseError {
    BadSize,
    BadMagic,
    UnsupportedVersion,
    UnknownRequiredFlags,
    BadBounds,
    BadTarget,
    NonZeroReserved,
    WrongGeneration,
    WrongParent,
    WrongTarget,
    WrongKernel,
    WrongAuthorityManifest,
    WrongTrustRoot,
    StaleSequence,
    MissingSignatures,
    DuplicateKey,
    UnknownKey,
    BadSignature,
    BadRotation,
}

impl TrustRoot {
    pub fn validate(&self) -> Result<(), ReleaseError> {
        let count = self.key_count as usize;
        if self.version == 0
            || count == 0
            || count > MAX_TRUST_KEYS
            || self.threshold == 0
            || self.threshold > self.key_count
        {
            return Err(ReleaseError::BadBounds);
        }
        for index in 0..count {
            if self.keys[index] == [0; 32] || self.keys[..index].contains(&self.keys[index]) {
                return Err(ReleaseError::DuplicateKey);
            }
        }
        if self.keys[count..].iter().any(|key| *key != [0; 32]) {
            return Err(ReleaseError::NonZeroReserved);
        }
        Ok(())
    }
}

impl<'a> Release<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, ReleaseError> {
        let bytes: &'a [u8; RELEASE_BYTES] = bytes.try_into().map_err(|_| ReleaseError::BadSize)?;
        if bytes[..8] != RELEASE_MAGIC {
            return Err(ReleaseError::BadMagic);
        }
        if read_u32(bytes, 8) != RELEASE_VERSION
            || read_u32(bytes, 12) as usize != RELEASE_HEADER_BYTES
        {
            return Err(ReleaseError::UnsupportedVersion);
        }
        if read_u64(bytes, 16) != 0 {
            return Err(ReleaseError::UnknownRequiredFlags);
        }
        let target_len = read_u32(bytes, 96) as usize;
        if target_len == 0 || target_len > MAX_TARGET_BYTES {
            return Err(ReleaseError::BadBounds);
        }
        let target_bytes = &bytes[104..104 + target_len];
        let target = str::from_utf8(target_bytes).map_err(|_| ReleaseError::BadTarget)?;
        if bytes[104 + target_len..136].iter().any(|byte| *byte != 0) {
            return Err(ReleaseError::NonZeroReserved);
        }
        let signature_count = read_u32(bytes, 200) as usize;
        if signature_count > MAX_RELEASE_SIGNATURES
            || bytes[204..RELEASE_HEADER_BYTES]
                .iter()
                .any(|byte| *byte != 0)
            || bytes[RELEASE_HEADER_BYTES + signature_count * RELEASE_SIGNATURE_BYTES..]
                .iter()
                .any(|byte| *byte != 0)
        {
            return Err(ReleaseError::NonZeroReserved);
        }
        let parent: [u8; 32] = bytes[56..88].try_into().unwrap();
        Ok(Self {
            bytes,
            generation: bytes[24..56].try_into().unwrap(),
            parent: (parent != [0; 32]).then_some(parent),
            sequence: read_u64(bytes, 88),
            target,
            trust_root_version: read_u32(bytes, 100),
            kernel: bytes[136..168].try_into().unwrap(),
            authority_manifest: bytes[168..200].try_into().unwrap(),
            signature_count,
        })
    }

    pub fn signed_payload(&self) -> &[u8] {
        &self.bytes[..RELEASE_HEADER_BYTES]
    }

    #[cfg(feature = "release-crypto")]
    pub fn verify_generation(
        &self,
        generation: &Generation<'_>,
        root: &TrustRoot,
    ) -> Result<(), ReleaseError> {
        if self.generation != generation.identity {
            return Err(ReleaseError::WrongGeneration);
        }
        if self.parent != generation.parent {
            return Err(ReleaseError::WrongParent);
        }
        if self.target != generation.target {
            return Err(ReleaseError::WrongTarget);
        }
        let kernel = generation
            .object(generation.kernel_object)
            .map_err(|_| ReleaseError::WrongKernel)?;
        if self.kernel != kernel.digest {
            return Err(ReleaseError::WrongKernel);
        }
        if self.authority_manifest != generation.authority_manifest_identity() {
            return Err(ReleaseError::WrongAuthorityManifest);
        }
        self.verify_signatures(root)
    }

    #[cfg(feature = "release-crypto")]
    pub fn verify_for_staging(
        &self,
        generation: &Generation<'_>,
        root: &TrustRoot,
        accepted_sequence: u64,
    ) -> Result<(), ReleaseError> {
        self.verify_generation(generation, root)?;
        if self.sequence <= accepted_sequence {
            return Err(ReleaseError::StaleSequence);
        }
        Ok(())
    }

    #[cfg(feature = "release-crypto")]
    pub fn verify_signatures(&self, root: &TrustRoot) -> Result<(), ReleaseError> {
        root.validate()?;
        if self.trust_root_version != root.version {
            return Err(ReleaseError::WrongTrustRoot);
        }
        if self.signature_count < root.threshold as usize {
            return Err(ReleaseError::MissingSignatures);
        }
        let signed = ssh_signed_payload(self.signed_payload());
        verify_signature_entries(
            &signed,
            &self.bytes[RELEASE_HEADER_BYTES..],
            self.signature_count,
            root,
        )
    }
}

#[cfg(feature = "release-crypto")]
pub fn apply_rotation(current: &TrustRoot, bytes: &[u8]) -> Result<TrustRoot, ReleaseError> {
    current.validate()?;
    let bytes: &[u8; ROTATION_BYTES] = bytes.try_into().map_err(|_| ReleaseError::BadSize)?;
    if bytes[..8] != ROTATION_MAGIC
        || read_u32(bytes, 8) != ROTATION_VERSION
        || read_u32(bytes, 12) as usize != ROTATION_HEADER_BYTES
        || read_u64(bytes, 16) != 0
    {
        return Err(ReleaseError::BadRotation);
    }
    let previous_version = read_u32(bytes, 24);
    let replacement_version = read_u32(bytes, 28);
    let replacement_threshold = read_u32(bytes, 32);
    let replacement_key_count = read_u32(bytes, 36);
    let previous_signature_count = read_u32(bytes, 40) as usize;
    let replacement_signature_count = read_u32(bytes, 44) as usize;
    if previous_version != current.version
        || replacement_version
            != current
                .version
                .checked_add(1)
                .ok_or(ReleaseError::BadRotation)?
        || previous_signature_count > MAX_RELEASE_SIGNATURES
        || replacement_signature_count > MAX_RELEASE_SIGNATURES
        || bytes[48..ROTATION_HEADER_BYTES]
            .iter()
            .any(|byte| *byte != 0)
    {
        return Err(ReleaseError::BadRotation);
    }
    let mut replacement = TrustRoot {
        version: replacement_version,
        threshold: replacement_threshold,
        key_count: replacement_key_count,
        keys: [[0; 32]; MAX_TRUST_KEYS],
    };
    for index in 0..MAX_TRUST_KEYS {
        let offset = ROTATION_HEADER_BYTES + index * 32;
        replacement.keys[index].copy_from_slice(&bytes[offset..offset + 32]);
    }
    replacement.validate()?;
    let previous_offset = ROTATION_HEADER_BYTES + MAX_TRUST_KEYS * 32;
    let replacement_offset = previous_offset + MAX_RELEASE_SIGNATURES * RELEASE_SIGNATURE_BYTES;
    if bytes
        [previous_offset + previous_signature_count * RELEASE_SIGNATURE_BYTES..replacement_offset]
        .iter()
        .any(|byte| *byte != 0)
        || bytes[replacement_offset + replacement_signature_count * RELEASE_SIGNATURE_BYTES..]
            .iter()
            .any(|byte| *byte != 0)
    {
        return Err(ReleaseError::NonZeroReserved);
    }
    let signed = ssh_signed_payload(&bytes[..previous_offset]);
    verify_signature_entries(
        &signed,
        &bytes[previous_offset..replacement_offset],
        previous_signature_count,
        current,
    )?;
    verify_signature_entries(
        &signed,
        &bytes[replacement_offset..],
        replacement_signature_count,
        &replacement,
    )?;

    Ok(replacement)
}

#[cfg(feature = "release-crypto")]
pub fn verify_ed25519(
    public_key: &[u8; 32],
    payload: &[u8],
    signature: &[u8; 64],
) -> Result<(), ReleaseError> {
    let key = VerifyingKey::from_bytes(public_key).map_err(|_| ReleaseError::BadSignature)?;
    let signature = Signature::from_bytes(signature);
    key.verify_strict(payload, &signature)
        .map_err(|_| ReleaseError::BadSignature)
}

#[cfg(feature = "release-crypto")]
fn verify_signature_entries(
    payload: &[u8],
    entries: &[u8],
    count: usize,
    root: &TrustRoot,
) -> Result<(), ReleaseError> {
    if count < root.threshold as usize {
        return Err(ReleaseError::MissingSignatures);
    }
    let mut previous = [0; 32];
    for index in 0..count {
        let offset = index * RELEASE_SIGNATURE_BYTES;
        let key_id: [u8; 32] = entries[offset..offset + 32].try_into().unwrap();
        if index > 0 && key_id <= previous {
            return Err(ReleaseError::DuplicateKey);
        }
        previous = key_id;
        let key = root.keys[..root.key_count as usize]
            .iter()
            .find(|key| crate::sha256::digest(key.as_slice()) == key_id)
            .ok_or(ReleaseError::UnknownKey)?;
        let signature: [u8; 64] = entries[offset + 32..offset + RELEASE_SIGNATURE_BYTES]
            .try_into()
            .map_err(|_| ReleaseError::BadSignature)?;
        verify_ed25519(key, payload, &signature)?;
    }
    Ok(())
}
#[cfg(feature = "release-crypto")]
fn ssh_signed_payload(payload: &[u8]) -> [u8; 73] {
    let mut signed = [0u8; 73];
    let mut offset = 0;
    signed[offset..offset + 6].copy_from_slice(b"SSHSIG");
    offset += 6;
    offset = write_ssh_string(&mut signed, offset, SIGN_NAMESPACE);
    offset = write_ssh_string(&mut signed, offset, &[]);
    offset = write_ssh_string(&mut signed, offset, b"sha256");
    let hash = crate::sha256::digest(payload);
    offset = write_ssh_string(&mut signed, offset, &hash);
    debug_assert_eq!(offset, signed.len());
    signed
}

#[cfg(feature = "release-crypto")]
fn write_ssh_string(output: &mut [u8], offset: usize, value: &[u8]) -> usize {
    let end = offset + 4 + value.len();
    output[offset..offset + 4].copy_from_slice(&(value.len() as u32).to_be_bytes());
    output[offset + 4..end].copy_from_slice(value);
    end
}

const fn hex32(hex: [u8; 64]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut index = 0;
    while index < 32 {
        out[index] = (nibble(hex[index * 2]) << 4) | nibble(hex[index * 2 + 1]);
        index += 1;
    }
    out
}

const fn nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => 0,
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
