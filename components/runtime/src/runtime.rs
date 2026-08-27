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

/// Stack reserved for one component thread. Components are bounded,
/// single-purpose tasks with no recursion in the runtime path, so this matches
/// the rust-sel4 child default rather than the larger root-task one.
pub const STACK_SIZE: usize = 64 * 1024;

/// Threads one component may run (B47).
///
/// Two: a main thread and one worker. That is what the generation can declare
/// and what proves the split the model claims — a process owns a CSpace and
/// VSpace, a thread owns a TCB, stack, IPC buffer, and schedule. Raising it is
/// a change here and in `child_vspace`, which maps one buffer/window pair per
/// thread.
pub const MAX_THREADS: usize = boot_contracts::component_runtime_abi::MAX_THREADS;

/// Granule size for this configuration, used to place the two runtime pages the
/// root maps above the image.
pub(crate) const GRANULE: usize = sel4::cap_type::Granule::FRAME_OBJECT_TYPE.bytes();
const _: () = assert!(GRANULE == boot_contracts::component_runtime_abi::GRANULE_BYTES);

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

/// Address of thread `index`'s IPC buffer.
///
/// `child_vspace` maps one buffer/window pair per thread above the image, in
/// thread order, so a thread computes its own from the index the root gave it.
/// The two images agree by the same arithmetic rather than by a table either
/// could get wrong (B47).
pub(crate) fn thread_ipc_buffer_addr(index: usize) -> usize {
    ipc_buffer_addr() + index * 2 * GRANULE
}

/// Address of thread `index`'s transfer window: the granule above its IPC
/// buffer. Both are placed by `slime-root/src/child_vspace.rs`, so this is a
/// compile-time agreement between the two images rather than a runtime search.
fn transfer_window_addr(index: usize) -> usize {
    thread_ipc_buffer_addr(index) + GRANULE
}

/// A worker thread's stack.
///
/// Sized and aligned like the main thread's: `sel4-runtime-common`'s
/// `declare_stack!` cannot be used twice — it defines one fixed symbol — so a
/// component declaring a worker gets this instead. The root reads
/// [`WorkerStack::top`] to set the second TCB's stack pointer.
#[repr(C, align(16))]
pub struct WorkerStack(core::cell::UnsafeCell<[u8; STACK_SIZE]>);

// SAFETY: the contents are never read or written through this type. It exists
// to reserve writable, aligned space whose address the root reads from the
// symbol table; the only accesses are the worker thread's own pushes and pops
// through `sp`, and no other thread has its address.
unsafe impl Sync for WorkerStack {}

impl WorkerStack {
    /// A zeroed worker stack.
    ///
    /// The `UnsafeCell` is load-bearing: a plain `static` of a type with no
    /// interior mutability is placed in a read-only section, and a thread whose
    /// `sp` points there faults on its first push. That is exactly the fault
    /// this hit — `Execute` at a near-null PC, because the prologue's store
    /// trapped before anything ran.
    pub const fn new() -> Self {
        Self(core::cell::UnsafeCell::new([0; STACK_SIZE]))
    }
}

impl Default for WorkerStack {
    fn default() -> Self {
        Self::new()
    }
}

/// This thread's index, read from the software thread pointer.
///
/// Components build for `aarch64-sel4-minimal`, which declares no
/// `has-thread-local`, so `#[thread_local]` is unavailable and `sel4`'s own
/// IPC-buffer slot is one process-wide static. `TPIDR_EL0` is per-thread in
/// hardware and the kernel context-switches it, which makes it the one place a
/// thread can keep a value no other thread can see or race. The root sets it
/// through `seL4_TCB_SetTLSBase` at thread creation; the main thread's is zero
/// because that is the register's reset value and the main thread is index 0.
pub fn thread_index() -> usize {
    let base: usize;
    // SAFETY: a register read with no memory operand and no side effects.
    unsafe {
        core::arch::asm!("mrs {base}, tpidr_el0", base = out(reg) base, options(nomem, nostack, preserves_flags));
    }
    // A value outside the table means the root set something this runtime does
    // not model; treating it as the main thread would let two threads share a
    // window, so refuse instead.
    if base >= MAX_THREADS {
        crate::syscall::early_debug_write(b"[slime-rt] thread index out of range\n");
        crate::exit(1)
    }
    base
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
pub unsafe fn start(main: fn(u32), startup_arg: u32) -> ! {
    // The main thread is index 0, which is `TPIDR_EL0`'s reset value, so this
    // reads correctly without the root having set anything.
    let index = thread_index();
    // SAFETY: called once during startup, before any seL4 invocation, and the
    // buffer is mapped for this thread's exclusive use.
    unsafe {
        sel4::set_ipc_buffer(
            (thread_ipc_buffer_addr(index) as *mut sel4::IpcBuffer)
                .as_mut()
                .unwrap(),
        );
    }
    bind_window(index);
    main(startup_arg);
    crate::exit(0)
}

/// Entry path for a component's second thread (B47).
///
/// Distinct from [`start`] in what it must not do: it never calls
/// `sel4::set_ipc_buffer`, because that static is process-wide and overwriting
/// it would repoint the main thread's syscalls at this thread's buffer. This
/// thread reaches its own buffer through the explicit context the transport
/// carries, which is why the transport takes a thread index rather than
/// consulting ambient state.
///
/// Returning ends this thread only; the component exits when its main thread
/// does.
///
/// # Safety
///
/// Must be called once, from the worker entrypoint, on a thread the root
/// created with a distinct `TPIDR_EL0`.
pub unsafe fn start_thread(body: fn(u32), startup_arg: u32) -> ! {
    let index = thread_index();
    if index == 0 {
        crate::syscall::early_debug_write(b"[slime-rt] worker thread has main index\n");
        crate::exit(1)
    }
    bind_window(index);
    body(startup_arg);
    // A worker that returns has finished its work, and there is nothing it can
    // call to retire itself: `exit` terminates the whole task, which would take
    // the main thread with it, and a child holds no TCB capability for its own
    // threads — the root keeps those. So park unschedulably and wait for the
    // root to reclaim the TCB when the task ends.
    //
    // `Wait` on an endpoint the thread does not hold would fault, so this is a
    // spin at the lowest priority the thread was given. A worker is expected to
    // loop rather than return; reaching here at all means its body finished.
    loop {
        core::hint::spin_loop();
    }
}

/// Binds thread `index`'s transfer window.
///
/// A failure here would leave every windowed operation returning
/// `ERR_INVALID_ARG`, so it is fatal. The marker is emitted directly because
/// the missing window is exactly what failed to bind.
fn bind_window(index: usize) {
    if crate::syscall::bind_startup_window(transfer_window_addr(index)) != crate::ERR_SUCCESS {
        crate::syscall::early_debug_write(b"[slime-rt] transfer window bind failed\n");
        crate::exit(1)
    }
}
