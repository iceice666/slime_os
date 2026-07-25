#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

//! C7.7 sample-plane integration and isolation.
//!
//! Composes the C7.2 factory allocation, C7.3 per-holder quotas, C7.4 mapping
//! and sealing, C7.5 loan/return lifecycle, and the C7.6 sample descriptor into
//! the two-component exit condition from `roadmap/02-core-runtime.md` (C7.7):
//! two isolated holders exchange and return a payload larger than the kernel
//! IPC message bound through a quota-charged shared buffer, while malformed
//! descriptors, every quota class, and peer death remain bounded, reclaim all
//! resources, and disturb neither an unrelated channel nor the retained v2
//! known-good decode path. Real x86-64 page tables, shared-buffer frames, and
//! IPC channels back every case under QEMU.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use boot_contracts::generation::{
    COMPONENT_LEN, FORMAT_VERSION_V2, GRANT_LEN, Generation, HEADER_LEN, KIND_BOOTSTRAP,
    KIND_KERNEL, MAGIC_V2, OBJECT_LEN, RIGHT_TRANSFER, ROLE_INIT, generation_identity,
};
use boot_contracts::sha256::Sha256;
use slime_os_kernel::capability::{Capability, CapabilityTable};
use slime_os_kernel::ipc::{self, MAX_CAPS_PER_MSG, MAX_MSG};
use slime_os_kernel::memory::address_space::AddressSpace;
use slime_os_kernel::memory::shared_buffer::{HolderQuota, SharedBufferError, SharedBufferTable};
use slime_os_kernel::memory::{PAGE_SIZE, PhysAddr};
use slime_os_kernel::{gdt, interrupts, memory};
use slime_proto::sample_descriptor::{
    CAPABILITY_KIND_LOAN, DESCRIPTOR_LEN, FLAG_LAST, FORMAT_VERSION, SAMPLE_DESCRIPTOR_MAGIC,
    WireSampleDescriptor,
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

const LENDER: u64 = 0x71;
const RECEIVER: u64 = 0x72;
const OTHER: u64 = 0x73;
const BASE_RECEIVER: u64 = 0x0000_0006_0000_0000;
const BASE_OTHER: u64 = 0x0000_0007_0000_0000;
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

fn no_caps() -> [Option<Capability>; MAX_CAPS_PER_MSG] {
    core::array::from_fn(|_| None)
}

/// End-to-end integration: a lender fills and seals a multi-page shared buffer,
/// loans the exact region, and sends only the 64-byte descriptor over a real
/// IPC channel. The receiver decodes and validates the descriptor from the
/// channel, maps only the loaned bytes, and reconstructs the whole payload —
/// which is larger than `MAX_MSG` — from the shared buffer. The channel only
/// ever moved the descriptor; the payload never entered the kernel queue.
#[test_case]
fn two_components_exchange_and_return_payload_over_message_bound() {
    let mut buffers = SharedBufferTable::new();
    let receiver_space = AddressSpace::new().expect("receiver address space");
    let lender = quota(4, 1, 0, 1);
    let receiver = quota(0, 0, 1, 0);

    // The descriptor crosses a real channel; the payload stays in the buffer.
    let (lender_ep, receiver_ep) = ipc::channel();
    let mut receiver_caps = CapabilityTable::new();

    let pages = 2usize;
    let payload_len = pages * PAGE_SIZE;
    assert!(payload_len > MAX_MSG);

    // Lender: allocate a quota-charged buffer, fill it, seal it, loan it.
    let region = buffers.create(LENDER, lender, pages, true).expect("buffer");
    assert_eq!(buffers.owner_pages(LENDER), pages as u32);
    assert_eq!(buffers.owner_buffers(LENDER), 1);
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
    assert_eq!(buffers.owner_loans(LENDER), 1);

    // Lender sends the descriptor; only 64 bytes enter the channel.
    let descriptor = admitted(loan.id(), 0, payload_len as u64);
    let sent = descriptor.encode();
    let mut send_caps = no_caps();
    assert_eq!(ipc::send(&lender_ep, &sent, &mut send_caps), 0);

    // Receiver reads exactly one control message from the channel.
    let mut buf = [0u8; MAX_MSG];
    let mut cap_out = [0u64; MAX_CAPS_PER_MSG];
    let received = ipc::recv(&receiver_ep, &mut buf, &mut cap_out, &mut receiver_caps);
    assert_eq!(received, DESCRIPTOR_LEN as i64);
    assert_eq!(received, MAX_MSG as i64);

    // Receiver validates before mapping or allocating any receiver state.
    let decoded = WireSampleDescriptor::decode(&buf).expect("descriptor decodes from channel");
    assert!(valid_sample_descriptor(&decoded, loan.id(), TYPE_ID, PAGE));

    buffers
        .map_loan(
            RECEIVER,
            receiver,
            decoded.loan_id,
            loan.region(),
            receiver_space.pml4(),
            BASE_RECEIVER,
            decoded.offset,
            decoded.length,
        )
        .expect("receiver maps loaned bytes");
    assert_eq!(buffers.owner_mappings(RECEIVER), 1);

    // Every mapped page names the exact loaned frame, read-only.
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

    // The receiver reconstructs the full >MAX_MSG payload from the buffer.
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

    // Returning the loan settles it and reclaims the receiver mapping; the
    // lender still owns the buffer it never released.
    buffers
        .return_loan(RECEIVER, decoded.loan_id, loan.region())
        .expect("return");
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

/// A malformed descriptor delivered over the channel is rejected before the
/// receiver maps or allocates anything, and the underlying loan and buffer are
/// untouched — the receiver can still map a well-formed descriptor afterward.
#[test_case]
fn malformed_descriptor_over_channel_maps_nothing() {
    let mut buffers = SharedBufferTable::new();
    let receiver_space = AddressSpace::new().expect("receiver address space");
    let lender = quota(2, 1, 0, 1);
    let receiver = quota(0, 0, 1, 0);

    let (lender_ep, receiver_ep) = ipc::channel();
    let mut receiver_caps = CapabilityTable::new();

    let region = buffers.create(LENDER, lender, 1, true).expect("buffer");
    buffers.seal(&region).expect("seal");
    let loan = buffers
        .loan(LENDER, RECEIVER, lender, &region, 0, PAGE)
        .expect("loan");

    // Lender sends a descriptor naming a stale loan identity.
    let stale = admitted(loan.id() ^ 1, 0, PAGE);
    let mut send_caps = no_caps();
    assert_eq!(ipc::send(&lender_ep, &stale.encode(), &mut send_caps), 0);

    let mut buf = [0u8; MAX_MSG];
    let mut cap_out = [0u64; MAX_CAPS_PER_MSG];
    assert_eq!(
        ipc::recv(&receiver_ep, &mut buf, &mut cap_out, &mut receiver_caps),
        DESCRIPTOR_LEN as i64
    );
    let decoded = WireSampleDescriptor::decode(&buf).expect("descriptor decodes");

    // Validation fails against the real loan the receiver holds, so the
    // receiver maps nothing.
    assert!(!valid_sample_descriptor(&decoded, loan.id(), TYPE_ID, PAGE));
    assert_eq!(buffers.owner_mappings(RECEIVER), 0);

    // Even if the receiver ignored validation, the loan-aware map path itself
    // rejects the stale identity before installing any page.
    assert!(matches!(
        buffers.map_loan(
            RECEIVER,
            receiver,
            decoded.loan_id,
            loan.region(),
            receiver_space.pml4(),
            BASE_RECEIVER,
            0,
            PAGE,
        ),
        Err(SharedBufferError::NotFound)
    ));
    assert_eq!(buffers.owner_mappings(RECEIVER), 0);
    assert!(!receiver_space.user_range_mapped(BASE_RECEIVER, PAGE_SIZE, false));

    // The loan is intact: a well-formed descriptor still maps and returns.
    let good = admitted(loan.id(), 0, PAGE);
    assert!(valid_sample_descriptor(&good, loan.id(), TYPE_ID, PAGE));
    buffers
        .map_loan(
            RECEIVER,
            receiver,
            good.loan_id,
            loan.region(),
            receiver_space.pml4(),
            BASE_RECEIVER,
            good.offset,
            good.length,
        )
        .expect("well-formed descriptor maps");
    assert_eq!(buffers.owner_mappings(RECEIVER), 1);
    buffers
        .return_loan(RECEIVER, good.loan_id, loan.region())
        .expect("return");
    buffers.release(&region).expect("cleanup");
    assert_eq!(buffers.total_pages(), 0);
}

/// Every per-holder quota class — byte pages, buffer count, mapping count, and
/// loan count — is enforced as a structured error, and none of the exhaustions
/// disturbs an unrelated owner's live buffer, mapping, or channel traffic.
#[test_case]
fn every_quota_class_is_bounded_and_isolated() {
    let mut buffers = SharedBufferTable::new();
    let other_space = AddressSpace::new().expect("unrelated address space");
    let map_space = AddressSpace::new().expect("mapping address space");
    let other = quota(4, 2, 2, 2);

    // Unrelated owner holds a live buffer + mapping and a live channel with a
    // message already in flight; nothing below may disturb it.
    let other_region = buffers
        .create(OTHER, other, 1, true)
        .expect("unrelated buffer");
    buffers
        .map(
            OTHER,
            other,
            &other_region,
            other_space.pml4(),
            BASE_OTHER,
            0,
            PAGE,
            true,
        )
        .expect("unrelated map");
    let (probe_tx, probe_rx) = ipc::channel();
    let mut probe_caps = CapabilityTable::new();
    let mut probe_send = no_caps();
    assert_eq!(ipc::send(&probe_tx, b"unrelated", &mut probe_send), 0);

    // byte-page quota: a two-page request under a one-page ceiling is rejected.
    assert!(matches!(
        buffers.create(0x101, quota(1, 4, 4, 4), 2, true),
        Err(SharedBufferError::QuotaExceeded)
    ));

    // buffer-count quota: the second buffer under a one-buffer ceiling fails.
    let buf_quota = quota(8, 1, 4, 4);
    let counted = buffers
        .create(0x102, buf_quota, 1, true)
        .expect("first buffer");
    assert!(matches!(
        buffers.create(0x102, buf_quota, 1, true),
        Err(SharedBufferError::QuotaExceeded)
    ));

    // mapping-count quota: the second mapping under a one-mapping ceiling fails.
    let map_quota = quota(8, 2, 1, 4);
    let mapped = buffers
        .create(0x103, map_quota, 1, true)
        .expect("mappable buffer");
    buffers
        .map(
            0x103,
            map_quota,
            &mapped,
            map_space.pml4(),
            0x0000_0008_0000_0000,
            0,
            PAGE,
            false,
        )
        .expect("first mapping");
    assert!(matches!(
        buffers.map(
            0x103,
            map_quota,
            &mapped,
            map_space.pml4(),
            0x0000_0008_0000_1000,
            0,
            PAGE,
            false,
        ),
        Err(SharedBufferError::QuotaExceeded)
    ));

    // loan-count quota: the second loan under a one-loan ceiling fails.
    let loan_quota = quota(8, 2, 4, 1);
    let lent = buffers
        .create(0x104, loan_quota, 1, true)
        .expect("loanable buffer");
    buffers.seal(&lent).expect("seal");
    buffers
        .loan(0x104, RECEIVER, loan_quota, &lent, 0, PAGE)
        .expect("first loan");
    assert!(matches!(
        buffers.loan(0x104, RECEIVER, loan_quota, &lent, 0, PAGE),
        Err(SharedBufferError::QuotaExceeded)
    ));

    // The unrelated owner is entirely undisturbed by every exhaustion.
    assert_eq!(buffers.owner_buffers(OTHER), 1);
    assert_eq!(buffers.owner_pages(OTHER), 1);
    assert_eq!(buffers.owner_mappings(OTHER), 1);
    assert!(other_space.user_range_mapped(BASE_OTHER, PAGE_SIZE, true));

    // The unrelated channel still delivers its queued message intact.
    let mut buf = [0u8; MAX_MSG];
    let mut cap_out = [0u64; MAX_CAPS_PER_MSG];
    assert_eq!(
        ipc::recv(&probe_rx, &mut buf, &mut cap_out, &mut probe_caps),
        b"unrelated".len() as i64
    );
    assert_eq!(&buf[..b"unrelated".len()], b"unrelated");

    // Clean up the live buffers the exhaustion scenarios left behind.
    buffers.release(&counted).expect("cleanup counted");
    buffers.release(&mapped).expect("cleanup mapped");
    // `lent` still has an outstanding loan, so a plain release would retain its
    // page until the loan settles; reclaiming the lender settles both.
    assert_eq!(buffers.reclaim_owner(0x104), 1);
    buffers.release(&other_region).expect("cleanup unrelated");
    assert_eq!(buffers.total_pages(), 0);
    assert_eq!(buffers.live_count(), 0);
}

/// Peer death on either side of an active loan settles every affected charge,
/// tears down the receiver's mapping, and leaves an unrelated owner's buffer
/// and an unrelated channel fully functional.
#[test_case]
fn peer_death_reclaims_all_and_preserves_unrelated_channel() {
    let mut buffers = SharedBufferTable::new();
    let receiver_space = AddressSpace::new().expect("receiver address space");
    let other_space = AddressSpace::new().expect("unrelated address space");
    let lender = quota(2, 1, 1, 1);
    let receiver = quota(0, 0, 1, 0);
    let other = quota(2, 1, 1, 0);

    // Unrelated owner + channel that must survive both reclaims.
    let unrelated = buffers
        .create(OTHER, other, 1, true)
        .expect("unrelated buffer");
    buffers
        .map(
            OTHER,
            other,
            &unrelated,
            other_space.pml4(),
            BASE_OTHER,
            0,
            PAGE,
            true,
        )
        .expect("unrelated map");
    let (probe_tx, probe_rx) = ipc::channel();
    let mut probe_caps = CapabilityTable::new();

    // Active loan with a live receiver mapping.
    let region = buffers.create(LENDER, lender, 1, true).expect("buffer");
    buffers.seal(&region).expect("seal");
    let loan = buffers
        .loan(LENDER, RECEIVER, lender, &region, 0, PAGE)
        .expect("loan");
    buffers
        .map_loan(
            RECEIVER,
            receiver,
            loan.id(),
            loan.region(),
            receiver_space.pml4(),
            BASE_RECEIVER,
            0,
            PAGE,
        )
        .expect("receiver map");

    // Receiver death settles only its loan and mapping; the lender retains its
    // buffer and the unrelated owner is untouched.
    assert_eq!(buffers.reclaim_owner(RECEIVER), 0);
    assert_eq!(buffers.owner_loans(LENDER), 0);
    assert_eq!(buffers.owner_mappings(RECEIVER), 0);
    assert_eq!(buffers.owner_pages(LENDER), 1);
    assert!(!receiver_space.user_range_mapped(BASE_RECEIVER, PAGE_SIZE, false));
    assert!(other_space.user_range_mapped(BASE_OTHER, PAGE_SIZE, true));

    // The unrelated channel still carries traffic after the reclaim.
    let mut probe_send = no_caps();
    assert_eq!(ipc::send(&probe_tx, b"alive", &mut probe_send), 0);
    let mut buf = [0u8; MAX_MSG];
    let mut cap_out = [0u64; MAX_CAPS_PER_MSG];
    assert_eq!(
        ipc::recv(&probe_rx, &mut buf, &mut cap_out, &mut probe_caps),
        b"alive".len() as i64
    );
    assert_eq!(&buf[..b"alive".len()], b"alive");

    // Lender death reclaims its retained buffer and every page charge.
    assert_eq!(buffers.reclaim_owner(LENDER), 1);
    assert_eq!(buffers.owner_pages(LENDER), 0);
    assert_eq!(buffers.owner_buffers(LENDER), 0);
    assert_eq!(buffers.total_pages(), 1); // only the unrelated buffer remains
    assert_eq!(buffers.owner_pages(OTHER), 1);
    assert!(other_space.user_range_mapped(BASE_OTHER, PAGE_SIZE, true));

    // The unrelated channel is still alive after the second reclaim.
    assert_eq!(ipc::send(&probe_tx, b"still", &mut probe_send), 0);
    assert_eq!(
        ipc::recv(&probe_rx, &mut buf, &mut cap_out, &mut probe_caps),
        b"still".len() as i64
    );
    assert_eq!(&buf[..b"still".len()], b"still");

    buffers.release(&unrelated).expect("cleanup unrelated");
    assert_eq!(buffers.total_pages(), 0);
    assert_eq!(buffers.live_count(), 0);
}

/// The retained v2 known-good decode path is orthogonal to the sample plane:
/// a v2 generation decodes identically before and after a full sample-plane
/// exchange, proving the C7.7 exercise does not perturb the rollback window.
#[test_case]
fn retained_v2_known_good_decode_is_unaffected() {
    let artifact = build_v2_known_good();

    // Decode before the exercise.
    let before = Generation::decode(&artifact).expect("v2 decodes before");
    assert_eq!(before.version, FORMAT_VERSION_V2);
    let before_identity = before.identity;
    let before_grant = before.grant(0).expect("grant present");
    let before_rights = before_grant.rights;
    assert!(before_grant.transferable);

    // Run a complete sample-plane exchange against a fresh table.
    run_sample_plane_exchange();

    // Decode again: the v2 known-good artifact is byte-for-byte unchanged.
    let after = Generation::decode(&artifact).expect("v2 decodes after");
    assert_eq!(after.version, FORMAT_VERSION_V2);
    assert_eq!(after.identity, before_identity);
    let after_grant = after.grant(0).expect("grant present");
    assert_eq!(after_grant.rights, before_rights);
    assert!(after_grant.transferable);
}

/// Exercise the full create → seal → loan → map → return → release lifecycle so
/// the v2 decode test observes real sample-plane side effects, not a no-op.
fn run_sample_plane_exchange() {
    let mut buffers = SharedBufferTable::new();
    let receiver_space = AddressSpace::new().expect("address space");
    let lender = quota(2, 1, 0, 1);
    let receiver = quota(0, 0, 1, 0);

    let region = buffers.create(LENDER, lender, 1, true).expect("buffer");
    buffers.seal(&region).expect("seal");
    let loan = buffers
        .loan(LENDER, RECEIVER, lender, &region, 0, PAGE)
        .expect("loan");
    buffers
        .map_loan(
            RECEIVER,
            receiver,
            loan.id(),
            loan.region(),
            receiver_space.pml4(),
            BASE_RECEIVER,
            0,
            PAGE,
        )
        .expect("map loan");
    buffers
        .return_loan(RECEIVER, loan.id(), loan.region())
        .expect("return");
    buffers.release(&region).expect("release");
    assert_eq!(buffers.total_pages(), 0);
}

/// Build a minimal valid format-2 generation with one self-grant, mirroring the
/// retained v2 layout: grant rights are packed as 4 bytes at +12 with
/// `transferable` at +16. This is the same shape the C7.1 boot-contracts decode
/// tests exercise, reproduced here so the kernel-side test owns a v2 artifact.
fn build_v2_known_good() -> Vec<u8> {
    let rights = RIGHT_TRANSFER | 1;

    // String table: (u16 len, bytes) entries at relative offsets.
    let mut strings = Vec::new();
    let mut push_str = |s: &str| -> u32 {
        let offset = strings.len() as u32;
        strings.extend_from_slice(&(s.len() as u16).to_le_bytes());
        strings.extend_from_slice(s.as_bytes());
        offset
    };
    let target_off = push_str("t");
    let obj_a_off = push_str("a");
    let obj_b_off = push_str("b");
    let init_off = push_str("init");
    let grant_off = push_str("g");

    let object_offset = HEADER_LEN;
    let component_offset = object_offset + 2 * OBJECT_LEN;
    let dependency_offset = component_offset + COMPONENT_LEN;
    let grant_offset = dependency_offset;
    let state_offset = grant_offset + GRANT_LEN;
    let health_offset = state_offset;
    let string_offset = health_offset;
    let payload_offset = string_offset + strings.len();
    let total_len = payload_offset + 2;

    let kernel_digest = {
        let mut h = Sha256::new();
        h.update(b"K");
        h.finalize()
    };
    let bootstrap_digest = {
        let mut h = Sha256::new();
        h.update(b"B");
        h.finalize()
    };

    let mut bytes = vec![0u8; total_len];
    bytes[..8].copy_from_slice(&MAGIC_V2);
    bytes[8..12].copy_from_slice(&FORMAT_VERSION_V2.to_le_bytes());
    bytes[12..16].copy_from_slice(&(HEADER_LEN as u32).to_le_bytes());
    bytes[56..64].copy_from_slice(&1u64.to_le_bytes()); // generation_number
    bytes[96..100].copy_from_slice(&target_off.to_le_bytes());
    bytes[100..104].copy_from_slice(&0u32.to_le_bytes()); // kernel_object
    bytes[104..108].copy_from_slice(&0u32.to_le_bytes()); // bootstrap_component
    bytes[108..112].copy_from_slice(&1u32.to_le_bytes()); // boot_attempts
    bytes[112..116].copy_from_slice(&2u32.to_le_bytes()); // object_count
    bytes[116..120].copy_from_slice(&1u32.to_le_bytes()); // component_count
    bytes[124..128].copy_from_slice(&1u32.to_le_bytes()); // grant_count
    bytes[136..144].copy_from_slice(&(object_offset as u64).to_le_bytes());
    bytes[144..152].copy_from_slice(&(component_offset as u64).to_le_bytes());
    bytes[152..160].copy_from_slice(&(dependency_offset as u64).to_le_bytes());
    bytes[160..168].copy_from_slice(&(grant_offset as u64).to_le_bytes());
    bytes[168..176].copy_from_slice(&(state_offset as u64).to_le_bytes());
    bytes[176..184].copy_from_slice(&(health_offset as u64).to_le_bytes());
    bytes[184..192].copy_from_slice(&(string_offset as u64).to_le_bytes());
    bytes[192..200].copy_from_slice(&(strings.len() as u64).to_le_bytes());
    bytes[200..208].copy_from_slice(&(payload_offset as u64).to_le_bytes());
    bytes[208..216].copy_from_slice(&(total_len as u64).to_le_bytes());

    // Object 0 (id "a", kernel), object 1 (id "b", bootstrap).
    let obj0 = object_offset;
    bytes[obj0..obj0 + 4].copy_from_slice(&obj_a_off.to_le_bytes());
    bytes[obj0 + 4..obj0 + 8].copy_from_slice(&KIND_KERNEL.to_le_bytes());
    bytes[obj0 + 8..obj0 + 16].copy_from_slice(&(payload_offset as u64).to_le_bytes());
    bytes[obj0 + 16..obj0 + 24].copy_from_slice(&1u64.to_le_bytes());
    bytes[obj0 + 24..obj0 + 56].copy_from_slice(&kernel_digest);
    let obj1 = object_offset + OBJECT_LEN;
    bytes[obj1..obj1 + 4].copy_from_slice(&obj_b_off.to_le_bytes());
    bytes[obj1 + 4..obj1 + 8].copy_from_slice(&KIND_BOOTSTRAP.to_le_bytes());
    bytes[obj1 + 8..obj1 + 16].copy_from_slice(&((payload_offset + 1) as u64).to_le_bytes());
    bytes[obj1 + 16..obj1 + 24].copy_from_slice(&1u64.to_le_bytes());
    bytes[obj1 + 24..obj1 + 56].copy_from_slice(&bootstrap_digest);

    // Component 0 ("init", object 1, role init).
    let comp = component_offset;
    bytes[comp..comp + 4].copy_from_slice(&init_off.to_le_bytes());
    bytes[comp + 4..comp + 8].copy_from_slice(&1u32.to_le_bytes()); // object_index
    bytes[comp + 8..comp + 12].copy_from_slice(&ROLE_INIT.to_le_bytes());

    // Grant 0 ("g", self, rights) in v2 layout: 32-bit rights at +12,
    // transferable at +16.
    let grant = grant_offset;
    bytes[grant..grant + 4].copy_from_slice(&grant_off.to_le_bytes());
    bytes[grant + 4..grant + 8].copy_from_slice(&0u32.to_le_bytes()); // source
    bytes[grant + 8..grant + 12].copy_from_slice(&0u32.to_le_bytes()); // target
    bytes[grant + 12..grant + 16].copy_from_slice(&(rights as u32).to_le_bytes());
    let transferable = u32::from(rights & RIGHT_TRANSFER != 0);
    bytes[grant + 16..grant + 20].copy_from_slice(&transferable.to_le_bytes());

    // String table and object payloads.
    bytes[string_offset..string_offset + strings.len()].copy_from_slice(&strings);
    bytes[payload_offset] = b'K';
    bytes[payload_offset + 1] = b'B';

    // Seal the generation identity over the zeroed identity window.
    let identity = generation_identity(&bytes);
    bytes[24..56].copy_from_slice(&identity);
    bytes
}
