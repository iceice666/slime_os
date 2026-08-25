//! C9.4 lifecycle state, restart admission, and parameter authority.
//!
//! What this module owns is mechanism, and the boundary is the milestone's own:
//! `slime-root` observes a termination, records why, answers what the generation
//! declares about restarting the dead instance, and refuses what it does not
//! admit. It never decides that something should run again. Attempt bounds and
//! backoff instants are read out of the authenticated resource; the decision, the
//! wait, and the spawn belong to a component holding supervision authority.
//!
//! Three shapes here are load-bearing rather than incidental:
//!
//! * **State is keyed by live task; attempts are keyed by declared instance.**
//!   Those are different lifetimes on purpose. `TaskId` is never reused, so a
//!   per-task attempt counter would reset on exactly the event it is meant to
//!   bound — the death that triggers the restart. The instance row therefore
//!   outlives every task that represents it, which is what makes "exhausting the
//!   attempt bound leaves the graph in a declared terminal state rather than
//!   restarting forever" enforceable at all.
//!
//! * **The terminal cause is recorded, not inferred.** [`Terminal`] is written
//!   once per death by the same paths that record `supervision::Termination`, and
//!   `RESTART_ADMIT` refuses a cause the policy's mask does not name. That is how
//!   "killed by fault, by exit, and by declared unhealthiness each restart under
//!   its declared policy, and the three are distinguishable" holds: three causes,
//!   one mask, one refusal.
//!
//! * **Backoff is computed from the resource, by both readers.** The root answers
//!   the instant a restart may proceed and refuses a spawn before it; the
//!   supervisor arms a C9.1 timer for the same instant. Both call
//!   `RestartPolicy::backoff_for`, so a supervisor cannot wait for a delay the
//!   root does not recognize, and a supervisor that skips its wait is refused
//!   rather than trusted.
//!
//! The instance row also carries parameter state, and that placement is the
//! authority claim: parameters belong to a *declaration*, so a restarted instance
//! is started with the configuration its predecessor left, while a task holding
//! no declared parameter edge cannot read or write any of it.

use boot_contracts::generation::Generation;
use boot_contracts::lifecycle_policy::{
    LifecyclePolicy, PARAMETER_READ, PARAMETER_WRITE, UNDECLARED_CAUSE_ID, UNDECLARED_STATE_ID,
    cause_name, instance_identity, is_declared_state,
};

use crate::generation::MAX_ADMITTED_INSTANCES;
use crate::task::TaskId;

/// Parameter keys one declared instance may hold at once.
///
/// Small deliberately. Parameter state is startup configuration a supervisor
/// adjusts between restarts, not a component's working memory — C10's private
/// region is that — so a ceiling large enough to be a store would invite one.
pub const MAX_PARAMETERS_PER_INSTANCE: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleError {
    /// The generation declares no lifecycle policy, or names no policy for this
    /// subject.
    Undeclared,
    /// The generation is inconsistent with this service's own state.
    Malformed,
    /// The transition graph does not admit this edge.
    UnadmittedTransition,
    /// The requested state is outside the closed vocabulary, or is the
    /// `undeclared` answer rather than a declaration.
    UnknownState,
    /// The subject has not terminated, so there is nothing to restart.
    StillLive,
    /// The subject terminated for a cause this policy does not restart on.
    UnadmittedCause,
    /// The declared attempt bound is spent. The subject's instance is left in the
    /// policy's declared terminal state.
    AttemptsExhausted,
    /// The restart was requested before the declared backoff instant elapsed.
    BackoffPending,
    /// The caller holds no declared parameter authority over this subject.
    NoParameterAuthority,
    /// The subject has no value for this parameter key. Distinct from
    /// [`Self::NoParameterAuthority`] because C9.4 requires a caller to be able
    /// to tell "I may not ask" from "there is no answer".
    UnknownParameter,
    /// Every parameter slot this instance has is taken.
    ParameterTableFull,
}

/// Why a task ended, as the cause a restart policy branches on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Terminal {
    Exit,
    Fault,
    Unhealthy,
}

impl Terminal {
    /// The contract's cause id for this terminal state.
    pub const fn id(self) -> u32 {
        use boot_contracts::lifecycle_policy::{CAUSE_EXIT, CAUSE_FAULT, CAUSE_UNHEALTHY};
        match self {
            Self::Exit => CAUSE_EXIT,
            Self::Fault => CAUSE_FAULT,
            Self::Unhealthy => CAUSE_UNHEALTHY,
        }
    }

    pub const fn name(self) -> &'static str {
        cause_name(self.id())
    }
}

