//! `slime-root`: the seL4 root task that owns Slime's dynamic mechanism.
//!
//! Startup is staged so that authority always follows a generation declaration:
//!
//! 1. decode and admit the embedded generation, classifying every component
//!    payload;
//! 2. take deterministic ownership of BootInfo CSlots and kernel untypeds;
//! 3. derive each child's endpoint authority from declared grants;
//! 4. build every child task — CSpace, VSpace, TCB, IPC buffer, badged root
//!    endpoint, fault handler — without running any of them;
//! 5. activate only after every allocation has succeeded, one task at a time so
//!    the serial record is ordered;
//! 6. serve requests and faults, then reclaim each task's recorded CSlots.
//!
//! The generation's own component payloads are `SLIMECM*` images built for the
//! retired Slime kernel, not ELF. Their graph is admitted and counted, but they
//! are not activated; the cutover proof is the native AArch64 child fixture in
//! `slime-root/child`.

#![no_main]
#![no_std]
// The modules below are `slime-root`'s mechanism surface: bounded, pure, and
// unit-tested in place. Startup exercises the allocation, task, IPC, and fault
// paths; the scheduling, timer, and shared-buffer state machines are owned here
// but driven by callers a parent integration adds, so not every item is
// reachable from `main`.
#![allow(dead_code)]

mod buffer_adapter;
mod channel;
mod child_vspace;
mod event;
mod fault;
mod generation;
mod graph;
mod ipc;
mod object_allocator;
mod parked;
mod platform_timer;
mod shared_buffer;
mod task;
mod timer;
mod transfer_window;
mod transit;

use core::ptr;

use boot_contracts::boot_layout::BootLayout;
use boot_contracts::generation::{Generation, KIND_RESOURCE};
use boot_contracts::shared_buffer_budget::{self as budget_magic, SharedBufferBudget};
use sel4_root_task::root_task;

use buffer_adapter::BufferAdapter;
use channel::{ChannelTable, LaunchedComponents, SlotCursors, WaitTarget};
use child_vspace::{ChildImage, GRANULE_SIZE, ScratchPage};
use event::TaskEpoch;
use fault::{LifecycleEventKind, SupervisionTable};
use generation::{Admission, Authority, RIGHT_EXEC, RIGHT_RECV, RIGHT_SEND, inbound_authority};
use graph::GraphTables;
use ipc::{IpcError, Operation, Response, poll_notification};
use object_allocator::ObjectAllocator;
use parked::{ParkReason, ParkedReplies};
use platform_timer::{PhysicalTimerAdapter, TIMER_IRQ};
use shared_buffer::{
    BufferHandle, GenerationEpoch, HolderId, HolderQuota, MappingRights, PAGE_SIZE,
    SharedBufferAdapter, SharedBufferTable, VSpaceCap,
};
use task::{Arrival, MAX_TASKS, Supervision, TaskId, TaskTable};
use timer::{PlatformTimer, ServiceTimerError, TimerScheduler, apply_deadline_programming};
use transfer_window::WindowTable;
use transit::Transit;

/// Report an unrecoverable startup condition and park the root task. Every
/// fallible step returns a typed error that ends up here; nothing panics.
macro_rules! fatal {
    ($($arg:tt)*) => {{
        sel4::debug_println!("SLIME_ROOT FATAL {}", format_args!($($arg)*));
        sel4::init_thread::suspend_self()
    }};
}

/// The generation this root task admits and launches.
///
/// Supplied by the build harness through `SLIME_GENERATION`, which
/// `scripts/build/build-sel4.py` points at the `aarch64-sel4-qemu-virt`
/// generation it just built. The checked-in fixture is the fallback, and it is
/// the retained x86 generation P5.1 admitted: its component payloads are the
/// retired kernel's segment-carrying images, which are admitted for authority
/// derivation and never activated. Which one is compiled in therefore decides
/// whether this boot launches a graph or reports that it cannot — and the boot
/// markers say which, rather than leaving it to be inferred.
/// Aligned to 8 for the same reason `CHILD_ELF` is: the component ELFs this
/// generation carries are parsed in place, at their address inside this
/// constant, and `object`'s ELF64 parser requires an 8-byte-aligned buffer.
/// Each image's payload sits at a multiple of 8 from the start of the
/// generation, so aligning the whole blob aligns every ELF within it.
/// Selected once here so the two `include_bytes!` sites — the length and the
/// value — can never disagree about which file they read. A `match` cannot do
/// this: the two arms are arrays of different lengths, so they have different
/// types and will not unify.
#[cfg(slime_generation_supplied)]
macro_rules! generation_bytes {
    () => {
        include_bytes!(env!("SLIME_GENERATION"))
    };
}
#[cfg(not(slime_generation_supplied))]
macro_rules! generation_bytes {
    () => {
        include_bytes!("../fixtures/generation.bin")
    };
}

static GENERATION: Aligned<{ generation_bytes!().len() }> = Aligned(*generation_bytes!());

const GENERATION_BYTES: &[u8] = &GENERATION.0;

/// The target profile this root task admits executables for. Named rather than
/// inferred: an image is admitted by equality on every axis the profile
/// declares, so the profile has to be a stated fact the build can be checked
/// against, not something derived from whatever happens to be running.
const TARGET_PROFILE: &str = "aarch64-sel4-qemu-virt";

/// The native child fixture, built for `aarch64-sel4-minimal.json`. Supplied by
/// the build harness; see `slime-root/child`. `include_bytes!` only guarantees
/// byte alignment, but `object`'s ELF64 parser requires an 8-byte-aligned
/// buffer; `Aligned` forces that without a runtime copy into a new allocation.
#[repr(align(8))]
struct Aligned<const N: usize>([u8; N]);

static CHILD_ELF: Aligned<{ include_bytes!(env!("CHILD_ELF")).len() }> =
    Aligned(*include_bytes!(env!("CHILD_ELF")));

/// Fixture request tag, mirroring `slime-root/child/src/main.rs`.
const REQUEST_TAG: sel4::Word = 0x534c_494d_4552_4551;

/// Directives the root returns to a fixture in reply MR1.
const DIRECTIVE_EXIT: sel4::Word = 0;
const DIRECTIVE_FAULT: sel4::Word = 1;

/// Child tasks this startup builds: one proving a clean request/exit cycle, one
/// proving fault delivery and supervision.
const FIXTURE_TASKS: usize = 2;

/// Service-loop iterations per fixture. A fixture contributes one request plus
/// one terminal message; the clean-exit fixture additionally contributes its
/// shared-buffer report and two supervised protection faults. The surplus
/// bounds unexpected traffic so the loop cannot spin forever.
const MAX_SERVICE_ITERATIONS: usize = 8;

/// The timer phase schedules a deadline this fraction of a second out — short
/// enough that a working IRQ path fires almost immediately, generous enough
/// that QEMU's timer emulation reliably delivers it.
const TIMER_PROOF_DEADLINE_DIVISOR: u64 = 100;

/// Wall-clock ceiling, in whole seconds of the timer's own counter, the
/// bounded wait loop tolerates before declaring the interrupt undelivered.
/// Generous relative to the scheduled deadline so real delivery never comes
/// close, while still keeping a wedged boot failing in a few seconds instead
/// of burning the full boot-check timeout.
const TIMER_PROOF_BOUND_SECONDS: u64 = 3;

#[repr(C, align(4096))]
struct FreePage([u8; GRANULE_SIZE]);

/// A root-image page whose virtual address becomes the loader's scratch window.
static mut FREE_PAGE: FreePage = FreePage([0; GRANULE_SIZE]);

/// What one fixture task is expected to demonstrate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Role {
    CleanExit,
    DeliberateFault,
}

impl Role {
    const fn directive(self) -> sel4::Word {
        match self {
            Self::CleanExit => DIRECTIVE_EXIT,
            Self::DeliberateFault => DIRECTIVE_FAULT,
        }
    }

    const fn name(self) -> &'static str {
        match self {
            Self::CleanExit => "clean-exit",
            Self::DeliberateFault => "deliberate-fault",
        }
    }
}

/// A fixture task and the declared component whose authority it carries.
#[derive(Clone, Copy)]
struct Fixture {
    id: TaskId,
    role: Role,
    source: &'static str,
    authority: Authority,
    terminated: bool,
}

// ---- shared-buffer phase contract ----
//
// Every constant below is duplicated in `slime-root/child/src/main.rs` under
// the same name. They are a compile-time agreement between the two images, not
// runtime discovery: the child never searches for its shared region, so what
// grants it access is the root's mapping and nothing else.

/// Where the root maps the read-write shared region in the child's VSpace.
/// Well clear of the child image (which loads at `0x200000`) and of its IPC
/// buffer, so the mapping cannot collide with an existing translation.
const SHARED_RW_VADDR: usize = 0x4000_0000;
/// Where the root maps the read-only shared region in the child's VSpace.
const SHARED_RO_VADDR: usize = 0x4001_0000;

/// Byte offset within each region at which the deterministic pattern lives.
const SHARED_PATTERN_OFFSET: usize = 64;

/// `b"SBUF_RW!"` big-endian: the exact bytes the root writes into the
/// read-write region and the child must read back.
const SHARED_RW_PATTERN: u64 = 0x5342_5546_5f52_5721;
/// `b"CHILD_OK"`: what the child writes back through the shared mapping.
const SHARED_CHILD_REPLY: u64 = 0x4348_494c_445f_4f4b;
/// What the child attempts, and must fail, to store into the read-only region.
const SHARED_RO_INTRUSION: u64 = 0xdead_beef_dead_beef;

/// Report flags the child returns in the shared-buffer report's MR2.
const REPORT_RW_READBACK_OK: sel4::Word = 1 << 0;
const REPORT_RO_WRITE_REFUSED: sel4::Word = 1 << 1;
/// Set by the root, not the child: the child cannot distinguish a supervised
/// instruction abort from a page that genuinely executed, so the execute-never
/// verdict comes from the root's own fault record.
const REPORT_EXECUTE_REFUSED: sel4::Word = 1 << 2;

/// The holder identity the shared-buffer phase charges. Derived from the
/// clean-exit fixture's logical task id, never from a capability pointer.
const SHARED_HOLDER: HolderId = HolderId(0);

/// Ceiling for the P5.1 fixture phase's shared-buffer holder. Two single-page
/// regions and two mappings is exactly what this phase needs; the phase fails
/// closed rather than raising its own ceiling.
///
/// This phase alone, and only because it has no generation to read: the fixture
/// is an ELF the root task embeds at compile time, not a declared component, so
/// there is no budget resource naming it. The component graph resolves every
/// holder's ceiling from the generation's `shared-buffer-budget` resource —
/// see [`declared_quota`] — and never consults this constant.
const SHARED_QUOTA: HolderQuota = HolderQuota {
    byte_pages: 2,
    buffer_count: 2,
    mapping_count: 2,
    loan_count: 0,
};

/// Supervised protection probes the shared-buffer phase expects. One store to
/// a read-only mapping, one branch into an execute-never page. A third fault
/// from the clean-exit fixture is not part of the contract and is treated as a
/// real failure.
const SHARED_EXPECTED_PROBES: usize = 2;

/// What the root observed while supervising the clean-exit fixture's
/// shared-buffer phase.
#[derive(Clone, Copy, Default)]
struct BufferPhase {
    /// Flags the child reported plus the execute-never verdict the root adds.
    flags: sel4::Word,
    /// The pattern word the child claims it read from the shared frame.
    observed: sel4::Word,
    /// Whether the child's report arrived at all.
    reported: bool,
    /// Supervised protection faults handled so far, bounding the resume path.
    probes: usize,
    /// A fault that did not match an expected probe. Any occurrence fails the
    /// phase; it is never silently resumed.
    unexpected: usize,
}

