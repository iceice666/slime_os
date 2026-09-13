//! A bounded wait set over one declared Notification (C9.2).
//!
//! # Why one Notification
//!
//! [`crate::notification_wait`] is one `seL4_Wait` on one capability, and a
//! component's Notifications live in distinct CSpace slots — so no primitive
//! here blocks on several objects at once, and the architecture decision behind
//! this milestone forbids the root from supplying a multiplexer. What does work
//! is the mechanism the kernel already provides: seL4 ORs the badges of
//! coalesced signals onto one Notification, and the waiter reads the accumulated
//! word. So every source this admits is a signaller of the *same* object, which
//! is exactly the topology `build-generation.py` already validates — one waiter,
//! one or more signallers.
//!
//! # Why the sources are declared
//!
//! A badge bit is not self-describing. `slime-root` derives a signaller's badge
//! from the *signaller's* declared slot, and C9.1's timer badge is contract data
//! chosen independently of any slot, so recovering "which source" from a badge
//! means reading a map the waiter cannot compute. [`WaitSet::declared`] asks the
//! root for that map at startup, exactly as [`crate::resolve_binding`] asks for a
//! slot number — the alternative is compiling peers' slot numbers into the
//! component, which is the coupling B70 removed.
//!
//! # What one wake means
//!
//! A wake means *at least one* registered source is ready, and the badge word
//! says which. It does not say how many events, or in what order they occurred:
//! two signals from one peer coalesce into one bit. So a dispatch drains its
//! source rather than counting wakes, and [`WaitSet::wait`] returns the whole
//! ready set from one block rather than one source per block.
//!
//! Ready sources dispatch in ascending badge order — the contract's tie rule and
//! the order the resource is encoded in, so identical readiness produces an
//! identical dispatch sequence across boots without this module sorting anything.
//!
//! # Where the state machine lives
//!
//! Registration, demultiplexing, the ready queue, and the tie rule are
//! [`boot_contracts::wait_set::dispatch`]; this is the shell that performs the
//! two operations touching the kernel — reading the declared sources from the
//! root, and blocking on the Notification. The split is not cosmetic: `slime-rt`
//! has no host build (`sel4-alloca`'s inline asm is ELF-only, which is why
//! `just test_host` excludes it), so a state machine tested here would be tests
//! nothing runs — B23's blind spot exactly.
//!
//! # Sizing
//!
//! The tables are fixed at the contract's per-waiter ceiling rather than
//! allocated. C9's plan called for allocating from the C10 private region on
//! C10.4's evidence, and that argument does not reach this size: nine `Source`
//! records plus nine `Ready` records is 216 bytes, against the 29960 bytes of
//! `.bss` C10.4 removed from `fabric-service`. A `Vec` here would add a
//! `private-heap` dependency to every component that waits — including the many
//! that link no allocator at all — to save a quarter of a page. The deviation is
//! deliberate: what C10.4 established is that a *worst-case-sized* table is worth
//! removing when it is large, and this one is not.

use boot_contracts::wait_set::dispatch::{Declared, Registry, RegistryError};
use boot_contracts::wait_set::{self, SourceKind};

use crate::syscall::{self, ERR_INVALID_ARG, ERR_WOULDBLOCK};

pub use boot_contracts::wait_set::dispatch::{MAX_READY, MAX_SOURCES, Ready, Source};
pub use boot_contracts::wait_set::{MAX_CALLBACKS_PER_WAKE, SourceKind as Kind};

/// Bytes one encoded source record occupies, from the contract.
const SOURCE_RECORD_BYTES: usize = wait_set::ENTRY_BYTES;

/// Why a wait-set operation was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitError {
    /// A registration, queueing, or dispatch ceiling was reached, or a badge was
    /// undeclared or already registered. The state machine's own refusal,
    /// forwarded unchanged so a caller matches on one vocabulary.
    Registry(RegistryError),
    /// `notification_wait`, `notification_poll`, or the root read failed.
    Transport(i64),
}

impl From<RegistryError> for WaitError {
    fn from(error: RegistryError) -> Self {
        Self::Registry(error)
    }
}

/// A bounded wait set over one declared Notification.
pub struct WaitSet {
    notification_slot: u32,
    registry: Registry,
    /// Wakes this set has blocked for, so a component can report that it parked
    /// rather than spun.
    wakes: usize,
}

impl WaitSet {
    /// A wait set over the Notification in `notification_slot`, admitting the
    /// sources `declared` names.
    pub const fn new(notification_slot: u32, declared: Declared) -> Self {
        Self {
            notification_slot,
            registry: Registry::new(declared),
            wakes: 0,
        }
    }

    /// Read this component's declared sources from the root and build a wait set
    /// over `notification_slot`.
    ///
    /// Self-scoped: the request names no waiter, so a component learns what its
    /// generation declares about it and nothing about a peer. A generation with
    /// no wait-set resource, and one that declares nothing for this component,
    /// both give an empty set — in both cases there is nothing to register.
    pub fn declared(notification_slot: u32) -> Result<Self, WaitError> {
        Ok(Self::new(notification_slot, read_declared()?))
    }

    pub const fn registered(&self) -> usize {
        self.registry.registered()
    }

    pub const fn wakes(&self) -> usize {
        self.wakes
    }

    /// The union of every registered badge.
    pub const fn mask(&self) -> u64 {
        self.registry.mask()
    }

    pub const fn ready_queued(&self) -> usize {
        self.registry.ready_queued()
    }

    /// This component's declared sources, for a caller choosing what to register.
    pub const fn declarations(&self) -> &Declared {
        self.registry.declarations()
    }

    /// Register one declared source. An undeclared badge is refused.
    pub fn register(&mut self, badge: u64) -> Result<Source, WaitError> {
        Ok(self.registry.register(badge)?)
    }

