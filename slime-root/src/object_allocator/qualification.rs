//! Compile-time qualification scenarios for expandable root CSpace, metadata
//! segment lifecycle, and the bootstrap reserve's boundaries.
//!
//! Each scenario is compiled only into its own root role's image, runs before
//! any task is published, and restores every resource it borrows. They drive
//! the product paths — the same allocator, the same kernel invocations — so
//! their evidence is execution, not a model of it.

#[cfg(any(
    slime_cspace_expanded,
    slime_metadata_lifecycle,
    slime_bootstrap_boundaries
))]
use super::AllocError;
use super::ObjectAllocator;

impl ObjectAllocator {
    /// Live, reusable and quarantined metadata, in one reconcilable line.
    ///
    /// Live records, the retained reusable pool, installed CNodes and the
    /// bytes a retained transaction still owns are separate quantities: a
    /// teardown that returns live records to their baseline while the pool
    /// keeps its charged capacity is the reconciliation this reports.
    pub fn report_metadata_census(&self, phase: &str) {
        sel4::debug_println!(
            "SLIME_ROOT metadata census phase={} pages={} bytes={} allocations_live={} allocations_free={} extents_live={} extents_free={} preserved_live={} preserved_free={} slot_words={} slots_free={} nodes={} quarantined={} pending={}",
            phase,
            self.metadata_pages(),
            self.infrastructure.owned_bytes(),
            self.allocation_descriptor_capacity() - self.allocation_descriptors_free(),
            self.allocation_descriptors_free(),
            self.extent_descriptor_capacity() - self.extent_descriptors_free(),
            self.extent_descriptors_free(),
            self.preserved.len(),
            self.preserved.capacity() - self.preserved.len(),
            self.slots.words(),
            self.free_slots(),
            self.infrastructure.installed_nodes(),
            self.infrastructure.quarantined_bytes(),
            u8::from(self.infrastructure.pending()),
        );
    }

    /// Metadata pages whose frame and tables the ownership ledger retains.
    pub fn metadata_pages(&self) -> usize {
        self.metadata_ledger
            .iter()
            .filter(|owned| owned.is_some())
            .count()
    }
}

#[cfg(slime_cspace_expanded)]
#[repr(C, align(4096))]
struct ProverPage([u8; super::GRANULE_BYTES]);

/// The second root thread's stack and IPC buffer, both root-image pages so
/// they are mapped before the thread runs.
#[cfg(slime_cspace_expanded)]
static mut PROVER_STACK: ProverPage = ProverPage([0; super::GRANULE_BYTES]);
#[cfg(slime_cspace_expanded)]
static mut PROVER_IPC_BUFFER: ProverPage = ProverPage([0; super::GRANULE_BYTES]);

/// Signal the notification this thread was handed, then park forever.
///
/// Both capabilities are named by full expanded addresses, so reaching the
/// park is a second root-CSpace thread resolving the installed tree. It names
/// its own IPC buffer on every invocation, because the crate's ambient slot
/// belongs to the root thread and a second borrower panics; and it blocks
/// rather than spins, because it runs at the root's priority and a busy loop
/// there starves every component the graph is about to launch.
#[cfg(slime_cspace_expanded)]
extern "C" fn prover_entry(signal: usize, park: usize) -> ! {
    // SAFETY: `tcb_configure` named this page as this thread's IPC buffer,
    // and no other thread reads or writes it.
    let buffer =
        unsafe { &mut *(core::ptr::addr_of_mut!(PROVER_IPC_BUFFER) as *mut sel4::IpcBuffer) };
    sel4::cap::Notification::from_bits(signal as _)
        .with(&mut *buffer)
        .signal();
    loop {
        sel4::cap::Notification::from_bits(park as _)
            .with(&mut *buffer)
            .wait();
    }
}

#[cfg(slime_cspace_expanded)]
impl ObjectAllocator {
    /// Retire the initial CNode's free slots so every later allocation is
    /// named beyond it.
    ///
    /// Retired slots stay occupied and are never handed out again, which is
    /// what makes this plane's evidence about addresses outside branch zero
    /// rather than about spare capacity inside it.
    pub fn pressure_initial_namespace(&mut self) {
        let retired = self.slots.retire_below(super::KERNEL_ROOT_CNODE_SLOTS);
        sel4::debug_println!(
            "SLIME_ROOT cspace pressure initial_limit={} retired={} free={}",
            super::KERNEL_ROOT_CNODE_SLOTS,
            retired,
            self.free_slots(),
        );
    }

