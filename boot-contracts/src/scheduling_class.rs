//! Decoding for the generation-authenticated C9.3 scheduling-class resource.
//!
//! Three things travel together here, because none is meaningful alone: the
//! band mapping (which class is which concrete seL4 TCB priority), the
//! per-instance assignment, and the promotion edges naming who may change whose
//! class at runtime.
//!
//! Two invariants this decoder owns are worth stating, because they are the
//! milestone's required checks made structural rather than procedural:
//!
//! * **A class and a priority cannot disagree.** The resource is the *only*
//!   statement of a thread's priority once a generation declares a policy, so
//!   there is no second number to contradict. What can still contradict is the
//!   manifest's own `Instance.priority`, and the builder refuses that
//!   combination before it reaches a wire record — see
//!   `validated_scheduling_class` in `scripts/build/build-generation.py`.
//! * **No component can widen itself.** A promotion entry whose holder and
//!   subject are the same identity is refused here, so self-promotion is a
//!   generation that does not decode rather than a request the root must
//!   remember to refuse. The root's own promotion path additionally refuses a
//!   subject equal to the caller, because a *runtime* caller could otherwise
//!   reach a legitimately declared edge from the wrong side.
//!
//! CPU quantity is bounded by nothing here. `KernelIsMCS OFF` gives the kernel
//! no budget to charge, and B77 made both readers refuse a nonzero
//! `budget_us`/`period_us`, so a class orders access to the CPU rather than
//! reserving an amount of it.

use crate::sha256::Sha256;

include!("generated/scheduling_class.rs");

pub const MAGIC: [u8; 8] = *b"SLIMESC\0";
pub const MAX_BYTES: usize = HEADER_BYTES
    + MAX_CLASSES * BAND_BYTES
    + MAX_INSTANCES * ENTRY_BYTES
    + MAX_PROMOTIONS * PROMOTION_BYTES;

/// The priority ceiling any declared band may name.
///
/// `slime-root` runs its service loop above every child, so a band at or above
/// the root's own priority lets one child stall the loop every other child
/// waits on (B48). This is the same ceiling `slime_root::task::CHILD_PRIORITY`
/// enforces when it applies a value to a TCB; pinned here so a generation
/// carrying an impossible band fails to decode rather than failing to launch.
pub const MAX_BAND_PRIORITY: u32 = 254;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    UnknownRequiredFlags,
    BadBounds,
    BadOrder,
    UnknownClass,
    /// Two bands name one priority, or a class has no band at all. Either makes
    /// the class-to-priority mapping unobservable.
    BadBandMapping,
    /// A promotion entry names its own holder as subject, or a ceiling with no
    /// band.
    SelfPromotion,
    /// A band priority above what the root may apply to a child.
    Impossible,
}

/// One class band: a class id and the exact TCB priority its threads run at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    pub class_id: u32,
    pub priority: u32,
}

/// One instance's declared class, main thread and workers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassAssignment {
    pub subject_identity: [u8; 32],
    pub class_id: u32,
    pub worker_class_id: u32,
}

/// One promotion edge, as decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Promotion {
    pub holder_identity: [u8; 32],
    pub subject_identity: [u8; 32],
    pub ceiling_priority: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct SchedulingClass<'a> {
    bytes: &'a [u8],
    class_count: usize,
    instance_count: usize,
    promotion_count: usize,
}