// The stack must clear the deepest frame the service loop reaches, which is a
// shared-buffer teardown: `SharedBufferTable::unmap` builds a `TeardownPlan`
// and an `ActionList` as locals, and both are fixed-size arrays sized for the
// whole table. At 256 KiB that frame ran off the bottom of the stack and the
// root task took a VM fault on `FREE_PAGE` — the scratch page `ScratchPage`
// deliberately leaves unmapped, which is the only reason the overflow was
// visible rather than silent corruption of whatever `.bss` lay below.
//
// This is backlog B3's failure mode a second time, in the same repository, for
// the same reason: a table sized for the graph, built in a stack frame. The
// bound is stated here rather than discovered again.
#[root_task(stack_size = 1024 * 1024, heap_size = 1024 * 128)]
fn main(bootinfo: &sel4::BootInfoPtr) -> ! {
    let generation = match Generation::decode(GENERATION_BYTES) {
        Ok(generation) => generation,
        Err(error) => fatal!("generation rejected: {error:?}"),
    };
    // The profile this root task runs. Every component payload is admitted
    // against it before the loader is offered a byte, so an executable built
    // for another target is refused rather than mapped (roadmap invariant 9).
    let profile = match boot_contracts::target_profile::TargetProfile::by_name(TARGET_PROFILE) {
        Ok(profile) => profile,
        Err(error) => fatal!("target profile {TARGET_PROFILE} unavailable: {error:?}"),
    };
    let admission = match Admission::admit(&generation, profile) {
        Ok(admission) => admission,
        Err(error) => fatal!("generation admission rejected: {error:?}"),
    };

    sel4::debug_println!(
        "SLIME_ROOT generation admitted number={} components={} grants={} health={} kernel={} bootstrap={}",
        generation.number,
        admission.len(),
        admission.grants,
        admission.health,
        admission.kernel_objects,
        admission.bootstrap_objects,
    );
    sel4::debug_println!(
        "SLIME_ROOT authority manifest={:02x?}",
        generation.authority_manifest_identity()
    );
    // The generation's component payloads are the retired Slime kernel's custom
    // images. Their graph is authoritative for authority derivation; their code
    // is not loadable by this root task and is deliberately never activated.
    sel4::debug_println!(
        "SLIME_ROOT graph admitted; legacy SLIMECM images not activated components={} slimecm={} elf={} unrecognized={}",
        admission.len(),
        admission.slime_component_images,
        admission.loadable,
        admission.unrecognized_images,
    );

    let mut allocator = match ObjectAllocator::new(bootinfo) {
        Ok(allocator) => allocator,
        Err(error) => fatal!("allocator rejected bootinfo: {error:?}"),
    };
    sel4::debug_println!(
        "SLIME_ROOT allocator slots={} untypeds={} bytes={}",
        allocator.slots_remaining(),
        allocator.untyped_count(),
        allocator.untyped_bytes_remaining(),
    );

    // ---- timer phase ----
    // Proves `TimerScheduler` (see `timer.rs`) is driven by a real seL4 IRQ
    // before any fixture task exists: acquire the one architected-timer PPI
    // seL4 leaves for userspace on this platform (`platform_timer.rs`),
    // schedule a short deadline, wait for the interrupt it raises, then
    // confirm the monotonic counter it reads actually advanced. The wait is
    // bounded by wall-clock ticks read directly from hardware rather than by
    // IRQ delivery, so a broken wiring fails loudly instead of hanging boot.
    let mut timer_adapter = match PhysicalTimerAdapter::acquire(&mut allocator) {
        Ok(adapter) => adapter,
        Err(error) => fatal!("timer source unavailable: {error:?}"),
    };
    sel4::debug_println!(
        "SLIME_TIMER acquired irq={TIMER_IRQ} freq_hz={}",
        timer_adapter.frequency_hz(),
    );

    let mut timer_scheduler = TimerScheduler::<1>::new();
    const TIMER_PROOF_OWNER: TaskEpoch = TaskEpoch::new(0, 0);
    let timer_start = match timer_adapter.monotonic_now() {
        Ok(now) => now,
        Err(error) => fatal!("timer clock unreadable: {error:?}"),
    };
    let deadline_ticks = (timer_adapter.frequency_hz() / TIMER_PROOF_DEADLINE_DIVISOR).max(1);
    let (_, scheduled) =
        match timer_scheduler.schedule_after(TIMER_PROOF_OWNER, timer_start, deadline_ticks) {
            Ok(scheduled) => scheduled,
            Err(error) => fatal!("timer proof deadline rejected: {error:?}"),
        };
    if let Err(error) = apply_deadline_programming(&mut timer_adapter, scheduled.programming) {
        fatal!("timer deadline could not be programmed: {error:?}")
    }

    let bound_ticks = timer_adapter
        .frequency_hz()
        .saturating_mul(TIMER_PROOF_BOUND_SECONDS);
    let mut polls: u64 = 0;
    loop {
        if poll_notification(timer_adapter.notification()) == Some(timer_adapter.signal_badge()) {
            break;
        }
        let elapsed = match timer_adapter.monotonic_now() {
            Ok(now) => now.0.wrapping_sub(timer_start.0),
            Err(error) => fatal!("timer clock unreadable while waiting: {error:?}"),
        };
        if elapsed > bound_ticks {
            fatal!(
                "SLIME_TIMER FAIL timeout waited_ticks={elapsed} bound_ticks={bound_ticks} polls={polls}"
            )
        }
        polls += 1;
    }
    sel4::debug_println!(
        "SLIME_TIMER delivered badge={:#x} polls={polls}",
        timer_adapter.signal_badge(),
    );

    // The two post-mutation variants carry the transition the expiry already
    // computed (see `timer.rs`), so even a failing platform step reports how
    // many wakes were decided instead of dropping them silently.
    let drained = match timer_scheduler.service_timer_source(&mut timer_adapter, |_| true) {
        Ok(transition) => transition,
        Err(ServiceTimerError::Program { error, transition }) => fatal!(
            "timer deadline reprogramming failed after delivery: {error:?} wakes={}",
            transition.events.len()
        ),
        Err(ServiceTimerError::Acknowledge { error, transition }) => fatal!(
            "timer acknowledgement failed after delivery: {error:?} wakes={}",
            transition.events.len()
        ),
        Err(error) => fatal!("timer service rejected the observed expiry: {error:?}"),
    };
    let timer_end = match timer_adapter.monotonic_now() {
        Ok(now) => now,
        Err(error) => fatal!("timer clock unreadable after service: {error:?}"),
    };
    sel4::debug_println!(
        "SLIME_TIMER serviced events={} programming={:?}",
        drained.events.len(),
        drained.programming,
    );
    sel4::debug_println!(
        "SLIME_TIMER advanced start={} end={} delta={}",
        timer_start.0,
        timer_end.0,
        timer_end.0.wrapping_sub(timer_start.0),
    );
    sel4::debug_println!("SLIME_TIMER OK");
    // ---- end timer phase ----

    let scratch = match ScratchPage::claim(bootinfo, ptr::addr_of!(FREE_PAGE) as usize) {
        Ok(scratch) => scratch,
        Err(error) => fatal!("scratch page unavailable: {error:?}"),
    };
    let image = match ChildImage::parse(&CHILD_ELF.0) {
        Ok(image) => image,
        Err(error) => fatal!("child image rejected: {error:?}"),
    };
    let service_endpoint = match allocator.allocate_fixed::<sel4::cap_type::Endpoint>() {
        Ok(slot) => slot.cap(),
        Err(error) => fatal!("service endpoint unavailable: {error:?}"),
    };

    // ---- component graph phase (P5.2) ----
    //
    // A generation whose payloads this root task can load gets its declared
    // components launched from those payloads, and that is the whole boot: the
    // graph is the proof, so there is nothing for the native fixture to add.
    //
    // A generation whose payloads are the retired kernel's segment-carrying
    // images cannot be launched, and the fixture path below runs instead. The
    // two are told apart by what the generation actually carries rather than by
    // a build flag, so one root task binary serves both and each says which it
    // took.
    if admission.loadable > 0 {
        launch_component_graph(
            &generation,
            &admission,
            &mut allocator,
            &scratch,
            service_endpoint,
        );
        sel4::init_thread::suspend_self()
    }
    // ---- end component graph phase ----

    // Fixture authority is generation-derived: each task carries the inbound
    // service authority of a declared component the generation grants both send
    // and receive rights to. With no such declaration there is no authority to
    // convey, and startup fails closed rather than inventing one.
    let mut authorities: [Option<(&str, Authority)>; FIXTURE_TASKS] = [None; FIXTURE_TASKS];
    let mut found = 0;
    for plan in admission.plans() {
        if found == FIXTURE_TASKS {
            break;
        }
        let record = match generation.component(plan.component) {
            Ok(record) => record,
            Err(error) => fatal!("component rejected: {error:?}"),
        };
        let authority = match inbound_authority(&generation, plan.component) {
            Ok(authority) => authority,
            Err(error) => fatal!("grant closure rejected: {error:?}"),
        };
        if authority.rights & RIGHT_SEND == 0 || authority.rights & RIGHT_RECV == 0 {
            continue;
        }
        authorities[found] = Some((record.name, authority));
        found += 1;
    }
    if found != FIXTURE_TASKS {
        fatal!(
            "generation declares {found} components with service authority, need {FIXTURE_TASKS}"
        )
    }

    let mut tasks = TaskTable::<MAX_TASKS>::new();
    let mut supervision = SupervisionTable::<MAX_TASKS>::new();
    let mut fixtures: [Option<Fixture>; FIXTURE_TASKS] = [None; FIXTURE_TASKS];

    for (index, role) in [Role::CleanExit, Role::DeliberateFault]
        .into_iter()
        .enumerate()
    {
        let Some((source, authority)) = authorities[index] else {
            fatal!("fixture {index} lost its declared authority")
        };
        let id = match tasks.create(
            &mut allocator,
            &image,
            service_endpoint,
            authority,
            Supervision::SelfManaged,
            sel4::init_thread::slot::VSPACE.cap(),
            &scratch,
            sel4::init_thread::slot::ASID_POOL.cap(),
        ) {
            Ok(id) => id,
            Err(error) => fatal!("child task construction failed: {error:?}"),
        };
        if let Err(error) = supervision.register(id.0, id.0) {
            fatal!("supervision registration failed: {error:?}")
        }
        let Some(task) = tasks.get(id) else {
            fatal!("constructed task {} is missing", id.0)
        };
        sel4::debug_println!(
            "SLIME_ROOT native fixture staged task={} role={} source={} badge={:#x} fault_badge={:#x} grants={} child_slots={} root_slots={} frames={} tables={} entry={:#x}",
            id.0,
            role.name(),
            source,
            id.service_badge(),
            id.fault_badge(),
            authority.grants,
            task.granted_slots(),
            task.cleanup.slot_count(),
            task.vspace.frames_mapped,
            task.vspace.tables_mapped,
            task.entry,
        );
        fixtures[index] = Some(Fixture {
            id,
            role,
            source,
            authority,
            terminated: false,
        });
    }

    sel4::debug_println!(
        "SLIME_ROOT allocations complete tasks={} objects={} slots={} bytes={}",
        tasks.len(),
        allocator.objects_allocated(),
        allocator.slots_allocated(),
        allocator.bytes_allocated(),
    );

    // ---- shared-buffer phase ----
    //
    // Runs after the timer phase and before activation, because the child must
    // find its shared region already mapped the moment it runs. The frames come
    // from the same untyped pool and CSlot range as every other object, and the
    // mappings are installed into the clean-exit fixture's own VSpace.
    let Some(buffer_fixture) = fixtures[0] else {
        fatal!("shared-buffer phase has no clean-exit fixture")
    };
    let Some(buffer_task) = tasks.get(buffer_fixture.id) else {
        fatal!("shared-buffer phase lost its fixture task")
    };
    let child_vspace_cap = VSpaceCap(buffer_task.vspace.vspace.bits() as usize);
    let mut buffers = SharedBufferTable::new(GenerationEpoch(generation.number));
    // The ceiling is declared before any allocation, so every later admission
    // reads generation-declared state instead of a caller argument.
    if let Err(error) = buffers.declare_quota(SHARED_HOLDER, SHARED_QUOTA) {
        fatal!("shared-buffer quota rejected: {error:?}")
    }

    let mut buffer_phase = BufferPhase::default();
    let (rw_handle, rw_frame, ro_handle) = {
        let mut adapter = BufferAdapter::new(&mut allocator);
        let (rw, rw_frame) = match setup_shared_region(
            &mut buffers,
            &mut adapter,
            child_vspace_cap,
            SHARED_RW_VADDR,
            MappingRights::ReadWrite,
            SHARED_RW_PATTERN,
            &scratch,
        ) {
            Ok(created) => created,
            Err(error) => fatal!("SLIME_BUF FAIL rw region: {error}"),
        };
        let (ro, _) = match setup_shared_region(
            &mut buffers,
            &mut adapter,
            child_vspace_cap,
            SHARED_RO_VADDR,
            MappingRights::ReadOnly,
            SHARED_RW_PATTERN,
            &scratch,
        ) {
            Ok(created) => created,
            Err(error) => fatal!("SLIME_BUF FAIL ro region: {error}"),
        };

        // The exact range and rights the table committed, read back from the
        // record rather than from the request, so the marker reports what was
        // installed rather than what was asked for.
        for index in 0..2 {
            let Some(record) = buffers.mapping(index) else {
                fatal!("SLIME_BUF FAIL mapping {index} missing after map")
            };
            sel4::debug_println!(
                "SLIME_BUF mapped buffer={} vaddr={:#x}..{:#x} pages={} rights={} holder={} frames={} tables={}",
                record.buffer.0,
                record.base,
                record.base + record.page_count as usize * PAGE_SIZE,
                record.page_count,
                match record.rights {
                    MappingRights::ReadOnly => "read-only",
                    MappingRights::ReadWrite => "read-write",
                },
                record.holder.0,
                adapter.frames_allocated(),
                adapter.tables_mapped(),
            );
        }
        sel4::debug_println!(
            "SLIME_BUF accounting live={} pages={} mappings={} holder_pages={} orphans={}",
            buffers.live_count(),
            buffers.total_pages(),
            buffers.mapping_count(),
            buffers.holder_pages(SHARED_HOLDER),
            buffers.orphan_count(),
        );
        (rw, rw_frame, ro)
    };

    // Every allocation for every task has succeeded, so activation is safe. One
    // fixture runs at a time: both children share a priority, so serving them
    // sequentially is what makes the serial record ordered.
    let mut activated = 0;
    for index in 0..FIXTURE_TASKS {
        let Some(fixture) = fixtures[index] else {
            continue;
        };
        if let Err(error) = tasks.activate(fixture.id) {
            fatal!("activation failed: {error:?}")
        }
        activated += 1;
        sel4::debug_println!(
            "SLIME_ROOT task activated task={} role={}",
            fixture.id.0,
            fixture.role.name()
        );
        serve(
            service_endpoint,
            index,
            &mut tasks,
            &mut supervision,
            &mut fixtures,
            &mut buffer_phase,
        );
    }

    // Adjudicate the phase from what the root itself observed. The child's own
    // claims are only half the evidence; the execute-never verdict and the
    // probe count come from the root's fault records.
    //
    // Reading the child's write-back requires binding the frame to the root's
    // scratch window, and a frame capability records exactly one mapping, so
    // the child's mapping is removed first. The fixture has already exited, so
    // nothing observes the removal. This is also the first real exercise of the
    // unmap path: the mapping count must drop while the region stays live.
    {
        let mut adapter = BufferAdapter::new(&mut allocator);
        if let Err(error) = buffers.unmap(
            &mut adapter,
            SHARED_HOLDER,
            rw_handle,
            child_vspace_cap,
            SHARED_RW_VADDR,
        ) {
            fatal!("SLIME_BUF FAIL rw unmap: {error:?}")
        }
    }
    if buffers.mapping_count() != 1 || buffers.live_count() != 2 {
        fatal!(
            "SLIME_BUF FAIL unmap accounting mappings={} live={}",
            buffers.mapping_count(),
            buffers.live_count()
        )
    }
    report_buffer_phase(&buffer_phase, rw_frame, &scratch);

    // Teardown: reclaim every frame and mapping the phase created, then confirm
    // the accounting is genuinely back at zero rather than merely unreferenced.
    {
        let mut adapter = BufferAdapter::new(&mut allocator);
        if let Err(error) = buffers.release(&mut adapter, SHARED_HOLDER, rw_handle) {
            fatal!("SLIME_BUF FAIL rw release: {error:?}")
        }
        if let Err(error) = buffers.release(&mut adapter, SHARED_HOLDER, ro_handle) {
            fatal!("SLIME_BUF FAIL ro release: {error:?}")
        }
        let orphans = buffers.orphan_count();
        if orphans != 0 {
            fatal!("SLIME_BUF FAIL teardown left {orphans} orphaned pages")
        }
        if buffers.live_count() != 0
            || buffers.total_pages() != 0
            || buffers.mapping_count() != 0
            || buffers.holder_pages(SHARED_HOLDER) != 0
            || buffers.holder_mappings(SHARED_HOLDER) != 0
        {
            fatal!(
                "SLIME_BUF FAIL teardown incomplete live={} pages={} mappings={} holder_pages={} holder_mappings={}",
                buffers.live_count(),
                buffers.total_pages(),
                buffers.mapping_count(),
                buffers.holder_pages(SHARED_HOLDER),
                buffers.holder_mappings(SHARED_HOLDER),
            )
        }
        sel4::debug_println!(
            "SLIME_BUF teardown unmapped={} revoked={} released={} live=0 pages=0 mappings=0 holder_pages=0 orphans=0",
            adapter.unmapped(),
            adapter.revoked(),
            adapter.released(),
        );
    }
    // ---- end shared-buffer phase ----

    let mut reclaimed_tasks = 0;
    let mut reclaimed_slots = 0;
    for fixture in fixtures.iter().flatten().copied() {
        match supervision.take_termination(fixture.id.0) {
            Ok(Some(termination)) => sel4::debug_println!(
                "SLIME_ROOT task settled task={} role={} termination={termination:?}",
                fixture.id.0,
                fixture.role.name(),
            ),
            Ok(None) => sel4::debug_println!(
                "SLIME_ROOT task unsettled task={} role={}",
                fixture.id.0,
                fixture.role.name()
            ),
            Err(error) => sel4::debug_println!(
                "SLIME_ROOT supervision rejected task={} error={error:?}",
                fixture.id.0
            ),
        }
        match tasks.reclaim(fixture.id) {
            Ok(record) => {
                reclaimed_tasks += 1;
                reclaimed_slots += record.slot_count();
                sel4::debug_println!(
                    "SLIME_ROOT task reclaimed task={} source={} slots={}..{}",
                    fixture.id.0,
                    fixture.source,
                    record.first_slot,
                    record.slot_end,
                );
            }
            Err(error) => fatal!("task reclamation failed: {error:?}"),
        }
    }
    sel4::debug_println!(
        "SLIME_ROOT cleanup tasks={reclaimed_tasks} slots={reclaimed_slots} live={}",
        tasks.len()
    );

    let granted: usize = fixtures
        .iter()
        .flatten()
        .map(|fixture| fixture.authority.grants)
        .sum();
    sel4::debug_println!(
        "SLIME_ROOT READY tasks={activated} grants={granted} declared_grants={} reclaimed_slots={}",
        admission.grants,
        tasks.reclaimed_slots(),
    );

    sel4::init_thread::suspend_self()
}