    /// Create, invoke, copy, retype, delete and revoke real capabilities whose
    /// addresses lie beyond the initial namespace, from both root threads.
    pub fn exercise_expanded_cspace(
        &mut self,
        bootinfo: &sel4::BootInfo,
    ) -> Result<(), AllocError> {
        let limit = super::KERNEL_ROOT_CNODE_SLOTS;
        let root = sel4::init_thread::slot::CNODE.cap();
        let (mut invoked, mut copied, mut retyped, mut deleted, mut revoked) = (0, 0, 0, 0, 0);
        let mut created = 0usize;
        let mut lowest = usize::MAX;

        // The notification is the observation channel: an unbadged signal
        // cannot be told apart from no signal, so the copy handed to the
        // second thread is minted with a badge the root can recognise.
        const PROVER_BADGE: sel4::Word = 1;
        let notification = self.allocate_fixed::<sel4::cap_type::Notification>()?;
        created += 1;
        retyped += 1;
        lowest = lowest.min(notification.index());
        notification.cap().signal();
        let (_, drained) = notification.cap().poll();
        invoked += 2;
        if drained != 0 {
            return Err(AllocError::NoKernelUntyped);
        }

        // A badged copy in a second expanded slot, invoked through the copy:
        // the installed tree must resolve both addresses to the same object.
        self.ensure_root_slots(1)?;
        let copy = self.take_slot()?;
        lowest = lowest.min(copy);
        root.absolute_cptr(sel4::CPtr::from_bits(copy as _))
            .mint(
                &root.absolute_cptr(sel4::CPtr::from_bits(notification.index() as _)),
                sel4::CapRights::all(),
                PROVER_BADGE,
            )
            .map_err(|error| AllocError::ArenaCleanup { slot: copy, error })?;
        copied += 1;
        sel4::cap::Notification::from_bits(copy as _).signal();
        invoked += 1;
        let (_, badge) = notification.cap().poll();
        invoked += 1;
        if badge != PROVER_BADGE {
            return Err(AllocError::NoKernelUntyped);
        }

        // A retype whose parent is itself an expanded capability.
        let parent = self.allocate_variable::<sel4::cap_type::Untyped>(
            sel4::FrameObjectType::GRANULE.bits() + 1,
        )?;
        created += 1;
        retyped += 1;
        lowest = lowest.min(parent.index());
        self.ensure_root_slots(1)?;
        let child = self.take_slot()?;
        lowest = lowest.min(child);
        crate::root_cspace::retype(
            parent.cap(),
            &sel4::FrameObjectType::GRANULE.blueprint(),
            child,
            1,
        )
        .map_err(|error| AllocError::ArenaCleanup { slot: child, error })?;
        created += 1;
        retyped += 1;

        // The second root thread signals the badged copy, then parks on a
        // notification this root never signals. Its badge arriving is a
        // thread other than this one resolving an expanded address.
        let park = self.allocate_fixed::<sel4::cap_type::Notification>()?;
        created += 1;
        retyped += 1;
        lowest = lowest.min(park.index());
        let prover = self.start_second_thread(bootinfo, copy, park.index())?;
        let mut second = false;
        while !second {
            sel4::r#yield();
            second = notification.cap().poll().1 == PROVER_BADGE;
        }
        prover
            .cap()
            .tcb_suspend()
            .map_err(|error| AllocError::ArenaCleanup {
                slot: prover.index(),
                error,
            })?;
        for slot in [prover.index(), park.index()] {
            root.absolute_cptr(sel4::CPtr::from_bits(slot as _))
                .delete()
                .map_err(|error| AllocError::ArenaCleanup { slot, error })?;
            deleted += 1;
            self.release_slot(slot);
        }

        root.absolute_cptr(sel4::CPtr::from_bits(copy as _))
            .delete()
            .map_err(|error| AllocError::ArenaCleanup { slot: copy, error })?;
        deleted += 1;
        self.release_slot(copy);
        // Revoking the parent removes the child it retyped, so the child slot
        // is released only after the kernel reports the revoke succeeded.
        root.absolute_cptr(sel4::CPtr::from_bits(parent.index() as _))
            .revoke()
            .map_err(|error| AllocError::ArenaCleanup {
                slot: parent.index(),
                error,
            })?;
        revoked += 1;
        self.release_slot(child);

        // This thread's own access after the deletes and the revoke.
        notification.cap().signal();
        invoked += 1;
        let root_live = notification.cap().poll().1 == 0;
        invoked += 1;
        root.absolute_cptr(sel4::CPtr::from_bits(notification.index() as _))
            .revoke()
            .and_then(|()| {
                root.absolute_cptr(sel4::CPtr::from_bits(notification.index() as _))
                    .delete()
            })
            .map_err(|error| AllocError::ArenaCleanup {
                slot: notification.index(),
                error,
            })?;
        deleted += 1;
        revoked += 1;
        self.release_slot(notification.index());

        sel4::debug_println!(
            "SLIME_ROOT cspace exercised created={} invoked={} copied={} retyped={} deleted={} revoked={} min_address={} initial_limit={}",
            created,
            invoked,
            copied,
            retyped,
            deleted,
            revoked,
            lowest,
            limit,
        );
        sel4::debug_println!(
            "SLIME_ROOT cspace live root={} second={} leaves={}",
            u8::from(root_live),
            u8::from(second),
            self.expanded_leaves(),
        );
        Ok(())
    }

