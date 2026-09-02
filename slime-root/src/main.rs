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
    buffer_adapter, child_vspace, clock, console, cspace, device, directory, event, fault,
    generation, graph, ipc, launched, lifecycle, notification, object_allocator, peer_endpoint,
    platform_timer, private_memory, scheduling, shared_buffer, supervision, task, timer,
    transfer_window, vm_attributes, wait_set,
};

use core::ptr;

use boot_contracts::generation::{
    CapabilityKind, Generation, Grant, GrantEndpoint, Instance, InstanceHealth, InstanceOwner,
    KIND_RESOURCE, MintedBinding,
};
use boot_contracts::private_memory_budget::{self, PrivateMemoryBudget};
use boot_contracts::shared_buffer_budget::{self as budget_magic, SharedBufferBudget};
use sel4_root_task::root_task;

use boot_contracts::generation::{RIGHT_EXEC, RIGHT_RECV, RIGHT_SEND, RIGHT_TRANSFER};
use buffer_adapter::BufferAdapter;
use child_vspace::{ChildImage, GRANULE_SIZE, ScratchPage};
#[cfg(slime_boot_selector)]
use device::MAX_BLOCK_DEVICES;
use event::{TaskEpoch, TimerId};
use fault::{LifecycleEventKind, SupervisionTable};
use generation::{Admission, Authority, bound_authority};
use ipc::{IpcError, Response, poll_notification};
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
use slime_proto::syscall_abi::{
    capability_table_labels, capability_transfer_labels, clock_labels, directory_labels,
    fixture_labels, lifecycle_labels, scheduling_labels, shared_buffer_labels, spawn_labels,
    supervision_labels,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BufferLifecycleRequest {
    Map,
    Unmap,
    Seal,
    Release,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoanLifecycleRequest {
    Map,
    Return,
    Revoke,
}

/// Report an unrecoverable startup condition and park the root task. Every
/// fallible step returns a typed error that ends up here; nothing panics.
macro_rules! fatal {
    ($($arg:tt)*) => {{
        sel4::debug_println!("SLIME_ROOT FATAL {}", format_args!($($arg)*));
        sel4::init_thread::suspend_self()
    }};
}
mod graph_runtime;
use graph_runtime::private_memory_cause;
#[cfg(not(slime_root_fixture))]
use graph_runtime::{RootEndpoints, RuntimeDevices, launch_instance_graph};

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

#[cfg(slime_boot_selector)]
const BOOT_BUNDLE_IDENTITY: [u8; 32] = decode_hex32(env!("SLIME_BOOT_BUNDLE_IDENTITY"));

/// The target profile this root task admits executables for. The build harness
/// states it explicitly for the root, child, generation, and loader together;
/// admission then compares every serialized target axis before mapping bytes.
const TARGET_PROFILE: &str = env!("SLIME_TARGET_PROFILE");

/// The native child fixture, built for this platform's seL4 userspace target.
/// Supplied by the build harness; see `slime-root/child`. `include_bytes!` only guarantees
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
/// shared-buffer report, two supervised protection faults, and — since C10.1 —
/// its private-memory phase: five growth operations (two size queries, two
/// growths, one refused) plus that phase's own report. Eleven for the clean-exit
/// fixture, so sixteen leaves the same surplus the earlier eight did, bounding
/// unexpected traffic so the loop cannot spin forever.
const MAX_SERVICE_ITERATIONS: usize = 16;

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

/// Run one IRQ-backed deadline and name the phase in every marker.
fn prove_timer(timer_adapter: &mut PhysicalTimerAdapter, phase: &str) {
    let mut timer_scheduler = TimerScheduler::<1>::new();
    const TIMER_PROOF_OWNER: TaskEpoch = TaskEpoch::new(0, 0);
    let timer_start = match timer_adapter.monotonic_now() {
        Ok(now) => now,
        Err(error) => fatal!("timer clock unreadable during {phase}: {error:?}"),
    };
    let deadline_ticks = (timer_adapter.frequency_hz() / TIMER_PROOF_DEADLINE_DIVISOR).max(1);
    let (_, scheduled) =
        match timer_scheduler.schedule_after(TIMER_PROOF_OWNER, timer_start, deadline_ticks) {
            Ok(scheduled) => scheduled,
            Err(error) => fatal!("timer proof deadline rejected during {phase}: {error:?}"),
        };
    if let Err(error) = apply_deadline_programming(timer_adapter, scheduled.programming) {
        fatal!("timer deadline could not be programmed during {phase}: {error:?}")
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
            Err(error) => fatal!("timer clock unreadable while waiting during {phase}: {error:?}"),
        };
        if elapsed > bound_ticks {
            fatal!(
                "SLIME_TIMER FAIL phase={phase} timeout waited_ticks={elapsed} bound_ticks={bound_ticks} polls={polls}"
            )
        }
        polls += 1;
    }
    if phase == "startup" {
        sel4::debug_println!(
            "SLIME_TIMER delivered badge={:#x} polls={polls}",
            timer_adapter.signal_badge(),
        );
    } else {
        sel4::debug_println!(
            "SLIME_TIMER phase={phase} delivered badge={:#x} polls={polls}",
            timer_adapter.signal_badge(),
        );
    }

    let drained = match timer_scheduler.service_timer_source(timer_adapter, |_| true) {
        Ok(transition) => transition,
        Err(ServiceTimerError::Program { error, transition }) => fatal!(
            "timer deadline reprogramming failed during {phase}: {error:?} wakes={}",
            transition.events.len()
        ),
        Err(ServiceTimerError::Acknowledge { error, transition }) => fatal!(
            "timer acknowledgement failed during {phase}: {error:?} wakes={}",
            transition.events.len()
        ),
        Err(error) => fatal!("timer service rejected the observed {phase} expiry: {error:?}"),
    };
    let timer_end = match timer_adapter.monotonic_now() {
        Ok(now) => now,
        Err(error) => fatal!("timer clock unreadable after {phase} service: {error:?}"),
    };
    if phase == "startup" {
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
    } else {
        sel4::debug_println!(
            "SLIME_TIMER phase={phase} serviced events={} programming={:?}",
            drained.events.len(),
            drained.programming,
        );
        sel4::debug_println!(
            "SLIME_TIMER phase={phase} advanced start={} end={} delta={}",
            timer_start.0,
            timer_end.0,
            timer_end.0.wrapping_sub(timer_start.0),
        );
        sel4::debug_println!("SLIME_TIMER phase={phase} OK");
    }
}

#[cfg(slime_duo_early_fault)]
fn run_duo_early_fault_control(
    timer_adapter: &mut PhysicalTimerAdapter,
    reset_registers: device::MappedGranule,
) {
    let refused = timer_adapter.program_deadline(event::MonotonicInstant(u64::MAX));
    if !matches!(
        refused,
        Err(platform_timer::PlatformTimerAckError::RegisterAccess)
    ) {
        fatal!("Duo early-fault control did not refuse an out-of-range RTC deadline")
    }
    sel4::debug_println!(
        "SLIME_DUO EARLY_FAULT phase=post-timer cause=timer-range-refused bounded=1"
    );
    sel4::debug_println!("SLIME_DUO reset request kind=cold");
    if !timer_adapter.request_cold_reset(reset_registers) {
        fatal!("CV1800B cold-reset register access failed after early fault")
    }
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(slime_cv1800b_duo)]
fn request_duo_cold_reset(
    timer_registers: device::MappedGranule,
    reset_registers: device::MappedGranule,
) -> ! {
    sel4::debug_println!("SLIME_DUO reset request kind=cold");
    if !platform_timer::request_cv1800b_cold_reset(timer_registers, reset_registers) {
        fatal!("CV1800B cold-reset register access failed")
    }
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(slime_duo_test_terminator)]
fn request_duo_test_reset() -> ! {
    sel4::debug_println!("SLIME_DUO test terminator accepted");
    // SAFETY: root startup writes both `Some` values before the console thread
    // starts; the two MMIO mappings remain live for the root's lifetime.
    let timer = unsafe { ptr::addr_of!(DUO_TIMER_REGISTERS).read() };
    let reset = unsafe { ptr::addr_of!(DUO_RESET_REGISTERS).read() };
    match (timer, reset) {
        (Some(timer), Some(reset)) => request_duo_cold_reset(timer, reset),
        _ => fatal!("Duo test reset registers unavailable"),
    }
}

#[cfg(slime_duo_uart)]
const fn const_parse_hex_usize(value: &str) -> usize {
    let bytes = value.as_bytes();
    assert!(
        bytes.len() > 2 && bytes[0] == b'0' && bytes[1] == b'x',
        "hex prefix required"
    );
    let mut index = 2;
    let mut parsed = 0usize;
    while index < bytes.len() {
        let digit = match bytes[index] {
            b'0'..=b'9' => bytes[index] - b'0',
            b'a'..=b'f' => bytes[index] - b'a' + 10,
            b'A'..=b'F' => bytes[index] - b'A' + 10,
            _ => panic!("invalid hexadecimal integer"),
        };
        parsed = parsed * 16 + digit as usize;
        index += 1;
    }
    assert!(parsed != 0, "integer must be nonzero");
    parsed
}

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

/// Standing window for the selected product terminal receiver.
///
/// QEMU maps PL011 at `0x0900_0000`; the Duo product maps UART0 at the physical
/// address supplied from the pinned board profile. Plane images map neither.
#[cfg(any(slime_qemu_keyboard, slime_duo_uart))]
static mut PRODUCT_UART_PAGE: FreePage = FreePage([0; GRANULE_SIZE]);

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

/// Standing window for the platform's memory-mapped timer registers, on the
/// profiles whose monotonic source is a device rather than an architected
/// register the kernel grants userspace access to.
#[cfg(any(target_arch = "riscv64", target_arch = "x86_64"))]
static mut TIMER_PAGE: FreePage = FreePage([0; GRANULE_SIZE]);

/// Standing window for the CV1800B RTC control granule used to reset the board
/// after an autonomous physical proof completes. It must be mapped before the
/// timer granule: both come from one device untyped and retype is monotonic.
#[cfg(slime_cv1800b_duo)]
static mut RESET_PAGE: FreePage = FreePage([0; GRANULE_SIZE]);
#[cfg(slime_duo_test_terminator)]
static mut DUO_RESET_REGISTERS: Option<device::MappedGranule> = None;
#[cfg(slime_duo_test_terminator)]
static mut DUO_TIMER_REGISTERS: Option<device::MappedGranule> = None;

/// Standing MMIO windows for the userspace-authority inventory, one per
/// granule in QEMU's virtio-mmio transport range.
///
/// The scan page is transient; a granule containing an attached transport must
/// remain mapped until its generation-declared userspace driver binds it.
#[cfg(not(slime_boot_selector))]
static mut AUTHORITY_MMIO_PAGES: [FreePage; VIRTIO_MMIO_GRANULES] =
    [const { FreePage([0; GRANULE_SIZE]) }; VIRTIO_MMIO_GRANULES];

#[cfg(slime_boot_selector)]
static mut BOOT_MMIO_PAGES: [FreePage; MAX_BLOCK_DEVICES] =
    [const { FreePage([0; GRANULE_SIZE]) }; MAX_BLOCK_DEVICES];
#[cfg(slime_boot_selector)]
static mut BOOT_QUEUE_PAGES: [FreePage; MAX_BLOCK_DEVICES] =
    [const { FreePage([0; GRANULE_SIZE]) }; MAX_BLOCK_DEVICES];
#[cfg(slime_boot_selector)]
static mut BOOT_BUFFER_PAGES: [FreePage; MAX_BLOCK_DEVICES] =
    [const { FreePage([0; GRANULE_SIZE]) }; MAX_BLOCK_DEVICES];

#[cfg(all(target_arch = "riscv64", not(slime_cv1800b_duo)))]
const TIMER_PADDR: usize = 0x0010_1000;
#[cfg(all(target_arch = "riscv64", slime_cv1800b_duo))]
const TIMER_PADDR: usize = 0x0502_6000;
/// The IA-PC HPET's architectural base address on QEMU q35.
///
/// A pinned machine fact, not discovery: firmware reports it in the ACPI HPET
/// table, and P6 reads no ACPI (H1 owns the real inventory). This is the
/// address that machine's HPET is fixed at, and a machine whose firmware
/// relocates it is a different platform profile.
#[cfg(target_arch = "x86_64")]
const TIMER_PADDR: usize = 0xfed0_0000;
#[cfg(slime_cv1800b_duo)]
const RESET_PADDR: usize = 0x0502_5000;
#[cfg(slime_duo_uart)]
const DUO_UART_PADDR: usize = const_parse_hex_usize(env!("SLIME_DUO_UART_PADDR"));
/// QEMU virt's architecture-specific virtio-mmio transport window.
///
/// These are pinned machine facts, not discovery. The generation's userspace
/// driver owns device semantics; root uses the constants only for the bounded
/// bootstrap inventory and IRQ-capability handoff.
///
/// QEMU q35 has no virtio-mmio window at all: virtio devices there are PCI
/// functions behind an ACPI-described host bridge. P6 explicitly does not
/// enumerate PCI or enable bus mastering — that is H2's — so this profile
/// declares an empty transport range and its bootstrap inventory finds no
/// devices. The range is stated as zero granules rather than omitted so the
/// scan is a real bounded loop over nothing instead of a separate code path.
#[cfg(target_arch = "aarch64")]
const VIRTIO_MMIO_BASE: usize = 0x0a00_0000;
#[cfg(target_arch = "riscv64")]
const VIRTIO_MMIO_BASE: usize = 0x1000_1000;
#[cfg(target_arch = "x86_64")]
const VIRTIO_MMIO_BASE: usize = 0;
/// Bytes between consecutive transports.
#[cfg(target_arch = "aarch64")]
const VIRTIO_MMIO_STRIDE: usize = 0x200;
#[cfg(any(target_arch = "riscv64", target_arch = "x86_64"))]
const VIRTIO_MMIO_STRIDE: usize = 0x1000;
const VIRTIO_MMIO_SLOTS_PER_GRANULE: usize = GRANULE_SIZE / VIRTIO_MMIO_STRIDE;
/// Number of transport granules to scan.
#[cfg(target_arch = "aarch64")]
const VIRTIO_MMIO_GRANULES: usize = 4;
#[cfg(target_arch = "riscv64")]
const VIRTIO_MMIO_GRANULES: usize = 8;
#[cfg(target_arch = "x86_64")]
const VIRTIO_MMIO_GRANULES: usize = 0;
/// Interrupt number of the first transport in seL4's IRQ namespace.
#[cfg(target_arch = "aarch64")]
const VIRTIO_MMIO_FIRST_IRQ: sel4::Word = 48;
#[cfg(target_arch = "riscv64")]
const VIRTIO_MMIO_FIRST_IRQ: sel4::Word = 1;
/// Unreachable on this profile: no transport exists to derive an IRQ for.
#[cfg(target_arch = "x86_64")]
const VIRTIO_MMIO_FIRST_IRQ: sel4::Word = 0;
/// Badge the device notification carries, distinct from the timer's.
const VIRTIO_IRQ_BADGE: sel4::Word = 0x2;

const fn virtio_irq(paddr: usize) -> sel4::Word {
    let index = ((paddr - VIRTIO_MMIO_BASE) / VIRTIO_MMIO_STRIDE) as sel4::Word;
    VIRTIO_MMIO_FIRST_IRQ + index
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

/// Growth operations the private-memory phase must actually charge a page to.
///
/// The phase issues five: two size queries, two growths, one refused. Only the
/// two growths are grants, which is the distinction a page total cannot make on
/// its own.
const MEM_EXPECTED_GRANTS: usize = 2;

/// The value the clean-exit fixture writes into its first private page and
/// re-reads after a further growth. `slime-root/child/src/main.rs::MEM_PATTERN`.
const MEM_PATTERN: u64 = 0x4d454d5f42415345;

/// Marks a report as the private-memory phase's rather than the shared-buffer
/// phase's; both arrive on the same label.
/// `slime-root/child/src/main.rs::MEM_REPORT_TAG`.
const MEM_REPORT_TAG: sel4::Word = 0x4d454d5f52505449;

/// Report flags the private-memory phase sets.
/// `slime-root/child/src/main.rs::REPORT_MEM_*`.
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
static mut OBJECT_ALLOCATOR: core::mem::MaybeUninit<ObjectAllocator> =
    core::mem::MaybeUninit::uninit();

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

    #[cfg(all(any(slime_qemu_keyboard, slime_duo_uart), not(slime_root_fixture)))]
    let product_input = {
        let uart_addr = ptr::addr_of!(PRODUCT_UART_PAGE) as usize;
        if let Err(error) = ScratchPage::claim(bootinfo, uart_addr) {
            fatal!("product input page unavailable: {error:?}")
        }
        #[cfg(slime_qemu_keyboard)]
        let (paddr, receiver) = {
            let registers = match device::DeviceRegion::map(
                allocator,
                sel4::init_thread::slot::VSPACE.cap(),
                uart_addr,
                device::QEMU_PL011_PADDR,
            ) {
                Ok(registers) => registers,
                Err(error) => fatal!("QEMU keyboard UART unavailable: {error:?}"),
            };
            (
                device::QEMU_PL011_PADDR,
                device::TerminalReceiver::Pl011(device::Pl011Input::new(registers)),
            )
        };
        #[cfg(slime_duo_uart)]
        let (paddr, receiver) = {
            let registers = match device::DeviceRegion::map(
                allocator,
                sel4::init_thread::slot::VSPACE.cap(),
                uart_addr,
                DUO_UART_PADDR,
            ) {
                Ok(registers) => registers,
                Err(error) => fatal!("Duo product UART unavailable: {error:?}"),
            };
            (
                DUO_UART_PADDR,
                device::TerminalReceiver::DwApb(device::DwApbInput::new(registers)),
            )
        };
        sel4::debug_println!("SLIME_ROOT product input ready uart={paddr:#x}");
        let input = device::TerminalInput::new(receiver);
        #[cfg(slime_duo_test_terminator)]
        let input = input.with_test_terminator(0x1d, request_duo_test_reset);
        Some(input)
    };
    #[cfg(all(not(any(slime_qemu_keyboard, slime_duo_uart)), not(slime_root_fixture)))]
    let product_input: Option<device::TerminalInput> = None;
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
    #[cfg(slime_cv1800b_duo)]
    let reset_registers = {
        let reset_addr = ptr::addr_of!(RESET_PAGE) as usize;
        if let Err(error) = ScratchPage::claim(bootinfo, reset_addr) {
            fatal!("reset page unavailable: {error:?}")
        }
        match device::DeviceRegion::map(
            allocator,
            sel4::init_thread::slot::VSPACE.cap(),
            reset_addr,
            RESET_PADDR,
        ) {
            Ok(region) => region.granule(),
            Err(error) => fatal!("reset registers unavailable: {error:?}"),
        }
    };
    #[cfg(all(slime_cv1800b_duo, not(slime_duo_uart)))]
    let timer_registers;
    #[cfg(target_arch = "riscv64")]
    {
        let timer_addr = ptr::addr_of!(TIMER_PAGE) as usize;
        if let Err(error) = ScratchPage::claim(bootinfo, timer_addr) {
            fatal!("timer page unavailable: {error:?}")
        }
        let registers = match device::DeviceRegion::map(
            allocator,
            sel4::init_thread::slot::VSPACE.cap(),
            timer_addr,
            TIMER_PADDR,
        ) {
            Ok(region) => region.granule(),
            Err(error) => fatal!("timer registers unavailable: {error:?}"),
        };
        #[cfg(all(slime_cv1800b_duo, not(slime_duo_uart)))]
        {
            timer_registers = registers;
        }
        #[cfg(slime_duo_test_terminator)]
        unsafe {
            ptr::addr_of_mut!(DUO_TIMER_REGISTERS).write(Some(registers));
            ptr::addr_of_mut!(DUO_RESET_REGISTERS).write(Some(reset_registers));
        }
        timer_adapter.attach_registers(registers);
    }
    #[cfg(target_arch = "x86_64")]
    {
        let timer_addr = ptr::addr_of!(TIMER_PAGE) as usize;
        if let Err(error) = ScratchPage::claim(bootinfo, timer_addr) {
            fatal!("timer page unavailable: {error:?}")
        }
        let registers = match device::DeviceRegion::map(
            allocator,
            sel4::init_thread::slot::VSPACE.cap(),
            timer_addr,
            TIMER_PADDR,
        ) {
            Ok(region) => region.granule(),
            Err(error) => fatal!("timer registers unavailable: {error:?}"),
        };
        timer_adapter.attach_registers(registers);
    }
    sel4::debug_println!(
        "SLIME_TIMER acquired irq={TIMER_IRQ} freq_hz={}",
        timer_adapter.frequency_hz(),
    );

    prove_timer(&mut timer_adapter, "startup");
    #[cfg(slime_duo_early_fault)]
    run_duo_early_fault_control(&mut timer_adapter, reset_registers);
    if let Err(error) = timer_adapter.bind_to(sel4::init_thread::slot::TCB.cap()) {
        fatal!("timer notification could not bind to root: {error:?}")
    }
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

    // ---- device phase ----
    //
    // The immutable boot selector is the one ordering exception: it must read
    // the generation from the boot disk before decoded generation policy can
    // assign that device. Ordinary embedded-generation images never construct
    // the retired root block driver; after admission, an IO-resource budget
    // grants raw MMIO/IRQ/DMA authority to supervised userspace drivers.
    #[cfg(slime_boot_selector)]
    let mut block_devices = graph_runtime::platform::probe_devices(bootinfo, allocator);
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
    #[cfg(all(not(slime_boot_selector), not(slime_root_fixture)))]
    let mut io_authority = if admission.io_resource_drivers.is_some() {
        graph_runtime::probe_authority_devices(bootinfo, allocator)
    } else {
        graph_runtime::platform::AuthorityInventory::new()
    };
    #[cfg(all(slime_boot_selector, not(slime_root_fixture)))]
    let mut io_authority = graph_runtime::platform::AuthorityInventory::new();
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
    // B70. The hop *count* above says a chain exists; it cannot say which
    // component the generation put on it. Both fabric brokers used to answer
    // that from a build-time table compiled into the component — a table that
    // could only agree with itself. The name is a generation fact, so the root
    // that admitted the generation is what states it, and the plane gates
    // assert against this line rather than against a constant in the component
    // they are checking.
    for name in generation::interposition_hop_names(&generation)
        .iter()
        .flatten()
    {
        sel4::debug_println!("SLIME_ROOT fabric interposition hop={name}");
    }
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
            RootEndpoints {
                service: service_endpoint,
                console: console_endpoint,
            },
            RuntimeDevices {
                timer: &mut timer_adapter,
                #[cfg(slime_boot_selector)]
                boot_blocks: &mut block_devices,
                input: product_input,
                io_authority: &mut io_authority,
            },
            #[cfg(slime_boot_selector)]
            &mut boot_runtime,
        );
        #[cfg(all(slime_cv1800b_duo, not(slime_duo_uart)))]
        request_duo_cold_reset(timer_registers, reset_registers);
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
            // The C10.1 private-memory quota, granted only to the clean-exit
            // fixture, which is the one that runs the growth phase.
            //
            // A compiled-in ceiling *for this phase alone*, on exactly the
            // grounds `SHARED_QUOTA` above records: the fixture is an ELF this
            // root embeds at compile time, not a declared component, so no
            // generation resource names it and there is no budget to read. Every
            // declared instance sits at zero until C10.2 supplies that
            // resource. The deliberate-fault fixture gets nothing, which is what
            // makes the deny-by-default arm observable on the same boot.
            if role == Role::CleanExit {
                PRIVATE_QUOTA_PAGES
            } else {
                0
            },
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
        sel4::debug_println!(
            "SLIME_MEM teardown grown={} reclaimed={} pages=0",
            table.grown_pages(),
            table.reclaimed_pages(),
        );
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

mod fixture_runtime;
use fixture_runtime::{report_buffer_phase, report_memory_phase, serve, setup_shared_region};
