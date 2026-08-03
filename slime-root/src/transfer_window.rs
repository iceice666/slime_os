//! Per-task transfer windows: the bounded regions oversized payloads cross in.
//!
//! The seL4 fast path carries four message registers. A Slime operation whose
//! payload does not fit them — a `recv` buffer, a spawn's grant array, a wait
//! source set — stages it in a page both ends can address instead of growing
//! the message. That page is the task's *transfer window*.
//!
//! `slime-root/src/child_vspace.rs` maps one granule for every child it builds,
//! directly above the child's IPC buffer, and keeps the frame capability in root
//! CSpace. The child declares it once at startup
//! (`components/runtime/src/runtime.rs::start`), and this table records the
//! declaration. Two properties follow, and both are why the window is root-mapped
//! rather than child-allocated:
//!
//! - a component the generation grants no `SharedBufferFactory` — `console` and
//!   every spawned application — still has a window, so `recv` works without
//!   handing it allocation authority it was never declared;
//! - the root reads and writes the window through its own frame capability, never
//!   through a child-supplied address, so a task cannot name a region it does not
//!   own by lying about its base.
//!
//! A task may bind only the window its own VSpace received, and only once. The
//! binding is checked against what the loader mapped, not accepted on the
//! caller's word.

use crate::ipc::IpcError;
use crate::task::TaskId;

/// Bytes of one transfer window. One granule, matching both the frame the
/// loader maps and `slime_rt`'s `MIN_TRANSFER_WINDOW`; the two are the same
/// agreement seen from either end.
pub const WINDOW_BYTES: usize = crate::child_vspace::GRANULE_SIZE;

/// The slot value a child uses to name the root-mapped startup window rather
/// than a shared buffer of its own. Slot 0 is null in every child CSpace, so it
/// cannot collide with a granted capability. Mirrors
/// `sel4_transport::STARTUP_WINDOW_SLOT`.
pub const STARTUP_WINDOW_SLOT: u32 = 0;

/// One task's declared window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Window {
    pub task: TaskId,
    /// Child virtual address of the window, as the loader mapped it.
    pub base: usize,
    pub len: usize,
    /// Root-held capability for the frame backing it. The root stages through
    /// this, never through `base`, which is only meaningful in the child.
    pub frame: sel4::cap::Granule,
}

/// Windows the root has mapped, and which of them their tasks have declared.
///
/// An entry exists from the moment the loader maps the frame; `bind` only flips
/// it to declared. So a bind naming a base the loader never mapped is refused
/// with the record in hand, rather than trusted and discovered later.
pub struct WindowTable<const CAPACITY: usize> {
    entries: [Option<(Window, bool)>; CAPACITY],
    len: usize,
}

impl<const CAPACITY: usize> WindowTable<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            entries: [None; CAPACITY],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Record the window the loader mapped for `task`. Called during task
    /// construction, before the task runs.
    pub fn declare(
        &mut self,
        task: TaskId,
        base: usize,
        frame: sel4::cap::Granule,
    ) -> Result<(), IpcError> {
        if self.entries.iter().flatten().any(|(w, _)| w.task == task) {
            return Err(IpcError::WaiterConflict);
        }
        let Some(slot) = self.entries.iter_mut().find(|entry| entry.is_none()) else {
            return Err(IpcError::WaitSetFull);
        };
        *slot = Some((
            Window {
                task,
                base,
                len: WINDOW_BYTES,
                frame,
            },
            false,
        ));
        self.len += 1;
        Ok(())
    }

    /// Bind `task`'s window in response to its startup declaration.
    ///
    /// Every argument is checked against the mapping the loader actually made:
    /// the slot must be the startup-window slot, and the base and length must be
    /// exactly what was mapped. A mismatch is `InvalidLength` — the task is
    /// describing a region it does not have.
    pub fn bind(
        &mut self,
        task: TaskId,
        slot: u32,
        base: usize,
        len: usize,
    ) -> Result<Window, IpcError> {
        if slot != STARTUP_WINDOW_SLOT {
            return Err(IpcError::InvalidOperation);
        }
        let Some((window, bound)) = self
            .entries
            .iter_mut()
            .flatten()
            .find(|(w, _)| w.task == task)
        else {
            return Err(IpcError::InvalidOperation);
        };
        if window.base != base || window.len != len {
            return Err(IpcError::InvalidLength);
        }
        // Rebinding would let a task point the root at a different region after
        // the root had already accepted one, so the first binding is final.
        if *bound {
            return Err(IpcError::WaiterConflict);
        }
        *bound = true;
        Ok(*window)
    }

    /// The window `task` has bound, if it has bound one. Operations that stage
    /// through the window resolve it here, so an unbound task's oversized
    /// payload is refused rather than written somewhere.
    pub fn bound(&self, task: TaskId) -> Option<Window> {
        self.entries
            .iter()
            .flatten()
            .find(|(w, bound)| *bound && w.task == task)
            .map(|(window, _)| *window)
    }

    /// Drop a task's window as part of reclaiming it. The frame itself is
    /// released by the task's cleanup record, which revokes the whole CSlot
    /// range; this only forgets the binding.
    pub fn release(&mut self, task: TaskId) -> bool {
        let Some(slot) = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_some_and(|(w, _)| w.task == task))
        else {
            return false;
        };
        *slot = None;
        self.len -= 1;
        true
    }
}

