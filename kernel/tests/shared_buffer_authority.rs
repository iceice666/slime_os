#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

//! C7.2 shared-buffer factory authority and allocation invariants.
//!
//! Exercises the exit condition from `roadmap/02-core-runtime.md` (C7.2):
//! a factory-authorized holder creates and releases a kernel-identified shared
//! buffer within fixed global bounds; an unauthorized component is denied,
//! exhaustion is structured and isolated, and no derivation widens authority.
//!
//! These run under QEMU against a brought-up kernel (so the physical frame
//! allocator is live). The shared-buffer table draws real contiguous frames.

#![allow(clippy::bool_assert_comparison)]

extern crate alloc;

use slime_os_kernel::capability::{
    CapError, Capability, CapabilityTable, DmaRegion, KernelObject, RIGHT_BUFFER_CREATE,
    RIGHT_BUFFER_MAP, RIGHT_BUFFER_WRITE, RIGHT_DMA_RELEASE, RIGHT_ENDPOINT_CREATE, RIGHT_SEND,
    RIGHT_TRANSFER,
};
use slime_os_kernel::memory::PhysAddr;
use slime_os_kernel::memory::shared_buffer::{
    MAX_BUFFER_PAGES, MAX_SHARED_BUFFERS, MAX_TOTAL_PAGES, SharedBufferError, SharedBufferTable,
};
use slime_os_kernel::{gdt, interrupts, memory};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    slime_os_kernel::limine::ensure_linked();
    unsafe { slime_os_kernel::boot::init_from_limine() };
    gdt::init();
    interrupts::init();
    memory::init();
    test_main();
    slime_os_kernel::hlt_loop()
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    slime_os_kernel::test_panic_handler(info)
}

/// A factory capability with only the creation right an unauthorized holder
/// lacks. This is the authority a generation grant would mint.
fn factory(rights: u64) -> Capability {
    Capability {
        object: KernelObject::SharedBufferFactory,
        rights,
    }
}

/// The capability table only accepts a factory carrying its object-specific
/// creation right; a component whose grant omits `RIGHT_BUFFER_CREATE` (or
/// carries a foreign right) cannot even install the factory, so it has no path
/// to allocate. This is the unauthorized-denial arm of the exit condition.
#[test_case]
fn factory_rights_are_object_specific() {
    let mut table = CapabilityTable::new();
    assert!(table.insert(factory(RIGHT_BUFFER_CREATE)).is_ok());

    let mut table = CapabilityTable::new();
    assert!(matches!(
        table.insert(factory(RIGHT_BUFFER_CREATE | RIGHT_ENDPOINT_CREATE)),
        Err(CapError::BadRights)
    ));
    let mut table = CapabilityTable::new();
    assert!(matches!(
        table.insert(factory(RIGHT_BUFFER_WRITE)),
        Err(CapError::BadRights)
    ));
}

/// A factory capability cannot be narrowed into any buffer-operation right:
/// holding creation authority never widens into write or map authority over a
/// specific buffer. Distinct authority, enforced at derive time.
#[test_case]
fn factory_does_not_grant_buffer_operations() {
    let cap = factory(RIGHT_BUFFER_CREATE | RIGHT_TRANSFER);
    assert!(cap.derive(RIGHT_BUFFER_CREATE).is_ok());
    assert!(cap.derive(RIGHT_BUFFER_WRITE).is_err());
    assert!(cap.derive(RIGHT_BUFFER_MAP).is_err());
}

/// A created buffer carries only buffer-operation rights, and derivation
/// narrows without widening or inventing rights. Creation authority is never
/// present on the buffer handle itself.
#[test_case]
fn buffer_rights_narrow_only() {
    let mut buffers = SharedBufferTable::new();
    let region = buffers.create(1, true).expect("create writable buffer");
    let cap = Capability {
        object: KernelObject::SharedBuffer(region.clone()),
        rights: RIGHT_BUFFER_WRITE | RIGHT_BUFFER_MAP | RIGHT_TRANSFER,
    };
    // Narrow to read-only map authority.
    let narrowed = cap.derive(RIGHT_BUFFER_MAP).expect("narrow to map");
    assert_eq!(narrowed.rights, RIGHT_BUFFER_MAP);
    // Cannot invent creation authority on the buffer handle.
    assert!(cap.derive(RIGHT_BUFFER_CREATE).is_err());
    // The table rejects a buffer capability carrying creation authority.
    let mut table = CapabilityTable::new();
    assert!(matches!(
        table.insert(Capability {
            object: KernelObject::SharedBuffer(region.clone()),
            rights: RIGHT_BUFFER_CREATE,
        }),
        Err(CapError::BadRights)
    ));
    buffers.release(&region).expect("cleanup");
}