/// Rights recorded on a shared buffer's own slot.
///
/// The region's real authority lives in the `BufferHandle` the table issued and
/// the quota it charged; this slot's rights only say the task holds it at all,
/// so the buffer plane's own checks stay the single place rights are decided.
const RIGHT_BUFFER_ALL: u64 = u64::MAX;

/// Delegation. Numbered as in `boot-contracts`' generation format and
/// `kernel/src/capability/mod.rs`, which agree; restated here because
/// `slime-root` tests it on a capability rather than on a grant.
const RIGHT_TRANSFER: u64 = 1 << 2;

/// Authority to map a loaned range. The rights a loan capability carries, and
/// the pair the retired kernel's `sys_shared_buffer_loan` installs on the
/// handle it returns.
const RIGHT_BUFFER_MAP: u64 = 1 << 9;

/// Largest component ELF the loader will copy through [`ElfScratch`]. Generous
/// against the five components this profile declares (the largest is ~44 KiB)
/// while keeping the buffer a bounded, statically sized object like every other
/// table in this crate.
const MAX_COMPONENT_ELF_BYTES: usize = 512 * 1024;

/// An 8-byte-aligned staging buffer for one component ELF at a time.
#[repr(align(8))]
struct ElfScratch {
    bytes: [u8; MAX_COMPONENT_ELF_BYTES],
}

/// The staging buffer, `const`-initialized in `.bss`.
///
/// A static rather than a local: at 512 KiB it would overflow the root task's
/// 256 KiB stack, which is exactly the failure B3 recorded — a 10 KiB stack
/// temporary silently corrupting adjacent memory instead of faulting. The same
/// reasoning that made `SHARED_BUFFER_TABLE` a plain `const`-initialized static
/// applies here, and more so.
static mut ELF_SCRATCH: ElfScratch = ElfScratch {
    bytes: [0; MAX_COMPONENT_ELF_BYTES],
};

const _: () = assert!(MAX_COMPONENT_ELF_BYTES >= 64 * 1024);

/// The graph's logical channels, as a static rather than a local.
///
/// Every queued message is stored inline, so this table is tens of kilobytes —
/// far too large to construct in a stack frame. That is backlog B3's failure
/// exactly: the retired kernel's `SharedBufferTable` was first built on a task
/// stack, overflowed it silently, and the boot wedged with no diagnostic.
///
/// It lands in `.data` rather than `.bss`, because `Option<Message>::None` is
/// not all-zero — the inline queue slots have no niche, so their `None` is a
/// non-zero discriminant. That costs image size and nothing else; what matters
/// is that it is not on the stack.
static mut CHANNELS: ChannelTable = ChannelTable::new();

impl ElfScratch {
    /// Copy `elf` into the buffer and return it at a guaranteed 8-byte
    /// alignment. Returns the payload's length when it does not fit, so the
    /// caller reports the bound rather than truncating to it.
    fn hold(&mut self, elf: &[u8]) -> Result<&[u8], usize> {
        let destination = self.bytes.get_mut(..elf.len()).ok_or(elf.len())?;
        destination.copy_from_slice(elf);
        Ok(destination)
    }
}

/// Launch every component whose payload this root task can load (P5.2).
///
/// Each component is built from the ELF its own generation object carries — not
/// from one embedded fixture — with its authority derived from the grants the
/// generation declares for it, and its transfer window recorded so its startup
/// bind can be checked against what was actually mapped.
///
/// Construction is staged exactly as the fixture path stages it: every task is
/// built and every window declared before any of them runs, so a failure part
/// way through leaves tasks that never ran rather than a half-started graph.
fn launch_component_graph(
    generation: &Generation<'_>,
    admission: &Admission,
    allocator: &mut ObjectAllocator,
    scratch: &ScratchPage,
    service_endpoint: sel4::cap::Endpoint,
) {
    let mut tasks = TaskTable::<MAX_TASKS>::new();
    let mut windows = WindowTable::<MAX_TASKS>::new();
    let mut graph = GraphTables::new();
    // The channel table holds every queued message inline — sixteen channels of
    // two sixteen-deep queues of 64-byte messages — so it is tens of kilobytes
    // and must never be constructed in a stack frame. `const`-initialized into
    // `.bss` and taken by reference here, which is the fix backlog B3 records
    // for the identical overflow in the retired kernel's `SharedBufferTable`:
    // there the table was published through a `LazyLock` and first built on a
    // 32 KiB task stack, overflowing it silently so the boot simply wedged.
    //
    // SAFETY: the root task is single-threaded and this is the only reference
    // taken to `CHANNELS`. It is held for the rest of this function, which does
    // not return until the graph is served.
    let channels = unsafe { &mut *ptr::addr_of_mut!(CHANNELS) };
    // Which component index each task came from, and how many executable slots
    // it already holds. Both are needed after the staging loop, by the channel
    // materialization that turns declared send/recv grants into queues.
    let mut launched_components = LaunchedComponents::new();
    let mut cursors = SlotCursors::new();
    let mut launched = 0;
    // The generation format packs object payloads without padding, so an ELF's
    // address inside the blob is whatever the preceding objects left it at,
    // while `object`'s ELF64 parser requires 8-byte alignment. Aligning the
    // generation as a whole is therefore not enough — only the first few images
    // land aligned. Each image is copied through this aligned buffer instead,
    // which keeps the wire format unchanged: the generation is also read by the
    // frozen x86 oracle, so adding padding to it would be a format change made
    // for one reader's parser.
    //
    // SAFETY: the root task is single-threaded and this is the only reference
    // taken to `ELF_SCRATCH`, held for the duration of this loop and released
    // before the function returns.
    let aligned = unsafe { &mut *ptr::addr_of_mut!(ELF_SCRATCH) };

    for plan in admission.loadable_plans() {
        let record = match generation.component(plan.component) {
            Ok(record) => record,
            Err(error) => fatal!("SLIME_GRAPH FAIL component rejected: {error:?}"),
        };
        let object = match generation.object(record.object) {
            Ok(object) => object,
            Err(error) => fatal!("SLIME_GRAPH FAIL object rejected: {error:?}"),
        };
        // Target admission happens again here, at the point of use, and its
        // result is what yields the bytes: the ELF cannot be reached without
        // passing it, so a wrong-target payload is refused before mapping
        // rather than by a check a caller could skip.
        let elf = match boot_contracts::component_image::admit_elf(
            object.bytes,
            match boot_contracts::target_profile::TargetProfile::by_name(TARGET_PROFILE) {
                Ok(profile) => profile,
                Err(error) => fatal!("SLIME_GRAPH FAIL profile unavailable: {error:?}"),
            },
        ) {
            Ok(elf) => elf,
            Err(error) => fatal!(
                "SLIME_GRAPH FAIL component {} refused: {error:?}",
                record.name
            ),
        };
        let elf = match aligned.hold(elf) {
            Ok(elf) => elf,
            Err(len) => fatal!(
                "SLIME_GRAPH FAIL component {} is {len} bytes, over the load bound",
                record.name
            ),
        };
        let image = match ChildImage::parse(elf) {
            Ok(image) => image,
            Err(error) => fatal!(
                "SLIME_GRAPH FAIL component {} image rejected: {error:?}",
                record.name
            ),
        };
        let authority = match inbound_authority(generation, plan.component) {
            Ok(authority) => authority,
            Err(error) => fatal!("SLIME_GRAPH FAIL grant closure rejected: {error:?}"),
        };
        let id = match tasks.create(
            allocator,
            &image,
            service_endpoint,
            authority,
            Supervision::SelfManaged,
            sel4::init_thread::slot::VSPACE.cap(),
            scratch,
            sel4::init_thread::slot::ASID_POOL.cap(),
        ) {
            Ok(id) => id,
            Err(error) => fatal!(
                "SLIME_GRAPH FAIL component {} construction failed: {error:?}",
                record.name
            ),
        };
        let Some(task) = tasks.get(id) else {
            fatal!("SLIME_GRAPH FAIL constructed task {} is missing", id.0)
        };
        if let Err(error) = windows.declare(
            id,
            task.vspace.transfer_window_addr,
            task.vspace.transfer_window,
            task.vspace.transfer_window_alias,
        ) {
            fatal!("SLIME_GRAPH FAIL window declaration rejected: {error:?}")
        }
        // Install the executables this component's outbound `exec | spawn`
        // grants name, at the slots its boot layout numbers them by. This is
        // what makes "launches its declared components with their declared
        // grants" a property of the table rather than a claim: `spawn-service`
        // can start exactly the two executables the generation grants it, and a
        // slot naming anything else resolves to nothing.
        let Ok(table) = graph.create(id) else {
            fatal!(
                "SLIME_GRAPH FAIL capability table unavailable for task {}",
                id.0
            )
        };
        let mut executables = 0;
        for index in 0..generation.grant_count() {
            let Ok(grant) = generation.grant(index) else {
                continue;
            };
            if grant.source != plan.component || grant.rights & RIGHT_EXEC == 0 {
                continue;
            }
            // Slot numbering mirrors the retired kernel's: an executable a
            // component may spawn is addressed by the boot layout's slot for
            // it, and `spawn-service`'s generated command profile compiles
            // against those same numbers.
            executables += 1;
            if let Err(error) = table.install(
                executables,
                graph::Capability {
                    resource: graph::Resource::Executable {
                        component: grant.target,
                    },
                    rights: grant.rights,
                },
            ) {
                fatal!(
                    "SLIME_GRAPH FAIL executable grant {} rejected: {error:?}",
                    grant.name
                )
            }
        }
        if let Err(error) = launched_components.record(plan.component, id) {
            fatal!("SLIME_GRAPH FAIL component index unrecorded: {error:?}")
        }
        if let Err(error) = cursors.declare(id, executables) {
            fatal!("SLIME_GRAPH FAIL slot cursor unavailable: {error:?}")
        }
        sel4::debug_println!(
            "SLIME_GRAPH staged task={} component={} grants={} executables={executables} window={:#x} frames={} tables={} entry={:#x}",
            id.0,
            record.name,
            authority.grants,
            task.vspace.transfer_window_addr,
            task.vspace.frames_mapped,
            task.vspace.tables_mapped,
            task.entry,
        );
        launched += 1;
    }

    sel4::debug_println!(
        "SLIME_GRAPH staged components={launched} loadable={} slimecm={} wrong_target={} unrecognized={}",
        admission.loadable,
        admission.slime_component_images,
        admission.wrong_target_images,
        admission.unrecognized_images,
    );

    // Turn the generation's declared send/recv grants into root-owned queues,
    // before any task runs. A component's first `recv` therefore finds either a
    // channel the generation declared for it or nothing at all — never a
    // half-built graph, and never a channel it was not granted.
    let layout = boot_layout_resource(generation);
    let bootstrap = launched_components.task_for(admission.bootstrap);
    let materialized = match channel::materialize(
        generation,
        layout.as_ref(),
        bootstrap,
        &launched_components,
        channels,
        &mut graph,
        &mut cursors,
    ) {
        Ok(report) => report,
        Err(error) => fatal!("SLIME_GRAPH FAIL channel materialization rejected: {error:?}"),
    };
    sel4::debug_println!(
        "SLIME_GRAPH channels grants={} channels={} queues={} slots={} unplaced={}",
        materialized.grants,
        materialized.channels,
        materialized.queues,
        materialized.slots,
        materialized.unplaced,
    );

    // Every allocation for every component has succeeded, so activation is
    // safe. Ordered by task id so the serial record is deterministic.
    let ids: [Option<TaskId>; MAX_TASKS] = {
        let mut ids = [None; MAX_TASKS];
        for (slot, task) in ids.iter_mut().zip(tasks.tasks()) {
            *slot = Some(task.id);
        }
        ids
    };
    for id in ids.iter().flatten().copied() {
        if let Err(error) = tasks.activate(id) {
            fatal!(
                "SLIME_GRAPH FAIL activation failed task={}: {error:?}",
                id.0
            )
        }
    }
    sel4::debug_println!("SLIME_GRAPH activated components={}", tasks.activated());

    // Every launched task gets the ceiling its generation declared for it,
    // before it can ask for a page, so an allocation is admitted against
    // generation-declared state rather than against whatever the caller asked
    // for — or against a constant compiled into the root task.
    //
    // Keyed by component *name* through `holder_identity`, which is the same
    // derivation `scripts/build/build-generation.py::holder_identity` writes
    // into the budget and the retired kernel reads back
    // (`kernel/src/runtime/generation.rs::shared_buffer_quota`). A component the
    // budget does not name gets `DENY` rather than a default: omission from the
    // table is a statement, not a gap.
    let mut buffers = SharedBufferTable::new(GenerationEpoch(generation.number));
    let budget = shared_buffer_budget(generation);
    // A budget promising more than this root task's fixed tables can ever grant
    // is rejected before any component runs, rather than discovered at runtime
    // by whichever component happens to allocate last. Same check the retired
    // kernel performs at admission (`kernel/src/runtime/generation.rs::decode`),
    // against this crate's own ceilings.
    if let Some(budget) = budget.as_ref()
        && let Err(error) = budget.validate_against(
            shared_buffer::MAX_BUFFER_PAGES as u32,
            shared_buffer::MAX_TOTAL_PAGES as u32,
            shared_buffer::MAX_SHARED_BUFFERS as u32,
            shared_buffer::MAX_MAPPINGS as u32,
            shared_buffer::MAX_LOANS as u32,
        )
    {
        fatal!("SLIME_GRAPH FAIL shared-buffer budget unsatisfiable: {error:?}")
    }
    let mut budgeted = 0;
    for (component, id) in launched_components.iter() {
        let Ok(record) = generation.component(component) else {
            fatal!("SLIME_GRAPH FAIL launched component {component} is unreadable")
        };
        let quota = declared_quota(budget.as_ref(), record.name);
        if quota != HolderQuota::DENY {
            budgeted += 1;
        }
        if let Err(error) = buffers.declare_quota(HolderId(u64::from(id.0)), quota) {
            fatal!("SLIME_GRAPH FAIL quota rejected task={}: {error:?}", id.0)
        }
        sel4::debug_println!(
            "SLIME_GRAPH quota task={} component={} pages={} buffers={} mappings={} loans={}",
            id.0,
            record.name,
            quota.byte_pages,
            quota.buffer_count,
            quota.mapping_count,
            quota.loan_count,
        );
    }
    sel4::debug_println!(
        "SLIME_GRAPH quotas declared={} budgeted={budgeted} holders={}",
        launched_components.len(),
        budget.as_ref().map_or(0, SharedBufferBudget::holder_count),
    );

    serve_component_graph(
        generation,
        service_endpoint,
        &mut tasks,
        &mut windows,
        &mut graph,
        channels,
        &mut buffers,
        allocator,
        scratch,
    );
}

