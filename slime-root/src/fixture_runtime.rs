use super::*;

/// Serve the badged root endpoint until fixture `index` reaches a terminal state
/// or its bounded iteration budget is spent.
///
/// Requests and faults arrive on the same endpoint object under different
/// badges, because the non-MCS kernel resolves a thread's fault handler in that
/// thread's own CSpace; see `task.rs`.
#[allow(clippy::too_many_arguments)]
pub(super) fn serve(
    endpoint: sel4::cap::Endpoint,
    index: usize,
    tasks: &mut TaskTable<MAX_TASKS>,
    allocator: &mut ObjectAllocator,
    supervision: &mut SupervisionTable<MAX_TASKS>,
    fixtures: &mut [Option<Fixture>; FIXTURE_TASKS],
    buffer_phase: &mut BufferPhase,
    memory_phase: &mut MemoryPhase,
) {
    for _ in 0..MAX_SERVICE_ITERATIONS {
        if fixtures[index].is_none_or(|fixture| fixture.terminated) {
            return;
        }
        let (info, badge) = endpoint.recv(());
        let Some((id, arrival)) = TaskId::from_badge(badge) else {
            sel4::debug_println!("SLIME_ROOT unbadged arrival badge={badge:#x} rejected");
            ipc::reply(Response::error(IpcError::InvalidOperation));
            continue;
        };
        let Some(position) = fixtures
            .iter()
            .position(|fixture| fixture.is_some_and(|fixture| fixture.id == id))
        else {
            sel4::debug_println!("SLIME_ROOT unknown task badge={badge:#x} rejected");
            ipc::reply(Response::error(IpcError::InvalidOperation));
            continue;
        };

        match arrival {
            Arrival::Request => {
                // `seL4_Recv` writes the fast message registers back into the
                // IPC buffer, so the request words are readable here.
                let words = sel4::with_ipc_buffer(|buffer| {
                    let mut words = [0 as sel4::Word; ipc::FAST_MESSAGE_REGISTERS];
                    let len = info.length().min(ipc::FAST_MESSAGE_REGISTERS);
                    words[..len].copy_from_slice(&buffer.msg_regs()[..len]);
                    words
                });
                serve_request(
                    &info,
                    &words,
                    id,
                    position,
                    tasks,
                    allocator,
                    supervision,
                    fixtures,
                    buffer_phase,
                    memory_phase,
                );
            }
            Arrival::Fault => serve_fault(
                &info,
                id,
                position,
                tasks,
                supervision,
                fixtures,
                buffer_phase,
            ),
        }
    }
    sel4::debug_println!(
        "SLIME_ROOT service budget exhausted iterations={MAX_SERVICE_ITERATIONS} task={}",
        fixtures[index].map_or(u32::MAX, |fixture| fixture.id.0)
    );
}

