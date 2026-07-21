#![no_std]

mod sha256;
mod syscall;

pub use sha256::sha256;
pub use syscall::{
    ERR_BAD_CAP, ERR_INVALID_ARG, ERR_OUT_OF_MEMORY, ERR_PEER_DEAD, ERR_SUCCESS, ERR_WOULDBLOCK,
    MAX_CAPS_PER_MSG, MAX_MSG, block_transact, debug_write, exit, health_confirm,
    recovery_reconstruct, recv, send, spawn, store_transact, unhealthy, yield_now,
};

use core::panic::PanicInfo;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    exit(1)
}

/// Defines the component's `_start` entry point, calling `$main` and exiting
/// 0 if it returns.
///
/// `$main` must be a `fn()`; the component syscall ABI has no argv/envp, so
/// entry takes no arguments.
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