impl<'a> SchedulingClass<'a> {
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
        let class_count = u16_at(bytes, 24)? as usize;
        let instance_count = u16_at(bytes, 26)? as usize;
        let promotion_count = u16_at(bytes, 28)? as usize;
        let total_len = u16_at(bytes, 30)? as usize;
        if class_count > MAX_CLASSES
            || instance_count > MAX_INSTANCES
            || promotion_count > MAX_PROMOTIONS
        {
            return Err(DecodeError::BadBounds);
        }
        let expected = HEADER_BYTES
            + class_count * BAND_BYTES
            + instance_count * ENTRY_BYTES
            + promotion_count * PROMOTION_BYTES;
        if total_len != expected || total_len != bytes.len() {
            return Err(DecodeError::BadBounds);
        }
        let decoded = Self {
            bytes,
            class_count,
            instance_count,
            promotion_count,
        };
        decoded.validate_bands()?;
        decoded.validate_instances()?;
        decoded.validate_promotions()?;
        Ok(decoded)
    }

    /// Bands are ascending by class id, name only declared classes, carry a
    /// priority the root may apply, and no two share a priority.
    ///
    /// Distinct priorities are load-bearing rather than tidy: the whole point of
    /// a class is that a `foreground` thread outranks a `bestEffort` one, and
    /// two classes mapped to one number make that unobservable while still
    /// looking like a declared policy.
    fn validate_bands(&self) -> Result<(), DecodeError> {
        let mut previous_class = 0u32;
        for index in 0..self.class_count {
            let band = decode_band(self.bytes, index)?;
            if !is_declared_class(band.class_id) {
                return Err(DecodeError::UnknownClass);
            }
            if index > 0 && band.class_id <= previous_class {
                return Err(DecodeError::BadOrder);
            }
            if band.priority > MAX_BAND_PRIORITY {
                return Err(DecodeError::Impossible);
            }
            if u64_at(band_bytes(self.bytes, index)?, 8)? != 0 {
                return Err(DecodeError::UnknownRequiredFlags);
            }
            for other in 0..index {
                if decode_band(self.bytes, other)?.priority == band.priority {
                    return Err(DecodeError::BadBandMapping);
                }
            }
            previous_class = band.class_id;
        }
        Ok(())
    }

    /// Assignments are ascending by subject identity with no duplicates, and
    /// every class they name has a band.
    fn validate_instances(&self) -> Result<(), DecodeError> {
        let mut previous = [0u8; 32];
        for index in 0..self.instance_count {
            let entry = decode_assignment(self.bytes, index)?;
            if entry.subject_identity == [0; 32]
                || (index > 0 && entry.subject_identity <= previous)
            {
                return Err(DecodeError::BadOrder);
            }
            if u64_at(entry_bytes(self.bytes, index)?, 40)? != 0 {
                return Err(DecodeError::UnknownRequiredFlags);
            }
            for class_id in [entry.class_id, entry.worker_class_id] {
                if !is_declared_class(class_id) {
                    return Err(DecodeError::UnknownClass);
                }
                if self.band_for(class_id).is_none() {
                    return Err(DecodeError::BadBandMapping);
                }
            }
            previous = entry.subject_identity;
        }
        Ok(())
    }

    /// Promotions are ascending by `(holder, subject)`, never self-edges, and
    /// every ceiling is a priority some declared band names.
    ///
    /// The ceiling being a *declared band's* priority rather than an arbitrary
    /// number is what keeps promotion inside the class vocabulary: a holder
    /// cannot be granted a ceiling between two bands and thereby reach a
    /// priority no class maps to.
    fn validate_promotions(&self) -> Result<(), DecodeError> {
        let mut previous = ([0u8; 32], [0u8; 32]);
        for index in 0..self.promotion_count {
            let entry = decode_promotion(self.bytes, index)?;
            if entry.holder_identity == [0; 32] || entry.subject_identity == [0; 32] {
                return Err(DecodeError::BadOrder);
            }
            if entry.holder_identity == entry.subject_identity {
                return Err(DecodeError::SelfPromotion);
            }
            let key = (entry.holder_identity, entry.subject_identity);
            if index > 0 && key <= previous {
                return Err(DecodeError::BadOrder);
            }
            if u32_at(promotion_bytes(self.bytes, index)?, 68)? != 0 {
                return Err(DecodeError::UnknownRequiredFlags);
            }
            if !self.declares_priority(entry.ceiling_priority) {
                return Err(DecodeError::SelfPromotion);
            }
            previous = key;
        }
        Ok(())
    }

    pub const fn class_count(&self) -> usize {
        self.class_count
    }
    pub const fn instance_count(&self) -> usize {
        self.instance_count
    }
    pub const fn promotion_count(&self) -> usize {
        self.promotion_count
    }

    pub fn band(&self, index: usize) -> Option<Band> {
        (index < self.class_count)
            .then(|| decode_band(self.bytes, index).expect("validated scheduling band"))
    }

    /// The priority a class runs at, or `None` when this resource declares no
    /// band for it.
    pub fn band_for(&self, class_id: u32) -> Option<u32> {
        (0..self.class_count)
            .map(|index| decode_band(self.bytes, index).expect("validated scheduling band"))
            .find(|band| band.class_id == class_id)
            .map(|band| band.priority)
    }

    fn declares_priority(&self, priority: u32) -> bool {
        (0..self.class_count).any(|index| {
            decode_band(self.bytes, index)
                .expect("validated band")
                .priority
                == priority
        })
    }

    pub fn assignment(&self, index: usize) -> Option<ClassAssignment> {
        (index < self.instance_count)
            .then(|| decode_assignment(self.bytes, index).expect("validated scheduling entry"))
    }

    /// The class assignment this policy declares for one instance identity, or
    /// `None` when it does not name that instance at all.
    ///
    /// `None` rather than a synthesized default, and the distinction is
    /// load-bearing rather than stylistic. The builder substitutes a band's
    /// priority into a thread's `ScheduleRecord` only for instances this table
    /// *names*; an unnamed instance keeps the root's own child default. A
    /// synthesized `normal` assignment here would therefore report a priority
    /// the thread is not running at — and `normal` need not even have a declared
    /// band, since a policy may legitimately declare only the bands it uses.
    /// Callers resolve the unnamed case against the same default the builder
    /// left in place.
    pub fn class_for(&self, identity: &[u8; 32]) -> Option<ClassAssignment> {
        (0..self.instance_count)
            .map(|index| decode_assignment(self.bytes, index).expect("validated scheduling entry"))
            .find(|entry| entry.subject_identity == *identity)
    }

    pub fn promotion(&self, index: usize) -> Option<Promotion> {
        (index < self.promotion_count)
            .then(|| decode_promotion(self.bytes, index).expect("validated promotion entry"))
    }

    /// The ceiling priority `holder` may promote `subject` to, or `None` when
    /// the generation declares no such edge.
    ///
    /// A self-edge cannot be found here because `validate_promotions` refuses to
    /// decode one, so this returning `Some` already means the two identities
    /// differ.
    pub fn promotion_ceiling(&self, holder: &[u8; 32], subject: &[u8; 32]) -> Option<u32> {
        (0..self.promotion_count)
            .map(|index| decode_promotion(self.bytes, index).expect("validated promotion entry"))
            .find(|entry| entry.holder_identity == *holder && entry.subject_identity == *subject)
            .map(|entry| entry.ceiling_priority)
    }
}

