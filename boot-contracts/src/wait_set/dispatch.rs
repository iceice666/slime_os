//! The bounded wait-set state machine (C9.2).
//!
//! Registration, badge demultiplexing, the ready queue, and the ascending-badge
//! tie rule, with no syscall in any of it. `slime-rt`'s `wait_set` wraps this
//! with the two things that do touch the kernel: reading the declared sources
//! from the root, and blocking on the Notification.
//!
//! # Why it lives here rather than in the runtime crate
//!
//! `slime-rt` cannot be built for a host target — `sel4-alloca`'s inline asm is
//! ELF-only — so `just test_host` deliberately excludes it and a `#[cfg(test)]`
//! module there would be exactly B23's blind spot: tests nothing compiles and
//! nothing runs. The state machine is also the half that most needs testing,
//! since it implements the dispatch order this contract *defines*. So it sits
//! beside the format whose tie rule it honours, where a host gate reaches it.

use super::{DRAIN_SLOT_ABSENT, ENTRY_BYTES, MAX_CALLBACKS_PER_WAKE, SourceKind};

/// Sources one wait set may register — the contract's per-waiter ceiling.
pub const MAX_SOURCES: usize = super::MAX_SOURCES_PER_WAITER;

/// Ready entries the queue may hold.
///
/// Equal to [`MAX_SOURCES`] because a single coalesced badge can carry every
/// registered source at once, so anything smaller would refuse a wake the kernel
/// is entitled to deliver.
pub const MAX_READY: usize = MAX_SOURCES;

/// The ready queue can never outgrow one dispatch pass, which is what makes
/// [`Registry::dispatch`] total: a wake may fill the queue, and a full queue
/// must be drainable without a second call.
const _: () = assert!(MAX_READY <= MAX_CALLBACKS_PER_WAKE);

/// Why a wait-set operation was refused.
///
/// Each variant is a distinct thing a caller can do something about: a source
/// limit is a composition error, an undeclared or duplicate badge is a
/// registration mistake, and an over-budget dispatch is a request for more work
/// than one wake may do.
///
/// There is deliberately no "ready queue full": [`Registry::queue`]'s own
/// documentation derives that the queue cannot overflow, so a variant for it
/// would be a status no input produces — the dead-guard shape backlog item B76
/// removed. The ready queue is still bounded; the bound is just proven rather
/// than enforced at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryError {
    /// [`MAX_SOURCES`] sources are already registered.
    SourceLimit,
    /// The badge is not one this component's generation declares for it, or it
    /// names more than one bit. Registering it would create a source no signal
    /// can reach.
    UndeclaredSource,
    /// This badge is already registered. A second registration would leave two
    /// entries the coalesced word cannot tell apart.
    DuplicateSource,
    /// A dispatch asked for more callbacks than [`MAX_CALLBACKS_PER_WAKE`]
    /// permits. Nothing was dispatched and nothing was lost.
    CallbackLimit,
}

/// One registered wake source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Source {
    /// Exactly one badge bit, from the generation's declaration.
    pub badge: u64,
    pub kind: SourceKind,
    /// The slot to drain, or `None` for a timer.
    pub drain_slot: Option<u32>,
}

/// One ready source, recovered from a badge word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ready {
    pub badge: u64,
    pub kind: SourceKind,
    pub drain_slot: Option<u32>,
}

/// A component's own declared sources.
///
/// Separate from [`Registry`] because the two answer different questions: this
/// is what the generation permits, and a registry is what the component chose
/// to register. Keeping them apart is what lets [`Registry::register`] refuse an
/// undeclared badge rather than trusting its caller.
#[derive(Debug, Clone, Copy)]
pub struct Declared {
    sources: [Option<Source>; MAX_SOURCES],
    len: usize,
}

impl Declared {
    pub const fn empty() -> Self {
        Self {
            sources: [None; MAX_SOURCES],
            len: 0,
        }
    }

