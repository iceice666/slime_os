#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

//! Spawn-authority and capability-table invariants.
//!
//! Mechanism-level rules from the capability matrix
//! (`docs/capability-matrix.md`):
//! - the capability table rejects rights that are meaningless for the
//!   object kind;
//! - spawn grants require `RIGHT_TRANSFER` — the same condition IPC sends
//!   enforce — so a non-transferable capability cannot be laundered into a
//!   spawned component;
//! - the live task table is bounded: spawning past `MAX_TASKS` fails with
//!   `TooManyTasks` instead of silently exhausting kernel memory.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use slime_os_kernel::capability::{
    CapError, Capability, CapabilityTable, KernelObject, PciFunctionInfo, RIGHT_BLOCK_READ,
    RIGHT_BLOCK_WRITE, RIGHT_ENDPOINT_CREATE, RIGHT_EXEC, RIGHT_MAP_MMIO, RIGHT_RECV, RIGHT_SEND,
    RIGHT_SPAWN, RIGHT_SUPERVISE, RIGHT_TRANSFER,
};
use slime_os_kernel::task::{self, MAX_TASKS, SpawnError};
use slime_os_kernel::{gdt, interrupts, ipc, memory};

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

fn endpoint_cap(rights: u32) -> Capability {
    let (a, _b) = ipc::channel();
    Capability {
        object: KernelObject::Endpoint(a),
        rights,
    }
}

fn executable_cap(rights: u32) -> Capability {
    Capability {
        object: KernelObject::Executable {
            name: None,
            bytes: &[0x90],
            spawn_budget: 1,
        },
        rights,
    }
}

fn block_device() -> KernelObject {
    KernelObject::BlockDevice(PciFunctionInfo {
        segment: 0,
        bus: 0,
        device: 4,
        function: 0,
        vendor_id: 0x1af4,
        device_id: 0x1042,
        class_code: 0x010000,
    })
}

/// Wrap a code blob in a single-segment executable component image
/// (`contracts/component/v1`) so spawn accepts it.
fn rx_image(code: &[u8]) -> Vec<u8> {
    use slime_os_kernel::component::*;
    let mut image = Vec::new();
    image.extend_from_slice(
        &WireImageHeader {
            magic: IMAGE_MAGIC,
            format_version: FORMAT_VERSION,
            header_size: HEADER_LEN as u32,
            kernel_abi: KERNEL_ABI_VERSION,
            entry_offset: 0,
            segment_count: 1,
            reserved: 0,
            stack_bytes: DEFAULT_STACK_BYTES,
        }
        .encode(),
    );
    image.extend_from_slice(
        &WireSegmentRecord {
            vaddr_offset: 0,
            mem_len: code.len() as u32,
            file_offset: 0,
            file_len: code.len() as u32,
            flags: SEGMENT_FLAG_EXEC,
            reserved: 0,
        }
        .encode(),
    );
    image.extend_from_slice(code);
    image
}

#[test_case]
fn table_accepts_rights_valid_for_object_kind() {
    let mut table = CapabilityTable::new();
    table
        .insert(endpoint_cap(RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER))
        .unwrap();
    table
        .insert(executable_cap(RIGHT_EXEC | RIGHT_SPAWN | RIGHT_TRANSFER))
        .unwrap();
    table
        .insert(Capability {
            object: block_device(),
            rights: RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE | RIGHT_TRANSFER,
        })
        .unwrap();
}

#[test_case]
fn table_rejects_rights_foreign_to_object_kind() {
    let cases = [
        endpoint_cap(RIGHT_EXEC),
        executable_cap(RIGHT_SEND),
        Capability {
            object: block_device(),
            rights: RIGHT_MAP_MMIO,
        },
        // Unknown bits are foreign to every object kind.
        endpoint_cap(1 << 31),
    ];
    for cap in cases {
        let mut table = CapabilityTable::new();
        assert!(matches!(table.insert(cap), Err(CapError::BadRights)));
    }
}

fn grant(slot: u32, rights: u32) -> task::SpawnGrant {
    task::SpawnGrant { slot, rights }
}

