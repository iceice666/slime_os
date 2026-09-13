#![no_std]
#![no_main]

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    // Request private backing before the deliberate fault so teardown must
    // reclaim a private extent as well as the task's static resources.
    let _ = slime_rt::private_memory_grow(1);
    slime_rt::debug_write(b"[reclamation-fault] deliberate fault\n");
    // SAFETY: B38 deliberately exercises the supervised VM-fault reclamation
    // path. Address zero is never mapped in a component VSpace.
    unsafe { (core::ptr::null_mut::<u64>()).write_volatile(0xB38) }
}
