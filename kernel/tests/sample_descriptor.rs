#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

//! C7.6 versioned sample-descriptor invariants.
//!
//! Exercises the exit condition from `roadmap/02-core-runtime.md` (C7.6): a
//! receiver validates a bounded versioned descriptor, maps only the exact loaned
//! bytes, and observes a payload larger than the control-message bound; every
//! malformed descriptor fails before mapping or allocation. The descriptor is a
//! fixed control message that fits the existing channel bound (it never widens
//! `MAX_MSG`), while the payload bytes stay in the shared buffer and never copy
//! through the kernel message queue. These tests use real x86-64 page tables and
//! shared-buffer frames under QEMU, composed over the C7.5 loan lifecycle.

extern crate alloc;

use slime_os_kernel::ipc::{MAX_MSG, Message};
use slime_os_kernel::memory::address_space::AddressSpace;
use slime_os_kernel::memory::shared_buffer::{HolderQuota, SharedBufferTable};
use slime_os_kernel::memory::{PAGE_SIZE, PhysAddr};
use slime_os_kernel::{gdt, interrupts, memory};
use slime_proto::sample_descriptor::{
    CAPABILITY_KIND_LOAN, DESCRIPTOR_LEN, FLAG_LAST, FORMAT_VERSION, KNOWN_FLAGS, MAX_SAMPLE_BYTES,
    SAMPLE_DESCRIPTOR_MAGIC, WireSampleDescriptor,
};
use slime_proto::valid_sample_descriptor;

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

const LENDER: u64 = 0xD6;
const RECEIVER: u64 = 0xE6;
const BASE_RECEIVER: u64 = 0x0000_0005_0000_0000;
const TYPE_ID: u64 = 0x5359_4E43_5459_5045; // "SYNCTYPE"
const PAGE: u64 = PAGE_SIZE as u64;

fn quota(pages: u32, buffers: u32, mappings: u32, loans: u32) -> HolderQuota {
    HolderQuota {
        byte_pages: pages,
        buffer_count: buffers,
        mapping_count: mappings,
        loan_count: loans,
    }
}

fn admitted(loan_id: u64, offset: u64, length: u64) -> WireSampleDescriptor {
    WireSampleDescriptor {
        magic: SAMPLE_DESCRIPTOR_MAGIC,
        version: FORMAT_VERSION,
        flags: FLAG_LAST,
        capability_kind: CAPABILITY_KIND_LOAN,
        loan_id,
        offset,
        length,
        type_identity: TYPE_ID,
        sequence: 1,
        reserved: [0; 8],
    }
}

/// The descriptor fits the existing channel control-message bound exactly, so
/// it can be sent through an ordinary channel without widening `MAX_MSG`.
#[test_case]
fn descriptor_fits_the_channel_message_bound() {
    assert_eq!(DESCRIPTOR_LEN, MAX_MSG);
}

/// An admitted descriptor round-trips byte-identically through the wire form;
/// unsupported versions, unknown flags, and short buffers fail closed.
#[test_case]
fn descriptor_round_trips_and_rejects_unsupported_versions_and_flags() {
    let descriptor = admitted(7, PAGE, PAGE);
    let encoded = descriptor.encode();
    assert_eq!(encoded.len(), DESCRIPTOR_LEN);
    assert_eq!(WireSampleDescriptor::decode(&encoded), Some(descriptor));
    assert_eq!(
        WireSampleDescriptor::decode(&encoded).unwrap().encode(),
        encoded
    );
    assert!(WireSampleDescriptor::decode(&encoded[..DESCRIPTOR_LEN - 1]).is_none());
    assert!(valid_sample_descriptor(&descriptor, 7, TYPE_ID, PAGE));

    let bad_version = WireSampleDescriptor {
        version: FORMAT_VERSION + 1,
        ..descriptor
    };
    assert!(!valid_sample_descriptor(&bad_version, 7, TYPE_ID, PAGE));

    let unknown_flag = WireSampleDescriptor {
        flags: KNOWN_FLAGS | (1 << 31),
        ..descriptor
    };
    assert!(!valid_sample_descriptor(&unknown_flag, 7, TYPE_ID, PAGE));

    let bad_magic = WireSampleDescriptor {
        magic: SAMPLE_DESCRIPTOR_MAGIC ^ 1,
        ..descriptor
    };
    assert!(!valid_sample_descriptor(&bad_magic, 7, TYPE_ID, PAGE));

    let dirty_reserved = WireSampleDescriptor {
        reserved: [1; 8],
        ..descriptor
    };
    assert!(!valid_sample_descriptor(&dirty_reserved, 7, TYPE_ID, PAGE));
}

