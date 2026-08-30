#![no_std]
#![no_main]

use slime_rt::{
    DmaDirection, DmaMapping, ERR_SUCCESS, debug_write, exit, io_device_bind, io_dma_map,
    io_dma_release, io_irq_ack, io_mmio_map, io_mmio_read32,
};

slime_rt::entry!(main);

fn main(_: u32) {
    let mut device_denials = 0;
    let mut mmio_denials = 0;
    let mut dma_denials = 0;
    let mut interrupt_denials = 0;

    if io_device_bind(0).is_ok() {
        fail(b"device enumeration");
    }
    device_denials += 1;

    if io_mmio_map(0, 1, 1, 0x0000_0015_0000_0000, 0, 0x200).is_ok() {
        fail(b"mmio map");
    }
    mmio_denials += 1;
    if io_mmio_read32(0, 1, 1, 0).is_ok() {
        fail(b"mmio read");
    }
    mmio_denials += 1;

    if io_dma_map(2, 3, 1, DmaDirection::DeviceRead).is_ok() {
        fail(b"dma map");
    }
    dma_denials += 1;
    if io_dma_release(
        2,
        DmaMapping {
            id: 1,
            epoch: 1,
            iova: 0,
        },
    ) == ERR_SUCCESS
    {
        fail(b"dma release");
    }
    dma_denials += 1;

    if io_irq_ack(4, 1, 0).is_ok() {
        fail(b"irq ack");
    }
    interrupt_denials += 1;

    write_number(b"[io-driver-intruder] denied device=", device_denials);
    write_number(b" mmio=", mmio_denials);
    write_number(b" dma=", dma_denials);
    write_number(b" interrupt=", interrupt_denials);
    debug_write(b"\n");
    exit(0)
}

fn write_number(prefix: &[u8], mut value: u64) {
    let mut digits = [0u8; 20];
    let mut offset = digits.len();
    loop {
        offset -= 1;
        digits[offset] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    debug_write(prefix);
    debug_write(&digits[offset..]);
}

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[io-driver-intruder] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