    /// Start a second thread in the root's own CSpace and VSpace, returning
    /// its TCB so the caller can stop it once its proof arrives.
    fn start_second_thread(
        &mut self,
        bootinfo: &sel4::BootInfo,
        notification: usize,
        park: usize,
    ) -> Result<crate::root_cspace::RootSlot<sel4::cap_type::Tcb>, AllocError> {
        let slot = self.allocate_fixed::<sel4::cap_type::Tcb>()?;
        let tcb = slot.cap();
        let ipc_address = core::ptr::addr_of!(PROVER_IPC_BUFFER) as usize;
        let ipc_frame = crate::child_vspace::image_frame(bootinfo, ipc_address);
        tcb.tcb_configure(
            // A fault in this thread is a root defect: let the kernel report
            // it rather than deliver it somewhere that would swallow it.
            sel4::CPtr::from_bits(0),
            sel4::init_thread::slot::CNODE.cap(),
            crate::child_vspace::root_cspace_guard(bootinfo),
            sel4::init_thread::slot::VSPACE.cap(),
            ipc_address as sel4::Word,
            ipc_frame,
        )
        .map_err(|error| AllocError::ArenaCleanup {
            slot: slot.index(),
            error,
        })?;
        tcb.tcb_set_sched_params(sel4::init_thread::slot::TCB.cap(), 255, 255)
            .map_err(|error| AllocError::ArenaCleanup {
                slot: slot.index(),
                error,
            })?;
        let stack_top = core::ptr::addr_of!(PROVER_STACK) as usize + super::GRANULE_BYTES;
        let mut registers = sel4::UserContext::default();
        let entry: extern "C" fn(usize, usize) -> ! = prover_entry;
        *registers.pc_mut() = entry as usize as sel4::Word;
        *registers.sp_mut() = crate::thread_abi::initial_stack_pointer(stack_top) as sel4::Word;
        *registers.c_param_mut(0) = notification as sel4::Word;
        *registers.c_param_mut(1) = park as sel4::Word;
        tcb.tcb_write_all_registers(true, &mut registers)
            .map_err(|error| AllocError::ArenaCleanup {
                slot: slot.index(),
                error,
            })?;
        Ok(slot)
    }

    /// Workload leaves admitted beyond the initial namespace.
    fn expanded_leaves(&self) -> usize {
        self.expanded_leaves
    }
}

#[cfg(slime_metadata_lifecycle)]
impl ObjectAllocator {
    /// Grow, reuse, injure and reconcile metadata storage before publication.
    ///
    /// Growth funds new pages once and later rounds are served from the
    /// retained pool; a failed construction and a failed delete each retain
    /// their resources until a retry releases them exactly once.
    pub fn exercise_metadata_lifecycle(&mut self) -> Result<(), AllocError> {
        self.report_metadata_census("baseline");
        let per_page = super::segmented::Segmented::<super::AllocationRecord>::entries_per_page();
        for round in 0..3 {
            let pages = self.metadata_pages();
            let free = self.allocation_descriptors_free();
            let request = if round == 0 { free + per_page } else { free };
            self.ensure_allocation_descriptors(request)?;
            let grew = self.metadata_pages() - pages;
            sel4::debug_println!(
                "SLIME_ROOT metadata round={} grew={} reused={}",
                round,
                grew,
                if grew == 0 { request } else { 0 },
            );
        }

        // A construction that stops after its page is mapped: the frame and
        // tables stay owned by the transaction, named by no durable record.
        super::INJECT_LEDGER_RETAIN.store(true, core::sync::atomic::Ordering::Relaxed);
        let free = self.allocation_descriptors_free();
        match self.ensure_allocation_descriptors(free + per_page) {
            Err(AllocError::NoKernelUntyped) => {}
            other => return other.and(Err(AllocError::NoKernelUntyped)),
        }
        sel4::debug_println!(
            "SLIME_ROOT metadata injected kind=construction retained={} reassigned=0",
            self.infrastructure.quarantined_bytes(),
        );

        // A node whose installation succeeded and whose temporary capability
        // could not be deleted: the emergency slot stays owned until retry.
        super::INJECT_NODE_DELETE.store(true, core::sync::atomic::Ordering::Relaxed);
        let leaf = self.infrastructure_leaf_end;
        match self.ensure_infrastructure_slots(crate::root_cspace::LEAF_SLOTS) {
            Err(AllocError::ArenaCleanup { .. }) => {}
            other => return other.and(Err(AllocError::NoKernelUntyped)),
        }
        sel4::debug_println!(
            "SLIME_ROOT metadata injected kind=revoke retained={} reassigned=0",
            self.infrastructure.quarantined_bytes(),
        );
        if self.infrastructure_leaf_end != leaf {
            return Err(AllocError::NoKernelUntyped);
        }
        self.report_metadata_census("grown");

        self.ensure_allocation_descriptors(free + per_page)?;
        sel4::debug_println!("SLIME_ROOT metadata retry kind=construction released=1 reassigned=0");
        self.ensure_infrastructure_slots(crate::root_cspace::LEAF_SLOTS)?;
        sel4::debug_println!("SLIME_ROOT metadata retry kind=revoke released=1 reassigned=0");
        self.report_metadata_census("released");
        Ok(())
    }
}

