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
// Startup exercises the allocation, task, IPC, and fault paths; the scheduling,
// timer, and shared-buffer state machines are owned by the library but driven
// by callers a parent integration adds, so not every item is reachable here.
#![allow(dead_code)]

// The mechanism modules live in `slime_root`, the library half of this package
// (B23), so their unit tests can run under `just test_host`. This binary links
// that library rather than recompiling the modules, which is what makes a host
// test result evidence about the root the seL4 image boots.
#[cfg(slime_boot_selector)]
use slime_root::boot_selector;
use slime_root::{
    buffer_adapter, channel, child_vspace, device, event, fault, generation, graph, ipc,
    object_allocator, parked, platform_timer, shared_buffer, supervision, task, timer,
    transfer_window, transit, virtio_blk,
};

use core::ptr;

use boot_contracts::generation::{
    Generation, Grant, GrantEndpoint, Instance, InstanceHealth, InstanceOwner, KIND_RESOURCE,
    MintedBinding,
};
use boot_contracts::shared_buffer_budget::{self as budget_magic, SharedBufferBudget};
use sel4_root_task::root_task;

use buffer_adapter::BufferAdapter;
use channel::{ChannelTable, LaunchedInstances, WaitTarget};
use child_vspace::{ChildImage, GRANULE_SIZE, ScratchPage};
use event::TaskEpoch;
use fault::{LifecycleEventKind, SupervisionTable};
use generation::{Admission, Authority, RIGHT_EXEC, RIGHT_RECV, RIGHT_SEND, bound_authority};
use graph::GraphTables;
use ipc::{IpcError, Operation, Response, poll_notification};
use object_allocator::ObjectAllocator;
use parked::{ParkReason, ParkedReplies};
use platform_timer::{PhysicalTimerAdapter, TIMER_IRQ};
use shared_buffer::{
    BufferHandle, GenerationEpoch, HolderId, HolderQuota, MappingRights, PAGE_SIZE,
    SharedBufferAdapter, SharedBufferTable, VSpaceCap,
};
use task::{Arrival, CHILD_CNODE_SIZE_BITS, MAX_TASKS, Supervision, TaskId, TaskTable};
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

#[cfg(slime_boot_selector)]
const BOOT_BUNDLE_IDENTITY: [u8; 32] = decode_hex32(env!("SLIME_BOOT_BUNDLE_IDENTITY"));

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

const fn decode_hex32(value: &str) -> [u8; 32] {
    let bytes = value.as_bytes();
    assert!(bytes.len() == 64);
    let mut out = [0u8; 32];
    let mut index = 0;
    while index < 32 {
        out[index] = (hex_nibble(bytes[index * 2]) << 4) | hex_nibble(bytes[index * 2 + 1]);
        index += 1;
    }
    out
}

const fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        b'A'..=b'F' => byte - b'A' + 10,
        _ => panic!("invalid boot bundle identity"),
    }
}

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

/// Two root-image pages reclaimed as temporary mappings for the foundation
/// non-alias probe. Separate from the loader scratch page: the proof needs both
/// frames mapped simultaneously.
static mut FOUNDATION_PAGES: [FreePage; 2] = [const { FreePage([0; GRANULE_SIZE]) }; 2];

/// A second root-image page, whose virtual address becomes the standing window
/// for one device's MMIO register bank (P5.4.2a).
///
/// Separate from `FREE_PAGE` rather than sharing it: the loader and every
/// windowed syscall map and unmap a child frame at the scratch address on each
/// use, so an MMIO frame left mapped there would be replaced by the next
/// transfer. A device register bank must stay mapped for as long as the driver
/// holds the device.
static mut DEVICE_PAGE: FreePage = FreePage([0; GRANULE_SIZE]);

/// Standing windows for each attached block device's register bank (P5.4.2b),
/// one per device since P5.4.3.
///
/// Separate from `DEVICE_PAGE`, which the probe reuses granule by granule as it
/// scans: a live device's registers must stay mapped, and two live devices need
/// two windows.
///
/// Arrays rather than a second set of named statics: the bound is
/// `MAX_BLOCK_DEVICES`, and duplicating three names per device would make the
/// table's size a property of how many statics someone remembered to add.
static mut BLOCK_MMIO_PAGES: [FreePage; MAX_BLOCK_DEVICES] =
    [const { FreePage([0; GRANULE_SIZE]) }; MAX_BLOCK_DEVICES];
/// Each block device's virtqueue rings.
static mut BLOCK_QUEUE_PAGES: [FreePage; MAX_BLOCK_DEVICES] =
    [const { FreePage([0; GRANULE_SIZE]) }; MAX_BLOCK_DEVICES];
/// Each device's request header, data buffer, and status byte.
static mut BLOCK_BUFFER_PAGES: [FreePage; MAX_BLOCK_DEVICES] =
    [const { FreePage([0; GRANULE_SIZE]) }; MAX_BLOCK_DEVICES];

/// Base of qemu-arm-virt's virtio-mmio transport window.
///
/// A *fixture* constant, not a discovery mechanism. The platform declares
/// thirty-two identical transports at `0x0a00_0000 + n * 0x200` and its own
/// device tree is the authority; the driver that eventually owns a device walks
/// the FDT BootInfo extra to find it. This slice proves the mapping and probe
/// mechanism, and for that the window only has to be one the machine declares.
const VIRTIO_MMIO_BASE: usize = 0x0a00_0000;
/// Bytes between consecutive transports.
const VIRTIO_MMIO_STRIDE: usize = 0x200;
/// How many fit in one granule: 4096 / 0x200.
const VIRTIO_MMIO_SLOTS_PER_GRANULE: usize = 8;
/// Granules covering all thirty-two declared transports: 32 / 8.
const VIRTIO_MMIO_GRANULES: usize = 4;
/// SPI number of the first transport's interrupt, from the platform's own
/// device tree (`interrupts = <0x00 0x10 0x01>` on `virtio_mmio@a000000`).
/// Transport `n` uses SPI `0x10 + n`.
const VIRTIO_MMIO_FIRST_SPI: sel4::Word = 0x10;
/// GIC SPIs are numbered from 32 in the kernel's IRQ space.
const GIC_SPI_BASE: sel4::Word = 32;
/// Badge the device notification carries, distinct from the timer's.
const VIRTIO_IRQ_BADGE: sel4::Word = 0x2;