/// Decode the generation's boot-layout resource, if it carries one.
///
/// The layout is what numbers the bootstrap component's capability slots, and
/// `init.rs` compiles against constants generated from the same table, so the
/// slot a component addresses and the slot the root fills are one number. A
/// generation without the resource is not an error here — only a graph whose
/// bootstrap component holds a declared channel needs it, and
/// `channel::materialize` reports that case as `UnlaidSlot` rather than
/// guessing a number.
///
/// A malformed resource *is* an error, but it is not this function's to raise:
/// returning `None` lets a graph that never consults the layout boot, and one
/// that does fails at the point of use with the channel it could not place.
fn boot_layout_resource<'a>(generation: &Generation<'a>) -> Option<BootLayout<'a>> {
    (0..generation.object_count())
        .filter_map(|index| generation.object(index).ok())
        .filter(|object| object.kind == KIND_RESOURCE)
        .find_map(|object| BootLayout::decode(object.bytes).ok())
        .filter(|layout| layout.generation_number() == generation.number)
}

/// Decode the generation's shared-buffer budget resource, if it carries one.
///
/// Located by magic among the `KIND_RESOURCE` objects, exactly as
/// `kernel/src/runtime/generation.rs::budget_object` does — the generation
/// authenticates every object against its digest table, so this decodes bytes
/// whose integrity is already established and enforces only structural
/// validity.
///
/// A generation carrying no budget is not an error: it declares that no
/// component holds any shared-buffer quota, and every holder then resolves to
/// [`HolderQuota::DENY`]. Neither is a malformed one — it also yields `DENY`
/// for every holder, which fails closed. Both are the same answer the retired
/// kernel gives, and it is the conservative one: a component denied a quota it
/// was promised fails at its own probe with a bounded error, whereas a
/// permissive fallback would hand out authority no generation declared.
fn shared_buffer_budget<'a>(generation: &Generation<'a>) -> Option<SharedBufferBudget<'a>> {
    // The *first* magic match decides, and a malformed one is `None` rather
    // than a reason to keep looking. Scanning past it would let a generation
    // carrying one bad budget and one good one resolve the good one, which is
    // not what "the generation declares a budget" means. `budget_object` in the
    // retired kernel returns `Some(Err(..))` here for the same reason.
    let object = (0..generation.object_count())
        .filter_map(|index| generation.object(index).ok())
        .filter(|object| object.kind == KIND_RESOURCE)
        .find(|object| object.bytes.len() >= 8 && object.bytes[..8] == budget_magic::MAGIC)?;
    SharedBufferBudget::decode(object.bytes).ok()
}

/// The ceiling this generation declares for the component named `component`.
///
/// [`HolderQuota::DENY`] when the generation declares no budget or the
/// component is absent from it — authority is never ambient, so a component the
/// budget does not name holds nothing rather than something small.
fn declared_quota(budget: Option<&SharedBufferBudget<'_>>, component: &str) -> HolderQuota {
    let Some(budget) = budget else {
        return HolderQuota::DENY;
    };
    match budget.quota_for(&budget_magic::holder_identity(component)) {
        Some(quota) => HolderQuota {
            byte_pages: quota.byte_pages,
            buffer_count: quota.buffer_count,
            mapping_count: quota.mapping_count,
            loan_count: quota.loan_count,
        },
        None => HolderQuota::DENY,
    }
}

/// Iterations the graph service loop will run before declaring the graph wedged.
///
/// Generous against what the five declared components actually issue — each
/// binds a window, and spawn-service additionally runs a shared-buffer probe
/// and spawns two children — while still bounding a livelock so it fails in
/// seconds rather than burning the gate's whole timeout.
const MAX_GRAPH_ITERATIONS: usize = 512;

/// Serve the root operation surface for the component graph.
///
/// Every arrival is decoded by `ipc::Operation::from_label`, so the whole legacy
/// syscall surface resolves to a bounded answer: an operation this cutover does
/// not mediate returns its ordinary Slime error rather than faulting the caller,
/// which is P5.2's third required check.
#[allow(clippy::too_many_arguments)]
fn serve_component_graph(
    generation: &Generation<'_>,
    endpoint: sel4::cap::Endpoint,
    tasks: &mut TaskTable<MAX_TASKS>,
    windows: &mut WindowTable<MAX_TASKS>,
    graph: &mut GraphTables,
    channels: &mut ChannelTable,
    buffers: &mut SharedBufferTable,
    allocator: &mut ObjectAllocator,
    scratch: &ScratchPage,
) {
    let mut live = tasks.len();
    let mut unsupported = 0;
    let mut unimplemented = 0;
    let mut buffers_served = 0;
    let mut loans_served = 0;
    let mut sends = 0;
    let mut receives = 0;
    let mut parks = 0;
    let mut peer_deaths = 0;
    let mut parked = ParkedReplies::new();
    // Capabilities between their send and the receive that collects them. Held
    // here rather than in either task's table, because in flight they belong to
    // neither; see `transit.rs`.
    let mut transit = Transit::new();
    for _ in 0..MAX_GRAPH_ITERATIONS {
        if live == 0 {
            break;
        }
        // Through `recv_request` rather than a hand-rolled register read, so the
        // bound `graph.rs` documents is the bound the loop enforces: a message
        // longer than the fast registers, or one carrying real seL4 extra-caps,
        // is refused here instead of being silently truncated. Slime capability
        // transfer is by logical slot number in the payload; the transport never
        // moves a seL4 capability, and this is what makes that checkable.
        let reception = ipc::recv_request(endpoint);
        let (info, badge) = (reception.info, reception.badge);
        let Some((id, arrival)) = TaskId::from_badge(badge) else {
            sel4::debug_println!("SLIME_GRAPH unbadged arrival badge={badge:#x} rejected");
            ipc::reply(Response::error(IpcError::InvalidOperation));
            continue;
        };
        if tasks.get(id).is_none() {
            sel4::debug_println!("SLIME_GRAPH unknown task badge={badge:#x} rejected");
            ipc::reply(Response::error(IpcError::InvalidOperation));
            continue;
        }
        if arrival == Arrival::Fault {
            // A fault the root cannot decode is still a fault: the thread is
            // stopped in the kernel and will never run again. Continuing without
            // tearing it down would leave `live` counting a task that can never
            // exit, so the loop would spin to its iteration bound and the graph
            // would look wedged rather than faulted.
            match fault::decode_fault(&info) {
                Ok(detail) => sel4::debug_println!(
                    "SLIME_GRAPH component fault task={} kind={:?} address={:?}",
                    id.0,
                    detail.kind,
                    detail.address,
                ),
                Err(error) => sel4::debug_println!(
                    "SLIME_GRAPH fault undecodable task={} error={error:?}",
                    id.0
                ),
            }
            if let Some(task) = tasks.get(id) {
                let _ = task.suspend();
            }
            // A faulted task is as dead as an exited one to everything holding a
            // channel to it, and its authority and window are as reclaimable.
            // The two paths do the same teardown for the same reason: a peer
            // blocked on a crashed producer would otherwise wait forever, and a
            // table left behind would misreport the terminal marker.
            reclaim_dead_task(
                channels,
                &mut parked,
                windows,
                graph,
                &mut transit,
                buffers,
                allocator,
                scratch,
                id,
                &mut peer_deaths,
            );
            graph.release(id);
            windows.release(id);
            live -= 1;
            continue;
        }

        let request = match reception.request {
            Ok(request) => request,
            Err(error) => {
                sel4::debug_println!(
                    "SLIME_GRAPH request rejected task={} label={} error={error:?}",
                    id.0,
                    info.label()
                );
                ipc::reply(Response::error(error));
                continue;
            }
        };
        let (operation, words) = (request.operation, request.mrs);

        // Every operation that can block is answered through a reply authority
        // saved out of the implicit slot *before* anything else runs, because
        // the implicit slot is transient and the next `recv` overwrites it. A
        // save the operation turns out not to need is answered over and
        // released on the same call, so the non-blocking path costs one CSlot
        // that is handed straight back rather than leaking.
        let saved = if parkable(operation) {
            match parked.save(allocator, id) {
                Ok(saved) => Some(saved),
                Err(error) => {
                    sel4::debug_println!(
                        "SLIME_GRAPH reply authority unavailable task={} error={error:?}",
                        id.0
                    );
                    ipc::reply(Response::error(error));
                    continue;
                }
            }
        } else {
            None
        };

        match operation {
            // The startup declaration of the window the loader already mapped.
            // Checked against that mapping rather than accepted on the caller's
            // word, so a task cannot name a region it does not have.
            Operation::TransferWindowBind => {
                let response =
                    match windows.bind(id, words[0] as u32, words[1] as usize, words[2] as usize) {
                        Ok(window) => {
                            sel4::debug_println!(
                                "SLIME_GRAPH window bound task={} base={:#x} len={}",
                                id.0,
                                window.base,
                                window.len
                            );
                            Response::success(0, 0)
                        }
                        Err(error) => {
                            sel4::debug_println!(
                                "SLIME_GRAPH window bind refused task={} error={error:?}",
                                id.0
                            );
                            Response::error(error)
                        }
                    };
                ipc::reply(response);
            }
            // A clean exit is a send, not a call: the task is suspended rather
            // than replied to.
            Operation::Exit => {
                let status = words[0] as i64;
                sel4::debug_println!("SLIME_GRAPH component exit task={} status={status}", id.0);
                if let Some(task) = tasks.get(id) {
                    let _ = task.suspend();
                }
                reclaim_dead_task(
                    channels,
                    &mut parked,
                    windows,
                    graph,
                    &mut transit,
                    buffers,
                    allocator,
                    scratch,
                    id,
                    &mut peer_deaths,
                );
                graph.release(id);
                windows.release(id);
                live -= 1;
            }
            // Spawn the executable a declared grant named. The slot resolves
            // through the caller's own table, so a component can start exactly
            // the executables its generation granted it and nothing else — an
            // ungranted slot resolves to nothing and is refused.
            Operation::Spawn => {
                let response = match graph.get(id).and_then(|table| table.get(words[0] as u32)) {
                    Some(graph::Capability {
                        resource: graph::Resource::Executable { component },
                        ..
                    }) => {
                        let name = generation
                            .component(component)
                            .map(|record| record.name)
                            .unwrap_or("<unknown>");
                        sel4::debug_println!(
                            "SLIME_GRAPH spawn authorized task={} slot={} component={name}",
                            id.0,
                            words[0],
                        );
                        // Constructing the child is P5.3's work: this slice
                        // proves the authority resolves, and answers with the
                        // bounded error rather than a partial spawn.
                        Response::error(IpcError::UnsupportedOperation)
                    }
                    _ => {
                        sel4::debug_println!(
                            "SLIME_GRAPH spawn refused task={} slot={} ungranted",
                            id.0,
                            words[0],
                        );
                        Response::error(IpcError::InvalidOperation)
                    }
                };
                ipc::reply(response);
            }
            // The shared-buffer plane, answered from the table that already
            // owns rights, quota, and frame accounting. `spawn-service` runs a
            // full create/map/write/seal/unmap/release cycle at startup and
            // exits non-zero if any step fails, so this is the operation set
            // that decides whether the declared graph reaches its service loop.
            Operation::SharedBufferCreate => {
                let holder = HolderId(u64::from(id.0));
                // The caller's own request, both fields. `slot_with_flag` packs
                // the writability into bit 32 of the same word as the factory
                // slot, exactly as `SharedBufferMap` reads it — a region created
                // writable when its creator asked for read-only would carry
                // `BufferRights::WRITE`, so the root would be widening rights
                // past what was requested.
                let writable = words[0] >> 32 != 0;
                let pages = words[1] as usize;
                let response = match serve_buffer_create(
                    buffers, allocator, holder, pages, writable,
                ) {
                    Ok(handle) => match graph.get_mut(id).and_then(|table| {
                        let slot = table.free_slot_from(1)?;
                        table
                            .install(
                                slot,
                                graph::Capability {
                                    resource: graph::Resource::SharedBuffer { handle },
                                    rights: RIGHT_BUFFER_ALL,
                                },
                            )
                            .ok()?;
                        Some(slot)
                    }) {
                        Some(slot) => {
                            buffers_served += 1;
                            sel4::debug_println!(
                                "SLIME_GRAPH buffer created task={} slot={slot} id={} pages={pages} writable={}",
                                id.0,
                                handle.id.0,
                                u8::from(writable),
                            );
                            Response::success(i64::from(slot), handle.id.0)
                        }
                        None => {
                            sel4::debug_println!(
                                "SLIME_GRAPH buffer slot unavailable task={}",
                                id.0
                            );
                            Response::error(IpcError::DestinationSlotsExhausted)
                        }
                    },
                    Err(error) => {
                        sel4::debug_println!(
                            "SLIME_GRAPH buffer create refused task={} pages={pages} class={}",
                            id.0,
                            buffer_error_class(error),
                        );
                        Response::error(buffer_error_status(error))
                    }
                };
                ipc::reply(response);
            }
            // The remaining shared-buffer operations act on a region this task
            // already holds, so they are answered against the same table.
            Operation::SharedBufferMap
            | Operation::SharedBufferUnmap
            | Operation::SharedBufferSeal
            | Operation::SharedBufferRelease => {
                let response = serve_buffer_lifecycle(
                    operation,
                    buffers,
                    allocator,
                    tasks,
                    graph,
                    id,
                    &words,
                    &mut buffers_served,
                );
                ipc::reply(response);
            }
            // The loan plane. A loan is the one authority this cutover moves
            // between components, and it is the narrow one: read-only over an
            // exact sealed subrange, bound to a receiver the lender named
            // through a capability, and settled exactly once.
            Operation::SharedBufferLoan => {
                let response = serve_buffer_loan(
                    buffers,
                    allocator,
                    graph,
                    channels,
                    id,
                    &words,
                    &mut loans_served,
                );
                ipc::reply(response);
            }
            Operation::SharedBufferLoanMap
            | Operation::SharedBufferReturn
            | Operation::SharedBufferRevoke => {
                let response = serve_loan_lifecycle(
                    operation,
                    buffers,
                    allocator,
                    tasks,
                    graph,
                    id,
                    &words,
                    &mut loans_served,
                );
                ipc::reply(response);
            }
            // The channel plane. A slot resolves through the caller's own table
            // with the right the operation needs, so a component can only send
            // on a channel it was granted `send` over and only receive on one it
            // was granted `recv` over — and an ungranted slot is refused
            // identically to an underpowered one, so the table cannot be probed.
            Operation::Send => {
                let saved = saved.expect("send is parkable");
                let response = serve_send(
                    channels,
                    graph,
                    windows,
                    &mut parked,
                    &mut transit,
                    scratch,
                    id,
                    &words,
                    &mut sends,
                    &mut receives,
                );
                parked.answer_saved(saved, response);
            }
            Operation::Recv => {
                let saved = saved.expect("recv is parkable");
                match serve_recv(
                    channels,
                    graph,
                    windows,
                    &mut transit,
                    scratch,
                    id,
                    &words,
                    &mut receives,
                ) {
                    Ok(response) => parked.answer_saved(saved, response),
                    // Nothing queued and the peer is alive. Hold the reply
                    // rather than answering `WouldBlock`: the component is
                    // blocked in a call either way, and answering would make it
                    // spin through `wait` and burn the loop's iteration bound.
                    Err(channel) => {
                        if let Err(error) = channels.register_wait(id, WaitTarget::Receive(channel))
                        {
                            parked.answer_saved(saved, Response::error(error));
                            continue;
                        }
                        match parked.commit(saved, ParkReason::Receive { channel }) {
                            Ok(()) => {
                                parks += 1;
                                sel4::debug_println!(
                                    "SLIME_GRAPH parked task={} channel={channel} reason=recv",
                                    id.0,
                                );
                            }
                            // The caller is blocked in a call, so a refused
                            // park still owes it an answer — the bounded error
                            // that says the receive did not happen. Dropping
                            // the save here would hang it silently.
                            Err((saved, error)) => {
                                channels.clear_waits(id);
                                sel4::debug_println!(
                                    "SLIME_GRAPH park refused task={} error={error:?}",
                                    id.0
                                );
                                parked.answer_saved(saved, Response::error(error));
                            }
                        }
                    }
                }
            }
            Operation::Wait => {
                let saved = saved.expect("wait is parkable");
                match serve_wait(channels, graph, windows, scratch, id, &words) {
                    // A source was already ready, so the wait is answered at
                    // once and the caller re-polls, exactly as `slime_rt::wait`
                    // documents.
                    Ok(true) => parked.answer_saved(saved, Response::success(0, 0)),
                    Ok(false) => match parked.commit(saved, ParkReason::Wait) {
                        Ok(()) => {
                            parks += 1;
                            sel4::debug_println!("SLIME_GRAPH parked task={} reason=wait", id.0);
                        }
                        // As for `recv` above: a refused park must still answer
                        // the blocked caller.
                        Err((saved, error)) => {
                            channels.clear_waits(id);
                            sel4::debug_println!(
                                "SLIME_GRAPH park refused task={} error={error:?}",
                                id.0
                            );
                            parked.answer_saved(saved, Response::error(error));
                        }
                    },
                    Err(error) => {
                        channels.clear_waits(id);
                        parked.answer_saved(saved, Response::error(error));
                    }
                }
            }
            // Every other label resolves to a bounded answer, but the two
            // reasons an operation goes unanswered are kept apart.
            //
            // `unmediated_response()` returning `Some` means the plane has no
            // seL4 mechanism owner in this cutover — storage, directory, input,
            // generation management, recovery. That is the designed answer, and
            // it is P5.2's third required check: the caller gets an ordinary
            // Slime error and keeps running.
            //
            // `None` means the operation *is* root-mediated and this dispatcher
            // simply has no handler for it yet. Collapsing the two would report
            // a gap in this slice as a property of the cutover, so it is
            // counted and named separately — the loan plane, `spawn`'s child
            // construction, and supervision are the live examples, and they are
            // P5.3.2 and P5.3.3's work.
            other => {
                let response = match other.unmediated_response() {
                    Some(response) => {
                        unsupported += 1;
                        sel4::debug_println!(
                            "SLIME_GRAPH unsupported operation task={} operation={} result={} caller_survives=1",
                            id.0,
                            other.label(),
                            response.result,
                        );
                        response
                    }
                    None => {
                        unimplemented += 1;
                        sel4::debug_println!(
                            "SLIME_GRAPH unimplemented operation task={} operation={} result={} caller_survives=1",
                            id.0,
                            other.label(),
                            IpcError::UnsupportedOperation.slime_status(),
                        );
                        Response::error(IpcError::UnsupportedOperation)
                    }
                };
                ipc::reply(response);
            }
        }
    }
    sel4::debug_println!(
        "SLIME_GRAPH served live={live} unsupported={unsupported} unimplemented={unimplemented} buffers={buffers_served} windows={} tables={}",
        windows.len(),
        graph.len(),
    );
    // The channel plane's own accounting, kept on its own line so P5.2's
    // terminal marker keeps the exact shape its gate already asserts.
    //
    // `parked=0` and `queues=0` together are what make teardown complete: no
    // task is still blocked on a reply the root owes it, and no queue still
    // believes it has a live peer. `replies` is every saved CSlot handed back,
    // which is what shows the save path is not a leak.
    sel4::debug_println!(
        "SLIME_GRAPH channels served sends={sends} receives={receives} parks={parks} settled={peer_deaths} parked={} queues={} replies={}",
        parked.len(),
        channels.live_queues(),
        parked.recycled(),
    );
    // The loan plane's accounting, on its own line for the same reason: P5.3.1's
    // gate asserts the line above by its exact shape.
    //
    // The four zeros are what make reclamation observable. `loans`, `mappings`,
    // and `regions` are the shared-buffer table's own live counts, so a loan
    // whose lender died without settling, or a mapping a dead receiver still
    // held, shows up here rather than as memory quietly retained. `transit` is
    // capabilities in flight — one still parked at teardown is one no task can
    // ever name.
    sel4::debug_println!(
        "SLIME_GRAPH loans served={loans_served} loans={} mappings={} regions={} transit={} orphans={} aliases={}",
        buffers.loan_count(),
        buffers.mapping_count(),
        buffers.live_count(),
        transit.len(),
        buffers.orphan_count(),
        buffer_adapter::live_frame_aliases(),
    );
}