    /// Append one source decoded from its contract-encoded record.
    ///
    /// The caller supplies bytes the root served verbatim out of the resource,
    /// so this is the same layout [`super::WaitSet`] decodes rather than a second
    /// reader of it. A malformed record — a badge that is not one bit, or a
    /// drain slot disagreeing with the kind — is refused, so a waiter cannot end
    /// up holding a source the resource's own validation would have rejected.
    pub fn push_record(&mut self, record: &[u8]) -> Result<Source, RegistryError> {
        let source = decode_source(record).ok_or(RegistryError::UndeclaredSource)?;
        let slot = self
            .sources
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(RegistryError::SourceLimit)?;
        *slot = Some(source);
        self.len += 1;
        Ok(source)
    }

    /// Append an already-decoded source. The declaration order is the caller's:
    /// the resource is encoded in ascending badge order, and preserving it is
    /// what makes dispatch order free.
    pub fn push(&mut self, source: Source) -> Result<(), RegistryError> {
        if source.badge.count_ones() != 1 || source.drain_slot.is_some() != source.kind.drains() {
            return Err(RegistryError::UndeclaredSource);
        }
        let slot = self
            .sources
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(RegistryError::SourceLimit)?;
        *slot = Some(source);
        self.len += 1;
        Ok(())
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The declared sources, in the order they were declared — which the
    /// resource fixes as ascending badge.
    pub fn iter(&self) -> impl Iterator<Item = Source> + '_ {
        self.sources.iter().flatten().copied()
    }

    /// The declaration for `badge`, or `None` when this component declares none.
    pub fn source(&self, badge: u64) -> Option<Source> {
        self.iter().find(|source| source.badge == badge)
    }

    /// The declared source of `kind` whose drain slot is `slot`.
    ///
    /// How a component names a source without knowing a badge number: it knows
    /// which of its own slots carries a route, so it asks for that slot's source
    /// and the generation supplies the bit.
    pub fn source_for_slot(&self, kind: SourceKind, slot: u32) -> Option<Source> {
        self.iter()
            .find(|source| source.kind == kind && source.drain_slot == Some(slot))
    }

    /// The declared timer source, if this component has one.
    pub fn timer(&self) -> Option<Source> {
        self.iter().find(|source| source.kind == SourceKind::Timer)
    }
}

/// The registered sources and their ready queue.
#[derive(Debug, Clone, Copy)]
pub struct Registry {
    declared: Declared,
    registered: [Option<Source>; MAX_SOURCES],
    registered_len: usize,
    mask: u64,
    ready: [Option<Ready>; MAX_READY],
    ready_len: usize,
}

impl Registry {
    pub const fn new(declared: Declared) -> Self {
        Self {
            declared,
            registered: [None; MAX_SOURCES],
            registered_len: 0,
            mask: 0,
            ready: [None; MAX_READY],
            ready_len: 0,
        }
    }

    pub const fn registered(&self) -> usize {
        self.registered_len
    }

    /// The union of every registered badge. A wake carrying nothing in this mask
    /// belongs to a source this registry did not register.
    pub const fn mask(&self) -> u64 {
        self.mask
    }

    pub const fn ready_queued(&self) -> usize {
        self.ready_len
    }

    pub const fn declarations(&self) -> &Declared {
        &self.declared
    }

    /// Register one declared source.
    ///
    /// `badge` must be one the generation declares for this component: an
    /// undeclared bit is refused rather than registered, because nothing could
    /// ever signal it and a caller would block forever on a source that looks
    /// live. That check is the whole authority story here — the generation
    /// decides what a component may be woken by, and this refuses everything
    /// else.
    pub fn register(&mut self, badge: u64) -> Result<Source, RegistryError> {
        let source = self
            .declared
            .source(badge)
            .ok_or(RegistryError::UndeclaredSource)?;
        if source.badge.count_ones() != 1 {
            return Err(RegistryError::UndeclaredSource);
        }
        if self.mask & source.badge != 0 {
            return Err(RegistryError::DuplicateSource);
        }
        // Inserted in ascending badge order, not at the first free slot. The
        // ascending order is what [`Self::queue`] walks to produce the
        // contract's dispatch tie rule, so it must be an invariant of this array
        // rather than a property of the order a component happened to register
        // in — a caller registering its timer before its stream source, or
        // re-registering into a hole `unregister` left, would otherwise dispatch
        // descending.
        let position = self.registered[..self.registered_len]
            .iter()
            .position(|slot| slot.is_some_and(|held| held.badge > source.badge))
            .unwrap_or(self.registered_len);
        if self.registered_len == MAX_SOURCES {
            return Err(RegistryError::SourceLimit);
        }
        self.registered
            .copy_within(position..self.registered_len, position + 1);
        self.registered[position] = Some(source);
        self.registered_len += 1;
        self.mask |= source.badge;
        Ok(source)
    }