/// The kernel IRQ number of the virtio-mmio transport at `paddr`.
///
/// The device tree is the authority and a driver will read it; this is the same
/// arithmetic it encodes — transport `n` at `VIRTIO_MMIO_BASE + n * 0x200` takes
/// SPI `VIRTIO_MMIO_FIRST_SPI + n`, and seL4 numbers an SPI from 32.
const fn virtio_irq(paddr: usize) -> sel4::Word {
    let index = ((paddr - VIRTIO_MMIO_BASE) / VIRTIO_MMIO_STRIDE) as sel4::Word;
    GIC_SPI_BASE + VIRTIO_MMIO_FIRST_SPI + index
}

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
static mut OBJECT_ALLOCATOR: core::mem::MaybeUninit<ObjectAllocator> =
    core::mem::MaybeUninit::uninit();

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
    let allocator = match ObjectAllocator::new(bootinfo) {
        Ok(value) => unsafe {
            // SAFETY: root startup is single-threaded and initializes this storage exactly once.
            (&raw mut OBJECT_ALLOCATOR).write(core::mem::MaybeUninit::new(value));
            (&raw mut OBJECT_ALLOCATOR)
                .cast::<ObjectAllocator>()
                .as_mut()
                .unwrap()
        },
        Err(error) => fatal!("allocator rejected bootinfo: {error:?}"),
    };
    let initial_slots = allocator.slots_remaining();
    let initial_untypeds = allocator.untyped_count();
    let initial_bytes = allocator.untyped_bytes_remaining();
    if initial_slots == 0 || initial_untypeds == 0 || initial_bytes == 0 {
        fatal!(
            "SLIME_FOUNDATION FAIL allocator slots={initial_slots} untypeds={initial_untypeds} bytes={initial_bytes}"
        )
    }
    sel4::debug_println!(
        "SLIME_ROOT allocator slots={initial_slots} untypeds={initial_untypeds} bytes={initial_bytes}",
    );

    // ---- timer phase ----
    // Proves `TimerScheduler` (see `timer.rs`) is driven by a real seL4 IRQ
    // before any fixture task exists: acquire the one architected-timer PPI
    // seL4 leaves for userspace on this platform (`platform_timer.rs`),
    // schedule a short deadline, wait for the interrupt it raises, then
    // confirm the monotonic counter it reads actually advanced. The wait is
    // bounded by wall-clock ticks read directly from hardware rather than by
    // IRQ delivery, so a broken wiring fails loudly instead of hanging boot.
    let mut timer_adapter = match PhysicalTimerAdapter::acquire(allocator) {
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

    let foundation_first = ptr::addr_of!(FOUNDATION_PAGES) as usize;
    let foundation_second = foundation_first + GRANULE_SIZE;
    for address in [foundation_first, foundation_second] {
        if let Err(error) = ScratchPage::claim(bootinfo, address) {
            fatal!("SLIME_FOUNDATION FAIL scratch unmap address={address:#x}: {error:?}")
        }
    }
    let foundation_before = (
        allocator.objects_allocated(),
        allocator.slots_allocated(),
        allocator.bytes_allocated(),
    );
    let mut foundation_adapter = BufferAdapter::new(allocator);
    if let Err(error) = foundation_adapter.prove_frame_independence(
        sel4::init_thread::slot::VSPACE.cap(),
        foundation_first,
        foundation_second,
    ) {
        fatal!("SLIME_FOUNDATION FAIL frame independence: {error:?}")
    }
    let foundation_after = (
        allocator.objects_allocated(),
        allocator.slots_allocated(),
        allocator.bytes_allocated(),
    );
    if foundation_after.0 != foundation_before.0 + 2
        || foundation_after.1 != foundation_before.1 + 2
        || foundation_after.2 != foundation_before.2 + 2 * GRANULE_SIZE
    {
        fatal!(
            "SLIME_FOUNDATION FAIL accounting before={foundation_before:?} after={foundation_after:?}"
        )
    }
    sel4::debug_println!(
        "SLIME_FOUNDATION frames independent objects_delta=2 slots_delta=2 bytes_delta={} caps_deleted=2",
        2 * GRANULE_SIZE,
    );

    // ---- device phase (P5.4.2a) ----
    //
    // Not conditional on a generation flag: the probe reports what BootInfo and
    // the platform actually declare, and a machine with no device untyped or no
    // attached transport reports exactly that. Storage *policy* stays userspace
    // and stays absent until P5.4.2b; what this establishes is the mechanism —
    // the root can name a device region, map it non-cacheably, and read a
    // register out of it.
    //
    // Every seL4 gate boots this path, so the markers are unconditional and
    // every plane's transcript carries them.
    let mut block_devices = probe_devices(bootinfo, allocator);
    #[cfg(slime_boot_selector)]
    let selected = {
        let device = block_devices
            .get_mut(0)
            .unwrap_or_else(|| fatal!("boot selector has no boot device"));
        match boot_selector::select(device, &BOOT_BUNDLE_IDENTITY) {
            Ok(selected) => selected,
            Err(error) => fatal!("boot selection rejected: {error:?}"),
        }
    };
    #[cfg(slime_boot_selector)]
    let boot_selector::SelectedGeneration {
        generation,
        runtime: mut boot_runtime,
    } = selected;
    #[cfg(not(slime_boot_selector))]
    let generation = match Generation::decode(GENERATION_BYTES) {
        Ok(generation) => generation,
        Err(error) => fatal!("generation rejected: {error:?}"),
    };
    #[cfg(slime_boot_selector)]
    sel4::debug_println!(
        "SLIME_BOOT selected identity={:02x?} number={} pending={} attempts={}",
        boot_runtime.running_identity(),
        generation.number,
        usize::from(boot_runtime.running_pending()),
        boot_runtime.remaining_attempts(),
    );
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
        "SLIME_ROOT generation admitted number={} executables={} instances={} grants={} health={} bootstrap={}",
        generation.number,
        admission.executable_len(),
        admission.instance_len(),
        admission.grants,
        admission.health,
        admission.bootstrap_objects,
    );
    // C8.2: the declared fabric graph, checked against this root's own ceilings
    // before any component launches. `absent` for the generations that declare
    // none; `admitted` is the wiring being observable, which no unit test over
    // a hand-built graph can show.
    //
    // The counts are C8.4's structural arm (P5.4.10). A transcript proves
    // samples moved; only reading the authenticated resource proves they moved
    // along edges the *generation* fixed. `kernel/tests/fabric_stream.rs` says
    // exactly this about itself — "the two things a transcript cannot show" —
    // and reports the shape rather than asserting a number here, because the
    // number is a property of the generation rather than of the root.
    sel4::debug_println!(
        "SLIME_ROOT fabric graph={} schemas={} routes={} participants={} interpositions={}",
        if admission.fabric_graph_admitted {
            "admitted"
        } else {
            "absent"
        },
        admission.fabric_schemas,
        admission.fabric_routes,
        admission.fabric_participants,
        admission.fabric_interpositions,
    );
    sel4::debug_println!(
        "SLIME_ROOT authority manifest={:02x?}",
        generation.authority_manifest_identity()
    );
    sel4::debug_println!(
        "SLIME_ROOT graph admitted executables={} instances={} slimecm={} elf={} unrecognized={}",
        admission.executable_len(),
        admission.instance_len(),
        admission.slime_component_images,
        admission.loadable,
        admission.unrecognized_images,
    );
    // ---- end device phase ----
    let image = match ChildImage::parse(&CHILD_ELF.0) {
        Ok(image) => image,
        Err(error) => fatal!("child image rejected: {error:?}"),
    };
    let service_endpoint = match allocator.allocate_fixed::<sel4::cap_type::Endpoint>() {
        Ok(slot) => slot.cap(),
        Err(error) => fatal!("service endpoint unavailable: {error:?}"),
    };
    // B41: console and debug traffic gets its own endpoint object. A noisy or
    // faulting console client then cannot consume the lifecycle dispatcher's
    // queue, and the two no longer share a fault domain.
    let console_endpoint = match allocator.allocate_fixed::<sel4::cap_type::Endpoint>() {
        Ok(slot) => slot.cap(),
        Err(error) => fatal!("console endpoint unavailable: {error:?}"),
    };

    // A v4 generation launches only explicitly declared root-owned autostart
    // instances. Catalogue-only executables remain inert until a bound exec
    // capability authorizes a userspace spawn.
    #[cfg(not(slime_root_fixture))]
    if admission.loadable > 0 {
        launch_instance_graph(
            &generation,
            &admission,
            allocator,
            &scratch,
            service_endpoint,
            console_endpoint,
            &mut block_devices,
            #[cfg(slime_boot_selector)]
            &mut boot_runtime,
        );
    }
    // ---- end generation graph phase ----

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
        match tasks.reclaim(allocator, fixture.id) {
            Ok(record) => {
                reclaimed_tasks += 1;
                reclaimed_slots += record.slot_count();
                sel4::debug_println!(
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
    sel4::debug_println!(
        "SLIME_ROOT cleanup tasks={reclaimed_tasks} slots={reclaimed_slots} live={}",
        tasks.len()
    );
    sel4::debug_println!(
        "SLIME_ROOT allocator live_slots={} live_objects={} live_bytes={} slot_reuses={} arena_reuses={}",
        allocator.live_slots(),
        allocator.live_objects(),
        allocator.live_bytes(),
        allocator.slots_reused(),
        allocator.arena_reuses(),
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

/// Authority to run an executable, held alongside `RIGHT_EXEC`. Holding an
/// image is not authority to start it: `preflight_spawn_grants` requires both.
const RIGHT_SPAWN: u64 = 1 << 16;

/// Authority to observe a spawned child's termination. The right the handle a
/// spawn returns carries, matching `kernel/src/capability/mod.rs`.
const RIGHT_SUPERVISE: u64 = 1 << 18;

/// Authority to mint a channel pair, held on an `EndpointFactory`.
const RIGHT_ENDPOINT_CREATE: u64 = 1 << 17;

/// Authority to allocate a shared buffer, held on a `SharedBufferFactory`.
/// Independent of the holder's quota by design: the grant authorizes the
/// operation and the budget bounds it (B13).
const RIGHT_BUFFER_CREATE: u64 = 1 << 24;

/// Authority to read sectors, held on a `Block` (P5.4.2c). Numbered as
/// `blockRead` in the generation's own rights table
/// (`scripts/build/build-generation.py`), which is the same numbering
/// `kernel/src/capability/mod.rs` uses.
const RIGHT_BLOCK_READ: u64 = 1 << 10;
/// Authority to write sectors and to flush. One bit for both, matching the
/// oracle: a caller that may change what is on the device may also ask for it
/// to be made durable, and a flush without writes is a no-op.
const RIGHT_BLOCK_WRITE: u64 = 1 << 11;

/// Authority over a `Directory` (M6.3, P5.4.3), numbered as the oracle's
/// `capability::RIGHT_DIRECTORY_*` and as `directoryRead` and friends in the
/// generation's rights table.
///
/// Four independent bits rather than a read/write pair: listing a directory and
/// resolving one name in it are different authorities, and derivation is a
/// third — a component may be allowed to *use* a scope without being allowed to
/// hand out narrower views of it.
const RIGHT_DIRECTORY_READ: u64 = 1 << 19;
const RIGHT_DIRECTORY_WRITE: u64 = 1 << 20;
const RIGHT_DIRECTORY_LIST: u64 = 1 << 21;
const RIGHT_DIRECTORY_DERIVE: u64 = 1 << 22;
/// Authority to read one decoded key event, held on an `Input` (M6.4).
const RIGHT_INPUT_READ: u64 = 1 << 23;

/// Every right a directory capability may carry, for bounding a derive request.
const RIGHTS_DIRECTORY_ALL: u64 =
    RIGHT_DIRECTORY_READ | RIGHT_DIRECTORY_WRITE | RIGHT_DIRECTORY_LIST | RIGHT_DIRECTORY_DERIVE;

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

/// Report what device authority BootInfo gives this root, and probe the
/// platform's virtio-mmio transports (P5.4.2a).
///
/// Three markers, and each is a distinct claim:
///
/// * `devices untypeds=` — BootInfo named this many device regions. Zero means
///   the platform declares no device memory, which is a fact about the machine
///   rather than a failure.
/// * `device mapped=` — one granule was retyped out of a device untyped and
///   mapped non-cacheably into the root's own VSpace. This is the mechanism
///   P5.4.2's block device needs and the root did not have.
/// * `virtio transport=` — a register read out of that mapping identified a
///   present transport. Absent means the slot exists but nothing is attached,
///   which is what all thirty-two report when QEMU is given no `-drive`.
///
/// Failure is reported and returned from, never fatal: no plane depends on a
/// device yet, and a root that refused to boot without one would break twelve
/// gates to prove nothing.
fn probe_devices(bootinfo: &sel4::BootInfo, allocator: &mut ObjectAllocator) -> BlockDevices {
    sel4::debug_println!(
        "SLIME_ROOT devices untypeds={}",
        allocator.device_untyped_count(),
    );
    let mut devices = BlockDevices::new();
    if allocator.device_untyped_count() == 0 {
        return devices;
    }
    // SAFETY: the root task is single-threaded and this is the only reference
    // taken to `DEVICE_PAGE`. Its address is granule-aligned by the type's
    // `repr(align(4096))`, and it is claimed exactly once.
    let base = ptr::addr_of!(DEVICE_PAGE) as usize;
    if let Err(error) = ScratchPage::claim(bootinfo, base) {
        sel4::debug_println!("SLIME_ROOT device page unavailable: {error:?}");
        return devices;
    }
    // Every transport the platform declares, a granule at a time. One claimed
    // root-image page is enough because the mapping is released between
    // granules — the frame capabilities stay, only the virtual window is
    // reused.
    //
    // Scanning rather than reading one slot: QEMU declares thirty-two identical
    // transports and attaches a device to the *highest* free one, so the
    // occupied slot is a function of how many devices the command line names.
    // A driver will read the FDT to enumerate them; the point here is that the
    // answer comes from register reads rather than from a guess.
    let mut found = 0;
    let mut mapped = 0;
    // Every attached transport, not merely the last one (P5.4.3). M6.7 crosses
    // a persistence boundary, so it needs a source device and a receiver
    // device at once — and a root that kept only the highest-numbered
    // transport could express the milestone's central claim, that an ungranted
    // device is untouched, only by having no second device to touch.
    let mut attached: [Option<device::VirtioMmio>; MAX_BLOCK_DEVICES] = [None; MAX_BLOCK_DEVICES];
    let mut regions: [Option<device::DeviceRegion>; MAX_BLOCK_DEVICES] =
        [const { None }; MAX_BLOCK_DEVICES];
    let mut attached_count = 0;
    // Granules already remapped to a driver's standing window, so a second
    // transport in the same page borrows rather than remaps (B29).
    let mut standing: [Option<(usize, device::MappedGranule)>; MAX_BLOCK_DEVICES] =
        [None; MAX_BLOCK_DEVICES];
    for granule in 0..VIRTIO_MMIO_GRANULES {
        let paddr = VIRTIO_MMIO_BASE + granule * GRANULE_SIZE;
        let region = match device::DeviceRegion::map(
            allocator,
            sel4::init_thread::slot::VSPACE.cap(),
            base,
            paddr,
        ) {
            Ok(region) => region,
            Err(error) => {
                sel4::debug_println!("SLIME_ROOT device map failed paddr={paddr:#x} {error:?}");
                return devices;
            }
        };
        mapped += 1;
        for slot in 0..VIRTIO_MMIO_SLOTS_PER_GRANULE {
            let Some(transport) = device::VirtioMmio::probe(&region, slot * VIRTIO_MMIO_STRIDE)
            else {
                continue;
            };
            found += 1;
            sel4::debug_println!(
                "SLIME_ROOT virtio transport={:#x} version={} device={} vendor={:#x}",
                transport.paddr,
                transport.version,
                transport.device_id,
                transport.vendor_id,
            );
            if attached_count < MAX_BLOCK_DEVICES {
                attached[attached_count] = Some(transport);
                attached_count += 1;
            } else {
                sel4::debug_println!(
                    "SLIME_ROOT virtio transport ignored paddr={:#x} reason=table-full",
                    transport.paddr,
                );
            }
        }
        // Keep the granule holding the attached transport rather than
        // releasing it: seL4's retype is monotonic, so a device untyped's page
        // can be reached exactly once per boot. Unmapping frees the virtual
        // window; the frame capability stays in `region` and is handed to the
        // driver below, which re-maps it at its own standing address.
        // Keep the granule if *any* attached transport lives in it. Several
        // can: the stride is 0x200 and a granule is 0x1000, so eight transports
        // share one page — which is why the region is looked up by address
        // below rather than owned by one transport.
        let holds_attached = attached[..attached_count]
            .iter()
            .any(|entry| entry.is_some_and(|found| found.paddr & !(GRANULE_SIZE - 1) == paddr));
        if holds_attached {
            if let Some(slot) = regions.iter_mut().find(|slot| slot.is_none()) {
                *slot = Some(region);
            }
            continue;
        }
        if let Err(error) = region.unmap() {
            sel4::debug_println!("SLIME_ROOT device unmap failed paddr={paddr:#x} {error:?}");
            return devices;
        }
    }
    // Bind the interrupt of whichever transport is attached (P5.4.2b).
    //
    // Only for a device that exists: `irq_control_get_trigger` succeeds for any
    // number the platform declares, so acquiring an unattached transport's line
    // would report a binding that can never fire and prove nothing.
    //
    // Level-triggered, because virtio-mmio holds its line asserted until the
    // driver writes `InterruptACK`. Nothing here acknowledges: there is no
    // driver yet to clear the device condition first, and acknowledging before
    // that is exactly the ordering mistake that storms. What this establishes
    // is that the root can *acquire and bind* a device IRQ; servicing one is
    // the transport's.
    // Highest physical address first, which is QEMU command-line order.
    //
    // QEMU fills virtio-mmio slots downward from the highest free one, so the
    // *first* `-device` on the command line lands at the highest address. A
    // generation naming "device 0" therefore means the first disk the operator
    // attached, which is the only ordering an operator can predict.
    attached[..attached_count].sort_unstable_by_key(|entry| {
        core::cmp::Reverse(entry.map_or(0, |transport| transport.paddr))
    });
    for entry in attached.iter().take(attached_count) {
        let Some(transport) = *entry else {
            continue;
        };
        #[cfg(not(slime_boot_selector))]
        {
            let irq = virtio_irq(transport.paddr);
            match device::DeviceIrq::acquire(allocator, irq, VIRTIO_IRQ_BADGE, true) {
                Ok(binding) => sel4::debug_println!(
                    "SLIME_ROOT virtio irq bound transport={:#x} irq={} badge={:#x}",
                    transport.paddr,
                    binding.irq(),
                    VIRTIO_IRQ_BADGE,
                ),
                Err(error) => {
                    sel4::debug_println!("SLIME_ROOT virtio irq unavailable irq={irq} {error:?}");
                }
            }
        }
        #[cfg(slime_boot_selector)]
        sel4::debug_println!(
            "SLIME_ROOT virtio irq polled transport={:#x}",
            transport.paddr,
        );
        let granule = transport.paddr & !(GRANULE_SIZE - 1);
        // A granule another driver already stands in? Then borrow that mapping
        // at this transport's own offset (B29). QEMU packs eight transports
        // into one page, so two attached disks routinely share one, and the
        // frame can be mapped exactly once.
        let block = if let Some(shared) = standing
            .iter()
            .find_map(|entry| entry.filter(|(paddr, _)| *paddr == granule).map(|(_, g)| g))
        {
            bring_up_shared_block(allocator, bootinfo, transport, shared, devices.len())
        } else {
            let region = regions.iter_mut().find_map(|slot| {
                let holds = slot
                    .as_ref()
                    .is_some_and(|region| region.paddr() == granule);
                if holds { slot.take() } else { None }
            });
            let Some(region) = region else {
                sel4::debug_println!(
                    "SLIME_ROOT virtio transport skipped paddr={:#x} reason=no-region",
                    transport.paddr,
                );
                continue;
            };
            match bring_up_block(allocator, bootinfo, transport, region, devices.len()) {
                Some((block, borrowed)) => {
                    if let Some(slot) = standing.iter_mut().find(|slot| slot.is_none()) {
                        *slot = Some((granule, borrowed));
                    }
                    Some(block)
                }
                None => None,
            }
        };
        if let Some(block) = block {
            devices.push(block);
        }
    }
    sel4::debug_println!(
        "SLIME_ROOT virtio probed granules={mapped} slots={} found={found}",
        mapped * VIRTIO_MMIO_SLOTS_PER_GRANULE,
    );
    devices
}

/// Block devices this cutover brings up, in stable physical-address order.
///
/// Two, because M6.7 transfers a generation from a source device to a receiver
/// and needs both at once. The bound is a table size rather than a policy: a
/// generation grants authority over a device by index, and an index the boot
/// did not fill is authority the root cannot back.
pub const MAX_BLOCK_DEVICES: usize = 2;

/// The brought-up devices.
pub struct BlockDevices {
    devices: [Option<virtio_blk::VirtioBlock>; MAX_BLOCK_DEVICES],
    len: usize,
}

impl Default for BlockDevices {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockDevices {
    pub const fn new() -> Self {
        Self {
            devices: [const { None }; MAX_BLOCK_DEVICES],
            len: 0,
        }
    }

    fn push(&mut self, device: virtio_blk::VirtioBlock) {
        if self.len < MAX_BLOCK_DEVICES {
            self.devices[self.len] = Some(device);
            self.len += 1;
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get_mut(&mut self, index: usize) -> Option<&mut virtio_blk::VirtioBlock> {
        self.devices.get_mut(index)?.as_mut()
    }
}

/// Bring up a second transport in a granule another driver already mapped
/// (B29, P5.4.3).
///
/// Everything `bring_up_block` does except the mapping: the frame is already
/// standing at a driver's window, so this allocates only the DMA pages and
/// hands the driver a borrow at its own offset.
fn bring_up_shared_block(
    allocator: &mut ObjectAllocator,
    bootinfo: &sel4::BootInfo,
    transport: device::VirtioMmio,
    shared: device::MappedGranule,
    index: usize,
) -> Option<virtio_blk::VirtioBlock> {
    let granule = transport.paddr & !(GRANULE_SIZE - 1);
    let offset = transport.paddr - granule;
    if index >= MAX_BLOCK_DEVICES {
        return None;
    }
    let queue_base = ptr::addr_of!(BLOCK_QUEUE_PAGES) as usize + index * GRANULE_SIZE;
    let buffer_base = ptr::addr_of!(BLOCK_BUFFER_PAGES) as usize + index * GRANULE_SIZE;
    for address in [queue_base, buffer_base] {
        if let Err(error) = ScratchPage::claim(bootinfo, address) {
            sel4::debug_println!("SLIME_ROOT block page unavailable: {error:?}");
            return None;
        }
    }
    let queue = match device::DmaPage::allocate(
        allocator,
        sel4::init_thread::slot::VSPACE.cap(),
        queue_base,
    ) {
        Ok(page) => page,
        Err(error) => {
            sel4::debug_println!("SLIME_ROOT block queue unavailable: {error:?}");
            return None;
        }
    };
    let buffer = match device::DmaPage::allocate(
        allocator,
        sel4::init_thread::slot::VSPACE.cap(),
        buffer_base,
    ) {
        Ok(page) => page,
        Err(error) => {
            sel4::debug_println!("SLIME_ROOT block buffer unavailable: {error:?}");
            return None;
        }
    };
    sel4::debug_println!(
        "SLIME_ROOT block dma queue={:#x} buffer={:#x}",
        queue.physical_address(),
        buffer.physical_address(),
    );
    let mut block = match virtio_blk::VirtioBlock::new(shared, offset, queue, buffer) {
        Ok(block) => block,
        Err(error) => {
            sel4::debug_println!("SLIME_ROOT block bring-up failed {error:?}");
            return None;
        }
    };
    sel4::debug_println!(
        "SLIME_ROOT block ready transport={:#x} sectors={}",
        transport.paddr,
        block.capacity_sectors(),
    );
    #[cfg(not(slime_boot_selector))]
    {
        let mut sector = [0u8; virtio_blk::SECTOR_BYTES];
        match block.read_sector(0, &mut sector) {
            Ok(()) => sel4::debug_println!(
                "SLIME_ROOT block read lba=0 bytes={} head={:02x}{:02x}{:02x}{:02x}",
                sector.len(),
                sector[0],
                sector[1],
                sector[2],
                sector[3],
            ),
            Err(error) => sel4::debug_println!("SLIME_ROOT block read failed lba=0 {error:?}"),
        }
    }
    Some(block)
}

/// Bring up the attached virtio block device and read one sector (P5.4.2b).
///
/// The transport's registers are re-mapped here rather than kept from the
/// probe: the probe unmaps each granule as it scans, so one claimed page can
/// cover all thirty-two slots. This maps the granule the attached transport
/// lives in and keeps it, which is what a live device needs.
///
/// Two DMA pages, both ordinary RAM the allocator can name physically: one for
/// the virtqueue rings, one for the request header, data buffer, and status
/// byte.
///
/// Reading sector 0 is the proof. A driver that negotiated the handshake but
/// never moved a byte would report a capacity and nothing else; a completed
/// read means descriptors the device followed, a buffer it wrote through DMA,
/// and a status byte it set.
fn bring_up_block(
    allocator: &mut ObjectAllocator,
    bootinfo: &sel4::BootInfo,
    transport: device::VirtioMmio,
    region: device::DeviceRegion,
    index: usize,
) -> Option<(virtio_blk::VirtioBlock, device::MappedGranule)> {
    let granule = transport.paddr & !(GRANULE_SIZE - 1);
    let offset = transport.paddr - granule;
    if index >= MAX_BLOCK_DEVICES {
        return None;
    }
    // Address arithmetic on the array base rather than indexing the static:
    // indexing reads it, and a mutable static may not be read outside `unsafe`.
    // Each element is exactly one granule, so the offset is exact.
    let base = ptr::addr_of!(BLOCK_MMIO_PAGES) as usize + index * GRANULE_SIZE;
    let queue_base = ptr::addr_of!(BLOCK_QUEUE_PAGES) as usize + index * GRANULE_SIZE;
    let buffer_base = ptr::addr_of!(BLOCK_BUFFER_PAGES) as usize + index * GRANULE_SIZE;
    for address in [base, queue_base, buffer_base] {
        if let Err(error) = ScratchPage::claim(bootinfo, address) {
            sel4::debug_println!("SLIME_ROOT block page unavailable: {error:?}");
            return None;
        }
    }
    // The frame the probe already retyped, moved to its own standing address so
    // the scan's shared window stays free.
    let region = match region.remap(sel4::init_thread::slot::VSPACE.cap(), base) {
        Ok(region) => region,
        Err(error) => {
            sel4::debug_println!("SLIME_ROOT block map failed paddr={granule:#x} {error:?}");
            return None;
        }
    };
    let queue = match device::DmaPage::allocate(
        allocator,
        sel4::init_thread::slot::VSPACE.cap(),
        queue_base,
    ) {
        Ok(page) => page,
        Err(error) => {
            sel4::debug_println!("SLIME_ROOT block queue unavailable: {error:?}");
            return None;
        }
    };
    let buffer = match device::DmaPage::allocate(
        allocator,
        sel4::init_thread::slot::VSPACE.cap(),
        buffer_base,
    ) {
        Ok(page) => page,
        Err(error) => {
            sel4::debug_println!("SLIME_ROOT block buffer unavailable: {error:?}");
            return None;
        }
    };
    sel4::debug_println!(
        "SLIME_ROOT block dma queue={:#x} buffer={:#x}",
        queue.physical_address(),
        buffer.physical_address(),
    );
    let borrowed = region.granule();
    let mut block = match virtio_blk::VirtioBlock::new(borrowed, offset, queue, buffer) {
        Ok(block) => block,
        Err(error) => {
            sel4::debug_println!("SLIME_ROOT block bring-up failed {error:?}");
            return None;
        }
    };
    sel4::debug_println!(
        "SLIME_ROOT block ready transport={:#x} sectors={}",
        transport.paddr,
        block.capacity_sectors(),
    );
    #[cfg(not(slime_boot_selector))]
    {
        let mut sector = [0u8; virtio_blk::SECTOR_BYTES];
        match block.read_sector(0, &mut sector) {
            Ok(()) => sel4::debug_println!(
                "SLIME_ROOT block read lba=0 bytes={} head={:02x}{:02x}{:02x}{:02x}",
                sector.len(),
                sector[0],
                sector[1],
                sector[2],
                sector[3],
            ),
            Err(error) => sel4::debug_println!("SLIME_ROOT block read failed lba=0 {error:?}"),
        }
    }
    // Bring-up reads. It does not write.
    //
    // It used to: a write/flush/read-back round trip on sector 1 proved the
    // other DMA direction at boot. Sector 1 is the GPT primary header, so on
    // any partitioned disk the root silently destroyed the partition table
    // before userspace ran — the store plane found it as a `bad-magic` primary
    // recovering from the backup on a *freshly built* fixture.
    //
    // The round trip was not worth a device-wide write from boot code that has
    // no idea what the disk holds. `sel4_storage_check` proves both directions
    // and a flush from userspace, on a sector the fixture designates, through a
    // capability — which is where a write belongs.
    //
    // The borrowed handle is returned beside the driver so the probe can give
    // it to another transport in the same granule (B29). `region` falls out of
    // scope here and releases nothing: `DeviceRegion` has no `Drop`, and a
    // bound device stays bound for the boot.
    Some((block, borrowed))
}

/// Answer one `BlockTransact` (P5.4.2c).
///
/// Three checks before a sector moves, in the order the oracle's
/// `sys_block_transact` makes them:
///
/// 1. the caller's slot must resolve to a `Block` capability — holding a slot
///    number is not authority;
/// 2. the request must decode as a `WireBlockRequest` with the right magic and
///    version, so a malformed frame is refused rather than interpreted;
/// 3. the operation must be covered by the capability's own rights —
///    `blockRead` for `OP_READ`, `blockWrite` for `OP_WRITE` and `OP_FLUSH`.
///
/// The payload travels through the caller's transfer window, like every other
/// windowed operation. One sector per request: `sector_count` above one is
/// refused rather than partially served, because a partial completion has no
/// The namespace root the boot starts from (M6.3, P5.4.3).
///
/// The identity of the directory snapshot `scripts/build/build-directory-fixture.py`
/// commits to the object store, hardcoded exactly as the oracle's
/// `bootstrap::directory_fixture_root` hardcodes it. The root task cannot
/// compute it: resolving a snapshot means reading the store, which is
/// userspace's. What it can do is start the namespace at a root a component
/// will recognise.
///
/// Zero on every plane whose fixture has no directory tree, which is every
/// plane but the filesystem one — and a zero root is what "nothing committed
/// yet" means, so the directory plane's empty-namespace arm still holds.
const DIRECTORY_FIXTURE_ROOT: [u8; 32] = [
    0xe8, 0xcd, 0xd1, 0x45, 0x6f, 0xe5, 0x4e, 0x59, 0xe3, 0xb6, 0x1a, 0x65, 0x5a, 0x2f, 0xbb, 0xfa,
    0xf1, 0x6d, 0x89, 0xa8, 0x77, 0x0a, 0xa1, 0x08, 0x05, 0x51, 0xbd, 0x84, 0xf6, 0x6b, 0x0f, 0xf2,
];

/// The key script a generation runs (M6.4, P5.4.3).
///
/// Selected by generation number, exactly as the oracle's `bootstrap` selects
/// its own. A generation with no entry here gets an empty script, so
/// `InputRead` on every other plane answers `WouldBlock` — which is what "no
/// key has been pressed" means, and is why holding the capability on a plane
/// with no session is harmless.
const fn input_script(generation: u64) -> &'static [u8] {
    match generation {
        // The dango plane: the oracle's generation-7 session, byte for byte, so
        // the same component produces the same transcript.
        30 => b"$(sysinfo)\n(with-env {MODE=ci} (with-cwd docs (with-stdin data $(echo ok))))\n$(inject)\n$(echo a b c)\n\x1b",
        // The input plane's own script: two characters, a space, a character,
        // and a newline — enough to prove ordering, the character encoding, the
        // named-key encoding, and exhaustion.
        31 => b"ab c\n",
        // The powerbox plane: the oracle's generation-9 session — a newline to
        // confirm the selection, then escape.
        32 => b"\n\x1b",
        _ => b"",
    }
}

/// The scripted key source (M6.4, P5.4.3).
///
/// A byte string and a cursor, exactly as the oracle's
/// `drivers::input::ScriptInput` is. There is no keyboard on the pinned QEMU
/// profile and a gate needs a deterministic session, so the "device" is a
/// script the generation selects — which is honest about what is being proved:
/// the *authority* path and the event encoding, not a PS/2 decoder.
pub struct ScriptedInput {
    bytes: &'static [u8],
    /// Per-task cursors, because the script is a *session* rather than a shared
    /// queue.
    ///
    /// The root launches every declared component, so two copies of a console
    /// component run and both read input. One cursor would let the
    /// root-launched copy drain the script before the spawned one asked, and
    /// the spawned session would park on an exhausted source — which is exactly
    /// what happened, and read as a hung component rather than a shared cursor.
    cursors: [usize; MAX_TASKS],
}

impl ScriptedInput {
    const fn new(bytes: &'static [u8]) -> Self {
        Self {
            bytes,
            cursors: [0; MAX_TASKS],
        }
    }

    /// The next event, or `None` when the script is spent. A spent script is
    /// `WouldBlock` rather than an error: no key has been pressed *yet* is the
    /// same answer a real keyboard gives.
    /// The next event for `task`.
    ///
    /// A spent script yields `Escape` forever rather than `None`. That is not a
    /// convenience: `dango.rs` loops on `WouldBlock` with a `wait` that this
    /// source always satisfies, so a reader whose script ran out would spin
    /// until the graph's iteration budget died — and it *would* run out, because
    /// the root launches an unconfigured copy of every declared component and
    /// that copy reads its own cursor to the end.
    ///
    /// Escape is the session's own quit key, so an exhausted script ends the
    /// reader exactly as the scripted `\x1b` ends the configured one.
    fn next_event(&mut self, task: TaskId) -> Option<u64> {
        let cursor = self.cursors.get_mut(task.0 as usize)?;
        let byte = self.bytes.get(*cursor).copied();
        match byte {
            Some(byte) => {
                *cursor += 1;
                Some(encode_key(byte))
            }
            None => Some(encode_key(0x1b)),
        }
    }
}

/// Encode one scripted byte as the runtime's key event, matching the oracle's
/// `syscall::encode_key_event` numbering so `slime_rt::input_read` decodes it
/// unchanged.
const fn encode_key(byte: u8) -> u64 {
    // The numbering is `syscall::decode` in `components/runtime`, read from the
    // decoder rather than guessed: 1..=13 are named keys, and a printable
    // character is `0x100 | ch`. Getting this wrong produced a session where
    // every keystroke arrived as a space, which is a decoder disagreement that
    // looks like a broken keyboard.
    let code: u64 = match byte {
        0x1b => 1,
        0x08 => 2,
        b'\t' => 3,
        b'\n' => 4,
        b' ' => 9,
        printable => 0x100 | printable as u64,
    };
    // Bit 32 is `pressed`. A script byte is a keypress, and without this every
    // event decoded as a *release* — which `dango.rs` discards, so the session
    // consumed its whole script and typed nothing.
    code | (1 << 32)
}

/// Answer `InputRead`: one decoded key event, if the caller may read them.
fn serve_input_read(
    graph: &GraphTables,
    input: &mut ScriptedInput,
    id: TaskId,
    words: &[sel4::Word],
) -> Response {
    let Some(table) = graph.get(id) else {
        return Response::error(IpcError::BadCapability);
    };
    let Ok(capability) = table.resolve(words[0] as u32, RIGHT_INPUT_READ) else {
        return Response::error(IpcError::BadCapability);
    };
    if !matches!(capability.resource, graph::Resource::Input) {
        return Response::error(IpcError::BadCapability);
    }
    match input.next_event(id) {
        Some(event) => Response::success(0, event),
        // Only a task id past the cursor table, which cannot happen for a task
        // the dispatcher is serving.
        None => Response::error(IpcError::WouldBlock),
    }
}

/// Which resource a declared grant names, and the rights mask that bounds it.
///
/// # Slot order comes from the grant's name
///
/// `build-generation.py` sorts grants by `(name, source, target)` before
/// encoding, so the manifest's *declaration* order is not preserved — the
/// encoded order is alphabetical by grant name, and that is what both placement
/// loops walk.
///
/// So a component holding several kinds fixes its slot layout by naming its
/// grants in the order it reads them. `sel4-powerbox.zti` names
/// `powerbox-a-root` and `powerbox-b-input` for exactly that reason:
/// `powerbox-chooser.rs` reads a directory at 1 and input at 2.
///
/// That is a sharp edge, and it is recorded as a follow-up rather than
/// defended: a component's expected layout should be declared data checked at
/// build time, the way the bootstrap component's boot layout already is.
///
/// One grant names one kind: a manifest that mixed `inputRead` with
/// `blockRead` in a single grant would be declaring two capabilities as one,
/// and the first match wins rather than silently installing both.
///
/// Executables are absent because they are placed by their own loop, at
/// `1..=n`, before anything here.
const fn declared_resource(rights: u64) -> Option<(u64, graph::Resource)> {
    if rights & RIGHTS_DIRECTORY_ALL != 0 {
        return Some((
            RIGHTS_DIRECTORY_ALL | RIGHT_TRANSFER,
            graph::Resource::Directory {
                namespace: 0,
                scope: graph::ScopeTable::ROOT,
            },
        ));
    }
    if rights & RIGHT_INPUT_READ != 0 {
        return Some((RIGHT_INPUT_READ, graph::Resource::Input));
    }
    if rights & RIGHT_ENDPOINT_CREATE != 0 {
        return Some((RIGHT_ENDPOINT_CREATE, graph::Resource::EndpointFactory));
    }
    if rights & RIGHT_BUFFER_CREATE != 0 {
        return Some((RIGHT_BUFFER_CREATE, graph::Resource::SharedBufferFactory));
    }
    if rights & (RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE) != 0 {
        // Device 0; a component declared several gets them renumbered by the
        // caller, which is the only place that knows how many it has already
        // placed.
        return Some((
            RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE,
            graph::Resource::Block { device: 0 },
        ));
    }
    None
}

pub struct Namespaces {
    roots: [[u8; 32]; MAX_NAMESPACES],
}

/// Namespaces this cutover supports. One, and the resource carries an index so
/// raising it later is a table change rather than a representation change.
const MAX_NAMESPACES: usize = 1;

impl Namespaces {
    const fn new() -> Self {
        Self {
            roots: [DIRECTORY_FIXTURE_ROOT; MAX_NAMESPACES],
        }
    }

    fn root(&self, namespace: u32) -> Option<[u8; 32]> {
        self.roots.get(namespace as usize).copied()
    }

    /// Replace a namespace root, but only if it still holds `expected`.
    ///
    /// The compare is the point. A writer builds a new tree from the root it
    /// read; if another writer committed in between, that tree is built on a
    /// stale parent and installing it would silently discard the other's work.
    /// A failed compare is `false`, not an error: the caller re-reads and
    /// retries, which is the ordinary path rather than a fault.
    fn commit(&mut self, namespace: u32, expected: [u8; 32], new: [u8; 32]) -> Option<bool> {
        let slot = self.roots.get_mut(namespace as usize)?;
        if *slot != expected {
            return Some(false);
        }
        *slot = new;
        Some(true)
    }
}

/// Answer `DirectoryInspect`: the namespace root this capability sees, and the
/// scope it sees it through.
///
/// `words[0]` is the capability slot and `words[1]` the rights the caller
/// claims to need — checked as a subset of what the capability carries, so a
/// component asking for `directoryWrite` on a read-only view is refused here
/// rather than discovering it at commit time.
///
/// The reply is the 32-byte root followed by the scope path, written through
/// the caller's transfer window because a scope can exceed a message.
fn serve_directory_inspect(
    graph: &GraphTables,
    namespaces: &Namespaces,
    scopes: &graph::ScopeTable,
    window: Option<transfer_window::Window>,
    scratch: &ScratchPage,
    id: TaskId,
    words: &[sel4::Word],
) -> Response {
    let Some(table) = graph.get(id) else {
        return Response::error(IpcError::BadCapability);
    };
    // `words[0]` packs the slot and the required rights, as `wire::slot_pair`
    // encodes them: slot low, rights high. One word because the operation's
    // argument list would otherwise exceed the fast registers.
    let slot = words[0] as u32;
    let required = words[0] >> 32;
    // A zero request is not "no requirement": it is a caller that did not say
    // what it needs, which the oracle refuses too.
    if required == 0 || required & !RIGHTS_DIRECTORY_ALL != 0 {
        return Response::error(IpcError::InvalidOperation);
    }
    let Ok(capability) = table.resolve(slot, required) else {
        return Response::error(IpcError::BadCapability);
    };
    let graph::Resource::Directory { namespace, scope } = capability.resource else {
        return Response::error(IpcError::BadCapability);
    };
    let Some(root) = namespaces.root(namespace) else {
        return Response::error(IpcError::BadCapability);
    };
    let path = scopes.path(scope);
    let mut reply = [0u8; 32 + graph::MAX_DIRECTORY_PATH];
    reply[..32].copy_from_slice(&root);
    reply[32..32 + path.len()].copy_from_slice(path);
    let descriptor =
        match transfer_window::write_staged_region(window, &reply[..32 + path.len()], scratch) {
            Ok(descriptor) => descriptor,
            Err(error) => return Response::error(error),
        };
    sel4::debug_println!(
        "SLIME_GRAPH directory inspected task={} slot={slot} namespace={namespace} scope={}",
        id.0,
        DisplayPath(path),
    );
    Response::success(path.len() as i64, descriptor)
}

/// Answer `DirectoryDerive`: a narrower view of the same namespace.
///
/// Two narrowings at once, and both are one-directional:
///
/// * the **scope** may only lengthen — the request's path is appended to the
///   source's, so a holder of `docs` derives `docs/notes` and can express no
///   path that escapes it. There is no syntax for `..`, because
///   `valid_directory_path` rejects the segment outright;
/// * the **rights** must be a subset of the source's, and `RIGHT_TRANSFER` is
///   checked separately so a view that may not be handed on cannot derive one
///   that may.
///
/// Non-consuming: the source capability stays exactly as it was, matching every
/// other derive-copy in this crate since B25.
fn serve_directory_derive(
    graph: &mut GraphTables,
    scopes: &mut graph::ScopeTable,
    window: Option<transfer_window::Window>,
    scratch: &ScratchPage,
    id: TaskId,
    words: &[sel4::Word],
) -> Response {
    // Same packing as inspect. The path's length is not a word: it comes from
    // the staged descriptor, so a caller cannot claim one length and stage
    // another.
    let slot = words[0] as u32;
    let rights = words[0] >> 32;
    if rights == 0 || rights & !(RIGHTS_DIRECTORY_ALL | RIGHT_TRANSFER) != 0 {
        return Response::error(IpcError::InvalidOperation);
    }
    let Some(transfer) = words.get(1).copied() else {
        return Response::error(IpcError::InvalidLength);
    };
    let frame = match transfer_window::read_staged_array(window, transfer, words, scratch) {
        Ok(frame) => frame,
        Err(error) => return Response::error(error),
    };
    let staged = frame.bytes();
    if staged.len() > graph::MAX_DIRECTORY_PATH {
        return Response::error(IpcError::InvalidLength);
    }
    let path_len = staged.len();
    let mut path = [0u8; graph::MAX_DIRECTORY_PATH];
    path[..path_len].copy_from_slice(staged);
    let Some(table) = graph.get_mut(id) else {
        return Response::error(IpcError::BadCapability);
    };
    // Resolved on `RIGHT_DIRECTORY_DERIVE` alone: holding a view is not
    // authority to hand out narrower ones.
    let Ok(source) = table.resolve(slot, RIGHT_DIRECTORY_DERIVE) else {
        return Response::error(IpcError::BadCapability);
    };
    let graph::Resource::Directory { namespace, scope } = source.resource else {
        return Response::error(IpcError::BadCapability);
    };
    // No widening, and `RIGHT_TRANSFER` is not implied by the rest.
    if rights & !source.rights != 0 {
        return Response::error(IpcError::BadCapability);
    }
    let Some(derived) = scopes.derive(scope, &path[..path_len]) else {
        return Response::error(IpcError::InvalidOperation);
    };
    let capability = graph::Capability {
        resource: graph::Resource::Directory {
            namespace,
            scope: derived,
        },
        rights,
    };
    let Some(free) = table.free_slot_from(1) else {
        return Response::error(IpcError::DestinationSlotsExhausted);
    };
    if table.install(free, capability).is_err() {
        return Response::error(IpcError::DestinationSlotsExhausted);
    }
    sel4::debug_println!(
        "SLIME_GRAPH directory derived task={} from={slot} to={free} namespace={namespace} scope={} rights={rights:#x}",
        id.0,
        DisplayPath(scopes.path(derived)),
    );
    Response::success(free as i64, 0)
}

/// Answer `DirectoryCommit`: replace the namespace root, atomically.
///
/// Two gates the oracle also applies, and each rules out a different attack:
///
/// * `RIGHT_DIRECTORY_WRITE`, so a reader cannot install anything;
/// * an **unscoped** capability, so a holder of `docs` cannot replace the
///   namespace-wide root with its own subtree — which would promote a subtree
///   snapshot to the whole filesystem and delete everything beside it.
///
/// The staged payload is two 32-byte identities: the root the caller believes
/// is live, and the one it built. A mismatch answers `WouldBlock`, which is the
/// retry signal rather than a failure.
fn serve_directory_commit(
    graph: &GraphTables,
    namespaces: &mut Namespaces,
    scopes: &graph::ScopeTable,
    window: Option<transfer_window::Window>,
    scratch: &ScratchPage,
    id: TaskId,
    words: &[sel4::Word],
) -> Response {
    let slot = words[0] as u32;
    let Some(transfer) = words.get(1).copied() else {
        return Response::error(IpcError::InvalidLength);
    };
    let frame = match transfer_window::read_staged_array(window, transfer, words, scratch) {
        Ok(frame) => frame,
        Err(error) => return Response::error(error),
    };
    let staged = frame.bytes();
    if staged.len() != 64 {
        return Response::error(IpcError::InvalidLength);
    }
    let mut expected = [0u8; 32];
    let mut new = [0u8; 32];
    expected.copy_from_slice(&staged[..32]);
    new.copy_from_slice(&staged[32..64]);
    let Some(table) = graph.get(id) else {
        return Response::error(IpcError::BadCapability);
    };
    let Ok(capability) = table.resolve(slot, RIGHT_DIRECTORY_WRITE) else {
        return Response::error(IpcError::BadCapability);
    };
    let graph::Resource::Directory { namespace, scope } = capability.resource else {
        return Response::error(IpcError::BadCapability);
    };
    if !scopes.is_root(scope) {
        sel4::debug_println!(
            "SLIME_GRAPH directory commit refused task={} slot={slot} namespace={namespace} reason=scoped scope={}",
            id.0,
            DisplayPath(scopes.path(scope)),
        );
        return Response::error(IpcError::BadCapability);
    }
    match namespaces.commit(namespace, expected, new) {
        Some(true) => {
            sel4::debug_println!(
                "SLIME_GRAPH directory committed task={} namespace={namespace} root={:02x}{:02x}{:02x}{:02x}",
                id.0,
                new[0],
                new[1],
                new[2],
                new[3],
            );
            Response::success(0, 0)
        }
        // The root moved under the caller. Not an error: re-read and retry.
        Some(false) => {
            sel4::debug_println!(
                "SLIME_GRAPH directory commit stale task={} namespace={namespace}",
                id.0,
            );
            Response::error(IpcError::WouldBlock)
        }
        None => Response::error(IpcError::BadCapability),
    }
}

/// A scope path in a marker, printed as text when it is text and as `-` when it
/// is empty, so an unscoped view is visibly distinct from a missing field.
struct DisplayPath<'a>(&'a [u8]);

impl core::fmt::Display for DisplayPath<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0.is_empty() {
            return formatter.write_str("-");
        }
        for byte in self.0 {
            formatter.write_str(
                core::str::from_utf8(core::slice::from_ref(byte)).map_err(|_| core::fmt::Error)?,
            )?;
        }
        Ok(())
    }
}

/// representation in the reply this cutover answers with.
#[allow(clippy::too_many_lines)]
fn serve_block_transact(
    graph: &GraphTables,
    devices: &mut BlockDevices,
    window: Option<transfer_window::Window>,
    scratch: &ScratchPage,
    id: TaskId,
    words: &[sel4::Word],
) -> Response {
    use slime_proto::block::{
        BLOCK_MAGIC, FORMAT_VERSION, OFF_REPLY_MAGIC, OFF_REPLY_SECTORS_DONE, OFF_REPLY_STATUS,
        OFF_REPLY_VERSION, OP_FLUSH, OP_READ, OP_WRITE, REPLY_LEN, WireBlockRequest,
    };

    let Some(slot) = words.first().map(|slot| *slot as u32) else {
        return Response::error(IpcError::InvalidLength);
    };
    let Some(table) = graph.get(id) else {
        return Response::error(IpcError::BadCapability);
    };
    let Some(capability) = table.get(slot) else {
        return Response::error(IpcError::BadCapability);
    };
    let graph::Resource::Block { device: index } = capability.resource else {
        return Response::error(IpcError::BadCapability);
    };
    let index = index as usize;
    // Which device: the capability's own index, placed by the generation. A
    // component holding the source cannot name the receiver, because the index
    // is in the capability rather than in the request.
    let Some(device) = devices.get_mut(index) else {
        // Authority the boot could not back: the generation granted the device
        // but none was attached. A bounded refusal, not a fault.
        return Response::error(IpcError::UnsupportedOperation);
    };
    let Some(transfer) = words.get(1).copied() else {
        return Response::error(IpcError::InvalidLength);
    };
    // The wide reader: a write carries its sector behind the 64-byte record, so
    // the request is 576 bytes and the *message* reader's 64-byte bound would
    // refuse it. `read_staged_array` refuses any descriptor naming a
    // capability, which is the rule this operation needs anyway.
    let frame = match transfer_window::read_staged_array(window, transfer, words, scratch) {
        Ok(frame) => frame,
        Err(error) => return Response::error(error),
    };
    let Some(request) = WireBlockRequest::decode(frame.bytes()) else {
        return Response::error(IpcError::InvalidLength);
    };
    if request.magic != BLOCK_MAGIC || request.version != FORMAT_VERSION {
        return Response::error(IpcError::InvalidLength);
    }
    let required = match request.op {
        OP_READ => RIGHT_BLOCK_READ,
        OP_WRITE | OP_FLUSH => RIGHT_BLOCK_WRITE,
        _ => return Response::error(IpcError::InvalidLength),
    };
    if capability.rights & required == 0 {
        sel4::debug_println!(
            "SLIME_GRAPH block refused task={} op={} class=rights",
            id.0,
            request.op,
        );
        return Response::error(IpcError::BadCapability);
    }
    // One sector per request. The reply carries `sectors_done`, so a partial
    // completion is representable — but nothing in this cutover produces one,
    // and accepting a count this driver would silently truncate is worse than
    // refusing it.
    if request.op != OP_FLUSH && request.sector_count != 1 {
        return Response::error(IpcError::InvalidLength);
    }

    let mut sector = [0u8; virtio_blk::SECTOR_BYTES];
    let outcome = match request.op {
        OP_READ => device.read_sector(request.lba, &mut sector),
        OP_WRITE => {
            let bytes = frame.bytes();
            let start = slime_proto::block::REQUEST_LEN;
            match bytes.get(start..start + virtio_blk::SECTOR_BYTES) {
                Some(payload) => {
                    sector.copy_from_slice(payload);
                    device.write_sector(request.lba, &sector)
                }
                None => return Response::error(IpcError::InvalidLength),
            }
        }
        _ => device.flush(),
    };
    let (status, sectors_done) = match outcome {
        Ok(()) => (0i32, if request.op == OP_FLUSH { 0 } else { 1u32 }),
        Err(error) => {
            sel4::debug_println!(
                "SLIME_GRAPH block failed task={} op={} lba={} {error:?}",
                id.0,
                request.op,
                request.lba,
            );
            (-1i32, 0)
        }
    };
    sel4::debug_println!(
        "SLIME_GRAPH block served task={} op={} lba={} status={status} sectors={sectors_done}",
        id.0,
        request.op,
        request.lba,
    );

    // The reply is the 64-byte record, and for a successful read the sector
    // follows it in the caller's window.
    //
    // Written as one region rather than a `StagedFrame`, whose bound is
    // `MAX_STAGED_BYTES` — the *message* bound, 64 bytes. A sector is not a
    // message: it crosses no channel and is bounded by the window, exactly as
    // `DebugWrite`'s line is. `write_staged_region` is the same write path
    // without the message-shaped ceiling.
    let mut reply = [0u8; REPLY_LEN + virtio_blk::SECTOR_BYTES];
    reply[OFF_REPLY_MAGIC..OFF_REPLY_MAGIC + 4].copy_from_slice(&BLOCK_MAGIC.to_le_bytes());
    reply[OFF_REPLY_VERSION..OFF_REPLY_VERSION + 4].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    reply[OFF_REPLY_STATUS..OFF_REPLY_STATUS + 4].copy_from_slice(&status.to_le_bytes());
    reply[OFF_REPLY_SECTORS_DONE..OFF_REPLY_SECTORS_DONE + 4]
        .copy_from_slice(&sectors_done.to_le_bytes());
    let length = if request.op == OP_READ && status == 0 {
        reply[REPLY_LEN..].copy_from_slice(&sector);
        reply.len()
    } else {
        REPLY_LEN
    };
    match transfer_window::write_staged_region(window, &reply[..length], scratch) {
        Ok(descriptor) => Response::success(length as i64, descriptor),
        Err(error) => Response::error(error),
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
/// Stage and activate exactly the root-owned autostart instances in a v4
/// generation. Executables are a catalogue: a loadable catalogue entry is not
/// itself a request to construct a task.
fn launch_instance_graph(
    generation: &Generation<'_>,
    admission: &Admission,
    allocator: &mut ObjectAllocator,
    scratch: &ScratchPage,
    service_endpoint: sel4::cap::Endpoint,
    console_endpoint: sel4::cap::Endpoint,
    block_devices: &mut BlockDevices,
    #[cfg(slime_boot_selector)] boot_runtime: &mut boot_selector::BootRuntime,
) {
    let mut tasks = TaskTable::<MAX_TASKS>::new();
    let mut windows = WindowTable::<MAX_TASKS>::new();
    let mut graph = GraphTables::new();
    let channels = unsafe { &mut *ptr::addr_of_mut!(CHANNELS) };
    let mut launched_instances = LaunchedInstances::new();
    let aligned = unsafe { &mut *ptr::addr_of_mut!(ELF_SCRATCH) };
    let mut launched = 0;

    for instance_index in 0..generation.instance_count() {
        let instance = match generation.instance(instance_index) {
            Ok(instance) => instance,
            Err(error) => fatal!("SLIME_GRAPH FAIL instance rejected: {error:?}"),
        };
        if !instance.is_root_autostart() {
            continue;
        }
        let executable = match generation.executable(instance.executable) {
            Ok(executable) => executable,
            Err(error) => fatal!("SLIME_GRAPH FAIL executable rejected: {error:?}"),
        };
        let Some(plan) = admission.executable_plan(instance.executable) else {
            fatal!(
                "SLIME_GRAPH FAIL executable {} was not admitted",
                executable.name
            )
        };
        if !plan.format.is_loadable() {
            fatal!(
                "SLIME_GRAPH FAIL root instance {} executable {} is not loadable",
                instance.name,
                executable.name
            )
        }
        let object = match generation.object(executable.object) {
            Ok(object) => object,
            Err(error) => fatal!("SLIME_GRAPH FAIL object rejected: {error:?}"),
        };
        let profile = match boot_contracts::target_profile::TargetProfile::by_name(TARGET_PROFILE) {
            Ok(profile) => profile,
            Err(error) => fatal!("SLIME_GRAPH FAIL profile unavailable: {error:?}"),
        };
        let elf = match boot_contracts::component_image::admit_elf(object.bytes, profile) {
            Ok(elf) => elf,
            Err(error) => fatal!(
                "SLIME_GRAPH FAIL executable {} refused: {error:?}",
                executable.name
            ),
        };
        let elf = match aligned.hold(elf) {
            Ok(elf) => elf,
            Err(len) => fatal!(
                "SLIME_GRAPH FAIL executable {} is {len} bytes, over the load bound",
                executable.name
            ),
        };
        let image = match ChildImage::parse(elf) {
            Ok(image) => image,
            Err(error) => fatal!(
                "SLIME_GRAPH FAIL executable {} image rejected: {error:?}",
                executable.name
            ),
        };
        let authority = match bound_authority(generation, instance) {
            Ok(authority) => authority,
            Err(error) => fatal!("SLIME_GRAPH FAIL binding authority rejected: {error:?}"),
        };
        // The child's CSpace is exactly as large as the admitted plan says its
        // declared authority needs, not a compiled-in shell.
        let cspace_size_bits = match generation.instance_cspace_size_bits(instance_index) {
            Ok(Some(bits)) => bits as usize,
            Ok(None) => fatal!(
                "SLIME_GRAPH FAIL instance {} has no planned CSpace",
                instance.name
            ),
            Err(error) => fatal!("SLIME_GRAPH FAIL CSpace plan rejected: {error:?}"),
        };
        // The child's own TCB and fault endpoint go where the plan declared
        // them. A plan that omits either leaves the root nowhere to install
        // authority the child needs, so it is refused rather than defaulted.
        let child_slots = match generation.instance_child_slots(instance_index) {
            Ok(Some(boot_contracts::generation::ChildSlotPlan {
                service: Some(service),
                console: Some(console),
                tcb: Some(tcb),
                fault: Some(fault),
            })) => match (task::ChildSlots {
                service: service as sel4::CPtrBits,
                console: console as sel4::CPtrBits,
                tcb: tcb as sel4::CPtrBits,
                fault: fault as sel4::CPtrBits,
            })
            .validate()
            {
                Ok(slots) => slots,
                Err(error) => fatal!(
                    "SLIME_GRAPH FAIL instance {} declares an unusable child layout: {error:?}",
                    instance.name
                ),
            },
            Ok(_) => fatal!(
                "SLIME_GRAPH FAIL instance {} has no planned service, console, TCB, or fault slot",
                instance.name
            ),
            Err(error) => fatal!("SLIME_GRAPH FAIL child slot plan rejected: {error:?}"),
        };
        let id = match tasks.create(
            allocator,
            &image,
            service_endpoint,
            console_endpoint,
            authority,
            Supervision::SelfManaged,
            sel4::init_thread::slot::VSPACE.cap(),
            scratch,
            sel4::init_thread::slot::ASID_POOL.cap(),
            None,
            Some(instance.executable),
            Some(instance_index),
            // Only the bootstrap instance composes a boot graph, so only it is
            // told which one. Every other instance starts with zero.
            if instance_index == generation.bootstrap() {
                generation.boot_action.id()
            } else {
                0
            },
            cspace_size_bits,
            child_slots,
        ) {
            Ok(id) => id,
            Err(error) => fatal!(
                "SLIME_GRAPH FAIL instance {} construction failed: {error:?}",
                instance.name
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
        let Ok(table) = graph.create(id) else {
            fatal!(
                "SLIME_GRAPH FAIL capability table unavailable for task {}",
                id.0
            )
        };

        let mut block_index = 0u8;
        for binding_index in 0..instance.binding_count() {
            let binding = match generation.binding(instance, binding_index) {
                Ok(binding) => binding,
                Err(error) => fatal!(
                    "SLIME_GRAPH FAIL instance {} binding rejected: {error:?}",
                    instance.name
                ),
            };
            let grant = match generation.grant(binding.grant) {
                Ok(grant) => grant,
                Err(error) => fatal!("SLIME_GRAPH FAIL bound grant rejected: {error:?}"),
            };
            if grant.rights & (RIGHT_SEND | RIGHT_RECV) != 0 {
                continue;
            }
            if !generation.grant_applies_to_instance(grant, instance_index) {
                fatal!(
                    "SLIME_GRAPH FAIL binding {} is unrelated to instance {}",
                    grant.name,
                    instance.name
                )
            }
            let capability = if grant.rights & RIGHT_EXEC != 0 {
                let GrantEndpoint::Executable(executable) = grant.target else {
                    fatal!(
                        "SLIME_GRAPH FAIL binding {} executable target rejected",
                        grant.name
                    )
                };
                graph::Capability {
                    resource: graph::Resource::Executable { executable },
                    rights: grant.rights,
                }
            } else {
                let Some((valid, resource)) = declared_resource(grant.rights) else {
                    fatal!(
                        "SLIME_GRAPH FAIL binding {} names no installable resource",
                        grant.name
                    )
                };
                if grant.rights & valid & !RIGHT_TRANSFER == 0 {
                    fatal!(
                        "SLIME_GRAPH FAIL binding {} carries invalid rights",
                        grant.name
                    )
                }
                let resource = match resource {
                    graph::Resource::Block { .. } => {
                        let device = block_index;
                        block_index = block_index.saturating_add(1);
                        graph::Resource::Block { device }
                    }
                    other => other,
                };
                graph::Capability {
                    resource,
                    rights: grant.rights & valid,
                }
            };
            let slot = match u32::try_from(binding.slot) {
                Ok(slot) => slot,
                Err(_) => fatal!(
                    "SLIME_GRAPH FAIL binding {} slot is out of range",
                    grant.name
                ),
            };
            if let Err(error) = table.install(slot, capability) {
                fatal!(
                    "SLIME_GRAPH FAIL binding {} slot={slot} rejected: {error:?}",
                    grant.name
                )
            }
        }
        if let Err(error) = launched_instances.record(instance_index, instance.executable, id) {
            fatal!("SLIME_GRAPH FAIL instance mapping rejected: {error:?}")
        }
        sel4::debug_println!(
            "SLIME_GRAPH staged task={} instance={} executable={} grants={} bindings={} window={:#x} frames={} tables={} entry={:#x}",
            id.0,
            instance.name,
            executable.name,
            authority.grants,
            instance.binding_count(),
            task.vspace.transfer_window_addr,
            task.vspace.frames_mapped,
            task.vspace.tables_mapped,
            task.entry,
        );
        launched += 1;
    }

    sel4::debug_println!(
        "SLIME_GRAPH staged instances={launched} root_autostart={} loadable_executables={} slimecm={} wrong_target={} unrecognized={}",
        admission.root_autostart_instances(generation).count(),
        admission.loadable,
        admission.slime_component_images,
        admission.wrong_target_images,
        admission.unrecognized_images,
    );

    let materialized =
        match channel::materialize(generation, &launched_instances, channels, &mut graph) {
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

    let bootstrap = launched_instances.task_for_instance(admission.bootstrap_instance);
    let table = bootstrap.and_then(|id| graph.get(id));
    sel4::debug_println!(
        "[layout] path={} slots={} max={}",
        generation
            .instance(admission.bootstrap_instance)
            .map_or("?", |instance| instance.name),
        table.map_or(0, |table| table.len()),
        graph::MAX_TASK_CAPS,
    );
    if let Some(table) = table {
        for (slot, capability) in table.slots() {
            let Some(capability) = capability else {
                continue;
            };
            sel4::debug_println!(
                "[layout] {slot} {} {} {:#x}",
                capability.resource.kind(),
                resource_label(generation, &capability.resource),
                capability.rights,
            );
        }
    }
    sel4::debug_println!("[layout] end");

    let mut active = [false; MAX_TASKS];
    let mut activated = 0;
    while activated < launched {
        let before = activated;
        for launched_instance in launched_instances.iter() {
            if active[launched_instance.instance] {
                continue;
            }
            let instance = match generation.instance(launched_instance.instance) {
                Ok(instance) => instance,
                Err(error) => fatal!("SLIME_GRAPH FAIL activation instance rejected: {error:?}"),
            };
            let mut ready = true;
            for dependency_index in 0..instance.dependency_count() {
                let dependency = match generation.dependency(instance, dependency_index) {
                    Ok(dependency) => dependency,
                    Err(error) => fatal!("SLIME_GRAPH FAIL dependency rejected: {error:?}"),
                };
                let dependency_index = (0..generation.instance_count())
                    .find(|index| {
                        generation
                            .instance(*index)
                            .is_ok_and(|candidate| candidate.name == dependency.name)
                    })
                    .unwrap_or(usize::MAX);
                if dependency_index >= active.len()
                    || launched_instances
                        .task_for_instance(dependency_index)
                        .is_none()
                {
                    fatal!(
                        "SLIME_GRAPH FAIL instance {} has non-root dependency {}",
                        instance.name,
                        dependency.name
                    )
                }
                ready &= active[dependency_index];
            }
            if !ready {
                continue;
            }
            if let Err(error) = tasks.activate(launched_instance.task) {
                fatal!(
                    "SLIME_GRAPH FAIL activation failed instance={}: {error:?}",
                    instance.name
                )
            }
            active[launched_instance.instance] = true;
            activated += 1;
        }
        if activated == before {
            fatal!("SLIME_GRAPH FAIL root instance dependency barrier unsatisfied")
        }
    }
    sel4::debug_println!("SLIME_GRAPH activated instances={activated}");

    let mut buffers = SharedBufferTable::new(GenerationEpoch(generation.number));
    let budget = shared_buffer_budget(generation);
    let mut budgeted = 0;
    for launched_instance in launched_instances.iter() {
        let instance = generation.instance(launched_instance.instance).unwrap();
        let quota = declared_quota(budget.as_ref(), instance.name);
        if quota != HolderQuota::DENY {
            budgeted += 1;
        }
        if let Err(error) =
            buffers.declare_quota(HolderId(u64::from(launched_instance.task.0)), quota)
        {
            fatal!(
                "SLIME_GRAPH FAIL quota rejected task={}: {error:?}",
                launched_instance.task.0
            )
        }
    }
    sel4::debug_println!(
        "SLIME_GRAPH quotas declared={} budgeted={budgeted} holders={}",
        launched_instances.len(),
        budget.as_ref().map_or(0, SharedBufferBudget::holder_count),
    );
    serve_instance_graph(
        generation,
        &mut launched_instances,
        service_endpoint,
        console_endpoint,
        &mut tasks,
        &mut windows,
        &mut graph,
        channels,
        &mut buffers,
        allocator,
        scratch,
        block_devices,
        #[cfg(slime_boot_selector)]
        boot_runtime,
    );
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
/// Generous against what the declared components actually issue — each binds a
/// window, and spawn-service additionally runs a shared-buffer probe and spawns
/// two children — while still bounding a livelock so it fails in seconds rather
/// than burning the gate's whole timeout.
///
/// **Headroom is measured, not assumed.** P5.5.1 made `recv` non-blocking, so a
/// component that blocks now costs two iterations where it cost one — the
/// `recv` that reports `WouldBlock` and the `wait` that parks.
///
/// The stream plane's nine tasks provision four route roles and broker seven
/// samples in **136** iterations. The **QoS** plane is far denser and is what
/// set this number: it drives a simulated clock through scheduled deadline,
/// lifespan, liveliness, and retry boundaries, and each boundary is a park/wake
/// cycle for the broker plus a sweep of every participant. At 512 it exhausted
/// the bound with `fabric-publisher`'s send still queued — diagnosed at length
/// as B28 and mistaken in turn for a lost wake, a scheduler fault, and an
/// always-ready park source before the cause turned out to be this constant.
/// Measured directly: **768 completes, 512 does not.** 2048 is that floor with
/// Raised for P5.4.3's dango plane, which is the densest composition
/// this port runs: a scripted console session is one round trip *per keystroke*
/// — 96 bytes of script — on top of four components' startup, every command's
/// profile resolution, and a spawn plus a supervised wait per launch. Measured
/// the same way B28 was: 2048 exhausted with the session still parked.
const MAX_GRAPH_ITERATIONS: usize = 32768;

/// Serve the root operation surface for the component graph.
///
/// Every arrival is decoded by `ipc::Operation::from_label`, so the whole legacy
/// syscall surface resolves to a bounded answer: an operation this cutover does
/// not mediate returns its ordinary Slime error rather than faulting the caller,
/// which is P5.2's third required check.
#[allow(clippy::too_many_arguments)]
fn serve_instance_graph(
    generation: &Generation<'_>,
    launched: &mut LaunchedInstances,
    endpoint: sel4::cap::Endpoint,
    console_endpoint: sel4::cap::Endpoint,
    tasks: &mut TaskTable<MAX_TASKS>,
    windows: &mut WindowTable<MAX_TASKS>,
    graph: &mut GraphTables,
    channels: &mut ChannelTable,
    buffers: &mut SharedBufferTable,
    allocator: &mut ObjectAllocator,
    scratch: &ScratchPage,
    // The block devices attached and brought up (P5.4.2c, P5.4.3). Empty on
    // every machine without a disk — a `BlockTransact` then answers a bounded
    // refusal — and up to `MAX_BLOCK_DEVICES` when M6.7's two are present.
    block_devices: &mut BlockDevices,
    #[cfg(slime_boot_selector)] boot_runtime: &mut boot_selector::BootRuntime,
) {
    sel4::debug_println!(
        "SLIME_ROOT allocator baseline live_slots={} live_objects={} live_bytes={}",
        allocator.live_slots(),
        allocator.live_objects(),
        allocator.live_bytes(),
    );
    // The shared filesystem namespaces (M6.3). Local to the serve loop, like
    // every other table here: nothing outlives the graph it belongs to.
    let mut namespaces = Namespaces::new();
    // The scripted key source, selected by generation number exactly as the
    // oracle's `bootstrap` selects it.
    let mut input = ScriptedInput::new(input_script(generation.number));
    // Interned directory scopes. A capability carries an index into this, not
    // a path: inlining 128 bytes into every `Resource` grew the capability
    // tables past the root's stack.
    let mut scopes = graph::ScopeTable::new();
    let mut live = tasks.len();
    let mut unsupported = 0;
    let mut unimplemented = 0;
    let mut buffers_served = 0;
    let mut loans_served = 0;
    let mut sends = 0;
    let mut receives = 0;
    let mut parks = 0;
    let mut peer_deaths = 0;
    let mut spawns = 0;
    let mut drops = 0;
    let mut reclaimed_slots = 0;
    let mut endpoints = 0;
    // Narrow-on-transfer moves served (C8.3). Counted apart from `sends`
    // because a transfer is the operation a broker provisions a role with, and
    // a graph where every participant holds its own declared edge performs
    // none.
    let mut transfers = 0;
    let mut parked = ParkedReplies::new();
    // How each dead child ended, kept past the task's own reclamation because
    // that is precisely when its parent asks. See `supervision.rs`.
    let mut terminations = supervision::Terminations::new();
    // Which parked tasks are waiting on which child. A supervision wait has no
    // queue to register on, so the registration lives here; see
    // `supervision.rs`.
    let mut supervision_waits = supervision::SupervisionWaits::new();
    // Capabilities between their send and the receive that collects them. Held
    // here rather than in either task's table, because in flight they belong to
    // neither; see `transit.rs`.
    let mut transit = Transit::new();
    let mut iterations = 0;
    let required = (0..generation.instance_count())
        .filter(|index| {
            generation.instance(*index).is_ok_and(|instance| {
                instance.autostart && instance.health == InstanceHealth::Required
            })
        })
        .count();
    let mut completed_required = [false; generation::MAX_ADMITTED_INSTANCES];
    let mut healthy_emitted = false;
    for _ in 0..MAX_GRAPH_ITERATIONS {
        iterations += 1;
        if live == 0 {
            // A graph that runs out of live tasks while a park is outstanding
            // did not settle: the parked task is blocked on a reply only this
            // loop can send, and leaving here makes the root return, which
            // marks it `Inactive` and strands every child still sending to it.
            // Reported rather than broken out of silently, because the boot
            // otherwise ends with an ordinary accounting summary and looks
            // healthy — that is precisely how B28 hid.
            if !parked.is_empty() {
                fatal!(
                    "SLIME_GRAPH FAIL graph settled with replies owed count={}",
                    parked.len(),
                )
            }
            sel4::debug_println!(
                "SLIME_ROOT allocator quiescent live_slots={} live_objects={} live_bytes={}",
                allocator.live_slots(),
                allocator.live_objects(),
                allocator.live_bytes(),
            );
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
            if let Some(instance_index) = tasks.get(id).and_then(|task| task.instance)
                && let Ok(instance) = generation.instance(instance_index)
                && instance.health == InstanceHealth::Required
            {
                fatal!("SLIME_GRAPH FAIL required instance {} fault", instance.name)
            }
            let reason = match fault::decode_fault(&info) {
                Ok(detail) => {
                    sel4::debug_println!(
                        "SLIME_GRAPH component fault task={} kind={:?} address={:?}",
                        id.0,
                        detail.kind,
                        detail.address,
                    );
                    detail.kind.reason_code()
                }
                Err(error) => {
                    sel4::debug_println!(
                        "SLIME_GRAPH fault undecodable task={} error={error:?}",
                        id.0
                    );
                    u64::MAX
                }
            };
            record_termination(
                &mut terminations,
                graph,
                &transit,
                id,
                supervision::Termination::Fault(reason),
            );
            wake_supervisors(&mut parked, &mut supervision_waits, id);
            if let Some(task) = tasks.get(id) {
                let _ = task.suspend();
            }
            reclaim_dead_task(
                channels,
                &mut parked,
                &mut transit,
                graph,
                buffers,
                allocator,
                &mut supervision_waits,
                id,
                &mut peer_deaths,
            );
            graph.release(id);
            windows.release(id);
            reclaim_task_objects(launched, tasks, allocator, &mut reclaimed_slots, id);
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
            // M6.3: the three directory operations (P5.4.3).
            //
            // Mechanism, not policy. What a directory *contains* is a
            // filesystem component's business, built over the object store;
            // what the root owns is the unforgeable part — a shared namespace
            // root, scoped views that derivation may only narrow, and an atomic
            // compare-and-swap that keeps two writers from losing an update.
            // M6.4: one scripted key event, gated on an `Input` capability.
            Operation::InputRead => {
                ipc::reply(serve_input_read(graph, &mut input, id, &words));
            }
            Operation::DirectoryInspect => {
                ipc::reply(serve_directory_inspect(
                    graph,
                    &namespaces,
                    &scopes,
                    windows.bound(id),
                    scratch,
                    id,
                    &words,
                ));
            }
            Operation::DirectoryDerive => {
                ipc::reply(serve_directory_derive(
                    graph,
                    &mut scopes,
                    windows.bound(id),
                    scratch,
                    id,
                    &words,
                ));
            }
            Operation::DirectoryCommit => {
                ipc::reply(serve_directory_commit(
                    graph,
                    &mut namespaces,
                    &scopes,
                    windows.bound(id),
                    scratch,
                    id,
                    &words,
                ));
            }
            // P5.4.2c: sectors, mediated.
            //
            // The root owns the driver because it owns the device untyped and
            // the DMA frames; what it does *not* own is any policy about what
            // the sectors mean. This arm authenticates the caller's capability,
            // checks the operation against its rights, and hands the request to
            // the driver. Partitioning, the object store, generations, and
            // recovery all sit above it in userspace, exactly as they do on the
            // oracle.
            Operation::BlockTransact => {
                ipc::reply(serve_block_transact(
                    graph,
                    block_devices,
                    windows.bound(id),
                    scratch,
                    id,
                    &words,
                ));
            }
            // A clean exit is a send, not a call: the task is suspended rather
            // than replied to.
            Operation::Exit => {
                let status = words[0] as i64;
                sel4::debug_println!("SLIME_GRAPH component exit task={} status={status}", id.0);
                if status != 0
                    && let Some(instance_index) = tasks.get(id).and_then(|task| task.instance)
                    && let Ok(instance) = generation.instance(instance_index)
                    && instance.health == InstanceHealth::Required
                {
                    fatal!(
                        "SLIME_GRAPH FAIL required instance {} exit status={status}",
                        instance.name
                    )
                }
                if status == 0
                    && let Some(instance_index) = tasks.get(id).and_then(|task| task.instance)
                    && generation
                        .instance(instance_index)
                        .is_ok_and(|instance| instance.health == InstanceHealth::Required)
                    && let Some(completed) = completed_required.get_mut(instance_index)
                {
                    *completed = true;
                }
                // Recorded before the reclamation that erases everything else
                // about this task, and before the parked-supervision wake
                // below, so a parent woken by this death finds the outcome
                // already there rather than racing it.
                record_termination(
                    &mut terminations,
                    graph,
                    &transit,
                    id,
                    supervision::Termination::Exit(status),
                );
                wake_supervisors(&mut parked, &mut supervision_waits, id);
                if let Some(task) = tasks.get(id) {
                    let _ = task.suspend();
                }
                reclaim_dead_task(
                    channels,
                    &mut parked,
                    &mut transit,
                    graph,
                    buffers,
                    allocator,
                    &mut supervision_waits,
                    id,
                    &mut peer_deaths,
                );
                graph.release(id);
                windows.release(id);
                reclaim_task_objects(launched, tasks, allocator, &mut reclaimed_slots, id);
                live -= 1;
            }
            // Spawn the executable a declared grant named. The slot resolves
            // through the caller's own table, so a component can start exactly
            // the executables its generation granted it and nothing else — an
            // ungranted slot resolves to nothing and is refused.
            //
            // The child's capabilities are derived copies of the parent's, each
            // narrowed to rights the parent already holds, installed at slots
            // `0..n` in the order the parent declared them. That numbering is
            // the whole distribution mechanism: a component addresses its first
            // spawn grant as slot 0, which is why `console.rs`,
            // `spawn-service.rs::RPC_SLOT`, and `launch_context::CONTEXT_SLOT`
            // all read 0.
            Operation::Spawn => {
                let response = serve_spawn(
                    generation,
                    launched,
                    tasks,
                    windows,
                    graph,
                    channels,
                    buffers,
                    allocator,
                    scratch,
                    endpoint,
                    console_endpoint,
                    id,
                    &words,
                    &mut spawns,
                );
                if response.result >= 0 {
                    live += 1;
                }
                ipc::reply(response);
            }
            // Mint a channel pair through a declared `EndpointFactory`.
            //
            // Both ends land in the caller's own table as distinct sides, which
            // is what makes the pair useful: the caller keeps one and copies or
            // moves the other to a child. Both directed queues exist from the
            // mint; no holder reassignment changes what a capability means.
            Operation::EndpointCreate => {
                let response = match graph
                    .get(id)
                    .ok_or(IpcError::InvalidOperation)
                    .and_then(|table| table.resolve(words[0] as u32, RIGHT_ENDPOINT_CREATE))
                {
                    Ok(graph::Capability {
                        resource: graph::Resource::EndpointFactory,
                        ..
                    }) => match mint_channel(channels, graph, &transit, id) {
                        Ok(key) => {
                            // Both slots reserved before either is installed:
                            // a pair with one end placed is a channel the
                            // caller can never finish setting up.
                            let placed = graph.get_mut(id).and_then(|table| {
                                let first = table.free_slot_from(1)?;
                                let producer = graph::Capability {
                                    resource: graph::Resource::Endpoint {
                                        channel: key,
                                        side: graph::Side::Producer,
                                    },
                                    rights: RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
                                };
                                table.install(first, producer).ok()?;
                                let Some(second) = table.free_slot_from(first + 1) else {
                                    table.drop_slot(first);
                                    return None;
                                };
                                let consumer = graph::Capability {
                                    resource: graph::Resource::Endpoint {
                                        channel: key,
                                        side: graph::Side::Consumer,
                                    },
                                    rights: RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
                                };
                                if table.install(second, consumer).is_err() {
                                    table.drop_slot(first);
                                    return None;
                                }
                                Some((first, second))
                            });
                            match placed {
                                Some((first, second)) => {
                                    endpoints += 1;
                                    sel4::debug_println!(
                                        "SLIME_GRAPH endpoint minted task={} key={key} slots={first},{second}",
                                        id.0,
                                    );
                                    Response::success(i64::from(first), second as sel4::Word)
                                }
                                // `BadCapability` (-1), matching
                                // `kernel/src/syscall/mod.rs::sys_endpoint_create`,
                                // which folds `available_slots() >= 2` into the
                                // same `allowed` test as the factory check and
                                // answers `ERR_BAD_CAP` for both. Exact-code
                                // parity matters here for the reason it did on
                                // the spawn refusal: components compare against
                                // literal values, and `spawn-service.rs`
                                // returns this one straight to its client.
                                None => Response::error(IpcError::BadCapability),
                            }
                        }
                        // The channel table is full: a bounded resource
                        // exhausted, not a capability the caller lacks.
                        Err(_) => Response::error(IpcError::DestinationSlotsExhausted),
                    },
                    // An ungranted slot and a slot naming another kind are
                    // refused identically, as everywhere else.
                    _ => Response::error(IpcError::BadCapability),
                };
                ipc::reply(response);
            }
            // Collect a child's outcome through the handle its spawn returned.
            //
            // Named through a capability, never through a task id: a component
            // can only learn the fate of a child it started, and only while it
            // still holds the handle. The record outlives the child itself —
            // see `supervision.rs` — because the answer is owed after the task
            // and its whole table are gone.
            Operation::SupervisionStatus => {
                let response = serve_supervision_status(graph, &terminations, id, &words);
                ipc::reply(response);
            }
            // Release a capability the caller holds.
            //
            // `spawn_or_fail` drops each supervision handle as soon as the
            // spawn returns, so a graph that launches many children does not
            // exhaust its own table on handles it never waits on. Dropping is
            // unconditional on rights: giving up authority needs none, and a
            // slot holding nothing is refused so a component cannot use the
            // answer to probe its table.
            Operation::CapDrop => {
                let slot = words[0] as u32;
                let dropped = graph.get_mut(id).is_some_and(|table| table.drop_slot(slot));
                ipc::reply(if dropped {
                    drops += 1;
                    Response::success(0, 0)
                } else {
                    Response::error(IpcError::BadCapability)
                });
            }
            // B25: a second supervision handle naming a task the caller already
            // supervises.
            //
            // Each spawn returns exactly one handle, and neither route places it
            // twice — a spawn grant copies but must run before the child exists,
            // and `CapTransfer` moves. So a parent that must introduce one child
            // to two others could not, despite holding the authority.
            //
            // Authority is unchanged by construction: the new capability names
            // the same task, and its rights are the source's own, so this can
            // only ever produce a handle the caller could already have passed on.
            // `RIGHT_SUPERVISE` is required to ask, which is the same gate
            // `serve_supervision_status` gates a query behind.
            Operation::SupervisionDerive => {
                ipc::reply(serve_supervision_derive(graph, id, &words));
            }
            // Emit a component's diagnostic line as one uninterruptible unit
            // (B18).
            //
            // Components used to bypass the root entirely here, calling
            // `seL4_DebugPutChar` per byte from their own thread. That is one
            // syscall per character, so the root's own `debug_println!` — or
            // another component's line — could land in the middle of a marker
            // and destroy it. The transcript then showed ` QoS matched` where
            // `[fabric] QoS matched` was written, and whichever gate required
            // that marker failed on a boot that was otherwise correct. It cost
            // this milestone's gate roughly one run in three.
            //
            // Serving it here fixes that by construction rather than by
            // ordering: the graph loop is single-threaded and answers one
            // request at a time, so a line assembled and printed inside this
            // arm cannot interleave with anything.
            //
            // The bytes travel like any other payload, through the caller's
            // transfer window, which is why this is the only operation whose
            // component-side implementation had a reason to avoid the root: a
            // task that has not bound a window cannot print. That is acceptable
            // — every launched component binds one before it runs, and a task
            // that has not is not yet in a state where its output would be
            // attributable anyway.
            //
            // Read with the *wide* reader rather than the message reader. A
            // diagnostic line is not a message: it crosses no channel, is
            // bounded by nothing the IPC contract states, and
            // `MAX_MESSAGE_BYTES` is 64. The visibility broker's
            // `write_record` emits a 64-byte record as 128 hex characters, so
            // under the narrow reader every one of C8.8's view and trace
            // records was refused as `InvalidLength` and the line vanished
            // from the transcript. `MAX_STAGED_ARRAY_BYTES` (1 KiB) is the
            // same bound the wide spawn-grant array already crosses this
            // window with.
            Operation::DebugWrite => {
                let response = match transfer_window::read_staged_array(
                    windows.bound(id),
                    words[1],
                    &words,
                    scratch,
                ) {
                    Ok(frame) => {
                        // One `debug_print!` for the whole payload. Not
                        // `debug_println!`: the component's bytes carry their
                        // own newline, and adding one would reflow every
                        // marker the x86 corpus records.
                        let bytes = frame.bytes();
                        if let Ok(text) = core::str::from_utf8(bytes) {
                            sel4::debug_print!("{text}");
                        } else {
                            // Not text. Printed as an explicit refusal rather
                            // than lossily, so a component cannot inject
                            // arbitrary bytes into a transcript gates parse.
                            sel4::debug_println!(
                                "SLIME_GRAPH debug write refused task={} bytes={} reason=not-utf8",
                                id.0,
                                bytes.len(),
                            );
                        }
                        Response::success(bytes.len() as i64, 0)
                    }
                    // `read_staged_array` refuses a descriptor naming any
                    // capability, which is the same rule the narrow path
                    // enforced explicitly: a diagnostic line carries none.
                    Err(error) => Response::error(error),
                };
                ipc::reply(response);
            }
            // C8.3's narrow-on-transfer move (P5.5.1). The one mechanism a
            // userspace fabric needs that neither `send` nor `spawn` provides:
            // `send` moves only a loan, whose handle names its own recipient,
            // and a spawn grant's destination is a task that does not exist
            // yet. A route role is neither — it goes to a task already running,
            // chosen by a broker at runtime, narrowed to one direction and made
            // non-delegable at the moment it crosses.
            //
            // Unparkable, like every other operation here except `send`,
            // `recv`, and `wait`: a queue-full transfer answers `WouldBlock`
            // and the broker retries, matching `sys_cap_transfer`.
            Operation::CapTransfer => {
                let response = serve_cap_transfer(
                    channels,
                    graph,
                    windows,
                    &mut parked,
                    &mut transit,
                    scratch,
                    &mut supervision_waits,
                    id,
                    &words,
                    &mut transfers,
                );
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
                // B13: the factory the caller named, resolved before anything
                // is admitted. The grant authorizes the operation and the
                // budget bounds it — two independent gates, exactly as
                // `kernel/src/syscall/mod.rs::sys_shared_buffer_create` has
                // them. Until P5.3.3 this slot was discarded and the quota was
                // the only bound, which made authority to allocate follow from
                // a budget entry: ambient authority through the back door.
                let factory = graph
                    .get(id)
                    .ok_or(IpcError::InvalidOperation)
                    .and_then(|table| {
                        table.resolve((words[0] & 0xffff_ffff) as u32, RIGHT_BUFFER_CREATE)
                    });
                let response = match factory {
                    Err(_)
                    | Ok(graph::Capability {
                        resource:
                            graph::Resource::Endpoint { .. }
                            | graph::Resource::Executable { .. }
                            | graph::Resource::EndpointFactory
                            | graph::Resource::Supervision { .. }
                            | graph::Resource::SharedBuffer { .. }
                            | graph::Resource::Loan { .. },
                        ..
                    }) => {
                        sel4::debug_println!(
                            "SLIME_GRAPH buffer create refused task={} class=ungranted",
                            id.0,
                        );
                        Response::error(IpcError::BadCapability)
                    }
                    Ok(_) => match serve_buffer_create(buffers, allocator, holder, pages, writable)
                    {
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
                                // As for `EndpointCreate` above:
                                // `sys_shared_buffer_create` folds
                                // `available_slots() >= 1` into its capability
                                // check and answers `ERR_BAD_CAP`.
                                Response::error(IpcError::BadCapability)
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
                    },
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
                    &mut supervision_waits,
                    id,
                    &words,
                    &mut sends,
                );
                parked.answer_saved(saved, response);
            }
            // `recv` is **non-blocking**, exactly as `kernel/src/ipc/mod.rs`
            // makes it: an empty queue whose peer is alive answers
            // `ERR_WOULDBLOCK` and the component decides what to do next.
            //
            // P5.3.1 parked the caller here instead, on the reasoning that a
            // component blocked in a call is blocked either way and answering
            // would make it spin through `wait`. That reasoning holds for a
            // component with **one** source — `console` — and is wrong for any
            // component with several, which is what P5.5.1's fabric first
            // showed: `provision` and `broker` sweep every control endpoint,
            // ingress, and ack before parking across the whole set, and a park
            // inside the sweep freezes it at the first empty source. The fabric
            // ended up parked on its ack channel holding samples the subscriber
            // was parked waiting for — a deadlock invisible to every one-source
            // graph, and produced by the root rather than by the component.
            //
            // So the poll/park split is the component's to make, and the two
            // steps stay separate: `recv` reports, `wait` parks. That is not a
            // reversal of P5.3.1's property — a component blocked on an empty
            // channel is still parked in the kernel and still woken by its
            // peer's send — only of *which operation* holds the reply. It does
            // not reintroduce a spin, because the component's next call is
            // `wait`, which parks; the round trip costs one extra dispatcher
            // iteration per park, not a busy loop.
            Operation::Recv => {
                let saved = saved.expect("recv is parkable");
                let response = match serve_recv(
                    channels,
                    graph,
                    windows,
                    &mut parked,
                    &mut transit,
                    &mut supervision_waits,
                    scratch,
                    id,
                    &words,
                    &mut receives,
                ) {
                    Ok(response) => response,
                    Err(_) => Response::error(IpcError::WouldBlock),
                };
                parked.answer_saved(saved, response);
            }
            Operation::Wait => {
                let saved = saved.expect("wait is parkable");
                match serve_wait(
                    channels,
                    graph,
                    windows,
                    scratch,
                    &terminations,
                    &mut supervision_waits,
                    id,
                    &words,
                ) {
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
                            supervision_waits.clear(id);
                            sel4::debug_println!(
                                "SLIME_GRAPH park refused task={} error={error:?}",
                                id.0
                            );
                            parked.answer_saved(saved, Response::error(error));
                        }
                    },
                    Err(error) => {
                        channels.clear_waits(id);
                        supervision_waits.clear(id);
                        parked.answer_saved(saved, Response::error(error));
                    }
                }
            }
            #[cfg(slime_boot_selector)]
            Operation::HealthConfirm => {
                let authorized = launched.instance_for_task(id).is_some_and(|index| {
                    generation.instance(index).is_ok_and(|instance| {
                        instance.autostart && instance.health == InstanceHealth::Required
                    })
                });
                let response = if !authorized {
                    Response::error(IpcError::BadCapability)
                } else {
                    match block_devices
                        .get_mut(0)
                        .ok_or(boot_selector::SelectorError::NoBootDevice)
                        .and_then(|device| boot_runtime.confirm(device))
                    {
                        Ok(()) => {
                            sel4::debug_println!("SLIME_BOOT promoted");
                            Response::success(0, 0)
                        }
                        Err(error) => {
                            sel4::debug_println!("SLIME_BOOT promotion refused error={error:?}");
                            Response::error(IpcError::InvalidOperation)
                        }
                    }
                };
                ipc::reply(response);
            }
            #[cfg(slime_boot_selector)]
            Operation::Unhealthy => {
                let authorized = launched.instance_for_task(id).is_some_and(|index| {
                    generation
                        .instance(index)
                        .is_ok_and(|instance| instance.health == InstanceHealth::Required)
                });
                let response = if !authorized {
                    Response::error(IpcError::BadCapability)
                } else {
                    match boot_runtime.mark_unhealthy() {
                        Ok(()) => {
                            sel4::debug_println!("SLIME_BOOT unhealthy");
                            Response::success(0, 0)
                        }
                        Err(error) => {
                            sel4::debug_println!("SLIME_BOOT unhealthy refused error={error:?}");
                            Response::error(IpcError::InvalidOperation)
                        }
                    }
                };
                ipc::reply(response);
            }
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
        if !healthy_emitted && required != 0 {
            let mut live_required = 0;
            let mut completed = 0;
            for instance_index in 0..generation.instance_count() {
                let Ok(instance) = generation.instance(instance_index) else {
                    continue;
                };
                if !instance.autostart || instance.health != InstanceHealth::Required {
                    continue;
                }
                if completed_required
                    .get(instance_index)
                    .copied()
                    .unwrap_or(false)
                {
                    completed += 1;
                    continue;
                }
                if tasks
                    .tasks()
                    .find(|task| task.instance == Some(instance_index))
                    .is_some_and(|task| parked.is_parked(task.id))
                {
                    live_required += 1;
                }
            }
            if live_required + completed == required {
                #[cfg(slime_boot_selector)]
                if boot_runtime.running_pending() {
                    let device = block_devices
                        .get_mut(0)
                        .unwrap_or_else(|| fatal!("boot promotion has no boot device"));
                    match boot_runtime.confirm(device) {
                        Ok(()) => sel4::debug_println!("SLIME_BOOT promoted"),
                        Err(error) => fatal!("boot promotion rejected: {error:?}"),
                    }
                }
                // The idle record is the supervisor's certification that the
                // whole declared graph came to rest: every required instance
                // parked, none completed, none failed. It is emitted whenever
                // that holds, not only for a single-instance graph — a
                // migrated graph whose participants are declared instances
                // reaches the same state with more of them.
                //
                // `completed` is reported separately below when any required
                // instance ran to completion instead of parking, which is a
                // finished generation rather than an idle one.
                if completed == 0 {
                    let digest = generation.identity;
                    sel4::debug_println!(
                        "SLIME_GRAPH healthy generation={} instances={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x} required={} live={} idle={} failed=0",
                        generation.number,
                        digest[0],
                        digest[1],
                        digest[2],
                        digest[3],
                        digest[4],
                        digest[5],
                        digest[6],
                        digest[7],
                        required,
                        live_required,
                        live_required,
                    );
                } else {
                    sel4::debug_println!(
                        "SLIME_GRAPH HEALTHY generation={} required={} live={} completed={} failed=0",
                        generation.number,
                        required,
                        live_required,
                        completed,
                    );
                }
                healthy_emitted = true;
            }
        }
    }
    // The loop's bound is a wedge detector, not a schedule: a graph that
    // settles leaves by `live == 0` well inside it. Reaching the last
    // iteration with tasks still live means no arrival advanced the graph, and
    // the accounting below would otherwise print an ordinary-looking summary —
    // exactly how B28 stayed invisible while `init` sat parked forever.
    if iterations == MAX_GRAPH_ITERATIONS && live != 0 {
        // Name the waiters *before* failing. The owed-reply accounting further
        // down never runs on this path — `fatal!` does not return — so a wedge
        // reported only its counts and the transcript had to be read backwards
        // to work out which task was stuck. That is the diagnosis this marker
        // exists to hand over, and withholding it on the one path that needs it
        // was the defect.
        for task in parked.tasks() {
            sel4::debug_println!(
                "SLIME_GRAPH wedged waiter task={} reason={:?}",
                task.0,
                parked.reason(task),
            );
            for (key, receive) in channels.registered_waits(task) {
                sel4::debug_println!(
                    "SLIME_GRAPH wedged waiter task={} channel={key} receive={receive}",
                    task.0,
                );
            }
        }
        fatal!(
            "SLIME_GRAPH FAIL graph iterations exhausted live={live} parked={}",
            parked.len(),
        )
    }
    sel4::debug_println!(
        "SLIME_GRAPH served live={live} unsupported={unsupported} unimplemented={unimplemented} buffers={buffers_served} windows={} tables={}",
        windows.len(),
        graph.len(),
    );
    // Task objects returned, on its own line so the marker above keeps the
    // exact shape four earlier gates already assert.
    //
    // `tasks=0` is the property: every task the graph created has been reclaimed
    // out of the table, which is what frees a parent's declared spawn budget and
    // what makes `CleanupRecord::revoke` run. Before P5.3.4 neither death path
    // reclaimed, so this would have read `tasks=N slots=0` on every boot — the
    // table full of dead entries and not one CSlot returned.
    sel4::debug_println!(
        "SLIME_GRAPH tasks reclaimed live={} slots={reclaimed_slots}",
        tasks.len(),
    );
    // The channel plane's own accounting, kept on its own line so P5.2's
    // terminal marker keeps the exact shape its gate already asserts.
    //
    // `parked=0` and `queues=0` together are what make teardown complete: no
    // task is still blocked on a reply the root owes it, and no queue still
    // believes it has a live peer. `replies` is every saved CSlot handed back,
    // which is what shows the save path is not a leak.
    //
    // `minted` is cumulative and appended last, for the reason
    // `terminated` is: `channel::sweep` reclaims entries once no holder can
    // name them (B22), so a live count reads low on any graph that released as
    // it went — the healthy case rather than the broken one. Appended rather
    // than inserted because the two gates that assert this line match a prefix
    // ending at `replies`, and below `MAX_CHANNELS` a boot mints exactly as
    // many as it ever held.
    sel4::debug_println!(
        "SLIME_GRAPH channels served sends={sends} receives={receives} parks={parks} settled={peer_deaths} parked={} queues={} replies={} minted={}",
        parked.len(),
        channels.live_queues(graph, &transit),
        parked.recycled(),
        channels.minted(),
    );
    // Which tasks are still owed an answer, when any are (B28).
    //
    // A separate line rather than a field on the one above, because two gates
    // match that line by a prefix ending at `replies` and would stop matching if
    // it grew. Emitted only when the set is non-empty, so a healthy boot — every
    // one of the nine passing planes — gains no line at all and no fixture
    // moves.
    //
    // `parked=` already says *how many* replies the root still holds; it cannot
    // say *which*, which is the datum B28 needs. A task parked at teardown was
    // woken and never resumed, or never woken at all, and the two are
    // indistinguishable from a count.
    if !parked.is_empty() {
        sel4::debug_println!("SLIME_GRAPH replies owed count={}", parked.len());
        for task in parked.tasks() {
            sel4::debug_println!("SLIME_GRAPH reply owed task={}", task.0);
        }
    }
    // The loan plane's accounting, on its own line for the same reason: P5.3.1's
    // gate asserts the line above by its exact shape.
    //
    // The four zeros are what make reclamation observable. `loans`, `mappings`,
    // and `regions` are the shared-buffer table's own live counts, so a loan
    // whose lender died without settling, or a mapping a dead receiver still
    // held, shows up here rather than as memory quietly retained. `transit` is
    // capabilities in flight — one still parked at teardown is one no task can
    // ever name.
    //
    // `quotas` is appended last, for the reason `minted` is on the channel
    // line: the five gates that assert this marker match a prefix ending at
    // `aliases`. It is the count of ceilings still bound at teardown, and it is
    // not zero — the root-launched components' quotas are declared at boot and
    // released only when their tasks die, so a healthy boot ends with the
    // launched set still bound. What it makes visible is B24's shape: a graph
    // that spawns and reaps repeatedly must not leave this climbing.
    sel4::debug_println!(
        "SLIME_GRAPH loans served={loans_served} loans={} mappings={} regions={} transit={} orphans={} aliases={} quotas={}",
        buffers.loan_count(),
        buffers.mapping_count(),
        buffers.live_count(),
        transit.len(),
        buffers.orphan_count(),
        buffer_adapter::live_frame_aliases(),
        buffers.quota_count(),
    );
    // The spawn plane's accounting, on its own line for the same reason the two
    // above are: each earlier gate asserts its own line by exact shape.
    //
    // `waits=0` is the teardown property here: no task is still registered on a
    // child's termination, which would mean a wake that can never arrive.
    // `terminated` is deliberately *not* zero — it is one record per child that
    // ended, so a zero here on a boot that spawned would mean the supervision
    // path recorded nothing.
    //
    // Cumulative, not live: records are reclaimed once no holder can name them
    // (`supervision::sweep`), so the live count would read zero on any graph
    // that collected its outcomes, which is the healthy case rather than the
    // broken one. Below `MAX_RECORDS` the two are equal — a task terminates at
    // most once and `next_id` never reuses an id — so every gate written
    // against the live count reads the same number here.
    sel4::debug_println!(
        "SLIME_GRAPH spawns served={spawns} drops={drops} endpoints={endpoints} terminated={} waits={}",
        terminations.recorded(),
        supervision_waits.len(),
    );
    // The C8.3 transfer plane, on its own line for the same reason: five
    // earlier gates assert the lines above by exact shape.
    //
    // `transfers` is how many narrow-on-transfer moves crossed. It is zero on
    // every graph where each participant holds the edge the generation declared
    // it — which is every seL4 gate before P5.5.1 — and nonzero exactly when a
    // broker provisioned a role, so the number distinguishes a graph whose
    // authority was placed from one whose authority was handed on.
    sel4::debug_println!("SLIME_GRAPH transfers served={transfers}");
    sel4::debug_println!(
        "SLIME_ROOT allocator live_slots={} live_objects={} live_bytes={} slot_reuses={} arena_reuses={}",
        allocator.live_slots(),
        allocator.live_objects(),
        allocator.live_bytes(),
        allocator.slots_reused(),
        allocator.arena_reuses(),
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
///
/// Every failure is `BadCapability` — `ERR_BAD_CAP`, status -1 — matching
/// `kernel/src/syscall/mod.rs::{sys_send,sys_recv}`, which answer it for all
/// three cases alike: a slot holding nothing, a slot holding another kind, and
/// a slot holding an endpoint without the right the operation needs. Components
/// compare against the literal, so this is ABI rather than diagnostics:
/// `fabric-publisher` asserts `recv(route_slot) == ERR_BAD_CAP` to prove its
/// send-only role carries no receive authority, and answering anything else
/// reads to it as "the denial did not fire".
///
/// Indistinguishable on purpose, too. Which of the three it was is not the
/// caller's business, and separating them would let a component map its own
/// table — or infer another's authority — by watching which refusal came back.
fn resolve_channel(
    graph: &GraphTables,
    id: TaskId,
    slot: u32,
    rights: u64,
) -> Result<(ipc::ChannelKey, graph::Side), IpcError> {
    let table = graph.get(id).ok_or(IpcError::BadCapability)?;
    match table
        .resolve(slot, rights)
        .map_err(|_| IpcError::BadCapability)?
        .resource
    {
        graph::Resource::Endpoint { channel, side } => Ok((channel, side)),
        // A slot the task holds but that names something else — an executable,
        // a factory, a buffer — is refused exactly as an ungranted one is.
        _ => Err(IpcError::BadCapability),
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
    supervision_waits: &mut supervision::SupervisionWaits,
    id: TaskId,
    words: &[sel4::Word; ipc::FAST_MESSAGE_REGISTERS],
    served: &mut usize,
) -> Response {
    let (channel, side) = match resolve_channel(graph, id, words[0] as u32, RIGHT_SEND) {
        Ok(endpoint) => endpoint,
        Err(error) => return Response::error(error),
    };
    let frame = match transfer_window::read_staged(windows.bound(id), words[1], words, scratch) {
        Ok(frame) => frame,
        Err(error) => return Response::error(error),
    };
    // Capabilities ride to the same receiving end as the bytes. Since B25 an
    // end may have more than one holder, choosing a task here would predict the
    // winner of a future dequeue and can bind the capability to the wrong one.
    let receiving_side = side.opposite();
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
    let Some(queue) = channels.send_queue_mut(channel, side) else {
        return Response::error(IpcError::InvalidOperation);
    };
    // Preflight, capability move, and commit are one atomic step over a queue
    // whose revision has not moved, so a refused send enqueues nothing and
    // moves nothing.
    let mut departing = DepartingCaps {
        graph,
        transit,
        sender: id,
        channel,
        receiving_side,
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
            .send_queue(channel, side)
            .map_or(0, ipc::Channel::len),
    );
    if moved != 0 {
        sel4::debug_println!(
            "SLIME_GRAPH capability transfer task={} channel={channel} side={} caps={moved}",
            id.0,
            receiving_side.name(),
        );
    }
    // A receiver parked in `wait` on this queue is owed its wake now: nothing
    // else will make it retry. The message it collects is counted when its own
    // `recv` takes it, not here.
    if let Some(wake) = wake {
        deliver_wake(channels, parked, supervision_waits, wake);
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
    parked: &mut ParkedReplies,
    transit: &mut Transit,
    supervision_waits: &mut supervision::SupervisionWaits,
    scratch: &ScratchPage,
    id: TaskId,
    words: &[sel4::Word; ipc::FAST_MESSAGE_REGISTERS],
    served: &mut usize,
) -> Result<Response, ipc::ChannelKey> {
    let (channel, side) = match resolve_channel(graph, id, words[0] as u32, RIGHT_RECV) {
        Ok(endpoint) => endpoint,
        Err(error) => return Ok(Response::error(error)),
    };
    let Some(queue) = channels.recv_queue_mut(channel, side) else {
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
    // Dequeue is the transition that makes a full queue writable. The channel
    // has consumed its send-capacity waiter and returned the wake with the
    // message, so deliver it now; dropping it leaves a sender parked forever
    // even though this receive created room. This precedes capability landing
    // deliberately: the dequeue already committed, so a later landing failure
    // must not erase the capacity transition.
    if let Some(wake) = outcome.wake {
        deliver_wake(channels, parked, supervision_waits, wake);
    }
    // Land the capabilities before the reply is built, because the reply must
    // report the slots they landed at. A landing that fails is not silently
    // dropped: the message is already dequeued, so the capabilities are handed
    // back to the transit table's reclamation rather than lost — see
    // `land_caps`.
    let landed = match land_caps(graph, transit, id, channel, side, &outcome.message) {
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
/// Tokens are bound to the queue end this message was dequeued from. The task
/// that wins the dequeue receives both bytes and capabilities; no sender-side
/// task prediction participates.
fn land_caps(
    graph: &mut GraphTables,
    transit: &mut Transit,
    id: TaskId,
    channel: ipc::ChannelKey,
    side: graph::Side,
    message: &ipc::Message,
) -> Result<LandedCaps, IpcError> {
    let mut landed = LandedCaps::default();
    for token in message.caps().iter().flatten().copied() {
        let Some(capability) = transit.arrive(token, channel, side) else {
            // Authority may expire while the bytes remain readable. A token
            // collected from a different end is rejected by the same path.
            sel4::debug_println!(
                "SLIME_GRAPH capability expired task={} channel={channel} side={} bytes={}",
                id.0,
                side.name(),
                message.len(),
            );
            continue;
        };
        let outcome = graph
            .get_mut(id)
            .ok_or(IpcError::InvalidOperation)
            .and_then(|table| {
                // From 1, never 0: a received slot number is reported to the
                // receiver, and every protocol that carries them — the spawn
                // request's `received_caps` among them — reads 0 as "no
                // capability". Landing one there makes it invisible to the
                // component that was just given it.
                let slot = table
                    .free_slot_from(1)
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
                if transit.depart(capability, id, channel, side).is_err() {
                    sel4::debug_println!(
                        "SLIME_GRAPH FAIL capability lost task={} reason=transit-full",
                        id.0,
                    );
                }
                unland_caps(graph, transit, id, channel, side, &landed);
                return Err(error);
            }
        }
    }
    Ok(landed)
}

/// Take back every capability [`land_caps`] installed, re-parking each one.
///
/// No message names the new tokens, so they are deliberately unreachable; the
/// destination end is retained only so [`Transit::reclaim`] can drop them when
/// its last holder dies and terminal accounting still reaches zero.
fn unland_caps(
    graph: &mut GraphTables,
    transit: &mut Transit,
    id: TaskId,
    channel: ipc::ChannelKey,
    side: graph::Side,
    landed: &LandedCaps,
) {
    let Some(table) = graph.get_mut(id) else {
        return;
    };
    for slot in landed.slots() {
        if let Some(capability) = table.get(*slot) {
            table.drop_slot(*slot);
            if transit.depart(capability, id, channel, side).is_err() {
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
#[allow(clippy::too_many_arguments)]
fn serve_wait(
    channels: &mut ChannelTable,
    graph: &GraphTables,
    windows: &WindowTable<MAX_TASKS>,
    scratch: &ScratchPage,
    terminations: &supervision::Terminations,
    supervision_waits: &mut supervision::SupervisionWaits,
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

    // A supervision source is ready when its child has already ended. That is
    // read from the termination record rather than from any queue, so it is
    // tested here rather than inside `ChannelTable::is_ready`.
    //
    // A free function rather than a closure over `channels`: the registration
    // loop below needs `channels` mutably, and a closure capturing it would
    // hold a shared borrow across that loop.
    fn any_ready(
        targets: &[Option<WaitTarget>; ipc::MAX_WAIT_SOURCES],
        channels: &ChannelTable,
        terminations: &supervision::Terminations,
        _id: TaskId,
    ) -> bool {
        targets.iter().flatten().any(|target| match target {
            WaitTarget::Supervision(child) => terminations.get(*child).is_some(),
            other => channels.is_ready(*other),
        })
    }

    if any_ready(&targets, channels, terminations, id) {
        return Ok(true);
    }
    for target in targets.iter().flatten() {
        match target {
            WaitTarget::Supervision(child) => supervision_waits.register(id, *child),
            other => channels.register_wait(id, *other)?,
        }
    }
    // Probe again after registering: a send that landed between the first probe
    // and the registration would otherwise be a lost wakeup. A child cannot die
    // in that window — the root is single-threaded and a death is observed in
    // this same loop — but the same re-probe covers both, and asymmetry here
    // would be a rule to remember rather than one the code enforces.
    if any_ready(&targets, channels, terminations, id) {
        channels.clear_waits(id);
        supervision_waits.clear(id);
        return Ok(true);
    }
    Ok(false)
}

/// Answer every parked `wait` that named `child`, now that it has ended.
///
/// The termination record must already be written when this runs: the woken
/// task re-polls `supervision_status` immediately, and a wake that arrived
/// before the record would send it straight back to parking on a child that is
/// already gone.
/// Record how `child` ended, reclaiming unobservable records if the table is
/// full.
///
/// The sweep runs lazily, on full, rather than on every death: one trigger
/// condition is one thing to keep correct, and a sweep that does not run leaves
/// records that still answer correctly. See [`supervision::sweep`].
///
/// A record lost after the sweep means every slot has a live holder — a real
/// resource limit rather than a bookkeeping bug — but the consequence is still
/// a parent that waits forever, so it is reported rather than dropped silently.
/// Matches `unland_caps`'s `SLIME_GRAPH FAIL capability lost … reason=` line.
fn record_termination(
    terminations: &mut supervision::Terminations,
    graph: &GraphTables,
    transit: &Transit,
    child: TaskId,
    termination: supervision::Termination,
) {
    if terminations.record(child, termination) {
        return;
    }
    let freed = supervision::sweep(terminations, graph, transit);
    if terminations.record(child, termination) {
        sel4::debug_println!(
            "SLIME_GRAPH supervision swept freed={freed} live={}",
            terminations.len()
        );
        return;
    }
    sel4::debug_println!(
        "SLIME_GRAPH FAIL termination lost task={} reason=records-full",
        child.0
    );
}

fn wake_supervisors(
    parked: &mut ParkedReplies,
    supervision_waits: &mut supervision::SupervisionWaits,
    child: TaskId,
) {
    // Collected before answering, because clearing a registration mutates the
    // table the iterator borrows.
    //
    // Bounded by `MAX_TASKS` rather than by `MAX_WAITS`: registration is
    // idempotent per (waiter, child) pair, so one child has at most one waiter
    // per task. Sizing this by the registration table's own bound would put
    // eight times as much on a 256 KiB root stack for entries that cannot
    // exist — and an oversized stack temporary is exactly backlog B3.
    let mut woken = [None; MAX_TASKS];
    let mut count = 0;
    for waiter in supervision_waits.waiters_for(child) {
        if let Some(slot) = woken.get_mut(count) {
            *slot = Some(waiter);
            count += 1;
        }
    }
    for waiter in woken.iter().take(count).flatten().copied() {
        // Every registration this waiter holds, not just the one that fired:
        // a wait set is answered once, so its other sources must stop being
        // able to answer it a second time.
        supervision_waits.clear(waiter);
        if parked.reason(waiter) == Some(ParkReason::Wait)
            && parked.wake(waiter, Response::success(0, 0))
        {
            sel4::debug_println!(
                "SLIME_GRAPH supervision woken task={} child={}",
                waiter.0,
                child.0,
            );
        }
    }
}

/// Deliver one wake to whichever task is parked for it.
///
/// A wake naming a task that is not parked is not an error — its registration
/// outlived the `wait` that made it — and answers nothing.
///
/// Every park is a `wait` since P5.5.1, so a wake carries only the wake:
/// `slime_rt::wait` documents that the caller re-polls every source afterwards,
/// and that re-poll is the `recv` that takes the message. Before this, `recv`
/// parked too and this function had to *deliver* the message to it — dequeue,
/// land the capabilities, and write the payload into the woken task's window —
/// which was a second copy of `serve_recv`'s body reachable by two paths that
/// had to stay in step. Making `recv` non-blocking removed it.
fn deliver_wake(
    channels: &mut ChannelTable,
    parked: &mut ParkedReplies,
    supervision_waits: &mut supervision::SupervisionWaits,
    wake: ipc::WakeDecision,
) {
    let task = TaskId(wake.task);
    if parked.reason(task).is_none() {
        return;
    }
    // Both halves of the woken task's wait set, cleared together: a wait is
    // answered once, so a supervision source in the same set must stop being
    // able to answer it again.
    channels.clear_waits(task);
    supervision_waits.clear(task);
    parked.wake(task, Response::success(0, 0));
}

/// Bytes one encoded spawn-grant record occupies in the caller's transfer
/// window: a slot word, then a rights word.
///
/// Matches `components/runtime/src/syscall/sel4_transport.rs::GRANT_RECORD_BYTES`.
const SPAWN_GRANT_RECORD_BYTES: usize = 16;

/// Grants one spawn call may carry. **B15 is closed here.**
///
/// This is the retired kernel's bound, which it had not been until P5.5.1.
/// `sys_spawn` there reads the grant array straight out of caller memory,
/// limited only by `kernel/src/capability/mod.rs::MAX_CAPS` (64). Here the
/// array crosses the transfer window as a staged payload, and it used to be
/// read by `transfer_window::read_staged` — whose bound is
/// `ipc::MAX_MESSAGE_BYTES`, 64 *bytes*, or four records. Real x86 callers
/// already exceeded that: `init.rs::GENERATION_MANAGER_CAPS` and `dango_caps()`
/// are six grants each, `spawn-service.rs` builds up to five, and
/// `launch_fabric_graph` hands the fabric nine. Every one of them would have
/// been refused `ERR_INVALID_ARG` on the cutover where the oracle succeeds.
///
/// The fix is a second staged bound rather than a wider message:
/// [`transfer_window::MAX_STAGED_ARRAY_BYTES`] bounds an *array* staged through
/// a window, where `MAX_STAGED_BYTES` bounds a *message*. See that constant for
/// why the two must stay separate numbers. The component side needed no change
/// at all — `sel4_transport::spawn` already encoded into a
/// `MAX_SPAWN_GRANTS * GRANT_RECORD_BYTES` buffer and staged it into a
/// 4096-byte window; the refusal was entirely on this side.
const MAX_SPAWN_GRANTS: usize = transfer_window::MAX_STAGED_ARRAY_BYTES / SPAWN_GRANT_RECORD_BYTES;

// The two sides of the ABI agree on the ceiling. `sel4_transport::spawn`
// encodes into a fixed `MAX_SPAWN_GRANTS * GRANT_RECORD_BYTES` array of its
// own; a root that accepted fewer would refuse a list the component staged
// successfully, and one that accepted more would be describing a payload no
// caller can produce.
const _: () = assert!(MAX_SPAWN_GRANTS == 64);

/// One requested grant, as the caller encoded it.
#[derive(Clone, Copy)]
struct SpawnGrant {
    /// The slot in the *caller's* table naming the capability to copy.
    slot: u32,
    /// The rights the copy carries. A subset of what the caller holds.
    rights: u64,
}

/// The capabilities a spawn will install in the child, derived and validated
/// before anything is constructed.
///
/// Derived first, installed later, and deliberately in that order: a grant list
/// naming a capability the caller does not hold must refuse the whole spawn
/// with nothing allocated, rather than leave a half-built child behind. This is
/// `kernel/src/task/mod.rs::preflight_spawn_grant`'s shape, and the reason is
/// the same one `serve_buffer_create` learned the hard way — a failure after
/// allocation is a leak unless every step before it was checked.
struct SpawnPlan {
    /// The generation executable index and declared instance to construct.
    executable: usize,
    instance: usize,
    /// Derived capabilities paired with their caller source slot, explicit
    /// child-local destination slot, and whether the declaration authorizing
    /// them is a minted binding. Spawn never consumes the parent slot.
    ///
    /// A generation-declared channel end is re-installed after construction
    /// from `ChannelTable`'s single pre-created edge, so copying the parent's
    /// end here would install the wrong side. A *minted* endpoint has no such
    /// declared edge — its object exists only because the parent created it at
    /// runtime — so the copy is the only way it reaches the child.
    granted: [Option<(u32, u32, graph::Capability, bool)>; MAX_SPAWN_GRANTS],
    count: usize,
    /// Whether the executable capability carried `RIGHT_TRANSFER`, which is
    /// what decides if the supervision handle the parent receives may itself be
    /// passed on. Read from the executable rather than from any grant, matching
    /// `spawn_from_cap`'s `transferable_supervision`.
    transferable_supervision: bool,
}

/// What a layout line names a resource by (B10).
///
/// An executable carries its component name, matching the oracle's
/// `dump_boot_layout`. Everything else is identified by kind and rights alone,
/// because that is all a layout needs to be comparable: a channel end's *key*
/// is an allocation detail that differs between two correct boots, so printing
/// it would make the fixture record noise rather than shape.
fn resource_label<'a>(generation: &Generation<'a>, resource: &graph::Resource) -> &'a str {
    match resource {
        graph::Resource::Executable { executable } => generation
            .executable(*executable)
            .map_or("?", |record| record.name),
        _ => "-",
    }
}

/// The rights a resource kind can meaningfully carry.
///
/// `kernel/src/capability/mod.rs::KernelObject::valid_rights`, restated against
/// this crate's resource enum. A shared buffer is the one open case: its rights
/// are minted by `serve_buffer_create` as `RIGHT_BUFFER_ALL` and narrowed per
/// loan, so enumerating them here would duplicate the shared-buffer table's own
/// vocabulary rather than describe the object.
const fn valid_rights(resource: &graph::Resource) -> u64 {
    match resource {
        graph::Resource::Executable { .. } => RIGHT_EXEC | RIGHT_SPAWN | RIGHT_TRANSFER,
        graph::Resource::Endpoint { .. } => RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        graph::Resource::EndpointFactory => RIGHT_ENDPOINT_CREATE | RIGHT_TRANSFER,
        graph::Resource::SharedBufferFactory => RIGHT_BUFFER_CREATE | RIGHT_TRANSFER,
        graph::Resource::Supervision { .. } => RIGHT_SUPERVISE | RIGHT_TRANSFER,
        graph::Resource::SharedBuffer { .. } | graph::Resource::Loan { .. } => RIGHT_BUFFER_ALL,
        // No `RIGHT_TRANSFER`: the device is singular and its authority is
        // placed by the generation, so there is no composition that would need
        // to hand it on. Adding the bit later is a deliberate act.
        graph::Resource::Block { .. } => RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE,
        // `RIGHT_TRANSFER` *is* allowed here, unlike the block device: M6.3
        // requires narrow-only directory derivation and transfer, and a
        // powerbox handing a requester one narrowed view is the whole point of
        // the resource. `serve_directory_derive` refuses to add the bit to a
        // capability that does not already carry it, so the delegation cannot
        // be manufactured by the holder.
        graph::Resource::Directory { .. } => RIGHTS_DIRECTORY_ALL | RIGHT_TRANSFER,
        // No `RIGHT_TRANSFER`: the source is singular and its authority is
        // placed by the generation, so nothing needs to hand it on.
        graph::Resource::Input => RIGHT_INPUT_READ,
    }
}

/// One capability the generation declares a child receives at spawn.
///
/// A grant-backed binding names a concrete authority edge; a minted binding
/// names the same edge with the object identity deferred to its minter. Both
/// fix the destination slot and the rights ceiling before activation.
enum DeclaredCapability<'a> {
    Granted(usize, Grant<'a>),
    Minted(MintedBinding<'a>),
}

impl DeclaredCapability<'_> {
    const fn slot(&self) -> usize {
        match self {
            Self::Granted(slot, _) => *slot,
            Self::Minted(minted) => minted.slot,
        }
    }
}

/// The child's `index`-th declared capability in ascending destination-slot
/// order, across grant-backed and minted declarations together.
///
/// Declaration *kind* is invisible to a spawning component: it lists grants in
/// the order its child's slots run. Ordering by slot here is what lets a caller
/// interleave the two without knowing which is which.
fn nth_declared_capability<'a>(
    generation: &Generation<'a>,
    child: Instance<'a>,
    child_instance: usize,
    index: usize,
) -> Result<DeclaredCapability<'a>, IpcError> {
    let mut selected: Option<DeclaredCapability<'a>> = None;
    let mut each = |candidate: DeclaredCapability<'a>| -> Result<(), IpcError> {
        // The candidate's rank is how many declarations sit below it, so the
        // one wanted here is the candidate with exactly `index` below it. Slots
        // are unique per child — the decoder rejects duplicates in both
        // sections — so the rank is total and exactly one candidate matches.
        let below = declarations_below(generation, child, child_instance, candidate.slot())?;
        if below == index {
            claim_rank(candidate, &mut selected)?;
        }
        Ok(())
    };
    for at in 0..child.binding_count() {
        let binding = generation
            .binding(child, at)
            .map_err(|_| IpcError::BadCapability)?;
        let declared = generation
            .grant(binding.grant)
            .map_err(|_| IpcError::BadCapability)?;
        each(DeclaredCapability::Granted(binding.slot, declared))?;
    }
    for at in 0..generation.minted_binding_count() {
        let minted = generation
            .minted_binding(at)
            .map_err(|_| IpcError::BadCapability)?;
        if minted.holder != child_instance {
            continue;
        }
        each(DeclaredCapability::Minted(minted))?;
    }
    selected.ok_or(IpcError::BadCapability)
}

/// How many of the child's declared capabilities land below `slot`.
fn declarations_below(
    generation: &Generation<'_>,
    child: Instance<'_>,
    child_instance: usize,
    slot: usize,
) -> Result<usize, IpcError> {
    let mut below = 0;
    for at in 0..child.binding_count() {
        let binding = generation
            .binding(child, at)
            .map_err(|_| IpcError::BadCapability)?;
        below += usize::from(binding.slot < slot);
    }
    for at in 0..generation.minted_binding_count() {
        let minted = generation
            .minted_binding(at)
            .map_err(|_| IpcError::BadCapability)?;
        below += usize::from(minted.holder == child_instance && minted.slot < slot);
    }
    Ok(below)
}

/// Record `candidate` as the declaration for its rank, refusing a second one.
///
/// Ranks are a bijection: the decoder rejects two declarations claiming one
/// holder slot, in either section or across them, so exactly one candidate can
/// have any given count of declarations below it. A collision here would mean
/// the decoder admitted a generation it should not have, so it fails the spawn
/// rather than silently picking a winner.
fn claim_rank<'a>(
    candidate: DeclaredCapability<'a>,
    selected: &mut Option<DeclaredCapability<'a>>,
) -> Result<(), IpcError> {
    if selected.replace(candidate).is_some() {
        return Err(IpcError::BadCapability);
    }
    Ok(())
}

/// Decode and validate a spawn against its declared child instance.
///
/// The executable capability chooses an executable catalogue entry, but it is
/// not itself an instance declaration. The caller's declared instance and that
/// executable must name exactly one child instance through `owner`. That child
/// fixes both the complete capability set and every child-local slot, so
/// request order cannot change layout: each capability is installed at the slot
/// its declaration names.
///
/// Each requested capability remains a narrowing copy of authority the caller
/// holds, and its rights must be covered by the declaration's ceiling. Missing,
/// extra, duplicate, or unrelated records refuse the whole spawn before
/// allocation.
fn preflight_spawn_grants(
    generation: &Generation<'_>,
    caller_instance: usize,
    table: &graph::CapabilityTable,
    executable_slot: u32,
    records: &[u8],
) -> Result<SpawnPlan, IpcError> {
    let Some(executable) = table.get(executable_slot) else {
        return Err(IpcError::BadCapability);
    };
    let graph::Resource::Executable {
        executable: executable_index,
    } = executable.resource
    else {
        return Err(IpcError::BadCapability);
    };
    if !executable.allows(RIGHT_EXEC | RIGHT_SPAWN) {
        return Err(IpcError::BadCapability);
    }
    let mut child_instance = None;
    for index in 0..generation.instance_count() {
        let instance = generation
            .instance(index)
            .map_err(|_| IpcError::BadCapability)?;
        if instance.owner == InstanceOwner::Instance(caller_instance)
            && instance.executable == executable_index
            && child_instance.replace(index).is_some()
        {
            return Err(IpcError::BadCapability);
        }
    }
    let child_instance = child_instance.ok_or(IpcError::BadCapability)?;
    let child = generation
        .instance(child_instance)
        .map_err(|_| IpcError::BadCapability)?;
    if !records.len().is_multiple_of(SPAWN_GRANT_RECORD_BYTES) {
        return Err(IpcError::InvalidLength);
    }
    let count = records.len() / SPAWN_GRANT_RECORD_BYTES;
    if count > MAX_SPAWN_GRANTS {
        return Err(IpcError::InvalidLength);
    }
    // The child's declared capability set is its grant-backed bindings plus the
    // capabilities its owner mints at runtime. Both are generation-declared:
    // a binding names a concrete authority edge, a minted binding names the
    // same edge with the object deferred to its minter.
    let minted_count = (0..generation.minted_binding_count())
        .filter(|index| {
            generation
                .minted_binding(*index)
                .is_ok_and(|minted| minted.holder == child_instance)
        })
        .count();
    // A self-loop grant — source and target both this instance — declares
    // authority the child holds in its own right, not a capability handed
    // across a spawn boundary. The root installs it directly, so the parent
    // neither passes it nor could: it does not hold it.
    let mut parent_supplied = 0;
    for index in 0..child.binding_count() {
        let Ok(binding) = generation.binding(child, index) else {
            return Err(IpcError::BadCapability);
        };
        let Ok(grant) = generation.grant(binding.grant) else {
            return Err(IpcError::BadCapability);
        };
        if grant.source != GrantEndpoint::Instance(child_instance)
            || grant.target != GrantEndpoint::Instance(child_instance)
        {
            parent_supplied += 1;
        }
    }
    if count != parent_supplied + minted_count {
        sel4::debug_println!(
            "SLIME_GRAPH spawn preflight instance={} reason=declared-count requested={count} bindings={parent_supplied} minted={minted_count}",
            child.name,
        );
        return Err(IpcError::BadCapability);
    }

    let mut requested = [None; MAX_SPAWN_GRANTS];
    for (destination, record) in requested
        .iter_mut()
        .zip(records.chunks_exact(SPAWN_GRANT_RECORD_BYTES))
    {
        let slot = u64::from_le_bytes(
            record[..8]
                .try_into()
                .map_err(|_| IpcError::InvalidLength)?,
        );
        let rights = u64::from_le_bytes(
            record[8..]
                .try_into()
                .map_err(|_| IpcError::InvalidLength)?,
        );
        let slot = u32::try_from(slot).map_err(|_| IpcError::BadCapability)?;
        *destination = Some(SpawnGrant { slot, rights });
    }

    let mut granted = [None; MAX_SPAWN_GRANTS];
    for index in 0..count {
        let request = requested[index].ok_or(IpcError::InvalidLength)?;
        if request.slot == executable_slot {
            return Err(IpcError::BadCapability);
        }
        if requested[..index]
            .iter()
            .flatten()
            .any(|seen| seen.slot == request.slot)
        {
            return Err(IpcError::BadCapability);
        }
        // Each request resolves to exactly one generation declaration, matched
        // in ascending destination-slot order across both kinds together. A
        // spawn grant array is positional and the child's capability table is
        // addressed by slot, so the caller's Nth grant is the declaration with
        // the Nth-lowest destination slot — whether that is a grant-backed
        // binding or an owner-minted one. Splitting the two kinds into separate
        // positional runs would force a caller to order its array by
        // declaration kind, which no component knows about.
        let declaration = nth_declared_capability(generation, child, child_instance, index)?;
        let minted_declaration = matches!(declaration, DeclaredCapability::Minted(_));
        let (destination, ceiling, label) = match declaration {
            DeclaredCapability::Granted(binding_slot, declared) => {
                // The grant must be one the *child* legitimately carries. Its
                // binding list is generation-declared, and `child_instance` was
                // resolved as an instance this caller owns, so provenance holds
                // without requiring the spawner to hold the grant too.
                // Requiring that would force init to retain route authority
                // over every channel it mints and hands on, which is exactly
                // the property the fabric planes deny it.
                if !generation.grant_applies_to_instance(declared, child_instance) {
                    sel4::debug_println!(
                        "SLIME_GRAPH spawn preflight binding={} index={index} reason=child-provenance",
                        declared.name,
                    );
                    return Err(IpcError::BadCapability);
                }
                (binding_slot, declared.rights, declared.name)
            }
            DeclaredCapability::Minted(minted) => {
                // A minted capability's object identity is deferred to its
                // minter, so only that minter may hand it over. Everything else
                // about the edge — holder, slot, rights ceiling — was fixed
                // before activation.
                if minted.owner != caller_instance {
                    sel4::debug_println!(
                        "SLIME_GRAPH spawn preflight minted={} index={index} reason=not-minter",
                        minted.name,
                    );
                    return Err(IpcError::BadCapability);
                }
                (minted.slot, minted.rights, minted.name)
            }
        };
        // A request carrying no rights would install an inert capability into
        // the slot the declaration reserved, consuming it without conveying
        // authority. A declaration always names a nonzero ceiling, so an empty
        // request is a malformed spawn rather than a narrowing.
        if request.rights == 0 || request.rights & !ceiling != 0 {
            sel4::debug_println!(
                "SLIME_GRAPH spawn preflight binding={label} index={index} reason=declared-rights requested={:#x} declared={:#x}",
                request.rights,
                ceiling,
            );
            return Err(IpcError::BadCapability);
        }
        let Some(held) = table.get(request.slot) else {
            sel4::debug_println!(
                "SLIME_GRAPH spawn preflight binding={label} index={index} reason=source-empty slot={}",
                request.slot,
            );
            return Err(IpcError::BadCapability);
        };
        if !held.allows(request.rights) || request.rights & !valid_rights(&held.resource) != 0 {
            sel4::debug_println!(
                "SLIME_GRAPH spawn preflight binding={label} index={index} reason=held-rights slot={} requested={:#x} held={:#x} valid={:#x}",
                request.slot,
                request.rights,
                held.rights,
                valid_rights(&held.resource),
            );
            return Err(IpcError::BadCapability);
        }
        let destination = u32::try_from(destination).map_err(|_| IpcError::BadCapability)?;
        granted[index] = Some((
            request.slot,
            destination,
            graph::Capability {
                resource: held.resource,
                rights: request.rights,
            },
            minted_declaration,
        ));
    }

    Ok(SpawnPlan {
        executable: executable_index,
        instance: child_instance,
        granted,
        count,
        transferable_supervision: executable.allows(RIGHT_TRANSFER),
    })
}

/// Construct the child a validated plan names, and install both tables.
///
/// Ordering is the whole safety argument, and it is the one
/// `launch_component_graph` already uses for the boot graph: nothing is
/// allocated until every check has passed, and the two failure points that
/// remain after allocation each tear down what they made.
///
/// The child's slot numbering comes only from its declared instance bindings.
/// Request order is merely the canonical binding-slice traversal used to pair
/// caller capabilities with declarations; it never becomes a destination slot.
/// first spawn grant.
#[allow(clippy::too_many_arguments)]
fn construct_child(
    generation: &Generation<'_>,
    tasks: &mut TaskTable<MAX_TASKS>,
    windows: &mut WindowTable<MAX_TASKS>,
    graph: &mut GraphTables,
    buffers: &mut SharedBufferTable,
    allocator: &mut ObjectAllocator,
    scratch: &ScratchPage,
    service_endpoint: sel4::cap::Endpoint,
    console_endpoint: sel4::cap::Endpoint,
    parent: TaskId,
    plan: &SpawnPlan,
) -> Result<TaskId, IpcError> {
    let record = generation
        .executable(plan.executable)
        .map_err(|_| IpcError::BadCapability)?;
    let instance = generation
        .instance(plan.instance)
        .map_err(|_| IpcError::BadCapability)?;
    let object = generation
        .object(record.object)
        .map_err(|_| IpcError::BadCapability)?;
    let profile = boot_contracts::target_profile::TargetProfile::by_name(TARGET_PROFILE)
        .map_err(|_| IpcError::BadCapability)?;
    let elf = boot_contracts::component_image::admit_elf(object.bytes, profile)
        .map_err(|_| IpcError::BadCapability)?;

    // SAFETY: the root task is single-threaded and this is the only reference
    // taken to `ELF_SCRATCH`. It is released before this function returns.
    let aligned = unsafe { &mut *ptr::addr_of_mut!(ELF_SCRATCH) };
    let elf = aligned.hold(elf).map_err(|_| IpcError::InvalidLength)?;
    let image = ChildImage::parse(elf).map_err(|_| IpcError::BadCapability)?;
    let authority = bound_authority(generation, instance).map_err(|_| IpcError::BadCapability)?;

    let id = tasks
        .create(
            allocator,
            &image,
            service_endpoint,
            console_endpoint,
            authority,
            Supervision::SelfManaged,
            sel4::init_thread::slot::VSPACE.cap(),
            scratch,
            sel4::init_thread::slot::ASID_POOL.cap(),
            Some(parent),
            Some(plan.executable),
            Some(plan.instance),
            // A dynamically spawned child is never the bootstrap instance.
            0,
            // A spawned child is a declared instance too, so its CSpace comes
            // from the same plan the boot graph reads.
            generation
                .instance_cspace_size_bits(plan.instance)
                .map_err(|_| IpcError::BadCapability)?
                .ok_or(IpcError::BadCapability)? as usize,
            match generation.instance_child_slots(plan.instance) {
                Ok(Some(boot_contracts::generation::ChildSlotPlan {
                    service: Some(service),
                    console: Some(console),
                    tcb: Some(tcb),
                    fault: Some(fault),
                })) => (task::ChildSlots {
                    service: service as sel4::CPtrBits,
                    console: console as sel4::CPtrBits,
                    tcb: tcb as sel4::CPtrBits,
                    fault: fault as sel4::CPtrBits,
                })
                .validate()
                .map_err(|_| IpcError::BadCapability)?,
                _ => return Err(IpcError::BadCapability),
            },
        )
        .map_err(|_| IpcError::DestinationSlotsExhausted)?;

    let Some(task) = tasks.get(id) else {
        release_child(tasks, windows, graph, buffers, allocator, id);
        return Err(IpcError::DestinationSlotsExhausted);
    };
    let (window_addr, window, window_alias) = (
        task.vspace.transfer_window_addr,
        task.vspace.transfer_window,
        task.vspace.transfer_window_alias,
    );
    if windows
        .declare(id, window_addr, window, window_alias)
        .is_err()
    {
        release_child(tasks, windows, graph, buffers, allocator, id);
        return Err(IpcError::DestinationSlotsExhausted);
    }

    let quota = declared_quota(shared_buffer_budget(generation).as_ref(), instance.name);
    if buffers
        .declare_quota(HolderId(u64::from(id.0)), quota)
        .is_err()
    {
        release_child(tasks, windows, graph, buffers, allocator, id);
        return Err(IpcError::DestinationSlotsExhausted);
    }
    sel4::debug_println!(
        "SLIME_GRAPH quota task={} instance={} executable={} pages={} buffers={} mappings={} loans={}",
        id.0,
        instance.name,
        record.name,
        quota.byte_pages,
        quota.buffer_count,
        quota.mapping_count,
        quota.loan_count,
    );
    let Ok(child_table) = graph.create(id) else {
        release_child(tasks, windows, graph, buffers, allocator, id);
        return Err(IpcError::DestinationSlotsExhausted);
    };
    for granted in plan.granted.iter().take(plan.count) {
        let Some((_, destination, capability, minted)) = granted else {
            continue;
        };
        // A generation-declared channel binding is installed from
        // ChannelTable's single pre-created edge after construction. The
        // request record still validates the parent's held authority in
        // preflight, but copying that parent end here would install the wrong
        // side and collide with the child's explicit binding slot.
        //
        // A minted endpoint has no pre-created edge — the parent created the
        // object at runtime, which is exactly what its declaration defers — so
        // for those the copy is the only way the capability reaches the child.
        if !minted && matches!(capability.resource, graph::Resource::Endpoint { .. }) {
            continue;
        }
        if child_table.install(*destination, *capability).is_err() {
            release_child(tasks, windows, graph, buffers, allocator, id);
            return Err(IpcError::DestinationSlotsExhausted);
        }
    }
    // A self-loop grant declares authority the child holds in its own right,
    // so no parent passes it and the loop above never sees it. The root is
    // the only party that can install it, and preflight has already excluded
    // these from the count the parent must satisfy.
    let Ok(child) = generation.instance(plan.instance) else {
        release_child(tasks, windows, graph, buffers, allocator, id);
        return Err(IpcError::BadCapability);
    };
    for index in 0..child.binding_count() {
        let Ok(binding) = generation.binding(child, index) else {
            release_child(tasks, windows, graph, buffers, allocator, id);
            return Err(IpcError::BadCapability);
        };
        let Ok(grant) = generation.grant(binding.grant) else {
            release_child(tasks, windows, graph, buffers, allocator, id);
            return Err(IpcError::BadCapability);
        };
        if grant.source != GrantEndpoint::Instance(plan.instance)
            || grant.target != GrantEndpoint::Instance(plan.instance)
        {
            continue;
        }
        // Same construction the boot path uses for a declared binding.
        let Some((_, resource)) = declared_resource(grant.rights) else {
            continue;
        };
        let capability = graph::Capability {
            resource,
            rights: grant.rights,
        };
        if child_table
            .install(binding.slot as u32, capability)
            .is_err()
        {
            release_child(tasks, windows, graph, buffers, allocator, id);
            return Err(IpcError::DestinationSlotsExhausted);
        }
        // The evidence that a child's own declared authority reached it. Only
        // the root can place these — the parent holds no copy — so this is the
        // only point at which it is observable.
        sel4::debug_println!(
            "SLIME_GRAPH declared placed task={} child={} slot={} kind={}",
            parent.0,
            id.0,
            binding.slot,
            resource.kind_name(),
        );
    }
    Ok(id)
}

/// Mint a channel pair, sweeping reclaimable channels first if the table is
/// full.
///
/// Backlog **B22**'s fix, lazily on full. Dispatch is single-threaded and a
/// spawned endpoint is an ordinary copied capability since B25, so there is no
/// multi-step holder transition for a sweep to interleave with.
fn mint_channel(
    channels: &mut ChannelTable,
    graph: &GraphTables,
    transit: &Transit,
    _id: TaskId,
) -> Result<u32, channel::ChannelError> {
    match channels.mint() {
        Ok(key) => Ok(key),
        Err(channel::ChannelError::TableFull) => {
            let freed = channel::sweep(channels, graph, transit);
            sel4::debug_println!(
                "SLIME_GRAPH channels swept freed={freed} live={} minted={}",
                channels.len(),
                channels.minted(),
            );
            channels.mint()
        }
        Err(error) => Err(error),
    }
}

/// Tear a partially constructed child back down.
///
/// This is the single unwind owner after `TaskTable::create` succeeds. Each
/// resource release is idempotent, so every later failure can call it without
/// tracking which construction stages completed. Quota is released before the
/// task identity becomes unreachable, and the task's object span is revoked
/// last.
fn release_child(
    tasks: &mut TaskTable<MAX_TASKS>,
    windows: &mut WindowTable<MAX_TASKS>,
    graph: &mut GraphTables,
    buffers: &mut SharedBufferTable,
    allocator: &mut ObjectAllocator,
    id: TaskId,
) {
    graph.release(id);
    windows.release(id);
    buffers.release_quota(HolderId(u64::from(id.0)));
    match tasks.reclaim(allocator, id) {
        Ok(cleanup) => sel4::debug_println!(
            "SLIME_GRAPH spawn unwound task={} slots={} arena={}",
            id.0,
            cleanup.slot_count(),
            cleanup.arena.index(),
        ),
        Err(error) => sel4::debug_println!(
            "SLIME_GRAPH spawn unwind incomplete task={} error={error:?}",
            id.0
        ),
    }
}

/// The live-child budget the generation declares for the component `task` is.
///
/// Zero when the task is not a launched component, or when the generation
/// declares no budget for it — deny by default, exactly as an absent
/// shared-buffer holder resolves to `HolderQuota::DENY`. A component the
/// manifest gives no budget spawns nothing.
fn spawner_budget(
    generation: &Generation<'_>,
    _launched: &LaunchedInstances,
    tasks: &TaskTable<MAX_TASKS>,
    task: TaskId,
) -> usize {
    tasks
        .get(task)
        .and_then(|task| task.executable)
        .and_then(|executable| generation.executable(executable).ok())
        .map_or(0, |record| usize::from(record.spawn_budget))
}

/// Serve one `spawn`: validate, construct, activate, and hand the parent a
/// supervision handle.
#[allow(clippy::too_many_arguments)]
fn serve_spawn(
    generation: &Generation<'_>,
    launched: &mut LaunchedInstances,
    tasks: &mut TaskTable<MAX_TASKS>,
    windows: &mut WindowTable<MAX_TASKS>,
    graph: &mut GraphTables,
    channels: &mut ChannelTable,
    buffers: &mut SharedBufferTable,
    allocator: &mut ObjectAllocator,
    scratch: &ScratchPage,
    service_endpoint: sel4::cap::Endpoint,
    console_endpoint: sel4::cap::Endpoint,
    id: TaskId,
    words: &[sel4::Word; ipc::FAST_MESSAGE_REGISTERS],
    spawns: &mut usize,
) -> Response {
    let executable_slot = words[0] as u32;
    // The wide reader (B15), because a grant array is not a message: at
    // `SPAWN_GRANT_RECORD_BYTES` each, the message bound admitted four records
    // where the oracle admits sixty-four. It refuses a descriptor naming any
    // capability itself — grants are logical slot numbers in the payload, and a
    // spawn carrying real seL4 capabilities is refused by `recv_request` before
    // reaching here.
    //
    // An empty grant list stages nothing, so a spawn granting no capabilities
    // still does not require a bound window: a zero-length transfer reports the
    // empty array, and `preflight_spawn_grants` reads zero records out of it.
    let frame =
        match transfer_window::read_staged_array(windows.bound(id), words[1], words, scratch) {
            Ok(frame) => frame,
            Err(error) => return Response::error(error),
        };

    let Some(table) = graph.get(id) else {
        return Response::error(IpcError::InvalidOperation);
    };
    let Some(caller_instance) = tasks.get(id).and_then(|task| task.instance) else {
        sel4::debug_println!(
            "SLIME_GRAPH spawn refused task={} slot={executable_slot} undeclared-instance",
            id.0,
        );
        return Response::error(IpcError::BadCapability);
    };
    let plan = match preflight_spawn_grants(
        generation,
        caller_instance,
        table,
        executable_slot,
        frame.bytes(),
    ) {
        Ok(plan) => plan,
        Err(error) => {
            sel4::debug_println!(
                "SLIME_GRAPH spawn refused task={} slot={executable_slot} ungranted",
                id.0,
            );
            return Response::error(error);
        }
    };
    let name = generation
        .instance(plan.instance)
        .map_or("<unknown>", |record| record.name);
    // `DestinationSlotsExhausted`, whose status is -5 — `ERR_OUT_OF_MEMORY`,
    if launched.task_for_instance(plan.instance).is_some() {
        sel4::debug_println!(
            "SLIME_GRAPH spawn refused task={} child={name} class=instance-live",
            id.0,
        );
        return Response::error(IpcError::BadCapability);
    }
    // matching `sys_spawn`, which maps `BudgetExhausted` and
    // `TooManyTasks` alike to `ERR_OUT_OF_MEMORY` and everything else to
    // `ERR_BAD_CAP`. That distinction is the caller's business here in a way the
    // preflight refusals are not: a component that has hit its ceiling learns
    // something true about itself and can wait for a child to exit, whereas a
    // component naming an ungranted slot learns nothing about its table.
    let budget = spawner_budget(generation, launched, tasks, id);
    let live = tasks.live_children(id);
    if live >= budget {
        sel4::debug_println!(
            // `child=` rather than `component=`: the budget is the *caller's*,
            // and naming the child's component beside it read as though the
            // ceiling belonged to the thing being refused.
            "SLIME_GRAPH spawn refused task={} child={name} class=budget live={live} budget={budget}",
            id.0,
        );
        return Response::error(IpcError::DestinationSlotsExhausted);
    }

    sel4::debug_println!(
        "SLIME_GRAPH spawn authorized task={} slot={executable_slot} component={name} grants={}",
        id.0,
        plan.count,
    );

    let child = match construct_child(
        generation,
        tasks,
        windows,
        graph,
        buffers,
        allocator,
        scratch,
        service_endpoint,
        console_endpoint,
        id,
        &plan,
    ) {
        Ok(child) => child,
        Err(error) => {
            sel4::debug_println!(
                "SLIME_GRAPH spawn failed task={} component={name} error={error:?}",
                id.0,
            );
            return Response::error(error);
        }
    };
    let installed_channels = match channels.install_instance(
        generation,
        plan.instance,
        child,
        graph,
    ) {
        Ok(installed) => installed,
        Err(error) => {
            release_child(tasks, windows, graph, buffers, allocator, child);
            sel4::debug_println!(
                "SLIME_GRAPH spawn failed task={} component={name} error=ChannelInstall({error:?})",
                id.0,
            );
            return Response::error(IpcError::BadCapability);
        }
    };

    // Declared channel ends come exclusively from the pre-created generation
    // catalogue. Request endpoint records authorize the delegation but do not
    // copy the parent's complementary side into the child.
    let copied = installed_channels;

    // The parent's handle, installed before the child runs. A child that exited
    // before its parent held a handle would leave the parent waiting on a task
    // it can never learn the fate of, so the ordering is load-bearing rather
    // than tidy.
    let handle = graph.get_mut(id).and_then(|table| {
        let slot = table.free_slot_from(1)?;
        table
            .install(
                slot,
                graph::Capability {
                    resource: graph::Resource::Supervision { task: child },
                    rights: RIGHT_SUPERVISE
                        | if plan.transferable_supervision {
                            RIGHT_TRANSFER
                        } else {
                            0
                        },
                },
            )
            .ok()?;
        Some(slot)
    });
    let Some(handle) = handle else {
        // The parent's table is full. The copied child table can simply be
        // released; the parent's grants were never consumed.
        release_child(tasks, windows, graph, buffers, allocator, child);
        sel4::debug_println!(
            "SLIME_GRAPH spawn failed task={} component={name} error=NoHandleSlot",
            id.0,
        );
        return Response::error(IpcError::DestinationSlotsExhausted);
    };

    if tasks.activate(child).is_err() {
        if let Some(table) = graph.get_mut(id) {
            table.drop_slot(handle);
        }
        release_child(tasks, windows, graph, buffers, allocator, child);
        sel4::debug_println!(
            "SLIME_GRAPH spawn failed task={} component={name} error=Activate",
            id.0,
        );
        return Response::error(IpcError::DestinationSlotsExhausted);
    }
    if launched
        .record(plan.instance, plan.executable, child)
        .is_err()
    {
        if let Some(table) = graph.get_mut(id) {
            table.drop_slot(handle);
        }
        release_child(tasks, windows, graph, buffers, allocator, child);
        return Response::error(IpcError::BadCapability);
    }
    *spawns += 1;
    sel4::debug_println!(
        "SLIME_GRAPH spawned task={} child={} component={name} grants={} channels={copied} handle={handle}",
        id.0,
        child.0,
        plan.count,
    );
    Response::success(i64::from(child.0), handle as sel4::Word)
}

/// Answer `supervision_status` for one handle.
///
/// `Ok(None)` — reported as `ERR_WOULDBLOCK` — means the child is still live.
/// A terminated child's outcome consumes the caller's handle slot, matching
/// `kernel/src/task/mod.rs::supervision_status`, so an outcome is collected
/// exactly once by each holder.
fn serve_supervision_status(
    graph: &mut GraphTables,
    terminations: &supervision::Terminations,
    id: TaskId,
    words: &[sel4::Word; ipc::FAST_MESSAGE_REGISTERS],
) -> Response {
    let slot = words[0] as u32;
    // `RIGHT_SUPERVISE` on the handle, not merely possession of it: a
    // supervision capability narrowed past that right names a child the holder
    // may pass on but not query, which is a distinction the retired kernel
    // makes and this one keeps.
    let Ok(graph::Capability {
        resource: graph::Resource::Supervision { task },
        ..
    }) = graph
        .get(id)
        .ok_or(IpcError::InvalidOperation)
        .and_then(|table| table.resolve(slot, RIGHT_SUPERVISE))
    else {
        return Response::error(IpcError::BadCapability);
    };
    let Some(termination) = terminations.get(task) else {
        return Response::error(IpcError::WouldBlock);
    };
    if let Some(table) = graph.get_mut(id) {
        table.drop_slot(slot);
    }
    let (kind, detail) = termination.encode();
    sel4::debug_println!(
        "SLIME_GRAPH supervision collected task={} child={} kind={kind}",
        id.0,
        task.0,
    );
    Response::success(kind, detail)
}

/// Install a second capability naming a task the caller already supervises (B25).
///
/// The oracle has no counterpart: on x86 a spawn grant *copies*, so a parent can
/// hand the same handle to any number of children by granting it at each spawn.
/// On seL4 a grant must run before the child exists and `cap_transfer` moves, so
/// a parent that must introduce one child to two later siblings has no route —
/// which is B25's blocking half.
///
/// Widens nothing, and that is the whole argument for adding an operation rather
/// than changing the move/copy semantics of an existing one:
///
/// * the result names the *same* task, so no new subject becomes reachable;
/// * its rights are the source's own, so no new verb becomes permitted;
/// * `RIGHT_SUPERVISE` on the source is required to ask, the same gate
///   `serve_supervision_status` puts in front of a query.
///
/// So a caller can only ever mint a handle it could already have transferred.
/// `Terminations` is unaffected: `graph::holds_supervision` already scans every
/// live table for *any* holder, because a handle has always been movable, and a
/// second holder is the same shape as the first.
fn serve_supervision_derive(
    graph: &mut GraphTables,
    id: TaskId,
    words: &[sel4::Word; ipc::FAST_MESSAGE_REGISTERS],
) -> Response {
    let slot = words[0] as u32;
    let Ok(capability) = graph
        .get(id)
        .ok_or(IpcError::InvalidOperation)
        .and_then(|table| table.resolve(slot, RIGHT_SUPERVISE))
    else {
        return Response::error(IpcError::BadCapability);
    };
    let graph::Capability {
        resource: graph::Resource::Supervision { task },
        rights,
    } = capability
    else {
        return Response::error(IpcError::BadCapability);
    };
    // From slot 1 for the same reason spawn's own install does: slot 0 is the
    // component's control endpoint and is never handed out by the root.
    let Some(derived) = graph.get_mut(id).and_then(|table| {
        let free = table.free_slot_from(1)?;
        table
            .install(
                free,
                graph::Capability {
                    resource: graph::Resource::Supervision { task },
                    rights,
                },
            )
            .ok()?;
        Some(free)
    }) else {
        return Response::error(IpcError::DestinationSlotsExhausted);
    };
    sel4::debug_println!(
        "SLIME_GRAPH supervision derived task={} child={} slot={derived}",
        id.0,
        task.0,
    );
    Response::success(0, sel4::Word::from(derived))
}

/// Move one capability to a channel's peer, narrowed to exactly the mask its
/// descriptor declares (C8.3, P5.5.1).
///
/// The root's counterpart to `kernel/src/syscall/mod.rs::sys_cap_transfer`, and
/// deliberately as generic: it knows nothing of routes, schemas, or graph roles.
/// A userspace broker composes a typed fabric out of it, which is what makes
/// "a participant never holds a route endpoint directly but is provisioned one
/// by the fabric" a property of the graph rather than of this function.
///
/// Four rules, restated from the oracle against this crate's tables:
///
/// 1. **Transfer authority at the source.** The moved capability must carry
///    `RIGHT_TRANSFER`, the same condition a `send` attachment applies.
/// 2. **Narrow only.** The destination mask must be a subset of the source's
///    rights *and* of the kind's meaningful rights. A widening mask is refused
///    before anything moves.
/// 3. **Transfer authority is not inherited.** `RIGHT_TRANSFER` is dropped at
///    the destination unless the descriptor sets `FLAG_RETAIN_TRANSFER`, so a
///    provisioned endpoint is non-delegable by default rather than by
///    convention. That single bit is what makes `fabric-publisher`'s
///    re-delegation arm fail rather than succeed.
/// 4. **The descriptor describes the move.** Its declared `object_kind` must be
///    the moved capability's real kind, and the peer parses the same bytes this
///    enforced — through `slime_proto::capability_transfer`, the same generated
///    module both ends read — so a descriptor cannot advertise authority the
///    receiver did not get.
///
/// # Why this is not `Resource::is_transferable`
///
/// [`graph::Resource::is_transferable`] answers `true` only for a loan, and its
/// doc explains why widening it would be wrong: it gates the **send** path,
/// where a capability rides a message chosen at runtime by whoever holds a
/// channel. This is a different question with a different gate. Here the mover
/// must hold `RIGHT_TRANSFER` on the capability itself — authority the
/// generation placed or a parent narrowed at spawn — and the oracle's own
/// `sys_cap_transfer` gates on exactly that bit rather than on any kind
/// predicate. So the kind gate on `send` stands unchanged and this path is
/// authorized by rights, matching the retired kernel line for line.
///
/// The move consumes the source capability, so the object never has two
/// holders, and a failed send restores the original at its full rights rather
/// than dropping it.
#[allow(clippy::too_many_arguments)]
fn serve_cap_transfer(
    channels: &mut ChannelTable,
    graph: &mut GraphTables,
    windows: &WindowTable<MAX_TASKS>,
    parked: &mut ParkedReplies,
    transit: &mut Transit,
    scratch: &ScratchPage,
    supervision_waits: &mut supervision::SupervisionWaits,
    id: TaskId,
    words: &[sel4::Word; ipc::FAST_MESSAGE_REGISTERS],
    transfers: &mut usize,
) -> Response {
    use slime_proto::capability_transfer::{
        FLAG_RETAIN_TRANSFER, TRANSFER_LEN, WireCapabilityTransfer,
    };

    let endpoint_slot = (words[0] & 0xffff_ffff) as u32;
    let capability_slot = (words[0] >> 32) as u32;
    let frame = match transfer_window::read_staged(windows.bound(id), words[1], words, scratch) {
        Ok(frame) => frame,
        Err(error) => return Response::error(error),
    };
    if frame.cap_count() != 0 || frame.bytes().len() != TRANSFER_LEN {
        return Response::error(IpcError::InvalidLength);
    }
    let Some(descriptor) = WireCapabilityTransfer::decode(frame.bytes()) else {
        return Response::error(IpcError::InvalidLength);
    };
    if !valid_transfer_descriptor(&descriptor) {
        return Response::error(IpcError::InvalidLength);
    }
    let rights = if descriptor.flags & FLAG_RETAIN_TRANSFER != 0 {
        descriptor.rights_mask
    } else {
        descriptor.rights_mask & !RIGHT_TRANSFER
    };
    if rights == 0 {
        return Response::error(IpcError::InvalidLength);
    }

    let (channel, side) = match resolve_channel(graph, id, endpoint_slot, RIGHT_SEND) {
        Ok(endpoint) => endpoint,
        Err(error) => return Response::error(error),
    };
    let receiving_side = side.opposite();
    if endpoint_slot == capability_slot {
        return Response::error(IpcError::BadCapability);
    }

    let Some(table) = graph.get_mut(id) else {
        return Response::error(IpcError::InvalidOperation);
    };
    let Some(source) = table.get(capability_slot) else {
        return Response::error(IpcError::BadCapability);
    };
    if !source.allows(RIGHT_TRANSFER)
        || !descriptor_names(descriptor.object_kind, &source.resource)
        || rights & !source.rights != 0
        || rights & !valid_rights(&source.resource) != 0
    {
        return Response::error(IpcError::BadCapability);
    }
    let moved = graph::Capability {
        resource: source.resource,
        rights,
    };
    let original = source;
    table.drop_slot(capability_slot);
    let token = match transit.depart(moved, id, channel, receiving_side) {
        Ok(token) => token,
        Err(error) => {
            let _ = graph
                .get_mut(id)
                .map(|table| table.install(capability_slot, original));
            return Response::error(error);
        }
    };

    // The carrier endpoint cannot be consumed by the message it must enqueue.
    // Other endpoint capabilities move without holder bookkeeping: their side
    // travels inside the capability itself.
    if matches!(moved.resource, graph::Resource::Endpoint { channel: key, .. } if key == channel) {
        restore_transferred(graph, transit, id, capability_slot, token, original);
        return Response::error(IpcError::BadCapability);
    }

    let message = match ipc::Message::new(frame.bytes(), &[token]) {
        Ok(message) => message,
        Err(error) => {
            restore_transferred(graph, transit, id, capability_slot, token, original);
            return Response::error(error);
        }
    };
    let Some(queue) = channels.send_queue_mut(channel, side) else {
        restore_transferred(graph, transit, id, capability_slot, token, original);
        return Response::error(IpcError::InvalidOperation);
    };
    let wake = match ipc::send_atomic(queue, message, &mut CarryCapabilities) {
        Ok(wake) => wake,
        Err(error) => {
            restore_transferred(graph, transit, id, capability_slot, token, original);
            return Response::error(error);
        }
    };
    *transfers += 1;
    sel4::debug_println!(
        "SLIME_GRAPH capability transferred task={} channel={channel} side={} kind={} rights={rights:#x}",
        id.0,
        receiving_side.name(),
        moved.resource.kind(),
    );
    if let Some(wake) = wake {
        deliver_wake(channels, parked, supervision_waits, wake);
    }
    Response::success(0, 0)
}

/// Put a capability back after a transfer that did not cross, at its original
/// rights. The endpoint side is part of `original`, so rollback needs no
/// channel-table mutation.
fn restore_transferred(
    graph: &mut GraphTables,
    transit: &mut Transit,
    id: TaskId,
    slot: u32,
    token: ipc::LogicalCap,
    original: graph::Capability,
) {
    if transit.recall(token, id).is_none() {
        sel4::debug_println!(
            "SLIME_GRAPH FAIL capability lost task={} slot={slot} reason=transit-empty",
            id.0,
        );
        return;
    }
    let restored = graph
        .get_mut(id)
        .is_some_and(|table| table.install(slot, original).is_ok());
    if !restored {
        sel4::debug_println!(
            "SLIME_GRAPH FAIL capability lost task={} slot={slot} reason=install-refused",
            id.0,
        );
    }
}

/// Structural validity of a transfer descriptor, independent of the capability
/// it accompanies.
///
/// `kernel/src/protocol/capability_transfer_proto.rs::valid_transfer`, restated
/// against this crate's rights vocabulary. The one difference is the rights
/// bound: the kernel checks `rights_mask & !RIGHT_ALL == 0` against its own
/// enumeration of every defined bit, which this crate does not have — it names
/// only the bits it interprets. The per-kind check in [`valid_rights`] is
/// stricter than `RIGHT_ALL` anyway, since it bounds the mask by what the
/// *named object* can carry rather than by what any object could, so an
/// undefined bit is refused there rather than admitted here — and refused with
/// the same `ERR_BAD_CAP` the kernel answers for a mask its capability does not
/// hold, so the difference is not observable.
fn valid_transfer_descriptor(
    descriptor: &slime_proto::capability_transfer::WireCapabilityTransfer,
) -> bool {
    use slime_proto::capability_transfer::{
        CAPABILITY_TRANSFER_MAGIC, FORMAT_VERSION, KNOWN_FLAGS,
    };

    descriptor.magic == CAPABILITY_TRANSFER_MAGIC
        && descriptor.version == FORMAT_VERSION
        // A nonzero status marks a denial, which carries no capability and
        // never reaches this operation.
        && descriptor.status == 0
        && descriptor.flags & !KNOWN_FLAGS == 0
        && descriptor.rights_mask != 0
        // A kind this contract does not define is a *malformed descriptor*, not
        // a capability failure, and the difference is the answer the caller
        // gets: `ERR_INVALID_ARG` here against `ERR_BAD_CAP` from
        // `descriptor_names` below. `descriptor_names` would refuse it anyway —
        // no resource arm matches an undefined code — so without this the
        // refusal still happens but under the wrong error, diverging from
        // `sys_cap_transfer` on exactly the path this milestone claims parity
        // for.
        && is_object_kind(descriptor.object_kind)
}

/// Whether this contract version defines `object_kind`.
///
/// `kernel/src/protocol/capability_transfer_proto.rs::is_object_kind`, restated.
/// The transferable set is deliberately narrow: the objects a userspace broker
/// legitimately hands to a participant.
const fn is_object_kind(object_kind: u32) -> bool {
    use slime_proto::capability_transfer::{
        OBJECT_KIND_DIRECTORY, OBJECT_KIND_ENDPOINT, OBJECT_KIND_SHARED_BUFFER,
        OBJECT_KIND_SHARED_BUFFER_LOAN, OBJECT_KIND_SUPERVISION,
    };

    matches!(
        object_kind,
        OBJECT_KIND_ENDPOINT
            | OBJECT_KIND_SHARED_BUFFER
            | OBJECT_KIND_SHARED_BUFFER_LOAN
            | OBJECT_KIND_SUPERVISION
            | OBJECT_KIND_DIRECTORY
    )
}

/// Whether the descriptor's declared `object_kind` is the resource's real kind.
///
/// Rule 4 of the transfer contract: the peer decides what it received by
/// reading this field, so a descriptor that could name a kind the capability is
/// not would let a broker advertise authority the root never installed. An
/// `object_kind` this contract does not define matches nothing.
///
/// The kinds deliberately do not cover every [`graph::Resource`]. An executable
/// and a factory have no descriptor code, so neither can be moved over a
/// channel at all — not because this refuses them by name, but because the
/// contract has no way to describe one. That is `contracts/capability-transfer`
/// stating which objects a userspace broker may legitimately hand to a
/// participant, and it is the same set the retired kernel's `kind_code`
/// enumerates.
fn descriptor_names(object_kind: u32, resource: &graph::Resource) -> bool {
    use slime_proto::capability_transfer::{
        OBJECT_KIND_DIRECTORY, OBJECT_KIND_ENDPOINT, OBJECT_KIND_SHARED_BUFFER,
        OBJECT_KIND_SHARED_BUFFER_LOAN, OBJECT_KIND_SUPERVISION,
    };

    match resource {
        graph::Resource::Endpoint { .. } => object_kind == OBJECT_KIND_ENDPOINT,
        graph::Resource::SharedBuffer { .. } => object_kind == OBJECT_KIND_SHARED_BUFFER,
        graph::Resource::Loan { .. } => object_kind == OBJECT_KIND_SHARED_BUFFER_LOAN,
        graph::Resource::Supervision { .. } => object_kind == OBJECT_KIND_SUPERVISION,
        graph::Resource::Directory { .. } => object_kind == OBJECT_KIND_DIRECTORY,
        graph::Resource::Executable { .. }
        | graph::Resource::EndpointFactory
        | graph::Resource::SharedBufferFactory
        | graph::Resource::Block { .. }
        | graph::Resource::Input => false,
    }
}

/// Return a dead task's own objects: its VSpace, image frames, CNode, and TCB.
///
/// The half of teardown `reclaim_dead_task` does not do. That function settles
/// what the task *held* — channels, buffers, loans, in-flight capabilities —
/// and this returns what the task *is*. Both death paths need both: a task
/// whose peers were all notified and whose buffers were all reclaimed still
/// occupies a `TaskTable` entry and still holds every root CSlot its
/// construction allocated.
///
/// Two things depend on it, which is why it is not merely tidiness:
///
/// - **`TaskTable::live_children`** counts the table, so a dead child that
///   stays in it consumes its parent's declared `spawnBudget` forever. The
///   budget would be a lifetime cap rather than the live-child cap the
///   generation declares and `sys_spawn` enforces.
/// - **`CleanupRecord::revoke`** is reachable only from `TaskTable::reclaim`,
///   so without this every component that exits or faults leaks its root
///   CSlots for the rest of the boot.
///
/// Reported rather than fatal: the objects stay recorded as the table's own
/// state and the terminal marker's count is what surfaces them, and a graph
/// whose other components are still running should not be stopped over one
/// task's cleanup.
fn reclaim_task_objects(
    launched: &mut LaunchedInstances,
    tasks: &mut TaskTable<MAX_TASKS>,
    allocator: &mut ObjectAllocator,
    reclaimed: &mut usize,
    id: TaskId,
) {
    launched.release_by_task(id);
    match tasks.reclaim(allocator, id) {
        Ok(record) => *reclaimed += record.slot_count(),
        Err(error) => sel4::debug_println!(
            "SLIME_GRAPH task reclaim incomplete task={} error={error:?}",
            id.0
        ),
    }
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
    transit: &mut Transit,
    graph: &GraphTables,
    buffers: &mut SharedBufferTable,
    allocator: &mut ObjectAllocator,
    supervision_waits: &mut supervision::SupervisionWaits,
    id: TaskId,
    settled: &mut usize,
) {
    parked.abandon(id);
    channels.clear_waits(id);
    // A dying task's own registrations, not the ones naming it: those are
    // answered by `wake_supervisors` before this runs, and clearing them here
    // would be clearing another task's wait set.
    supervision_waits.clear(id);

    let held = graph
        .get(id)
        .map_or(0, graph::CapabilityTable::endpoints_held);
    let mut wakes = channel::DeathWakes::new();
    channels.mark_dead(graph, id, &mut wakes);

    let mut woken = 0;
    for (_, wake) in wakes.drain() {
        // A wake naming a task that is not parked is not an error: it was
        // registered by a `wait` that has since been answered, and the
        // registration outlived it. `deliver_wake` returns without doing
        // anything in that case.
        deliver_wake(channels, parked, supervision_waits, wake);
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
    let stranded = transit.reclaim(graph, id);

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
    // The ceiling last, after every charge against it is settled: a quota
    // dropped before `reclaim_holder` would leave the table charging a holder
    // it can no longer bound. Nothing can be charged again once the task is
    // gone, so this is the point at which the entry becomes unreachable.
    //
    // B24: `quotas` had no free path, and its key derives from a task id that
    // never rewinds, so `MAX_CHARGE_HOLDERS` counted the holders a boot ever
    // constructed rather than those live at once.
    if buffers.release_quota(holder) {
        sel4::debug_println!(
            "SLIME_GRAPH quota released task={} live={}",
            id.0,
            buffers.quota_count(),
        );
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
    channel: ipc::ChannelKey,
    receiving_side: graph::Side,
    /// Every `(original slot, token)` this transfer parked. Recorded because
    /// `send_atomic` can still fail after `transfer_atomic` returns.
    departed: [Option<(ipc::LogicalCap, ipc::LogicalCap)>; ipc::MAX_MESSAGE_CAPS],
    /// Why this transfer refused, if it did.
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
        // A loan remains bound to its declared receiver. The send destination
        // is now a channel end rather than one guessed task, so require that
        // declared receiver to hold that end. A co-holder may dequeue the bytes,
        // but cannot use a loan not naming it; the intended receiver remains
        // authorized exactly as on x86.
        if let Some(graph::Capability {
            resource: graph::Resource::Loan { handle },
            ..
        }) = self
            .graph
            .get(self.sender)
            .and_then(|table| table.get(slot))
        {
            let Ok(receiver) = u32::try_from(handle.receiver.0) else {
                return Err(IpcError::UnsupportedCapabilityTransfer);
            };
            if !self
                .graph
                .get(TaskId(receiver))
                .is_some_and(|table| table.reaches_endpoint(self.channel, self.receiving_side))
            {
                return Err(IpcError::UnsupportedCapabilityTransfer);
            }
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
        match self
            .transit
            .depart(capability, self.sender, self.channel, self.receiving_side)
        {
            Ok(token) => Ok(token),
            Err(error) => {
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
/// `receiver_slot` names the receiver through a capability, never through an
/// ambient task id a component supplied — which is the property the exit
/// condition asks for. Two resource kinds satisfy that, and the caller may use
/// either.
///
/// A **supervision handle** names its subject outright. It was minted by the
/// spawn that created that task and names nothing else, ever. This is how the
/// retired kernel does it (`kernel/src/syscall/mod.rs::sys_shared_buffer_loan`),
/// and it is what `sample-lender` — unmodified — passes at its `RECEIVER_SLOT`,
/// so accepting it is what lets a component written against that ABI run here
/// (P5.3.4).
///
/// A **channel end** names its peer. P5.3.2 admitted only this, because no
/// spawn existed to mint a handle; it is kept because it is a real bound in its
/// own right — a component can only loan to a task the generation gave it an
/// edge to — and because a graph without spawn has no handle to name.
///
/// Neither widens the other. A supervision handle is authority over a task the
/// caller *created*, from an executable the generation granted it; a channel end
/// is authority over a task the generation *connected* it to. Both are
/// delegations the manifest made, differing in which one they rest on.
///
/// Note what is *not* read: the x86 grant `sample-plane-receiver-supervision` is
/// `source = init, target = sample-lender` and means "init may hand
/// sample-lender a handle", naming no subject at all. A handle's subject comes
/// from the spawn that minted it, which is the only thing that could know it.
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
    //
    // **Two kinds resolve here**, and the difference is which question the
    // caller is answering.
    //
    // A `Supervision` handle names its subject outright: it was minted by the
    // spawn that created that task and names nothing else, ever. That is how
    // the retired kernel does it (`sys_shared_buffer_loan`), and it is what
    // `sample-lender` — unmodified — passes at `RECEIVER_SLOT`, so accepting it
    // is what lets a component written against that ABI run here unchanged.
    //
    // A channel end names its peer. P5.3.2 admitted only this, because no spawn
    // existed to mint a handle; it is kept because it is a real bound in its
    // own right — a component can only loan to a task the generation gave it an
    // edge to — and because `sel4-loan.zti`'s graph has no spawn in it.
    //
    // Neither widens the other. A supervision handle is authority over a task
    // the caller *created*; a channel end is authority over a task the
    // generation *connected* it to. In both cases the receiver is reached
    // through a capability rather than an ambient task id, which is what the
    // exit condition asks for.
    let resolved = graph.get(id).and_then(|table| {
        table
            .resolve(receiver_slot, RIGHT_SUPERVISE)
            .ok()
            .and_then(|capability| match capability.resource {
                graph::Resource::Supervision { task } => Some((task, None)),
                _ => None,
            })
            .or_else(|| {
                let capability = table.resolve(receiver_slot, RIGHT_SEND).ok()?;
                match capability.resource {
                    graph::Resource::Endpoint { channel, side } => graph
                        .unique_holder_of_endpoint_side(channel, side.opposite(), Some(id))
                        .map(|task| (task, Some(channel))),
                    _ => None,
                }
            })
    });
    let Some((peer, edge)) = resolved else {
        sel4::debug_println!(
            "SLIME_GRAPH loan refused task={} slot={receiver_slot} class=absent-or-ambiguous",
            id.0,
        );
        return Response::error(IpcError::BadCapability);
    };
    if peer == id {
        return Response::error(IpcError::BadCapability);
    }
    // A channel-derived receiver is admitted only over a declared delegable
    // edge. A supervision handle already names its task directly and needs no
    // channel transferability statement.
    if let Some(channel) = edge
        && channels.transferable(channel) != Some(true)
    {
        sel4::debug_println!(
            "SLIME_GRAPH loan refused task={} slot={buffer_slot} class=undelegated",
            id.0,
        );
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
    //
    // **A loan slot resolves here too**, for `unmap` alone, because
    // `sys_shared_buffer_unmap` accepts one: its resolution arm is
    // `SharedBufferLoan(loan) => loan.region()`, so a receiver that mapped
    // through `loan_map` unmaps through the same slot it mapped with. Without
    // this a component doing exactly that — which `fabric-subscriber` does on
    // every shared sample — is answered `ERR_BAD_CAP` on a slot it holds, and
    // has no other slot to name: the region belongs to the *lender*, and the
    // receiver was never issued a buffer capability for it.
    //
    // Only `unmap`. The oracle's `map`, `seal`, and `release` each require a
    // `SharedBuffer` and refuse a loan, so widening those would grant a
    // receiver authority over a region it merely borrows. The asymmetry is the
    // oracle's, not this function's.
    //
    // A loan resolves to a *different table call* rather than to a converted
    // handle, because the two authorize differently: a receiver does not own
    // the region, so `unmap`'s owner check would refuse it. See
    // `SharedBufferTable::unmap_loan`.
    enum Subject {
        Buffer(shared_buffer::BufferHandle),
        Loan(shared_buffer::LoanHandle),
    }
    let resolved = graph.get(id).and_then(|table| match table.get(slot) {
        Some(graph::Capability {
            resource: graph::Resource::SharedBuffer { handle },
            ..
        }) => Some(Subject::Buffer(handle)),
        Some(graph::Capability {
            resource: graph::Resource::Loan { handle },
            ..
        }) if operation == Operation::SharedBufferUnmap => Some(Subject::Loan(handle)),
        _ => None,
    });
    let Some(subject) = resolved else {
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
    // A loan reaches only the unmap arm, by construction above.
    let handle = match subject {
        Subject::Buffer(handle) => handle,
        Subject::Loan(loan) => {
            let outcome = buffers.unmap_loan(&mut adapter, holder, loan, vspace, words[1] as usize);
            return finish_buffer_lifecycle(operation, graph, id, slot, outcome, served);
        }
    };
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
    finish_buffer_lifecycle(operation, graph, id, slot, outcome, served)
}

/// Turn a buffer-lifecycle table outcome into the wire response.
///
/// Extracted so the loan-unmap path and the buffer path answer identically:
/// the same success accounting, the same marker, and the same error mapping.
/// Two copies would be two places for those to drift.
fn finish_buffer_lifecycle(
    operation: Operation,
    graph: &mut GraphTables,
    id: TaskId,
    slot: u32,
    outcome: Result<(), shared_buffer::SharedBufferError>,
    served: &mut usize,
) -> Response {
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