/// Stable identity of an instance this resource names.
///
/// Domain-separated from every other contract's fold, so an identity minted for
/// a clock holder or a wait-set waiter cannot be read as a scheduling subject.
pub fn instance_identity(name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"slime-scheduling-class-instance-v1");
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.finalize()
}

/// Whether `class_id` is a class a manifest may assign or a promotion may
/// request.
///
/// [`UNDECLARED_CLASS_ID`] is deliberately excluded. It is the answer `CLASS_READ`
/// gives for an instance the resource does not name, never an assignment: it
/// maps to no band, so admitting it as a promotion target would ask the root to
/// apply a priority no class declares.
pub const fn is_declared_class(class_id: u32) -> bool {
    matches!(
        class_id,
        CLASS_FOREGROUND | CLASS_NORMAL | CLASS_BEST_EFFORT
    )
}

/// The manifest spelling of a class, for markers and diagnostics.
pub const fn class_name(class_id: u32) -> &'static str {
    match class_id {
        UNDECLARED_CLASS_ID => "undeclared",
        CLASS_FOREGROUND => "foreground",
        CLASS_NORMAL => "normal",
        CLASS_BEST_EFFORT => "bestEffort",
        _ => "?",
    }
}

fn band_bytes(bytes: &[u8], index: usize) -> Result<&[u8], DecodeError> {
    let offset = HEADER_BYTES + index * BAND_BYTES;
    bytes
        .get(offset..offset + BAND_BYTES)
        .ok_or(DecodeError::Truncated)
}