#[allow(clippy::too_many_arguments)]
fn serve_request(
    info: &sel4::MessageInfo,
    words: &[sel4::Word; ipc::FAST_MESSAGE_REGISTERS],
    id: TaskId,
    position: usize,
    tasks: &mut TaskTable<MAX_TASKS>,
    allocator: &mut ObjectAllocator,
    supervision: &mut SupervisionTable<MAX_TASKS>,
    fixtures: &mut [Option<Fixture>; FIXTURE_TASKS],
    buffer_phase: &mut BufferPhase,
    memory_phase: &mut MemoryPhase,
) {
    let Some(role) = fixtures[position].map(|fixture| fixture.role) else {
        ipc::reply(Response::error(IpcError::InvalidOperation));
        return;
    };

    match info.label() {
        // The fixture's request. Answering it with the task's directive is what
        // proves a grant-derived endpoint carries real service authority.
        // The clean-exit fixture's shared-buffer report. The root records what
        // the child claims and answers immediately; adjudication happens once,
        // after the fixture has finished, in `report_buffer_phase`.
        fixture_labels::DIRECTIVE => {
            if info.length() < 2 || words[0] != REQUEST_TAG {
                sel4::debug_println!(
                    "SLIME_ROOT request malformed task={} len={} tag={:#x}",
                    id.0,
                    info.length(),
                    words[0]
                );
                ipc::reply(Response::error(IpcError::InvalidLength));
                return;
            }
            sel4::debug_println!(
                "SLIME_ROOT request badge={:#x} task={} service_label={} directive={}",
                id.service_badge(),
                id.0,
                fixture_labels::DIRECTIVE,
                role.directive(),
            );
            match supervision.ipc_completed(id.0, fixture_labels::DIRECTIVE, 0) {
                Ok(event) => report(&event.kind, id, position, fixtures),
                Err(error) => sel4::debug_println!(
                    "SLIME_ROOT ipc accounting rejected task={} error={error:?}",
                    id.0
                ),
            }
            ipc::reply(Response::success(0, role.directive()));
        }
        // The clean-exit fixture's phase reports. Both arrive on this label and
        // are told apart by the tag in MR1: the shared-buffer phase sends its
        // observed pattern word, the private-memory phase sends
        // `MEM_REPORT_TAG`. The root records what the child claims and answers
        // immediately; adjudication happens once, after the fixture has
        // finished, in `report_buffer_phase` and `report_memory_phase`.
        shared_buffer_labels::MAP => {
            if info.length() < 3 || words[0] != REQUEST_TAG {
                sel4::debug_println!(
                    "SLIME_ROOT shared report malformed task={} len={} tag={:#x}",
                    id.0,
                    info.length(),
                    words[0]
                );
                ipc::reply(Response::error(IpcError::InvalidLength));
                return;
            }
            if words[1] == MEM_REPORT_TAG {
                memory_phase.flags |= words[2] & MEM_REPORT_ALL;
                memory_phase.reported = true;
                sel4::debug_println!(
                    "SLIME_MEM child reported task={} flags={:#x}",
                    id.0,
                    words[2],
                );
                ipc::reply(Response::success(0, 0));
                return;
            }
            buffer_phase.observed = words[1];
            // The child contributes only what it can actually attest to; the
            // execute-never verdict is the root's and is preserved here.
            buffer_phase.flags |= words[2] & (REPORT_RW_READBACK_OK | REPORT_RO_WRITE_REFUSED);
            buffer_phase.reported = true;
            sel4::debug_println!(
                "SLIME_BUF child reported task={} observed={:#x} flags={:#x}",
                id.0,
                buffer_phase.observed,
                words[2],
            );
            ipc::reply(Response::success(0, 0));
        }
        // C10.1's growth, served on the fixture path too. The mechanism is the
        // same `TaskTable` operation the component graph's dispatcher calls;
        // what differs is only which loop received the request.
        lifecycle_labels::PRIVATE_MEMORY_GROW => {
            let delta = words[0] as usize;
            let response = match tasks.grow_private_memory(allocator, id, delta) {
                Ok(previous) => {
                    let region = tasks
                        .get(id)
                        .map(|task| task.private_memory)
                        .unwrap_or(private_memory::Region::DENIED);
                    sel4::debug_println!(
                        "SLIME_MEM grown task={} delta={delta} previous={previous} pages={} base={:#x} quota={} total={}",
                        id.0,
                        region.pages(),
                        region.base(),
                        region.quota(),
                        tasks.private_memory().total_pages(),
                    );
                    Response::success(previous as i64, region.base() as sel4::Word)
                }
                Err(task::TaskError::PrivateMemory(error)) => {
                    sel4::debug_println!(
                        "SLIME_MEM refused task={} delta={delta} cause={} detail={error:?}",
                        id.0,
                        private_memory_cause(&error),
                    );
                    Response::error(IpcError::TransferFailed)
                }
                Err(error) => {
                    sel4::debug_println!(
                        "SLIME_MEM rejected task={} delta={delta} error={error:?}",
                        id.0
                    );
                    Response::error(IpcError::InvalidOperation)
                }
            };
            ipc::reply(response);
        }
        // A clean exit is a send, not a call: the task is suspended rather than
        // replied to.
        lifecycle_labels::EXIT => {
            let status = words[0] as i64;
            match supervision.exit(id.0, status) {
                Ok(transition) => {
                    report(&transition.event.kind, id, position, fixtures);
                    if let Some(fixture) = fixtures[position].as_mut() {
                        fixture.terminated = true;
                    }
                }
                Err(error) => sel4::debug_println!(
                    "SLIME_ROOT exit supervision rejected task={} error={error:?}",
                    id.0
                ),
            }
            stop(tasks, id, "exit");
        }
        label => {
            let response = Response::error(IpcError::UnsupportedOperation);
            sel4::debug_println!(
                "SLIME_ROOT request unsupported task={} service_label={} result={}",
                id.0,
                label,
                response.result,
            );
            ipc::reply(response);
        }
    }
}

