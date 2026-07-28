#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

//! C8.3 narrow-on-transfer rights algebra.
//!
//! `just fabric_authority_check`'s live arm proves a real fabric service
//! derives real route roles over the real syscall. This file is the algebra
//! underneath it: the exact rules `SYS_CAP_TRANSFER` composes, exercised
//! directly against `Capability`, `CapabilityTable`, and the descriptor
//! validator so a rule can be broken here without needing a boot to notice.
//!
//! The kernel's whole C8 contribution is one generic mechanism, so the rules
//! are few and each one is load-bearing:
//!
//!   * the destination mask is a subset of the source rights *and* of the
//!     object's meaningful rights — widening in either direction is refused;
//!   * `RIGHT_TRANSFER` is dropped unless explicitly retained, so a provisioned
//!     capability is non-delegable by default;
//!   * a descriptor must name the moved capability's real object kind, so the
//!     bytes the peer reads describe what actually crossed;
//!   * the move is exactly one holder to one holder — never a duplicate.

extern crate alloc;
use slime_os_kernel::capability::{
    Capability, CapabilityTable, KernelObject, RIGHT_BUFFER_CREATE, RIGHT_BUFFER_MAP,
    RIGHT_BUFFER_WRITE, RIGHT_RECV, RIGHT_SEND, RIGHT_SUPERVISE, RIGHT_TRANSFER,
};
use slime_os_kernel::capability_transfer_proto::{
    CAPABILITY_TRANSFER_MAGIC, FLAG_RETAIN_TRANSFER, FORMAT_VERSION, OBJECT_KIND_ENDPOINT,
    OBJECT_KIND_SHARED_BUFFER, OBJECT_KIND_SUPERVISION, TRANSFER_LEN, WireCapabilityTransfer,
    destination_rights, kind_matches, valid_transfer,
};
use slime_os_kernel::ipc::{self, MAX_MSG};
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

fn descriptor(object_kind: u32, rights_mask: u64, flags: u32) -> WireCapabilityTransfer {
    WireCapabilityTransfer {
        magic: CAPABILITY_TRANSFER_MAGIC,
        version: FORMAT_VERSION,
        status: 0,
        flags,
        object_kind,
        direction: 1,
        rights_mask,
        route_identity: [0x5a; 32],
    }
}

/// The descriptor is a control message, so it must fit the channel bound
/// exactly like every other one. If it grew past `MAX_MSG` a transfer would
/// silently truncate its own rights mask.
#[test_case]
fn descriptor_is_exactly_one_control_message() {
    assert_eq!(TRANSFER_LEN, MAX_MSG);
    let encoded = descriptor(OBJECT_KIND_ENDPOINT, RIGHT_SEND, 0).encode();
    assert_eq!(encoded.len(), MAX_MSG);
    assert_eq!(
        WireCapabilityTransfer::decode(&encoded),
        Some(descriptor(OBJECT_KIND_ENDPOINT, RIGHT_SEND, 0)),
        "an admitted descriptor round-trips byte-identically"
    );
}