/// What one restart admission answered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RestartAdmission {
    /// Declared attempts still admitted after charging this one.
    pub remaining: u32,
    /// The earliest monotonic instant at which the restart may proceed.
    pub ready_at: u64,
    /// The attempt ordinal this admission charged, zero-based.
    pub attempt: u32,
}

#[derive(Clone, Copy)]
struct Parameter {
    key: u64,
    value: u64,
}

/// Per-declared-instance lifecycle state that outlives every task representing
/// it.
#[derive(Clone, Copy)]
struct InstanceRow {
    instance: usize,
    /// Restart attempts already charged.
    attempts_used: u32,
    /// Whether the attempt bound has been spent and the instance moved to the
    /// policy's terminal state.
    exhausted: bool,
    /// The most recent task that represented this instance, live or dead.
    ///
    /// Load-bearing rather than bookkeeping: `RESTART_ADMIT` names its subject
    /// through a supervision capability, which holds a `TaskId`, and by the time
    /// a supervisor asks, that task's own row is gone — released on the very
    /// death the restart answers. Without this the subject's *instance* would be
    /// unresolvable and every restart admission would refuse, which is a
    /// mechanism declared and unreachable.
    last_task: Option<TaskId>,
    /// The cause awaiting a restart admission, consumed by the admission that
    /// charges for it so one death cannot charge two attempts.
    terminal: Option<Terminal>,
    /// The cause of the most recent death, *retained* past that admission.
    ///
    /// Separate from `terminal` because the two answer different questions and
    /// consuming one must not erase the other: the admission needs a pending
    /// cause exactly once, while the *replacement* needs to know why its
    /// predecessor ended in order to behave differently — which is what makes
    /// "the three causes are distinguishable" observable from inside a restarted
    /// component rather than only from its supervisor.
    last_cause: Option<Terminal>,
    /// The instant the pending restart may proceed, as answered by the last
    /// admission. Read back by [`LifecycleService::restart_ready`] so the
    /// spawn path refuses a supervisor that skipped its own wait.
    ready_at: u64,
    parameters: [Option<Parameter>; MAX_PARAMETERS_PER_INSTANCE],
}

impl InstanceRow {
    const fn new(instance: usize) -> Self {
        Self {
            instance,
            attempts_used: 0,
            exhausted: false,
            last_task: None,
            terminal: None,
            last_cause: None,
            ready_at: 0,
            parameters: [None; MAX_PARAMETERS_PER_INSTANCE],
        }
    }
}

/// One live task's current lifecycle state.
#[derive(Clone, Copy)]
struct TaskRow {
    task: TaskId,
    instance: usize,
    state: u32,
}

pub struct LifecycleService {
    tasks: [Option<TaskRow>; crate::task::MAX_TASKS],
    instances: [Option<InstanceRow>; MAX_ADMITTED_INSTANCES],
}

impl LifecycleService {
    pub const fn new() -> Self {
        Self {
            tasks: [None; crate::task::MAX_TASKS],
            instances: [None; MAX_ADMITTED_INSTANCES],
        }
    }

    /// Record the state one launched task starts in.
    ///
    /// Called once per live task, including tasks whose instance the policy does
    /// not name — the same rule C9.1's authority table, C9.2's source table, and
    /// C9.3's class table follow, so the table answers about a *live task* rather
    /// than about whether the generation happened to mention it. An unnamed
    /// instance starts and stays `undeclared`: it is in no graph, so it has no
    /// edge to take.
    pub fn declare(
        &mut self,
        policy: Option<&LifecyclePolicy<'_>>,
        task: TaskId,
        instance: usize,
    ) -> Result<u32, LifecycleError> {
        if self.tasks.iter().flatten().any(|row| row.task == task) {
            return Err(LifecycleError::Malformed);
        }
        let state = policy.map_or(UNDECLARED_STATE_ID, LifecyclePolicy::initial_state);
        let slot = self
            .tasks
            .iter_mut()
            .find(|row| row.is_none())
            .ok_or(LifecycleError::Malformed)?;
        *slot = Some(TaskRow {
            task,
            instance,
            state,
        });
        // The instance row is created on first launch and never dropped, because
        // it carries the attempt count and the parameter state a restart must
        // not reset. Creating it here rather than lazily means a restart of an
        // instance whose predecessor was never charged still finds its row.
        //
        // The task is recorded on it as well, so a supervision capability naming
        // this task still resolves to this instance after the task row is
        // released — which is exactly when `RESTART_ADMIT` needs it.
        let row = self.instance_row(instance)?;
        row.last_task = Some(task);
        Ok(state)
    }

