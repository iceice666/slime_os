//! Decoding for the generation-authenticated C9.5 recording resource.
//!
//! Declares which instances participate in a recording stream, whether each is
//! claimed *deterministic*, and how many records the stream holds. The
//! nondeterminism classification the deterministic claim is checked against is
//! not here: it is contract data the host builder joins against the generation's
//! own grant table, and `slime-root` re-derives the same verdict from the
//! resource plus that table. Compiling a copy of those three right lists into
//! the image would put a second classification where nothing reads it.
//!
//! Three guards here are load-bearing rather than tidy:
//!
//! * **Bytes are bounded before anything is sized.** [`RecordingPolicy::decode`]
//!   refuses a length outside `HEADER_BYTES..=MAX_BYTES` before it reads a count,
//!   and refuses a declared `record_capacity` above `MAX_RECORD_CAPACITY` before
//!   any reader allocates or maps for it. That is C9.5's fourth required check,
//!   and it is structural: a recorder cannot declare a stream larger than the
//!   format admits, so no reader has to defend against one.
//!
//! * **A stream is exactly one recorder and one replayer.** A stream with two
//!   recorders has two writers for one artifact; a stream with two replayers
//!   compares one recording twice and calls the agreement evidence. Both are
//!   refused at decode, so the pairing a gate observes is the pairing the
//!   generation declared.
//!
//! * **The record length is the fabric trace's own.** `RECORD_BYTES` is asserted
//!   equal to `slime_proto`'s `TRACE_RECORD_LEN` by the components that stream
//!   it; here the ceiling is expressed in whole records so a truncated stream and
//!   a complete one are never the same observation.

use crate::sha256::Sha256;

include!("generated/recording_policy.rs");

pub const MAGIC: [u8; 8] = *b"SLIMERC\0";
pub const MAX_BYTES: usize = HEADER_BYTES + MAX_INSTANCES * ENTRY_BYTES;

/// The largest recording stream this format admits, in bytes.
///
/// What a replayer bounds its own buffer by, and what the gate checks a declared
/// capacity against. Expressed here rather than at each reader so "bound the
/// recorded trace bytes before allocation" has one number.
pub const MAX_STREAM_BYTES: usize = MAX_RECORD_CAPACITY * RECORD_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    UnknownRequiredFlags,
    BadBounds,
    BadOrder,
    UnknownRole,
    UnknownFlags,
    BadCapacity,
    /// A stream declaring two recorders, two replayers, or only one of the two.
    UnpairedStream,
    /// Two entries of one stream declaring different record capacities. One
    /// recording has one length.
    CapacityConflict,
    /// A recorder named a stream grant. Only a replayer receives its stream, so
    /// an exemption on the writing side would excuse authority nothing needs.
    UnexpectedStreamGrant,
}

/// What one instance does with a recording stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Captures its own nondeterminism sources and writes the stream.
    Record,
    /// Consumes a recorded stream and produces the typed outputs two boots are
    /// compared on.
    Replay,
}

impl Role {
    pub const fn id(self) -> u32 {
        match self {
            Self::Record => ROLE_RECORD,
            Self::Replay => ROLE_REPLAY,
        }
    }

    pub const fn from_id(id: u32) -> Option<Self> {
        Some(match id {
            ROLE_RECORD => Self::Record,
            ROLE_REPLAY => Self::Replay,
            _ => return None,
        })
    }
}

/// One instance's declared participation in one recording stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordingEntry {
    pub instance_identity: [u8; 32],
    /// Joins a recorder to its replayer. Declared rather than negotiated: a
    /// replayer that accepted any recording handed to it would be replaying
    /// whatever arrived.
    pub stream_identity: u64,
    /// The generation grant the recording itself travels over, or `None`.
    ///
    /// The declared exception to the determinism check, and it exists because
    /// every authority that carries bytes into a component is an unrecorded
    /// source — `recv` included — so a replayer needs one such authority to
    /// receive the recording it replays. Admission subtracts exactly this
    /// grant's rights and checks everything else unchanged, so the exception is
    /// one named edge rather than a hole in the classification.
    pub stream_grant_identity: Option<u64>,
    pub role: Role,
    /// Whether the generation claims this participation is deterministic. The
    /// claim is refused at admission when the instance also holds an unrecorded
    /// nondeterminism source outside its declared stream grant.
    pub deterministic: bool,
    /// Records this stream holds. Bounded by `MAX_RECORD_CAPACITY`.
    pub record_capacity: usize,
}