/// Whether this operation may end with the caller parked, and so needs its
/// reply authority saved before anything else runs.
///
/// `send` is included even though it never parks: it can still fail after the
/// window read, and answering a caller whose implicit reply slot is intact
/// while other operations answer over saved capabilities would make the reply
/// path depend on which arm ran. One rule for the whole channel plane is easier
/// to hold correct than three.
const fn parkable(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::Send | Operation::Recv | Operation::Wait
    )
}

/// Resolve a slot through the caller's own table to the channel it names,
/// requiring `rights`.
fn resolve_channel(
    graph: &GraphTables,
    id: TaskId,
    slot: u32,
    rights: u64,
) -> Result<ipc::ChannelKey, IpcError> {
    let table = graph.get(id).ok_or(IpcError::InvalidOperation)?;
    match table.resolve(slot, rights)?.resource {
        graph::Resource::Endpoint { channel } => Ok(channel),
        // A slot the task holds but that names something else — an executable,
        // a factory, a buffer — is refused exactly as an ungranted one is.
        _ => Err(IpcError::InvalidOperation),
    }
}

/// Enqueue one message onto the channel the caller named.
///
/// A capability the message carries is moved out of the caller's table and
/// parked in the transit table, inside the same atomic step as the enqueue —
/// see [`DepartingCaps`]. Only a loan can move; every other resource kind is
/// authority the generation placed, and the refusal is a bounded Slime error
/// with the caller still running.
#[allow(clippy::too_many_arguments)]
fn serve_send(
    channels: &mut ChannelTable,
    graph: &mut GraphTables,
    windows: &WindowTable<MAX_TASKS>,
    parked: &mut ParkedReplies,
    transit: &mut Transit,
    scratch: &ScratchPage,
    id: TaskId,
    words: &[sel4::Word; ipc::FAST_MESSAGE_REGISTERS],
    served: &mut usize,
    woken_receives: &mut usize,
) -> Response {
    let channel = match resolve_channel(graph, id, words[0] as u32, RIGHT_SEND) {
        Ok(channel) => channel,
        Err(error) => return Response::error(error),
    };
    let frame = match transfer_window::read_staged(windows.bound(id), words[1], words, scratch) {
        Ok(frame) => frame,
        Err(error) => return Response::error(error),
    };
    // Whoever receives on this channel is who the capabilities are bound to,
    // fixed here rather than at collection time: a capability in flight names
    // the task it was sent to, so a later change to who is receiving cannot
    // redirect it.
    let Some(peer) = channels.peer(channel, id) else {
        return Response::error(IpcError::InvalidOperation);
    };
    let slots = match frame.cap_slots() {
        Ok(slots) => slots,
        Err(error) => return Response::error(error),
    };
    let message = match ipc::Message::new(frame.bytes(), &slots[..frame.cap_count()]) {
        Ok(message) => message,
        Err(error) => return Response::error(error),
    };
    let len = message.len();
    let caps = message.cap_count();
    let Some(queue) = channels.send_queue_mut(channel, id) else {
        return Response::error(IpcError::InvalidOperation);
    };
    // Preflight, capability move, and commit are one atomic step over a queue
    // whose revision has not moved, so a refused send enqueues nothing and
    // moves nothing.
    let mut departing = DepartingCaps {
        graph,
        transit,
        sender: id,
        receiver: peer,
        departed: [None; ipc::MAX_MESSAGE_CAPS],
        refusal: None,
    };
    let wake = match ipc::send_atomic(queue, message, &mut departing) {
        Ok(wake) => wake,
        Err(error) => {
            // `send_atomic` re-checks the queue after the move, so a commit can
            // fail with capabilities already parked. They belong to nobody at
            // that point; hand them back to the sender, which still holds them
            // as far as it knows.
            departing.recall_all();
            // The adapter's own reason when it has one, because `send_atomic`
            // reports every adapter failure as `TransferFailed` and a component
            // that named an unmovable capability is owed the refusal rather
            // than a generic failure.
            let error = departing.refusal.unwrap_or(error);
            if error == IpcError::UnsupportedCapabilityTransfer {
                sel4::debug_println!(
                    "SLIME_GRAPH capability transfer refused task={} channel={channel} caps={caps}",
                    id.0,
                );
            }
            return Response::error(error);
        }
    };
    let moved = departing.departed();
    *served += 1;
    sel4::debug_println!(
        "SLIME_GRAPH sent task={} channel={channel} bytes={len} caps={moved} queued={}",
        id.0,
        channels
            .send_queue(channel, id)
            .map_or(0, ipc::Channel::len),
    );
    if moved != 0 {
        sel4::debug_println!(
            "SLIME_GRAPH capability transfer task={} channel={channel} to={} caps={moved}",
            id.0,
            peer.0,
        );
    }
    // A receiver blocked on this queue is owed its answer now: it is parked in
    // a call, so nothing else will make it retry.
    if let Some(wake) = wake {
        // Counted as a receive, because it is one: the woken task's `recv` is
        // completed here rather than retried. Leaving it out would make the
        // send and receive totals disagree by exactly the number of messages
        // that took the wake path, which is the path this slice exists to add.
        if deliver_wake(channels, parked, windows, scratch, graph, transit, wake) {
            *woken_receives += 1;
        }
    }
    Response::success(0, 0)
}

/// Dequeue one message for the caller, or report the channel it must park on.
///
/// `Err(channel)` is not a failure: it is "nothing queued and the peer is
/// alive", which the dispatcher turns into a held reply. A dead peer is an
/// answer and comes back as `Ok`.
#[allow(clippy::too_many_arguments)]
fn serve_recv(
    channels: &mut ChannelTable,
    graph: &mut GraphTables,
    windows: &WindowTable<MAX_TASKS>,
    transit: &mut Transit,
    scratch: &ScratchPage,
    id: TaskId,
    words: &[sel4::Word; ipc::FAST_MESSAGE_REGISTERS],
    served: &mut usize,
) -> Result<Response, ipc::ChannelKey> {
    let channel = match resolve_channel(graph, id, words[0] as u32, RIGHT_RECV) {
        Ok(channel) => channel,
        Err(error) => return Ok(Response::error(error)),
    };
    let Some(queue) = channels.recv_queue_mut(channel, id) else {
        return Ok(Response::error(IpcError::InvalidOperation));
    };
    // The message's capabilities are transit tokens, not slot numbers, so
    // nothing here moves them: the tokens ride out on the dequeued message and
    // `land_caps` resolves them below. The destination-capacity preflight is
    // still real — the queue refuses to hand over a message this task has no
    // room for, leaving it queued.
    let available = graph
        .get(id)
        .map_or(0, |table| graph::MAX_TASK_CAPS - table.len());
    let outcome = match ipc::receive_atomic(queue, available, &mut CarryCapabilities) {
        Ok(outcome) => outcome,
        Err(IpcError::WouldBlock) => return Err(channel),
        Err(error) => return Ok(Response::error(error)),
    };
    // Land the capabilities before the reply is built, because the reply must
    // report the slots they landed at. A landing that fails is not silently
    // dropped: the message is already dequeued, so the capabilities are handed
    // back to the transit table's reclamation rather than lost — see
    // `land_caps`.
    let landed = match land_caps(graph, transit, id, &outcome.message) {
        Ok(landed) => landed,
        Err(error) => return Ok(Response::error(error)),
    };
    let response = deliver_message(windows, scratch, id, &outcome.message, &landed);
    if response.result >= 0 {
        *served += 1;
        sel4::debug_println!(
            "SLIME_GRAPH received task={} channel={channel} bytes={} caps={}",
            id.0,
            outcome.message.len(),
            landed.len(),
        );
    }
    Ok(response)
}

/// Slots one received message's capabilities landed at, in message order.
#[derive(Clone, Copy, Default)]
struct LandedCaps {
    slots: [u32; ipc::MAX_MESSAGE_CAPS],
    len: usize,
}

impl LandedCaps {
    const fn len(&self) -> usize {
        self.len
    }

    fn slots(&self) -> &[u32] {
        &self.slots[..self.len]
    }
}

/// Install each capability a received message carries into the receiver's
/// table, and report the slots.
///
/// The message carries transit tokens; the receiver learns slot numbers. That
/// substitution is the whole point of the transit table — a sender's slot
/// number means nothing in the receiver's table — and it happens here, once,
/// after the message has been dequeued and before the reply names anything.
///
/// All or none. A token that resolves and is then followed by one that does not
/// would otherwise leave the receiver holding half a transfer with no way to
/// learn what it got. On failure every capability already installed is returned
/// to the transit table bound to the same receiver, so it is still reclaimed
/// when either end dies rather than stranded in a table nobody reads.
fn land_caps(
    graph: &mut GraphTables,
    transit: &mut Transit,
    id: TaskId,
    message: &ipc::Message,
) -> Result<LandedCaps, IpcError> {
    let mut landed = LandedCaps::default();
    for token in message.caps().iter().flatten().copied() {
        let Some(capability) = transit.arrive(token, id) else {
            // The token names nothing. That means the capability was reclaimed
            // while this message sat in its queue — its sender or its intended
            // receiver died — and the message has already been dequeued by the
            // time this runs, so refusing here would consume the payload and
            // report only a failure.
            //
            // The bytes are delivered anyway, minus the capability. A message
            // is not void because authority that rode alongside it went away,
            // and a receiver told "transfer failed" would have lost a payload
            // it could still have read.
            sel4::debug_println!(
                "SLIME_GRAPH capability expired task={} bytes={}",
                id.0,
                message.len(),
            );
            continue;
        };
        let outcome = graph
            .get_mut(id)
            .ok_or(IpcError::InvalidOperation)
            .and_then(|table| {
                let slot = table
                    .free_slot_from(0)
                    .ok_or(IpcError::DestinationSlotsExhausted)?;
                table.install(slot, capability)?;
                Ok(slot)
            });
        match outcome {
            Ok(slot) => {
                landed.slots[landed.len] = slot;
                landed.len += 1;
            }
            Err(error) => {
                // This one never landed, so put it back before unwinding the
                // ones that did; otherwise it would be the single capability
                // this path lost.
                //
                // A re-park that itself fails — the transit table full — has
                // nowhere left to put the capability, so it is reported rather
                // than dropped in silence. The terminal `transit=` count cannot
                // show it: a capability that never got back into the table is
                // exactly the one that count stops seeing.
                if transit.depart(capability, id, id).is_err() {
                    sel4::debug_println!(
                        "SLIME_GRAPH FAIL capability lost task={} reason=transit-full",
                        id.0,
                    );
                }
                unland_caps(graph, transit, id, &landed);
                return Err(error);
            }
        }
    }
    Ok(landed)
}