    /// Register the declared source of `kind` carried by `slot`.
    pub fn register_slot(&mut self, kind: SourceKind, slot: u32) -> Result<Source, RegistryError> {
        let source = self
            .declared
            .source_for_slot(kind, slot)
            .ok_or(RegistryError::UndeclaredSource)?;
        self.register(source.badge)
    }

    /// Register this component's declared timer source.
    pub fn register_timer(&mut self) -> Result<Source, RegistryError> {
        let source = self
            .declared
            .timer()
            .ok_or(RegistryError::UndeclaredSource)?;
        self.register(source.badge)
    }

    /// Remove a registered source, so a retired peer is not waited on again.
    ///
    /// Already-queued readiness for it is dropped too: a source that is no
    /// longer registered must not be dispatched, which is the rule
    /// `fabric-service` applies by hand when it retires a dead publisher.
    pub fn unregister(&mut self, badge: u64) -> bool {
        let Some(position) = self.registered[..self.registered_len]
            .iter()
            .position(|slot| slot.is_some_and(|source| source.badge == badge))
        else {
            return false;
        };
        // Compacted, not holed. `register` maintains ascending badge order by
        // insertion position, so a hole left here would be filled by the next
        // registration regardless of its badge and break the ordering invariant
        // `queue` depends on.
        self.registered
            .copy_within(position + 1..self.registered_len, position);
        self.registered_len -= 1;
        self.registered[self.registered_len] = None;
        self.mask &= !badge;
        let mut kept = 0;
        for index in 0..self.ready_len {
            let entry = self.ready[index];
            self.ready[index] = None;
            if entry.is_some_and(|entry| entry.badge != badge) {
                self.ready[kept] = entry;
                kept += 1;
            }
        }
        self.ready_len = kept;
        true
    }

    /// Turn one accumulated badge word into queued ready entries, returning how
    /// many were added.
    ///
    /// Ascending badge order, because [`Self::register`] inserts by badge and
    /// [`Self::unregister`] compacts: one pass over `registered` is therefore
    /// already the contract's dispatch tie rule, independently of the order a
    /// component registered in, so nothing here sorts and identical readiness
    /// always produces an identical sequence.
    ///
    /// Bits outside [`Self::mask`] are ignored rather than refused. A component
    /// may legitimately hold sources outside one registry, and a wake carrying
    /// only those queues nothing.
    ///
    /// # The queue cannot overflow
    ///
    /// Not a claim about inputs: a property of the two invariants above.
    /// [`Self::register`] refuses a duplicate badge, so `registered` holds at
    /// most [`MAX_SOURCES`] pairwise-distinct badges; every entry queued here has
    /// a badge distinct from those already queued; and [`MAX_READY`] equals
    /// [`MAX_SOURCES`]. So by the time the queue held [`MAX_READY`] entries, every
    /// registered badge would already be queued and the dedup below would skip
    /// every remaining candidate. There is therefore no ceiling error to return,
    /// and returning one would be a branch no input reaches — a dead guard
    /// reading as working redundancy, which is the shape backlog item B76
    /// removed. The `const` assertion beside [`MAX_READY`] is what keeps the
    /// equality load-bearing.
    pub fn queue(&mut self, badge: u64) -> usize {
        let mut queued = 0;
        for index in 0..self.registered_len {
            let Some(source) = self.registered[index] else {
                continue;
            };
            if badge & source.badge == 0 {
                continue;
            }
            // Already queued and not yet dispatched. Two signals that coalesce
            // into one bit are one readiness, so a second entry would dispatch
            // the same source twice against one ready state.
            if self.ready[..self.ready_len]
                .iter()
                .flatten()
                .any(|entry| entry.badge == source.badge)
            {
                continue;
            }
            debug_assert!(self.ready_len < MAX_READY, "distinct badges are bounded");
            self.ready[self.ready_len] = Some(Ready {
                badge: source.badge,
                kind: source.kind,
                drain_slot: source.drain_slot,
            });
            self.ready_len += 1;
            queued += 1;
        }
        queued
    }

