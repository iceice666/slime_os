//! `slime-root`: the seL4 root task that owns Slime's dynamic mechanism.
//!
//! This binary is the entry point; the library holds the stages. Authority
//! always follows a generation declaration, so the order is fixed:
//!
//! 1. `main` takes deterministic ownership of BootInfo CSlots and kernel
//!    untypeds, and installs the root's console where the platform has one;
//! 2. `slime_root::boot::run` proves the root's foundations, then decodes and
//!    admits the generation, classifying every component payload;
//! 3. `slime_root::graph_runtime` derives each child's authority from declared
//!    grants and builds every child task — CSpace, VSpace, TCB, IPC buffer,
//!    badged root endpoint, fault handler — without running any of them;
//! 4. it activates only after every allocation has succeeded, one task at a
//!    time so the serial record is ordered;
//! 5. it serves requests and faults, and reclaims each task's recorded CSlots.
//!
//! A stage that cannot go on prints its record and returns; `main` halts.
//!
//! When no executable of the generation is loadable, `main` runs the fixture
//! below in place of stages 3 to 5: the native child in `components/root-child`,
//! with endpoint authority borrowed from the declared instances.

#![no_main]
#![no_std]
// Startup exercises the allocation, task, IPC, and fault paths; the scheduling,
// timer, and shared-buffer state machines are owned by the library but driven
// by callers a parent integration adds, so not every item is reachable here.
#![allow(dead_code)]

// The mechanism modules live in `slime_root`, the library half of this package
// (B23), so their unit tests can run under `just test_host`. This binary links
// that library rather than recompiling the modules, which is what makes a host
// test result evidence about the root the seL4 image boots.
#[cfg(slime_root_console_pl011)]
use slime_root::device;
use slime_root::{
    buffer_adapter, child_vspace, fault, generation, ipc, object_allocator, private_memory,
    shared_buffer, task, vm_attributes,
};

#[cfg(slime_root_console_pl011)]
use core::ptr;

use sel4_root_task::root_task;

use boot_contracts::generation::{RIGHT_RECV, RIGHT_SEND};
use buffer_adapter::BufferAdapter;
#[cfg(slime_root_console_pl011)]
use child_vspace::GRANULE_SIZE;
use child_vspace::ScratchPage;
use fault::{LifecycleEventKind, SupervisionTable};
use generation::{Authority, bound_authority};
use ipc::{IpcError, Response};
use object_allocator::ObjectAllocator;
use shared_buffer::{
    BufferHandle, GenerationEpoch, HolderId, HolderQuota, MappingRights, PAGE_SIZE,
    SharedBufferTable, VSpaceCap,
};
use task::{Arrival, CHILD_CNODE_SIZE_BITS, MAX_TASKS, Supervision, TaskId, TaskTable};

// B59: the operation labels are generated from
// `contracts/syscall-abi/v1/schema.zt` and shared with `components/runtime`, so
// the dispatcher and the userspace wrapper cannot disagree about what a label
// means. `sel4::Word` is `u64` on every admitted target profile, so the
// generated `u64` constants are the dispatcher's own type.
//
// Two operations' self-scoping is a property of the root's implementation
// rather than of the ABI, so it stays documented here:
// - `capability_table_labels::OCCUPANCY` (C8.13.3) counts the CSpace belonging
//   to the badge the root authenticated, so the request carries no task
//   argument to forge.
// - `shared_buffer_labels::OCCUPANCY` derives its holder from the badge for the
//   same reason.
use slime_proto::syscall_abi::{fixture_labels, lifecycle_labels, shared_buffer_labels};

use slime_root::fatal;
#[cfg(slime_root_console_pl011)]
use slime_root::graph_runtime::FreePage;
use slime_root::graph_runtime::private_memory_cause;
#[cfg(not(slime_root_fixture))]
use slime_root::graph_runtime::{RootEndpoints, RuntimeDevices, launch_instance_graph};

