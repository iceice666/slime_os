#![no_std]
#![no_main]

slime_rt::entry!(main);

fn main() {
    slime_rt::debug_write(b"[reclamation-fault] deliberate fault\n");
    // SAFETY: B38 deliberately exercises the supervised VM-fault reclamation
    // path. Address zero is never mapped in a component VSpace.
    unsafe { (core::ptr::null_mut::<u64>()).write_volatile(0xB38) }
}