fn serve_fault(
    info: &sel4::MessageInfo,
    id: TaskId,
    position: usize,
    tasks: &mut TaskTable<MAX_TASKS>,
    supervision: &mut SupervisionTable<MAX_TASKS>,
    fixtures: &mut [Option<Fixture>; FIXTURE_TASKS],
    buffer_phase: &mut BufferPhase,
) {
    let record = match fault::decode_fault(info) {
        Ok(record) => record,
        Err(error) => {
            sel4::debug_println!("SLIME_ROOT fault undecodable task={} error={error:?}", id.0);
            return;
        }
    };

    // A shared-buffer protection probe is a fault the root *expects*: the
    // clean-exit fixture deliberately violates a mapping's rights so the
    // enforcement can be observed. Such a fault is supervised and resumed
    // rather than treated as a termination, which is what lets the fixture go
    // on to its ordinary clean exit and keeps every pre-existing marker firing.
    //
    // The recovery is bounded three ways: only the clean-exit fixture is
    // eligible, only at the two exact addresses the phase mapped, and only
    // `SHARED_EXPECTED_PROBES` times in total. Anything else falls through to
    // the ordinary termination path below.
    if fixtures[position].is_some_and(|fixture| fixture.role == Role::CleanExit)
        && let Some(probe) = classify_probe(&record)
        && buffer_phase.probes < SHARED_EXPECTED_PROBES
    {
        buffer_phase.probes += 1;
        #[cfg(not(target_arch = "x86_64"))]
        if probe == Probe::Execute {
            buffer_phase.flags |= REPORT_EXECUTE_REFUSED;
        }
        sel4::debug_println!(
            "SLIME_BUF probe refused task={} kind={} access={:?} address={:#x} instruction={:#x}",
            id.0,
            probe.name(),
            match record.kind {
                fault::FaultKind::VirtualMemory { access, .. } => access,
                _ => fault::AccessKind::Unknown,
            },
            record.address.unwrap_or_default(),
            record.instruction.unwrap_or_default(),
        );
        if let Err(error) = resume_past_probe(tasks, id, probe) {
            fatal!("SLIME_BUF FAIL probe resume task={} error={error:?}", id.0)
        }
        return;
    }
    if fixtures[position].is_some_and(|fixture| fixture.role == Role::CleanExit) {
        // The clean-exit fixture faulted somewhere the phase did not plan for.
        // Recording it is what makes the phase report fail loudly instead of
        // silently resuming an unattributable fault.
        buffer_phase.unexpected += 1;
    }
    match supervision.fault(id.0, record) {
        Ok(transition) => {
            report(&transition.event.kind, id, position, fixtures);
            if let Some(fixture) = fixtures[position].as_mut() {
                fixture.terminated = true;
            }
        }
        Err(error) => sel4::debug_println!(
            "SLIME_ROOT fault supervision rejected task={} error={error:?}",
            id.0
        ),
    }
    // A faulted thread is already blocked on its fault endpoint; suspending it
    // makes the stop explicit and keeps reclamation uniform with a clean exit.
    stop(tasks, id, "fault");
}

fn stop(tasks: &TaskTable<MAX_TASKS>, id: TaskId, after: &str) {
    if let Some(task) = tasks.get(id)
        && let Err(error) = task.suspend()
    {
        sel4::debug_println!(
            "SLIME_ROOT suspend after {after} failed task={} error={error:?}",
            id.0
        );
    }
}

/// Emit one lifecycle observation. Markers name the logical task and role only;
/// no badge, CSlot, or physical identifier appears in an event.
fn report(
    kind: &LifecycleEventKind,
    id: TaskId,
    position: usize,
    fixtures: &[Option<Fixture>; FIXTURE_TASKS],
) {
    let role = fixtures[position].map_or("unknown", |fixture| fixture.role.name());
    match kind {
        LifecycleEventKind::IpcCompleted {
            service_label,
            result,
        } => sel4::debug_println!(
            "SLIME_ROOT child request served task={} role={role} service_label={service_label} result={result}",
            id.0,
        ),
        LifecycleEventKind::Exited { status } => sel4::debug_println!(
            "SLIME_ROOT child exit observed task={} role={role} status={status}",
            id.0
        ),
        LifecycleEventKind::Faulted(record) => sel4::debug_println!(
            "SLIME_ROOT child fault observed task={} role={role} kind={:?} instruction={:?} address={:?}",
            id.0,
            record.kind,
            record.instruction,
            record.address,
        ),
        other => sel4::debug_println!(
            "SLIME_ROOT child event task={} role={role} kind={other:?}",
            id.0
        ),
    }
}

