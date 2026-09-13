use core::str;

#[cfg(feature = "release-crypto")]
use crate::generation::Generation;
#[cfg(feature = "release-crypto")]
use ed25519_dalek::{Signature, VerifyingKey};

pub const RELEASE_MAGIC: [u8; 8] = *b"SLIMERL\0";
include!("generated/release.rs");

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
    pub boot_bundle: [u8; 32],
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
    WrongBootBundle,
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
        let version = read_u32(bytes, RELEASE_HEADER_FORMAT_VERSION_OFFSET);
        if version != RELEASE_VERSION
            || read_u32(bytes, RELEASE_HEADER_HEADER_SIZE_OFFSET) as usize != RELEASE_HEADER_BYTES
        {
            return Err(ReleaseError::UnsupportedVersion);
        }
        if read_u64(bytes, RELEASE_HEADER_REQUIRED_FLAGS_OFFSET) != 0 {
            return Err(ReleaseError::UnknownRequiredFlags);
        }
        let target_len = read_u32(bytes, RELEASE_HEADER_TARGET_LEN_OFFSET) as usize;
        if target_len == 0 || target_len > MAX_TARGET_BYTES {
            return Err(ReleaseError::BadBounds);
        }
        let target_bytes =
            &bytes[RELEASE_HEADER_TARGET_OFFSET..RELEASE_HEADER_TARGET_OFFSET + target_len];
        let target = str::from_utf8(target_bytes).map_err(|_| ReleaseError::BadTarget)?;
        if bytes
            [RELEASE_HEADER_TARGET_OFFSET + target_len..RELEASE_HEADER_BOOT_BUNDLE_IDENTITY_OFFSET]
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(ReleaseError::NonZeroReserved);
        }
        let signature_count = read_u32(bytes, RELEASE_HEADER_SIGNATURE_COUNT_OFFSET) as usize;
        if signature_count > MAX_RELEASE_SIGNATURES
            || bytes[RELEASE_HEADER_RESERVED_OFFSET..RELEASE_HEADER_BYTES]
                .iter()
                .any(|byte| *byte != 0)
            || bytes[RELEASE_HEADER_BYTES + signature_count * RELEASE_SIGNATURE_BYTES..]
                .iter()
                .any(|byte| *byte != 0)
        {
            return Err(ReleaseError::NonZeroReserved);
        }
        let parent: [u8; 32] = bytes
            [RELEASE_HEADER_PARENT_IDENTITY_OFFSET..RELEASE_HEADER_RELEASE_SEQUENCE_OFFSET]
            .try_into()
            .unwrap();
        let boot_bundle: [u8; 32] = bytes
            [RELEASE_HEADER_BOOT_BUNDLE_IDENTITY_OFFSET..RELEASE_HEADER_AUTHORITY_MANIFEST_OFFSET]
            .try_into()
            .unwrap();
        if version == RELEASE_VERSION && boot_bundle == [0; 32] {
            return Err(ReleaseError::WrongBootBundle);
        }
        Ok(Self {
            bytes,
            generation: bytes
                [RELEASE_HEADER_GENERATION_IDENTITY_OFFSET..RELEASE_HEADER_PARENT_IDENTITY_OFFSET]
                .try_into()
                .unwrap(),
            parent: (parent != [0; 32]).then_some(parent),
            sequence: read_u64(bytes, RELEASE_HEADER_RELEASE_SEQUENCE_OFFSET),
            target,
            trust_root_version: read_u32(bytes, RELEASE_HEADER_TRUST_ROOT_VERSION_OFFSET),
            boot_bundle,
            authority_manifest: bytes
                [RELEASE_HEADER_AUTHORITY_MANIFEST_OFFSET..RELEASE_HEADER_SIGNATURE_COUNT_OFFSET]
                .try_into()
                .unwrap(),
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
        if !generation.is_v5() {
            return Err(ReleaseError::WrongBootBundle);
        }
        if self.authority_manifest != generation.authority_manifest_identity() {
            return Err(ReleaseError::WrongAuthorityManifest);
        }
        self.verify_signatures(root)
    }

    #[cfg(feature = "release-crypto")]
    pub fn verify_boot_bundle(&self, expected: &[u8; 32]) -> Result<(), ReleaseError> {
        if self.boot_bundle != *expected {
            return Err(ReleaseError::WrongBootBundle);
        }
        Ok(())
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
        || read_u32(bytes, ROTATION_HEADER_FORMAT_VERSION_OFFSET) != ROTATION_VERSION
        || read_u32(bytes, ROTATION_HEADER_HEADER_SIZE_OFFSET) as usize != ROTATION_HEADER_BYTES
        || read_u64(bytes, ROTATION_HEADER_REQUIRED_FLAGS_OFFSET) != 0
    {
        return Err(ReleaseError::BadRotation);
    }
    let previous_version = read_u32(bytes, ROTATION_HEADER_PREVIOUS_VERSION_OFFSET);
    let replacement_version = read_u32(bytes, ROTATION_HEADER_REPLACEMENT_VERSION_OFFSET);
    let replacement_threshold = read_u32(bytes, ROTATION_HEADER_REPLACEMENT_THRESHOLD_OFFSET);
    let replacement_key_count = read_u32(bytes, ROTATION_HEADER_REPLACEMENT_KEY_COUNT_OFFSET);
    let previous_signature_count =
        read_u32(bytes, ROTATION_HEADER_PREVIOUS_SIGNATURE_COUNT_OFFSET) as usize;
    let replacement_signature_count =
        read_u32(bytes, ROTATION_HEADER_REPLACEMENT_SIGNATURE_COUNT_OFFSET) as usize;
    if previous_version != current.version
        || replacement_version
            != current
                .version
                .checked_add(1)
                .ok_or(ReleaseError::BadRotation)?
        || previous_signature_count > MAX_RELEASE_SIGNATURES
        || replacement_signature_count > MAX_RELEASE_SIGNATURES
        || bytes[ROTATION_HEADER_RESERVED_OFFSET..ROTATION_HEADER_BYTES]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn root(threshold: u32, key_count: u32) -> TrustRoot {
        let mut keys = [[0u8; 32]; MAX_TRUST_KEYS];
        for (index, key) in keys.iter_mut().enumerate().take(key_count as usize) {
            key.fill(index as u8 + 1);
        }
        TrustRoot {
            version: 1,
            threshold,
            key_count,
            keys,
        }
    }

    /// One signed release naming a parent, a target, and a generation digest.
    /// The signature area stays zeroed: `decode` requires the tail past
    /// `signature_count` entries to be zero, and a count of zero means all of
    /// it. Signature *verification* is behind `release-crypto` and is not what
    /// this corpus covers.
    fn valid() -> [u8; RELEASE_BYTES] {
        const TARGET: &[u8] = b"x86_64-qemu-virtio";
        let mut bytes = [0u8; RELEASE_BYTES];
        bytes[..8].copy_from_slice(&RELEASE_MAGIC);
        bytes[8..12].copy_from_slice(&RELEASE_VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(RELEASE_HEADER_BYTES as u32).to_le_bytes());
        bytes[24..56].fill(0xC1);
        bytes[56..88].fill(0xD2);
        bytes[88..96].copy_from_slice(&42u64.to_le_bytes());
        bytes[96..100].copy_from_slice(&(TARGET.len() as u32).to_le_bytes());
        bytes[100..104].copy_from_slice(&1u32.to_le_bytes());
        bytes[104..104 + TARGET.len()].copy_from_slice(TARGET);
        bytes[136..168].fill(0xE3);
        bytes[168..200].fill(0xF4);
        bytes
    }

    /// Every field the decoder promises. Without this the refusal corpus below
    /// could pass on a decoder that refuses everything.
    #[test]
    fn a_well_formed_release_decodes_with_every_field() {
        let bytes = valid();
        let release = Release::decode(&bytes).expect("valid release");
        assert_eq!(release.generation, [0xC1; 32]);
        assert_eq!(release.parent, Some([0xD2; 32]));
        assert_eq!(release.sequence, 42);
        assert_eq!(release.target, "x86_64-qemu-virtio");
        assert_eq!(release.trust_root_version, 1);
        assert_eq!(release.boot_bundle, [0xE3; 32]);
        assert_eq!(release.authority_manifest, [0xF4; 32]);
        assert_eq!(release.signed_payload().len(), RELEASE_HEADER_BYTES);
    }

    /// An all-zero parent is *absent*, not an ancestor whose identity is zero.
    /// The rollback chain reads this to find the first release.
    #[test]
    fn a_zero_parent_decodes_as_absent() {
        let mut bytes = valid();
        bytes[56..88].fill(0);
        assert_eq!(Release::decode(&bytes).expect("valid").parent, None);
    }

    /// The release is a fixed-size record, so anything else is refused on size
    /// alone rather than read with a shifted layout.
    #[test]
    fn any_length_other_than_one_record_is_bad_size() {
        let bytes = valid();
        assert_eq!(
            Release::decode(&bytes[..RELEASE_BYTES - 1]).err(),
            Some(ReleaseError::BadSize)
        );
        let oversized = [0u8; RELEASE_BYTES + 1];
        assert_eq!(
            Release::decode(&oversized).err(),
            Some(ReleaseError::BadSize)
        );
    }

    #[test]
    fn a_foreign_magic_is_refused_before_anything_else() {
        let mut bytes = valid();
        bytes[0] = b'X';
        assert_eq!(Release::decode(&bytes).err(), Some(ReleaseError::BadMagic));
    }

    /// A future format is refused rather than read with this version's offsets,
    /// and so is a header claiming a size this build did not compile.
    #[test]
    fn a_wrong_version_or_header_size_is_unsupported() {
        for (offset, value) in [
            (8usize, RELEASE_VERSION + 1),
            (12, RELEASE_HEADER_BYTES as u32 + 8),
        ] {
            let mut bytes = valid();
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert_eq!(
                Release::decode(&bytes).err(),
                Some(ReleaseError::UnsupportedVersion),
                "offset {offset}",
            );
        }
    }

    #[test]
    fn release_v1_is_rejected_after_v2_cutover() {
        let mut bytes = valid();
        bytes[RELEASE_HEADER_FORMAT_VERSION_OFFSET..RELEASE_HEADER_FORMAT_VERSION_OFFSET + 4]
            .copy_from_slice(&RELEASE_VERSION_V1.to_le_bytes());
        assert_eq!(
            Release::decode(&bytes).err(),
            Some(ReleaseError::UnsupportedVersion)
        );
    }

    /// A target is what binds a release to the hardware it may boot. An empty
    /// one names nothing, and one past the field cannot be held.
    #[test]
    fn an_empty_or_oversized_target_is_out_of_bounds() {
        for len in [0u32, MAX_TARGET_BYTES as u32 + 1] {
            let mut bytes = valid();
            bytes[96..100].copy_from_slice(&len.to_le_bytes());
            assert_eq!(
                Release::decode(&bytes).err(),
                Some(ReleaseError::BadBounds),
                "target_len {len}",
            );
        }
    }

    /// The target is compared as a string against the generation's own, so a
    /// non-UTF-8 target is refused here rather than becoming a comparison that
    /// can never match.
    #[test]
    fn a_non_utf8_target_is_refused() {
        let mut bytes = valid();
        bytes[96..100].copy_from_slice(&2u32.to_le_bytes());
        bytes[104] = 0xFF;
        bytes[105] = 0xFE;
        bytes[106..136].fill(0);
        assert_eq!(Release::decode(&bytes).err(), Some(ReleaseError::BadTarget));
    }

    /// Every reserved region is an extension point, and each is checked: the
    /// slack after the target, the header tail, and the signature area past the
    /// declared count. A producer that means something by those bytes must not
    /// be silently accepted.
    #[test]
    fn a_nonzero_reserved_byte_is_refused_wherever_it_sits() {
        let target_len = "x86_64-qemu-virtio".len();
        for offset in [104 + target_len, 204, RELEASE_HEADER_BYTES] {
            let mut bytes = valid();
            bytes[offset] = 1;
            assert_eq!(
                Release::decode(&bytes).err(),
                Some(ReleaseError::NonZeroReserved),
                "reserved byte at {offset}",
            );
        }
    }

    #[test]
    fn a_nonzero_required_flag_is_refused() {
        let mut bytes = valid();
        bytes[16] = 1;
        assert_eq!(
            Release::decode(&bytes).err(),
            Some(ReleaseError::UnknownRequiredFlags)
        );
    }

    /// More signatures than the record can hold is refused, and the count is
    /// what decides how much of the signature area must be zero.
    #[test]
    fn more_signatures_than_the_record_holds_is_refused() {
        let mut bytes = valid();
        bytes[200..204].copy_from_slice(&(MAX_RELEASE_SIGNATURES as u32 + 1).to_le_bytes());
        assert_eq!(
            Release::decode(&bytes).err(),
            Some(ReleaseError::NonZeroReserved)
        );
    }

    /// A declared signature slot makes that slot's bytes legal, and the slack
    /// after it still is not. This is the pair that shows `signature_count`
    /// actually moves the boundary rather than being ignored.
    #[test]
    fn a_declared_signature_slot_admits_its_own_bytes_only() {
        let mut bytes = valid();
        bytes[200..204].copy_from_slice(&1u32.to_le_bytes());
        bytes[RELEASE_HEADER_BYTES] = 0xAA;
        Release::decode(&bytes).expect("one declared signature admits its bytes");

        bytes[RELEASE_HEADER_BYTES + RELEASE_SIGNATURE_BYTES] = 0xBB;
        assert_eq!(
            Release::decode(&bytes).err(),
            Some(ReleaseError::NonZeroReserved)
        );
    }

    /// A quorum of zero would accept an unsigned release, and one above the key
    /// count could never be met, so both are refused before any signature is
    /// checked.
    #[test]
    fn a_trust_root_with_an_unmeetable_threshold_is_refused() {
        assert_eq!(root(1, 2).validate(), Ok(()));
        assert_eq!(root(2, 2).validate(), Ok(()));
        assert_eq!(root(0, 2).validate(), Err(ReleaseError::BadBounds));
        assert_eq!(root(3, 2).validate(), Err(ReleaseError::BadBounds));
    }

    /// A version of zero is not a version, and a root holding no keys cannot
    /// authorise anything.
    #[test]
    fn a_trust_root_without_a_version_or_keys_is_refused() {
        let mut zero_version = root(1, 2);
        zero_version.version = 0;
        assert_eq!(zero_version.validate(), Err(ReleaseError::BadBounds));

        assert_eq!(root(1, 0).validate(), Err(ReleaseError::BadBounds));

        let mut over = root(1, 2);
        over.key_count = MAX_TRUST_KEYS as u32 + 1;
        assert_eq!(over.validate(), Err(ReleaseError::BadBounds));
    }

    /// A duplicated key would let one signer satisfy a threshold of two, which
    /// is the whole point of a quorum. A zero key is not a key.
    #[test]
    fn a_duplicate_or_zero_trust_key_is_refused() {
        let mut duplicate = root(2, 2);
        duplicate.keys[1] = duplicate.keys[0];
        assert_eq!(duplicate.validate(), Err(ReleaseError::DuplicateKey));

        let mut zeroed = root(2, 2);
        zeroed.keys[1] = [0; 32];
        assert_eq!(zeroed.validate(), Err(ReleaseError::DuplicateKey));
    }

    /// Key slots past `key_count` are reserved, so a key parked there cannot
    /// quietly become live when the count later grows.
    #[test]
    fn a_key_past_the_declared_count_is_reserved_space() {
        let mut trailing = root(1, 2);
        trailing.keys[2].fill(0x99);
        assert_eq!(trailing.validate(), Err(ReleaseError::NonZeroReserved));
    }
}
