#![no_std]

#[cfg(feature = "heap")]
mod heap;
mod sha256;
mod syscall;

mod runtime;

#[cfg(feature = "heap")]
pub use heap::{BumpHeap, HEAP_BYTES, heap_used};
pub use sha256::sha256;
pub use syscall::{
    BufferLoan, DIRECTORY_ROOT_BYTES, ERR_BAD_CAP, ERR_INVALID_ARG, ERR_OUT_OF_MEMORY,
    ERR_PEER_DEAD, ERR_SUCCESS, ERR_WOULDBLOCK, InputEvent, InputKey, MAX_CAPS_PER_MSG,
    MAX_DIRECTORY_PATH, MAX_MSG, MAX_WAIT_SOURCES, Rights, SharedBuffer, SpawnGrant, Spawned,
    Termination, WaitSource, block_transact, block_transact_sector, block_transact_write, cap_drop,
    cap_transfer, debug_write, directory_commit, directory_derive, directory_inspect,
    endpoint_create, exit, generation_receive, generation_transact, health_confirm, input_read,
    recovery_reconstruct, recv, send, shared_buffer_create, shared_buffer_loan,
    shared_buffer_loan_map, shared_buffer_map, shared_buffer_release, shared_buffer_return,
    shared_buffer_revoke, shared_buffer_seal, shared_buffer_unmap, spawn, store_transact,
    supervision_derive, supervision_status, unhealthy, wait, yield_now,
};

/// The CSpace slot holding this component's root service endpoint — its only
/// root authority on the native seL4 transport.
pub use syscall::ROOT_SERVICE_SLOT;

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    exit(1)
}

/// Defines the component's seL4 entry point, calling `$main` and exiting 0 if
/// it returns. Startup installs the stack, Rust language runtime, IPC buffer,
/// and bound transfer window before component code runs.
///
/// `$main` takes the authenticated startup argument the root placed in this
/// thread's first C parameter. It is the generation's boot action for the
/// bootstrap instance and zero for every other component, so a component
/// composes its graph from admitted data rather than from a build flag.
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        $crate::_private::declare_stack!($crate::_private::STACK_SIZE);
        $crate::_private::declare_entrypoint_with_stack_init!();
        $crate::_private::declare_rust_entrypoint! {
            __slime_rt_entrypoint(startup_arg: u32)
        }

        fn __slime_rt_entrypoint(startup_arg: u32) -> ! {
            let main: fn(u32) = $main;
            unsafe { $crate::_private::start(main, startup_arg) }
        }
    };
}

pub mod _private {
    pub use sel4_runtime_common::{
        declare_entrypoint_with_stack_init, declare_rust_entrypoint, declare_stack,
    };

    pub use crate::runtime::{STACK_SIZE, start};
}