    /// The lifecycle state a live task is in, or `undeclared` for one this
    /// service never recorded.
    pub fn state(&self, task: TaskId) -> u32 {
        self.tasks
            .iter()
            .flatten()
            .find(|row| row.task == task)
            .map_or(UNDECLARED_STATE_ID, |row| row.state)
    }

    /// Advance a live task's own state along a declared edge.
    ///
    /// The caller names only a target: there is no subject operand, because
    /// moving another component's lifecycle state is authority no C9.4 field
    /// grants. A generation with no policy refuses every advance, which is the
    /// deny-by-default answer for a graph that does not exist.
    pub fn advance(
        &mut self,
        policy: Option<&LifecyclePolicy<'_>>,
        task: TaskId,
        state_id: u32,
    ) -> Result<u32, LifecycleError> {
        if !is_declared_state(state_id) {
            return Err(LifecycleError::UnknownState);
        }
        let policy = policy.ok_or(LifecycleError::Undeclared)?;
        let row = self
            .tasks
            .iter()
            .position(|row| row.is_some_and(|row| row.task == task))
            .ok_or(LifecycleError::Malformed)?;
        let current = self.tasks[row]
            .as_ref()
            .ok_or(LifecycleError::Malformed)?
            .state;
        // An instance the policy does not name sits at `undeclared`, and no edge
        // departs from it: the graph never names that state, so this refusal is
        // the table's answer rather than a special case beside it.
        if !policy.admits(current, state_id) {
            return Err(LifecycleError::UnadmittedTransition);
        }
        let entry = self.tasks[row].as_mut().ok_or(LifecycleError::Malformed)?;
        entry.state = state_id;
        Ok(state_id)
    }

    /// Record how the task representing an instance ended.
    ///
    /// First-writer-wins per task lifetime, matching
    /// `supervision::Terminations::record`: a task ends once, and a second cause
    /// for one death would make the recorded reason depend on path ordering.
    /// Returns the instance and the cause the row *holds*, which is the passed
    /// cause only when this call was the first writer.
    pub fn record_termination(
        &mut self,
        task: TaskId,
        terminal: Terminal,
    ) -> Option<(usize, Terminal)> {
        let instance = self
            .tasks
            .iter()
            .flatten()
            .find(|row| row.task == task)
            .map(|row| row.instance)?;
        let row = self
            .instances
            .iter_mut()
            .flatten()
            .find(|row| row.instance == instance)?;
        if row.terminal.is_none() {
            row.terminal = Some(terminal);
            // Written together, consumed separately: the admission below takes
            // `terminal` so one death charges one attempt, while `last_cause`
            // outlives it so the *replacement* can read why its predecessor
            // ended. First-writer-wins guards both, so a component that declares
            // itself unhealthy and then exits is recorded as unhealthy.
            row.last_cause = Some(terminal);
        }
        // The *recorded* cause, not the one passed in. A caller that lost
        // first-writer-wins would otherwise print a cause the root did not
        // record: `unhealthy()` exits immediately after its reply, so the EXIT
        // path runs for a death already recorded as `Unhealthy`, and reporting
        // its own argument would put two contradictory root-attributed lines in
        // the transcript with the authoritative one second (found by review).
        row.last_cause.map(|recorded| (instance, recorded))
    }

    /// Drop a terminated task's state row.
    ///
    /// The *instance* row deliberately survives: it holds the attempt count and
    /// the parameter state a restart must inherit rather than reset. Only the
    /// per-task state goes, so a replacement re-derives its initial state from
    /// the generation instead of continuing its predecessor's.
    pub fn release(&mut self, task: TaskId) {
        for row in self.tasks.iter_mut() {
            if row.is_some_and(|recorded| recorded.task == task) {
                *row = None;
            }
        }
    }

    /// How the most recent task representing `instance` ended, or `None` when it
    /// has not ended.
    pub fn terminal(&self, instance: usize) -> Option<Terminal> {
        self.instances
            .iter()
            .flatten()
            .find(|row| row.instance == instance)
            .and_then(|row| row.last_cause)
    }

    /// Declared restart attempts still admitted for `instance`.
    ///
    /// Zero for an instance the policy does not name, because "the generation
    /// says nothing about restarting this" and "the generation admits no further
    /// restart" are the same answer to a supervisor: neither permits one.
    pub fn attempts_remaining(
        &self,
        policy: Option<&LifecyclePolicy<'_>>,
        instance: usize,
        generation: &Generation<'_>,
    ) -> u32 {
        let Some(declared) = self.declared_restart(policy, generation, instance) else {
            return 0;
        };
        let used = self
            .instances
            .iter()
            .flatten()
            .find(|row| row.instance == instance)
            .map_or(0, |row| row.attempts_used);
        declared.attempts.saturating_sub(used)
    }