impl RecordingEntry {
    /// This entry's stream, in bytes. What a reader sizes a buffer by.
    pub const fn stream_bytes(&self) -> usize {
        self.record_capacity * RECORD_BYTES
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RecordingPolicy<'a> {
    bytes: &'a [u8],
    entry_count: usize,
}

impl<'a> RecordingPolicy<'a> {
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
        let entry_count = u32_at(bytes, 24)? as usize;
        let total_len = u32_at(bytes, 28)? as usize;
        if entry_count > MAX_INSTANCES
            || total_len != HEADER_BYTES + entry_count * ENTRY_BYTES
            || total_len != bytes.len()
        {
            return Err(DecodeError::BadBounds);
        }
        // Sorted by `instance_identity` strictly ascending. Strictly, not
        // `(instance, stream)` ascending, and the difference is load-bearing: a
        // table admitting one instance on two streams would decode, pass stream
        // pairing, and pass admission, while `entry_for` and the root's
        // `RECORDING_SOURCES` answer report only the first row — so the role,
        // capacity, and determinism claim an instance reads back would depend on
        // which stream name happened to hash lower (found by review). One entry
        // per instance is the API's promise, so it is the decoder's rule.
        let mut previous: Option<[u8; 32]> = None;
        for index in 0..entry_count {
            let entry = decode_entry(bytes, index)?;
            if entry.instance_identity == [0; 32] || entry.stream_identity == 0 {
                return Err(DecodeError::BadOrder);
            }
            if let Some(last_instance) = previous
                && entry.instance_identity <= last_instance
            {
                return Err(DecodeError::BadOrder);
            }
            previous = Some(entry.instance_identity);
        }
        let policy = Self { bytes, entry_count };
        policy.check_stream_pairing()?;
        Ok(policy)
    }

    /// Every declared stream carries exactly one recorder and one replayer, and
    /// its two entries agree on the record capacity.
    ///
    /// Checked over the whole table rather than per entry because it is a
    /// property of a *stream*: neither entry alone is wrong, and reading one
    /// without the other is how an unpaired recording gets admitted.
    fn check_stream_pairing(&self) -> Result<(), DecodeError> {
        for index in 0..self.entry_count {
            let entry = self.entry(index).ok_or(DecodeError::Truncated)?;
            // Count from the first entry of each stream only, so a stream is
            // examined once rather than once per participant.
            let first = (0..index)
                .filter_map(|earlier| self.entry(earlier))
                .all(|earlier| earlier.stream_identity != entry.stream_identity);
            if !first {
                continue;
            }
            let mut recorders = 0usize;
            let mut replayers = 0usize;
            for other in (0..self.entry_count).filter_map(|other| self.entry(other)) {
                if other.stream_identity != entry.stream_identity {
                    continue;
                }
                if other.record_capacity != entry.record_capacity {
                    return Err(DecodeError::CapacityConflict);
                }
                match other.role {
                    Role::Record => recorders += 1,
                    Role::Replay => replayers += 1,
                }
            }
            if recorders != 1 || replayers != 1 {
                return Err(DecodeError::UnpairedStream);
            }
        }
        Ok(())
    }

    pub const fn entry_count(&self) -> usize {
        self.entry_count
    }

    pub fn entry(&self, index: usize) -> Option<RecordingEntry> {
        (index < self.entry_count)
            .then(|| decode_entry(self.bytes, index).expect("validated recording entry"))
    }

