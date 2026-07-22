use alloc::vec::Vec;
use spin::{LazyLock, Mutex};

use boot_contracts::bootstate::BootState;
use boot_contracts::generation::{
    Generation, POLICY_DISCARD_ON_ROLLBACK, POLICY_EPHEMERAL, POLICY_IMMUTABLE, POLICY_PRESERVE,
    POLICY_SNAPSHOT_BEFORE_UPGRADE,
};

pub const MAX_ROOTS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthState {
    RunningKnownGood,
    RunningPending,
    Confirmed,
    Unhealthy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateAction {
    Reuse,
    CreateEmpty,
    Snapshot,
    DiscardOnRollback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatePlan<'a> {
    pub name: &'a str,
    pub schema_version: u32,
    pub action: StateAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootSet {
    roots: [[u8; 32]; MAX_ROOTS],
    len: usize,
}

impl RootSet {
    pub const fn new() -> Self {
        Self {
            roots: [[0; 32]; MAX_ROOTS],
            len: 0,
        }
    }

    pub fn insert(&mut self, root: [u8; 32]) -> bool {
        if root == [0; 32] || self.contains(root) {
            return true;
        }
        if self.len == self.roots.len() {
            return false;
        }
        self.roots[self.len] = root;
        self.len += 1;
        true
    }

    pub fn contains(&self, root: [u8; 32]) -> bool {
        self.roots[..self.len].contains(&root)
    }

    pub fn remove(&mut self, root: [u8; 32]) -> bool {
        let Some(index) = self.roots[..self.len]
            .iter()
            .position(|candidate| *candidate == root)
        else {
            return false;
        };
        self.len -= 1;
        self.roots[index] = self.roots[self.len];
        self.roots[self.len] = [0; 32];
        true
    }

    pub fn as_slice(&self) -> &[[u8; 32]] {
        &self.roots[..self.len]
    }
}

impl Default for RootSet {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
struct Manager {
    bootstate: Option<BootState>,
    running: [u8; 32],
    health: HealthState,
    rollback_roots: RootSet,
    staged_roots: RootSet,
    persistent_state_roots: RootSet,
    accepted_release_sequence: u64,
    running_release_sequence: u64,
}

impl Manager {
    const fn new() -> Self {
        Self {
            bootstate: None,
            running: [0; 32],
            health: HealthState::RunningKnownGood,
            rollback_roots: RootSet::new(),
            staged_roots: RootSet::new(),
            persistent_state_roots: RootSet::new(),
            accepted_release_sequence: 0,
            running_release_sequence: 0,
        }
    }

    /// True if `object` is reachable from any retained root: the current
    /// BootState (known-good, pending, generation and state roots), the
    /// running generation, or any rollback, staged, or persistent-state
    /// root. Checked directly against every source so no root is ever
    /// dropped by a bounded intermediate set.
    fn retains(&self, object: &[u8; 32]) -> bool {
        if let Some(state) = self.bootstate
            && (state.known_good == *object
                || state.pending == Some(*object)
                || state.generation_root == *object
                || state.state_root == *object)
        {
            return true;
        }
        self.running == *object
            || self.rollback_roots.contains(*object)
            || self.staged_roots.contains(*object)
            || self.persistent_state_roots.contains(*object)
    }
}

static MANAGER: LazyLock<Mutex<Manager>> = LazyLock::new(|| Mutex::new(Manager::new()));

pub fn init() {
    let mut manager = MANAGER.lock();
    manager.running = crate::boot::generation_identity();
    if let Some(context) = crate::boot::bootstate() {
        manager.bootstate = Some(BootState {
            sequence: context.sequence,
            known_good: context.known_good,
            pending: context.pending,
            remaining_attempts: context.remaining_attempts,
            generation_root: context.generation_root,
            state_root: context.state_root,
            accepted_release_sequence: context.accepted_release_sequence,
        });
        manager.accepted_release_sequence = context.accepted_release_sequence;
        manager.running_release_sequence = context.running_release_sequence;
        manager.health = if context.running_pending {
            HealthState::RunningPending
        } else {
            HealthState::RunningKnownGood
        };
        let _ = manager.persistent_state_roots.insert(context.state_root);
    }
}

pub fn health_state() -> HealthState {
    MANAGER.lock().health
}

pub fn confirm_running_pending() -> bool {
    let mut manager = MANAGER.lock();
    let Some(state) = manager.bootstate else {
        return false;
    };
    let Ok(promoted) = state.promote_pending(manager.running, manager.running_release_sequence)
    else {
        return false;
    };
    let previous = state.known_good;
    if !manager.rollback_roots.insert(previous) {
        return false;
    }
    manager.bootstate = Some(promoted);
    manager.accepted_release_sequence = promoted.accepted_release_sequence;
    manager.health = HealthState::Confirmed;
    true
}

pub fn mark_unhealthy() {
    MANAGER.lock().health = HealthState::Unhealthy;
}

pub fn retain_staged(root: [u8; 32]) -> bool {
    MANAGER.lock().staged_roots.insert(root)
}

pub fn is_staged(root: [u8; 32]) -> bool {
    MANAGER.lock().staged_roots.contains(root)
}

pub fn remove_staged(root: [u8; 32]) {
    MANAGER.lock().staged_roots.remove(root);
}

pub fn record_bootstate(state: BootState) {
    let mut manager = MANAGER.lock();
    manager.bootstate = Some(state);
    manager.accepted_release_sequence = state.accepted_release_sequence;
}

pub fn accepted_release_sequence() -> u64 {
    MANAGER.lock().accepted_release_sequence
}

pub fn retain_persistent_state(root: [u8; 32]) -> bool {
    MANAGER.lock().persistent_state_roots.insert(root)
}

pub fn collect_unreachable(sealed: &[[u8; 32]]) -> Vec<[u8; 32]> {
    // Reachability is tested against every retained root directly: the six
    // root categories can together exceed `MAX_ROOTS`, so a merged fixed-size
    // set could drop a root and report a still-reachable object as
    // collectable. `retains` checks each source and never drops.
    let manager = MANAGER.lock();
    sealed
        .iter()
        .copied()
        .filter(|object| !manager.retains(object))
        .collect()
}

pub fn state_plan<'a>(generation: &'a Generation<'a>, rollback: bool) -> Vec<StatePlan<'a>> {
    let mut plan = Vec::new();
    for index in 0..generation.state_count() {
        let Ok(state) = generation.state(index) else {
            continue;
        };
        let action = match state.policy {
            POLICY_IMMUTABLE | POLICY_PRESERVE => StateAction::Reuse,
            POLICY_EPHEMERAL => StateAction::CreateEmpty,
            POLICY_SNAPSHOT_BEFORE_UPGRADE => StateAction::Snapshot,
            POLICY_DISCARD_ON_ROLLBACK if rollback => StateAction::DiscardOnRollback,
            POLICY_DISCARD_ON_ROLLBACK => StateAction::Reuse,
            _ => continue,
        };
        plan.push(StatePlan {
            name: state.name,
            schema_version: state.schema_version,
            action,
        });
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    #[test_case]
    fn root_set_dedupes_and_ignores_zero() {
        let mut set = RootSet::new();
        assert!(set.insert(root(1)));
        assert!(set.insert(root(1)));
        assert!(set.insert([0; 32]));
        assert!(set.insert(root(2)));
        assert_eq!(set.as_slice().len(), 2);
        assert!(set.contains(root(1)));
        assert!(set.contains(root(2)));
        assert!(!set.contains(root(3)));
        assert!(!set.contains([0; 32]));
    }

    #[test_case]
    fn root_set_is_bounded() {
        let mut set = RootSet::new();
        for byte in 1..=(MAX_ROOTS as u16) {
            assert!(set.insert(root(byte as u8)));
        }
        assert_eq!(set.as_slice().len(), MAX_ROOTS);
        // One past the bound with a novel root is rejected, not silently dropped.
        assert!(!set.insert(root(0xff)));
        // A root already present still succeeds at the bound.
        assert!(set.insert(root(1)));
    }
}
