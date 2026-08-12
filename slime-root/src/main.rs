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
    buffer_adapter, child_vspace, console, device, directory, event, fault, generation, graph, ipc,
    launched, notification, object_allocator, peer_endpoint, platform_timer, shared_buffer,
    supervision, task, timer, transfer_window, virtio_blk,
};

use core::ptr;

use boot_contracts::generation::{
    Generation, Grant, GrantEndpoint, Instance, InstanceHealth, InstanceOwner, KIND_RESOURCE,
    MintedBinding,
};
use boot_contracts::shared_buffer_budget::{self as budget_magic, SharedBufferBudget};
use sel4_root_task::root_task;

use boot_contracts::generation::RIGHT_TRANSFER;
use buffer_adapter::BufferAdapter;
use child_vspace::{ChildImage, GRANULE_SIZE, ScratchPage};
use console::{RIGHT_BLOCK_READ, RIGHT_BLOCK_WRITE};
use device::{BlockDevices, MAX_BLOCK_DEVICES};
use directory::RIGHTS_DIRECTORY_ALL;
use event::TaskEpoch;
use fault::{LifecycleEventKind, SupervisionTable};
use generation::{Admission, Authority, RIGHT_EXEC, RIGHT_RECV, RIGHT_SEND, bound_authority};
use graph::GraphTables;
use ipc::{IpcError, Operation, Response, poll_notification};
use launched::LaunchedInstances;
use object_allocator::ObjectAllocator;
use platform_timer::{PhysicalTimerAdapter, TIMER_IRQ};
use shared_buffer::{
    BufferHandle, GenerationEpoch, HolderId, HolderQuota, MappingRights, PAGE_SIZE,
    SharedBufferAdapter, SharedBufferTable, VSpaceCap,
};
use task::{Arrival, CHILD_CNODE_SIZE_BITS, MAX_TASKS, Supervision, TaskId, TaskTable};
use timer::{PlatformTimer, ServiceTimerError, TimerScheduler, apply_deadline_programming};
use transfer_window::{WindowTable, descriptor_thread};

/// One staged transfer window per admitted component thread.
const MAX_WINDOW_ENTRIES: usize = MAX_TASKS * child_vspace::MAX_CHILD_THREADS;

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

/// A root-image page whose virtual address becomes the console dispatcher's
/// scratch window (B41).
///
/// Separate from the loader's: the console thread maps a caller's window into
/// its own address while the main dispatcher may be mapping another into
/// [`FREE_PAGE`], and one shared virtual address cannot hold both.
static mut CONSOLE_PAGE: FreePage = FreePage([0; GRANULE_SIZE]);

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
    // The plan's total against the root's actual CSpace, which only the
    // allocator knows (B49). Per-instance ceilings say each process fits; this
    // says they all fit together, before any component starts rather than
    // partway through construction with children already running.
    let planned_slots = match generation::admit_total_slots(&generation, allocator.free_slots()) {
        Ok(required) => required,
        Err(error) => fatal!("generation admission rejected: {error:?}"),
    };
    sel4::debug_println!(
        "SLIME_ROOT plan slots required={planned_slots} available={}",
        allocator.free_slots(),
    );

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
            bootinfo,
            allocator,
            &scratch,
            service_endpoint,
            console_endpoint,
            &mut block_devices,
            #[cfg(slime_boot_selector)]
            &mut boot_runtime,
        );
        loop {
            core::hint::spin_loop();
        }
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
            task::CHILD_PRIORITY,
            // The fixture path predates the thread plan and runs one thread.
            1,
            // No workers, so no worker priorities.
            [task::CHILD_PRIORITY; child_vspace::MAX_CHILD_THREADS],
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

/// Authority to map a loaned range. The rights a loan capability carries, and
/// the pair the retired kernel's `sys_shared_buffer_loan` installs on the
/// handle it returns.
const RIGHT_BUFFER_MAP: u64 = 1 << 9;

/// Authority to map a loaned range writable (B46). A read-only C7.6 sample
/// loan never carries it; a stream ring loan does, because two peers advance
/// disjoint header fields of one region.
const RIGHT_BUFFER_WRITE: u64 = 1 << 8;

/// Authority to run an executable, held alongside `RIGHT_EXEC`. Holding an
/// image is not authority to start it: `preflight_spawn_grants` requires both.
const RIGHT_SPAWN: u64 = 1 << 16;

/// Authority to observe a spawned child's termination. The right the handle a
/// spawn returns carries, matching `kernel/src/capability/mod.rs`.
const RIGHT_SUPERVISE: u64 = 1 << 18;

/// Authority to allocate a shared buffer, held on a `SharedBufferFactory`.
/// Independent of the holder's quota by design: the grant authorizes the
/// operation and the budget bounds it (B13).
const RIGHT_BUFFER_CREATE: u64 = 1 << 24;

/// Authority to read one decoded key event, held on an `Input` (M6.4).
const RIGHT_INPUT_READ: u64 = 1 << 23;

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

/// Generation-global native object catalogues. Objects live for the generation;
/// child-local derived capabilities are charged to and reclaimed with each task.
static mut PEER_ENDPOINTS: peer_endpoint::PeerEndpointTable =
    peer_endpoint::PeerEndpointTable::new();
static mut NOTIFICATIONS: notification::NotificationTable = notification::NotificationTable::new();

const MAX_CAPABILITY_EXPORTS: usize = 64;
#[derive(Clone, Copy)]
struct CapabilityExport {
    id: u32,
    sender: TaskId,
    receiver: TaskId,
    capability: graph::Capability,
    /// The kernel ticket a native-Endpoint export minted, or `None` for a
    /// root-owned logical capability, which has no kernel object to hand over.
    ticket: Option<sel4::CPtrBits>,
    retain: bool,
    finalized: bool,
}
struct CapabilityExports {
    entries: [Option<CapabilityExport>; MAX_CAPABILITY_EXPORTS],
    next_id: u32,
    exported: usize,
    imported: usize,
    cancelled: usize,
    finalized: usize,
}
impl CapabilityExports {
    const fn new() -> Self {
        Self {
            entries: [None; MAX_CAPABILITY_EXPORTS],
            next_id: 1,
            exported: 0,
            imported: 0,
            cancelled: 0,
            finalized: 0,
        }
    }
    fn len(&self) -> usize {
        self.entries.iter().filter(|entry| entry.is_some()).count()
    }
    fn get_mut(&mut self, id: u32) -> Option<&mut CapabilityExport> {
        self.entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.id == id)
    }
    fn remove(&mut self, id: u32) -> Option<CapabilityExport> {
        let slot = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_some_and(|entry| entry.id == id))?;
        slot.take()
    }
}
static mut CAPABILITY_EXPORTS: CapabilityExports = CapabilityExports::new();
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
/// The console dispatcher's stack and IPC buffer, both root-image pages so
/// they are mapped before the thread runs (B41).
static mut CONSOLE_STACK: console::ConsoleStack = console::ConsoleStack::new();
static mut CONSOLE_IPC_BUFFER: FreePage = FreePage([0; GRANULE_SIZE]);

