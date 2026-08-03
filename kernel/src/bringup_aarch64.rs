//! AArch64 platform bring-up for the `aarch64-qemu-virt` profile.
//!
//! P2.1 established verified EL1 entry, translation, memory, PL011, and the
//! deterministic exit path. P2.2 extends that live path with exception vectors,
//! synchronous fault decoding, a complete `svc`/`eret` register round trip,
//! and shared syscall dispatch. Component scheduling, timer delivery, and
//! devices remain in P2.3–P2.5.

use slime_os_kernel::arch::cpu;
use slime_os_kernel::arch::target::uart;
use slime_os_kernel::{QemuExitCode, boot, exit_qemu, memory, serial_println};

pub fn bringup() {
    // The UART is already usable at its identity-mapped physical address, which
    // is how anything before `memory::init` is visible at all.
    uart::init();
    serial_println!("[serial] Slime OS aarch64-qemu-virt bring-up");

    // Report what the CPU accepted, not what stage-0 intended: an EL2 boot or a
    // silently-refused MMU configuration would otherwise look identical here.
    serial_println!("[serial] exception level EL{}", cpu::exception_level());
    let translation = cpu::translation_config();
    serial_println!(
        "[serial] mmu={} dcache={} icache={} t0sz={} t1sz={}",
        u8::from(translation.mmu_enabled),
        u8::from(translation.data_cache_enabled),
        u8::from(translation.instruction_cache_enabled),
        translation.low_address_size,
        translation.high_address_size,
    );

    // Physical and virtual memory over the direct map stage-0 established.
    // After this, `alloc` works.
    memory::init();
    // Rebase the UART onto the direct map now that its offset is published, so
    // diagnostics no longer depend on the identity map surviving.
    uart::use_direct_map(boot::direct_map_offset());
    serial_println!(
        "[serial] direct map offset={:#x}",
        boot::direct_map_offset()
    );
    {
        let allocator = memory::pmm::FRAME_ALLOCATOR.lock();
        serial_println!(
            "[serial] PMM: {} / {} frames free",
            allocator.free_frames(),
            allocator.total_frames(),
        );
    }
    serial_println!("[serial] heap online");

    // Prove the heap really works before claiming it does.
    {
        use alloc::vec::Vec;
        let mut values = Vec::new();
        for value in 0..256u64 {
            values.push(value * value);
        }
        serial_println!("[serial] heap check: sum={}", values.iter().sum::<u64>());
    }

    report_generation();

    serial_println!("[bringup] aarch64 EL1 vertical slice reached");

    slime_os_kernel::interrupts::init();
    if option_env!("SLIME_AARCH64_TRAP_CHECK") == Some("1") {
        let entry_masked = !cpu::interrupts_enabled();
        // SAFETY: vectors are installed. No GIC source is enabled during this
        // bounded window; it exists only to exercise both DAIF restore paths.
        unsafe { cpu::enable_interrupts() };
        let enabled_window = cpu::interrupts_enabled();
        let masked_inside = cpu::without_interrupts(|| !cpu::interrupts_enabled());
        let restored_enabled = cpu::interrupts_enabled();
        // SAFETY: restore the masked EL1 bring-up state before any later slice
        // enables an interrupt controller.
        unsafe { cpu::disable_interrupts() };
        let final_masked = !cpu::interrupts_enabled();
        serial_println!(
            "[aarch64-trap] daif entry_masked={} enabled_window={} masked_inside={} restored_enabled={} final_masked={}",
            entry_masked,
            enabled_window,
            masked_inside,
            restored_enabled,
            final_masked,
        );
        if !(entry_masked && enabled_window && masked_inside && restored_enabled && final_masked) {
            serial_println!("[aarch64-trap] failed: DAIF mask state was not restored");
            exit_qemu(QemuExitCode::Failed);
        }

        slime_os_kernel::trap::run_el1_breakpoint_probe();

        if !slime_os_kernel::trap::run_user_probe() {
            exit_qemu(QemuExitCode::Failed);
        }
        serial_println!("[aarch64-trap] complete");
    }
    exit_qemu(QemuExitCode::Success);
}

/// Report the generation the verified stage-0 selected, proving the handoff
/// decoded and that this kernel is running the artifact that was authenticated.
fn report_generation() {
    let identity = boot::generation_identity();
    serial_println!(
        "[serial] generation identity={:02x}{:02x}{:02x}{:02x} bytes={}",
        identity[0],
        identity[1],
        identity[2],
        identity[3],
        boot::generation().len(),
    );
    match boot::bootstate() {
        Some(state) => serial_println!(
            "[serial] bootstate slot={} sequence={} attempts={} running_pending={}",
            state.slot,
            state.sequence,
            state.remaining_attempts,
            state.running_pending,
        ),
        None => serial_println!("[serial] bootstate absent"),
    }
}
