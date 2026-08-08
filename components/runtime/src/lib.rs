#![no_std]

mod sha256;
mod syscall;

#[cfg(not(feature = "sel4"))]
mod arch;
#[cfg(feature = "sel4")]
mod runtime;

pub use sha256::sha256;
pub use syscall::{
    BufferLoan, ERR_BAD_CAP, ERR_INVALID_ARG, ERR_OUT_OF_MEMORY, ERR_PEER_DEAD, ERR_SUCCESS,
    ERR_WOULDBLOCK, InputEvent, InputKey, MAX_CAPS_PER_MSG, MAX_DIRECTORY_PATH, MAX_MSG,
    MAX_WAIT_SOURCES, MIN_TRANSFER_WINDOW, Rights, SharedBuffer, SpawnGrant, Spawned, Termination,
    WaitSource, block_transact, cap_drop, cap_transfer, debug_write, directory_commit,
    directory_derive, directory_inspect, endpoint_create, exit, generation_receive,
    generation_transact, health_confirm, input_read, recovery_reconstruct, recv, send,
    shared_buffer_create, shared_buffer_loan, shared_buffer_loan_map, shared_buffer_map,
    shared_buffer_release, shared_buffer_return, shared_buffer_revoke, shared_buffer_seal,
    shared_buffer_unmap, spawn, store_transact, supervision_derive, supervision_status,
    transfer_window_bind, unhealthy, wait, yield_now,
};

/// The CSpace slot holding this component's root service endpoint — its only
/// root authority on the native seL4 transport.
#[cfg(feature = "sel4")]
pub use syscall::ROOT_SERVICE_SLOT;

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    exit(1)
}

/// Defines the component's `_start` entry point, calling `$main` and exiting
/// 0 if it returns.
///
/// `$main` must be a `fn()`; the Slime operation API has no argv/envp, so entry
/// takes no arguments.
///
/// On the native seL4 transport this also performs the startup a real seL4 task
/// requires — stack, Rust language runtime, and this thread's IPC buffer —
/// through `sel4-runtime-common`, so component sources stay identical across
/// transports.
#[cfg(feature = "sel4")]
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        $crate::_private::declare_stack!($crate::_private::STACK_SIZE);
        $crate::_private::declare_entrypoint_with_stack_init!();
        $crate::_private::declare_rust_entrypoint! {
            __slime_rt_entrypoint()
        }

        fn __slime_rt_entrypoint() -> ! {
            let main: fn() = $main;
            unsafe { $crate::_private::start(main) }
        }
    };
}

/// Defines the component's `_start` entry point, calling `$main` and exiting
/// 0 if it returns.
///
/// `$main` must be a `fn()`; the Slime operation API has no argv/envp, so entry
/// takes no arguments.
#[cfg(not(feature = "sel4"))]
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn _start() -> ! {
            let main: fn() = $main;
            main();
            $crate::exit(0)
        }
    };
}

#[cfg(feature = "sel4")]
#[doc(hidden)]
pub mod _private {
    pub use sel4_runtime_common::{
        declare_entrypoint_with_stack_init, declare_rust_entrypoint, declare_stack,
    };

    pub use crate::runtime::{STACK_SIZE, start};
}