    /// The exact encoded bytes of entry `index`, for a reader that serves them
    /// on rather than re-encoding a decoded copy.
    pub fn entry_bytes(&self, index: usize) -> Option<&'a [u8]> {
        (index < self.entry_count).then(|| entry_bytes(self.bytes, index).expect("validated entry"))
    }

    /// `instance`'s declared participation, or `None` for an instance this
    /// resource does not name.
    ///
    /// One entry per instance: an instance on two streams would be recording and
    /// replaying at once, which the ascending sort admits but no role means, so
    /// the first match is the only match in a decoded table.
    pub fn entry_for(&self, instance: &[u8; 32]) -> Option<RecordingEntry> {
        (0..self.entry_count)
            .filter_map(|index| self.entry(index))
            .find(|entry| entry.instance_identity == *instance)
    }

    /// Whether the generation claims `instance` is deterministic.
    ///
    /// Absence denies: an instance this resource does not name makes no
    /// determinism claim, which is what every generation before C9.5 did.
    pub fn is_deterministic(&self, instance: &[u8; 32]) -> bool {
        self.entry_for(instance)
            .is_some_and(|entry| entry.deterministic)
    }

    /// The peer of `instance`'s stream: its replayer if it records, its recorder
    /// if it replays.
    pub fn peer_of(&self, instance: &[u8; 32]) -> Option<RecordingEntry> {
        let entry = self.entry_for(instance)?;
        (0..self.entry_count)
            .filter_map(|index| self.entry(index))
            .find(|other| {
                other.stream_identity == entry.stream_identity && other.role != entry.role
            })
    }
}

/// Stable identity of a recording participant. Its own domain tag, so an
/// identity computed for a clock holder, a wait-set waiter, a scheduling
/// subject, or a lifecycle instance can never authenticate here.
pub fn instance_identity(name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"slime-recording-policy-instance-v1");
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.finalize()
}

/// Stable identity of a declared recording stream.
///
/// Its own domain tag for `instance_identity`'s reason: a stream name and an
/// instance name may coincide, and folding them the same way would let one
/// authenticate as the other.
pub fn stream_identity(name: &str) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"slime-recording-policy-stream-v1");
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    u64::from_le_bytes(hasher.finalize()[..8].try_into().unwrap())
}
/// Stable identity of the generation grant a replayer receives its stream over.
///
/// Its own domain tag for [`stream_identity`]'s reason, and the separation
/// matters more here: this identity names a *grant*, and admission subtracts the
/// named grant's rights from a determinism check. A stream name lifted verbatim
/// into this field would exempt whichever grant happened to fold to the same
/// eight bytes.
pub fn stream_grant_identity(name: &str) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(b"slime-recording-policy-stream-grant-v1");
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    u64::from_le_bytes(hasher.finalize()[..8].try_into().unwrap())
}

fn entry_bytes(bytes: &[u8], index: usize) -> Result<&[u8], DecodeError> {
    let offset = HEADER_BYTES + index * ENTRY_BYTES;
    bytes
        .get(offset..offset + ENTRY_BYTES)
        .ok_or(DecodeError::Truncated)
}