/// Which protection a supervised fault demonstrated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Probe {
    /// A store refused by a read-only mapping.
    ReadOnlyWrite,
    /// A branch refused by an execute-never mapping.
    ///
    /// Absent on x86-64: `seL4_X86_VMAttributes` is a cache-policy selector
    /// with no execute bit, so a data mapping there is executable and no such
    /// fault can occur. See `crate::vm_attributes`.
    #[cfg(not(target_arch = "x86_64"))]
    Execute,
}

impl Probe {
    const fn name(self) -> &'static str {
        match self {
            Self::ReadOnlyWrite => "ro-write",
            #[cfg(not(target_arch = "x86_64"))]
            Self::Execute => "wx-execute",
        }
    }
}

/// Decide whether a fault is one of the phase's two planned probes.
///
/// Both the access kind *and* the faulting address must match: a write
/// anywhere else, or an execute fetch from any page other than the shared data
/// region, is not a probe and must not be resumed.
fn classify_probe(record: &fault::FaultRecord) -> Option<Probe> {
    let fault::FaultKind::VirtualMemory { access, .. } = record.kind else {
        return None;
    };
    let address = usize::try_from(record.address?).ok()?;
    match access {
        fault::AccessKind::Write
            if (SHARED_RO_VADDR..SHARED_RO_VADDR + PAGE_SIZE).contains(&address) =>
        {
            Some(Probe::ReadOnlyWrite)
        }
        #[cfg(not(target_arch = "x86_64"))]
        fault::AccessKind::Execute
            if (SHARED_RW_VADDR..SHARED_RW_VADDR + PAGE_SIZE).contains(&address) =>
        {
            Some(Probe::Execute)
        }
        _ => None,
    }
}

/// Step a probing thread past the instruction that faulted, then let it run.
///
/// Data faults report the store itself, so resuming means advancing the PC by
/// exactly that store's width. AArch64 instructions are fixed-width; the RV64
/// fixture emits its probing store inside an explicit `.option norvc` block so
/// this one instruction is fixed-width even though the image otherwise uses the
/// compressed extension; the x86-64 fixture emits a pinned three-byte encoding
/// (`48 89 01`, `mov %rax, (%rcx)`) for the same reason. Each width is stated
/// beside the architecture that emits it, because a mismatch resumes the thread
/// mid-instruction rather than failing.
///
/// Execute faults report the non-executable branch target, so resume at the
/// link register the indirect call set.
fn resume_past_probe(
    tasks: &TaskTable<MAX_TASKS>,
    id: TaskId,
    probe: Probe,
) -> Result<(), sel4::Error> {
    #[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
    const DATA_INSTRUCTION_BYTES: sel4::Word = 4;
    #[cfg(target_arch = "x86_64")]
    const DATA_INSTRUCTION_BYTES: sel4::Word = 3;

    let Some(task) = tasks.get(id) else {
        return Err(sel4::Error::InvalidCapability);
    };
    let mut context = task.tcb.tcb_read_all_registers(false)?;
    let resume_at = match probe {
        Probe::ReadOnlyWrite => context.pc().wrapping_add(DATA_INSTRUCTION_BYTES),
        #[cfg(not(target_arch = "x86_64"))]
        Probe::Execute => {
            #[cfg(target_arch = "aarch64")]
            {
                *context.gpr(30)
            }
            #[cfg(target_arch = "riscv64")]
            {
                context.inner().ra
            }
        }
    };

    *context.pc_mut() = resume_at;
    // `resume = true`: the thread is blocked on its fault endpoint, and this is
    // the reply that releases it.
    task.tcb.tcb_write_all_registers(true, &mut context)
}

/// Create one shared region, seed it with `pattern`, and map it into the
/// child's VSpace at `vaddr` with exactly `rights`.
///
/// The pattern is written through the root's own scratch window rather than
/// through the child's mapping, so a read-only region really is read-only
/// everywhere the child can see it: the root never holds a writable alias.
#[allow(clippy::too_many_arguments)]
pub(super) fn setup_shared_region(
    buffers: &mut SharedBufferTable,
    adapter: &mut BufferAdapter<'_>,
    vspace: VSpaceCap,
    vaddr: usize,
    rights: MappingRights,
    pattern: u64,
    scratch: &ScratchPage,
) -> Result<(BufferHandle, shared_buffer::FrameCap), &'static str> {
    let frame = adapter.allocate_frame().map_err(|_| "frame allocation")?;
    let anchors = shared_buffer::FrameAnchors::from_slice(&[frame]).map_err(|_| "frame anchors")?;
    // Created writable so the root may seed it; the *mapping* rights are what
    // the child is bound by, and those are narrowed below.
    let handle = buffers
        .create(SHARED_HOLDER, anchors, true)
        .map_err(|_| "region admission")?;

    write_pattern_through_scratch(frame, scratch, pattern).map_err(|_| "pattern seed")?;

    buffers
        .map(
            adapter,
            SHARED_HOLDER,
            handle,
            vspace,
            vaddr,
            0,
            PAGE_SIZE,
            rights,
        )
        .map_err(|_| "child mapping")?;
    Ok((handle, frame))
}