    /// Take the next ready source, in the documented dispatch order.
    pub fn next_ready(&mut self) -> Option<Ready> {
        if self.ready_len == 0 {
            return None;
        }
        let entry = self.ready[0];
        self.ready.copy_within(1..self.ready_len, 0);
        self.ready_len -= 1;
        self.ready[self.ready_len] = None;
        entry
    }

    /// Dispatch up to `budget` queued ready sources, in order.
    ///
    /// The remainder stays queued, in order, so a caller that budgets its work
    /// loses no readiness — which is the point: a component servicing a slow
    /// source can bound one pass without forgetting the rest.
    ///
    /// A `budget` above [`MAX_CALLBACKS_PER_WAKE`] is refused outright and
    /// dispatches nothing. The ceiling is the contract's, so asking for more
    /// work than one wake may do is a composition error rather than a request to
    /// silently clamp.
    pub fn dispatch_bounded(
        &mut self,
        budget: usize,
        mut handler: impl FnMut(Ready),
    ) -> Result<usize, RegistryError> {
        if budget > MAX_CALLBACKS_PER_WAKE {
            return Err(RegistryError::CallbackLimit);
        }
        let mut dispatched = 0;
        while dispatched < budget {
            let Some(entry) = self.next_ready() else {
                break;
            };
            handler(entry);
            dispatched += 1;
        }
        Ok(dispatched)
    }

    /// Dispatch every queued ready source, in order.
    ///
    /// [`Self::dispatch_bounded`] at the contract's ceiling, which is also the
    /// ready queue's capacity — so this always drains the queue, and the
    /// `const` assertion above is what keeps that true.
    pub fn dispatch(&mut self, handler: impl FnMut(Ready)) -> Result<usize, RegistryError> {
        self.dispatch_bounded(MAX_CALLBACKS_PER_WAKE, handler)
    }
}

/// Decode one contract-encoded source record.
///
/// Offsets are the contract's own, matching `super::decode_entry`: the root
/// serves the resource's bytes verbatim, so a waiter reading them with different
/// offsets would decode garbage rather than fail.
fn decode_source(record: &[u8]) -> Option<Source> {
    if record.len() < ENTRY_BYTES {
        return None;
    }
    let badge = u64::from_le_bytes(record.get(32..40)?.try_into().ok()?);
    let kind = SourceKind::from_id(u32::from_le_bytes(record.get(48..52)?.try_into().ok()?))?;
    let slot = u32::from_le_bytes(record.get(52..56)?.try_into().ok()?);
    let drain_slot = (slot != DRAIN_SLOT_ABSENT).then_some(slot);
    if badge.count_ones() != 1 || drain_slot.is_some() != kind.drains() {
        return None;
    }
    Some(Source {
        badge,
        kind,
        drain_slot,
    })
}

#[cfg(test)]
mod tests {
    use super::{Declared, MAX_SOURCES, Ready, Registry, RegistryError, Source};
    use crate::wait_set::{MAX_CALLBACKS_PER_WAKE, SourceKind};

    fn declared(sources: &[Source]) -> Declared {
        let mut declared = Declared::empty();
        for source in sources {
            declared.push(*source).unwrap();
        }
        declared
    }

    fn stream(badge: u64, slot: u32) -> Source {
        Source {
            badge,
            kind: SourceKind::Stream,
            drain_slot: Some(slot),
        }
    }

    fn timer(badge: u64) -> Source {
        Source {
            badge,
            kind: SourceKind::Timer,
            drain_slot: None,
        }
    }

