//! Decoding for the generation-authenticated C9.4 lifecycle-policy resource.
//!
//! Four tables travel together, because none of them is a policy on its own: the
//! admitted transition graph with its entry and exhaustion states, the
//! per-instance restart bound and backoff, the health dependencies that gate a
//! start, and the parameter authority.
//!
//! Three invariants this decoder owns are worth stating, because they are the
//! milestone's required checks made structural rather than procedural:
//!
//! * **An unadmitted transition cannot be reached.** [`LifecyclePolicy::admits`]
//!   is a lookup in a table this decoder has already validated to be ascending,
//!   duplicate-free, and free of self-edges, so "the graph is enforced" is one
//!   table lookup rather than a set of conditions a caller must remember.
//!
//! * **An attempt bound is a number, and a spent bound is a state.** The decoder
//!   refuses an `attempts` value above the contract ceiling and a `causes` mask
//!   naming a cause outside the closed vocabulary, so a policy that would
//!   restart forever, or restart on a cause nothing can produce, does not decode.
//!   Where the count *lives* is the root's business — per declared instance,
//!   across task lifetimes — and this resource only says how large it may get.
//!
//! * **Parameter authority is directional and explicit.** Read and write are
//!   separate bits over the same `(holder, subject)` pair, and unlike
//!   `scheduling_class`'s promotion edge the reflexive pair is admitted: a
//!   component owning its own configuration is the ordinary case, whereas a
//!   component widening its own scheduling class never is. What is *not*
//!   admitted is an empty flag set, because an edge granting neither read nor
//!   write is a declaration with no content that reads like authority.
//!
//! Backoff is declared here and computed here ([`RestartPolicy::backoff_for`]),
//! not in the supervisor. C9.4's check is that backoff is observed against
//! C9.1's clock rather than a spin count, and that only holds if the root and
//! the supervisor resolve the *same* delay from the same declared numbers.

use crate::sha256::Sha256;

include!("generated/lifecycle_policy.rs");

pub const MAGIC: [u8; 8] = *b"SLIMELC\0";
pub const MAX_BYTES: usize = HEADER_BYTES
    + MAX_TRANSITIONS * TRANSITION_BYTES
    + MAX_INSTANCES * RESTART_BYTES
    + MAX_DEPENDENCIES * DEPENDENCY_BYTES
    + MAX_PARAMETER_GRANTS * PARAMETER_BYTES;

/// Every terminal cause a restart policy may name, as a mask.
pub const CAUSE_ALL: u32 =
    cause_bit(CAUSE_EXIT) | cause_bit(CAUSE_FAULT) | cause_bit(CAUSE_UNHEALTHY);

/// Both parameter authority bits.
pub const PARAMETER_ALL: u32 = PARAMETER_READ | PARAMETER_WRITE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    UnknownRequiredFlags,
    BadBounds,
    BadOrder,
    /// A state id outside the closed vocabulary, or `undeclared` where a
    /// declaration is required.
    UnknownState,
    /// A terminal cause outside the closed vocabulary.
    UnknownCause,
    /// A transition from a state to itself. A self-edge is not a transition: it
    /// would admit a component "advancing" without leaving its state, which
    /// makes an observed advance indistinguishable from a no-op.
    SelfTransition,
    /// An attempt bound above the contract ceiling, or a backoff outside the
    /// declared range.
    BadRestartBound,
    /// A dependency naming itself, which can never be satisfied: an instance
    /// waiting for its own state to advance is a start that cannot happen.
    SelfDependency,
    /// A parameter grant carrying neither read nor write.
    EmptyAuthority,
    /// The initial state is reachable by no declared transition, or the terminal
    /// state is one no exhausted instance could be placed in.
    UnreachableState,
}

/// One admitted lifecycle edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    pub from_state: u32,
    pub to_state: u32,
}

/// One instance's declared restart policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RestartPolicy {
    pub subject_identity: [u8; 32],
    pub attempts: u32,
    pub causes: u32,
    pub backoff_ns: u64,
    pub backoff_factor: u32,
}

impl RestartPolicy {
    /// Whether this policy restarts on `cause_id`.
    pub const fn restarts_on(&self, cause_id: u32) -> bool {
        is_declared_cause(cause_id) && self.causes & cause_bit(cause_id) != 0
    }

    /// The declared delay before restart attempt `attempt`, zero-based.
    ///
    /// Computed here rather than by each reader, because the root refuses a
    /// restart before this instant and the supervisor arms a C9.1 timer for it:
    /// two implementations of one growth rule is how a supervisor comes to wait
    /// for a delay the root does not recognize. Saturating at the contract
    /// ceiling rather than wrapping, so a large factor and a large attempt count
    /// cannot multiply into a short delay.
    pub const fn backoff_for(&self, attempt: u32) -> u64 {
        let mut delay = self.backoff_ns;
        let mut remaining = attempt;
        while remaining > 0 {
            // Widened before the multiply: `backoff_ns` is already bounded by
            // `MAX_BACKOFF_NS`, but the product of two in-range u64s is not, and
            // a wrap here would turn a long backoff into an immediate one.
            let scaled =
                (delay as u128) * (self.backoff_factor as u128) / (BACKOFF_FACTOR_SCALE as u128);
            if scaled >= MAX_BACKOFF_NS as u128 {
                return MAX_BACKOFF_NS;
            }
            delay = scaled as u64;
            remaining -= 1;
        }
        delay
    }
}