#[test_case]
fn preflight_requires_spawn_right_and_preserves_source() {
    let mut table = CapabilityTable::new();
    let executable = table.insert(executable_cap(RIGHT_EXEC)).unwrap();
    assert!(matches!(
        task::preflight_spawn_grant(&table, executable, &[]),
        Err(SpawnError::BadExecutable)
    ));

    let mut table = CapabilityTable::new();
    let executable = table
        .insert(executable_cap(RIGHT_EXEC | RIGHT_SPAWN))
        .unwrap();
    let endpoint = table
        .insert(endpoint_cap(RIGHT_RECV | RIGHT_TRANSFER))
        .unwrap();
    let plan =
        task::preflight_spawn_grant(&table, executable, &[grant(endpoint, RIGHT_RECV)]).unwrap();
    assert_eq!(plan.image, &[0x90]);
    assert_eq!(plan.caps.len(), 1);
    assert_eq!(plan.caps[0].rights, RIGHT_RECV);
    assert_eq!(
        table.get(endpoint).unwrap().rights,
        RIGHT_RECV | RIGHT_TRANSFER
    );
}

#[test_case]
fn preflight_rejects_widening_and_unheld_transfer() {
    let mut table = CapabilityTable::new();
    let executable = table
        .insert(executable_cap(RIGHT_EXEC | RIGHT_SPAWN))
        .unwrap();
    let endpoint = table.insert(endpoint_cap(RIGHT_RECV)).unwrap();
    assert!(matches!(
        task::preflight_spawn_grant(&table, executable, &[grant(endpoint, RIGHT_SEND)]),
        Err(SpawnError::BadCapability)
    ));
    assert!(matches!(
        task::preflight_spawn_grant(
            &table,
            executable,
            &[grant(endpoint, RIGHT_RECV | RIGHT_TRANSFER)]
        ),
        Err(SpawnError::BadCapability)
    ));
}

#[test_case]
fn factory_and_supervision_rights_are_object_specific() {
    let mut table = CapabilityTable::new();
    table
        .insert(Capability {
            object: KernelObject::EndpointFactory,
            rights: RIGHT_ENDPOINT_CREATE,
        })
        .unwrap();
    table
        .insert(Capability {
            object: KernelObject::Supervision(42),
            rights: RIGHT_SUPERVISE,
        })
        .unwrap();
    assert!(matches!(
        table.insert(Capability {
            object: KernelObject::EndpointFactory,
            rights: RIGHT_SEND,
        }),
        Err(CapError::BadRights)
    ));
}

#[test_case]
fn supervision_reasons_remain_structured() {
    let cases = [
        task::TermReason::Exit(7),
        task::TermReason::Fault(task::UserFaultReason::PageFault),
        task::TermReason::Timeout,
        task::TermReason::PeerLoss,
    ];
    assert_ne!(cases[0], cases[1]);
    assert_ne!(cases[1], cases[2]);
    assert_ne!(cases[2], cases[3]);
}

#[test_case]
fn preflight_rejects_bad_grant_slots() {
    let mut table = CapabilityTable::new();
    let executable = table
        .insert(executable_cap(RIGHT_EXEC | RIGHT_SPAWN))
        .unwrap();
    let endpoint = table
        .insert(endpoint_cap(RIGHT_RECV | RIGHT_TRANSFER))
        .unwrap();
    // The executable slot itself is not a grant.
    assert!(matches!(
        task::preflight_spawn_grant(&table, executable, &[grant(executable, RIGHT_EXEC)]),
        Err(SpawnError::BadCapability)
    ));
    // Duplicate grants would copy the same source ambiguously.
    assert!(matches!(
        task::preflight_spawn_grant(
            &table,
            executable,
            &[grant(endpoint, RIGHT_RECV), grant(endpoint, RIGHT_RECV)]
        ),
        Err(SpawnError::BadCapability)
    ));
    // Missing slot.
    assert!(matches!(
        task::preflight_spawn_grant(&table, executable, &[grant(63, RIGHT_RECV)]),
        Err(SpawnError::BadCapability)
    ));
    // Missing executable slot.
    assert!(matches!(
        task::preflight_spawn_grant(&table, 63, &[]),
        Err(SpawnError::BadExecutable)
    ));
}

#[test_case]
fn spawn_fails_structured_when_task_table_full() {
    let image = rx_image(&[0x90]);
    for _ in 0..MAX_TASKS {
        task::spawn_with_caps(&image, vec![]).unwrap();
    }
    let result = task::spawn_with_caps(&image, vec![]);
    assert!(matches!(result, Err(SpawnError::TooManyTasks)));
}
