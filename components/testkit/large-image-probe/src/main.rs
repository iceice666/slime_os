#![no_std]
#![no_main]

slime_rt::entry!(main);

#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(include_str!(concat!(env!("OUT_DIR"), "/regions.S")));

const INITIALIZED_BYTES: usize = 36 * 1024 * 1024;
const ZERO_BYTES: usize = 24 * 1024 * 1024;

#[cfg(target_arch = "aarch64")]
unsafe extern "C" {
    static large_image_rodata: u64;
    static mut large_image_data: u64;
    static mut large_image_bss: u64;
    fn large_image_text_0() -> u64;
    fn large_image_text_1() -> u64;
    fn large_image_text_2() -> u64;
}

fn fail() -> ! {
    slime_rt::debug_write(b"[large-image-probe] FAIL verification\n");
    slime_rt::exit(1)
}

#[cfg(target_arch = "aarch64")]
fn main(_startup_arg: u32) {
    let incarnation = loop {
        match slime_rt::lifecycle_parameter_read(slime_rt::PARAMETER_SELF_SLOT, 0) {
            Ok(value @ 1..=4) => break value,
            Ok(_) => slime_rt::yield_now(),
            Err(slime_rt::ERR_INVALID_ARG) => slime_rt::yield_now(),
            Err(_) => fail(),
        }
    };
    // Parameter state survives task reclamation. Consume this token before
    // either terminal path so the next incarnation waits for its own token.
    slime_rt::lifecycle_parameter_write(slime_rt::PARAMETER_SELF_SLOT, 0, 0)
        .unwrap_or_else(|_| fail());
    // SAFETY: the linker reserves each complete region at these symbols; every
    // access below stays inside its region, and only writable regions are written.
    unsafe {
        for (base, bytes, seed, writable) in [
            (
                core::ptr::addr_of!(large_image_rodata).cast_mut(),
                INITIALIZED_BYTES,
                0x524f_4441_5441_0000_u64,
                false,
            ),
            (
                core::ptr::addr_of_mut!(large_image_data),
                INITIALIZED_BYTES,
                0x4441_5441_0000_0000_u64,
                true,
            ),
            (
                core::ptr::addr_of_mut!(large_image_bss),
                ZERO_BYTES,
                0,
                true,
            ),
        ] {
            for word in 0..bytes / 8 {
                let expected = if seed == 0 { 0 } else { seed ^ word as u64 };
                if base.add(word).read_volatile() != expected {
                    fail();
                }
            }
            if writable {
                // Separate write and read passes also detect aliased image frames.
                for word in 0..bytes / 8 {
                    base.add(word)
                        .write_volatile(0xa55a_0000_0000_0000 ^ incarnation ^ word as u64);
                }
                for word in 0..bytes / 8 {
                    if base.add(word).read_volatile()
                        != 0xa55a_0000_0000_0000 ^ incarnation ^ word as u64
                    {
                        fail();
                    }
                }
            }
        }
        if large_image_text_0() != 1 || large_image_text_1() != 2 || large_image_text_2() != 3 {
            fail();
        }
    }
    slime_rt::debug_write(
        b"[large-image-probe] verified text_calls=3 rodata=37748736 data=37748736 bss=25165824\n",
    );
    if incarnation == 3 {
        // SAFETY: issue the deliberate permission-faulting store as an instruction,
        // not a Rust write through a pointer to an immutable object.
        unsafe {
            core::arch::asm!(
                "str xzr, [{address}]",
                address = in(reg) core::ptr::addr_of!(large_image_rodata),
                options(nostack),
            );
        }
        fail();
    }
}

#[cfg(not(target_arch = "aarch64"))]
fn main(_startup_arg: u32) {
    let _ = (INITIALIZED_BYTES, ZERO_BYTES);
    fail();
}