/// One health dependency: `subject` does not start until `dependency` reaches
/// `required_state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthDependency {
    pub subject_identity: [u8; 32],
    pub dependency_identity: [u8; 32],
    pub required_state: u32,
}

/// One parameter-state authority edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParameterGrant {
    pub holder_identity: [u8; 32],
    pub subject_identity: [u8; 32],
    pub authority_flags: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct LifecyclePolicy<'a> {
    bytes: &'a [u8],
    initial_state: u32,
    terminal_state: u32,
    transition_count: usize,
    restart_count: usize,
    dependency_count: usize,
    parameter_count: usize,
}

impl<'a> LifecyclePolicy<'a> {
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
        let initial_state = u32::from(u16_at(bytes, 24)?);
        let terminal_state = u32::from(u16_at(bytes, 26)?);
        let transition_count = u16_at(bytes, 28)? as usize;
        let restart_count = u16_at(bytes, 30)? as usize;
        let dependency_count = u16_at(bytes, 32)? as usize;
        let parameter_count = u16_at(bytes, 34)? as usize;
        let total_len = u32_at(bytes, 36)? as usize;
        if transition_count > MAX_TRANSITIONS
            || restart_count > MAX_INSTANCES
            || dependency_count > MAX_DEPENDENCIES
            || parameter_count > MAX_PARAMETER_GRANTS
        {
            return Err(DecodeError::BadBounds);
        }
        let expected = HEADER_BYTES
            + transition_count * TRANSITION_BYTES
            + restart_count * RESTART_BYTES
            + dependency_count * DEPENDENCY_BYTES
            + parameter_count * PARAMETER_BYTES;
        if total_len != expected || total_len != bytes.len() {
            return Err(DecodeError::BadBounds);
        }
        // Both graph endpoints must be real states. `undeclared` is the answer
        // for an instance outside the policy, so naming it here would make the
        // graph's entry the same value as "not in the graph".
        if !is_declared_state(initial_state) || !is_declared_state(terminal_state) {
            return Err(DecodeError::UnknownState);
        }
        if initial_state == terminal_state {
            return Err(DecodeError::UnreachableState);
        }
        let decoded = Self {
            bytes,
            initial_state,
            terminal_state,
            transition_count,
            restart_count,
            dependency_count,
            parameter_count,
        };
        decoded.validate_transitions()?;
        decoded.validate_restarts()?;
        decoded.validate_dependencies()?;
        decoded.validate_parameters()?;
        Ok(decoded)
    }

    /// Transitions are ascending by `(from, to)` with no duplicates, name only
    /// declared states, and are never self-edges.
    ///
    /// A policy declaring restarts must also declare an edge *into* its terminal
    /// state, or exhaustion would move an instance somewhere the graph says it
    /// cannot go — and "exhausting the attempt bound leaves the graph in a
    /// declared terminal state" would be a claim about a state no edge reaches.
    fn validate_transitions(&self) -> Result<(), DecodeError> {
        let mut previous = (0u32, 0u32);
        let mut reaches_terminal = false;
        for index in 0..self.transition_count {
            let edge = decode_transition(self.bytes, index)?;
            if !is_declared_state(edge.from_state) || !is_declared_state(edge.to_state) {
                return Err(DecodeError::UnknownState);
            }
            if edge.from_state == edge.to_state {
                return Err(DecodeError::SelfTransition);
            }
            let key = (edge.from_state, edge.to_state);
            if index > 0 && key <= previous {
                return Err(DecodeError::BadOrder);
            }
            if u64_at(transition_bytes(self.bytes, index)?, 8)? != 0 {
                return Err(DecodeError::UnknownRequiredFlags);
            }
            if edge.to_state == self.terminal_state {
                reaches_terminal = true;
            }
            previous = key;
        }
        if self.restart_count > 0 && !reaches_terminal {
            return Err(DecodeError::UnreachableState);
        }
        Ok(())
    }

    /// Restart policies are ascending by subject identity with no duplicates,
    /// carry an attempt bound within the ceiling, name only declared causes, and
    /// declare a backoff the contract can represent.
    fn validate_restarts(&self) -> Result<(), DecodeError> {
        let mut previous = [0u8; 32];
        for index in 0..self.restart_count {
            let entry = decode_restart(self.bytes, index)?;
            if entry.subject_identity == [0; 32]
                || (index > 0 && entry.subject_identity <= previous)
            {
                return Err(DecodeError::BadOrder);
            }
            let bytes = restart_bytes(self.bytes, index)?;
            if bytes[52..64] != [0u8; 12] {
                return Err(DecodeError::UnknownRequiredFlags);
            }
            if entry.attempts > MAX_RESTART_ATTEMPTS {
                return Err(DecodeError::BadRestartBound);
            }
            // An empty cause mask with a nonzero attempt bound is a policy that
            // can never fire, which reads as supervision while providing none.
            if entry.causes == 0 || entry.causes & !CAUSE_ALL != 0 {
                return Err(DecodeError::UnknownCause);
            }
            if entry.backoff_ns > MAX_BACKOFF_NS {
                return Err(DecodeError::BadRestartBound);
            }
            // Below `BACKOFF_FACTOR_SCALE` is *shrinking* backoff: each attempt
            // would wait less than the last, which inverts the mechanism's
            // purpose while still declaring one.
            if entry.backoff_factor < BACKOFF_FACTOR_SCALE
                || entry.backoff_factor > MAX_BACKOFF_FACTOR
            {
                return Err(DecodeError::BadRestartBound);
            }
            previous = entry.subject_identity;
        }
        Ok(())
    }

    /// Dependencies are ascending by `(subject, dependency)` with no duplicates,
    /// never self-edges, and name a declared required state.
    fn validate_dependencies(&self) -> Result<(), DecodeError> {
        let mut previous = ([0u8; 32], [0u8; 32]);
        for index in 0..self.dependency_count {
            let entry = decode_dependency(self.bytes, index)?;
            if entry.subject_identity == [0; 32] || entry.dependency_identity == [0; 32] {
                return Err(DecodeError::BadOrder);
            }
            if entry.subject_identity == entry.dependency_identity {
                return Err(DecodeError::SelfDependency);
            }
            let key = (entry.subject_identity, entry.dependency_identity);
            if index > 0 && key <= previous {
                return Err(DecodeError::BadOrder);
            }
            if u32_at(dependency_bytes(self.bytes, index)?, 68)? != 0 {
                return Err(DecodeError::UnknownRequiredFlags);
            }
            if !is_declared_state(entry.required_state) {
                return Err(DecodeError::UnknownState);
            }
            previous = key;
        }
        Ok(())
    }

    /// Parameter grants are ascending by `(holder, subject)` with no duplicates
    /// and carry at least one authority bit.
    ///
    /// The reflexive edge is admitted here, unlike a scheduling promotion edge:
    /// see the module header.
    fn validate_parameters(&self) -> Result<(), DecodeError> {
        let mut previous = ([0u8; 32], [0u8; 32]);
        for index in 0..self.parameter_count {
            let entry = decode_parameter(self.bytes, index)?;
            if entry.holder_identity == [0; 32] || entry.subject_identity == [0; 32] {
                return Err(DecodeError::BadOrder);
            }
            let key = (entry.holder_identity, entry.subject_identity);
            if index > 0 && key <= previous {
                return Err(DecodeError::BadOrder);
            }
            if u32_at(parameter_bytes(self.bytes, index)?, 68)? != 0 {
                return Err(DecodeError::UnknownRequiredFlags);
            }
            if entry.authority_flags == 0 || entry.authority_flags & !PARAMETER_ALL != 0 {
                return Err(DecodeError::EmptyAuthority);
            }
            previous = key;
        }
        Ok(())
    }

    pub const fn initial_state(&self) -> u32 {
        self.initial_state
    }
    pub const fn terminal_state(&self) -> u32 {
        self.terminal_state
    }
    pub const fn transition_count(&self) -> usize {
        self.transition_count
    }
    pub const fn restart_count(&self) -> usize {
        self.restart_count
    }
    pub const fn dependency_count(&self) -> usize {
        self.dependency_count
    }
    pub const fn parameter_count(&self) -> usize {
        self.parameter_count
    }

    pub fn transition(&self, index: usize) -> Option<Transition> {
        (index < self.transition_count)
            .then(|| decode_transition(self.bytes, index).expect("validated transition"))
    }

    /// Whether this policy admits moving from `from_state` to `to_state`.
    ///
    /// The only question the root asks of the transition graph, so it is one
    /// lookup rather than an iteration each caller writes: an advance the table
    /// does not carry is refused, including every advance out of a state the
    /// graph never names.
    pub fn admits(&self, from_state: u32, to_state: u32) -> bool {
        (0..self.transition_count)
            .map(|index| decode_transition(self.bytes, index).expect("validated transition"))
            .any(|edge| edge.from_state == from_state && edge.to_state == to_state)
    }

    pub fn restart(&self, index: usize) -> Option<RestartPolicy> {
        (index < self.restart_count)
            .then(|| decode_restart(self.bytes, index).expect("validated restart policy"))
    }

    /// The restart policy declared for one instance identity, or `None` when
    /// this resource names no policy for it.
    ///
    /// `None` rather than a synthesized zero-attempt default, for the reason
    /// `scheduling_class::class_for` returns `None`: the root distinguishes "the
    /// generation declares this instance is never restarted" from "the
    /// generation says nothing about this instance", and only the first is a
    /// policy a supervisor may report having honoured.
    pub fn restart_for(&self, identity: &[u8; 32]) -> Option<RestartPolicy> {
        (0..self.restart_count)
            .map(|index| decode_restart(self.bytes, index).expect("validated restart policy"))
            .find(|entry| entry.subject_identity == *identity)
    }

    pub fn dependency(&self, index: usize) -> Option<HealthDependency> {
        (index < self.dependency_count)
            .then(|| decode_dependency(self.bytes, index).expect("validated dependency"))
    }

    /// Every dependency edge declared for one subject, in table order.
    pub fn dependencies_of(
        &self,
        subject: &[u8; 32],
    ) -> impl Iterator<Item = HealthDependency> + '_ {
        let subject = *subject;
        (0..self.dependency_count)
            .map(|index| decode_dependency(self.bytes, index).expect("validated dependency"))
            .filter(move |entry| entry.subject_identity == subject)
    }

    pub fn parameter(&self, index: usize) -> Option<ParameterGrant> {
        (index < self.parameter_count)
            .then(|| decode_parameter(self.bytes, index).expect("validated parameter grant"))
    }

    /// The parameter authority `holder` holds over `subject`, or `None` when the
    /// generation declares no edge between them.
    ///
    /// Absence is the deny-by-default answer, and it covers the reflexive case:
    /// a component the table does not name as its own holder cannot reach its
    /// own parameters, which is what makes this an authority rather than a
    /// namespace.
    pub fn parameter_authority(&self, holder: &[u8; 32], subject: &[u8; 32]) -> Option<u32> {
        (0..self.parameter_count)
            .map(|index| decode_parameter(self.bytes, index).expect("validated parameter grant"))
            .find(|entry| entry.holder_identity == *holder && entry.subject_identity == *subject)
            .map(|entry| entry.authority_flags)
    }
}

