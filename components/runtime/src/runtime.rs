//! Child runtime for the native seL4 transport.
//!
//! On seL4 a component is an ordinary ELF task, not a kernel-launched Slime
//! task, so `_start` cannot be a bare jump into `main`: the stack pointer, the
//! Rust language runtime, and this thread's IPC buffer must be established
//! first. That is exactly what `sel4-runtime-common` does for every other
//! rust-sel4 task, so [`crate::entry!`] reuses it rather than growing a private
//! entry sequence.
//!
//! The IPC buffer sits on the first page boundary past `_end`, which is where
//! the root service maps it when it builds the child's VSpace. Nothing else is
//! discovered at startup: the component's authority is exactly the capabilities
//! its generation placed in its CSpace, with the root service endpoint at slot
//! [`crate::ROOT_SERVICE_SLOT`].

use core::ptr;

use sel4::CapTypeForFrameObjectOfFixedSize;

/// Stack reserved for a component's initial (and only) thread. Components are
/// bounded, single-purpose tasks with no recursion in the runtime path, so this
/// matches the rust-sel4 child default rather than the larger root-task one.
pub const STACK_SIZE: usize = 64 * 1024;

sel4::sel4_cfg_if! {
    if #[sel4_cfg(PRINTING)] {
        fn debug_put_char(character: u8) {
            sel4::debug_put_char(character)
        }
    } else {
        fn debug_put_char(_: u8) {}
    }
}

// `sel4-runtime-common` reports startup failures through `sel4_panicking_env`,
// which resolves its character sink at link time.
sel4_panicking_env::register_debug_put_char!(debug_put_char);

/// Address of this thread's IPC buffer: the first page boundary past the
/// image, matching the child VSpace the root service constructs.
fn ipc_buffer() -> *mut sel4::IpcBuffer {
    unsafe extern "C" {
        static _end: usize;
    }
    (ptr::addr_of!(_end) as usize)
        .next_multiple_of(sel4::cap_type::Granule::FRAME_OBJECT_TYPE.bytes())
        as *mut sel4::IpcBuffer
}

/// Binds the IPC buffer and runs the component body. Returning from `main` is
/// a clean exit, exactly as it is on the legacy transport.
///
/// # Safety
///
/// Must be called once, from the runtime entrypoint, on the initial thread.
pub unsafe fn start(main: fn()) -> ! {
    // SAFETY: called once during startup, before any seL4 invocation, and the
    // buffer is mapped for this thread's exclusive use.
    unsafe {
        sel4::set_ipc_buffer(ipc_buffer().as_mut().unwrap());
    }
    main();
    crate::exit(0)
}