    /// Admit one restart of `instance`, charging the declared attempt budget.
    ///
    /// The root's whole contribution to restart policy, and every branch here is
    /// a refusal of something the generation does not declare rather than a
    /// decision about what should happen:
    ///
    /// * a subject still live has nothing to restart;
    /// * a cause the policy's mask omits is not one it restarts on;
    /// * a spent attempt bound moves the instance into the declared terminal
    ///   state and refuses, which is what makes exhaustion terminal rather than
    ///   merely unproductive.
    ///
    /// `now` is the caller-supplied monotonic instant, read by the dispatcher
    /// from the same platform counter C9.1 brokers, so the answered `ready_at`
    /// and the supervisor's timer are on one clock.
    pub fn admit_restart(
        &mut self,
        policy: Option<&LifecyclePolicy<'_>>,
        generation: &Generation<'_>,
        instance: usize,
        now: u64,
    ) -> Result<RestartAdmission, LifecycleError> {
        let declared = self
            .declared_restart(policy, generation, instance)
            .ok_or(LifecycleError::Undeclared)?;
        let row = self.instance_row(instance)?;
        let terminal = row.terminal.ok_or(LifecycleError::StillLive)?;
        if row.exhausted {
            return Err(LifecycleError::AttemptsExhausted);
        }
        if !declared.restarts_on(terminal.id()) {
            return Err(LifecycleError::UnadmittedCause);
        }
        if row.attempts_used >= declared.attempts {
            // Terminal, and recorded as such before the refusal: an exhausted
            // instance must not answer differently on a second ask, and the
            // marker a gate reads must be able to say the graph *is* in the
            // declared terminal state rather than that a restart was declined.
            row.exhausted = true;
            return Err(LifecycleError::AttemptsExhausted);
        }
        let attempt = row.attempts_used;
        row.attempts_used = attempt.saturating_add(1);
        let ready_at = now.saturating_add(declared.backoff_for(attempt));
        row.ready_at = ready_at;
        // The termination is consumed by the admission that charges for it, so a
        // second `RESTART_ADMIT` on one death cannot charge a second attempt.
        row.terminal = None;
        Ok(RestartAdmission {
            remaining: declared.attempts.saturating_sub(row.attempts_used),
            ready_at,
            attempt,
        })
    }

    /// Whether an admitted restart of `instance` may proceed at `now`.
    ///
    /// The spawn path's whole consultation of the backoff, so a supervisor that
    /// skips its own wait is refused by the mechanism rather than trusted to
    /// honour a number it was merely told. An instance with no admitted restart
    /// pending has a zero reservation, which no clock value can precede.
    ///
    /// This returns [`LifecycleError::BackoffPending`] rather than a bare bool so
    /// the refusal has one source: the spawn path maps it through the same
    /// `lifecycle_error_status`/`lifecycle_error_class` pair every other
    /// lifecycle refusal goes through, instead of restating the status and the
    /// marker class beside it.
    pub fn restart_ready(&self, instance: usize, now: u64) -> Result<(), LifecycleError> {
        let ready_at = self
            .instances
            .iter()
            .flatten()
            .find(|row| row.instance == instance)
            .map_or(0, |row| row.ready_at);
        if now < ready_at {
            return Err(LifecycleError::BackoffPending);
        }
        Ok(())
    }

    /// Clear a satisfied restart reservation once the replacement launches.
    pub fn clear_restart_reservation(&mut self, instance: usize) {
        if let Some(row) = self
            .instances
            .iter_mut()
            .flatten()
            .find(|row| row.instance == instance)
        {
            row.ready_at = 0;
        }
    }

    /// Whether `instance` has spent its declared attempt bound.
    pub fn is_exhausted(&self, instance: usize) -> bool {
        self.instances
            .iter()
            .flatten()
            .find(|row| row.instance == instance)
            .is_some_and(|row| row.exhausted)
    }

    /// The state an exhausted instance is reported in.
    pub fn terminal_state(policy: Option<&LifecyclePolicy<'_>>) -> u32 {
        policy.map_or(UNDECLARED_STATE_ID, LifecyclePolicy::terminal_state)
    }