/// Stable identity of an instance this resource names.
///
/// Domain-separated from every other contract's fold, so an identity minted for
/// a clock holder, a wait-set waiter, or a scheduling subject cannot be read as
/// a lifecycle subject.
pub fn instance_identity(name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"slime-lifecycle-policy-instance-v1");
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.finalize()
}

/// Whether `state_id` is a state a manifest may declare or a component may
/// advance to.
///
/// [`UNDECLARED_STATE_ID`] is deliberately excluded, exactly as
/// `scheduling_class::is_declared_class` excludes its undeclared id: it is the
/// answer `STATE_READ` gives for an instance the policy does not name, never a
/// declaration, so admitting it as an advance target would ask the root to move
/// a component into "not in the graph".
pub const fn is_declared_state(state_id: u32) -> bool {
    matches!(
        state_id,
        STATE_INITIALIZE
            | STATE_CONFIGURE
            | STATE_START
            | STATE_READY
            | STATE_RUNNING
            | STATE_DEGRADED
            | STATE_STOP
            | STATE_ERROR
    )
}

/// Whether `cause_id` is a terminal cause a restart policy may name.
pub const fn is_declared_cause(cause_id: u32) -> bool {
    matches!(cause_id, CAUSE_EXIT | CAUSE_FAULT | CAUSE_UNHEALTHY)
}