/// Take back every capability [`land_caps`] installed, re-parking each one.
///
/// Re-parked as `id -> id`: the transfer did not complete, and the receiver is
/// the only task that could still legitimately be handed it, so binding it to
/// anyone else would be inventing a destination. It is unreachable either way —
/// no message names its new token — and [`Transit::reclaim`] drops it when the
/// task ends, which is the property that matters: the terminal marker still
/// reaches zero.
fn unland_caps(graph: &mut GraphTables, transit: &mut Transit, id: TaskId, landed: &LandedCaps) {
    let Some(table) = graph.get_mut(id) else {
        return;
    };
    for slot in landed.slots() {
        if let Some(capability) = table.get(*slot) {
            table.drop_slot(*slot);
            // As in `land_caps`: a re-park with nowhere to go is reported. It
            // cannot happen on this path — every entry being returned came out
            // of this table moments ago, so the room is there — but a silent
            // drop is not the failure to choose if it ever does.
            if transit.depart(capability, id, id).is_err() {
                sel4::debug_println!(
                    "SLIME_GRAPH FAIL capability lost task={} reason=transit-full",
                    id.0,
                );
            }
        }
    }
}

/// Write a received message into the caller's window and build its reply.
///
/// The frame carries the *landed* slot numbers, not the tokens the message
/// held: the component's `recv` reads them back as capability slots it can name
/// in a later operation, and a token would name nothing in its table.
fn deliver_message(
    windows: &WindowTable<MAX_TASKS>,
    scratch: &ScratchPage,
    id: TaskId,
    message: &ipc::Message,
    landed: &LandedCaps,
) -> Response {
    let frame = match transfer_window::StagedFrame::from_parts(message.bytes(), landed.slots()) {
        Ok(frame) => frame,
        Err(error) => return Response::error(error),
    };
    // The component's `collect` reads the frame back at the descriptor this
    // reply names, so the two must agree byte for byte about where it is.
    match transfer_window::write_staged(windows.bound(id), &frame, scratch) {
        Ok(()) => Response::success(message.len() as i64, frame.reply_descriptor()),
        Err(error) => Response::error(error),
    }
}

/// Arm the caller's wait set, reporting whether a source was already ready.
///
/// Registration is all-or-nothing across the whole set, and every registration
/// is dropped again when one fires, so a stale waiter can never make a later
/// wake name a task that is no longer parked.
fn serve_wait(
    channels: &mut ChannelTable,
    graph: &GraphTables,
    windows: &WindowTable<MAX_TASKS>,
    scratch: &ScratchPage,
    id: TaskId,
    words: &[sel4::Word; ipc::FAST_MESSAGE_REGISTERS],
) -> Result<bool, IpcError> {
    let count = words[0] as usize;
    if count == 0 || count > ipc::MAX_WAIT_SOURCES {
        return Err(IpcError::InvalidLength);
    }
    let frame = transfer_window::read_staged(windows.bound(id), words[1], words, scratch)?;
    let bytes = frame.bytes();
    if bytes.len() != count * channel::WAIT_RECORD_BYTES {
        return Err(IpcError::InvalidLength);
    }
    let table = graph.get(id).ok_or(IpcError::InvalidOperation)?;

    let mut targets = [None; ipc::MAX_WAIT_SOURCES];
    let mut mediated = 0;
    for (slot, record) in targets
        .iter_mut()
        .zip(bytes.chunks_exact(channel::WAIT_RECORD_BYTES))
    {
        let record = u64::from_le_bytes(record.try_into().map_err(|_| IpcError::InvalidLength)?);
        let target = channel::resolve_wait_source(record, table)?;
        if target != WaitTarget::Unmediated {
            mediated += 1;
        }
        *slot = Some(target);
    }
    // A set naming only planes this cutover does not mediate would park forever,
    // because nothing can ever make one ready. Refuse it, so the caller gets a
    // bounded error and keeps running.
    if mediated == 0 {
        return Err(IpcError::UnsupportedOperation);
    }

    let ready = targets
        .iter()
        .flatten()
        .any(|target| channels.is_ready(id, *target));
    if ready {
        return Ok(true);
    }
    for target in targets.iter().flatten() {
        channels.register_wait(id, *target)?;
    }
    // Probe again after registering: a send that landed between the first probe
    // and the registration would otherwise be a lost wakeup.
    if targets
        .iter()
        .flatten()
        .any(|target| channels.is_ready(id, *target))
    {
        channels.clear_waits(id);
        return Ok(true);
    }
    Ok(false)
}

/// Deliver one wake to whichever task is parked for it.
///
/// Reports whether a queued message was handed over, so the caller can count it
/// as the receive it is. A wake naming a task that is not parked is not an
/// error — its registration outlived the `wait` that made it — and delivers
/// nothing.
fn deliver_wake(
    channels: &mut ChannelTable,
    parked: &mut ParkedReplies,
    windows: &WindowTable<MAX_TASKS>,
    scratch: &ScratchPage,
    graph: &mut GraphTables,
    transit: &mut Transit,
    wake: ipc::WakeDecision,
) -> bool {
    let task = TaskId(wake.task);
    let Some(reason) = parked.reason(task) else {
        return false;
    };
    channels.clear_waits(task);
    let delivered = matches!(reason, ParkReason::Receive { .. });
    let response = match reason {
        // A parked `recv` is owed the message itself, not a nudge to retry: the
        // component is blocked in one call and will not issue another.
        //
        // Capabilities land here exactly as they do on the unparked path: this
        // *is* that task's `recv` completing, just completed by its peer's send
        // rather than by its own call, so a message carrying a loan must land
        // it in the woken task's table and report the slot.
        ParkReason::Receive { channel } => {
            let available = graph
                .get(task)
                .map_or(0, |table| graph::MAX_TASK_CAPS - table.len());
            match channels.recv_queue_mut(channel, task) {
                Some(queue) => {
                    match ipc::receive_atomic(queue, available, &mut CarryCapabilities) {
                        Ok(outcome) => match land_caps(graph, transit, task, &outcome.message) {
                            Ok(landed) => {
                                deliver_message(windows, scratch, task, &outcome.message, &landed)
                            }
                            Err(error) => Response::error(error),
                        },
                        Err(error) => Response::error(error),
                    }
                }
                None => Response::error(IpcError::InvalidOperation),
            }
        }
        // A parked `wait` is owed only the wake; `slime_rt::wait` documents that
        // the caller re-polls every source afterwards.
        ParkReason::Wait => Response::success(0, 0),
    };
    // The wake is delivered unconditionally — a task parked in `wait` is owed
    // its answer just as much as one parked in `recv`, and short-circuiting on
    // `delivered` here would hold it forever. Only the *count* distinguishes
    // them: a `wait` that fired carried no message, and a delivery the queue
    // refused is not a receive either.
    let answered = parked.wake(task, response);
    delivered && answered && response.result >= 0
}

/// Settle every channel a dying task held an end of, and answer whoever was
/// blocked on it.
///
/// This is the half of peer death that a suspend alone does not do. Stopping
/// the thread makes the task stop *producing*; it does not tell the task at the
/// other end that nothing more is coming. A component parked in `recv` is
/// blocked inside a call the root owes an answer to, so without this it waits
/// for a message that can never arrive and the graph never drains.
///
/// The dying task's own parked reply is abandoned rather than answered: there
/// is no one left to receive it, and its CSlot still has to come back.
///
/// The same argument applies to everything else the task held, which is why
/// this also reclaims its shared buffers and its in-flight capabilities:
///
/// - **Buffers and loans.** `SharedBufferTable::reclaim_holder` settles every
///   loan the task lent or received, tears down its mappings, and reclaims the
///   regions it owned. Without it a dead lender's loan stays live and its
///   receiver keeps a mapping of a region nothing can reclaim — the retained
///   pages outlive the task that was charged for them.
/// - **In-flight capabilities.** One parked between a send and a receive
///   belongs to no table, so neither end's release reaches it.
#[allow(clippy::too_many_arguments)]
fn reclaim_dead_task(
    channels: &mut ChannelTable,
    parked: &mut ParkedReplies,
    windows: &WindowTable<MAX_TASKS>,
    graph: &mut GraphTables,
    transit: &mut Transit,
    buffers: &mut SharedBufferTable,
    allocator: &mut ObjectAllocator,
    scratch: &ScratchPage,
    id: TaskId,
    settled: &mut usize,
) {
    parked.abandon(id);
    channels.clear_waits(id);

    let held = channels.held_by(id);
    let mut wakes = channel::DeathWakes::new();
    channels.mark_dead(id, &mut wakes);

    let mut woken = 0;
    for (_, wake) in wakes.drain() {
        // A wake naming a task that is not parked is not an error: it was
        // registered by a `wait` that has since been answered, and the
        // registration outlived it. `deliver_wake` returns without doing
        // anything in that case.
        deliver_wake(channels, parked, windows, scratch, graph, transit, wake);
        woken += 1;
    }
    if held != 0 {
        *settled += held;
        sel4::debug_println!(
            "SLIME_GRAPH peer death task={} channels={held} woken={woken}",
            id.0,
        );
    }

    // After the wakes, not before: a wake can complete a parked `recv` that
    // lands a capability, and reclaiming in-flight entries first would drop one
    // the woken task was about to receive.
    let stranded = transit.reclaim(id);

    // Settle everything the shared-buffer table charged this holder. Reported
    // whenever it did anything, so the terminal accounting can be read against
    // per-task lines rather than only as a total.
    let holder = HolderId(u64::from(id.0));
    let charged = buffers.holder_buffers(holder)
        + buffers.holder_mappings(holder)
        + buffers.holder_loans(holder);
    if charged != 0 || stranded != 0 {
        let mut adapter = BufferAdapter::new(allocator);
        match buffers.reclaim_holder(&mut adapter, holder) {
            Ok(actions) => sel4::debug_println!(
                "SLIME_GRAPH holder reclaimed task={} charges={charged} actions={} stranded={stranded}",
                id.0,
                actions.len(),
            ),
            // Reported rather than fatal: the table keeps whatever it could not
            // tear down as its own recorded state — an orphaned page stays
            // named so it is never revoked while mapped — and the terminal
            // marker's non-zero counts are what surface it.
            Err(error) => sel4::debug_println!(
                "SLIME_GRAPH holder reclaim incomplete task={} class={}",
                id.0,
                buffer_error_class(error),
            ),
        }
    }
}

/// Carry a dequeued message's capabilities through unchanged.
///
/// The receive side's whole policy, because there is nothing for it to do: the
/// values in a queued message are transit tokens, and turning one into a slot
/// number in the receiver's table needs the receiver's table — which
/// `receive_atomic` has no access to and should not. So the tokens ride out on
/// the dequeued message and [`land_caps`] resolves them, after the dequeue has
/// committed and before the reply names anything.
///
/// That still leaves `receive_atomic`'s guarantee intact. Its job here is the
/// destination-capacity preflight: it refuses to hand over a message carrying
/// more capabilities than the receiver has room for, leaving it queued. What it
/// does not do is decide *where* they go.
///
/// The send side is [`DepartingCaps`], which does move things — that is the
/// direction where a capability leaves a table.
struct CarryCapabilities;

impl ipc::CapabilityTransfer for CarryCapabilities {
    type Error = IpcError;

    fn transfer_atomic(
        &mut self,
        capabilities: &[Option<ipc::LogicalCap>; ipc::MAX_MESSAGE_CAPS],
    ) -> Result<[Option<ipc::LogicalCap>; ipc::MAX_MESSAGE_CAPS], Self::Error> {
        Ok(*capabilities)
    }
}

/// Move the sender's capabilities into the transit table, all or none.
///
/// Runs inside `ipc::send_atomic`, between the queue preflight and the commit,
/// which is the only place it can run and stay atomic with the enqueue. Doing
/// the move earlier would strand a capability whenever the enqueue then failed
/// — the sender's table would no longer hold it and no queue would carry its
/// token — and doing it later would mean committing a message naming a
/// transfer that had not happened.
///
/// The input slots are the *sender's* logical slot numbers, as its `send`
/// staged them; the output is transit tokens. `receive_atomic` on the other end
/// never sees a slot number from this table, which is why the two directions
/// use different adapters.
///
/// Every check is inside `transfer_atomic` rather than in the caller, so a
/// failure at the fourth capability rolls the first three back. A partial move
/// is the one outcome the trait exists to exclude.
struct DepartingCaps<'a> {
    graph: &'a mut GraphTables,
    transit: &'a mut Transit,
    sender: TaskId,
    receiver: TaskId,
    /// Every `(original slot, token)` this transfer parked. Recorded because
    /// `send_atomic` can still fail *after* `transfer_atomic` returns — its
    /// commit re-checks the queue — and a capability parked for a message that
    /// was never enqueued belongs to nobody. [`Self::recall_all`] is what the
    /// caller runs on that path.
    departed: [Option<(ipc::LogicalCap, ipc::LogicalCap)>; ipc::MAX_MESSAGE_CAPS],
    /// Why this transfer refused, if it did.
    ///
    /// `send_atomic` collapses every adapter failure to
    /// `IpcError::TransferFailed`, because the trait's error type is the
    /// adapter's own and it cannot know what one means. That is right for the
    /// queue, and wrong for the caller: a component that named a
    /// non-transferable capability should learn that, not "the transfer
    /// failed". So the reason is kept here and read back by `serve_send`.
    refusal: Option<IpcError>,
}

impl ipc::CapabilityTransfer for DepartingCaps<'_> {
    type Error = IpcError;

    fn transfer_atomic(
        &mut self,
        capabilities: &[Option<ipc::LogicalCap>; ipc::MAX_MESSAGE_CAPS],
    ) -> Result<[Option<ipc::LogicalCap>; ipc::MAX_MESSAGE_CAPS], Self::Error> {
        let mut tokens = [None; ipc::MAX_MESSAGE_CAPS];
        for (index, slot) in capabilities
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| slot.map(|slot| (index, slot)))
        {
            // A slot named twice would be taken once and then fail to resolve,
            // reading as an ordinary "no such capability" rather than as the
            // duplicate it is. The retired kernel refuses the same case
            // (`kernel/src/syscall/mod.rs::sys_send`).
            if capabilities[..index].contains(&Some(slot)) {
                self.recall_all();
                self.refusal = Some(IpcError::BadCapability);
                return Err(IpcError::BadCapability);
            }
            match self.depart_one(slot) {
                Ok(token) => {
                    self.departed[index] = Some((slot, token));
                    tokens[index] = Some(token);
                }
                Err(error) => {
                    self.recall_all();
                    self.refusal = Some(error);
                    return Err(error);
                }
            }
        }
        Ok(tokens)
    }
}

