use crate::sha256::{Sha256, digest};

pub const MAGIC: [u8; 8] = *b"SLIMEBS\0";
include!("generated/bootstate.rs");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BootState {
    pub sequence: u64,
    pub known_good: [u8; 32],
    pub pending: Option<[u8; 32]>,
    pub remaining_attempts: u32,
    pub generation_root: [u8; 32],
    pub state_root: [u8; 32],
    pub accepted_release_sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootStateError {
    BadMagic,
    UnsupportedVersion,
    BadHeaderSize,
    UnknownRequiredFlags,
    MaxSequence,
    ZeroKnownGood,
    ZeroGenerationRoot,
    BadPendingAttempts,
    NonZeroReserved,
    BadChecksum,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootTransitionError {
    NoPending,
    AttemptsExhausted,
    WrongRunningGeneration,
    StaleRelease,
    SequenceExhausted,
}

impl BootState {
    pub fn encode(self) -> Result<[u8; SLOT_BYTES], BootStateError> {
        validate(&self)?;
        let mut out = [0u8; SLOT_BYTES];
        out[..8].copy_from_slice(&MAGIC);
        out[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        out[12..16].copy_from_slice(&(SLOT_BYTES as u32).to_le_bytes());
        out[16..24].copy_from_slice(&REQUIRED_FLAGS.to_le_bytes());
        out[24..32].copy_from_slice(&self.sequence.to_le_bytes());
        out[32..64].copy_from_slice(&self.known_good);
        if let Some(pending) = self.pending {
            out[64..96].copy_from_slice(&pending);
        }
        out[96..100].copy_from_slice(&self.remaining_attempts.to_le_bytes());
        out[104..136].copy_from_slice(&self.generation_root);
        out[136..168].copy_from_slice(&self.state_root);
        out[RELEASE_SEQUENCE_OFFSET..CHECKSUM_OFFSET]
            .copy_from_slice(&self.accepted_release_sequence.to_le_bytes());
        let checksum = slot_checksum(&out);
        out[CHECKSUM_OFFSET..CHECKSUM_END].copy_from_slice(&checksum);
        Ok(out)
    }

    pub fn decode(bytes: &[u8; SLOT_BYTES]) -> Result<Self, BootStateError> {
        if bytes[..8] != MAGIC {
            return Err(BootStateError::BadMagic);
        }
        if read_u32(bytes, 8) != FORMAT_VERSION {
            return Err(BootStateError::UnsupportedVersion);
        }
        if read_u32(bytes, 12) as usize != SLOT_BYTES {
            return Err(BootStateError::BadHeaderSize);
        }
        if read_u64(bytes, 16) != REQUIRED_FLAGS {
            return Err(BootStateError::UnknownRequiredFlags);
        }
        if bytes[100..104].iter().any(|byte| *byte != 0)
            || bytes[CHECKSUM_END..].iter().any(|byte| *byte != 0)
        {
            return Err(BootStateError::NonZeroReserved);
        }
        let expected: [u8; 32] = bytes[CHECKSUM_OFFSET..CHECKSUM_END].try_into().unwrap();
        if slot_checksum(bytes) != expected {
            return Err(BootStateError::BadChecksum);
        }
        let pending_bytes: [u8; 32] = bytes[64..96].try_into().unwrap();
        let state = Self {
            sequence: read_u64(bytes, 24),
            known_good: bytes[32..64].try_into().unwrap(),
            pending: (pending_bytes != [0; 32]).then_some(pending_bytes),
            remaining_attempts: read_u32(bytes, 96),
            generation_root: bytes[104..136].try_into().unwrap(),
            state_root: bytes[136..168].try_into().unwrap(),
            accepted_release_sequence: read_u64(bytes, RELEASE_SEQUENCE_OFFSET),
        };
        validate(&state)?;
        Ok(state)
    }

    pub fn stage_pending(
        self,
        pending: [u8; 32],
        attempts: u32,
        generation_root: [u8; 32],
        state_root: [u8; 32],
    ) -> Result<Self, BootTransitionError> {
        if attempts == 0 {
            return Err(BootTransitionError::AttemptsExhausted);
        }
        Ok(Self {
            sequence: next_sequence(self.sequence)?,
            known_good: self.known_good,
            pending: Some(pending),
            remaining_attempts: attempts,
            generation_root,
            state_root,
            accepted_release_sequence: self.accepted_release_sequence,
        })
    }

    pub fn consume_pending_attempt(self) -> Result<Self, BootTransitionError> {
        if self.pending.is_none() {
            return Err(BootTransitionError::NoPending);
        }
        if self.remaining_attempts == 0 {
            return Err(BootTransitionError::AttemptsExhausted);
        }
        Ok(Self {
            sequence: next_sequence(self.sequence)?,
            remaining_attempts: self.remaining_attempts - 1,
            ..self
        })
    }

    pub fn promote_pending(
        self,
        running: [u8; 32],
        release_sequence: u64,
    ) -> Result<Self, BootTransitionError> {
        if self.pending != Some(running) {
            return Err(BootTransitionError::WrongRunningGeneration);
        }
        if release_sequence <= self.accepted_release_sequence {
            return Err(BootTransitionError::StaleRelease);
        }
        Ok(Self {
            sequence: next_sequence(self.sequence)?,
            known_good: running,
            pending: None,
            remaining_attempts: 0,
            accepted_release_sequence: release_sequence,
            ..self
        })
    }

    pub fn rollback_pending(self) -> Result<Self, BootTransitionError> {
        if self.pending.is_none() {
            return Ok(self);
        }
        Ok(Self {
            sequence: next_sequence(self.sequence)?,
            pending: None,
            remaining_attempts: 0,
            ..self
        })
    }
}

/// Which of the two fixed slots a record came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    /// The slot a commit must write.
    ///
    /// Older-slot-first is the whole redundancy argument: the selected root
    /// stays intact until the new one is durable, so an interruption at any
    /// point leaves at least one valid slot.
    pub fn other(self) -> Slot {
        match self {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedBootState {
    pub slot: Slot,
    pub state: BootState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionError {
    /// Both slots decode but disagree at the same sequence. Choosing either
    /// would be a guess about which commit won.
    ConflictingSlots,
    /// Neither slot decodes. There is no root to boot.
    NoValidBootState,
}

/// Select the live root from two slots.
///
/// The highest sequence wins; one damaged slot is tolerated, because that is
/// what the second slot is for. Two valid slots at the same sequence that are
/// not byte-identical is a hard reject rather than a coin flip.
///
/// Lives here rather than in `stage0` because it is not stage-0's rule: the
/// generation-management component applies the same one, and `stage0` pulls in
/// `uefi`, so a component could not have reached it there.
pub fn select_bootstate(
    a: &[u8; SLOT_BYTES],
    b: &[u8; SLOT_BYTES],
) -> Result<SelectedBootState, SelectionError> {
    match (BootState::decode(a), BootState::decode(b)) {
        (Ok(a), Ok(b)) if a.sequence > b.sequence => Ok(SelectedBootState {
            slot: Slot::A,
            state: a,
        }),
        (Ok(a), Ok(b)) if b.sequence > a.sequence => Ok(SelectedBootState {
            slot: Slot::B,
            state: b,
        }),
        (Ok(a), Ok(b)) if a == b => Ok(SelectedBootState {
            slot: Slot::A,
            state: a,
        }),
        (Ok(_), Ok(_)) => Err(SelectionError::ConflictingSlots),
        (Ok(a), Err(_)) => Ok(SelectedBootState {
            slot: Slot::A,
            state: a,
        }),
        (Err(_), Ok(b)) => Ok(SelectedBootState {
            slot: Slot::B,
            state: b,
        }),
        (Err(_), Err(_)) => Err(SelectionError::NoValidBootState),
    }
}

fn next_sequence(sequence: u64) -> Result<u64, BootTransitionError> {
    sequence
        .checked_add(1)
        .filter(|sequence| *sequence != u64::MAX)
        .ok_or(BootTransitionError::SequenceExhausted)
}

pub fn slot_checksum(bytes: &[u8; SLOT_BYTES]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(&bytes[..CHECKSUM_OFFSET]);
    hasher.update(&[0u8; 32]);
    hasher.update(&bytes[CHECKSUM_END..]);
    hasher.finalize()
}

pub fn empty_state_root() -> [u8; 32] {
    digest(&[])
}

fn validate(state: &BootState) -> Result<(), BootStateError> {
    if state.sequence == u64::MAX {
        return Err(BootStateError::MaxSequence);
    }
    if state.known_good == [0; 32] {
        return Err(BootStateError::ZeroKnownGood);
    }
    if state.generation_root == [0; 32] {
        return Err(BootStateError::ZeroGenerationRoot);
    }
    match (state.pending, state.remaining_attempts) {
        (None, 0) | (Some(_), _) => Ok(()),
        _ => Err(BootStateError::BadPendingAttempts),
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

    const G1: [u8; 32] = [1; 32];
    const G2: [u8; 32] = [2; 32];
    const GENERATION_ROOT: [u8; 32] = [3; 32];

    fn state(pending: Option<[u8; 32]>, remaining_attempts: u32) -> BootState {
        BootState {
            sequence: 1,
            known_good: G1,
            pending,
            remaining_attempts,
            generation_root: GENERATION_ROOT,
            state_root: empty_state_root(),
            accepted_release_sequence: 1,
        }
    }

    #[test]
    fn exhausted_pending_round_trips() {
        let expected = state(Some(G2), 0);
        let encoded = expected.encode().unwrap();

        assert_eq!(BootState::decode(&encoded), Ok(expected));
    }

    #[test]
    fn attempts_without_pending_are_rejected() {
        assert_eq!(
            state(None, 1).encode(),
            Err(BootStateError::BadPendingAttempts)
        );
    }

    #[test]
    fn pending_attempt_is_consumed_before_selection() {
        let pending = state(Some(G2), 2).consume_pending_attempt().unwrap();

        assert_eq!(pending.sequence, 2);
        assert_eq!(pending.pending, Some(G2));
        assert_eq!(pending.remaining_attempts, 1);
        assert_eq!(pending.known_good, G1);
    }

    #[test]
    fn promotion_requires_the_running_pending_generation() {
        let pending = state(Some(G2), 1);

        assert_eq!(
            pending.promote_pending(G1, 2),
            Err(BootTransitionError::WrongRunningGeneration)
        );
        let promoted = pending.promote_pending(G2, 2).unwrap();
        assert_eq!(promoted.known_good, G2);
        assert_eq!(promoted.pending, None);
        assert_eq!(promoted.remaining_attempts, 0);
        assert_eq!(promoted.accepted_release_sequence, 2);
        assert_eq!(
            pending.promote_pending(G2, 1),
            Err(BootTransitionError::StaleRelease)
        );
    }

    #[test]
    fn rollback_is_idempotent_after_pending_is_cleared() {
        let rolled_back = state(Some(G2), 0).rollback_pending().unwrap();

        assert_eq!(rolled_back.pending, None);
        assert_eq!(rolled_back.remaining_attempts, 0);
        assert_eq!(rolled_back.rollback_pending(), Ok(rolled_back));
    }

    /// Slot selection, which moved here from `stage0` so a component could
    /// apply the same rule. It had no tests there.
    mod selection {
        use super::*;

        fn slot(sequence: u64, known_good: [u8; 32]) -> [u8; SLOT_BYTES] {
            BootState {
                sequence,
                known_good,
                pending: None,
                remaining_attempts: 0,
                generation_root: GENERATION_ROOT,
                state_root: empty_state_root(),
                accepted_release_sequence: 0,
            }
            .encode()
            .expect("encodable")
        }

        #[test]
        fn the_highest_sequence_wins_from_either_slot() {
            let low = slot(1, G1);
            let high = slot(2, G2);
            let selected = select_bootstate(&low, &high).expect("selectable");
            assert_eq!(selected.slot, Slot::B, "the newer slot is selected");
            assert_eq!(selected.state.sequence, 2);
            let selected = select_bootstate(&high, &low).expect("selectable");
            assert_eq!(selected.slot, Slot::A, "order does not decide it");
            assert_eq!(selected.state.sequence, 2);
        }

        #[test]
        fn one_damaged_slot_is_tolerated() {
            let good = slot(3, G1);
            let mut damaged = slot(2, G2);
            damaged[CHECKSUM_OFFSET] ^= 0xff;
            assert_eq!(
                select_bootstate(&good, &damaged).expect("selectable").slot,
                Slot::A,
            );
            assert_eq!(
                select_bootstate(&damaged, &good).expect("selectable").slot,
                Slot::B,
                "a damaged newer slot does not outrank a valid older one",
            );
        }

        #[test]
        fn identical_slots_select_without_conflict() {
            let both = slot(4, G1);
            let selected = select_bootstate(&both, &both).expect("selectable");
            assert_eq!(selected.slot, Slot::A);
            assert_eq!(selected.state.sequence, 4);
        }

        #[test]
        fn two_valid_slots_at_one_sequence_are_refused() {
            // Same sequence, different content: two commits claim the same
            // generation number, and picking either would be a guess.
            assert_eq!(
                select_bootstate(&slot(5, G1), &slot(5, G2)),
                Err(SelectionError::ConflictingSlots),
            );
        }

        #[test]
        fn no_valid_slot_is_refused_rather_than_invented() {
            let mut a = slot(6, G1);
            let mut b = slot(7, G2);
            a[0] ^= 0xff;
            b[CHECKSUM_OFFSET] ^= 0xff;
            assert_eq!(
                select_bootstate(&a, &b),
                Err(SelectionError::NoValidBootState),
            );
        }

        #[test]
        fn a_commit_targets_the_slot_that_was_not_selected() {
            // Older-slot-first: the selected root must survive the write.
            let selected = select_bootstate(&slot(8, G1), &slot(9, G2)).expect("selectable");
            assert_eq!(selected.slot, Slot::B);
            assert_eq!(selected.slot.other(), Slot::A);
            assert_eq!(Slot::A.other(), Slot::B);
        }
    }
}