/// The mask bit one terminal cause occupies.
///
/// Ids are one-based so bit zero stays unused, keeping an all-zero `causes`
/// field distinguishable from a policy that names the first cause.
pub const fn cause_bit(cause_id: u32) -> u32 {
    if is_declared_cause(cause_id) {
        1u32 << (cause_id - 1)
    } else {
        0
    }
}

/// The manifest spelling of a state, for markers and diagnostics.
pub const fn state_name(state_id: u32) -> &'static str {
    match state_id {
        UNDECLARED_STATE_ID => "undeclared",
        STATE_INITIALIZE => "Initialize",
        STATE_CONFIGURE => "Configure",
        STATE_START => "Start",
        STATE_READY => "Ready",
        STATE_RUNNING => "Running",
        STATE_DEGRADED => "Degraded",
        STATE_STOP => "Stop",
        STATE_ERROR => "Error",
        _ => "?",
    }
}

/// The manifest spelling of a terminal cause, for markers and diagnostics.
pub const fn cause_name(cause_id: u32) -> &'static str {
    match cause_id {
        UNDECLARED_CAUSE_ID => "live",
        CAUSE_EXIT => "exit",
        CAUSE_FAULT => "fault",
        CAUSE_UNHEALTHY => "unhealthy",
        _ => "?",
    }
}

fn transition_bytes(bytes: &[u8], index: usize) -> Result<&[u8], DecodeError> {
    let offset = HEADER_BYTES + index * TRANSITION_BYTES;
    bytes
        .get(offset..offset + TRANSITION_BYTES)
        .ok_or(DecodeError::Truncated)
}

fn restart_bytes(bytes: &[u8], index: usize) -> Result<&[u8], DecodeError> {
    let transitions = u16_at(bytes, 28)? as usize;
    let offset = HEADER_BYTES + transitions * TRANSITION_BYTES + index * RESTART_BYTES;
    bytes
        .get(offset..offset + RESTART_BYTES)
        .ok_or(DecodeError::Truncated)
}

