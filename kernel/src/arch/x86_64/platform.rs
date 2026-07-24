//! Platform reset and power-off mechanisms discovered through ACPI.
//!
//! These functions are kernel mechanisms only. Userspace policy decides when
//! they may be invoked; early Framework bring-up calls them only from explicit
//! diagnostics or a trusted service path.

use crate::acpi::GenericAddress;
use crate::{println, serial_println};

const SLP_EN: u16 = 1 << 13;
const SLP_TYP_SHIFT: u16 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformError {
    AcpiUnavailable,
    UnsupportedAddressSpace(u8),
    InvalidRegister,
    ResetUnavailable,
    ShutdownUnavailable,
}

pub fn reset() -> ! {
    match try_reset() {
        Ok(()) => unreachable!("platform reset returned"),
        Err(error) => {
            serial_println!("[platform] ACPI reset unavailable: {:?}", error);
            println!("[platform] ACPI reset failed: {:?}", error);
            // The i8042 controller reset command is a bounded legacy fallback.
            if wait_input_buffer_empty() {
                unsafe { outb(0x64, 0xfe) };
                for _ in 0..1_000_000 {
                    core::hint::spin_loop();
                }
            }
            // Last-resort reset: load an empty IDT and trigger a triple fault.
            #[repr(C, packed)]
            struct Idtr {
                limit: u16,
                base: u64,
            }
            let empty = Idtr { limit: 0, base: 0 };
            unsafe {
                core::arch::asm!(
                    "cli",
                    "lidt [{}]",
                    "int3",
                    in(reg) &empty,
                    options(noreturn),
                );
            }
        }
    }
}

pub fn shutdown_or_reset() -> ! {
    match try_shutdown() {
        Ok(()) => unreachable!("platform shutdown returned"),
        Err(error) => {
            serial_println!("[platform] shutdown unavailable: {:?}; resetting", error);
            println!("[platform] shutdown failed: {:?}; resetting", error);
            reset()
        }
    }
}

pub fn shutdown() -> ! {
    match try_shutdown() {
        Ok(()) => unreachable!("platform shutdown returned"),
        Err(error) => {
            serial_println!("[platform] shutdown unavailable: {:?}", error);
            println!("[platform] shutdown failed: {:?}", error);
            crate::hlt_loop()
        }
    }
}

pub fn try_reset() -> Result<(), PlatformError> {
    let power = &crate::acpi::get()
        .ok_or(PlatformError::AcpiUnavailable)?
        .power;
    let register = power
        .reset_register
        .ok_or(PlatformError::ResetUnavailable)?;
    write_register(register, power.reset_value as u64)
}

pub fn try_shutdown() -> Result<(), PlatformError> {
    let power = &crate::acpi::get()
        .ok_or(PlatformError::AcpiUnavailable)?
        .power;
    if power.hardware_reduced {
        let register = power
            .sleep_control
            .ok_or(PlatformError::ShutdownUnavailable)?;
        // Reduced-hardware sleep control: SLP_TYP is bits 2..4, SLP_EN is bit 5.
        write_register(register, ((power.s5_type_a as u64) << 2) | (1 << 5))?;
        return wait_for_poweroff();
    }

    let pm1a = power
        .pm1a_control
        .ok_or(PlatformError::ShutdownUnavailable)?;
    let value_a = ((power.s5_type_a as u16) << SLP_TYP_SHIFT) | SLP_EN;
    write_register(pm1a, value_a as u64)?;
    if let Some(pm1b) = power.pm1b_control {
        let value_b = ((power.s5_type_b as u16) << SLP_TYP_SHIFT) | SLP_EN;
        write_register(pm1b, value_b as u64)?;
    }
    wait_for_poweroff()
}

fn wait_for_poweroff() -> Result<(), PlatformError> {
    for _ in 0..1_000_000 {
        core::hint::spin_loop();
    }
    Err(PlatformError::ShutdownUnavailable)
}

fn write_register(register: GenericAddress, value: u64) -> Result<(), PlatformError> {
    if register.address == 0 || register.bit_offset != 0 {
        return Err(PlatformError::InvalidRegister);
    }
    let width = register_width(register)?;
    match register.address_space {
        0 => {
            let address = crate::memory::PhysAddr(register.address).to_virt().as_u64();
            unsafe {
                match width {
                    1 => core::ptr::write_volatile(address as *mut u8, value as u8),
                    2 => core::ptr::write_volatile(address as *mut u16, value as u16),
                    4 => core::ptr::write_volatile(address as *mut u32, value as u32),
                    8 => core::ptr::write_volatile(address as *mut u64, value),
                    _ => return Err(PlatformError::InvalidRegister),
                }
            }
        }
        1 if register.address <= u16::MAX as u64 => unsafe {
            let port = register.address as u16;
            match width {
                1 => outb(port, value as u8),
                2 => outw(port, value as u16),
                4 => outl(port, value as u32),
                _ => return Err(PlatformError::InvalidRegister),
            }
        },
        space => return Err(PlatformError::UnsupportedAddressSpace(space)),
    }
    Ok(())
}

fn register_width(register: GenericAddress) -> Result<usize, PlatformError> {
    match register.access_size {
        1 => Ok(1),
        2 => Ok(2),
        3 => Ok(4),
        4 => Ok(8),
        0 => match register.bit_width {
            1..=8 => Ok(1),
            9..=16 => Ok(2),
            17..=32 => Ok(4),
            33..=64 => Ok(8),
            _ => Err(PlatformError::InvalidRegister),
        },
        _ => Err(PlatformError::InvalidRegister),
    }
}

fn wait_input_buffer_empty() -> bool {
    for _ in 0..100_000 {
        if unsafe { inb(0x64) } & 0x02 == 0 {
            return true;
        }
    }
    false
}

unsafe fn outb(port: u16, value: u8) {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

unsafe fn outw(port: u16, value: u16) {
    unsafe {
        core::arch::asm!(
            "out dx, ax",
            in("dx") port,
            in("ax") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

unsafe fn outl(port: u16, value: u32) {
    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") port,
            in("eax") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!(
            "in al, dx",
            out("al") value,
            in("dx") port,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}
