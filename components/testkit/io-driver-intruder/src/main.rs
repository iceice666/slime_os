#![no_std]
#![no_main]

use slime_rt::{debug_write, exit, io_device_bind, io_irq_wait_ack, io_mmio_map, io_mmio_read32};

slime_rt::entry!(main);

fn main(_: u32) {
    if io_device_bind(0).is_ok() {
        fail(b"device enumeration");
    }
    if io_mmio_map(0, 1, 1, 0x0000_0015_0000_0000, 0, 0x200).is_ok() {
        fail(b"mmio map");
    }
    if io_mmio_read32(0, 1, 1, 0).is_ok() {
        fail(b"mmio read");
    }
    if io_irq_wait_ack(2, 1, 0).is_ok() {
        fail(b"irq ack");
    }
    debug_write(b"[io-driver-intruder] device mmio dma interrupt denials proven\n");
    exit(0)
}

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[io-driver-intruder] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
