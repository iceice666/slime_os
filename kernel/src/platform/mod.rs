#![cfg(target_arch = "x86_64")]

//! PC-class platform assembly for the `x86_64-qemu-virtio` profile.
//!
//! These modules describe the *machine*, not the instruction set: firmware
//! description tables ([`acpi`]), the PCI ECAM configuration space ([`pci`]),
//! the ACPI-described i8042 keyboard transport ([`i8042_keyboard`]), and
//! ACPI-described reset/power-off ([`power`]). Splitting them from
//! `crate::arch` keeps ACPI/PCI/UEFI policy from becoming the interface a
//! device-tree platform such as `aarch64-rpi5` would have to implement.
//!
//! They are selected by target profile: an AArch64 profile supplies device-tree
//! discovery in their place.

pub mod acpi;
pub mod i8042_keyboard;
pub mod pci;
pub mod power;
