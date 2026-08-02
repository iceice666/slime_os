//! AArch64 platform bring-up for the `aarch64-qemu-virt` profile.
//!
//! P2.1's scope: prove the kernel reached EL1 with the MMU on, that the
//! stage-0 handoff decodes, that physical and virtual memory management come up
//! over the direct map, and that the result is observable over PL011 and
//! reported through a deterministic exit.
//!
//! What is deliberately absent is the rest of P2. There are no exception
//! vectors (P2.2), no components (P2.3), no timer or interrupt controller
//! (P2.4), and no block transport (P2.5), so this does not call
//! `bootstrap::start` — launching components without vectors installed would
//! fault into nothing. Bring-up ends by reporting success and exiting.

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
    // P2.2 installs exception vectors; until then there is nothing further to
    // run, so report success through the profile's exit path rather than
    // spinning and timing the gate out.
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
