#![no_std]
#![no_main]

use slime_rt::{
    SpawnGrant, Termination, debug_write, exit, io_device_bind, io_irq_wait_ack, io_mmio_map,
    io_mmio_read32, io_queue_map, lifecycle_restart_admit, resolve_binding, spawn,
    supervision_status, yield_now,
};

slime_rt::entry!(main);

const DEVICE_SLOT: u32 = 0;
const REGION_SLOT: u32 = 1;
const IRQ_SLOT: u32 = 2;
const RIGHT_MAP_MMIO: u64 = 16;
const RIGHT_DMA_PIN: u64 = 32;
const RIGHT_DMA_RELEASE: u64 = 64;
const RIGHT_IRQ_ACK: u64 = 128;
const DMA_SLOT: u32 = 3;
const BASE: u64 = 0x0000_0014_0000_0000;
const DMA_BASE: u64 = BASE + 0x1000;

fn main(_: u32) {
    if let Ok(executable) = resolve_binding(b"io-driver-worker-executable") {
        run_supervisor(executable)
    }
    run_driver()
}

fn run_supervisor(executable: u32) -> ! {
    let grants = [
        grant(
            resolve_binding(b"probe-device").unwrap_or_else(|_| fail(b"resolve device")),
            RIGHT_MAP_MMIO,
        ),
        grant(
            resolve_binding(b"probe-mmio").unwrap_or_else(|_| fail(b"resolve mmio")),
            RIGHT_MAP_MMIO,
        ),
        grant(
            resolve_binding(b"probe-irq").unwrap_or_else(|_| fail(b"resolve irq")),
            RIGHT_IRQ_ACK,
        ),
        grant(
            resolve_binding(b"probe-dma").unwrap_or_else(|_| fail(b"resolve dma")),
            RIGHT_DMA_PIN | RIGHT_DMA_RELEASE,
        ),
    ];
    let first = spawn(executable, &grants).unwrap_or_else(|_| fail(b"initial driver spawn"));
    let subject = slime_rt::supervision_derive(first.supervision_slot)
        .unwrap_or_else(|_| fail(b"derive restart subject"));
    loop {
        match supervision_status(first.supervision_slot) {
            Ok(Some(Termination::Fault(_))) => break,
            Ok(None) => yield_now(),
            _ => fail(b"driver did not fault"),
        }
    }
    debug_write(b"[io-driver-supervisor] predecessor fault collected\n");
    let admission = lifecycle_restart_admit(subject).unwrap_or_else(|_| fail(b"restart admit"));
    while slime_rt::monotonic_read().is_ok_and(|now| now < admission.ready_at) {
        yield_now();
    }
    let second = spawn(executable, &[]).unwrap_or_else(|_| fail(b"replacement spawn"));
    loop {
        match supervision_status(second.supervision_slot) {
            Ok(Some(Termination::Exit(0))) => break,
            Ok(None) => yield_now(),
            _ => fail(b"replacement failed"),
        }
    }
    debug_write(b"[io-driver-supervisor] replacement completed\n");
    debug_write(b"[io-driver-probe] io driver authority plane complete\n");
    exit(0)
}
fn run_driver() -> ! {
    let device = io_device_bind(DEVICE_SLOT).unwrap_or_else(|_| fail(b"bind granted device"));
    debug_write(b"[io-driver-probe] bind exactly one device proven\n");
    if device.epoch > 1 {
        if io_mmio_read32(DEVICE_SLOT, REGION_SLOT, device.epoch - 1, 0).is_ok() {
            fail(b"predecessor epoch accepted");
        }
        write_value(b"[io-driver-probe] fresh epoch=", device.epoch);
        write_value(
            b"[io-driver-probe] predecessor epoch refused=",
            device.epoch - 1,
        );
        exit(0)
    }

    if io_mmio_map(DEVICE_SLOT, REGION_SLOT, device.epoch, BASE, 0, 0x200).is_ok() {
        fail(b"shared-granule direct map widened");
    }
    debug_write(b"[io-driver-probe] shared-granule direct map refused not widened\n");

    let magic = io_mmio_read32(DEVICE_SLOT, REGION_SLOT, device.epoch, 0)
        .unwrap_or_else(|_| fail(b"mediated in-range read"));
    if magic != 0x7472_6976 {
        fail(b"mediated read wrong transport");
    }
    if io_mmio_read32(DEVICE_SLOT, REGION_SLOT, device.epoch, 0x200).is_ok() {
        fail(b"mediated adjacent transport exposed");
    }
    if io_mmio_read32(DEVICE_SLOT, REGION_SLOT, device.epoch + 1, 0).is_ok() {
        fail(b"mediated stale epoch accepted");
    }
    debug_write(b"[io-driver-probe] qemu packed transport mediated exact range proven\n");

    if device.epoch > 1 {
        if io_mmio_read32(DEVICE_SLOT, REGION_SLOT, device.epoch - 1, 0).is_ok() {
            fail(b"predecessor epoch accepted");
        }
        write_value(b"[io-driver-probe] fresh epoch=", device.epoch);
        write_value(
            b"[io-driver-probe] predecessor epoch refused=",
            device.epoch - 1,
        );
        debug_write(b"[io-driver-probe] opaque dma path exposes no physical address proven\n");
        exit(0)
    }

    io_mmio_map(DEVICE_SLOT, REGION_SLOT, device.epoch, BASE, 0, 0x1000)
        .unwrap_or_else(|_| fail(b"live mmio map"));
    if io_irq_wait_ack(IRQ_SLOT, device.epoch, 0).is_ok() {
        debug_write(b"[io-driver-probe] declared interrupt acknowledge proven\n");
    }
    let _queue = io_queue_map(DMA_SLOT, device.epoch, DMA_BASE, 2)
        .unwrap_or_else(|_| fail(b"live queue dma"));
    debug_write(b"[io-driver-probe] declared interrupt bound no-spoof proven\n");
    debug_write(b"[io-driver-probe] opaque dma path exposes no physical address proven\n");
    debug_write(b"[io-driver-probe] faulting with live authority\n");
    unsafe {
        core::ptr::null_mut::<u64>().write_volatile(1);
    }
    fail(b"null write did not fault")
}

const fn grant(slot: u32, rights: u64) -> SpawnGrant {
    SpawnGrant { slot, rights }
}

fn write_value(prefix: &[u8], value: u64) {
    debug_write(prefix);
    let mut digits = [0u8; 20];
    let mut cursor = digits.len();
    let mut remaining = value;
    if remaining == 0 {
        cursor -= 1;
        digits[cursor] = b'0';
    }
    while remaining != 0 {
        cursor -= 1;
        digits[cursor] = b'0' + (remaining % 10) as u8;
        remaining /= 10;
    }
    debug_write(&digits[cursor..]);
    debug_write(b"\n");
}

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[io-driver-probe] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