/// The generation this root task admits and launches.
///
/// Supplied by the build harness through `SLIME_GENERATION`, which
/// `scripts/build/build-sel4.py` points at the generation for this build's
/// exact seL4 target profile. The checked-in fixture is the fallback, and it is
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
#[cfg(all(not(slime_boot_selector), slime_generation_supplied))]
macro_rules! generation_bytes {
    () => {
        include_bytes!(env!("SLIME_GENERATION"))
    };
}
#[cfg(all(not(slime_boot_selector), not(slime_generation_supplied)))]
macro_rules! generation_bytes {
    () => {
        include_bytes!("../fixtures/generation.bin")
    };
}

#[cfg(not(slime_boot_selector))]
static GENERATION: Aligned<{ generation_bytes!().len() }> = Aligned(*generation_bytes!());

#[cfg(not(slime_boot_selector))]
const GENERATION_BYTES: &[u8] = &GENERATION.0;

/// The native child fixture, built for this platform's seL4 userspace target.
/// Supplied by the build harness; see `components/root-child`. `include_bytes!` only guarantees
/// byte alignment, but `object`'s ELF64 parser requires an 8-byte-aligned
/// buffer; `Aligned` forces that without a runtime copy into a new allocation.
#[repr(align(8))]
struct Aligned<const N: usize>([u8; N]);

static CHILD_ELF: Aligned<{ include_bytes!(env!("CHILD_ELF")).len() }> =
    Aligned(*include_bytes!(env!("CHILD_ELF")));

/// Fixture request tag, mirroring `components/root-child/src/main.rs`.
const REQUEST_TAG: sel4::Word = 0x534c_494d_4552_4551;

/// Directives the root returns to a fixture in reply MR1.
const DIRECTIVE_EXIT: sel4::Word = 0;
const DIRECTIVE_FAULT: sel4::Word = 1;

/// Child tasks this startup builds: one proving a clean request/exit cycle, one
/// proving fault delivery and supervision.
const FIXTURE_TASKS: usize = 2;

/// Service-loop iterations per fixture. A fixture contributes one request plus
/// one terminal message; the clean-exit fixture additionally contributes its
/// shared-buffer report, two supervised protection faults, and — since C10.1 —
/// its private-memory phase: five growth operations (two size queries, two
/// growths, one refused) plus that phase's own report. Eleven for the clean-exit
/// fixture, so sixteen leaves the same surplus the earlier eight did, bounding
/// unexpected traffic so the loop cannot spin forever.
const MAX_SERVICE_ITERATIONS: usize = 16;

/// Standing window for the root's own console UART.
///
/// Every image for QEMU `virt` maps the PL011 at `0x0900_0000` here and writes
/// its transcript through it. The interactive product reads its keyboard from
/// the same page: a device page is retyped once per boot.
#[cfg(slime_root_console_pl011)]
static mut ROOT_CONSOLE_PAGE: FreePage = FreePage([0; GRANULE_SIZE]);

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
// Every constant below is duplicated in `components/root-child/src/main.rs` under
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

/// Ceiling for the C10.1 fixture phase's private-memory region, in pages.
///
/// Four, which is the smallest number that exercises every property the
/// mechanism claims: two growths (so the base is proven stable across more than
/// one), a batch larger than one page (so a partial mapping would be visible),
/// and a refusal at the ceiling with the region still intact.
///
/// This phase alone, on exactly the grounds [`SHARED_QUOTA`] records: the
/// fixture is an embedded ELF rather than a declared component, so no
/// generation resource names it. Every declared instance sits at
/// deny-by-default zero until C10.2 adds the budget resource that would name
/// one.
const PRIVATE_QUOTA_PAGES: usize = 4;

/// Pages the incremental-rollback fixture retries after its injected failure.
///
/// One large-frame span, not the target's whole region ceiling. What this
/// fixture proves is that a failure partway through a growth preserves the
/// pages already committed and that the retry reaches the same quota over the
/// same base — a property of one span's worth of pages, which the embedded
/// child touches byte by byte. Scaling it with a raised region ceiling would
/// make the fixture walk 64 MiB of guest memory to assert nothing further, and
/// the child's own growth request is this same constant.
#[cfg(slime_private_fail_second_allocation)]
const PRIVATE_RETRY_QUOTA_PAGES: usize = child_vspace::LARGE_FRAME_PAGES;