/// A factory-authorized holder creates and releases a kernel-identified buffer
/// within bounds; identities are unforgeable and monotonic, and release
/// returns the page and object charges.
#[test_case]
fn create_and_release_round_trip() {
    let mut buffers = SharedBufferTable::new();
    assert_eq!(buffers.live_count(), 0);
    assert_eq!(buffers.total_pages(), 0);

    let a = buffers.create(2, true).expect("create a");
    let b = buffers.create(3, false).expect("create b");
    assert_ne!(a.id(), b.id(), "identities are distinct");
    assert_eq!(a.pages(), 2);
    assert_eq!(b.writable(), false);
    assert_eq!(buffers.live_count(), 2);
    assert_eq!(buffers.total_pages(), 5);

    buffers.release(&a).expect("release a");
    assert_eq!(buffers.live_count(), 1);
    assert_eq!(buffers.total_pages(), 3);

    // A second release of the same region fails structurally.
    assert!(matches!(
        buffers.release(&a),
        Err(SharedBufferError::NotFound)
    ));

    // A newly created buffer gets a fresh identity, never a recycled one.
    let c = buffers.create(1, true).expect("create c");
    assert_ne!(c.id(), a.id());
    assert_ne!(c.id(), b.id());

    buffers.release(&b).expect("release b");
    buffers.release(&c).expect("release c");
    assert_eq!(buffers.total_pages(), 0);
}

/// Zero-page and oversized requests are rejected structurally before any frame
/// is pulled.
#[test_case]
fn bad_sizes_rejected() {
    let mut buffers = SharedBufferTable::new();
    assert!(matches!(
        buffers.create(0, true),
        Err(SharedBufferError::BadSize)
    ));
    // Exceeding the per-buffer page cap is a structural BadSize, before any
    // frame is pulled.
    assert!(matches!(
        buffers.create(MAX_BUFFER_PAGES + 1, true),
        Err(SharedBufferError::BadSize)
    ));
    assert_eq!(buffers.live_count(), 0);
    assert_eq!(buffers.total_pages(), 0);
}

/// The object-count ceiling is enforced independently of the byte ceiling and
/// exhaustion does not disturb an unrelated live holder.
#[test_case]
fn object_exhaustion_is_structured_and_isolated() {
    let mut buffers = SharedBufferTable::new();
    let mut live = alloc::vec::Vec::new();
    for _ in 0..MAX_SHARED_BUFFERS {
        live.push(buffers.create(1, true).expect("fill table"));
    }
    let survivor_id = live[0].id();
    assert!(matches!(
        buffers.create(1, true),
        Err(SharedBufferError::ObjectsExhausted)
    ));
    // The unrelated holder is untouched by the failed allocation.
    assert_eq!(live[0].id(), survivor_id);
    assert_eq!(buffers.live_count(), MAX_SHARED_BUFFERS);
    for region in &live {
        buffers.release(region).expect("cleanup");
    }
}

/// The global page ceiling is enforced independently of the object count, and
/// a rejected over-budget request leaves an existing holder's charge intact.
#[test_case]
fn byte_exhaustion_is_structured_and_isolated() {
    let mut buffers = SharedBufferTable::new();
    // Fill the global page budget with modest buffers. Small contiguous runs
    // are reliably available under QEMU fragmentation; the count stays well
    // under the object ceiling so byte exhaustion is what bites — proving the
    // two ceilings are independent.
    let chunk = 16;
    let full = MAX_TOTAL_PAGES / chunk;
    assert!(full < MAX_SHARED_BUFFERS, "byte budget must bite first");
    let mut live = alloc::vec::Vec::new();
    for _ in 0..full {
        live.push(buffers.create(chunk, true).expect("fill budget"));
    }
    assert_eq!(buffers.total_pages(), MAX_TOTAL_PAGES);
    let before = buffers.total_pages();
    let live_before = buffers.live_count();
    // Even a single-page request now crosses the ceiling.
    assert!(matches!(
        buffers.create(1, true),
        Err(SharedBufferError::BytesExhausted)
    ));
    // The existing holders' charges are unchanged; nothing leaked or dropped.
    assert_eq!(buffers.total_pages(), before);
    assert_eq!(buffers.live_count(), live_before);
    for region in &live {
        buffers.release(region).expect("cleanup");
    }
    assert_eq!(buffers.total_pages(), 0);
}

/// DMA authority and shared-sample authority are distinct capability kinds:
/// neither object accepts the other's rights.
#[test_case]
fn dma_and_shared_sample_authority_are_distinct() {
    let mut table = CapabilityTable::new();
    // A DMA region capability cannot carry a shared-buffer right.
    assert!(matches!(
        table.insert(Capability {
            object: KernelObject::DmaMemory(DmaRegion::new(PhysAddr(0x1000), 1)),
            rights: RIGHT_BUFFER_WRITE,
        }),
        Err(CapError::BadRights)
    ));
    // A DMA region only accepts its own release right.
    let mut table = CapabilityTable::new();
    assert!(
        table
            .insert(Capability {
                object: KernelObject::DmaMemory(DmaRegion::new(PhysAddr(0x1000), 1)),
                rights: RIGHT_DMA_RELEASE,
            })
            .is_ok()
    );
    // A shared-buffer factory cannot carry endpoint or send authority.
    let mut table = CapabilityTable::new();
    assert!(matches!(
        table.insert(factory(RIGHT_SEND)),
        Err(CapError::BadRights)
    ));
}