impl DepartingCaps<'_> {
    /// Take one capability out of the sender's table and park it.
    ///
    /// The transferability test is on the resource *kind*: it decides whether
    /// the root has a mechanism for the move at all, and today it has one only
    /// for a loan. `RIGHT_TRANSFER` is checked too, but note what it is and is
    /// not doing here — every loan capability is minted carrying it
    /// (`serve_buffer_loan`), so on this path it never discriminates. The
    /// generation's delegation statement is enforced one step earlier, at the
    /// mint: a loan is refused outright over a channel the generation did not
    /// mark `transferable`, so a capability that reaches this function is
    /// already one the generation allowed to move. The bit is kept as a
    /// standing precondition rather than removed, because a future kind minted
    /// without it must not become movable by default.
    fn depart_one(&mut self, slot: ipc::LogicalCap) -> Result<ipc::LogicalCap, IpcError> {
        // A loan is bound to its receiver at the mint, so sending it to anyone
        // else produces a capability the recipient can hold and never use —
        // every operation on it fails `authorize_loan` — while the loan stays
        // charged against the lender until it revokes or dies. Refusing the
        // send instead keeps the lender's own accounting honest, and costs
        // nothing: a loan sent to its declared receiver is the only send that
        // was ever going to work.
        if let Some(graph::Capability {
            resource: graph::Resource::Loan { handle },
            ..
        }) = self
            .graph
            .get(self.sender)
            .and_then(|table| table.get(slot))
            && handle.receiver != HolderId(u64::from(self.receiver.0))
        {
            return Err(IpcError::UnsupportedCapabilityTransfer);
        }
        // Both failures answer `BadCapability`, which is `ERR_BAD_CAP` — what
        // `sys_send` answers for the same two cases, and what a component
        // written against the retired kernel therefore tests for. They are also
        // indistinguishable from each other, so a send cannot be used to probe
        // which slots hold what.
        let table = self
            .graph
            .get_mut(self.sender)
            .ok_or(IpcError::BadCapability)?;
        let capability = table.get(slot).ok_or(IpcError::BadCapability)?;
        if !capability.resource.is_transferable() || !capability.allows(RIGHT_TRANSFER) {
            return Err(IpcError::UnsupportedCapabilityTransfer);
        }
        // Removed before parking, so the capability is in exactly one place at
        // every point: the sender's table, then the transit table, then the
        // receiver's. `rollback` puts it back at the same slot on failure.
        table.drop_slot(slot);
        match self.transit.depart(capability, self.sender, self.receiver) {
            Ok(token) => Ok(token),
            Err(error) => {
                // The transit table refused, so nothing holds the capability.
                // Reinstalling at the slot it just left cannot collide: this is
                // the only path that emptied it, and nothing ran in between.
                let _ = table.install(slot, capability);
                Err(error)
            }
        }
    }

    /// Return every capability this transfer parked to the sender's table, at
    /// the slot each one came from.
    ///
    /// The original slot, not a free one: the sender named those numbers in the
    /// message it sent and will name them again if it retries. Handing back a
    /// different slot would leave a component holding a capability at a number
    /// it never learned.
    ///
    /// Idempotent, so the failure paths inside `transfer_atomic` and the
    /// post-commit path in `serve_send` can both call it. Reinstalling cannot
    /// collide: only this transfer emptied those slots, and nothing else ran in
    /// between.
    fn recall_all(&mut self) {
        for (slot, token) in self.departed.iter_mut().filter_map(Option::take) {
            let Some(capability) = self.transit.recall(token, self.sender) else {
                continue;
            };
            if let Some(table) = self.graph.get_mut(self.sender) {
                let _ = table.install(slot, capability);
            }
        }
    }

    /// How many capabilities this transfer parked.
    fn departed(&self) -> usize {
        self.departed.iter().flatten().count()
    }
}

/// Allocate one shared region for `holder` and admit it against the quota the
/// generation declared.
///
/// The page bound is read from the holder's own declared ceiling, not from a
/// constant: a request past it is refused before a frame is allocated, and the
/// table's own `preflight_buffer_charge` refuses it again against live usage.
/// Both are the same generation-declared number, so a holder the budget does
/// not name is refused here at `byte_pages == 0` rather than allocating a frame
/// the admission below would only hand back.
fn serve_buffer_create(
    buffers: &mut SharedBufferTable,
    allocator: &mut ObjectAllocator,
    holder: HolderId,
    pages: usize,
    writable: bool,
) -> Result<BufferHandle, shared_buffer::SharedBufferError> {
    if pages == 0 || pages > buffers.quota(holder).byte_pages as usize {
        // Named as the class it is rather than as a generic bad argument: a
        // request past the holder's declared page ceiling is a quota refusal,
        // and it is one of the four the milestone requires be observable.
        return Err(shared_buffer::SharedBufferError::QuotaExceeded);
    }
    // One frame per requested page. Allocating a single frame regardless of
    // `pages` would produce a region whose anchor count disagreed with what the
    // caller asked for, and every later range check reads the anchor count — so
    // a two-page request would create a one-page region and then refuse the
    // caller's own two-page mapping as out of range.
    let mut frames = [shared_buffer::FrameCap(0); shared_buffer::MAX_BUFFER_PAGES];
    let requested = frames
        .get_mut(..pages)
        .ok_or(shared_buffer::SharedBufferError::BadSize)?;
    let mut adapter = BufferAdapter::new(allocator);
    let mut allocated = 0;
    // `create` documents that a refused admission leaves the caller owning every
    // anchor it supplied, so the frames are released here on both failure paths.
    // A partial allocation is the same case seen earlier.
    let outcome = (|| {
        for frame in requested.iter_mut() {
            *frame = adapter
                .allocate_frame()
                .map_err(|_| shared_buffer::SharedBufferError::BytesExhausted)?;
            allocated += 1;
        }
        let anchors = shared_buffer::FrameAnchors::from_slice(requested)?;
        buffers.create(holder, anchors, writable)
    })();
    if outcome.is_err() {
        for frame in requested.iter().take(allocated) {
            let _ = adapter.perform(shared_buffer::AdapterAction::ReleaseFrame { frame: *frame });
        }
    }
    outcome
}

/// Mint one loan of a sealed subrange, bound to the receiver the caller named.
///
/// # Naming the receiver
///
/// `receiver_slot` must name a channel end the caller holds, and the loan is
/// bound to the task at the other end of it. The receiver is therefore reached
/// through a capability the generation declared, never through an ambient task
/// id a component supplied — which is the property the exit condition asks for.
///
/// This is *not* how the retired kernel names it. There, `sys_shared_buffer_loan`
/// resolves `receiver_slot` to a `RIGHT_SUPERVISE` handle over the receiving
/// task, minted when `init` spawned it
/// (`kernel/src/syscall/mod.rs::sys_shared_buffer_loan`). This cutover has no
/// spawn — that is P5.3.3 — so no supervision handle exists to name, and there
/// is no reading of a `supervise` grant that would produce one: the x86 grant
/// `sample-plane-receiver-supervision` is `source = init, target = sample-lender`
/// and means "init may hand sample-lender a handle", naming no subject at all.
/// Deriving a subject from it would be inventing a rule the only witness for
/// that right contradicts.
///
/// So the channel peer stands in, and it is a real bound rather than a
/// convenience: a component can only loan to a task the generation already
/// connected it to. P5.3.3 replaces this with the supervision handle once spawn
/// mints one, and the operation's shape does not change — only which resource
/// kind `receiver_slot` resolves to.
#[allow(clippy::too_many_arguments)]
fn serve_buffer_loan(
    buffers: &mut SharedBufferTable,
    allocator: &mut ObjectAllocator,
    graph: &mut GraphTables,
    channels: &ChannelTable,
    id: TaskId,
    words: &[sel4::Word; ipc::FAST_MESSAGE_REGISTERS],
    served: &mut usize,
) -> Response {
    let lender = HolderId(u64::from(id.0));
    let buffer_slot = (words[0] & 0xffff_ffff) as u32;
    let receiver_slot = (words[0] >> 32) as u32;
    let offset = words[1] as usize;
    let length = words[2] as usize;

    // Both slots resolve through the caller's own table. A component that holds
    // neither the buffer nor a channel to the receiver is refused identically
    // to one that holds the wrong kind at that number, so the table cannot be
    // probed by watching which error comes back.
    let Some(graph::Capability {
        resource: graph::Resource::SharedBuffer { handle },
        ..
    }) = graph.get(id).and_then(|table| table.get(buffer_slot))
    else {
        return Response::error(IpcError::BadCapability);
    };
    // The receiver is named through a channel end, so a slot holding nothing
    // and a slot holding real authority of another kind are refused
    // identically: which one it was is not the caller's business, and
    // distinguishing them would let a component map its own table by probing.
    // One marker covers both for the same reason.
    // `RIGHT_SEND` on the end, not merely possession of it. A loan exists to be
    // transferred, and it reaches its receiver over this same channel — so a
    // receive-only end names a peer this component could never deliver to, and
    // minting against it would burn a `loan_count` on a loan nobody can
    // collect. Resolved with the right the delivery will need, exactly as
    // `resolve_channel` does for the send itself.
    let Some(graph::Capability {
        resource: graph::Resource::Endpoint { channel },
        ..
    }) = graph
        .get(id)
        .and_then(|table| table.resolve(receiver_slot, RIGHT_SEND).ok())
    else {
        sel4::debug_println!(
            "SLIME_GRAPH loan refused task={} slot={receiver_slot} class=absent",
            id.0,
        );
        return Response::error(IpcError::BadCapability);
    };
    let Some(peer) = channels.peer(channel, id) else {
        sel4::debug_println!(
            "SLIME_GRAPH loan refused task={} slot={receiver_slot} class=absent",
            id.0,
        );
        return Response::error(IpcError::BadCapability);
    };
    // A loopback channel names the lender itself. Loaning to oneself would
    // charge a loan against a receiver that already owns the region, so it is
    // refused rather than admitted as a degenerate transfer.
    if peer == id {
        return Response::error(IpcError::BadCapability);
    }
    // The generation's delegation bit, on the edge the loan will cross.
    //
    // A loan exists to be transferred — it reaches its receiver over this
    // channel and nowhere else — so an edge the generation did not mark
    // `transferable` cannot carry one, and minting it would produce a
    // capability whose only destination is closed. Refusing at the mint is what
    // makes the bit load-bearing rather than decorative: without this check the
    // *kind* alone decides, and `transferable = false` in a manifest would
    // change nothing observable.
    if channels.transferable(channel) != Some(true) {
        sel4::debug_println!(
            "SLIME_GRAPH loan refused task={} slot={buffer_slot} class=undelegated",
            id.0,
        );
        // `BadCapability`, the same answer as an absent slot and a wrong-kind
        // one — and deliberately so. A distinct code here would be a free
        // oracle: this check runs before `buffers.loan()`, so it consumes no
        // quota and leaves no state, and a component could sweep every slot
        // number learning which hold channel ends. That is exactly what
        // `CapabilityTable::resolve` refuses to leak, and what
        // `sys_shared_buffer_loan` answers `ERR_BAD_CAP` for.
        //
        // The marker above keeps the distinction where only the root can read
        // it.
        return Response::error(IpcError::BadCapability);
    }
    let receiver = HolderId(u64::from(peer.0));

    // The table decides: it holds the region's rights, its sealed state, the
    // range, and the lender's `loan_count` ceiling. Nothing is re-checked here
    // that it already checks, so there is one place a loan can be refused.
    let handle = match buffers.loan(lender, receiver, handle, offset, length) {
        Ok(handle) => handle,
        Err(error) => {
            sel4::debug_println!(
                "SLIME_GRAPH loan refused task={} slot={buffer_slot} class={}",
                id.0,
                buffer_error_class(error),
            );
            return Response::error(buffer_error_status(error));
        }
    };
    // The loan capability goes to the *lender*, which is what the ABI returns
    // and what `sample-lender` then names in its `send`. The receiver gets it
    // only when that send delivers — a loan the lender minted but never
    // transferred is one the receiver cannot map.
    let installed = graph.get_mut(id).and_then(|table| {
        let slot = table.free_slot_from(1)?;
        table
            .install(
                slot,
                graph::Capability {
                    resource: graph::Resource::Loan { handle },
                    rights: RIGHT_BUFFER_MAP | RIGHT_TRANSFER,
                },
            )
            .ok()?;
        Some(slot)
    });
    let Some(slot) = installed else {
        // The loan exists in the table but the lender cannot name it, so it
        // would be charged against the quota forever. Revoking is the only way
        // back to the state before the call.
        //
        // A fresh loan has no mappings, so the teardown this drives issues no
        // adapter action at all — but it is run through the real adapter rather
        // than assumed empty, because "a loan just minted maps nothing" is a
        // property of the table, not something this call site should encode.
        let mut adapter = BufferAdapter::new(allocator);
        let _ = buffers.revoke_loan(&mut adapter, lender, handle);
        sel4::debug_println!("SLIME_GRAPH loan slot unavailable task={}", id.0);
        return Response::error(IpcError::DestinationSlotsExhausted);
    };
    *served += 1;
    sel4::debug_println!(
        "SLIME_GRAPH loan created task={} slot={slot} id={} to={} offset={offset} length={length}",
        id.0,
        handle.id.0,
        peer.0,
    );
    Response::success(i64::from(slot), handle.id.0)
}

