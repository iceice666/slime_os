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
    CapError, Capability, CapabilityTable, KernelObject, RIGHT_BLOCK_READ, RIGHT_BLOCK_WRITE,
    RIGHT_EXEC, RIGHT_MAP_MMIO, RIGHT_RECV, RIGHT_SEND, RIGHT_TRANSFER,
};
use slime_os_kernel::task::{self, MAX_TASKS, SpawnError};
use slime_os_kernel::{gdt, interrupts, ipc, memory};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    slime_os_kernel::limine::ensure_linked();
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
        object: KernelObject::Executable(&[0x90]),
        rights,
    }
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
        .insert(executable_cap(RIGHT_EXEC | RIGHT_TRANSFER))
        .unwrap();
    table
        .insert(Capability {
            object: KernelObject::BlockDevice,
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
            object: KernelObject::BlockDevice,
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

#[test_case]
fn preflight_rejects_grant_without_transfer() {
    let mut table = CapabilityTable::new();
    let executable = table.insert(executable_cap(RIGHT_EXEC)).unwrap();
    let untransferable = table.insert(endpoint_cap(RIGHT_RECV)).unwrap();
    let result = task::preflight_spawn_grant(&table, executable, &[untransferable]);
    assert!(matches!(result, Err(SpawnError::BadCapability)));
}

#[test_case]
fn preflight_accepts_transferable_grant() {
    let mut table = CapabilityTable::new();
    let executable = table.insert(executable_cap(RIGHT_EXEC)).unwrap();
    let endpoint = table
        .insert(endpoint_cap(RIGHT_RECV | RIGHT_TRANSFER))
        .unwrap();
    let (code, granted) = task::preflight_spawn_grant(&table, executable, &[endpoint]).unwrap();
    assert_eq!(code, &[0x90]);
    assert_eq!(granted.len(), 1);
    assert_eq!(granted[0].rights, RIGHT_RECV | RIGHT_TRANSFER);
}

#[test_case]
fn preflight_rejects_bad_grant_slots() {
    let mut table = CapabilityTable::new();
    let executable = table.insert(executable_cap(RIGHT_EXEC)).unwrap();
    let endpoint = table
        .insert(endpoint_cap(RIGHT_RECV | RIGHT_TRANSFER))
        .unwrap();
    // The executable slot itself is not a grant.
    assert!(matches!(
        task::preflight_spawn_grant(&table, executable, &[executable]),
        Err(SpawnError::BadCapability)
    ));
    // Duplicate grants would move the same capability twice.
    assert!(matches!(
        task::preflight_spawn_grant(&table, executable, &[endpoint, endpoint]),
        Err(SpawnError::BadCapability)
    ));
    // Missing slot.
    assert!(matches!(
        task::preflight_spawn_grant(&table, executable, &[63]),
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