/// Every structural arm the syscall checks before it touches a capability.
#[test_case]
fn malformed_descriptors_are_rejected_before_any_move() {
    assert!(valid_transfer(&descriptor(
        OBJECT_KIND_ENDPOINT,
        RIGHT_SEND,
        0
    )));

    let bad_magic = WireCapabilityTransfer {
        magic: CAPABILITY_TRANSFER_MAGIC ^ 1,
        ..descriptor(OBJECT_KIND_ENDPOINT, RIGHT_SEND, 0)
    };
    assert!(!valid_transfer(&bad_magic));

    let bad_version = WireCapabilityTransfer {
        version: FORMAT_VERSION + 1,
        ..descriptor(OBJECT_KIND_ENDPOINT, RIGHT_SEND, 0)
    };
    assert!(!valid_transfer(&bad_version), "unsupported version");

    let unknown_flag = descriptor(OBJECT_KIND_ENDPOINT, RIGHT_SEND, 0x8000_0000);
    assert!(!valid_transfer(&unknown_flag), "unknown flag");

    // A move granting nothing is a drop, and `SYS_CAP_DROP` already spells it.
    assert!(!valid_transfer(&descriptor(OBJECT_KIND_ENDPOINT, 0, 0)));

    // An undefined rights bit can never be meaningful for any object.
    let unknown_right = descriptor(OBJECT_KIND_ENDPOINT, 1 << 40, 0);
    assert!(!valid_transfer(&unknown_right));

    // A denial reply is not a move: it never reaches the syscall.
    let denial = WireCapabilityTransfer {
        status: -1,
        ..descriptor(OBJECT_KIND_ENDPOINT, RIGHT_SEND, 0)
    };
    assert!(!valid_transfer(&denial));

    // 0 is not a defined object kind, so a zeroed descriptor names nothing.
    assert!(!valid_transfer(&descriptor(0, RIGHT_SEND, 0)));
}

/// The declared object kind must be the moved capability's real kind. Without
/// this the descriptor a peer reads could describe an object other than the one
/// that crossed, and the peer's own validation would be checking a fiction.
#[test_case]
fn descriptor_kind_must_match_the_moved_object() {
    let (endpoint, _peer) = ipc::channel();
    let endpoint = KernelObject::Endpoint(endpoint);
    assert!(kind_matches(OBJECT_KIND_ENDPOINT, &endpoint));
    assert!(!kind_matches(OBJECT_KIND_SHARED_BUFFER, &endpoint));
    assert!(!kind_matches(OBJECT_KIND_SUPERVISION, &endpoint));

    // An object outside the transferable set matches no declared kind at all,
    // so a factory can never be moved through this path.
    let factory = KernelObject::SharedBufferFactory;
    for kind in [
        OBJECT_KIND_ENDPOINT,
        OBJECT_KIND_SHARED_BUFFER,
        OBJECT_KIND_SUPERVISION,
    ] {
        assert!(
            !kind_matches(kind, &factory),
            "an untransferable object kind matches no descriptor"
        );
    }
}

/// Transfer authority is not inherited. A route role handed to a participant is
/// terminal unless the provisioning side deliberately says otherwise, so
/// non-delegability is the default rather than a convention.
#[test_case]
fn transfer_authority_is_dropped_unless_explicitly_retained() {
    let asking = descriptor(OBJECT_KIND_ENDPOINT, RIGHT_SEND | RIGHT_TRANSFER, 0);
    assert_eq!(
        destination_rights(&asking),
        RIGHT_SEND,
        "naming the transfer bit is not enough to keep it"
    );

    let retaining = descriptor(
        OBJECT_KIND_ENDPOINT,
        RIGHT_SEND | RIGHT_TRANSFER,
        FLAG_RETAIN_TRANSFER,
    );
    assert_eq!(
        destination_rights(&retaining),
        RIGHT_SEND | RIGHT_TRANSFER,
        "retention is available, but only deliberately"
    );

    // Retention cannot conjure the bit out of a mask that never named it.
    let no_bit = descriptor(OBJECT_KIND_ENDPOINT, RIGHT_SEND, FLAG_RETAIN_TRANSFER);
    assert_eq!(destination_rights(&no_bit), RIGHT_SEND);
}