/// Answer loan-map, return, and revoke for a loan the caller holds.
///
/// Each resolves the loan through the caller's own table, and the table's own
/// `authorize_loan` then checks the recorded loan agrees about who the receiver
/// is. Two independent checks of the same claim, because they answer different
/// questions: the table lookup asks whether this component was given the loan,
/// and `authorize_loan` asks whether the loan still exists and still names it.
#[allow(clippy::too_many_arguments)]
fn serve_loan_lifecycle(
    operation: Operation,
    buffers: &mut SharedBufferTable,
    allocator: &mut ObjectAllocator,
    tasks: &TaskTable<MAX_TASKS>,
    graph: &mut GraphTables,
    id: TaskId,
    words: &[sel4::Word; ipc::FAST_MESSAGE_REGISTERS],
    served: &mut usize,
) -> Response {
    let holder = HolderId(u64::from(id.0));
    let slot = (words[0] & 0xffff_ffff) as u32;

    // Revoke is the lender's operation and names the *buffer*, plus the loan's
    // assigned identity; the other two are the receiver's and name the loan.
    let handle = if operation == Operation::SharedBufferRevoke {
        let Some(graph::Capability {
            resource: graph::Resource::SharedBuffer { handle: buffer },
            ..
        }) = graph.get(id).and_then(|table| table.get(slot))
        else {
            return Response::error(IpcError::BadCapability);
        };
        // Reconstructed from the buffer the lender holds plus the identity it
        // named. The `receiver` field is a placeholder the revoke path never
        // reads — `revoke_loan` resolves the recorded loan by id and checks the
        // caller is its lender, so the receiver comes from the record.
        shared_buffer::LoanHandle {
            id: shared_buffer::LoanId(words[1]),
            buffer: buffer.id,
            epoch: buffers.epoch(),
            receiver: holder,
        }
    } else {
        let Some(graph::Capability {
            resource: graph::Resource::Loan { handle },
            ..
        }) = graph.get(id).and_then(|table| table.get(slot))
        else {
            return Response::error(IpcError::BadCapability);
        };
        handle
    };

    let Some(task) = tasks.get(id) else {
        return Response::error(IpcError::InvalidOperation);
    };
    let vspace = VSpaceCap(task.vspace.vspace.bits() as usize);
    let mut adapter = BufferAdapter::new(allocator);
    let outcome = match operation {
        Operation::SharedBufferLoanMap => buffers.map_loan(
            &mut adapter,
            holder,
            handle,
            vspace,
            words[1] as usize,
            words[2] as usize,
            words[3] as usize,
        ),
        Operation::SharedBufferReturn => buffers.return_loan(&mut adapter, holder, handle),
        Operation::SharedBufferRevoke => buffers.revoke_loan(&mut adapter, holder, handle),
        _ => Err(shared_buffer::SharedBufferError::NotFound),
    };
    match outcome {
        Ok(()) => {
            *served += 1;
            // A settled loan is no longer authority anyone holds, so the slot
            // naming it is emptied rather than left naming a consumed identity.
            // A second operation on it then finds no capability and is refused,
            // which is what makes single-return observable from the component
            // rather than only inside the table.
            //
            // `revoke` names the *buffer*, not the loan, so its own slot stays
            // — but a lender that minted a loan, kept the capability rather
            // than sending it, and then revoked would otherwise hold a `Loan`
            // naming a torn-down record for the rest of its life. The `Err`
            // cleanup below does not reach that case either: the stale handle
            // fails `authorize_loan` with `WrongReceiver`, not `NotFound`.
            if let Some(table) = graph.get_mut(id) {
                match operation {
                    Operation::SharedBufferReturn => {
                        table.drop_slot(slot);
                    }
                    Operation::SharedBufferRevoke => {
                        drop_loan_slots(table, handle.id);
                    }
                    _ => {}
                }
            }
            sel4::debug_println!(
                "SLIME_GRAPH loan {} task={} slot={slot} id={}",
                loan_operation_name(operation),
                id.0,
                handle.id.0,
            );
            Response::success(0, 0)
        }
        Err(error) => {
            // A receiver whose loan was settled out from under it — the lender
            // revoked it, or died and reclamation tore it down — holds a
            // capability naming nothing. Drop it here so the slot comes back,
            // matching `sys_shared_buffer_return`'s handling of the same case.
            if operation == Operation::SharedBufferReturn
                && error == shared_buffer::SharedBufferError::NotFound
                && let Some(table) = graph.get_mut(id)
            {
                table.drop_slot(slot);
            }
            sel4::debug_println!(
                "SLIME_GRAPH loan {} refused task={} slot={slot} class={}",
                loan_operation_name(operation),
                id.0,
                buffer_error_class(error),
            );
            Response::error(buffer_error_status(error))
        }
    }
}

/// Empty every slot in `table` naming the loan `id`.
///
/// By identity rather than by slot number, because the caller of a revoke names
/// the buffer and never the loan: the slot holding the loan capability is one
/// only this table can find.
fn drop_loan_slots(table: &mut graph::CapabilityTable, id: shared_buffer::LoanId) {
    for slot in 0..graph::MAX_TASK_CAPS as u32 {
        if let Some(graph::Capability {
            resource: graph::Resource::Loan { handle },
            ..
        }) = table.get(slot)
            && handle.id == id
        {
            table.drop_slot(slot);
        }
    }
}

const fn loan_operation_name(operation: Operation) -> &'static str {
    match operation {
        Operation::SharedBufferLoanMap => "mapped",
        Operation::SharedBufferReturn => "returned",
        Operation::SharedBufferRevoke => "revoked",
        _ => "unknown",
    }
}

/// Which ceiling or check refused an operation, as a stable marker token.
///
/// The wire status a component sees is deliberately coarse — `slime_rt` has six
/// codes and every quota class collapses to `ERR_OUT_OF_MEMORY`, exactly as the
/// retired kernel's does. That is the right ABI: a component's response to a
/// full quota does not depend on which of the four ceilings it hit.
///
/// A gate's does. "Each of the four quota classes fails at ceiling+1" is not
/// observable from a status code that says only "quota", so the class is named
/// in the marker instead. Widening the status would change the ABI to make a
/// test easier; widening the marker changes nothing a component can see.
const fn buffer_error_class(error: shared_buffer::SharedBufferError) -> &'static str {
    use shared_buffer::SharedBufferError as Error;
    match error {
        Error::QuotaExceeded => "quota",
        Error::BytesExhausted => "pages",
        Error::ObjectsExhausted => "buffers",
        Error::MappingsExhausted => "mappings",
        Error::LoansExhausted => "loans",
        Error::NotSealed => "unsealed",
        Error::RightsDenied => "rights",
        Error::WriteDenied => "write",
        Error::WrongOwner => "owner",
        Error::WrongReceiver => "receiver",
        Error::BadRange => "range",
        Error::BadSize => "size",
        Error::NotFound => "absent",
        Error::EpochMismatch => "epoch",
        _ => "other",
    }
}

/// The Slime status a shared-buffer failure answers with.
///
/// Every exhausted ceiling is `ERR_OUT_OF_MEMORY` and every authority failure
/// is `ERR_BAD_CAP`, matching `kernel/src/syscall/mod.rs::shared_buffer_error_code`
/// so a component sees one ABI whichever kernel is under it.
const fn buffer_error_status(error: shared_buffer::SharedBufferError) -> IpcError {
    use shared_buffer::SharedBufferError as Error;
    match error {
        // Every exhausted ceiling, whether the holder's declared quota or a
        // fixed table bound, is `ERR_OUT_OF_MEMORY`.
        Error::QuotaExceeded
        | Error::BytesExhausted
        | Error::ObjectsExhausted
        | Error::MappingsExhausted
        | Error::LoansExhausted
        | Error::ChargesExhausted
        | Error::IdentityExhausted => IpcError::DestinationSlotsExhausted,
        // Every authority failure — absent, wrong holder, insufficient rights,
        // stale epoch — is `ERR_BAD_CAP`, indistinguishable to the caller.
        Error::NotFound
        | Error::WrongOwner
        | Error::WrongReceiver
        | Error::RightsDenied
        | Error::WriteDenied
        | Error::NotSealed
        | Error::EpochMismatch => IpcError::BadCapability,
        // A malformed range or size is a bad argument, not bad authority.
        Error::BadSize | Error::BadRange | Error::BadFrameAnchors => IpcError::InvalidLength,
        _ => IpcError::TransferFailed,
    }
}

/// Answer map/unmap/seal/release for a region the caller already holds.
///
/// Every one resolves through the table, which is where rights and quota live,
/// so a task naming a region it does not hold is refused by the same mechanism
/// that bounds one it does.
#[allow(clippy::too_many_arguments)]
fn serve_buffer_lifecycle(
    operation: Operation,
    buffers: &mut SharedBufferTable,
    allocator: &mut ObjectAllocator,
    tasks: &TaskTable<MAX_TASKS>,
    graph: &mut GraphTables,
    id: TaskId,
    words: &[sel4::Word; ipc::FAST_MESSAGE_REGISTERS],
    served: &mut usize,
) -> Response {
    let holder = HolderId(u64::from(id.0));
    let slot = (words[0] & 0xffff_ffff) as u32;
    // The handle is resolved from the caller's own table, never reconstructed
    // from the message: it carries rights and an epoch, so accepting one off
    // the wire would let a component name authority it was not issued.
    let Some(graph::Capability {
        resource: graph::Resource::SharedBuffer { handle },
        ..
    }) = graph.get(id).and_then(|table| table.get(slot))
    else {
        // `ERR_BAD_CAP`, which is what a component tests for: `sample-lender`
        // proves a released buffer is unnameable by requiring exactly that code
        // from the second release, and the slot is empty by then because the
        // first one emptied it.
        return Response::error(IpcError::BadCapability);
    };
    let Some(task) = tasks.get(id) else {
        return Response::error(IpcError::InvalidOperation);
    };
    let vspace = VSpaceCap(task.vspace.vspace.bits() as usize);
    let mut adapter = BufferAdapter::new(allocator);
    let outcome = match operation {
        Operation::SharedBufferMap => {
            let writable = words[0] >> 32 != 0;
            buffers.map(
                &mut adapter,
                holder,
                handle,
                vspace,
                words[1] as usize,
                words[2] as usize,
                words[3] as usize,
                if writable {
                    MappingRights::ReadWrite
                } else {
                    MappingRights::ReadOnly
                },
            )
        }
        Operation::SharedBufferUnmap => {
            buffers.unmap(&mut adapter, holder, handle, vspace, words[1] as usize)
        }
        Operation::SharedBufferSeal => buffers.seal(&mut adapter, holder, handle),
        Operation::SharedBufferRelease => buffers.release(&mut adapter, holder, handle),
        _ => Err(shared_buffer::SharedBufferError::NotFound),
    };
    match outcome {
        Ok(()) => {
            *served += 1;
            // A released region is no longer authority the task holds, so its
            // slot is emptied here rather than left naming a dead handle.
            if operation == Operation::SharedBufferRelease
                && let Some(table) = graph.get_mut(id)
            {
                table.drop_slot(slot);
            }
            Response::success(0, 0)
        }
        Err(error) => {
            // The stage *and* the class, because they answer different
            // questions: which operation was refused, and which ceiling or
            // check refused it. A gate asserting that the mapping quota bites
            // at ceiling+1 needs the second, and the wire status cannot carry
            // it — see `buffer_error_class`.
            sel4::debug_println!(
                "SLIME_GRAPH buffer {} refused task={} slot={slot} class={}",
                buffer_operation_name(operation),
                id.0,
                buffer_error_class(error),
            );
            Response::error(buffer_error_status(error))
        }
    }
}

const fn buffer_operation_name(operation: Operation) -> &'static str {
    match operation {
        Operation::SharedBufferMap => "map",
        Operation::SharedBufferUnmap => "unmap",
        Operation::SharedBufferSeal => "seal",
        Operation::SharedBufferRelease => "release",
        _ => "unknown",
    }
}

/// Serve the badged root endpoint until fixture `index` reaches a terminal state
/// or its bounded iteration budget is spent.
///
/// Requests and faults arrive on the same endpoint object under different
/// badges, because the non-MCS kernel resolves a thread's fault handler in that
/// thread's own CSpace; see `task.rs`.
fn serve(
    endpoint: sel4::cap::Endpoint,
    index: usize,
    tasks: &mut TaskTable<MAX_TASKS>,
    supervision: &mut SupervisionTable<MAX_TASKS>,
    fixtures: &mut [Option<Fixture>; FIXTURE_TASKS],
    buffer_phase: &mut BufferPhase,
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
                    supervision,
                    fixtures,
                    buffer_phase,
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
    supervision: &mut SupervisionTable<MAX_TASKS>,
    fixtures: &mut [Option<Fixture>; FIXTURE_TASKS],
    buffer_phase: &mut BufferPhase,
) {
    let operation = match Operation::from_label(info.label()) {
        Ok(operation) => operation,
        Err(error) => {
            sel4::debug_println!(
                "SLIME_ROOT request rejected task={} label={} error={error:?}",
                id.0,
                info.label()
            );
            ipc::reply(Response::error(error));
            return;
        }
    };
    let Some(role) = fixtures[position].map(|fixture| fixture.role) else {
        ipc::reply(Response::error(IpcError::InvalidOperation));
        return;
    };

    match operation {
        // The fixture's request. Answering it with the task's directive is what
        // proves a grant-derived endpoint carries real service authority.
        Operation::DebugWrite => {
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
                "SLIME_ROOT request badge={:#x} task={} operation={} directive={}",
                id.service_badge(),
                id.0,
                operation.label(),
                role.directive(),
            );
            match supervision.ipc_completed(id.0, operation, 0) {
                Ok(event) => report(&event.kind, id, position, fixtures),
                Err(error) => sel4::debug_println!(
                    "SLIME_ROOT ipc accounting rejected task={} error={error:?}",
                    id.0
                ),
            }
            ipc::reply(Response::success(0, role.directive()));
        }
        // The clean-exit fixture's shared-buffer report. The root records what
        // the child claims and answers immediately; adjudication happens once,
        // after the fixture has finished, in `report_buffer_phase`.
        Operation::SharedBufferMap => {
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
        // A clean exit is a send, not a call: the task is suspended rather than
        // replied to.
        Operation::Exit => {
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
        // Every other label decodes and receives a bounded answer, so no
        // component can fault the root task by naming a plane this cutover does
        // not mediate.
        other => {
            let response = other
                .unmediated_response()
                .unwrap_or(Response::error(IpcError::UnsupportedOperation));
            sel4::debug_println!(
                "SLIME_ROOT request unmediated task={} operation={} result={}",
                id.0,
                other.label(),
                response.result
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
        LifecycleEventKind::IpcCompleted { operation, result } => sel4::debug_println!(
            "SLIME_ROOT child request served task={} role={role} operation={} result={result}",
            id.0,
            operation.label()
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
    Execute,
}

impl Probe {
    const fn name(self) -> &'static str {
        match self {
            Self::ReadOnlyWrite => "ro-write",
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
/// The two probes need different resumption points, because AArch64 reports
/// them differently:
///
/// - a data abort reports the faulting *store*, so the thread resumes at the
///   following instruction (`pc + 4`; every A64 instruction is 4 bytes);
/// - an instruction abort reports the branch *target*, so `pc + 4` would land
///   inside the same non-executable page and fault forever. The thread resumes
///   at its link register instead, which `blr` set to the instruction after the
///   branch.
///
/// Either way the thread advances, so a repeating fault cannot loop: the
/// `SHARED_EXPECTED_PROBES` ceiling in `serve_fault` bounds it a second time.
fn resume_past_probe(
    tasks: &TaskTable<MAX_TASKS>,
    id: TaskId,
    probe: Probe,
) -> Result<(), sel4::Error> {
    const A64_INSTRUCTION_BYTES: sel4::Word = 4;
    const LINK_REGISTER: usize = 30;

    let Some(task) = tasks.get(id) else {
        return Err(sel4::Error::InvalidCapability);
    };
    let mut context = task.tcb.tcb_read_all_registers(false)?;
    let resume_at = match probe {
        Probe::ReadOnlyWrite => context.pc().wrapping_add(A64_INSTRUCTION_BYTES),
        Probe::Execute => *context.gpr(LINK_REGISTER),
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
fn setup_shared_region(
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
        sel4::VmAttributes::DEFAULT | sel4::VmAttributes::EXECUTE_NEVER,
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
        sel4::VmAttributes::DEFAULT | sel4::VmAttributes::EXECUTE_NEVER,
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
fn report_buffer_phase(
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
    if phase.flags & REPORT_EXECUTE_REFUSED == 0 {
        fatal!("SLIME_BUF FAIL execute-never mapping did not refuse execution")
    }
    if phase.probes != SHARED_EXPECTED_PROBES {
        fatal!(
            "SLIME_BUF FAIL supervised {} probe(s), expected {SHARED_EXPECTED_PROBES}",
            phase.probes
        )
    }
    sel4::debug_println!(
        "SLIME_BUF rights enforced ro_write=refused wx_execute=refused probes={} supervised=1",
        phase.probes,
    );
}