fn dependency_bytes(bytes: &[u8], index: usize) -> Result<&[u8], DecodeError> {
    let transitions = u16_at(bytes, 28)? as usize;
    let restarts = u16_at(bytes, 30)? as usize;
    let offset = HEADER_BYTES
        + transitions * TRANSITION_BYTES
        + restarts * RESTART_BYTES
        + index * DEPENDENCY_BYTES;
    bytes
        .get(offset..offset + DEPENDENCY_BYTES)
        .ok_or(DecodeError::Truncated)
}

fn parameter_bytes(bytes: &[u8], index: usize) -> Result<&[u8], DecodeError> {
    let transitions = u16_at(bytes, 28)? as usize;
    let restarts = u16_at(bytes, 30)? as usize;
    let dependencies = u16_at(bytes, 32)? as usize;
    let offset = HEADER_BYTES
        + transitions * TRANSITION_BYTES
        + restarts * RESTART_BYTES
        + dependencies * DEPENDENCY_BYTES
        + index * PARAMETER_BYTES;
    bytes
        .get(offset..offset + PARAMETER_BYTES)
        .ok_or(DecodeError::Truncated)
}

fn decode_transition(bytes: &[u8], index: usize) -> Result<Transition, DecodeError> {
    let entry = transition_bytes(bytes, index)?;
    Ok(Transition {
        from_state: u32_at(entry, 0)?,
        to_state: u32_at(entry, 4)?,
    })
}

fn decode_restart(bytes: &[u8], index: usize) -> Result<RestartPolicy, DecodeError> {
    let entry = restart_bytes(bytes, index)?;
    Ok(RestartPolicy {
        subject_identity: identity_at(entry, 0)?,
        attempts: u32_at(entry, 32)?,
        causes: u32_at(entry, 36)?,
        backoff_ns: u64_at(entry, 40)?,
        backoff_factor: u32_at(entry, 48)?,
    })
}

fn decode_dependency(bytes: &[u8], index: usize) -> Result<HealthDependency, DecodeError> {
    let entry = dependency_bytes(bytes, index)?;
    Ok(HealthDependency {
        subject_identity: identity_at(entry, 0)?,
        dependency_identity: identity_at(entry, 32)?,
        required_state: u32_at(entry, 64)?,
    })
}

fn decode_parameter(bytes: &[u8], index: usize) -> Result<ParameterGrant, DecodeError> {
    let entry = parameter_bytes(bytes, index)?;
    Ok(ParameterGrant {
        holder_identity: identity_at(entry, 0)?,
        subject_identity: identity_at(entry, 32)?,
        authority_flags: u32_at(entry, 64)?,
    })
}

fn identity_at(bytes: &[u8], offset: usize) -> Result<[u8; 32], DecodeError> {
    bytes
        .get(offset..offset + 32)
        .ok_or(DecodeError::Truncated)?
        .try_into()
        .map_err(|_| DecodeError::Truncated)
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, DecodeError> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(DecodeError::Truncated)?
            .try_into()
            .map_err(|_| DecodeError::Truncated)?,
    ))
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, DecodeError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(DecodeError::Truncated)?
            .try_into()
            .map_err(|_| DecodeError::Truncated)?,
    ))
}

fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, DecodeError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(DecodeError::Truncated)?
            .try_into()
            .map_err(|_| DecodeError::Truncated)?,
    ))
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    use alloc::vec::Vec;

    fn header(
        initial: u32,
        terminal: u32,
        transitions: usize,
        restarts: usize,
        dependencies: usize,
        parameters: usize,
    ) -> Vec<u8> {
        let total = HEADER_BYTES
            + transitions * TRANSITION_BYTES
            + restarts * RESTART_BYTES
            + dependencies * DEPENDENCY_BYTES
            + parameters * PARAMETER_BYTES;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&(initial as u16).to_le_bytes());
        bytes.extend_from_slice(&(terminal as u16).to_le_bytes());
        bytes.extend_from_slice(&(transitions as u16).to_le_bytes());
        bytes.extend_from_slice(&(restarts as u16).to_le_bytes());
        bytes.extend_from_slice(&(dependencies as u16).to_le_bytes());
        bytes.extend_from_slice(&(parameters as u16).to_le_bytes());
        bytes.extend_from_slice(&(total as u32).to_le_bytes());
        bytes
    }

    fn transition(from: u32, to: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&from.to_le_bytes());
        bytes.extend_from_slice(&to.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes
    }

    fn restart(
        identity: [u8; 32],
        attempts: u32,
        causes: u32,
        backoff_ns: u64,
        factor: u32,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&identity);
        bytes.extend_from_slice(&attempts.to_le_bytes());
        bytes.extend_from_slice(&causes.to_le_bytes());
        bytes.extend_from_slice(&backoff_ns.to_le_bytes());
        bytes.extend_from_slice(&factor.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 12]);
        bytes
    }

    fn dependency(subject: [u8; 32], on: [u8; 32], state: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&subject);
        bytes.extend_from_slice(&on);
        bytes.extend_from_slice(&state.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes
    }

    fn parameter(holder: [u8; 32], subject: [u8; 32], flags: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&holder);
        bytes.extend_from_slice(&subject);
        bytes.extend_from_slice(&flags.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes
    }

    /// The identities the fixtures below use, ordered so table sort order is
    /// predictable regardless of what the fold produces.
    fn ordered_identities() -> ([u8; 32], [u8; 32]) {
        let a = instance_identity("worker");
        let b = instance_identity("supervisor");
        if a < b { (a, b) } else { (b, a) }
    }

    fn valid_policy() -> Vec<u8> {
        let (low, high) = ordered_identities();
        let mut bytes = header(STATE_INITIALIZE, STATE_ERROR, 3, 1, 1, 2);
        bytes.extend(transition(STATE_INITIALIZE, STATE_RUNNING));
        bytes.extend(transition(STATE_RUNNING, STATE_DEGRADED));
        bytes.extend(transition(STATE_RUNNING, STATE_ERROR));
        bytes.extend(restart(
            low,
            2,
            cause_bit(CAUSE_FAULT) | cause_bit(CAUSE_EXIT),
            1000,
            512,
        ));
        bytes.extend(dependency(low, high, STATE_RUNNING));
        bytes.extend(parameter(low, low, PARAMETER_READ));
        bytes.extend(parameter(high, low, PARAMETER_READ | PARAMETER_WRITE));
        bytes
    }

    #[test]
    fn a_valid_policy_resolves_every_table() {
        let bytes = valid_policy();
        let policy = LifecyclePolicy::decode(&bytes).expect("valid policy");
        let (low, high) = ordered_identities();
        assert_eq!(policy.initial_state(), STATE_INITIALIZE);
        assert_eq!(policy.terminal_state(), STATE_ERROR);
        assert_eq!(policy.transition_count(), 3);
        assert!(policy.admits(STATE_RUNNING, STATE_DEGRADED));
        assert!(!policy.admits(STATE_DEGRADED, STATE_RUNNING));
        let restart = policy.restart_for(&low).expect("declared restart policy");
        assert_eq!(restart.attempts, 2);
        assert!(restart.restarts_on(CAUSE_FAULT));
        assert!(!restart.restarts_on(CAUSE_UNHEALTHY));
        assert_eq!(policy.restart_for(&high), None);
        assert_eq!(
            policy.dependencies_of(&low).count(),
            1,
            "the declared dependency edge resolves for its subject"
        );
        assert_eq!(policy.dependencies_of(&high).count(), 0);
        assert_eq!(policy.parameter_authority(&low, &low), Some(PARAMETER_READ));
        assert_eq!(
            policy.parameter_authority(&high, &low),
            Some(PARAMETER_READ | PARAMETER_WRITE)
        );
        assert_eq!(
            policy.parameter_authority(&low, &high),
            None,
            "an undeclared direction grants nothing"
        );
    }

    #[test]
    fn backoff_grows_per_attempt_and_saturates() {
        let bytes = valid_policy();
        let policy = LifecyclePolicy::decode(&bytes).expect("valid policy");
        let (low, _) = ordered_identities();
        let restart = policy.restart_for(&low).expect("declared restart policy");
        assert_eq!(restart.backoff_for(0), 1000);
        assert_eq!(restart.backoff_for(1), 2000);
        assert_eq!(restart.backoff_for(2), 4000);
        // A factor of 2 applied 64 times would wrap a u64; the ceiling holds.
        assert_eq!(restart.backoff_for(64), MAX_BACKOFF_NS);
    }

    #[test]
    fn a_flat_factor_never_grows_the_delay() {
        let (low, _) = ordered_identities();
        let mut bytes = header(STATE_INITIALIZE, STATE_ERROR, 1, 1, 0, 0);
        bytes.extend(transition(STATE_INITIALIZE, STATE_ERROR));
        bytes.extend(restart(low, 3, cause_bit(CAUSE_FAULT), 500, 256));
        let policy = LifecyclePolicy::decode(&bytes).expect("valid flat policy");
        let restart = policy.restart_for(&low).expect("declared restart policy");
        assert_eq!(restart.backoff_for(0), 500);
        assert_eq!(restart.backoff_for(7), 500);
    }

    #[test]
    fn a_self_transition_does_not_decode() {
        let mut bytes = header(STATE_INITIALIZE, STATE_ERROR, 1, 0, 0, 0);
        bytes.extend(transition(STATE_RUNNING, STATE_RUNNING));
        assert_eq!(
            LifecyclePolicy::decode(&bytes).unwrap_err(),
            DecodeError::SelfTransition
        );
    }

    #[test]
    fn an_undeclared_state_is_not_a_graph_endpoint() {
        let bytes = header(UNDECLARED_STATE_ID, STATE_ERROR, 0, 0, 0, 0);
        assert_eq!(
            LifecyclePolicy::decode(&bytes).unwrap_err(),
            DecodeError::UnknownState
        );
        let bytes = header(STATE_INITIALIZE, UNDECLARED_STATE_ID, 0, 0, 0, 0);
        assert_eq!(
            LifecyclePolicy::decode(&bytes).unwrap_err(),
            DecodeError::UnknownState
        );
    }

    #[test]
    fn an_entry_state_equal_to_the_terminal_state_does_not_decode() {
        let bytes = header(STATE_ERROR, STATE_ERROR, 0, 0, 0, 0);
        assert_eq!(
            LifecyclePolicy::decode(&bytes).unwrap_err(),
            DecodeError::UnreachableState
        );
    }

    #[test]
    fn a_restart_policy_needs_an_edge_into_the_terminal_state() {
        let (low, _) = ordered_identities();
        let mut bytes = header(STATE_INITIALIZE, STATE_ERROR, 1, 1, 0, 0);
        bytes.extend(transition(STATE_INITIALIZE, STATE_RUNNING));
        bytes.extend(restart(low, 1, cause_bit(CAUSE_FAULT), 0, 256));
        assert_eq!(
            LifecyclePolicy::decode(&bytes).unwrap_err(),
            DecodeError::UnreachableState,
            "exhaustion must be able to reach the declared terminal state"
        );
    }

    #[test]
    fn an_attempt_bound_above_the_ceiling_does_not_decode() {
        let (low, _) = ordered_identities();
        let mut bytes = header(STATE_INITIALIZE, STATE_ERROR, 1, 1, 0, 0);
        bytes.extend(transition(STATE_INITIALIZE, STATE_ERROR));
        bytes.extend(restart(
            low,
            MAX_RESTART_ATTEMPTS + 1,
            cause_bit(CAUSE_FAULT),
            0,
            256,
        ));
        assert_eq!(
            LifecyclePolicy::decode(&bytes).unwrap_err(),
            DecodeError::BadRestartBound
        );
    }

    #[test]
    fn an_empty_or_unknown_cause_mask_does_not_decode() {
        let (low, _) = ordered_identities();
        for causes in [0u32, 1 << 20] {
            let mut bytes = header(STATE_INITIALIZE, STATE_ERROR, 1, 1, 0, 0);
            bytes.extend(transition(STATE_INITIALIZE, STATE_ERROR));
            bytes.extend(restart(low, 1, causes, 0, 256));
            assert_eq!(
                LifecyclePolicy::decode(&bytes).unwrap_err(),
                DecodeError::UnknownCause,
                "causes={causes:#x}"
            );
        }
    }

    #[test]
    fn a_shrinking_or_oversized_backoff_factor_does_not_decode() {
        let (low, _) = ordered_identities();
        for factor in [0u32, BACKOFF_FACTOR_SCALE - 1, MAX_BACKOFF_FACTOR + 1] {
            let mut bytes = header(STATE_INITIALIZE, STATE_ERROR, 1, 1, 0, 0);
            bytes.extend(transition(STATE_INITIALIZE, STATE_ERROR));
            bytes.extend(restart(low, 1, cause_bit(CAUSE_FAULT), 10, factor));
            assert_eq!(
                LifecyclePolicy::decode(&bytes).unwrap_err(),
                DecodeError::BadRestartBound,
                "factor={factor}"
            );
        }
    }

    #[test]
    fn a_self_dependency_does_not_decode() {
        let (low, _) = ordered_identities();
        let mut bytes = header(STATE_INITIALIZE, STATE_ERROR, 0, 0, 1, 0);
        bytes.extend(dependency(low, low, STATE_RUNNING));
        assert_eq!(
            LifecyclePolicy::decode(&bytes).unwrap_err(),
            DecodeError::SelfDependency
        );
    }

    #[test]
    fn a_parameter_grant_must_carry_an_authority_bit() {
        let (low, high) = ordered_identities();
        for flags in [0u32, 1 << 8] {
            let mut bytes = header(STATE_INITIALIZE, STATE_ERROR, 0, 0, 0, 1);
            bytes.extend(parameter(low, high, flags));
            assert_eq!(
                LifecyclePolicy::decode(&bytes).unwrap_err(),
                DecodeError::EmptyAuthority,
                "flags={flags:#x}"
            );
        }
    }

    #[test]
    fn a_reflexive_parameter_grant_decodes() {
        let (low, _) = ordered_identities();
        let mut bytes = header(STATE_INITIALIZE, STATE_ERROR, 0, 0, 0, 1);
        bytes.extend(parameter(low, low, PARAMETER_WRITE));
        let policy = LifecyclePolicy::decode(&bytes).expect("reflexive grant is admissible");
        assert_eq!(
            policy.parameter_authority(&low, &low),
            Some(PARAMETER_WRITE),
            "a component may be granted authority over its own parameters"
        );
    }

    #[test]
    fn unsorted_or_duplicate_rows_do_not_decode() {
        let (low, high) = ordered_identities();
        let mut bytes = header(STATE_INITIALIZE, STATE_ERROR, 2, 0, 0, 0);
        bytes.extend(transition(STATE_RUNNING, STATE_ERROR));
        bytes.extend(transition(STATE_INITIALIZE, STATE_RUNNING));
        assert_eq!(
            LifecyclePolicy::decode(&bytes).unwrap_err(),
            DecodeError::BadOrder
        );

        let mut bytes = header(STATE_INITIALIZE, STATE_ERROR, 1, 2, 0, 0);
        bytes.extend(transition(STATE_INITIALIZE, STATE_ERROR));
        bytes.extend(restart(high, 1, cause_bit(CAUSE_FAULT), 0, 256));
        bytes.extend(restart(low, 1, cause_bit(CAUSE_FAULT), 0, 256));
        assert_eq!(
            LifecyclePolicy::decode(&bytes).unwrap_err(),
            DecodeError::BadOrder
        );

        let mut bytes = header(STATE_INITIALIZE, STATE_ERROR, 0, 0, 0, 2);
        bytes.extend(parameter(low, high, PARAMETER_READ));
        bytes.extend(parameter(low, high, PARAMETER_READ));
        assert_eq!(
            LifecyclePolicy::decode(&bytes).unwrap_err(),
            DecodeError::BadOrder
        );
    }

    #[test]
    fn reserved_bytes_must_be_zero() {
        let mut bytes = header(STATE_INITIALIZE, STATE_ERROR, 1, 0, 0, 0);
        let mut edge = transition(STATE_INITIALIZE, STATE_ERROR);
        edge[8] = 1;
        bytes.extend(edge);
        assert_eq!(
            LifecyclePolicy::decode(&bytes).unwrap_err(),
            DecodeError::UnknownRequiredFlags
        );

        let (low, _) = ordered_identities();
        let mut bytes = header(STATE_INITIALIZE, STATE_ERROR, 1, 1, 0, 0);
        bytes.extend(transition(STATE_INITIALIZE, STATE_ERROR));
        let mut row = restart(low, 1, cause_bit(CAUSE_FAULT), 0, 256);
        row[63] = 1;
        bytes.extend(row);
        assert_eq!(
            LifecyclePolicy::decode(&bytes).unwrap_err(),
            DecodeError::UnknownRequiredFlags
        );
    }

    #[test]
    fn envelope_magic_version_and_length_are_checked() {
        let bytes = valid_policy();
        assert_eq!(
            LifecyclePolicy::decode(&bytes[..HEADER_BYTES - 1]).unwrap_err(),
            DecodeError::Truncated
        );

        let mut wrong_magic = bytes.clone();
        wrong_magic[0] = b'X';
        assert_eq!(
            LifecyclePolicy::decode(&wrong_magic).unwrap_err(),
            DecodeError::BadMagic
        );

        let mut wrong_version = bytes.clone();
        wrong_version[8] = FORMAT_VERSION as u8 + 1;
        assert_eq!(
            LifecyclePolicy::decode(&wrong_version).unwrap_err(),
            DecodeError::UnsupportedVersion
        );

        let mut wrong_flags = bytes.clone();
        wrong_flags[16] = 1;
        assert_eq!(
            LifecyclePolicy::decode(&wrong_flags).unwrap_err(),
            DecodeError::UnknownRequiredFlags
        );

        let mut short = bytes.clone();
        short.pop();
        assert_eq!(
            LifecyclePolicy::decode(&short).unwrap_err(),
            DecodeError::BadBounds
        );

        let mut over_bound = header(STATE_INITIALIZE, STATE_ERROR, 0, 0, 0, 0);
        over_bound[28..30].copy_from_slice(&((MAX_TRANSITIONS + 1) as u16).to_le_bytes());
        assert_eq!(
            LifecyclePolicy::decode(&over_bound).unwrap_err(),
            DecodeError::BadBounds
        );
    }

    #[test]
    fn identity_folds_are_domain_separated() {
        assert_ne!(
            instance_identity("worker"),
            crate::scheduling_class::instance_identity("worker"),
            "a scheduling subject identity must not authenticate as a lifecycle subject"
        );
        assert_ne!(
            instance_identity("worker"),
            crate::wait_set::waiter_identity("worker"),
        );
    }

    #[test]
    fn cause_bits_are_disjoint_and_exclude_the_undeclared_id() {
        assert_eq!(cause_bit(UNDECLARED_CAUSE_ID), 0);
        let mut seen = 0u32;
        for cause in [CAUSE_EXIT, CAUSE_FAULT, CAUSE_UNHEALTHY] {
            let bit = cause_bit(cause);
            assert_ne!(bit, 0, "cause {cause} has no bit");
            assert_eq!(seen & bit, 0, "cause {cause} shares a bit");
            seen |= bit;
        }
        assert_eq!(seen, CAUSE_ALL);
    }

    #[test]
    fn undeclared_ids_are_not_declarable() {
        assert!(!is_declared_state(UNDECLARED_STATE_ID));
        assert!(!is_declared_cause(UNDECLARED_CAUSE_ID));
        assert_eq!(state_name(UNDECLARED_STATE_ID), "undeclared");
        assert_eq!(cause_name(UNDECLARED_CAUSE_ID), "live");
    }
}