impl<const CAPACITY: usize> Default for WindowTable<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{STARTUP_WINDOW_SLOT, WINDOW_BYTES, WindowTable};
    use crate::ipc::IpcError;
    use crate::task::TaskId;

    const BASE: usize = 0x21_0000;

    fn frame() -> sel4::cap::Granule {
        sel4::cap::Granule::from_bits(7)
    }

    fn table() -> WindowTable<4> {
        let mut table = WindowTable::new();
        table.declare(TaskId(0), BASE, frame()).unwrap();
        table
    }

    #[test]
    fn a_declared_window_binds_once_at_its_exact_mapping() {
        let mut table = table();
        assert_eq!(table.bound(TaskId(0)), None, "unbound until declared");
        let window = table
            .bind(TaskId(0), STARTUP_WINDOW_SLOT, BASE, WINDOW_BYTES)
            .unwrap();
        assert_eq!(window.base, BASE);
        assert_eq!(table.bound(TaskId(0)), Some(window));
        assert_eq!(
            table.bind(TaskId(0), STARTUP_WINDOW_SLOT, BASE, WINDOW_BYTES),
            Err(IpcError::WaiterConflict),
            "the first binding is final"
        );
    }

    #[test]
    fn a_window_the_loader_never_mapped_is_refused() {
        let mut table = table();
        assert_eq!(
            table.bind(TaskId(0), STARTUP_WINDOW_SLOT, BASE + 0x1000, WINDOW_BYTES),
            Err(IpcError::InvalidLength),
            "a task cannot name a region it was not given"
        );
        assert_eq!(
            table.bind(TaskId(0), STARTUP_WINDOW_SLOT, BASE, WINDOW_BYTES * 2),
            Err(IpcError::InvalidLength),
            "nor claim more of it than was mapped"
        );
        assert_eq!(table.bound(TaskId(0)), None, "a refused bind binds nothing");
    }

    #[test]
    fn a_task_with_no_declared_window_cannot_bind_one() {
        let mut table = table();
        assert_eq!(
            table.bind(TaskId(1), STARTUP_WINDOW_SLOT, BASE, WINDOW_BYTES),
            Err(IpcError::InvalidOperation)
        );
    }

    #[test]
    fn only_the_startup_slot_names_the_root_mapped_window() {
        let mut table = table();
        assert_eq!(
            table.bind(TaskId(0), STARTUP_WINDOW_SLOT + 1, BASE, WINDOW_BYTES),
            Err(IpcError::InvalidOperation)
        );
    }

    #[test]
    fn windows_are_per_task_and_released_on_reclaim() {
        let mut table = table();
        table.declare(TaskId(1), BASE + 0x8000, frame()).unwrap();
        table
            .bind(TaskId(1), STARTUP_WINDOW_SLOT, BASE + 0x8000, WINDOW_BYTES)
            .unwrap();
        assert_eq!(table.len(), 2);
        // One task's binding says nothing about another's.
        assert_eq!(table.bound(TaskId(0)), None);
        assert!(table.bound(TaskId(1)).is_some());

        assert!(table.release(TaskId(1)));
        assert_eq!(table.bound(TaskId(1)), None);
        assert_eq!(table.len(), 1);
        assert!(
            !table.release(TaskId(1)),
            "releasing twice is not a release"
        );
    }

    #[test]
    fn a_task_cannot_hold_two_windows() {
        let mut table = table();
        assert_eq!(
            table.declare(TaskId(0), BASE + 0x8000, frame()),
            Err(IpcError::WaiterConflict)
        );
    }
}
