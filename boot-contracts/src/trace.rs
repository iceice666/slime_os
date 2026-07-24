//! Bounded, versioned BootState transition-trace records (M5.6c).
//!
//! Stage-0 and the generation-management service emit one canonical line per
//! durable BootState transition so the conformance checker
//! (`scripts/check-bootstate-trace.py`) can validate each finite trace against
//! the checked M5.6a/M5.6b state machines in `contracts/bootstate/model/`.
//!
//! The format is deliberately fixed-width and allocation-free: rendering never
//! touches the heap or a device, and a record maps 1:1 onto exactly one model
//! action, so instrumentation cannot become a new unbounded boot dependency.

use core::fmt::{self, Write};

include!("generated/bootstate_trace.rs");

/// Model action identity a durable transition corresponds to. Only the
/// transitions the implementation actually performs durably are represented
/// here; the conformance checker (`scripts/check-bootstate-trace.py`) carries
/// the complete model vocabulary, including actions used only for adversarial
/// rejection tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Stage-0 durably decremented the pending attempt before transfer.
    ConsumeAttempt,
    /// The generation-management service promoted the running pending
    /// generation to known-good, retaining the previous known-good root.
    Promotion,
    /// Booted the known-good generation with no pending present.
    BootKnownGood,
    /// Booted the known-good generation after pending attempts were exhausted.
    BootExhaustedKnownGood,
    /// Generation service durably selected a validated staged generation.
    StagePending,
    /// Generation service durably cleared the pending generation.
    Rollback,
}

impl Action {
    /// Stable token used in the trace line and matched by the model oracle.
    pub const fn as_str(self) -> &'static str {
        match self {
            Action::ConsumeAttempt => "consume-attempt",
            Action::Promotion => "promotion",
            Action::BootKnownGood => "boot-known-good",
            Action::BootExhaustedKnownGood => "boot-exhausted-known-good",
            Action::StagePending => "stage-pending",
            Action::Rollback => "rollback",
        }
    }
}

/// Durable commit boundary at which the transition became persistent. The
/// string forms match the `lastCut`/commit vocabulary in the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Commit {
    /// No durable slot write occurred (a pure boot selection).
    None,
    /// After the consumed-attempt slot commit.
    AfterAttemptCommit,
    /// After the health-promotion slot commit.
    HealthPromotion,
    /// After a staged generation became pending.
    AfterPendingCommit,
    /// After rollback cleared the pending generation.
    RollbackUpdate,
}

impl Commit {
    /// Stable token used in the trace line and matched by the model oracle.
    pub const fn as_str(self) -> &'static str {
        match self {
            Commit::None => "none",
            Commit::AfterAttemptCommit => "after-attempt-commit",
            Commit::HealthPromotion => "health-promotion",
            Commit::AfterPendingCommit => "after-pending-commit",
            Commit::RollbackUpdate => "rollback-update",
        }
    }
}

/// One durable BootState transition, carrying every field M5.6c requires:
/// selected slot, durable sequence, known-good and pending identities,
/// attempts before and after, generation and state roots, action identity,
/// and commit boundary.
#[derive(Debug, Clone, Copy)]
pub struct Record {
    pub action: Action,
    pub commit: Commit,
    /// Slot selected for boot: 0 = A, 1 = B.
    pub selected_slot: u8,
    /// Slot the durable write targeted, if any: 0 = A, 1 = B.
    pub target_slot: Option<u8>,
    pub sequence_before: u64,
    pub sequence_after: u64,
    pub attempts_before: u32,
    pub attempts_after: u32,
    pub known_good: [u8; 32],
    pub pending: Option<[u8; 32]>,
    pub generation_root: [u8; 32],
    pub state_root: [u8; 32],
}

/// A rendered trace line held in a fixed-capacity buffer.
pub struct Line {
    data: [u8; MAX_LINE],
    len: usize,
}

impl Line {
    /// The rendered UTF-8 line, without a trailing newline.
    pub fn as_str(&self) -> &str {
        // Only ASCII is ever written, so this never fails.
        core::str::from_utf8(&self.data[..self.len]).unwrap_or("")
    }
}

struct Buffer {
    data: [u8; MAX_LINE],
    len: usize,
}

