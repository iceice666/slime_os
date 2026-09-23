#![no_std]
#![no_main]

//! An adaptive holder that also drives a device.
//!
//! The private-memory planes prove that shared buffers and loans are refused
//! inside a holder's reserved window. Device mappings reach the same address
//! space through a different service, so a window they did not exclude would
//! be defended against one caller and open to another. This holder therefore
//! binds a real device, obtains its authority, and only then asks for MMIO and
//! queue mappings at three addresses whose expected answers differ: its own
//! backed page, an unbacked address inside the same reservation, and an
//! address outside it.
//!
//! Both refusals must happen after the capability and rights checks succeed,
//! which is why the positive controls run first: a refusal produced by missing
//! authority would prove nothing about the window.

use slime_rt::{
    SpawnGrant, debug_write, exit, io_device_bind, io_mmio_map, io_queue_map, private_memory_grow,
    resolve_binding, spawn,
};

slime_rt::entry!(main);

const DEVICE_SLOT: u32 = 0;
const REGION_SLOT: u32 = 1;
const DMA_SLOT: u32 = 2;
const RIGHT_MAP_MMIO: u64 = 16;
const RIGHT_DMA_PIN: u64 = 32;
const RIGHT_DMA_RELEASE: u64 = 64;

/// Outside every child's private window: the loader places an image and its
/// thread pages far below this, and a window is reserved above them.
const OUTSIDE_BASE: u64 = 0x0000_0014_0000_0000;
const OUTSIDE_DMA_BASE: u64 = OUTSIDE_BASE + 0x1000;
/// An offset inside the holder's reservation that its single backed page does
/// not cover. The declared maximum is larger than this, so the address is
/// reserved and unbacked rather than merely unmapped.
const UNBACKED_OFFSET: u64 = 64 * 1024 * 1024;

fn main(_: u32) {
    if let Ok(executable) = resolve_binding(b"adaptive-io-executable") {
        run_supervisor(executable)
    }
    run_holder()
}

fn run_supervisor(executable: u32) -> ! {
    let grants = [
        grant(
            resolve_binding(b"adaptive-io-device").unwrap_or_else(|_| fail(b"resolve device")),
            RIGHT_MAP_MMIO,
        ),
        grant(
            resolve_binding(b"adaptive-io-mmio").unwrap_or_else(|_| fail(b"resolve mmio")),
            RIGHT_MAP_MMIO,
        ),
        grant(
            resolve_binding(b"adaptive-io-dma").unwrap_or_else(|_| fail(b"resolve dma")),
            RIGHT_DMA_PIN | RIGHT_DMA_RELEASE,
        ),
    ];
    // One retry, because a construction that failed after its incarnation was
    // bound must leave the subject spawnable again; a second failure is a
    // real one.
    if spawn(executable, &grants).is_err() {
        debug_write(b"[private-adaptive:io-supervisor] spawn refused, retrying\n");
        spawn(executable, &grants).unwrap_or_else(|_| fail(b"io holder respawn"));
    }
    debug_write(b"[private-adaptive:io-supervisor] holder spawned\n");
    exit(0)
}

fn run_holder() -> ! {
    // One real page, so the first refused address is backed private memory
    // rather than an address that merely falls inside a reservation.
    private_memory_grow(1).unwrap_or_else(|_| fail(b"first private page"));
    // The growth answers the count *before* it, so the holder's own state is
    // read back with a zero-delta query rather than inferred from the reply.
    let region = private_memory_grow(0).unwrap_or_else(|_| fail(b"window query"));
    let base = region.base as u64;
    if base == 0 || region.pages != 1 {
        fail(b"holder has no window")
    }
    let device = io_device_bind(DEVICE_SLOT).unwrap_or_else(|_| fail(b"bind granted device"));

    // Positive controls first: the same capabilities, rights and epoch that the
    // refusals below use, proving the refusals are the destination's doing.
    io_mmio_map(
        DEVICE_SLOT,
        REGION_SLOT,
        device.epoch,
        OUTSIDE_BASE,
        0,
        0x1000,
    )
    .unwrap_or_else(|_| fail(b"mmio map outside the window"));
    io_queue_map(DMA_SLOT, device.epoch, OUTSIDE_DMA_BASE, 2)
        .unwrap_or_else(|_| fail(b"queue map outside the window"));
    debug_write(b"[private-adaptive:io] outside_window mmio=1 queue=1\n");

    let backed_refused =
        u64::from(io_mmio_map(DEVICE_SLOT, REGION_SLOT, device.epoch, base, 0, 0x1000).is_err());
    let unbacked_refused = u64::from(
        io_mmio_map(
            DEVICE_SLOT,
            REGION_SLOT,
            device.epoch,
            base + UNBACKED_OFFSET,
            0,
            0x1000,
        )
        .is_err(),
    );
    let queue_backed_refused = u64::from(io_queue_map(DMA_SLOT, device.epoch, base, 2).is_err());
    let queue_unbacked_refused =
        u64::from(io_queue_map(DMA_SLOT, device.epoch, base + UNBACKED_OFFSET, 2).is_err());
    if backed_refused != 1
        || unbacked_refused != 1
        || queue_backed_refused != 1
        || queue_unbacked_refused != 1
    {
        fail(b"a device mapping entered the private window")
    }

    // The window survived every attempt: its one backed page still answers,
    // and its page count is unchanged by the four refusals.
    let after = private_memory_grow(0).unwrap_or_else(|_| fail(b"window query"));
    if after.base as u64 != base || after.pages != region.pages {
        fail(b"a refused device mapping changed the window")
    }
    unsafe {
        (base as *mut u64).write_volatile(0x5ade_0001);
        if (base as *const u64).read_volatile() != 0x5ade_0001 {
            fail(b"the holder's own page stopped answering")
        }
    }
    write_value(
        b"[private-adaptive:io] mmio_backed_refused=",
        backed_refused,
    );
    write_value(
        b"[private-adaptive:io] mmio_unbacked_refused=",
        unbacked_refused,
    );
    write_value(
        b"[private-adaptive:io] queue_backed_refused=",
        queue_backed_refused,
    );
    write_value(
        b"[private-adaptive:io] queue_unbacked_refused=",
        queue_unbacked_refused,
    );
    write_value(b"[private-adaptive:io] pages=", after.pages as u64);
    debug_write(b"[private-adaptive:io] device mappings excluded from the private window\n");
    exit(0)
}

const fn grant(slot: u32, rights: u64) -> SpawnGrant {
    SpawnGrant { slot, rights }
}

fn write_value(label: &[u8], value: u64) {
    let mut digits = [0u8; 20];
    let mut length = 0;
    let mut remaining = value;
    loop {
        digits[length] = b'0' + (remaining % 10) as u8;
        length += 1;
        remaining /= 10;
        if remaining == 0 {
            break;
        }
    }
    let mut line = [0u8; 96];
    let mut cursor = 0;
    for byte in label.iter().copied().take(line.len() - 24) {
        line[cursor] = byte;
        cursor += 1;
    }
    while length != 0 {
        length -= 1;
        line[cursor] = digits[length];
        cursor += 1;
    }
    line[cursor] = b'\n';
    cursor += 1;
    debug_write(&line[..cursor]);
}

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[private-adaptive:io] FAIL ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