/// Everything the console loop reads, in a `static` so the thread can hold a
/// pointer to it for as long as it runs.
static mut CONSOLE_CONTEXT: Option<console::ConsoleContext> = None;

/// The scripted key source, owned by the console thread (B41).
static mut CONSOLE_INPUT: Option<console::ScriptedInput> = None;

/// Start the console dispatcher (B41).
///
/// A second root thread, sharing the root's CSpace and VSpace and running at
/// the root's own priority, so it round-robins with the service loop rather
/// than starving behind it or preempting it.
///
/// The thread names its own IPC buffer on every invocation rather than using
/// the crate's ambient slot, so it needs no thread-local state: there is one
/// such slot per address space, and a blocked receive holds it borrowed for as
/// long as it waits.
/// What the second dispatcher serves over, gathered so the launcher's
/// signature names one thing per concern rather than nine positional
/// references.
struct ConsoleTables<'a> {
    windows: &'a WindowTable<MAX_WINDOW_ENTRIES>,
    graph: &'a GraphTables,
    script: &'static [u8],
    devices: &'a mut BlockDevices,
    namespaces: &'a mut directory::Namespaces,
    scopes: &'a graph::ScopeTable,
}

fn start_console_dispatcher(
    bootinfo: &sel4::BootInfo,
    allocator: &mut ObjectAllocator,
    endpoint: sel4::cap::Endpoint,
    tables: ConsoleTables<'_>,
) {
    let ConsoleTables {
        windows,
        graph,
        script,
        devices,
        namespaces,
        scopes,
    } = tables;
    let scratch_addr = ptr::addr_of!(CONSOLE_PAGE) as usize;
    let scratch = match ScratchPage::claim(bootinfo, scratch_addr) {
        Ok(scratch) => scratch,
        Err(error) => fatal!("console scratch unavailable: {error:?}"),
    };
    let tcb = match allocator.allocate_fixed::<sel4::cap_type::Tcb>() {
        Ok(slot) => slot.cap(),
        Err(error) => fatal!("console TCB unavailable: {error:?}"),
    };
    let ipc_addr = ptr::addr_of!(CONSOLE_IPC_BUFFER) as usize;
    let ipc_frame = child_vspace::image_frame(bootinfo, ipc_addr);

    // SAFETY: single-threaded until the resume below, and this is the only
    // reference taken to either static.
    let context = unsafe {
        let slot = &mut *ptr::addr_of_mut!(CONSOLE_CONTEXT);
        let input = &mut *ptr::addr_of_mut!(CONSOLE_INPUT);
        *input = Some(console::ScriptedInput::new(script));
        let input = match input.as_mut() {
            Some(input) => ptr::addr_of_mut!(*input),
            None => fatal!("console input unset"),
        };
        *slot = Some(console::ConsoleContext {
            endpoint,
            scratch,
            windows: windows as *const _,
            buffer: ptr::addr_of_mut!(CONSOLE_IPC_BUFFER) as *mut sel4::IpcBuffer,
            input,
            graph: graph as *const _,
            devices: ptr::addr_of_mut!(*devices),
            namespaces: ptr::addr_of_mut!(*namespaces),
            scopes: scopes as *const _,
        });
        match slot.as_ref() {
            Some(context) => ptr::addr_of!(*context),
            None => fatal!("console context unset"),
        }
    };

    let configured = tcb.tcb_configure(
        // No fault handler: a fault in this thread is a root defect, and a
        // null handler makes the kernel report it rather than deliver it
        // somewhere that would swallow it.
        sel4::CPtr::from_bits(0),
        sel4::init_thread::slot::CNODE.cap(),
        // The root CNode's own guard. A zero guard faults every lookup: a CPtr
        // resolves to `WORD_SIZE` bits and the CNode holds fewer.
        child_vspace::root_cspace_guard(bootinfo),
        sel4::init_thread::slot::VSPACE.cap(),
        ipc_addr as sel4::Word,
        ipc_frame,
    );
    if let Err(error) = configured {
        fatal!("console thread configure failed: {error:?}")
    }
    // The root's own priority: this thread answers on equal terms with the
    // service loop rather than starving behind it.
    let scheduled = tcb.tcb_set_sched_params(sel4::init_thread::slot::TCB.cap(), 255, 255);
    if let Err(error) = scheduled {
        fatal!("console thread priority failed: {error:?}")
    }

    let stack_top = ptr::addr_of!(CONSOLE_STACK) as usize + size_of::<console::ConsoleStack>();
    let mut registers = sel4::UserContext::default();
    // Through a fn pointer rather than casting the item directly, which
    // clippy refuses: the address is the same, the intent is explicit.
    let entry: extern "C" fn(usize) -> ! = console_entry;
    *registers.pc_mut() = entry as usize as sel4::Word;
    // AArch64 requires a 16-byte aligned stack pointer.
    *registers.sp_mut() = (stack_top & !0xf) as sel4::Word;
    *registers.c_param_mut(0) = context as usize as sel4::Word;
    let started = tcb.tcb_write_all_registers(true, &mut registers);
    if let Err(error) = started {
        fatal!("console thread start failed: {error:?}")
    }
    sel4::debug_println!("SLIME_ROOT console dispatcher started");
}

/// The console thread's entry point.
extern "C" fn console_entry(context: usize) -> ! {
    // No `set_ipc_buffer`: this thread names its buffer on every invocation
    // instead, so it touches none of the crate's ambient state.
    //
    // SAFETY: `context` is the `CONSOLE_CONTEXT` pointer written by
    // `start_console_dispatcher`, which lives in a `static`.
    unsafe { console::serve(&*(context as *const console::ConsoleContext)) }
}