/// Overflowed offset/length, wrong capability kind, stale loan identity, and a
/// mismatched type identity are all rejected by validation before the receiver
/// maps or allocates anything.
#[test_case]
fn malformed_descriptors_fail_before_mapping() {
    let loan_id = 42;
    // Overflowing offset + length.
    let overflow = WireSampleDescriptor {
        offset: u64::MAX - (PAGE - 1),
        length: PAGE,
        ..admitted(loan_id, 0, PAGE)
    };
    assert!(!valid_sample_descriptor(&overflow, loan_id, TYPE_ID, PAGE));

    // Length beyond the descriptor's bounded sample ceiling.
    let too_large = admitted(loan_id, 0, MAX_SAMPLE_BYTES as u64 + PAGE);
    assert!(!valid_sample_descriptor(&too_large, loan_id, TYPE_ID, PAGE));

    // Misaligned offset/length and a zero length.
    for (offset, length) in [(1, PAGE), (0, PAGE + 1), (0, 0)] {
        let misaligned = admitted(loan_id, offset, length);
        assert!(!valid_sample_descriptor(
            &misaligned,
            loan_id,
            TYPE_ID,
            PAGE
        ));
    }

    // Wrong capability kind (not a loan reference).
    let wrong_kind = WireSampleDescriptor {
        capability_kind: CAPABILITY_KIND_LOAN + 1,
        ..admitted(loan_id, 0, PAGE)
    };
    assert!(!valid_sample_descriptor(
        &wrong_kind,
        loan_id,
        TYPE_ID,
        PAGE
    ));

    // Stale/wrong loan identity: descriptor names a different loan than held.
    let stale = admitted(loan_id, 0, PAGE);
    assert!(!valid_sample_descriptor(&stale, loan_id + 1, TYPE_ID, PAGE));
    // A zero loan identity is never valid.
    assert!(!valid_sample_descriptor(
        &admitted(0, 0, PAGE),
        0,
        TYPE_ID,
        PAGE
    ));

    // Mismatched type identity.
    assert!(!valid_sample_descriptor(&stale, loan_id, TYPE_ID ^ 1, PAGE));
    // A zero type identity is never valid.
    let zero_type = WireSampleDescriptor {
        type_identity: 0,
        ..stale
    };
    assert!(!valid_sample_descriptor(&zero_type, loan_id, 0, PAGE));
}