    /// One coalesced word carries every ready source, so a single wake queues
    /// them all — the property that makes this a wait set rather than a sweep.
    /// Ascending badge order is the contract's tie rule, so a word signalled out
    /// of order still dispatches in it.
    #[test]
    fn one_badge_word_queues_every_ready_source_in_badge_order() {
        let mut registry = Registry::new(declared(&[
            stream(1 << 1, 3),
            timer(1 << 4),
            stream(1 << 7, 5),
        ]));
        registry.register(1 << 1).unwrap();
        registry.register(1 << 4).unwrap();
        registry.register(1 << 7).unwrap();
        assert_eq!(registry.queue((1 << 7) | (1 << 1) | (1 << 20)), 2);
        let mut seen = [0u64; 4];
        let mut count = 0;
        registry
            .dispatch(|ready: Ready| {
                seen[count] = ready.badge;
                count += 1;
            })
            .unwrap();
        assert_eq!(&seen[..count], &[1 << 1, 1 << 7]);
    }

    /// Two sources signalling before the waiter runs are both dispatched from
    /// the one coalesced badge, rather than one being lost or needing a second
    /// block. This is C9.2's second required check.
    #[test]
    fn coalesced_signals_are_both_dispatched_from_one_wake() {
        let mut registry = Registry::new(declared(&[stream(1 << 0, 1), stream(1 << 2, 2)]));
        registry.register(1 << 0).unwrap();
        registry.register(1 << 2).unwrap();
        assert_eq!(registry.queue(1 | (1 << 2)), 2);
        assert_eq!(registry.dispatch(|_| {}).unwrap(), 2);
        assert_eq!(registry.ready_queued(), 0);
    }

    /// An unregistered badge queues nothing and is not an error: a component may
    /// hold sources outside one registry, and swallowing the wake would hide it.
    #[test]
    fn an_unregistered_badge_queues_nothing() {
        let mut registry = Registry::new(declared(&[stream(1 << 1, 3)]));
        registry.register(1 << 1).unwrap();
        assert_eq!(registry.queue(1 << 9), 0);
        assert_eq!(registry.ready_queued(), 0);
    }

    /// Only a declared badge registers. An undeclared bit would be a source
    /// nothing can signal, so a caller waiting on it would wait forever — C9.2's
    /// "registering a source a component has no authority for is refused".
    #[test]
    fn an_undeclared_badge_is_refused() {
        let mut registry = Registry::new(declared(&[stream(1 << 1, 3)]));
        assert_eq!(
            registry.register(1 << 2),
            Err(RegistryError::UndeclaredSource)
        );
        assert_eq!(registry.register(0), Err(RegistryError::UndeclaredSource));
        assert_eq!(registry.registered(), 0);
        assert_eq!(registry.mask(), 0);
    }

    /// A badge registers once: the coalesced word cannot tell two entries on one
    /// bit apart, so a second would dispatch one readiness twice.
    #[test]
    fn a_badge_registers_once() {
        let mut registry = Registry::new(declared(&[stream(1 << 1, 3)]));
        registry.register(1 << 1).unwrap();
        assert_eq!(
            registry.register(1 << 1),
            Err(RegistryError::DuplicateSource)
        );
        assert_eq!(registry.registered(), 1);
    }

    /// Two coalesced signals on one bit are one readiness, so a second wake
    /// before the first is dispatched does not queue a duplicate.
    #[test]
    fn a_repeated_wake_does_not_queue_the_same_source_twice() {
        let mut registry = Registry::new(declared(&[stream(1 << 1, 3)]));
        registry.register(1 << 1).unwrap();
        assert_eq!(registry.queue(1 << 1), 1);
        assert_eq!(registry.queue(1 << 1), 0);
        assert_eq!(registry.ready_queued(), 1);
    }