fn decode_entry(bytes: &[u8], index: usize) -> Result<RecordingEntry, DecodeError> {
    let entry = entry_bytes(bytes, index)?;
    if entry[60..64].iter().any(|byte| *byte != 0) {
        return Err(DecodeError::UnknownRequiredFlags);
    }
    let role = Role::from_id(u32_at(entry, 48)?).ok_or(DecodeError::UnknownRole)?;
    let flags = u32_at(entry, 52)?;
    if flags & !KNOWN_FLAGS != 0 {
        return Err(DecodeError::UnknownFlags);
    }
    let record_capacity = u32_at(entry, 56)? as usize;
    // Bounded before any reader sizes a buffer, and nonzero because a stream
    // holding no records cannot carry the terminal evidence that distinguishes a
    // complete recording from a truncated one.
    if record_capacity == 0 || record_capacity > MAX_RECORD_CAPACITY {
        return Err(DecodeError::BadCapacity);
    }
    let stream_grant = u64_at(entry, 40)?;
    // Only a replayer may name a stream grant. A recorder *writes* its stream,
    // and writing is neutral, so an exemption there would excuse an authority
    // nothing in the mechanism needs — which is exactly how a declared exception
    // becomes a hole.
    if stream_grant != 0 && role != Role::Replay {
        return Err(DecodeError::UnexpectedStreamGrant);
    }
    Ok(RecordingEntry {
        instance_identity: entry[..32].try_into().unwrap(),
        stream_identity: u64_at(entry, 32)?,
        stream_grant_identity: (stream_grant != 0).then_some(stream_grant),
        role,
        deterministic: flags & FLAG_DETERMINISTIC != 0,
        record_capacity,
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
    use alloc::vec::Vec;

    fn header(entries: usize) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&(entries as u32).to_le_bytes());
        bytes.extend_from_slice(&((HEADER_BYTES + entries * ENTRY_BYTES) as u32).to_le_bytes());
        bytes
    }

    fn entry(
        instance: &str,
        stream: &str,
        role: Role,
        deterministic: bool,
        capacity: u32,
    ) -> Vec<u8> {
        entry_with_grant(instance, stream, None, role, deterministic, capacity)
    }

    fn entry_with_grant(
        instance: &str,
        stream: &str,
        grant: Option<&str>,
        role: Role,
        deterministic: bool,
        capacity: u32,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&instance_identity(instance));
        bytes.extend_from_slice(&stream_identity(stream).to_le_bytes());
        bytes.extend_from_slice(&grant.map_or(0, stream_grant_identity).to_le_bytes());
        bytes.extend_from_slice(&role.id().to_le_bytes());
        bytes.extend_from_slice(&if deterministic { FLAG_DETERMINISTIC } else { 0 }.to_le_bytes());
        bytes.extend_from_slice(&capacity.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 4]);
        bytes
    }

    /// Encode a table, sorting the entries the way the builder does so a test
    /// exercising a semantic guard is not refused for encoding order first.
    fn encode(mut entries: Vec<Vec<u8>>) -> Vec<u8> {
        entries.sort_by(|left, right| left[..32].cmp(&right[..32]));
        let mut bytes = header(entries.len());
        for entry in entries {
            bytes.extend_from_slice(&entry);
        }
        bytes
    }

    fn paired() -> Vec<u8> {
        encode(alloc::vec![
            entry("replay-recorder", "replay-stream", Role::Record, false, 16),
            entry("replay-replayer", "replay-stream", Role::Replay, true, 16),
        ])
    }

    #[test]
    fn a_paired_stream_decodes_with_its_declared_roles() {
        let bytes = paired();
        let policy = RecordingPolicy::decode(&bytes).expect("paired stream decodes");
        assert_eq!(policy.entry_count(), 2);
        let recorder = policy
            .entry_for(&instance_identity("replay-recorder"))
            .expect("recorder is named");
        assert_eq!(recorder.role, Role::Record);
        assert!(!recorder.deterministic);
        assert_eq!(recorder.record_capacity, 16);
        assert_eq!(recorder.stream_bytes(), 16 * RECORD_BYTES);
        let replayer = policy
            .entry_for(&instance_identity("replay-replayer"))
            .expect("replayer is named");
        assert_eq!(replayer.role, Role::Replay);
        assert!(replayer.deterministic);
    }

    #[test]
    fn an_unnamed_instance_makes_no_determinism_claim() {
        let bytes = paired();
        let policy = RecordingPolicy::decode(&bytes).expect("decodes");
        assert!(!policy.is_deterministic(&instance_identity("someone-else")));
        assert!(
            policy
                .entry_for(&instance_identity("someone-else"))
                .is_none()
        );
    }

    #[test]
    fn each_participant_resolves_the_other_end_of_its_stream() {
        let bytes = paired();
        let policy = RecordingPolicy::decode(&bytes).expect("decodes");
        let peer = policy
            .peer_of(&instance_identity("replay-recorder"))
            .expect("recorder has a replayer");
        assert_eq!(peer.instance_identity, instance_identity("replay-replayer"));
        let back = policy
            .peer_of(&instance_identity("replay-replayer"))
            .expect("replayer has a recorder");
        assert_eq!(back.instance_identity, instance_identity("replay-recorder"));
    }

    #[test]
    fn a_capacity_above_the_format_ceiling_is_refused() {
        let bytes = encode(alloc::vec![
            entry(
                "replay-recorder",
                "replay-stream",
                Role::Record,
                false,
                MAX_RECORD_CAPACITY as u32 + 1,
            ),
            entry(
                "replay-replayer",
                "replay-stream",
                Role::Replay,
                true,
                MAX_RECORD_CAPACITY as u32 + 1,
            ),
        ]);
        assert_eq!(
            RecordingPolicy::decode(&bytes).unwrap_err(),
            DecodeError::BadCapacity
        );
    }

    #[test]
    fn a_stream_holding_no_records_is_refused() {
        let bytes = encode(alloc::vec![
            entry("replay-recorder", "replay-stream", Role::Record, false, 0),
            entry("replay-replayer", "replay-stream", Role::Replay, true, 0),
        ]);
        assert_eq!(
            RecordingPolicy::decode(&bytes).unwrap_err(),
            DecodeError::BadCapacity
        );
    }

    #[test]
    fn a_stream_with_no_replayer_is_refused() {
        let bytes = encode(alloc::vec![entry(
            "replay-recorder",
            "replay-stream",
            Role::Record,
            false,
            16
        )]);
        assert_eq!(
            RecordingPolicy::decode(&bytes).unwrap_err(),
            DecodeError::UnpairedStream
        );
    }

    #[test]
    fn a_stream_with_two_replayers_is_refused() {
        let bytes = encode(alloc::vec![
            entry("replay-recorder", "replay-stream", Role::Record, false, 16),
            entry("replay-replayer", "replay-stream", Role::Replay, true, 16),
            entry("replay-observer", "replay-stream", Role::Replay, true, 16),
        ]);
        assert_eq!(
            RecordingPolicy::decode(&bytes).unwrap_err(),
            DecodeError::UnpairedStream
        );
    }

    #[test]
    fn one_streams_two_ends_must_agree_on_its_length() {
        let bytes = encode(alloc::vec![
            entry("replay-recorder", "replay-stream", Role::Record, false, 16),
            entry("replay-replayer", "replay-stream", Role::Replay, true, 8),
        ]);
        assert_eq!(
            RecordingPolicy::decode(&bytes).unwrap_err(),
            DecodeError::CapacityConflict
        );
    }

    #[test]
    fn a_descending_table_is_refused() {
        let mut entries = alloc::vec![
            entry("replay-recorder", "replay-stream", Role::Record, false, 16),
            entry("replay-replayer", "replay-stream", Role::Replay, true, 16),
        ];
        entries.sort_by(|left, right| right[..32].cmp(&left[..32]));
        let mut bytes = header(entries.len());
        for entry in entries {
            bytes.extend_from_slice(&entry);
        }
        assert_eq!(
            RecordingPolicy::decode(&bytes).unwrap_err(),
            DecodeError::BadOrder
        );
    }
    /// One instance may declare one participation, and the rule is the decoder's
    /// because the API's promise is.
    ///
    /// `(instance, stream)` ascending would admit an instance on two streams:
    /// such a table pairs, admits, and then reports only its first row through
    /// `entry_for` and `RECORDING_SOURCES`, so the role and determinism claim an
    /// instance reads back would depend on which stream name hashed lower (found
    /// by review).
    #[test]
    fn one_instance_cannot_declare_two_participations() {
        let bytes = encode(alloc::vec![
            entry("replay-recorder", "stream-a", Role::Record, false, 16),
            entry("replay-replayer", "stream-a", Role::Replay, true, 16),
            // The same instance again, on a second fully paired stream.
            entry("replay-replayer", "stream-b", Role::Record, false, 16),
            entry("replay-observer", "stream-b", Role::Replay, false, 16),
        ]);
        assert_eq!(
            RecordingPolicy::decode(&bytes).unwrap_err(),
            DecodeError::BadOrder
        );
    }

    /// A replayer's declared stream grant decodes, and only a replayer may name
    /// one: a recorder *writes* its stream, so an exemption there would excuse
    /// authority the mechanism does not need.
    #[test]
    fn only_a_replayer_may_name_a_stream_grant() {
        let bytes = encode(alloc::vec![
            entry("replay-recorder", "replay-stream", Role::Record, false, 16),
            entry_with_grant(
                "replay-replayer",
                "replay-stream",
                Some("replay-stream-channel"),
                Role::Replay,
                true,
                16,
            ),
        ]);
        let policy = RecordingPolicy::decode(&bytes).expect("a declared grant decodes");
        let replayer = policy
            .entry_for(&instance_identity("replay-replayer"))
            .expect("named");
        assert_eq!(
            replayer.stream_grant_identity,
            Some(stream_grant_identity("replay-stream-channel"))
        );
        let recorder = policy
            .entry_for(&instance_identity("replay-recorder"))
            .expect("named");
        assert_eq!(recorder.stream_grant_identity, None);

        let refused = encode(alloc::vec![
            entry_with_grant(
                "replay-recorder",
                "replay-stream",
                Some("replay-stream-channel"),
                Role::Record,
                false,
                16,
            ),
            entry("replay-replayer", "replay-stream", Role::Replay, true, 16),
        ]);
        assert_eq!(
            RecordingPolicy::decode(&refused).unwrap_err(),
            DecodeError::UnexpectedStreamGrant
        );
    }

    /// The grant fold is its own domain, so a stream name cannot be lifted into
    /// the field whose rights admission subtracts.
    #[test]
    fn the_stream_grant_fold_is_its_own_domain() {
        assert_ne!(
            stream_grant_identity("replay-stream"),
            stream_identity("replay-stream")
        );
    }

    #[test]
    fn an_unknown_role_is_refused_rather_than_ignored() {
        let mut bytes = paired();
        let role_offset = HEADER_BYTES + 48;
        bytes[role_offset..role_offset + 4].copy_from_slice(&(MAX_ROLE + 1).to_le_bytes());
        assert_eq!(
            RecordingPolicy::decode(&bytes).unwrap_err(),
            DecodeError::UnknownRole
        );
    }

    #[test]
    fn an_undeclared_entry_flag_is_refused() {
        let mut bytes = paired();
        let flags_offset = HEADER_BYTES + 52;
        bytes[flags_offset..flags_offset + 4].copy_from_slice(&(KNOWN_FLAGS + 1).to_le_bytes());
        assert_eq!(
            RecordingPolicy::decode(&bytes).unwrap_err(),
            DecodeError::UnknownFlags
        );
    }
    /// Both folds are pinned against the exact bytes
    /// `build-generation.py`'s `recording_instance_identity` and
    /// `recording_stream_identity` produce for these names.
    ///
    /// Two readers of one identity is how a resource comes to authenticate on
    /// one side and not the other, and the failure mode is silent: a component
    /// the generation names would simply read back as unnamed. Pinning the
    /// literal bytes makes a divergence a host-test failure rather than a plane
    /// that boots with every determinism claim quietly absent.
    #[test]
    fn the_identity_folds_match_the_host_builders() {
        assert_eq!(
            instance_identity("replay-recorder"),
            [
                89, 17, 245, 108, 72, 18, 224, 62, 60, 245, 222, 198, 60, 221, 148, 254, 101, 22,
                75, 225, 30, 122, 192, 245, 171, 232, 228, 251, 208, 54, 138, 239
            ]
        );
        assert_eq!(stream_identity("replay-stream"), 0xa9a1_db25_03f1_b043);
    }

    /// The domain tags separate the two folds and separate this resource from
    /// its four C9 siblings.
    #[test]
    fn an_identity_from_a_sibling_resource_does_not_authenticate_here() {
        assert_ne!(
            instance_identity("replay-recorder"),
            crate::lifecycle_policy::instance_identity("replay-recorder")
        );
        assert_ne!(
            instance_identity("replay-recorder"),
            crate::wait_set::waiter_identity("replay-recorder")
        );
        assert_ne!(
            instance_identity("replay-recorder"),
            crate::scheduling_class::instance_identity("replay-recorder")
        );
        // The stream fold is not the instance fold truncated: a stream and an
        // instance may share a name, and folding both the same way would let one
        // be lifted into the other's field.
        assert_ne!(
            stream_identity("replay-recorder"),
            u64::from_le_bytes(
                instance_identity("replay-recorder")[..8]
                    .try_into()
                    .unwrap()
            )
        );
    }

    #[test]
    fn a_truncated_stream_is_refused_rather_than_partially_read() {
        let full = paired();
        for length in 0..full.len() {
            let error = RecordingPolicy::decode(&full[..length]).expect_err("refuses short bytes");
            assert!(
                matches!(error, DecodeError::Truncated | DecodeError::BadBounds),
                "length {length} gave {error:?}"
            );
        }
    }

    #[test]
    fn the_stream_ceiling_is_expressed_in_whole_records() {
        assert_eq!(MAX_STREAM_BYTES, MAX_RECORD_CAPACITY * RECORD_BYTES);
        assert_eq!(MAX_STREAM_BYTES % RECORD_BYTES, 0);
    }
}