/// End-to-end: a lender fills a sealed multi-page buffer with a payload larger
/// than `MAX_MSG`, loans the exact region, and encodes a descriptor into a
/// single control message. The receiver validates the descriptor, maps only the
/// loaned bytes, and reconstructs the full payload from the shared buffer — the
/// message queue only ever carried the 64-byte descriptor, never the payload.
#[test_case]
fn receiver_observes_payload_larger_than_message_bound() {
    let mut buffers = SharedBufferTable::new();
    let receiver_space = AddressSpace::new().expect("receiver address space");
    let lender = quota(4, 1, 0, 1);
    let receiver = quota(0, 0, 2, 0);

    // Two pages == 8192 bytes, well beyond the 64-byte control-message bound.
    let pages = 2usize;
    let payload_len = pages * PAGE_SIZE;
    assert!(payload_len > MAX_MSG);
    let region = buffers.create(LENDER, lender, pages, true).expect("buffer");

    // Lender writes a recognizable payload directly into the buffer frames
    // before sealing. This models the producer filling the sample plane.
    // SAFETY: `region` names `pages` contiguous frames reachable through HHDM.
    let producer = unsafe {
        core::slice::from_raw_parts_mut(region.phys().to_virt().as_mut_ptr::<u8>(), payload_len)
    };
    for (index, byte) in producer.iter_mut().enumerate() {
        *byte = (index % 251) as u8;
    }

    buffers.seal(&region).expect("seal");
    let loan = buffers
        .loan(LENDER, RECEIVER, lender, &region, 0, payload_len as u64)
        .expect("loan");

    // The descriptor is the only thing that crosses the channel. It fits one
    // control message; the payload never enters the queue.
    let descriptor = admitted(loan.id(), 0, payload_len as u64);
    let message = Message {
        bytes: descriptor.encode(),
        len: DESCRIPTOR_LEN,
        caps: core::array::from_fn(|_| None),
    };
    assert_eq!(message.len, MAX_MSG);

    // Receiver side: decode the control message and validate before mapping.
    let received =
        WireSampleDescriptor::decode(&message.bytes).expect("descriptor decodes from message");
    assert!(valid_sample_descriptor(&received, loan.id(), TYPE_ID, PAGE));

    // Map only the exact loaned bytes into the receiver's address space.
    buffers
        .map_loan(
            RECEIVER,
            receiver,
            received.loan_id,
            loan.region(),
            receiver_space.pml4(),
            BASE_RECEIVER,
            received.offset,
            received.length,
        )
        .expect("receiver maps loaned bytes");

    // Every mapped page names the exact loaned buffer frame, read-only.
    for page in 0..pages {
        let virt = BASE_RECEIVER + (page * PAGE_SIZE) as u64;
        assert_eq!(
            receiver_space.user_translation(virt),
            Some(PhysAddr(region.phys().0 + (page * PAGE_SIZE) as u64)),
            "mapping must name the exact loaned buffer page"
        );
        assert!(receiver_space.user_range_mapped(virt, PAGE_SIZE, false));
        assert!(
            !receiver_space.user_range_mapped(virt, PAGE_SIZE, true),
            "loaned mapping must never be writable"
        );
    }

    // Reconstruct the full payload from the shared buffer via the mapping. The
    // whole >MAX_MSG payload is observed even though the queue only moved 64
    // descriptor bytes.
    for index in 0..payload_len {
        let virt = BASE_RECEIVER + index as u64;
        let phys = receiver_space
            .user_translation(virt & !(PAGE - 1))
            .expect("mapped page");
        // SAFETY: translation proved this physical byte is a live loaned frame.
        let observed = unsafe {
            (phys.to_virt().as_mut_ptr::<u8>())
                .add((index as u64 & (PAGE - 1)) as usize)
                .read()
        };
        assert_eq!(
            observed,
            (index % 251) as u8,
            "payload byte mismatch at {index}"
        );
    }

    buffers
        .return_loan(RECEIVER, received.loan_id, loan.region())
        .expect("return");
    // Returning settles the loan and reclaims the receiver mapping; the lender
    // still owns the buffer it never released.
    assert_eq!(buffers.owner_loans(LENDER), 0);
    assert_eq!(buffers.owner_mappings(RECEIVER), 0);
    assert_eq!(buffers.owner_pages(LENDER), pages as u32);
    assert!(!receiver_space.user_range_mapped(BASE_RECEIVER, payload_len, false));

    // The lender releasing its buffer returns every page and charge.
    buffers.release(&region).expect("cleanup lender");
    assert_eq!(buffers.total_pages(), 0);
    assert_eq!(buffers.live_count(), 0);
    assert_eq!(buffers.owner_pages(LENDER), 0);
}