/// Write `pattern` into a frame through the root's scratch window.
///
/// The frame is mapped read-write at the scratch address just long enough for
/// the store, then unmapped, so the root retains no standing alias of a region
/// it hands to a child.
fn write_pattern_through_scratch(
    frame: shared_buffer::FrameCap,
    scratch: &ScratchPage,
    pattern: u64,
) -> Result<(), sel4::Error> {
    let cap = sel4::init_thread::Slot::<sel4::cap_type::Granule>::from_index(frame.0).cap();
    cap.frame_map(
        sel4::init_thread::slot::VSPACE.cap(),
        scratch.addr(),
        sel4::CapRights::read_write(),
        vm_attributes::data(),
    )?;
    // SAFETY: `scratch.addr()` is a granule-aligned page mapped read-write into
    // this VSpace for the duration of this store and aliased by no live Rust
    // reference. `SHARED_PATTERN_OFFSET + 8` is inside the 4 KiB page and the
    // address is 8-byte aligned, so the write is in bounds and aligned.
    unsafe {
        ((scratch.addr() + SHARED_PATTERN_OFFSET) as *mut u64).write_volatile(pattern);
    }
    cap.frame_unmap()
}

/// Read one word back out of a frame through the root's scratch window.
fn read_word_through_scratch(
    frame: shared_buffer::FrameCap,
    scratch: &ScratchPage,
) -> Result<u64, sel4::Error> {
    let cap = sel4::init_thread::Slot::<sel4::cap_type::Granule>::from_index(frame.0).cap();
    cap.frame_map(
        sel4::init_thread::slot::VSPACE.cap(),
        scratch.addr(),
        sel4::CapRights::read_write(),
        vm_attributes::data(),
    )?;
    // SAFETY: as for `write_pattern_through_scratch`; the same page, offset,
    // and alignment, and the mapping is live for the duration of this load.
    let value = unsafe { ((scratch.addr() + SHARED_PATTERN_OFFSET) as *const u64).read_volatile() };
    cap.frame_unmap()?;
    Ok(value)
}

