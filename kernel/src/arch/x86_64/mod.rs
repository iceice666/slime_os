//! The x86-64 architecture mechanisms.
//!
//! Only ISA and firmware-entry mechanism lives here: CPU control ([`cpu`]),
//! translation tables ([`paging`]), traps and the saved user frame ([`trap`]),
//! privilege transitions ([`context`]), descriptor tables ([`gdt`],
//! [`interrupts`]), the interrupt controller and timer ([`apic`]), the
//! diagnostic UART ([`uart`]), the legacy keyboard controller ([`i8042`]), and
//! the firmware handoff ([`boot`], [`limine`]).
//!
//! Machine assembly that is not ISA mechanism — ACPI tables, PCI ECAM, and
//! power control — lives in `crate::platform`, so a different machine with the
//! same ISA replaces those without touching these files.

pub mod apic;
pub mod boot;
pub mod context;
pub mod cpu;
pub mod gdt;
pub mod i8042;
pub mod interrupts;
pub mod limine;
pub mod paging;
pub mod trap;
pub mod uart;