/// The two narrowing rules the syscall applies before consuming the source:
/// the mask must be within the source's own rights, and within the rights the
/// object kind defines. Either violation is a widening.
#[test_case]
fn masked_transfer_cannot_widen_rights() {
    let (endpoint, _peer) = ipc::channel();
    let source = Capability {
        object: KernelObject::Endpoint(endpoint),
        rights: RIGHT_SEND | RIGHT_TRANSFER,
    };

    assert_eq!(
        source.derive(RIGHT_SEND).expect("narrowing").rights,
        RIGHT_SEND
    );
    assert!(
        source.derive(RIGHT_SEND | RIGHT_RECV).is_err(),
        "a mask naming a right the source does not hold is refused"
    );

    // Object-meaningful rights are the second bound. A buffer right is
    // meaningless on an endpoint, so it can never be installed on one even if
    // some source somehow held it.
    let over_broad = RIGHT_SEND | RIGHT_BUFFER_WRITE;
    assert!(
        over_broad & !source.object.valid_rights() != 0,
        "buffer authority is not meaningful for an endpoint"
    );
    let mut table = CapabilityTable::new();
    let (other, _other_peer) = ipc::channel();
    assert!(
        table
            .insert(Capability {
                object: KernelObject::Endpoint(other),
                rights: over_broad,
            })
            .is_err(),
        "the table refuses rights meaningless for the object kind"
    );
}

/// A move is exactly one holder to one holder: the source slot is consumed, and
/// the destination holds only the narrowed copy. Duplication here would silently
/// leave two components on a route the graph declared once.
#[test_case]
fn a_move_consumes_the_source_and_never_duplicates() {
    let (endpoint, _peer) = ipc::channel();
    let mut source_table = CapabilityTable::new();
    let slot = source_table
        .insert(Capability {
            object: KernelObject::Endpoint(endpoint),
            rights: RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        })
        .expect("source insert");

    let moved = source_table
        .get(slot)
        .expect("source present")
        .derive(RIGHT_SEND)
        .expect("narrowing derive");
    let consumed = source_table.take(slot).expect("move consumes the source");
    assert_eq!(consumed.rights, RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER);
    assert!(
        source_table.get(slot).is_none(),
        "the source slot is empty after the move"
    );

    let mut destination = CapabilityTable::new();
    let landed = destination.insert(moved).expect("destination insert");
    assert_eq!(
        destination.get(landed).expect("landed").rights,
        RIGHT_SEND,
        "the destination holds exactly the narrowed mask"
    );

    // Restoring on a failed send puts back the original rights, not the mask:
    // a refused move must leave the holder exactly as it was.
    source_table.put(slot, consumed).expect("restore");
    assert_eq!(
        source_table.get(slot).expect("restored").rights,
        RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER
    );
}

/// The same algebra over the other transferable object kinds, so the mechanism
/// stays generic rather than quietly endpoint-only.
#[test_case]
fn narrowing_applies_to_every_transferable_object_kind() {
    let region = slime_os_kernel::memory::shared_buffer::SHARED_BUFFER_TABLE
        .lock()
        .create(
            0x8c,
            slime_os_kernel::memory::shared_buffer::HolderQuota {
                byte_pages: 4,
                buffer_count: 1,
                mapping_count: 2,
                loan_count: 1,
            },
            1,
            true,
        )
        .expect("buffer for the narrowing check");

    let buffer = Capability {
        object: KernelObject::SharedBuffer(region.clone()),
        rights: RIGHT_BUFFER_WRITE | RIGHT_BUFFER_MAP | RIGHT_TRANSFER,
    };
    // A read-only downstream handoff: map authority survives, write does not.
    assert_eq!(
        buffer.derive(RIGHT_BUFFER_MAP).expect("narrow").rights,
        RIGHT_BUFFER_MAP
    );
    assert!(
        buffer.derive(RIGHT_BUFFER_CREATE).is_err(),
        "creation authority is a factory right, not a buffer right"
    );
    assert!(kind_matches(OBJECT_KIND_SHARED_BUFFER, &buffer.object));

    let supervision = Capability {
        object: KernelObject::Supervision(0x8d),
        rights: RIGHT_SUPERVISE | RIGHT_TRANSFER,
    };
    assert_eq!(
        supervision.derive(RIGHT_SUPERVISE).expect("narrow").rights,
        RIGHT_SUPERVISE
    );
    assert!(kind_matches(OBJECT_KIND_SUPERVISION, &supervision.object));

    slime_os_kernel::memory::shared_buffer::SHARED_BUFFER_TABLE
        .lock()
        .release(&region)
        .expect("release the narrowing-check buffer");
}