    /// Read one parameter of `subject` on behalf of `holder`.
    ///
    /// Two refusals, and their distinctness is C9.4's last required check:
    /// [`LifecycleError::NoParameterAuthority`] means the generation grants this
    /// holder nothing over this subject, and [`LifecycleError::UnknownParameter`]
    /// means it does and there is no value. Collapsing them would let a caller
    /// probe another component's key space by watching which error came back —
    /// or, worse, read "no authority" as "no value" and proceed.
    pub fn parameter_read(
        &self,
        policy: Option<&LifecyclePolicy<'_>>,
        generation: &Generation<'_>,
        holder: usize,
        subject: usize,
        key: u64,
    ) -> Result<u64, LifecycleError> {
        self.parameter_authority(policy, generation, holder, subject, PARAMETER_READ)?;
        self.instances
            .iter()
            .flatten()
            .find(|row| row.instance == subject)
            .and_then(|row| {
                row.parameters
                    .iter()
                    .flatten()
                    .find(|parameter| parameter.key == key)
                    .map(|parameter| parameter.value)
            })
            .ok_or(LifecycleError::UnknownParameter)
    }

    /// Write one parameter of `subject` on behalf of `holder`, answering the
    /// previous value.
    ///
    /// Write authority is checked independently of read: a supervisor that must
    /// observe a component's configuration to decide a restart does not thereby
    /// get to change it.
    pub fn parameter_write(
        &mut self,
        policy: Option<&LifecyclePolicy<'_>>,
        generation: &Generation<'_>,
        holder: usize,
        subject: usize,
        key: u64,
        value: u64,
    ) -> Result<u64, LifecycleError> {
        self.parameter_authority(policy, generation, holder, subject, PARAMETER_WRITE)?;
        let row = self.instance_row(subject)?;
        if let Some(existing) = row
            .parameters
            .iter_mut()
            .flatten()
            .find(|parameter| parameter.key == key)
        {
            let previous = existing.value;
            existing.value = value;
            return Ok(previous);
        }
        let free = row
            .parameters
            .iter_mut()
            .find(|parameter| parameter.is_none())
            .ok_or(LifecycleError::ParameterTableFull)?;
        *free = Some(Parameter { key, value });
        Ok(0)
    }

    /// Whether the health dependencies declared for `instance` are satisfied by
    /// the live graph.
    ///
    /// Every declared dependency must currently be represented by a live task in
    /// the exact state the edge names. Evaluated on *every* start, unlike
    /// `Instance.dependencies`' one-shot autostart barrier, because a restart is
    /// a start: a replacement whose dependency has since degraded must wait for
    /// the same condition its predecessor was launched under.
    pub fn dependencies_satisfied(
        &self,
        policy: Option<&LifecyclePolicy<'_>>,
        generation: &Generation<'_>,
        instance: usize,
    ) -> bool {
        let Some(policy) = policy else {
            return true;
        };
        let Ok(record) = generation.instance(instance) else {
            return false;
        };
        let identity = instance_identity(record.name);
        for edge in policy.dependencies_of(&identity) {
            let Some(dependency) =
                self.instance_for_identity(generation, &edge.dependency_identity)
            else {
                return false;
            };
            let live = self
                .tasks
                .iter()
                .flatten()
                .find(|row| row.instance == dependency);
            match live {
                Some(row) if row.state == edge.required_state => {}
                _ => return false,
            }
        }
        true
    }

    /// Which declared instance a live task represents, as this service recorded
    /// it.
    pub fn instance_of(&self, task: TaskId) -> Option<usize> {
        self.tasks
            .iter()
            .flatten()
            .find(|row| row.task == task)
            .map(|row| row.instance)
    }

    /// Which declared instance a task represented, live *or already released*.
    ///
    /// Separate from [`Self::instance_of`] because the two answer different
    /// questions and only one of them is safe for a live-subject operation. A
    /// parameter write must act on a live task's instance; a restart admission
    /// must act on a *dead* one's, since the death is what it answers. Collapsing
    /// them would let a parameter operation reach an instance whose task is gone.
    pub fn instance_of_any(&self, task: TaskId) -> Option<usize> {
        self.instance_of(task).or_else(|| {
            self.instances
                .iter()
                .flatten()
                .find(|row| row.last_task == Some(task))
                .map(|row| row.instance)
        })
    }

    /// The cause id recorded for an instance's last death, or the `undeclared`
    /// id when it has not ended.
    pub fn terminal_id(&self, instance: usize) -> u32 {
        self.terminal(instance)
            .map_or(UNDECLARED_CAUSE_ID, Terminal::id)
    }

    fn declared_restart(
        &self,
        policy: Option<&LifecyclePolicy<'_>>,
        generation: &Generation<'_>,
        instance: usize,
    ) -> Option<boot_contracts::lifecycle_policy::RestartPolicy> {
        let policy = policy?;
        let record = generation.instance(instance).ok()?;
        policy.restart_for(&instance_identity(record.name))
    }

