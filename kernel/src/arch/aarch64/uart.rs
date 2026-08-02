//! The PL011 UART used for AArch64 diagnostics.
//!
//! P2 implements the MMIO register access once the device base is discovered
//! from the firmware handoff or device tree.

/// Configure the UART for diagnostic output.
pub fn init() {
    unimplemented!("aarch64 PL011 init: implemented by P2")
}

/// Transmit one byte.
pub fn write_byte(_byte: u8) {
    unimplemented!("aarch64 PL011 transmit: implemented by P2")
}