/// Adjudicate the shared-buffer phase from what the root observed, and print
/// the ordered markers that record it.
///
/// Every verdict here is the root's: the child's self-reported flags are
/// checked, not trusted, and the execute-never result comes from the root's own
/// fault record. Any shortfall is fatal rather than a missing marker, so a
/// protection that silently stopped working fails the gate loudly.
pub(super) fn report_buffer_phase(
    phase: &BufferPhase,
    rw_frame: shared_buffer::FrameCap,
    scratch: &ScratchPage,
) {
    if !phase.reported {
        fatal!("SLIME_BUF FAIL child never reported the shared-buffer phase")
    }
    if phase.unexpected != 0 {
        fatal!(
            "SLIME_BUF FAIL {} unattributable fault(s) from the clean-exit fixture",
            phase.unexpected
        )
    }

    // (b) The child observed exactly the bytes the root wrote.
    if phase.flags & REPORT_RW_READBACK_OK == 0 || phase.observed != SHARED_RW_PATTERN as sel4::Word
    {
        fatal!(
            "SLIME_BUF FAIL child read {:#x} expected {:#x}",
            phase.observed,
            SHARED_RW_PATTERN
        )
    }

    // The reverse direction: the child's write-back must be visible to the root
    // through the same frame, which is what distinguishes a shared mapping from
    // a copy handed to the child at startup.
    let echoed = match read_word_through_scratch(rw_frame, scratch) {
        Ok(value) => value,
        Err(error) => fatal!("SLIME_BUF FAIL reading child write-back: {error:?}"),
    };
    if echoed != SHARED_CHILD_REPLY {
        fatal!(
            "SLIME_BUF FAIL child write-back {echoed:#x} expected {:#x}",
            SHARED_CHILD_REPLY
        )
    }
    sel4::debug_println!(
        "SLIME_BUF readback vaddr={:#x} root_wrote={:#x} child_read={:#x} child_wrote={:#x} match=1",
        SHARED_RW_VADDR + SHARED_PATTERN_OFFSET,
        SHARED_RW_PATTERN,
        phase.observed,
        echoed,
    );

    // (c) Both protections held, each observed as a real fault rather than as
    // a rejected bookkeeping flag.
    if phase.flags & REPORT_RO_WRITE_REFUSED == 0 {
        fatal!("SLIME_BUF FAIL read-only mapping accepted a child write")
    }
    #[cfg(not(target_arch = "x86_64"))]
    if phase.flags & REPORT_EXECUTE_REFUSED == 0 {
        fatal!("SLIME_BUF FAIL execute-never mapping did not refuse execution")
    }
    // The inverse assertion on x86-64: the flag is only ever set from an
    // observed Execute fault, and no such fault can occur where the frame
    // attribute does not exist. Seeing it would mean the fault vocabulary or
    // the probe classification changed, which must fail rather than pass
    // quietly.
    #[cfg(target_arch = "x86_64")]
    if phase.flags & REPORT_EXECUTE_REFUSED != 0 {
        fatal!("SLIME_BUF FAIL execute fault reported where no execute attribute exists")
    }
    if phase.probes != SHARED_EXPECTED_PROBES {
        fatal!(
            "SLIME_BUF FAIL supervised {} probe(s), expected {SHARED_EXPECTED_PROBES}",
            phase.probes
        )
    }
    #[cfg(not(target_arch = "x86_64"))]
    sel4::debug_println!(
        "SLIME_BUF rights enforced ro_write=refused wx_execute=refused probes={} supervised=1",
        phase.probes,
    );
    // Distinct text on purpose: this profile did not observe an execute
    // refusal, and printing the other marker would make an unenforced mapping
    // read as an enforced one in a transcript.
    #[cfg(target_arch = "x86_64")]
    sel4::debug_println!(
        "SLIME_BUF rights enforced ro_write=refused wx_execute=unenforced probes={} supervised=1",
        phase.probes,
    );
}

/// Adjudicate the private-memory phase from what the root observed, and print
/// the ordered markers that record it (C10.1).
///
/// Every verdict is the root's. The child reports what it saw from inside its
/// own address space — zeros, a surviving pattern, a refusal — and the root
/// checks those against its own page accounting, which the child cannot see and
/// cannot forge. A shortfall is fatal rather than a missing marker, so a
/// mechanism that silently stopped bounding growth fails the gate loudly.
pub(super) fn report_memory_phase(phase: &MemoryPhase, tasks: &TaskTable<MAX_TASKS>) {
    if !phase.reported {
        fatal!("SLIME_MEM FAIL child never reported the private-memory phase")
    }
    let missing = MEM_REPORT_ALL & !phase.flags;
    if missing != 0 {
        fatal!(
            "SLIME_MEM FAIL child reported {:#x}, missing {missing:#x} of {MEM_REPORT_ALL:#x}",
            phase.flags
        )
    }
    // The root's own half: the clean-exit fixture grew to exactly its declared
    // ceiling and nothing else grew at all. The child can attest that its
    // pattern survived; only the root can say how many pages it handed out.
    let table = tasks.private_memory();
    if table.total_pages() != PRIVATE_QUOTA_PAGES {
        fatal!(
            "SLIME_MEM FAIL {} live page(s), expected exactly {PRIVATE_QUOTA_PAGES}",
            table.total_pages()
        )
    }
    // Exactly two grants, which is the property a total alone cannot state: the
    // two size queries and the refusal must each take no page, so a mechanism
    // that charged a query, or charged twice per growth, would reach the same
    // four-page total by a different and wrong route.
    if table.grants() != MEM_EXPECTED_GRANTS {
        fatal!(
            "SLIME_MEM FAIL {} growth grant(s), expected exactly {MEM_EXPECTED_GRANTS}",
            table.grants()
        )
    }
    sel4::debug_println!(
        "SLIME_MEM enforced quota={PRIVATE_QUOTA_PAGES} pages={} grants={} grown={} reclaimed={} flags={:#x}",
        table.total_pages(),
        table.grants(),
        table.grown_pages(),
        table.reclaimed_pages(),
        phase.flags,
    );
}