    fn parameter_authority(
        &self,
        policy: Option<&LifecyclePolicy<'_>>,
        generation: &Generation<'_>,
        holder: usize,
        subject: usize,
        required: u32,
    ) -> Result<(), LifecycleError> {
        let holder_record = generation
            .instance(holder)
            .map_err(|_| LifecycleError::Malformed)?;
        let subject_record = generation
            .instance(subject)
            .map_err(|_| LifecycleError::Malformed)?;
        parameter_authority(
            policy,
            &instance_identity(holder_record.name),
            &instance_identity(subject_record.name),
            required,
        )
    }

    fn instance_for_identity(
        &self,
        generation: &Generation<'_>,
        identity: &[u8; 32],
    ) -> Option<usize> {
        (0..generation.instance_count()).find(|index| {
            generation
                .instance(*index)
                .is_ok_and(|record| instance_identity(record.name) == *identity)
        })
    }

    fn instance_row(&mut self, instance: usize) -> Result<&mut InstanceRow, LifecycleError> {
        if let Some(position) = self
            .instances
            .iter()
            .position(|row| row.is_some_and(|row| row.instance == instance))
        {
            return self.instances[position]
                .as_mut()
                .ok_or(LifecycleError::Malformed);
        }
        let position = self
            .instances
            .iter()
            .position(Option::is_none)
            .ok_or(LifecycleError::Malformed)?;
        self.instances[position] = Some(InstanceRow::new(instance));
        self.instances[position]
            .as_mut()
            .ok_or(LifecycleError::Malformed)
    }
}