impl Write for Buffer {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let bytes = text.as_bytes();
        let end = self.len.checked_add(bytes.len()).ok_or(fmt::Error)?;
        if end > MAX_LINE {
            return Err(fmt::Error);
        }
        self.data[self.len..end].copy_from_slice(bytes);
        self.len = end;
        Ok(())
    }
}

struct Hex<'a>(&'a [u8; 32]);

impl fmt::Display for Hex<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

const fn slot_name(slot: u8) -> &'static str {
    if slot == 0 { "A" } else { "B" }
}

impl Record {
    /// Render the record into a bounded line. `MAX_LINE` covers the complete
    /// worst-case representation, pinned by `renders_maximal_record`.
    pub fn render(&self) -> Line {
        let mut buffer = Buffer {
            data: [0u8; MAX_LINE],
            len: 0,
        };
        let target = match self.target_slot {
            Some(slot) => slot_name(slot),
            None => "-",
        };
        write!(
            buffer,
            "{prefix} v{version} action={action} commit={commit} \
             selected_slot={selected} target_slot={target} \
             sequence_before={sequence_before} sequence_after={sequence_after} \
             attempts_before={attempts_before} attempts_after={attempts_after} \
             known_good={known_good} ",
            prefix = TRACE_PREFIX,
            version = TRACE_VERSION,
            action = self.action.as_str(),
            commit = self.commit.as_str(),
            selected = slot_name(self.selected_slot),
            target = target,
            sequence_before = self.sequence_before,
            sequence_after = self.sequence_after,
            attempts_before = self.attempts_before,
            attempts_after = self.attempts_after,
            known_good = Hex(&self.known_good),
        )
        .expect("BootState trace line exceeds MAX_LINE");
        match self.pending {
            Some(pending) => {
                write!(buffer, "pending={} ", Hex(&pending))
                    .expect("BootState trace line exceeds MAX_LINE");
            }
            None => {
                write!(buffer, "pending=none ").expect("BootState trace line exceeds MAX_LINE");
            }
        }
        write!(
            buffer,
            "generation_root={generation_root} state_root={state_root}",
            generation_root = Hex(&self.generation_root),
            state_root = Hex(&self.state_root),
        )
        .expect("BootState trace line exceeds MAX_LINE");
        Line {
            data: buffer.data,
            len: buffer.len,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Record {
        Record {
            action: Action::ConsumeAttempt,
            commit: Commit::AfterAttemptCommit,
            selected_slot: 0,
            target_slot: Some(1),
            sequence_before: 2,
            sequence_after: 3,
            attempts_before: 2,
            attempts_after: 1,
            known_good: [0x11; 32],
            pending: Some([0x22; 32]),
            generation_root: [0x33; 32],
            state_root: [0x44; 32],
        }
    }

    #[test]
    fn renders_all_fields() {
        let record = sample();
        let line = record.render();
        let text = line.as_str();
        assert!(text.starts_with("[bootstate-trace] v1 "));
        assert!(text.contains("action=consume-attempt"));
        assert!(text.contains("commit=after-attempt-commit"));
        assert!(text.contains("selected_slot=A"));
        assert!(text.contains("target_slot=B"));
        assert!(text.contains("attempts_before=2 attempts_after=1"));
        assert!(text.contains(&"11".repeat(32)));
        assert!(text.contains(&"22".repeat(32)));
        assert!(text.len() <= MAX_LINE);
    }

    #[test]
    fn renders_maximal_record() {
        let record = Record {
            action: Action::BootExhaustedKnownGood,
            commit: Commit::HealthPromotion,
            selected_slot: 1,
            target_slot: Some(0),
            sequence_before: u64::MAX,
            sequence_after: u64::MAX,
            attempts_before: u32::MAX,
            attempts_after: u32::MAX,
            known_good: [0xff; 32],
            pending: Some([0xff; 32]),
            generation_root: [0xff; 32],
            state_root: [0xff; 32],
        };
        let line = record.render();
        let text = line.as_str();
        assert!(text.ends_with(&"ff".repeat(32)));
        assert!(text.len() <= MAX_LINE);
    }

    #[test]
    fn renders_absent_pending_and_target() {
        let record = Record {
            action: Action::BootExhaustedKnownGood,
            commit: Commit::None,
            target_slot: None,
            pending: None,
            ..sample()
        };
        let line = record.render();
        let text = line.as_str();
        assert!(text.contains("target_slot=-"));
        assert!(text.contains("pending=none"));
    }
}