fn entry_bytes(bytes: &[u8], index: usize) -> Result<&[u8], DecodeError> {
    let class_count = u16_at(bytes, 24)? as usize;
    let offset = HEADER_BYTES + class_count * BAND_BYTES + index * ENTRY_BYTES;
    bytes
        .get(offset..offset + ENTRY_BYTES)
        .ok_or(DecodeError::Truncated)
}

fn promotion_bytes(bytes: &[u8], index: usize) -> Result<&[u8], DecodeError> {
    let class_count = u16_at(bytes, 24)? as usize;
    let instance_count = u16_at(bytes, 26)? as usize;
    let offset = HEADER_BYTES
        + class_count * BAND_BYTES
        + instance_count * ENTRY_BYTES
        + index * PROMOTION_BYTES;
    bytes
        .get(offset..offset + PROMOTION_BYTES)
        .ok_or(DecodeError::Truncated)
}

fn decode_band(bytes: &[u8], index: usize) -> Result<Band, DecodeError> {
    let band = band_bytes(bytes, index)?;
    Ok(Band {
        class_id: u32_at(band, 0)?,
        priority: u32_at(band, 4)?,
    })
}

fn decode_assignment(bytes: &[u8], index: usize) -> Result<ClassAssignment, DecodeError> {
    let entry = entry_bytes(bytes, index)?;
    Ok(ClassAssignment {
        subject_identity: entry[..32].try_into().unwrap(),
        class_id: u32_at(entry, 32)?,
        worker_class_id: u32_at(entry, 36)?,
    })
}