    /// Register the declared source of `kind` carried by `slot`.
    pub fn register_slot(&mut self, kind: SourceKind, slot: u32) -> Result<Source, WaitError> {
        Ok(self.registry.register_slot(kind, slot)?)
    }

    /// Register this component's declared timer source.
    pub fn register_timer(&mut self) -> Result<Source, WaitError> {
        Ok(self.registry.register_timer()?)
    }

    /// Remove a registered source, dropping its queued readiness.
    pub fn unregister(&mut self, badge: u64) -> bool {
        self.registry.unregister(badge)
    }

    /// Block until at least one registered source is ready, then queue the whole
    /// ready set. Returns how many entries this wake added.
    ///
    /// One block per ready set, not per source: the badge word carries every
    /// source that signalled, so a second block to discover the rest would be
    /// the hand-rolled sweep this replaces.
    ///
    /// A wake carrying only unregistered bits queues nothing and returns zero
    /// rather than blocking again. The caller decides — a component that also
    /// holds sources outside this set legitimately sees such a wake, and
    /// swallowing it here would hide it.
    pub fn wait(&mut self) -> Result<usize, WaitError> {
        let badge =
            syscall::notification_wait(self.notification_slot).map_err(WaitError::Transport)?;
        self.wakes += 1;
        Ok(self.registry.queue(badge))
    }

    /// Queue the ready set without blocking, for a caller with work already in
    /// hand. Returns how many entries were queued.
    pub fn poll(&mut self) -> Result<usize, WaitError> {
        match syscall::notification_poll(self.notification_slot) {
            Ok(Some(badge)) => {
                self.wakes += 1;
                Ok(self.registry.queue(badge))
            }
            Ok(None) => Ok(0),
            Err(error) => Err(WaitError::Transport(error)),
        }
    }

    /// Queue the ready set for an already-observed badge word, returning how
    /// many entries were added.
    ///
    /// For a component that polled the Notification itself, or that must
    /// reconstruct readiness from a badge it received another way. Infallible:
    /// the ready queue cannot overflow, which
    /// [`boot_contracts::wait_set::dispatch::Registry::queue`] derives.
    pub fn queue(&mut self, badge: u64) -> usize {
        self.registry.queue(badge)
    }

    /// Take the next ready source, in the documented dispatch order.
    pub fn next_ready(&mut self) -> Option<Ready> {
        self.registry.next_ready()
    }

    /// Dispatch up to `budget` queued ready sources, in order. The remainder
    /// stays queued.
    pub fn dispatch_bounded(
        &mut self,
        budget: usize,
        handler: impl FnMut(Ready),
    ) -> Result<usize, WaitError> {
        Ok(self.registry.dispatch_bounded(budget, handler)?)
    }

    /// Dispatch every queued ready source, in order.
    pub fn dispatch(&mut self, handler: impl FnMut(Ready)) -> Result<usize, WaitError> {
        Ok(self.registry.dispatch(handler)?)
    }
}

/// Read this component's own declared sources from the root, paging until the
/// table ends.
///
/// Records arrive in the contract's ascending-badge order and are kept in it, so
/// a later dispatch is already in the documented order.
fn read_declared() -> Result<Declared, WaitError> {
    let mut declared = Declared::empty();
    let mut cursor = 0usize;
    loop {
        let mut page = [0u8; SOURCE_RECORD_BYTES * MAX_SOURCES];
        let count = match syscall::wait_sources(cursor, &mut page) {
            Ok(count) => count,
            // `ERR_INVALID_ARG` here means the generation declares no wait-set
            // resource at all, which is not an error: a component that waits on
            // nothing still runs.
            //
            // The root's status mapping is deliberately coarse — `slime_status`
            // folds `InvalidOperation` *and* `InvalidLength` onto this one code —
            // so that reading is only sound because of who builds the request.
            // This wrapper does: it always sends exactly the three words the
            // operation declares, from a page whose size is fixed by the
            // contract below, so the root's length arm is unreachable from here
            // and `InvalidOperation` (no resource) is the only remaining
            // producer. A caller hand-rolling the request could not draw this
            // conclusion, which is why the wrapper and this reasoning live
            // together.
            Err(ERR_INVALID_ARG) => return Ok(declared),
            Err(error) => return Err(WaitError::Transport(error)),
        };
        if count == 0 {
            return Ok(declared);
        }
        for index in 0..count {
            let start = index * SOURCE_RECORD_BYTES;
            let record = page
                .get(start..start + SOURCE_RECORD_BYTES)
                .ok_or(WaitError::Transport(ERR_INVALID_ARG))?;
            declared.push_record(record)?;
        }
        cursor += count;
        // A short answer is the end of the table. Asking again would re-read
        // past the end and answer zero, which is the same conclusion one round
        // trip later.
        if count < MAX_SOURCES {
            return Ok(declared);
        }
    }
}

/// Drain one message from a ready source's endpoint.
///
/// A convenience over [`crate::recv`] for the common dispatch body, with the
/// rule that matters made explicit: a badge means readiness, not a message
/// count, so a source is drained until it answers [`crate::ERR_WOULDBLOCK`]
/// rather than once per wake. Returns `Ok(None)` at that point.
pub fn drain(slot: u32, buffer: &mut [u8; crate::MAX_MSG]) -> Result<Option<usize>, i64> {
    let mut caps = [0u64; crate::MAX_CAPS_PER_MSG];
    let result = syscall::recv(slot, buffer, &mut caps);
    if result == ERR_WOULDBLOCK {
        return Ok(None);
    }
    if result < 0 {
        return Err(result);
    }
    Ok(Some(result as usize))
}
