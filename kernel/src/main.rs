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
use slime_os_kernel::{
    acpi, gdt, hardware_inventory, input, interrupts, memory, pci, println, serial_println, time,
};
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
    // UEFI may leave floating-point/SIMD state disabled. Rust code can emit
    // SSE2 instructions on x86_64, so establish the architectural baseline
    // before calling any ordinary Rust function.
    unsafe {
        core::arch::asm!(
            "mov rax, cr0",
            "and rax, {clear_em}",
            "or rax, {set_mp}",
            "mov cr0, rax",
            "mov rax, cr4",
            "or rax, {set_osfxsr}",
            "mov cr4, rax",
            clear_em = const !(1u64 << 2),
            set_mp = const 1u64 << 1,
            set_osfxsr = const (1u64 << 9) | (1u64 << 10),
            out("rax") _,
            options(nostack),
        );
    }
    unsafe extern "C" {
        static mut __bss_start: u8;
        static mut __stop_data: u8;
    }
    unsafe {
        let start = core::ptr::addr_of_mut!(__bss_start);
        let end = core::ptr::addr_of_mut!(__stop_data);
        core::ptr::write_bytes(start, 0, end.offset_from(start) as usize);
    }
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

    // PCI ECAM discovery (M5.1). Preserve typed failures; inventory mode needs
    // to distinguish an unavailable MCFG from an empty topology.
    let functions = pci::init().and_then(|segments| {
        pci::enumerate().inspect(|functions| {
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
        })
    });
    if let Err(error) = &functions {
        serial_println!("[pci] discovery failed: {:?}", error);
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

    let usb_controller_present = functions.as_ref().is_ok_and(|functions| {
        functions
            .iter()
            .any(|function| function.class_code & 0x00ff_ffff == 0x0c_03_30)
    });
    let input_report = input::init_with_report(
        &platform.madt,
        platform.i8042_present,
        usb_controller_present,
    );
    match input_report.result() {
        Ok(()) => println!("[bringup] keyboard online"),
        Err(error) => {
            serial_println!("[input] keyboard unavailable: {:?}", error);
            println!("[bringup] keyboard unavailable: {:?}", error);
        }
    }
    if option_env!("SLIME_FRAMEWORK_INVENTORY") == Some("1") {
        let inventory_result = hardware_inventory::emit(
            platform,
            functions
                .as_ref()
                .map(|items| items.as_slice())
                .map_err(|error| *error),
            &input_report,
        );
        if let Err(error) = inventory_result {
            serial_println!("[hw-report] failed error={:?}", error);
            println!("[hw-report] failed error={:?}", error);
        }
        #[cfg(not(test))]
        if option_env!("SLIME_FRAMEWORK_INVENTORY_QEMU") == Some("1") {
            let code = if inventory_result.is_ok() {
                slime_os_kernel::QemuExitCode::Success
            } else {
                slime_os_kernel::QemuExitCode::Failed
            };
            slime_os_kernel::exit_qemu(code);
            slime_os_kernel::hlt_loop();
        }
        #[cfg(not(test))]
        slime_os_kernel::platform::shutdown_or_reset();
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