fn launch_instance_graph(
    generation: &Generation<'_>,
    admission: &Admission,
    bootinfo: &sel4::BootInfo,
    allocator: &mut ObjectAllocator,
    scratch: &ScratchPage,
    service_endpoint: sel4::cap::Endpoint,
    console_endpoint: sel4::cap::Endpoint,
    block_devices: &mut BlockDevices,
    #[cfg(slime_boot_selector)] boot_runtime: &mut boot_selector::BootRuntime,
) {
    let mut tasks = TaskTable::<MAX_TASKS>::new();
    let mut windows = WindowTable::<MAX_WINDOW_ENTRIES>::new();
    let mut graph = GraphTables::new();
    let peers = unsafe { &mut *ptr::addr_of_mut!(PEER_ENDPOINTS) };
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
        // Priority likewise. An instance that declares none resolves to the
        // root's default, which is what every child ran at before the plan's
        // `ScheduleRecord` was consulted at all (B48).
        let declared_priority = match generation.instance_priority(instance_index) {
            Ok(Some(priority)) => sel4::Word::from(priority),
            Ok(None) => task::CHILD_PRIORITY,
            Err(error) => fatal!("SLIME_GRAPH FAIL schedule plan rejected: {error:?}"),
        };
        // Its own record rather than a field on `staged`: the priority a
        // thread runs at is not observable from anything else in the
        // transcript, and a declaration nothing can check is indistinguishable
        // from the constant it replaced (B48).
        sel4::debug_println!(
            "SLIME_GRAPH schedule instance={} priority={declared_priority} default={}",
            instance.name,
            task::CHILD_PRIORITY,
        );
        // Threads the plan declares for this instance (B47). One unless the
        // manifest asked for more; the root builds exactly this many TCBs, so
        // a declared thread that never runs would be visible here as a count
        // the transcript disagrees with.
        let declared_threads = match generation.instance_threads(instance_index) {
            Ok(Some(threads)) => threads,
            Ok(None) => 1,
            Err(error) => fatal!("SLIME_GRAPH FAIL thread plan rejected: {error:?}"),
        };
        sel4::debug_println!(
            "SLIME_GRAPH threads instance={} count={declared_threads}",
            instance.name,
        );
        // Each worker's own declared priority (B48). Resolved here rather than
        // in `task::create` so the transcript records what the plan asked for,
        // the same way the main thread's priority is recorded above.
        let mut declared_worker_priorities = [declared_priority; child_vspace::MAX_CHILD_THREADS];
        for (thread_index, slot) in declared_worker_priorities
            .iter_mut()
            .enumerate()
            .take(declared_threads)
            .skip(1)
        {
            let resolved = match generation.thread_priority(instance_index, thread_index) {
                Ok(Some(priority)) => sel4::Word::from(priority),
                Ok(None) => declared_priority,
                Err(error) => fatal!("SLIME_GRAPH FAIL thread schedule rejected: {error:?}"),
            };
            *slot = resolved;
            sel4::debug_println!(
                "SLIME_GRAPH schedule instance={} thread={thread_index} priority={resolved}",
                instance.name,
            );
        }
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
            declared_priority,
            declared_threads,
            declared_worker_priorities,
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
        // One window per thread (B47): each stages its own payloads, and a
        // thread whose window was never declared is refused at bind and cannot
        // receive anything.
        for (thread, pages) in task
            .vspace
            .pages
            .iter()
            .take(task.vspace.threads)
            .enumerate()
        {
            if let Err(error) = windows.declare(
                id,
                thread,
                pages.transfer_window_addr,
                pages.transfer_window,
                pages.transfer_window_alias,
            ) {
                fatal!("SLIME_GRAPH FAIL window declaration rejected: {error:?}")
            }
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
            // A factory is authority to mint objects, so where it lands is the
            // difference between a component reaching its own declared factory
            // and reaching none. The boot layout names the slot and this
            // records that the root honoured it.
            if matches!(capability.resource, graph::Resource::SharedBufferFactory) {
                sel4::debug_println!(
                    "SLIME_GRAPH factory placed task={} component={} slot={slot} kind={}",
                    id.0,
                    instance.name,
                    capability.resource.kind_name(),
                );
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
            task.vspace.main().transfer_window_addr,
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
    let materialized = match peers.materialize(generation, &launched_instances, allocator, &tasks) {
        Ok(report) => report,
        Err(error) => fatal!("SLIME_GRAPH FAIL endpoint materialization rejected: {error:?}"),
    };
    sel4::debug_println!(
        "SLIME_GRAPH peer endpoints created={} grants={} installed={}",
        peers.len(),
        materialized.grants,
        materialized.installed,
    );
    let notifications = unsafe { &mut *ptr::addr_of_mut!(NOTIFICATIONS) };
    let mut notification_report = match notifications.materialize(generation, allocator) {
        Ok(report) => report,
        Err(error) => fatal!("SLIME_GRAPH FAIL notification materialization rejected: {error:?}"),
    };
    for launched in launched_instances.iter() {
        let Some(task) = tasks.get(launched.task) else {
            fatal!(
                "SLIME_GRAPH FAIL launched task {} is missing",
                launched.task.0
            )
        };
        let installed = match notifications.install_instance(
            generation,
            launched.instance,
            launched.task,
            allocator,
            task.cleanup.arena,
            task.cnode,
            task.cnode_size_bits,
        ) {
            Ok(installed) => installed,
            Err(error) => fatal!("SLIME_GRAPH FAIL notification install rejected: {error:?}"),
        };
        notification_report.bindings += installed;
    }
    sel4::debug_println!(
        "SLIME_GRAPH notifications created={} bindings={}",
        notification_report.created,
        notification_report.bindings,
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
        // The same record the spawn path emits. The boot path declared its
        // quotas silently and printed only the aggregate, so a per-instance
        // ceiling could be wrong in the generation and invisible in the
        // transcript -- and a gate checking the declared ceilings against the
        // observed ones saw only the spawned children (B52).
        sel4::debug_println!(
            "SLIME_GRAPH quota task={} instance={} executable={} pages={} buffers={} mappings={} loans={}",
            launched_instance.task.0,
            instance.name,
            generation
                .executable(launched_instance.executable)
                .map_or("<unknown>", |record| record.name),
            quota.byte_pages,
            quota.buffer_count,
            quota.mapping_count,
            quota.loan_count,
        );
    }
    sel4::debug_println!(
        "SLIME_GRAPH quotas declared={} budgeted={budgeted} holders={}",
        launched_instances.len(),
        budget.as_ref().map_or(0, SharedBufferBudget::holder_count),
    );
    // The shared filesystem namespaces (M6.3) and interned directory scopes.
    // Declared here rather than inside the serve loop because the console
    // thread reads both and starts first (B45); a capability carries a scope
    // index rather than a path, since inlining 128 bytes into every
    // `Resource` grew the capability tables past the root's stack.
    let mut namespaces = directory::Namespaces::new();
    let mut scopes = graph::ScopeTable::new();

    // B41: the console dispatcher starts before the service loop, so console
    // traffic has a receiver for as long as any child can send. `windows`
    // outlives it — `serve_instance_graph` does not return.
    start_console_dispatcher(
        bootinfo,
        allocator,
        console_endpoint,
        ConsoleTables {
            windows: &windows,
            graph: &graph,
            script: input_script(generation.number),
            devices: block_devices,
            namespaces: &mut namespaces,
            scopes: &scopes,
        },
    );

    serve_instance_graph(
        generation,
        &mut launched_instances,
        service_endpoint,
        console_endpoint,
        &mut tasks,
        &mut windows,
        &mut graph,
        &mut buffers,
        allocator,
        scratch,
        &mut scopes,
        #[cfg(slime_boot_selector)]
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
    windows: &mut WindowTable<MAX_WINDOW_ENTRIES>,
    graph: &mut GraphTables,
    buffers: &mut SharedBufferTable,
    allocator: &mut ObjectAllocator,
    scratch: &ScratchPage,
    // The block devices, needed here only by the selector variant's promotion
    // path. Component block traffic reaches the console thread, which owns
    // the tables (B43), so the service loop no longer touches them.
    // Interned directory scopes. Derive is the only writer and stays here,
    // because it also writes the caller's capability table, which this loop
    // writes on `cap_drop` and on a spawn's result (B45).
    scopes: &mut graph::ScopeTable,
    #[cfg(slime_boot_selector)] block_devices: &mut BlockDevices,
    #[cfg(slime_boot_selector)] boot_runtime: &mut boot_selector::BootRuntime,
) {
    sel4::debug_println!(
        "SLIME_ROOT allocator baseline live_slots={} live_objects={} live_bytes={}",
        allocator.live_slots(),
        allocator.live_objects(),
        allocator.live_bytes(),
    );
    let mut live = tasks.len();
    let mut unsupported = 0;
    let mut unimplemented = 0;
    let mut buffers_served = 0;
    let mut loans_served = 0;
    let mut spawns = 0;
    let mut drops = 0;
    let mut reclaimed_slots = 0;
    let mut terminations = supervision::Terminations::new();
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
        sel4::with_ipc_buffer_mut(|buffer| {
            buffer.set_recv_slot(
                &sel4::init_thread::slot::CNODE
                    .cap()
                    .absolute_cptr(sel4::cap::Unspecified::from_bits(task::CHILD_SLOT_RECEIVE)),
            );
        });
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
                id,
                supervision::Termination::Fault(reason),
            );
            if let Some(task) = tasks.get(id) {
                let _ = task.suspend();
            }
            reclaim_dead_task(buffers, allocator, id);
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

        match operation {
            // M6.3: the three directory operations (P5.4.3).
            //
            // Mechanism, not policy. What a directory *contains* is a
            // filesystem component's business, built over the object store;
            // what the root owns is the unforgeable part — a shared namespace
            // root, scoped views that derivation may only narrow, and an atomic
            // compare-and-swap that keeps two writers from losing an update.
            // M6.4: one scripted key event, gated on an `Input` capability.
            Operation::DirectoryDerive => {
                ipc::reply(directory::serve_directory_derive(
                    graph,
                    scopes,
                    windows.bound(id, descriptor_thread(words[1])),
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
                record_termination(
                    &mut terminations,
                    graph,
                    id,
                    supervision::Termination::Exit(status),
                );
                if let Some(task) = tasks.get(id) {
                    let _ = task.suspend();
                }
                reclaim_dead_task(buffers, allocator, id);
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
            Operation::CapabilityExport => {
                ipc::reply(serve_capability_export(
                    generation, launched, graph, allocator, tasks, id, &words,
                ));
            }
            Operation::CapabilityImport => {
                ipc::reply(serve_capability_import(graph, id, &words));
            }
            Operation::CapabilityExportCancel => {
                ipc::reply(serve_capability_cancel(graph, tasks, id, &words));
            }
            Operation::CapabilityExportFinalize => {
                ipc::reply(serve_capability_finalize(id, &words));
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
                    Ok(graph::Capability {
                        resource: graph::Resource::SharedBufferFactory,
                        ..
                    }) => {
                        match serve_buffer_create(buffers, allocator, holder, pages, writable) {
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
                        }
                    }
                    _ => {
                        sel4::debug_println!(
                            "SLIME_GRAPH buffer create refused task={} class=ungranted",
                            id.0
                        );
                        Response::error(IpcError::BadCapability)
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
                    generation,
                    launched,
                    buffers,
                    allocator,
                    graph,
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
        if required != 0 {
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
                    .any(|task| task.instance == Some(instance_index))
                {
                    live_required += 1;
                }
            }
            if live_required + completed == required && (!healthy_emitted || live_required == 0) {
                #[cfg(slime_boot_selector)]
                if !healthy_emitted && boot_runtime.running_pending() {
                    let device = block_devices
                        .get_mut(0)
                        .unwrap_or_else(|| fatal!("boot promotion has no boot device"));
                    match boot_runtime.confirm(device) {
                        Ok(()) => sel4::debug_println!("SLIME_BOOT promoted"),
                        Err(error) => fatal!("boot promotion rejected: {error:?}"),
                    }
                }
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
                } else if live_required == 0 {
                    // Emitted after the accounting summary below: the QEMU
                    // gates stop reading at this terminal certification.
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
    if iterations == MAX_GRAPH_ITERATIONS && live != 0 {
        fatal!("SLIME_GRAPH FAIL graph iterations exhausted live={live}")
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
    let exports = unsafe { &mut *ptr::addr_of_mut!(CAPABILITY_EXPORTS) };
    for slot in &mut exports.entries {
        let Some(export) = slot.take() else { continue };
        if export.finalized {
            exports.imported = exports.imported.saturating_add(1);
            if let Some(bits) = export.ticket {
                let ticket = sel4::init_thread::slot::CNODE
                    .cap()
                    .absolute_cptr(sel4::cap::Endpoint::from_bits(bits));
                let _ = ticket.delete();
            }
        } else {
            *slot = Some(export);
        }
    }
    sel4::debug_println!(
        "SLIME_GRAPH native task_caps={} exports={} tickets={}",
        graph.len(),
        exports.len(),
        exports.len(),
    );
    sel4::debug_println!(
        "SLIME_GRAPH capabilities exports={} imports={} cancels={} finalized={} outstanding={} tickets={}",
        exports.exported,
        exports.imported,
        exports.cancelled,
        exports.finalized,
        exports.len(),
        exports.len(),
    );
    // The channel plane's own accounting, kept on its own line so P5.2's
    // terminal marker keeps the exact shape its gate already asserts.
    //
    sel4::debug_println!(
        "SLIME_GRAPH loans served={loans_served} loans={} mappings={} regions={} orphans={} aliases={} quotas={}",
        buffers.loan_count(),
        buffers.mapping_count(),
        buffers.live_count(),
        buffers.orphan_count(),
        buffer_adapter::live_frame_aliases(),
        buffers.quota_count(),
    );
    sel4::debug_println!(
        "SLIME_GRAPH spawns served={spawns} drops={drops} terminated={}",
        terminations.recorded(),
    );
    sel4::debug_println!(
        "SLIME_ROOT allocator live_slots={} live_objects={} live_bytes={} slot_reuses={} arena_reuses={}",
        allocator.live_slots(),
        allocator.live_objects(),
        allocator.live_bytes(),
        allocator.slots_reused(),
        allocator.arena_reuses(),
    );
    let completed = completed_required.iter().filter(|done| **done).count();
    if live == 0 && required != 0 && completed == required {
        sel4::debug_println!(
            "SLIME_GRAPH HEALTHY generation={} required={} live=0 completed={} failed=0",
            generation.number,
            required,
            completed,
        );
    }
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
    /// Root-mediated capabilities only. Native endpoint and notification
    /// bindings are installed separately into the child's CSpace.
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
        graph::Resource::SharedBufferFactory => RIGHT_BUFFER_CREATE | RIGHT_TRANSFER,
        graph::Resource::Supervision { .. } => RIGHT_SUPERVISE | RIGHT_TRANSFER,
        graph::Resource::NativeEndpoint => RIGHT_SEND | RIGHT_RECV,
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
        // Native Endpoint capabilities are materialized by the root from this
        // declaration after construction; they never cross the spawn request.
        if declared.rights & (RIGHT_SEND | RIGHT_RECV) == 0 {
            each(DeclaredCapability::Granted(binding.slot, declared))?;
        }
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
        let declared = generation
            .grant(binding.grant)
            .map_err(|_| IpcError::BadCapability)?;
        below +=
            usize::from(declared.rights & (RIGHT_SEND | RIGHT_RECV) == 0 && binding.slot < slot);
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
    launched: &LaunchedInstances,
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
    // The child's spawn-supplied set is its non-endpoint grant-backed bindings
    // plus the capabilities its owner mints at runtime. Native Endpoints are
    // generation-declared too, but the root materializes them directly after
    // construction, so the parent neither supplies nor can counterfeit them.
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
        if grant.rights & (RIGHT_SEND | RIGHT_RECV) == 0
            && (grant.source != GrantEndpoint::Instance(child_instance)
                || grant.target != GrantEndpoint::Instance(child_instance))
        {
            parent_supplied += 1;
        }
    }
    // The declared set describes a launch: every declaration, in ascending
    // destination-slot order, matched positionally against the request. A
    // *respawn* — the same instance, after the first died and was collected —
    // may instead bring nothing at all, which is what a supervisor retrying a
    // child it can no longer equip does (B51).
    //
    // Nothing, not fewer. The matching is positional: request N binds to the
    // declaration with the Nth-lowest destination slot, so a partial request
    // would silently install the caller's first capability at some other
    // declaration's slot with that declaration's rights ceiling. An empty
    // request has no such ambiguity, and a full one is checked exactly as a
    // first launch is.
    //
    // The count rule itself is unchanged for every first launch, which is what
    // B39 and B40 added it for.
    let declared_total = parent_supplied + minted_count;
    let respawn = launched.ever_launched(child_instance);
    if count != declared_total && !(respawn && count == 0) {
        sel4::debug_println!(
            "SLIME_GRAPH spawn preflight instance={} reason=declared-count requested={count} bindings={parent_supplied} minted={minted_count} respawn={}",
            child.name,
            u8::from(respawn),
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
    windows: &mut WindowTable<MAX_WINDOW_ENTRIES>,
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
            // As the boot path: a spawned child is a declared instance, so its
            // priority comes from the same plan, and is recorded for the same
            // reason -- a priority nothing reports is indistinguishable from
            // the constant it replaced (B48).
            {
                let priority = match generation.instance_priority(plan.instance) {
                    Ok(Some(priority)) => sel4::Word::from(priority),
                    Ok(None) => task::CHILD_PRIORITY,
                    Err(_) => return Err(IpcError::BadCapability),
                };
                sel4::debug_println!(
                    "SLIME_GRAPH schedule instance={} priority={priority} default={}",
                    generation
                        .instance(plan.instance)
                        .map_or("<unknown>", |record| record.name),
                    task::CHILD_PRIORITY,
                );
                priority
            },
            // As the boot path: the thread count comes from the same plan, so
            // a spawned instance declaring a worker gets one (B47).
            match generation.instance_threads(plan.instance) {
                Ok(Some(threads)) => threads,
                Ok(None) => 1,
                Err(_) => return Err(IpcError::BadCapability),
            },
            // As the boot path: each worker's own declared priority (B48).
            {
                let main_priority = match generation.instance_priority(plan.instance) {
                    Ok(Some(priority)) => sel4::Word::from(priority),
                    Ok(None) => task::CHILD_PRIORITY,
                    Err(_) => return Err(IpcError::BadCapability),
                };
                let mut priorities = [main_priority; child_vspace::MAX_CHILD_THREADS];
                for (thread_index, slot) in priorities.iter_mut().enumerate().skip(1) {
                    match generation.thread_priority(plan.instance, thread_index) {
                        Ok(Some(priority)) => *slot = sel4::Word::from(priority),
                        Ok(None) => {}
                        Err(_) => return Err(IpcError::BadCapability),
                    }
                }
                priorities
            },
        )
        .map_err(|_| IpcError::DestinationSlotsExhausted)?;

    let Some(task) = tasks.get(id) else {
        release_child(tasks, windows, graph, buffers, allocator, id);
        return Err(IpcError::DestinationSlotsExhausted);
    };
    for (thread, pages) in task
        .vspace
        .pages
        .iter()
        .take(task.vspace.threads)
        .enumerate()
    {
        if windows
            .declare(
                id,
                thread,
                pages.transfer_window_addr,
                pages.transfer_window,
                pages.transfer_window_alias,
            )
            .is_err()
        {
            release_child(tasks, windows, graph, buffers, allocator, id);
            return Err(IpcError::DestinationSlotsExhausted);
        }
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
        let Some((_, destination, capability, _minted)) = granted else {
            continue;
        };
        if child_table.install(*destination, *capability).is_err() {
            release_child(tasks, windows, graph, buffers, allocator, id);
            return Err(IpcError::DestinationSlotsExhausted);
        }
    }

    // so no parent passes it and the loop above never sees it. The root is
    // the only party that can install it, and preflight has already excluded
    // these from the count the parent must satisfy.
    let Ok(child) = generation.instance(plan.instance) else {
        release_child(tasks, windows, graph, buffers, allocator, id);
        return Err(IpcError::BadCapability);
    };
    // Counts the block devices placed into *this* child, in declaration order.
    let mut block_index = 0u8;
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
        // Same construction the boot path uses for a declared binding,
        // including its device renumbering: `declared_resource` answers
        // `Block { device: 0 }` for every block grant, because only the
        // installer knows how many it has already placed. A component holding
        // two device capabilities would otherwise see both resolve to device
        // 0, and its second device would silently be its first.
        let Some((_, resource)) = declared_resource(grant.rights) else {
            continue;
        };
        let resource = match resource {
            graph::Resource::Block { .. } => {
                let device = block_index;
                block_index = block_index.saturating_add(1);
                graph::Resource::Block { device }
            }
            other => other,
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

/// Tear a partially constructed child back down.
///
/// This is the single unwind owner after `TaskTable::create` succeeds. Each
/// resource release is idempotent, so every later failure can call it without
/// tracking which construction stages completed. Quota is released before the
/// task identity becomes unreachable, and the task's object span is revoked
/// last.
fn release_child(
    tasks: &mut TaskTable<MAX_TASKS>,
    windows: &mut WindowTable<MAX_WINDOW_ENTRIES>,
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
    windows: &mut WindowTable<MAX_WINDOW_ENTRIES>,
    graph: &mut GraphTables,
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
    let frame = match transfer_window::read_staged_array(
        windows.bound(id, descriptor_thread(words[1])),
        words[1],
        words,
        scratch,
    ) {
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
        launched,
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
    let (child_arena, child_cnode, child_cnode_bits) = match tasks.get(child) {
        Some(task) => (task.cleanup.arena, task.cnode, task.cnode_size_bits),
        None => {
            release_child(tasks, windows, graph, buffers, allocator, child);
            return Response::error(IpcError::DestinationSlotsExhausted);
        }
    };
    let copied = match unsafe { &*ptr::addr_of!(PEER_ENDPOINTS) }.install_instance(
        generation,
        plan.instance,
        child,
        allocator,
        child_arena,
        child_cnode,
        child_cnode_bits,
    ) {
        Ok(installed) => installed,
        Err(error) => {
            release_child(tasks, windows, graph, buffers, allocator, child);
            sel4::debug_println!(
                "SLIME_GRAPH spawn failed task={} component={name} error=EndpointInstall({error:?})",
                id.0
            );
            return Response::error(IpcError::BadCapability);
        }
    };
    let notification_copied = match unsafe { &*ptr::addr_of!(NOTIFICATIONS) }.install_instance(
        generation,
        plan.instance,
        child,
        allocator,
        child_arena,
        child_cnode,
        child_cnode_bits,
    ) {
        Ok(installed) => installed,
        Err(error) => {
            release_child(tasks, windows, graph, buffers, allocator, child);
            sel4::debug_println!(
                "SLIME_GRAPH spawn failed task={} component={name} error=NotificationInstall({error:?})",
                id.0
            );
            return Response::error(IpcError::BadCapability);
        }
    };

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
        "SLIME_GRAPH spawned task={} child={} component={name} grants={} endpoints={copied} notifications={notification_copied} handle={handle}",
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

/// Return a dead task's own objects: its VSpace, image frames, CNode, and TCB.
///
fn record_termination(
    terminations: &mut supervision::Terminations,
    graph: &GraphTables,
    child: TaskId,
    termination: supervision::Termination,
) {
    if terminations.record(child, termination) {
        return;
    }
    let freed = supervision::sweep(terminations, graph);
    if !terminations.record(child, termination) {
        sel4::debug_println!(
            "SLIME_GRAPH FAIL termination lost task={} reason=records-full",
            child.0
        );
    } else {
        sel4::debug_println!(
            "SLIME_GRAPH supervision swept freed={freed} live={}",
            terminations.len()
        );
    }
}

fn reclaim_dead_task(buffers: &mut SharedBufferTable, allocator: &mut ObjectAllocator, id: TaskId) {
    let holder = HolderId(u64::from(id.0));
    let charged = buffers.holder_buffers(holder)
        + buffers.holder_mappings(holder)
        + buffers.holder_loans(holder);
    if charged != 0 {
        let mut adapter = BufferAdapter::new(allocator);
        match buffers.reclaim_holder(&mut adapter, holder) {
            Ok(actions) => sel4::debug_println!(
                "SLIME_GRAPH holder reclaimed task={} charges={charged} actions={}",
                id.0,
                actions.len()
            ),
            Err(error) => sel4::debug_println!(
                "SLIME_GRAPH holder reclaim incomplete task={} class={}",
                id.0,
                buffer_error_class(error)
            ),
        }
    }
    if buffers.release_quota(holder) {
        sel4::debug_println!(
            "SLIME_GRAPH quota released task={} live={}",
            id.0,
            buffers.quota_count()
        );
    }
}

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

fn capability_kind(resource: &graph::Resource) -> u32 {
    use slime_proto::capability_transfer::*;
    match resource {
        graph::Resource::SharedBuffer { .. } => OBJECT_KIND_SHARED_BUFFER,
        graph::Resource::Loan { .. } => OBJECT_KIND_SHARED_BUFFER_LOAN,
        graph::Resource::Supervision { .. } => OBJECT_KIND_SUPERVISION,
        graph::Resource::Directory { .. } => OBJECT_KIND_DIRECTORY,
        graph::Resource::NativeEndpoint => OBJECT_KIND_ENDPOINT,
        _ => 0,
    }
}

fn serve_capability_export(
    generation: &Generation<'_>,
    launched: &LaunchedInstances,
    graph: &mut GraphTables,
    allocator: &mut ObjectAllocator,
    tasks: &TaskTable<MAX_TASKS>,
    sender: TaskId,
    words: &[sel4::Word; ipc::FAST_MESSAGE_REGISTERS],
) -> Response {
    let carrier = words[0] as u32;
    let source_slot = (words[0] >> 32) as u32;
    let expected_kind = words[1] as u32;
    let retain = words[1] >> 32 != 0;
    let rights = words[3];
    let Some(sender_instance) = launched.instance_for_task(sender) else {
        return Response::error(IpcError::BadCapability);
    };
    let Some(sender_task) = tasks.get(sender) else {
        return Response::error(IpcError::BadCapability);
    };
    let Some(receiver) = unsafe { &*ptr::addr_of!(PEER_ENDPOINTS) }.receiver_for(
        generation,
        sender_instance,
        carrier,
        launched,
    ) else {
        return Response::error(IpcError::BadCapability);
    };

    let (capability, source_endpoint) = if expected_kind
        == slime_proto::capability_transfer::OBJECT_KIND_ENDPOINT
    {
        let Some((endpoint, side, transferable)) = unsafe { &*ptr::addr_of!(PEER_ENDPOINTS) }
            .endpoint_for(generation, sender_instance, source_slot)
        else {
            return Response::error(IpcError::BadCapability);
        };
        let declared = match side {
            peer_endpoint::Side::Producer => RIGHT_SEND,
            peer_endpoint::Side::Consumer => RIGHT_RECV,
            peer_endpoint::Side::Both => RIGHT_SEND | RIGHT_RECV,
        } | if transferable { RIGHT_TRANSFER } else { 0 };
        if !transferable
            || rights == 0
            || rights & !(RIGHT_SEND | RIGHT_RECV) != 0
            || rights & declared != rights
        {
            sel4::debug_println!(
                "SLIME_GRAPH endpoint export rejected task={} source_slot={} carrier={} rights={rights:#x} declared={declared:#x} transferable={}",
                sender.0,
                source_slot,
                carrier,
                u8::from(transferable)
            );
            return Response::error(IpcError::BadCapability);
        }
        (
            graph::Capability {
                resource: graph::Resource::NativeEndpoint,
                rights,
            },
            Some(endpoint),
        )
    } else {
        let Some(table) = graph.get_mut(sender) else {
            return Response::error(IpcError::BadCapability);
        };
        let Some(source) = table.get(source_slot) else {
            return Response::error(IpcError::BadCapability);
        };
        let kind = capability_kind(&source.resource);
        if kind == 0
            || kind != expected_kind
            || !source.allows(RIGHT_TRANSFER)
            || rights == 0
            || rights & !source.rights != 0
            || rights & !valid_rights(&source.resource) != 0
        {
            return Response::error(IpcError::BadCapability);
        }
        if !retain {
            table.drop_slot(source_slot);
        }
        (
            graph::Capability {
                resource: source.resource,
                rights,
            },
            None,
        )
    };

    let exports = unsafe { &mut *ptr::addr_of_mut!(CAPABILITY_EXPORTS) };
    let Some(slot) = exports.entries.iter_mut().find(|entry| entry.is_none()) else {
        return Response::error(IpcError::DestinationSlotsExhausted);
    };
    if tasks.get(receiver).is_none() {
        return Response::error(IpcError::BadCapability);
    }
    // A native Endpoint crosses as a real kernel capability: the root mints a
    // ticket at the narrowed rights and places it in the sender's authority
    // region, from where the sender's own IPC carries it. Every other kind is
    // a root-owned logical capability with no kernel object the peer could
    // hold, so it crosses as a table entry the receiver claims with
    // `CapabilityImport`. The two are distinguished here and nowhere else.
    let ticket = match source_endpoint {
        Some(endpoint) => {
            let cap_rights = match (rights & RIGHT_SEND != 0, rights & RIGHT_RECV != 0) {
                (true, true) => sel4::CapRights::all(),
                (true, false) => sel4::CapRightsBuilder::none()
                    .write(true)
                    .grant_reply(true)
                    .build(),
                (false, true) => sel4::CapRightsBuilder::none().read(true).build(),
                (false, false) => return Response::error(IpcError::BadCapability),
            };
            let ticket_slot = match allocator.reserve_slot::<sel4::cap_type::Endpoint>() {
                Ok(ticket) => ticket,
                Err(_) => return Response::error(IpcError::DestinationSlotsExhausted),
            };
            let ticket = sel4::init_thread::slot::CNODE
                .cap()
                .absolute_cptr(ticket_slot.cap());
            if ticket
                .mint(
                    &sel4::init_thread::slot::CNODE.cap().absolute_cptr(endpoint),
                    cap_rights.clone(),
                    0,
                )
                .is_err()
            {
                return Response::error(IpcError::BadCapability);
            }
            let sender_ticket_slot =
                task::CHILD_SLOT_AUTHORITY_BASE + source_slot as sel4::CPtrBits;
            if sender_task
                .cnode
                .absolute_cptr_from_bits_with_depth(sender_ticket_slot, sender_task.cnode_size_bits)
                .copy(&ticket, cap_rights)
                .is_err()
            {
                return Response::error(IpcError::BadCapability);
            }
            Some(ticket_slot.cptr().bits())
        }
        None => None,
    };
    let id = exports.next_id;
    exports.next_id = exports.next_id.checked_add(1).unwrap_or(1);
    *slot = Some(CapabilityExport {
        id,
        sender,
        receiver,
        capability,
        ticket,
        retain,
        finalized: false,
    });
    exports.exported = exports.exported.saturating_add(1);
    sel4::debug_println!(
        "SLIME_GRAPH capability exported task={} id={} kind={} rights={rights:#x} retain={}",
        sender.0,
        id,
        capability.resource.kind_name(),
        u8::from(retain)
    );
    Response::success(i64::from(id), 0)
}

fn serve_capability_finalize(
    sender: TaskId,
    words: &[sel4::Word; ipc::FAST_MESSAGE_REGISTERS],
) -> Response {
    let id = words[0] as u32;
    let exports = unsafe { &mut *ptr::addr_of_mut!(CAPABILITY_EXPORTS) };
    let Some(export) = exports.get_mut(id) else {
        return Response::success(0, 0);
    };
    if export.sender != sender {
        return Response::error(IpcError::BadCapability);
    }
    if !export.finalized {
        export.finalized = true;
        exports.finalized = exports.finalized.saturating_add(1);
    }
    Response::success(0, 0)
}

fn serve_capability_import(
    graph: &mut GraphTables,
    receiver: TaskId,
    words: &[sel4::Word; ipc::FAST_MESSAGE_REGISTERS],
) -> Response {
    let id = words[0] as u32;
    let exports = unsafe { &mut *ptr::addr_of_mut!(CAPABILITY_EXPORTS) };
    // Id zero means "the oldest finalized export addressed to me". A logical
    // capability crosses with no kernel object, so its receiver has no ticket
    // to name and the 64-byte descriptor has no field to carry an id in.
    // Ordering makes the claim unambiguous anyway: exports are recorded in the
    // order their senders finalized them, and a sender finalizes before its
    // message is sent, so the Nth delegation a receiver observes on an
    // endpoint is the Nth finalized export addressed to it.
    let id = if id == 0 {
        let Some(oldest) = exports
            .entries
            .iter()
            .flatten()
            .filter(|entry| entry.receiver == receiver && entry.finalized)
            .map(|entry| entry.id)
            .min()
        else {
            return Response::error(IpcError::BadCapability);
        };
        oldest
    } else {
        id
    };
    let Some(export) = exports.get_mut(id) else {
        return Response::error(IpcError::BadCapability);
    };
    if export.receiver != receiver || !export.finalized {
        return Response::error(IpcError::BadCapability);
    }
    let export = *export;
    let _ = exports.remove(id);
    let Some(table) = graph.get_mut(receiver) else {
        return Response::error(IpcError::BadCapability);
    };
    let Some(slot) = table.free_slot_from(1) else {
        return Response::error(IpcError::DestinationSlotsExhausted);
    };
    if table.install(slot, export.capability).is_err() {
        return Response::error(IpcError::DestinationSlotsExhausted);
    }
    exports.imported = exports.imported.saturating_add(1);
    sel4::debug_println!(
        "SLIME_GRAPH capability imported task={} id={} kind={} rights={:#x} retain={}",
        receiver.0,
        id,
        export.capability.resource.kind_name(),
        export.capability.rights,
        u8::from(export.retain)
    );
    Response::success(i64::from(slot), 0)
}

fn serve_capability_cancel(
    graph: &mut GraphTables,
    tasks: &TaskTable<MAX_TASKS>,
    sender: TaskId,
    words: &[sel4::Word; ipc::FAST_MESSAGE_REGISTERS],
) -> Response {
    let id = words[0] as u32;
    let exports = unsafe { &mut *ptr::addr_of_mut!(CAPABILITY_EXPORTS) };
    let Some(export) = exports.remove(id) else {
        return Response::error(IpcError::BadCapability);
    };
    if export.sender != sender || export.finalized {
        return Response::error(IpcError::BadCapability);
    }
    if let Some(receiver) = tasks.get(export.receiver) {
        let _ = receiver
            .cnode
            .absolute_cptr_from_bits_with_depth(task::CHILD_SLOT_RECEIVE, receiver.cnode_size_bits)
            .delete();
    }
    if !export.retain {
        let Some(table) = graph.get_mut(sender) else {
            return Response::error(IpcError::BadCapability);
        };
        let Some(slot) = table.free_slot_from(1) else {
            return Response::error(IpcError::DestinationSlotsExhausted);
        };
        if table.install(slot, export.capability).is_err() {
            return Response::error(IpcError::DestinationSlotsExhausted);
        }
    }
    exports.cancelled = exports.cancelled.saturating_add(1);
    Response::success(0, 0)
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
    generation: &Generation<'_>,
    launched: &LaunchedInstances,
    buffers: &mut SharedBufferTable,
    allocator: &mut ObjectAllocator,
    graph: &mut GraphTables,
    id: TaskId,
    words: &[sel4::Word; ipc::FAST_MESSAGE_REGISTERS],
    served: &mut usize,
) -> Response {
    let lender = HolderId(u64::from(id.0));
    let buffer_slot = (words[0] & 0xffff_ffff) as u32;
    let receiver_slot = (words[0] >> 32) as u32;
    let offset = words[1] as usize;
    // Bit 63 of the length word asks for a writable loan (B46). The length
    // itself is bounded by the region, so the high bit is free; the table
    // still refuses unless the lender holds `WRITE` on an unsealed region.
    let writable = words[2] >> 63 != 0;
    let length = (words[2] & !(1 << 63)) as usize;

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
    // A slot holding nothing and a slot holding real authority of another kind
    // are refused identically: which one it was is not the caller's business,
    // and distinguishing them would let a component map its own table by
    // probing. One marker covers both for the same reason.
    //
    // **Two kinds resolve here**, and the difference is which question the
    // caller is answering.
    //
    // A `Supervision` handle names its subject outright: it was minted by the
    // spawn that created that task and names nothing else, ever. That is how
    // the retired kernel does it, and it is what `sample-lender` — unmodified
    // — passes at `RECEIVER_SLOT`.
    //
    // A **declared native endpoint** names its peer. The generation fixed both
    // ends of that edge before either task ran, so the slot identifies exactly
    // one counterpart and the caller cannot point it elsewhere. This is what
    // breaks the stream plane's ordering cycle (B46): the fabric loans a ring
    // to each participant while `fabric-publisher-b` loans its large sample
    // back to the fabric, so requiring supervision in both directions would
    // need each to be spawned before the other.
    //
    // Neither widens the other. A supervision handle is authority over a task
    // the caller *created*; an endpoint is authority over a task the
    // generation *connected* it to. Both name the receiver through a
    // capability rather than an ambient task id, which is what the exit
    // condition asks for.
    let by_supervision = graph.get(id).and_then(|table| {
        table
            .resolve(receiver_slot, RIGHT_SUPERVISE)
            .ok()
            .and_then(|capability| match capability.resource {
                graph::Resource::Supervision { task } => Some(task),
                _ => None,
            })
    });
    let resolved = by_supervision.or_else(|| {
        let sender_instance = launched.instance_for_task(id)?;
        unsafe { &*ptr::addr_of!(PEER_ENDPOINTS) }.receiver_for(
            generation,
            sender_instance,
            receiver_slot,
            launched,
        )
    });
    let Some(peer) = resolved else {
        sel4::debug_println!(
            "SLIME_GRAPH loan refused task={} slot={receiver_slot} class=absent",
            id.0
        );
        return Response::error(IpcError::BadCapability);
    };
    if peer == id {
        return Response::error(IpcError::BadCapability);
    }
    let receiver = HolderId(u64::from(peer.0));

    // The table decides: it holds the region's rights, its sealed state, the
    // range, and the lender's `loan_count` ceiling. Nothing is re-checked here
    // that it already checks, so there is one place a loan can be refused.
    let handle = match buffers.loan(lender, receiver, handle, offset, length, writable) {
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
                    rights: RIGHT_BUFFER_MAP
                        | RIGHT_TRANSFER
                        | if writable { RIGHT_BUFFER_WRITE } else { 0 },
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
            // A placeholder for the same reason as `receiver`: `revoke_loan`
            // resolves the recorded loan by id, and the record owns both.
            writable: false,
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
        // The clean-exit fixture's shared-buffer report. The root records what
        // the child claims and answers immediately; adjudication happens once,
        // after the fixture has finished, in `report_buffer_phase`.
        Operation::FixtureDirective => {
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