    /// The source ceiling refuses with its own error and leaves the registry
    /// usable: sources already registered still queue and dispatch.
    #[test]
    fn the_source_ceiling_refuses_and_leaves_the_registry_usable() {
        let mut sources = [stream(1, 0); MAX_SOURCES];
        for (index, source) in sources.iter_mut().enumerate() {
            *source = stream(1 << index, index as u32);
        }
        let mut declared = declared(&sources);
        // One more declaration than a wait set may register.
        assert_eq!(
            declared.push(stream(1 << MAX_SOURCES, MAX_SOURCES as u32)),
            Err(RegistryError::SourceLimit)
        );
        let mut registry = Registry::new(declared);
        for index in 0..MAX_SOURCES {
            registry.register(1 << index).unwrap();
        }
        assert_eq!(
            registry.register(1 << MAX_SOURCES),
            Err(RegistryError::UndeclaredSource)
        );
        assert_eq!(registry.registered(), MAX_SOURCES);
        assert_eq!(registry.queue(1), 1);
    }

    /// A bounded dispatch loses no readiness: undispatched entries stay queued,
    /// in order, so the next pass resumes where this one stopped. A budget above
    /// the contract's ceiling is refused rather than clamped, with nothing
    /// dispatched.
    #[test]
    fn a_bounded_dispatch_keeps_the_remainder_queued() {
        let mut sources = [stream(1, 0); MAX_SOURCES];
        for (index, source) in sources.iter_mut().enumerate() {
            *source = stream(1 << index, index as u32);
        }
        let mut registry = Registry::new(declared(&sources));
        for index in 0..MAX_SOURCES {
            registry.register(1 << index).unwrap();
        }
        assert_eq!(registry.queue(u64::MAX), MAX_SOURCES);
        assert_eq!(
            registry.dispatch_bounded(MAX_CALLBACKS_PER_WAKE + 1, |_| {
                unreachable!("an over-budget dispatch runs no handler")
            }),
            Err(RegistryError::CallbackLimit)
        );
        assert_eq!(registry.ready_queued(), MAX_SOURCES);
        let mut seen = [0u64; MAX_SOURCES];
        let mut count = 0;
        assert_eq!(
            registry
                .dispatch_bounded(2, |ready: Ready| {
                    seen[count] = ready.badge;
                    count += 1;
                })
                .unwrap(),
            2
        );
        assert_eq!(&seen[..2], &[1, 1 << 1]);
        assert_eq!(registry.ready_queued(), MAX_SOURCES - 2);
        assert_eq!(registry.next_ready().unwrap().badge, 1 << 2);
    }

    /// Unregistering drops the source's queued readiness too: a retired peer
    /// must not be dispatched, which `fabric-service` does by hand today.
    #[test]
    fn unregistering_drops_queued_readiness() {
        let mut registry = Registry::new(declared(&[stream(1 << 1, 3), stream(1 << 2, 4)]));
        registry.register(1 << 1).unwrap();
        registry.register(1 << 2).unwrap();
        assert_eq!(registry.queue((1 << 1) | (1 << 2)), 2);
        assert!(registry.unregister(1 << 1));
        assert_eq!(registry.ready_queued(), 1);
        assert_eq!(registry.next_ready().unwrap().badge, 1 << 2);
        assert_eq!(registry.mask(), 1 << 2);
        assert!(!registry.unregister(1 << 1));
    }

    /// The tie rule holds independently of the order a component registered in.
    ///
    /// Found by review: `registered` was filled at the first free slot, so a
    /// caller registering its timer before its stream source dispatched
    /// descending — a contract violation, since ascending badge order is what
    /// `contracts/wait-set/v1` promises and what makes repeated boots
    /// reproducible. Every other test here registers ascending and so could not
    /// see it.
    #[test]
    fn out_of_order_registration_still_dispatches_ascending() {
        let mut registry = Registry::new(declared(&[stream(1 << 3, 0), timer(1 << 9)]));
        registry.register_timer().unwrap();
        registry.register_slot(SourceKind::Stream, 0).unwrap();
        assert_eq!(registry.queue((1 << 3) | (1 << 9)), 2);
        assert_eq!(registry.next_ready().unwrap().badge, 1 << 3);
        assert_eq!(registry.next_ready().unwrap().badge, 1 << 9);
    }

