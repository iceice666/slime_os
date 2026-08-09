//! Child runtime for the native seL4 transport.
//!
//! On seL4 a component is an ordinary ELF task, not a kernel-launched Slime
//! task, so `_start` cannot be a bare jump into `main`: the stack pointer, the
//! Rust language runtime, and this thread's IPC buffer must be established
//! first. That is exactly what `sel4-runtime-common` does for every other
//! rust-sel4 task, so [`crate::entry!`] reuses it rather than growing a private
//! entry sequence.
//!
//! The IPC buffer sits on the first page boundary past `_end`, and the transfer
//! window on the page after that, which is where the root service maps them
//! when it builds the child's VSpace. Nothing else is discovered at startup:
//! the component's authority is exactly the capabilities its generation placed
//! in its CSpace, with the root service endpoint at slot
//! [`crate::ROOT_SERVICE_SLOT`].

use core::ptr;

use sel4::CapTypeForFrameObjectOfFixedSize;

/// Stack reserved for a component's initial (and only) thread. Components are
/// bounded, single-purpose tasks with no recursion in the runtime path, so this
/// matches the rust-sel4 child default rather than the larger root-task one.
pub const STACK_SIZE: usize = 64 * 1024;

/// Granule size for this configuration, used to place the two runtime pages the
/// root maps above the image.
const GRANULE: usize = sel4::cap_type::Granule::FRAME_OBJECT_TYPE.bytes();

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
fn ipc_buffer_addr() -> usize {
    unsafe extern "C" {
        static _end: usize;
    }
    (ptr::addr_of!(_end) as usize).next_multiple_of(GRANULE)
}

/// Address of this component's transfer window: the granule above the IPC
/// buffer. Both are placed by `slime-root/src/child_vspace.rs`, so this is a
/// compile-time agreement between the two images rather than a runtime search.
fn transfer_window_addr() -> usize {
    ipc_buffer_addr() + GRANULE
}

/// Binds the IPC buffer and the transfer window, then runs the component body.
/// Returning from `main` is a clean exit, exactly as it is on the legacy
/// transport.
///
/// The window must be bound before the body runs: `recv`, `spawn` and `wait`
/// all stage through it, and refuse to truncate a payload when none is bound.
/// The root maps it, so binding is a declaration of an existing mapping rather
/// than an allocation — which is what lets a component holding no
/// `SharedBufferFactory` still receive messages.
///
/// # Safety
///
/// Must be called once, from the runtime entrypoint, on the initial thread.
pub unsafe fn start(main: fn()) -> ! {
    // SAFETY: called once during startup, before any seL4 invocation, and the
    // buffer is mapped for this thread's exclusive use.
    unsafe {
        sel4::set_ipc_buffer(
            (ipc_buffer_addr() as *mut sel4::IpcBuffer)
                .as_mut()
                .unwrap(),
        );
    }
    // A failure here would leave every windowed operation returning
    // `ERR_INVALID_ARG`, so it is fatal. The marker is emitted in register-sized
    // chunks because the missing window is exactly what failed to bind.
    if crate::syscall::bind_startup_window(transfer_window_addr()) != crate::ERR_SUCCESS {
        crate::syscall::early_debug_write(b"[slime-rt] transfer window bind failed\n");
        crate::exit(1)
    }
    main();
    crate::exit(0)
}
