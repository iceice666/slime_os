#![no_std]
#![no_main]

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    // B38 proves general CSlot/untyped reclaim survives a fault teardown, not
    // just a clean exit; this task's own private extent is what proves that
    // the same holds for the segmented private backing MEM-ARENAS introduced.
    // A refusal here (declared no quota) is not fatal to the plane: the
    // deliberate fault below still exercises the general path, and the
    // gate's kind-specific reusable-byte assertion is what would catch a
    // missing grant, not this call.
    let _ = slime_rt::private_memory_grow(1);
    slime_rt::debug_write(b"[reclamation-fault] deliberate fault\n");
    // SAFETY: B38 deliberately exercises the supervised VM-fault reclamation
    // path. Address zero is never mapped in a component VSpace.
    unsafe { (core::ptr::null_mut::<u64>()).write_volatile(0xB38) }
}
