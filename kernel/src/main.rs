#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use core::panic::PanicInfo;

#[cfg(not(test))]
use boot_contracts::handoff::KernelHandoffV1;
use slime_os_kernel::frame_buffer::init_framebuffer;
use slime_os_kernel::{acpi, gdt, input, interrupts, memory, pci, println, serial_println, time};
#[cfg(test)]
mod testing;

/// Kernel entry point.
///
/// # Safety
///
/// Production stage-0 supplies a verified `KernelHandoffV1`; the Cargo test
/// runner uses Limine and initializes the legacy test boot context instead.
#[cfg(not(test))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start(handoff: *const KernelHandoffV1) -> ! {
    unsafe { slime_os_kernel::boot::init_from_handoff(handoff) };
    kernel_main();
    slime_os_kernel::hlt_loop()
}

#[cfg(test)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    slime_os_kernel::limine::ensure_linked();
    unsafe { slime_os_kernel::boot::init_from_limine() };
    kernel_main();
    slime_os_kernel::hlt_loop()
}

fn kernel_main() {
    // Console first: every subsequent diagnostic is visible.
    init_framebuffer();
    println!("Slime OS Framework bring-up image");
    serial_println!("[serial] Hello World{}!", "");

    // GDT/TSS before the IDT: the IDT gates read our code selector, and the
    // Double Fault gate needs the TSS's IST stack to exist.
    gdt::init();
    serial_println!("[serial] GDT+TSS loaded");
    println!("[bringup] GDT+TSS loaded");

    // Load the IDT so exceptions route to our handlers instead of
    // triple-faulting QEMU into a silent reset.
    interrupts::init();
    serial_println!("[serial] IDT loaded");
    println!("[bringup] IDT loaded");

    // Physical + virtual memory and the kernel heap. After this, `alloc`
    // works and page faults are reported deterministically.
    memory::init();
    {
        let fa = memory::pmm::FRAME_ALLOCATOR.lock();
        serial_println!(
            "[serial] PMM: {} / {} frames free",
            fa.free_frames(),
            fa.total_frames(),
        );
    }
    serial_println!("[serial] heap online");
    println!("[bringup] heap online");

    let platform = acpi::init().expect("ACPI discovery failed");
    let io_apics = platform.madt.io_apics.iter().flatten().count();
    serial_println!(
        "[acpi] revision={} root={:?} tables={} ioapics={} i8042={}",
        platform.revision,
        platform.root_kind,
        platform.table_count,
        io_apics,
        platform.i8042_present,
    );
    println!(
        "[bringup] ACPI {:?}: {} tables",
        platform.root_kind, platform.table_count
    );

    // PCI ECAM discovery (M5.1). Best-effort: enumerate and report the
    // bounded function set; an absent MCFG is non-fatal for the existing
    // component vertical slice.
    match pci::init() {
        Ok(segments) => {
            let functions = pci::enumerate().unwrap_or_default();
            serial_println!(
                "[pci] {} segment(s), {} function(s)",
                segments.len(),
                functions.len(),
            );
            for f in functions.iter().take(8) {
                serial_println!(
                    "[pci] seg{} {:#04x}:{:02x}.{} vendor={:#06x} device={:#06x} class={:#06x}",
                    f.segment,
                    f.bus,
                    f.device,
                    f.function,
                    f.vendor_id,
                    f.device_id,
                    f.class_code,
                );
            }
        }
        Err(error) => {
            serial_println!("[pci] MCFG/ECAM unavailable: {:?}", error);
        }
    }

    // Prove the heap really works before relying on it.
    {
        use alloc::vec::Vec;
        let mut v = Vec::new();
        for i in 0..256 {
            v.push(i * i);
        }
        serial_println!("[serial] heap check: sum={}", v.iter().sum::<u64>());
    }

    // APIC timer: interrupt-driven monotonic clock. Enables interrupts.
    time::init();
    serial_println!(
        "[serial] APIC timer online (count={})",
        time::apic::timer_count()
    );
    println!("[bringup] APIC timer online");

    match input::init(&platform.madt, platform.i8042_present) {
        Ok(()) => println!("[bringup] keyboard online"),
        Err(error) => {
            serial_println!("[input] keyboard unavailable: {:?}", error);
            println!("[bringup] keyboard unavailable: {:?}", error);
        }
    }
    serial_println!("[platform] ACPI shutdown/reset mechanisms discovered");
    serial_println!("[policy] storage writes require an explicit disposable-QEMU generation grant");

    #[cfg(not(test))]
    {
        // #BP: trigger a breakpoint and prove we come back.
        // SAFETY: `int3` is a trap; our #BP handler returns normally.
        unsafe {
            core::arch::asm!("int3", options(nostack, preserves_flags));
        }
        serial_println!("[serial] survived int3");
        println!("[bringup] survived int3");

        // Prove the timer is ticking: wait ~50 ms and confirm uptime advanced.
        let before = time::ticks();
        time::sleep_ms(50);
        serial_println!(
            "[serial] timer ticks: {} -> {} (uptime {} ms)",
            before,
            time::ticks(),
            time::uptime_ms(),
        );
        println!("[bringup] timer ticks {} -> {}", before, time::ticks(),);

        slime_os_kernel::bootstrap::start();
    }

    #[cfg(test)]
    test_main();
}

/// This function is called on panic.
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    slime_os_kernel::hlt_loop()
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    slime_os_kernel::test_panic_handler(info)
}