/// Pages the phases must still hold when the clean-exit fixture reports.
#[cfg(not(slime_private_fail_second_allocation))]
const MEM_EXPECTED_PAGES: usize = PRIVATE_QUOTA_PAGES;
#[cfg(slime_private_fail_second_allocation)]
const MEM_EXPECTED_PAGES: usize = 1 + PRIVATE_RETRY_QUOTA_PAGES;

/// Growth operations that must actually charge pages.
#[cfg(not(slime_private_fail_second_allocation))]
const MEM_EXPECTED_GRANTS: usize = 2;
#[cfg(slime_private_fail_second_allocation)]
const MEM_EXPECTED_GRANTS: usize = 2;

/// The value the clean-exit fixture writes into its first private page and
/// re-reads after a further growth. `components/root-child/src/main.rs::MEM_PATTERN`.
const MEM_PATTERN: u64 = 0x4d454d5f42415345;

/// Marks a report as the private-memory phase's rather than the shared-buffer
/// phase's; both arrive on the same label.
/// `components/root-child/src/main.rs::MEM_REPORT_TAG`.
const MEM_REPORT_TAG: sel4::Word = 0x4d454d5f52505449;

/// Report flags the private-memory phase sets.
/// `components/root-child/src/main.rs::REPORT_MEM_*`.
const REPORT_MEM_QUERY_OK: sel4::Word = 1 << 0;
const REPORT_MEM_FIRST_GROWTH_OK: sel4::Word = 1 << 1;
const REPORT_MEM_ZEROED: sel4::Word = 1 << 2;
const REPORT_MEM_SECOND_GROWTH_OK: sel4::Word = 1 << 3;
const REPORT_MEM_BASE_STABLE: sel4::Word = 1 << 4;
const REPORT_MEM_QUOTA_REFUSED: sel4::Word = 1 << 5;
const REPORT_MEM_REFUSAL_HAD_NO_EFFECT: sel4::Word = 1 << 6;

/// Every flag the phase must set.
///
/// The root owns this rather than the fixture, for the same reason the
/// execute-never verdict is the root's: a phase that judged its own
/// completeness could pass by making fewer observations than it was supposed
/// to. Compared as an exact mask rather than a count, so a missing observation
/// names itself in the report marker.
const MEM_REPORT_ALL: sel4::Word = REPORT_MEM_QUERY_OK
    | REPORT_MEM_FIRST_GROWTH_OK
    | REPORT_MEM_ZEROED
    | REPORT_MEM_SECOND_GROWTH_OK
    | REPORT_MEM_BASE_STABLE
    | REPORT_MEM_QUOTA_REFUSED
    | REPORT_MEM_REFUSAL_HAD_NO_EFFECT;

/// What the root observed of the clean-exit fixture's private-memory phase.
#[derive(Clone, Copy, Default)]
struct MemoryPhase {
    /// Flags the fixture reported.
    flags: sel4::Word,
    /// Whether the report arrived at all.
    reported: bool,
}
/// The allocator's descriptor tables.
///
/// `const`-initialized, so the `MAX`-valued record sentinels place this in
/// `.data` and the image carries its bytes. It must stay a static initializer:
/// the value is multi-megabyte, and any form that constructs it before storing
/// it — `MaybeUninit::new(ObjectAllocator::empty())`, or a local moved into
/// place — materializes it in a frame and overruns the root's 1 MiB stack.
/// Reducing the image cost therefore requires zero-valued sentinels, not a
/// different storage class.
static mut OBJECT_ALLOCATOR: ObjectAllocator = ObjectAllocator::empty();