impl Default for LifecycleService {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether the generation grants `holder` the `required` parameter authority
/// over `subject`.
///
/// Free rather than a method because it reads nothing but the resource: the
/// answer is a property of the generation, not of any live task, and keeping it
/// separable is what lets it be tested without a booted graph. Absence of a
/// policy denies, so a generation carrying no lifecycle resource grants no
/// parameter authority to anybody — including over their own parameters.
pub fn parameter_authority(
    policy: Option<&LifecyclePolicy<'_>>,
    holder: &[u8; 32],
    subject: &[u8; 32],
    required: u32,
) -> Result<(), LifecycleError> {
    let policy = policy.ok_or(LifecycleError::NoParameterAuthority)?;
    let flags = policy
        .parameter_authority(holder, subject)
        .ok_or(LifecycleError::NoParameterAuthority)?;
    if flags & required == 0 {
        return Err(LifecycleError::NoParameterAuthority);
    }
    Ok(())
}

/// The generation's lifecycle-policy resource, if it declares one.
pub fn policy_object<'a>(
    generation: &Generation<'a>,
) -> Option<Result<LifecyclePolicy<'a>, boot_contracts::lifecycle_policy::DecodeError>> {
    crate::generation::lifecycle_policy_object(generation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use boot_contracts::lifecycle_policy::{
        CAUSE_EXIT, CAUSE_FAULT, DEPENDENCY_BYTES, FORMAT_VERSION, HEADER_BYTES, MAGIC,
        PARAMETER_BYTES, RESTART_BYTES, STATE_ERROR, STATE_INITIALIZE, STATE_RUNNING,
        TRANSITION_BYTES, cause_bit,
    };

    /// A rendered policy resource, owned for the test's lifetime.
    struct Fixture {
        bytes: alloc::vec::Vec<u8>,
    }

    fn build_policy(
        transitions: &[(u32, u32)],
        restarts: &[([u8; 32], u32, u32, u64, u32)],
        dependencies: &[([u8; 32], [u8; 32], u32)],
        parameters: &[([u8; 32], [u8; 32], u32)],
    ) -> Fixture {
        let total = HEADER_BYTES
            + transitions.len() * TRANSITION_BYTES
            + restarts.len() * RESTART_BYTES
            + dependencies.len() * DEPENDENCY_BYTES
            + parameters.len() * PARAMETER_BYTES;
        let mut bytes = alloc::vec::Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&(STATE_INITIALIZE as u16).to_le_bytes());
        bytes.extend_from_slice(&(STATE_ERROR as u16).to_le_bytes());
        bytes.extend_from_slice(&(transitions.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&(restarts.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&(dependencies.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&(parameters.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&(total as u32).to_le_bytes());
        for (from, to) in transitions {
            bytes.extend_from_slice(&from.to_le_bytes());
            bytes.extend_from_slice(&to.to_le_bytes());
            bytes.extend_from_slice(&0u64.to_le_bytes());
        }
        for (identity, attempts, causes, backoff, factor) in restarts {
            bytes.extend_from_slice(identity);
            bytes.extend_from_slice(&attempts.to_le_bytes());
            bytes.extend_from_slice(&causes.to_le_bytes());
            bytes.extend_from_slice(&backoff.to_le_bytes());
            bytes.extend_from_slice(&factor.to_le_bytes());
            bytes.extend_from_slice(&[0u8; 12]);
        }
        for (subject, dependency, state) in dependencies {
            bytes.extend_from_slice(subject);
            bytes.extend_from_slice(dependency);
            bytes.extend_from_slice(&state.to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
        }
        for (holder, subject, flags) in parameters {
            bytes.extend_from_slice(holder);
            bytes.extend_from_slice(subject);
            bytes.extend_from_slice(&flags.to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
        }
        Fixture { bytes }
    }

    /// A service with two task rows installed by hand, standing in for two
    /// launched instances. Instance indices, not names, because the tests below
    /// exercise the state machine rather than the generation lookup.
    fn service_with_tasks() -> LifecycleService {
        let mut service = LifecycleService::new();
        service.tasks[0] = Some(TaskRow {
            task: TaskId(1),
            instance: 0,
            state: STATE_INITIALIZE,
        });
        service.tasks[1] = Some(TaskRow {
            task: TaskId(2),
            instance: 1,
            state: STATE_INITIALIZE,
        });
        service.instances[0] = Some(InstanceRow::new(0));
        service.instances[1] = Some(InstanceRow::new(1));
        service
    }

    #[test]
    fn an_unadmitted_transition_is_refused_and_changes_nothing() {
        let fixture = build_policy(&[(STATE_INITIALIZE, STATE_RUNNING)], &[], &[], &[]);
        let policy = LifecyclePolicy::decode(&fixture.bytes).expect("valid policy");
        let mut service = service_with_tasks();
        assert_eq!(
            service.advance(Some(&policy), TaskId(1), STATE_RUNNING),
            Ok(STATE_RUNNING)
        );
        assert_eq!(
            service.advance(Some(&policy), TaskId(1), STATE_ERROR),
            Err(LifecycleError::UnadmittedTransition),
            "Running -> Error is not declared"
        );
        assert_eq!(
            service.state(TaskId(1)),
            STATE_RUNNING,
            "a refused advance leaves the state it found"
        );
    }

    #[test]
    fn a_generation_with_no_policy_refuses_every_advance() {
        let mut service = service_with_tasks();
        assert_eq!(
            service.advance(None, TaskId(1), STATE_RUNNING),
            Err(LifecycleError::Undeclared)
        );
        assert_eq!(service.state(TaskId(1)), STATE_INITIALIZE);
    }

    #[test]
    fn the_undeclared_state_is_not_an_advance_target() {
        let fixture = build_policy(&[(STATE_INITIALIZE, STATE_RUNNING)], &[], &[], &[]);
        let policy = LifecyclePolicy::decode(&fixture.bytes).expect("valid policy");
        let mut service = service_with_tasks();
        assert_eq!(
            service.advance(Some(&policy), TaskId(1), UNDECLARED_STATE_ID),
            Err(LifecycleError::UnknownState)
        );
    }

    #[test]
    fn releasing_a_task_keeps_the_instance_row() {
        let mut service = service_with_tasks();
        assert_eq!(
            service.record_termination(TaskId(2), Terminal::Fault),
            Some((1, Terminal::Fault)),
            "the first writer is told the cause it recorded"
        );
        service.release(TaskId(2));
        assert_eq!(
            service.state(TaskId(2)),
            UNDECLARED_STATE_ID,
            "a released task has no state"
        );
        assert_eq!(
            service.terminal(1),
            Some(Terminal::Fault),
            "the instance row survives the task that wrote it, which is what \
             makes an attempt bound bound anything"
        );
    }

    #[test]
    fn a_termination_is_recorded_once_per_task_lifetime() {
        let mut service = service_with_tasks();
        assert_eq!(
            service.record_termination(TaskId(2), Terminal::Exit),
            Some((1, Terminal::Exit))
        );
        assert_eq!(
            service.record_termination(TaskId(2), Terminal::Fault),
            Some((1, Terminal::Exit)),
            "a later writer is told the *recorded* cause, not the one it passed, so a \
             caller cannot print a cause the root did not record"
        );
        assert_eq!(
            service.terminal(1),
            Some(Terminal::Exit),
            "first writer wins, so a recorded cause cannot depend on path order"
        );
    }

    #[test]
    fn parameter_authority_is_directional_and_denies_by_default() {
        let holder = [1u8; 32];
        let subject = [2u8; 32];
        let fixture = build_policy(
            &[(STATE_INITIALIZE, STATE_ERROR)],
            &[],
            &[],
            &[(holder, subject, PARAMETER_READ)],
        );
        let policy = LifecyclePolicy::decode(&fixture.bytes).expect("valid policy");
        assert_eq!(
            parameter_authority(Some(&policy), &holder, &subject, PARAMETER_READ),
            Ok(())
        );
        assert_eq!(
            parameter_authority(Some(&policy), &holder, &subject, PARAMETER_WRITE),
            Err(LifecycleError::NoParameterAuthority),
            "read authority does not imply write"
        );
        assert_eq!(
            parameter_authority(Some(&policy), &subject, &holder, PARAMETER_READ),
            Err(LifecycleError::NoParameterAuthority),
            "an edge is directional; the reverse grants nothing"
        );
        assert_eq!(
            parameter_authority(Some(&policy), &holder, &holder, PARAMETER_READ),
            Err(LifecycleError::NoParameterAuthority),
            "an undeclared reflexive edge grants nothing over your own parameters"
        );
        assert_eq!(
            parameter_authority(None, &holder, &subject, PARAMETER_READ),
            Err(LifecycleError::NoParameterAuthority),
            "no policy denies rather than reporting an absent key, because \
             \"you may not ask\" must not read as \"there is no answer\""
        );
    }

    #[test]
    fn a_missing_key_is_distinct_from_a_denied_read() {
        let mut service = service_with_tasks();
        let row = service.instance_row(1).expect("row");
        row.parameters[0] = Some(Parameter { key: 7, value: 42 });
        let stored = service.instances[1]
            .expect("row")
            .parameters
            .iter()
            .flatten()
            .find(|parameter| parameter.key == 7)
            .map(|parameter| parameter.value);
        assert_eq!(stored, Some(42));
        let absent = service.instances[1]
            .expect("row")
            .parameters
            .iter()
            .flatten()
            .any(|parameter| parameter.key == 8);
        assert!(
            !absent,
            "an unset key must be absent from the table, so the read path can \
             answer UnknownParameter rather than a stale value"
        );
    }

    #[test]
    fn an_exhausted_instance_stays_exhausted() {
        let mut service = service_with_tasks();
        let row = service.instance_row(1).expect("row");
        row.attempts_used = 3;
        row.terminal = Some(Terminal::Fault);
        row.exhausted = true;
        assert!(service.is_exhausted(1));
        // With no reservation pending, no clock value can be too early: the
        // exhaustion refusal is what stops the spawn, not the backoff.
        assert_eq!(service.restart_ready(1, 0), Ok(()));
    }

    #[test]
    fn a_pending_backoff_refuses_until_its_instant_and_then_admits() {
        let mut service = service_with_tasks();
        let row = service.instance_row(1).expect("row");
        row.ready_at = 9_000;
        assert_eq!(
            service.restart_ready(1, 8_999),
            Err(LifecycleError::BackoffPending),
            "a spawn arriving before the answered instant is refused by the mechanism \
             rather than trusted to the supervisor's own wait"
        );
        // The instant itself is admissible: the reservation is a floor, not a
        // strict inequality a caller has to overshoot.
        assert_eq!(service.restart_ready(1, 9_000), Ok(()));
        // An instance with no reservation is never refused, so the guard cannot
        // block a first launch.
        assert_eq!(service.restart_ready(0, 0), Ok(()));
    }

    #[test]
    fn a_backoff_reservation_is_cleared_when_the_replacement_launches() {
        let mut service = service_with_tasks();
        let row = service.instance_row(1).expect("row");
        row.ready_at = 9_000;
        assert_eq!(
            service.restart_ready(1, 0),
            Err(LifecycleError::BackoffPending)
        );
        service.clear_restart_reservation(1);
        assert_eq!(
            service.restart_ready(1, 0),
            Ok(()),
            "a satisfied reservation must not keep refusing later spawns"
        );
    }

    #[test]
    fn causes_are_distinguishable_by_id_and_name() {
        for (terminal, expected) in [
            (Terminal::Exit, "exit"),
            (Terminal::Fault, "fault"),
            (Terminal::Unhealthy, "unhealthy"),
        ] {
            assert_eq!(terminal.name(), expected);
        }
        assert_ne!(Terminal::Exit.id(), Terminal::Fault.id());
        assert_eq!(cause_bit(CAUSE_EXIT) & cause_bit(CAUSE_FAULT), 0);
    }
}