fn decode_promotion(bytes: &[u8], index: usize) -> Result<Promotion, DecodeError> {
    let entry = promotion_bytes(bytes, index)?;
    Ok(Promotion {
        holder_identity: entry[..32].try_into().unwrap(),
        subject_identity: entry[32..64].try_into().unwrap(),
        ceiling_priority: u32_at(entry, 64)?,
    })
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
    use alloc::vec::Vec;

    /// `SchedulingClass` holds a borrowed slice and deliberately does not
    /// implement `PartialEq`, so refusal tests compare the error alone.
    fn decode_error(bytes: &[u8]) -> Option<DecodeError> {
        SchedulingClass::decode(bytes).err()
    }

    fn band(class_id: u32, priority: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&class_id.to_le_bytes());
        bytes.extend_from_slice(&priority.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes
    }

    fn assignment(name: &str, class_id: u32, worker_class_id: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&instance_identity(name));
        bytes.extend_from_slice(&class_id.to_le_bytes());
        bytes.extend_from_slice(&worker_class_id.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes
    }

    fn promotion(holder: &str, subject: &str, ceiling: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&instance_identity(holder));
        bytes.extend_from_slice(&instance_identity(subject));
        bytes.extend_from_slice(&ceiling.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes
    }

    fn resource(bands: &[Vec<u8>], entries: &[Vec<u8>], promotions: &[Vec<u8>]) -> Vec<u8> {
        let total = HEADER_BYTES
            + bands.len() * BAND_BYTES
            + entries.len() * ENTRY_BYTES
            + promotions.len() * PROMOTION_BYTES;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&(bands.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&(promotions.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&(total as u16).to_le_bytes());
        for record in bands.iter().chain(entries).chain(promotions) {
            bytes.extend_from_slice(record);
        }
        bytes
    }

    /// Three sorted bands, two assignments, one promotion edge — the shape the
    /// plane fixture declares.
    fn valid() -> Vec<u8> {
        resource(
            &[
                band(CLASS_FOREGROUND, 200),
                band(CLASS_NORMAL, 150),
                band(CLASS_BEST_EFFORT, 100),
            ],
            &sorted_assignments(&[
                ("sched-foreground", CLASS_FOREGROUND, CLASS_FOREGROUND),
                ("sched-burner", CLASS_BEST_EFFORT, CLASS_BEST_EFFORT),
            ]),
            &[promotion("sched-foreground", "sched-burner", 150)],
        )
    }

    fn sorted_assignments(entries: &[(&str, u32, u32)]) -> Vec<Vec<u8>> {
        let mut names: Vec<(&str, u32, u32)> = entries.to_vec();
        names.sort_by_key(|(name, _, _)| instance_identity(name));
        names
            .into_iter()
            .map(|(name, class_id, worker)| assignment(name, class_id, worker))
            .collect()
    }

    #[test]
    fn a_declared_policy_resolves_every_band_and_assignment() {
        let bytes = valid();
        let policy = SchedulingClass::decode(&bytes).expect("valid policy");
        assert_eq!(policy.class_count(), 3);
        assert_eq!(policy.instance_count(), 2);
        assert_eq!(policy.promotion_count(), 1);
        assert_eq!(policy.band_for(CLASS_FOREGROUND), Some(200));
        assert_eq!(policy.band_for(CLASS_BEST_EFFORT), Some(100));
        let foreground = policy
            .class_for(&instance_identity("sched-foreground"))
            .expect("foreground is named");
        assert_eq!(foreground.class_id, CLASS_FOREGROUND);
        let burner = policy
            .class_for(&instance_identity("sched-burner"))
            .expect("burner is named");
        assert_eq!(burner.class_id, CLASS_BEST_EFFORT);
    }

    /// An instance the resource does not name has no assignment at all.
    ///
    /// `None` rather than a synthesized `normal`: the builder leaves such an
    /// instance's `ScheduleRecord` at the root's child default, so reporting a
    /// band here would name a priority the thread is not running at. Found by
    /// review, where the two readers disagreed for exactly this case.
    #[test]
    fn an_unnamed_instance_has_no_declared_assignment() {
        let bytes = valid();
        let policy = SchedulingClass::decode(&bytes).expect("valid policy");
        assert_eq!(policy.class_for(&instance_identity("sched-unnamed")), None);
    }

    /// C9.3's "no component can widen itself", enforced structurally: a
    /// self-edge does not decode.
    #[test]
    fn a_self_promotion_edge_is_refused() {
        let bytes = resource(
            &[band(CLASS_FOREGROUND, 200), band(CLASS_NORMAL, 150)],
            &[],
            &[promotion("sched-burner", "sched-burner", 200)],
        );
        assert_eq!(decode_error(&bytes), Some(DecodeError::SelfPromotion));
    }

    /// A ceiling no band names would let a holder reach a priority outside the
    /// class vocabulary.
    #[test]
    fn a_promotion_ceiling_must_name_a_declared_band() {
        let bytes = resource(
            &[band(CLASS_FOREGROUND, 200), band(CLASS_NORMAL, 150)],
            &[],
            &[promotion("sched-foreground", "sched-burner", 175)],
        );
        assert_eq!(decode_error(&bytes), Some(DecodeError::SelfPromotion));
    }

    /// Two classes at one priority is a policy whose effect cannot be observed.
    #[test]
    fn two_bands_may_not_share_one_priority() {
        let bytes = resource(
            &[band(CLASS_FOREGROUND, 150), band(CLASS_NORMAL, 150)],
            &[],
            &[],
        );
        assert_eq!(decode_error(&bytes), Some(DecodeError::BadBandMapping));
    }

    /// A band above the root's child ceiling would let a child stall the
    /// service loop (B48), so it fails to decode rather than failing to launch.
    #[test]
    fn a_band_above_the_child_ceiling_is_impossible() {
        let bytes = resource(&[band(CLASS_FOREGROUND, 255)], &[], &[]);
        assert_eq!(decode_error(&bytes), Some(DecodeError::Impossible));
    }

    #[test]
    fn an_assignment_naming_an_unbanded_class_is_refused() {
        let bytes = resource(
            &[band(CLASS_NORMAL, 150)],
            &[assignment(
                "sched-foreground",
                CLASS_FOREGROUND,
                CLASS_NORMAL,
            )],
            &[],
        );
        assert_eq!(decode_error(&bytes), Some(DecodeError::BadBandMapping));
    }

    #[test]
    fn an_undeclared_class_id_is_refused() {
        let bytes = resource(&[band(9, 150)], &[], &[]);
        assert_eq!(decode_error(&bytes), Some(DecodeError::UnknownClass));
    }

    #[test]
    fn bands_must_ascend_by_class_id() {
        let bytes = resource(
            &[band(CLASS_NORMAL, 150), band(CLASS_FOREGROUND, 200)],
            &[],
            &[],
        );
        assert_eq!(decode_error(&bytes), Some(DecodeError::BadOrder));
    }

    #[test]
    fn assignments_must_ascend_and_not_repeat() {
        let repeated = assignment("sched-burner", CLASS_BEST_EFFORT, CLASS_BEST_EFFORT);
        let bytes = resource(
            &[band(CLASS_BEST_EFFORT, 100)],
            &[repeated.clone(), repeated],
            &[],
        );
        assert_eq!(decode_error(&bytes), Some(DecodeError::BadOrder));
    }

    #[test]
    fn a_truncated_or_mismatched_length_is_refused() {
        let bytes = valid();
        assert_eq!(
            decode_error(&bytes[..bytes.len() - 1]),
            Some(DecodeError::BadBounds)
        );
        assert_eq!(
            decode_error(&bytes[..HEADER_BYTES - 1]),
            Some(DecodeError::Truncated)
        );
    }

    #[test]
    fn a_wrong_magic_or_version_is_refused() {
        let mut bytes = valid();
        bytes[0] = b'X';
        assert_eq!(decode_error(&bytes), Some(DecodeError::BadMagic));
        let mut bytes = valid();
        bytes[8] = 9;
        assert_eq!(decode_error(&bytes), Some(DecodeError::UnsupportedVersion));
    }

    #[test]
    fn unknown_required_flags_are_refused() {
        let mut bytes = valid();
        bytes[16] = 1;
        assert_eq!(
            decode_error(&bytes),
            Some(DecodeError::UnknownRequiredFlags)
        );
    }

    /// Identity is domain-separated, so a name folded for another contract
    /// cannot be read as a scheduling subject.
    #[test]
    fn instance_identity_is_domain_separated() {
        assert_ne!(
            instance_identity("sched-burner"),
            crate::clock_authority::holder_identity("sched-burner")
        );
    }

    #[test]
    fn a_declared_edge_resolves_its_ceiling_and_nothing_else() {
        let bytes = valid();
        let policy = SchedulingClass::decode(&bytes).expect("valid policy");
        assert_eq!(
            policy.promotion_ceiling(
                &instance_identity("sched-foreground"),
                &instance_identity("sched-burner"),
            ),
            Some(150)
        );
        // The reverse edge is a different edge, and this generation declares
        // only one direction.
        assert_eq!(
            policy.promotion_ceiling(
                &instance_identity("sched-burner"),
                &instance_identity("sched-foreground"),
            ),
            None
        );
    }
}