/// Supervised protection probes the shared-buffer phase expects. One store to
/// a read-only mapping, one branch into an execute-never page. A third fault
/// from the clean-exit fixture is not part of the contract and is treated as a
/// real failure.
///
/// x86-64 expects only the first: seL4 exposes no execute-never frame
/// attribute there, so a data page is executable and the branch probe cannot
/// fault. See `slime_root::vm_attributes`.
#[cfg(not(target_arch = "x86_64"))]
const SHARED_EXPECTED_PROBES: usize = 2;
#[cfg(target_arch = "x86_64")]
const SHARED_EXPECTED_PROBES: usize = 1;

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
// shared-buffer teardown. At 256 KiB that frame ran off the bottom of the
// stack and the root task took a VM fault on `FREE_PAGE` — the scratch page
// `ScratchPage` deliberately leaves unmapped, which is the only reason the
// overflow was visible rather than silent corruption of whatever `.bss` lay
// below.
//
// This is backlog B3's failure mode, in the same repository, for the same
// reason: a table sized for the graph, built in a stack frame. Raising the
// stack to 1 MiB was not enough — `ActionList` reached 144 KiB and every
// by-value return stacked a second copy, so the stream plane's loan teardown
// overflowed again, this time faulting inside `build_actions` itself. It lives
// on the heap now (`ActionList::boxed`), which is why the heap is sized for
// two of them plus room: one held by a teardown in progress, one being built.
//
// The bound is stated here rather than discovered a third time.
#[root_task(stack_size = 1024 * 1024, heap_size = 1024 * 512)]
fn main(bootinfo: &sel4::BootInfoPtr) -> ! {
    // SAFETY: root startup is single-threaded, this is the sole initialization,
    // and the returned reference is held for the root task's lifetime.
    let allocator = unsafe { &mut *core::ptr::addr_of_mut!(OBJECT_ALLOCATOR) };
    if let Err(error) = allocator.initialize(bootinfo) {
        fatal!("allocator rejected bootinfo: {error:?}")
    }
    let root_node = allocator
        .allocate(slime_root::root_cspace::blueprint())
        .unwrap_or_else(|error| fatal!("root CSpace backing: {error:?}"));
    slime_root::root_cspace::install(root_node.cap().downcast())
        .unwrap_or_else(|error| fatal!("root CSpace cutover: {error:?}"));
    allocator.release_slot(root_node.index());
    allocator
        .initialize_metadata(bootinfo)
        .unwrap_or_else(|error| fatal!("root metadata bootstrap: {error:?}"));
    // The first record `main` itself emits: every later byte goes to the
    // root's own UART. The allocator's bring-up records and a fatal error
    // before this line reach the kernel's console only.
    #[cfg(slime_root_console_pl011)]
    #[cfg_attr(not(slime_qemu_keyboard), allow(unused_variables))]
    let console_registers = {
        let console_addr = ptr::addr_of!(ROOT_CONSOLE_PAGE) as usize;
        if let Err(error) = ScratchPage::claim(bootinfo, console_addr) {
            fatal!("root console page unavailable: {error:?}")
        }
        let registers = match device::DeviceRegion::map(
            allocator,
            sel4::init_thread::slot::VSPACE.cap(),
            console_addr,
            device::QEMU_PL011_PADDR,
        ) {
            Ok(registers) => registers,
            Err(error) => fatal!("root console UART unavailable: {error:?}"),
        };
        slime_root::diagnostic::install(console_addr);
        slime_root::diagnostic_println!(
            "SLIME_ROOT console uart=pl011 paddr={:#x}",
            device::QEMU_PL011_PADDR
        );
        registers
    };
    #[cfg_attr(slime_root_fixture, allow(unused_mut))]
    let mut booted = slime_root::boot::run(
        bootinfo,
        allocator,
        slime_root::boot::Inputs {
            #[cfg(not(slime_boot_selector))]
            generation: GENERATION_BYTES,
            child_elf: &CHILD_ELF.0,
            #[cfg(slime_root_console_pl011)]
            console_registers,
        },
    )
    .unwrap_or_else(|stop| slime_root::graph_runtime::halt(stop));
    // A v4 generation launches only explicitly declared root-owned autostart
    // instances. Catalogue-only executables remain inert until a bound exec
    // capability authorizes a userspace spawn.
    #[cfg(not(slime_root_fixture))]
    if booted.admission.loadable > 0 {
        launch_instance_graph(
            &booted.generation,
            &booted.admission,
            bootinfo,
            allocator,
            &booted.scratch,
            RootEndpoints {
                service: booted.service_endpoint,
                console: booted.console_endpoint,
            },
            RuntimeDevices {
                timer: &mut booted.timer_adapter,
                #[cfg(slime_boot_selector)]
                boot_blocks: &mut booted.block_devices,
                input: booted.product_input.take(),
                io_authority: &mut booted.io_authority,
            },
            #[cfg(slime_boot_selector)]
            &mut booted.boot_runtime,
        )
        .unwrap_or_else(|stop| slime_root::graph_runtime::halt(stop));
        #[cfg(all(slime_cv1800b_duo, not(slime_product_uart)))]
        slime_root::boot::request_duo_cold_reset(booted.timer_registers, booted.reset_registers);
        loop {
            core::hint::spin_loop();
        }
    }
    // ---- end generation graph phase ----
    let slime_root::boot::Booted {
        generation,
        admission,
        scratch,
        image,
        service_endpoint,
        console_endpoint,
        ..
    } = booted;

    // The native fallback fixtures borrow endpoint authority from declared
    // instances only when no executable in the generation is loadable.
    let mut authorities: [Option<Authority>; FIXTURE_TASKS] = [None; FIXTURE_TASKS];
    let mut found = 0;
    for instance in admission.instances(&generation) {
        if found == FIXTURE_TASKS {
            break;
        }
        let authority = match bound_authority(&generation, instance) {
            Ok(authority) => authority,
            Err(error) => fatal!("grant closure rejected: {error:?}"),
        };
        if authority.rights & RIGHT_SEND == 0 || authority.rights & RIGHT_RECV == 0 {
            continue;
        }
        authorities[found] = Some(authority);
        found += 1;
    }
    if found != FIXTURE_TASKS {
        fatal!("generation declares {found} instances with service authority, need {FIXTURE_TASKS}")
    }

    let mut tasks = TaskTable::<MAX_TASKS>::new();
    let mut supervision = SupervisionTable::<MAX_TASKS>::new();
    let mut fixtures: [Option<Fixture>; FIXTURE_TASKS] = [None; FIXTURE_TASKS];

    for (index, role) in [Role::CleanExit, Role::DeliberateFault]
        .into_iter()
        .enumerate()
    {
        let Some(authority) = authorities[index] else {
            fatal!("fixture {index} lost its declared authority")
        };
        let id = match tasks.create(
            allocator,
            &image,
            service_endpoint,
            console_endpoint,
            authority,
            Supervision::SelfManaged,
            sel4::init_thread::slot::VSPACE.cap(),
            &scratch,
            sel4::init_thread::slot::ASID_POOL.cap(),
            // The P5.1 fixture path: neither spawned by a component nor built
            // from a generation component, so it carries neither. Every
            // per-component bound reads as absent for it, which is correct —
            // no manifest describes this task.
            None,
            None,
            None,
            0,
            // The fixture path constructs tasks outside any admitted plan,
            // so it keeps the minimum four-slot shell.
            CHILD_CNODE_SIZE_BITS,
            task::ChildSlots::SHELL,
            task::CHILD_PRIORITY,
            // The fixture path predates the thread plan and runs one thread.
            1,
            // No workers, so no worker priorities.
            [task::CHILD_PRIORITY; child_vspace::MAX_CHILD_THREADS],
            // The C10.1 private-memory quota. The clean-exit fixture always
            // exercises the four-page phase. The injected rollback image also
            // gives the deliberate-fault fixture one full window so it can
            // prove a failed initial small-page transaction retries against the
            // retained leaf mapping.
            if role == Role::CleanExit {
                PRIVATE_QUOTA_PAGES
            } else {
                #[cfg(slime_private_fail_second_allocation)]
                {
                    PRIVATE_RETRY_QUOTA_PAGES
                }
                #[cfg(not(slime_private_fail_second_allocation))]
                {
                    0
                }
            },
            // The fixture path carries no policy, so no adaptive window.
            None,
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
        slime_root::diagnostic_println!(
            "SLIME_ROOT native fixture staged task={} role={} source={} badge={:#x} fault_badge={:#x} grants={} child_slots={} root_slots={} frames={} tables={} entry={:#x}",
            id.0,
            role.name(),
            "generation",
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
            source: "generation",
            authority,
            terminated: false,
        });
    }

    slime_root::diagnostic_println!(
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
    let mut memory_phase = MemoryPhase::default();
    let (rw_handle, rw_frame, ro_handle) = {
        let mut adapter = BufferAdapter::new(allocator);
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
            slime_root::diagnostic_println!(
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
        slime_root::diagnostic_println!(
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
        slime_root::diagnostic_println!(
            "SLIME_ROOT task activated task={} role={}",
            fixture.id.0,
            fixture.role.name()
        );
        serve(
            service_endpoint,
            index,
            &mut tasks,
            allocator,
            &mut supervision,
            &mut fixtures,
            &mut buffer_phase,
            &mut memory_phase,
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
        let mut adapter = BufferAdapter::new(allocator);
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
    report_memory_phase(&memory_phase, &tasks);

    // Teardown: reclaim every frame and mapping the phase created, then confirm
    // the accounting is genuinely back at zero rather than merely unreferenced.
    {
        let mut adapter = BufferAdapter::new(allocator);
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
        slime_root::diagnostic_println!(
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
            Ok(Some(termination)) => slime_root::diagnostic_println!(
                "SLIME_ROOT task settled task={} role={} termination={termination:?}",
                fixture.id.0,
                fixture.role.name(),
            ),
            Ok(None) => slime_root::diagnostic_println!(
                "SLIME_ROOT task unsettled task={} role={}",
                fixture.id.0,
                fixture.role.name()
            ),
            Err(error) => slime_root::diagnostic_println!(
                "SLIME_ROOT supervision rejected task={} error={error:?}",
                fixture.id.0
            ),
        }
        match tasks.reclaim(allocator, fixture.id) {
            Ok(record) => {
                reclaimed_tasks += 1;
                reclaimed_slots += record.slot_count();
                slime_root::diagnostic_println!(
                    "SLIME_ROOT task reclaimed task={} source={} slots={} arena={}",
                    fixture.id.0,
                    fixture.source,
                    record.slot_count(),
                    record.arena.index(),
                );
            }
            Err(error) => fatal!("task reclamation failed: {error:?}"),
        }
    }
    // Every private page returned when its task died (C10.1). The frames go
    // with the arena revoke above; this is the charge, which is the half a
    // revoke cannot see — so a page count that stayed nonzero here is a leak
    // the frame allocator would not report.
    {
        let table = tasks.private_memory();
        if table.total_pages() != 0 || table.reclaimed_pages() != table.grown_pages() {
            fatal!(
                "SLIME_MEM FAIL teardown left pages={} grown={} reclaimed={}",
                table.total_pages(),
                table.grown_pages(),
                table.reclaimed_pages(),
            )
        }
        slime_root::diagnostic_println!(
            "SLIME_MEM teardown grown={} reclaimed={} pages=0",
            table.grown_pages(),
            table.reclaimed_pages(),
        );
    }
    slime_root::diagnostic_println!(
        "SLIME_ROOT cleanup tasks={reclaimed_tasks} slots={reclaimed_slots} live={}",
        tasks.len()
    );
    slime_root::diagnostic_println!(
        "SLIME_ROOT allocator live_slots={} live_objects={} live_bytes={} slot_reuses={} extent_reuses={}",
        allocator.live_slots(),
        allocator.live_objects(),
        allocator.live_bytes(),
        allocator.slots_reused(),
        allocator.extents_reused(),
    );

    let granted: usize = fixtures
        .iter()
        .flatten()
        .map(|fixture| fixture.authority.grants)
        .sum();
    slime_root::diagnostic_println!(
        "SLIME_ROOT READY tasks={activated} grants={granted} declared_grants={} reclaimed_slots={}",
        admission.grants,
        tasks.reclaimed_slots(),
    );

    sel4::init_thread::suspend_self()
}

mod fixture_runtime;
use fixture_runtime::{report_buffer_phase, report_memory_phase, serve, setup_shared_region};