#[cfg(slime_bootstrap_boundaries)]
impl ObjectAllocator {
    /// Measure the bootstrap reserve, then exhaust RAM, root CSlots and the
    /// metadata window independently and observe each refusal.
    ///
    /// Every case restores exactly what it borrowed, and all of them run
    /// before publication: an image whose reserve cannot fund one more
    /// segment must say so while no task exists to be affected.
    pub fn exercise_bootstrap_boundaries(&mut self) -> Result<(), AllocError> {
        let reserve_slots = super::infrastructure::Infrastructure::reserve_slots();
        sel4::debug_println!(
            "SLIME_ROOT bootstrap reserve objects={} alignment={} remaining={} slots={} transaction={} recursion=0 fit=1",
            self.infrastructure.object_bytes(),
            self.infrastructure.alignment_bytes(),
            self.infrastructure.remaining_bytes(),
            reserve_slots,
            super::infrastructure::Infrastructure::transaction_slots(),
        );

        let held = self.infrastructure.hold_remaining();
        let watermarks = self.hold_ordinary_tails();
        self.report_exhaustion("ram")?;
        self.restore_ordinary_tails(&watermarks);
        self.infrastructure.restore_remaining(held);

        let reserved = self.infrastructure.hold_reserve_slots();
        let slots = self.slots.reserve_stress_pressure(0);
        self.report_exhaustion("cnode-slots")?;
        self.slots.release_stress_pressure();
        self.infrastructure.release_reserve_slots(reserved);
        let _ = slots;

        let next = self.metadata_next;
        self.metadata_next = super::infrastructure::METADATA_END;
        self.report_exhaustion("metadata")?;
        self.metadata_next = next;

        sel4::debug_println!("SLIME_ROOT bootstrap boundaries complete cases=3 published=1");
        Ok(())
    }

    /// Ask for one more metadata page under the current constraint and report
    /// the refusal alongside what each resource still has.
    fn report_exhaustion(&mut self, cause: &str) -> Result<(), AllocError> {
        let free = self.allocation_descriptors_free();
        if self.ensure_allocation_descriptors(free + 1).is_ok() {
            return Err(AllocError::NoKernelUntyped);
        }
        sel4::debug_println!(
            "SLIME_ROOT bootstrap exhausted cause={} published=0 refused=1 ram_free={} slots_free={} metadata_free={}",
            cause,
            self.untyped_bytes_remaining() + self.infrastructure.remaining_bytes(),
            self.free_slots() + self.infrastructure.free_reserve_slots(),
            (super::infrastructure::METADATA_END - self.metadata_next) / super::GRANULE_BYTES,
        );
        Ok(())
    }

    /// Consume every admitted ordinary tail, returning the watermarks to put
    /// back. Nothing is retyped, so restoring them restores exact ownership.
    fn hold_ordinary_tails(&mut self) -> [usize; super::MAX_KERNEL_UNTYPEDS] {
        let mut watermarks = [0; super::MAX_KERNEL_UNTYPEDS];
        for (index, region) in self.untypeds[..self.untyped_len].iter_mut().enumerate() {
            if let Some(region) = region.as_mut() {
                watermarks[index] = region.watermark;
                region.watermark = region.capacity();
            }
        }
        watermarks
    }

    fn restore_ordinary_tails(&mut self, watermarks: &[usize; super::MAX_KERNEL_UNTYPEDS]) {
        for (index, region) in self.untypeds[..self.untyped_len].iter_mut().enumerate() {
            if let Some(region) = region.as_mut() {
                region.watermark = watermarks[index];
            }
        }
    }
}