    /// And it survives a retirement: the second half of the same defect was that
    /// `unregister` left a hole the next registration filled regardless of its
    /// badge, so a re-registering component dispatched out of order.
    #[test]
    fn re_registering_after_a_retirement_still_dispatches_ascending() {
        let mut registry = Registry::new(declared(&[
            stream(1 << 0, 0),
            stream(1 << 1, 1),
            stream(1 << 2, 2),
            stream(1 << 3, 3),
        ]));
        for badge in [1 << 0, 1 << 1, 1 << 2] {
            registry.register(badge).unwrap();
        }
        assert!(registry.unregister(1 << 1));
        registry.register(1 << 3).unwrap();
        assert_eq!(registry.queue(u64::MAX), 3);
        let mut order = [0u64; 3];
        let mut count = 0;
        registry
            .dispatch(|ready| {
                order[count] = ready.badge;
                count += 1;
            })
            .unwrap();
        assert_eq!(order, [1 << 0, 1 << 2, 1 << 3]);
        // The retired badge is gone from the mask and re-registers cleanly.
        assert_eq!(registry.mask(), (1 << 0) | (1 << 2) | (1 << 3));
        registry.register(1 << 1).unwrap();
        assert_eq!(registry.queue(u64::MAX), 4);
        assert_eq!(registry.next_ready().unwrap().badge, 1 << 0);
        assert_eq!(registry.next_ready().unwrap().badge, 1 << 1);
    }

    /// A component names a source by a slot it already holds, or by kind for the
    /// timer, so it never needs to know a badge number — which is what keeps
    /// peers' slot numbers out of the component.
    #[test]
    fn a_source_is_named_by_slot_or_kind_not_by_badge() {
        let mut registry = Registry::new(declared(&[stream(1 << 1, 3), timer(1 << 4)]));
        assert_eq!(
            registry.register_slot(SourceKind::Stream, 3).unwrap().badge,
            1 << 1
        );
        assert_eq!(registry.register_timer().unwrap().badge, 1 << 4);
        assert_eq!(
            registry.register_slot(SourceKind::Stream, 9),
            Err(RegistryError::UndeclaredSource)
        );
        assert_eq!(registry.registered(), 2);
    }

    /// A timer source dispatches with no slot to drain: C9.1 delivers no
    /// payload, so the badge is the whole event.
    #[test]
    fn a_timer_source_dispatches_without_a_slot() {
        let mut registry = Registry::new(declared(&[timer(1 << 9)]));
        registry.register_timer().unwrap();
        assert_eq!(registry.queue(1 << 9), 1);
        let ready = registry.next_ready().unwrap();
        assert_eq!(ready.kind, SourceKind::Timer);
        assert_eq!(ready.drain_slot, None);
    }

    /// A record decoded from the wire agrees with the resource's own decoder, so
    /// a waiter and the root cannot disagree about what a badge means.
    #[test]
    fn a_wire_record_decodes_to_its_declared_source() {
        let mut record = [0u8; crate::wait_set::ENTRY_BYTES];
        record[32..40].copy_from_slice(&(1u64 << 5).to_le_bytes());
        record[48..52].copy_from_slice(&crate::wait_set::KIND_SUPERVISION.to_le_bytes());
        record[52..56].copy_from_slice(&7u32.to_le_bytes());
        let mut declared = Declared::empty();
        let source = declared.push_record(&record).unwrap();
        assert_eq!(source.badge, 1 << 5);
        assert_eq!(source.kind, SourceKind::Supervision);
        assert_eq!(source.drain_slot, Some(7));

        // A timer record carrying a slot is refused here exactly as the resource
        // decoder refuses it, so a malformed reply cannot install a source whose
        // dispatch would read a slot C9.1 never named.
        let mut malformed = [0u8; crate::wait_set::ENTRY_BYTES];
        malformed[32..40].copy_from_slice(&1u64.to_le_bytes());
        malformed[48..52].copy_from_slice(&crate::wait_set::KIND_TIMER.to_le_bytes());
        malformed[52..56].copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(
            Declared::empty().push_record(&malformed),
            Err(RegistryError::UndeclaredSource)
        );
    }
}
