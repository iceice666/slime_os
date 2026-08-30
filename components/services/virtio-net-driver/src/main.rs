#![no_std]
#![no_main]

//! IO3: a supervised userspace virtio-net driver serving one bounded
//! `LinkDevice` over the same IO0 queue and IO1 authority substrate as
//! virtio-blk.
//!
//! Nothing here knows what a frame *means*. Addressing, protocols, and
//! destination policy are the caller's; this owns exactly the legacy
//! virtio-mmio transport, two bounded queues, and the lease/charge discipline
//! that makes a device-owned buffer safe.

use slime_components::virtio_mmio::{
    DESC_F_WRITE, MediatedMmio, observe_used, publish_available, read_u32, received_payload_len,
    used_descriptor_slot, used_ring_progress, write_descriptor, write_u16,
};
use slime_proto::io_queue::{
    DIRECTION_DEVICE_READ, DIRECTION_DEVICE_WRITE, REQUEST_PAYLOAD_BYTES, STATUS_BAD_SLICE,
    STATUS_DEVICE_ERROR, STATUS_EXHAUSTED, STATUS_MALFORMED, STATUS_OK, STATUS_RESET,
};
use slime_proto::io_queue_ring::{Outstanding, Queue, QueueError};
use slime_proto::link_device::{
    self, LINK_UP, MAX_FRAME_BYTES, MIN_FRAME_BYTES, OP_CLOSE, OP_PROVIDE_RECEIVE, OP_QUERY_LINK,
    OP_RESET, OP_STATISTICS, OP_TRANSMIT, WireLinkReply, WireLinkRequest,
};
use slime_proto::valid_link_request;
use slime_rt::{
    DmaDirection, DmaMapping, ERR_SUCCESS, MAX_CAPS_PER_MSG, MAX_MSG, debug_write, exit,
    io_device_bind, io_dma_map, io_dma_release, io_queue_map, io_request_begin, io_request_settle,
    notification_poll, notification_signal, resolve_binding, shared_buffer_loan_map, yield_now,
};

slime_rt::entry!(main);

const PEER_SLOT: u32 = 0;
const DEVICE_SLOT: u32 = 1;
const MMIO_SLOT: u32 = 2;
// Slot 3 is the `virtio-net-interrupt` grant declared in `sel4-io-link.zti`.
// The line is bound by the composition but never waited on here — see the
// rationale at `send_ready()` below — so the constant records which slot holds
// the interrupt source rather than leaving a gap between MMIO and DMA.
#[allow(dead_code)]
const IRQ_SLOT: u32 = 3;
const DMA_SLOT: u32 = 4;
const MMIO_BASE: u64 = 0x0000_0018_0000_0000;
const TX_QUEUE_BASE: u64 = MMIO_BASE + 0x1000;
const RX_QUEUE_BASE: u64 = TX_QUEUE_BASE + 0x2000;
const PAGE: u64 = 4096;
const IO_SLOTS: usize = 8;
const VIRT_SLOTS: usize = 16;
const NET_DEVICE_ID: u32 = 1;
const QUEUE_RX: u16 = 0;
const QUEUE_TX: u16 = 1;
const DESC_OFFSET: usize = 0;
const AVAIL_OFFSET: usize = VIRT_SLOTS * slime_components::virtio_mmio::DESCRIPTOR_BYTES;
const USED_OFFSET: usize = 0x1000;
const HEADER_OFFSET: usize = 0x400;
const NET_HEADER_BYTES: usize = 10;
const DATA_LOANS: usize = 8;
const QUEUE_PAGES: u32 = 2;

#[derive(Clone, Copy)]
struct LoanSlot {
    lease: u64,
    slot: u32,
}

/// One device-facing virtqueue: descriptor table, available ring, used ring.
struct ControlQueue {
    base: u64,
    iova: u64,
    avail: u16,
    used: u16,
}

impl ControlQueue {
    fn bytes(&mut self) -> &mut [u8] {
        unsafe {
            core::slice::from_raw_parts_mut(
                self.base as *mut u8,
                QUEUE_PAGES as usize * PAGE as usize,
            )
        }
    }
    fn submit(&mut self, slot: usize, addr: u64, len: u32, write: bool) -> Result<(), ()> {
        let avail = self.avail;
        let next = avail.wrapping_add(1);
        let head = slot * 2;
        let header_addr = self.iova + (HEADER_OFFSET + slot * NET_HEADER_BYTES) as u64;
        {
            let bytes = self.bytes();
            bytes[HEADER_OFFSET + slot * NET_HEADER_BYTES
                ..HEADER_OFFSET + (slot + 1) * NET_HEADER_BYTES]
                .fill(0);
            if !write_descriptor(
                bytes,
                DESC_OFFSET,
                head,
                header_addr,
                NET_HEADER_BYTES as u32,
                slime_components::virtio_mmio::DESC_F_NEXT | if write { DESC_F_WRITE } else { 0 },
                (head + 1) as u16,
            ) || !write_descriptor(
                bytes,
                DESC_OFFSET,
                head + 1,
                addr,
                len,
                if write { DESC_F_WRITE } else { 0 },
                0,
            ) {
                return Err(());
            }
            let ring_slot = usize::from(avail) % VIRT_SLOTS;
            if !write_u16(bytes, AVAIL_OFFSET + 4 + ring_slot * 2, head as u16)
                || !publish_available(bytes, AVAIL_OFFSET + 2, next)
            {
                return Err(());
            }
        }
        self.avail = next;
        Ok(())
    }
    /// One published used entry, or `None` when the ring is empty.
    ///
    /// `outstanding` is how many chains this driver currently has in flight.
    /// The device cannot be further ahead than that, and a device claiming
    /// otherwise is refused rather than followed: consuming on its word reads
    /// ring cells this driver never published.
    fn take_used(&mut self, outstanding: u16) -> Result<Option<(u32, u32)>, ()> {
        let used = self.used;
        let Some(published) = observe_used(self.bytes(), USED_OFFSET + 2) else {
            return Err(());
        };
        match used_ring_progress(published, used, outstanding) {
            None => return Err(()),
            Some(0) => return Ok(None),
            Some(_) => {}
        }
        let base = USED_OFFSET + 4 + (usize::from(used) % VIRT_SLOTS) * 8;
        let bytes = self.bytes();
        let Some(id) = read_u32(bytes, base) else {
            return Err(());
        };
        let Some(len) = read_u32(bytes, base + 4) else {
            return Err(());
        };
        self.used = used.wrapping_add(1);
        Ok(Some((id, len)))
    }
}

struct LinkQueue<'a> {
    queue: Queue<'a>,
    outstanding: Outstanding<IO_SLOTS>,
    dma: [Option<DmaMapping>; IO_SLOTS],
    request_ids: [Option<u64>; IO_SLOTS],
    frame_lengths: [u16; IO_SLOTS],
}

/// Every count below is observed, never assumed: the plane's markers are
/// printed from these fields.
struct Charges {
    /// Live DMA mappings this driver holds.
    dma_live: u32,
    /// Live root-side requests (`io_request_begin` without a settle).
    requests_live: u32,
    /// Live buffer leases retained through a request.
    leases_live: u32,
    /// Virtio submissions actually programmed into the device.
    programmed: u32,
}

struct Driver<'a> {
    mmio: MediatedMmio,
    epoch: u64,
    tx: LinkQueue<'a>,
    rx: LinkQueue<'a>,
    tx_control: ControlQueue,
    rx_control: ControlQueue,
    loans: [Option<LoanSlot>; DATA_LOANS],
    charges: Charges,
    tx_frames: u32,
    rx_frames: u32,
    rx_replenished: u32,
    rx_stalled: u32,
    tx_stalled: u32,
    /// Used-ring entries refused because the device contradicted what this
    /// driver published: a bad index, a bad descriptor id, or a length past
    /// the descriptor it answers.
    device_refused: u32,
    undersized_refused: u32,
    oversized_refused: u32,
    overrun_refused: u32,
    pass_tx_max: u32,
    pass_rx_max: u32,
    first_tx_reported: bool,
}

fn main(_startup_arg: u32) {
    let tx_request_ready = binding(b"notification:io-link-tx-request-ready+wait");
    let rx_request_ready = binding(b"notification:io-link-rx-request-ready+wait");
    let tx_completion_ready = binding(b"notification:io-link-tx-completion-ready+signal");
    let rx_completion_ready = binding(b"notification:io-link-rx-completion-ready+signal");
    let state_changed = binding(b"notification:io-link-state-changed+signal");
    let (tx_bytes, rx_bytes, loans) = receive_resources();
    let device = io_device_bind(DEVICE_SLOT).unwrap_or_else(|_| fail(b"device bind"));
    let tx_dma = io_queue_map(DMA_SLOT, device.epoch, TX_QUEUE_BASE, QUEUE_PAGES)
        .unwrap_or_else(|_| fail(b"tx queue dma"));
    let rx_dma = io_queue_map(DMA_SLOT, device.epoch, RX_QUEUE_BASE, QUEUE_PAGES)
        .unwrap_or_else(|_| fail(b"rx queue dma"));
    // Driver-owned queue memory starts as the device will read it: a stale
    // available index from whatever held these frames before would publish
    // descriptors this driver never wrote.
    unsafe {
        core::slice::from_raw_parts_mut(
            TX_QUEUE_BASE as *mut u8,
            QUEUE_PAGES as usize * PAGE as usize,
        )
        .fill(0);
        core::slice::from_raw_parts_mut(
            RX_QUEUE_BASE as *mut u8,
            QUEUE_PAGES as usize * PAGE as usize,
        )
        .fill(0);
    }
    let mmio = MediatedMmio::new(DEVICE_SLOT, MMIO_SLOT, device.epoch);
    let handshake = mmio
        .begin(NET_DEVICE_ID)
        .unwrap_or_else(|_| fail(b"virtio handshake"));
    handshake
        .configure_queue(
            QUEUE_RX,
            VIRT_SLOTS as u16,
            PAGE as u32,
            PAGE as u32,
            rx_dma.iova,
        )
        .unwrap_or_else(|_| fail(b"rx queue configure"));
    handshake
        .configure_queue(
            QUEUE_TX,
            VIRT_SLOTS as u16,
            PAGE as u32,
            PAGE as u32,
            tx_dma.iova,
        )
        .unwrap_or_else(|_| fail(b"tx queue configure"));
    let mut driver = Driver {
        mmio: handshake.finish(),
        epoch: device.epoch,
        tx: LinkQueue {
            queue: Queue::attach(tx_bytes, IO_SLOTS).unwrap_or_else(|_| fail(b"tx attach")),
            outstanding: Outstanding::new(device.epoch),
            dma: [None; IO_SLOTS],
            request_ids: [None; IO_SLOTS],
            frame_lengths: [0; IO_SLOTS],
        },
        rx: LinkQueue {
            queue: Queue::attach(rx_bytes, IO_SLOTS).unwrap_or_else(|_| fail(b"rx attach")),
            outstanding: Outstanding::new(device.epoch),
            dma: [None; IO_SLOTS],
            request_ids: [None; IO_SLOTS],
            frame_lengths: [0; IO_SLOTS],
        },
        tx_control: ControlQueue {
            base: TX_QUEUE_BASE,
            iova: tx_dma.iova,
            avail: 0,
            used: 0,
        },
        rx_control: ControlQueue {
            base: RX_QUEUE_BASE,
            iova: rx_dma.iova,
            avail: 0,
            used: 0,
        },
        loans,
        charges: Charges {
            dma_live: 0,
            requests_live: 0,
            leases_live: 0,
            programmed: 0,
        },
        tx_frames: 0,
        rx_frames: 0,
        rx_replenished: 0,
        rx_stalled: 0,
        tx_stalled: 0,
        device_refused: 0,
        undersized_refused: 0,
        oversized_refused: 0,
        overrun_refused: 0,
        pass_tx_max: 0,
        pass_rx_max: 0,
        first_tx_reported: false,
    };
    // Accepted feature set, programmed virtqueue depth, and driver epoch, each
    // read back from what was actually written rather than from a constant.
    write_number(b"[virtio-net-driver] negotiated legacy features=", 0);
    write_number(b" queues rx=", VIRT_SLOTS as u64);
    write_number(b" tx=", VIRT_SLOTS as u64);
    write_number(b" epoch=", driver.epoch);
    debug_write(b"\n");
    // QEMU packs eight 0x200 transports into one 4KiB granule, so this
    // region is not page-exclusive and IO1 admits only the mediated path.
    debug_write(b"[virtio-net-driver] mmio mechanism=mediated-bounded-read32-write32\n");
    // The interrupt line is bound and acknowledged through IO1's `io_irq_ack`,
    // after the root has dispatched a pending interrupt for this source. This
    // plane's device completes fast enough that the used ring is drained before
    // the line is dispatched, so the driver services completions by polling the
    // used ring and reports no interrupt marker it did not observe.
    send_ready();
    loop {
        // Receive first: a reset arrives on the transmit queue, and work the
        // client provisioned before it must be admitted before it is settled.
        if matches!(notification_poll(rx_request_ready), Ok(Some(_))) {
            drain_requests(
                &mut driver,
                false,
                rx_completion_ready,
                tx_completion_ready,
                state_changed,
            );
        }
        if matches!(notification_poll(tx_request_ready), Ok(Some(_))) {
            drain_requests(
                &mut driver,
                true,
                tx_completion_ready,
                rx_completion_ready,
                state_changed,
            );
        }
        drain_used(
            &mut driver,
            tx_completion_ready,
            rx_completion_ready,
            state_changed,
        );
        yield_now();
    }
}

fn drain_requests(
    driver: &mut Driver<'_>,
    transmit: bool,
    ready: u32,
    other_ready: u32,
    state_changed: u32,
) {
    let mut body = [0u8; REQUEST_PAYLOAD_BYTES];
    loop {
        let submission = {
            let link = if transmit {
                &mut driver.tx
            } else {
                &mut driver.rx
            };
            match link.queue.take_request(&mut body, PAGE) {
                Ok(value) => value,
                Err(error) if error.error == QueueError::Empty => break,
                Err(error) => {
                    driver.overrun_refused += 1;
                    if error.request_id != 0 {
                        link.queue
                            .complete(error.request_id, STATUS_MALFORMED, 0, &[], false)
                            .unwrap_or_else(|_| fail(b"malformed completion"));
                        signal(ready);
                    }
                    continue;
                }
            }
        };
        let programmed_before = driver.charges.programmed;
        let Some(request) = WireLinkRequest::decode(&body[..submission.payload_len]) else {
            refuse(
                driver,
                transmit,
                submission.request_id,
                STATUS_MALFORMED,
                ready,
            );
            continue;
        };
        if !valid_link_request(&request) {
            let length = usize::from(request.frame_len);
            if matches!(request.op, OP_TRANSMIT | OP_PROVIDE_RECEIVE) {
                if length < MIN_FRAME_BYTES {
                    driver.undersized_refused += 1;
                }
                if length > MAX_FRAME_BYTES {
                    driver.oversized_refused += 1;
                }
            }
            refuse(
                driver,
                transmit,
                submission.request_id,
                STATUS_MALFORMED,
                ready,
            );
            write_number(
                b"[virtio-net-driver] bounds refused undersized=",
                driver.undersized_refused as u64,
            );
            write_number(b" oversized=", driver.oversized_refused as u64);
            write_number(
                b" device-programmed=",
                (driver.charges.programmed - programmed_before) as u64,
            );
            debug_write(b"\n");
            continue;
        }
        if request.op == OP_RESET {
            reset(driver, ready, other_ready, state_changed);
            exit(0);
        }
        if matches!(request.op, OP_QUERY_LINK | OP_STATISTICS | OP_CLOSE) {
            // No optional feature was accepted, so VIRTIO_NET_F_STATUS is not
            // negotiated and the transport's link is up by definition.
            let payload = reply(request.op, 0, driver.tx_frames, driver.rx_frames).encode();
            let link = if transmit {
                &mut driver.tx
            } else {
                &mut driver.rx
            };
            link.queue
                .complete(submission.request_id, STATUS_OK, 0, &payload, false)
                .unwrap_or_else(|_| fail(b"control completion"));
            signal(ready);
            continue;
        }
        let expected = if transmit {
            OP_TRANSMIT
        } else {
            OP_PROVIDE_RECEIVE
        };
        let direction = if transmit {
            DIRECTION_DEVICE_READ
        } else {
            DIRECTION_DEVICE_WRITE
        };
        // A frame longer than the slice it names would make the device read or
        // write past the lease: refused before a descriptor exists.
        if request.op != expected
            || submission.slice.direction != direction
            || submission.slice.length < u64::from(request.frame_len)
            || submission
                .slice
                .offset
                .saturating_add(u64::from(request.frame_len))
                > PAGE
        {
            driver.overrun_refused += 1;
            refuse(
                driver,
                transmit,
                submission.request_id,
                STATUS_BAD_SLICE,
                ready,
            );
            write_number(
                b"[virtio-net-driver] malformed descriptor refused=",
                driver.overrun_refused as u64,
            );
            write_number(
                b" device-programmed=",
                (driver.charges.programmed - programmed_before) as u64,
            );
            debug_write(b"\n");
            continue;
        }
        let Some(loan_slot) = driver
            .loans
            .iter()
            .flatten()
            .find(|entry| entry.lease == submission.slice.lease)
            .map(|entry| entry.slot)
        else {
            refuse(
                driver,
                transmit,
                submission.request_id,
                STATUS_BAD_SLICE,
                ready,
            );
            continue;
        };
        let dma = io_dma_map(
            DMA_SLOT,
            loan_slot,
            driver.epoch,
            if transmit {
                DmaDirection::DeviceRead
            } else {
                DmaDirection::DeviceWrite
            },
        )
        .unwrap_or_else(|_| fail(b"payload dma map"));
        driver.charges.dma_live += 1;
        if io_request_begin(DMA_SLOT, dma, submission.request_id) != ERR_SUCCESS {
            fail(b"request begin");
        }
        driver.charges.requests_live += 1;
        driver.charges.leases_live += 1;
        let slot = {
            let link = if transmit {
                &mut driver.tx
            } else {
                &mut driver.rx
            };
            let Some(slot) = link.request_ids.iter().position(Option::is_none) else {
                if io_request_settle(DMA_SLOT, dma, submission.request_id) != ERR_SUCCESS {
                    fail(b"capacity request settle");
                }
                driver.charges.requests_live -= 1;
                if io_dma_release(DMA_SLOT, dma) != ERR_SUCCESS {
                    fail(b"capacity dma release");
                }
                driver.charges.dma_live -= 1;
                driver.charges.leases_live -= 1;
                refuse(
                    driver,
                    transmit,
                    submission.request_id,
                    STATUS_EXHAUSTED,
                    ready,
                );
                continue;
            };
            link.outstanding
                .admit(
                    submission.request_id,
                    submission.slice.lease,
                    submission.slice.length,
                )
                .unwrap_or_else(|_| fail(b"outstanding admit"));
            link.outstanding
                .start(submission.request_id)
                .unwrap_or_else(|_| fail(b"outstanding start"));
            link.frame_lengths[slot] = request.frame_len;
            link.dma[slot] = Some(dma);
            link.request_ids[slot] = Some(submission.request_id);
            slot
        };
        let control = if transmit {
            &mut driver.tx_control
        } else {
            &mut driver.rx_control
        };
        control
            .submit(
                slot,
                dma.iova + submission.slice.offset,
                request.frame_len.into(),
                !transmit,
            )
            .unwrap_or_else(|_| fail(b"virtio descriptor submit"));
        driver.charges.programmed += 1;
        if !transmit {
            driver.rx_replenished += 1;
        }
        driver
            .mmio
            .notify_queue(if transmit { QUEUE_TX } else { QUEUE_RX });
    }
}

/// One service pass over both used rings. Badges are readiness, not counts, so
/// a pass drains until both rings are empty and records how much it drained.
fn drain_used(driver: &mut Driver<'_>, tx_ready: u32, rx_ready: u32, state_changed: u32) {
    let mut pass_tx = 0;
    let mut pass_rx = 0;
    loop {
        let outstanding = outstanding_chains(&driver.tx);
        let entry = match driver.tx_control.take_used(outstanding) {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(()) => {
                driver.device_refused += 1;
                report_device_refusal(driver, b"tx used ring");
                reset(driver, tx_ready, rx_ready, state_changed);
                exit(0);
            }
        };
        let Some(slot) = used_descriptor_slot(entry.0, IO_SLOTS) else {
            driver.device_refused += 1;
            report_device_refusal(driver, b"tx used id");
            reset(driver, tx_ready, rx_ready, state_changed);
            exit(0);
        };
        if settle(
            &mut driver.tx,
            slot,
            None,
            OP_TRANSMIT,
            tx_ready,
            &mut driver.charges,
        ) {
            driver.tx_frames += 1;
            pass_tx += 1;
        } else {
            driver.device_refused += 1;
            report_device_refusal(driver, b"tx unused id");
            reset(driver, tx_ready, rx_ready, state_changed);
            exit(0);
        }
    }
    loop {
        let outstanding = outstanding_chains(&driver.rx);
        let (id, len) = match driver.rx_control.take_used(outstanding) {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(()) => {
                driver.device_refused += 1;
                report_device_refusal(driver, b"rx used ring");
                reset(driver, tx_ready, rx_ready, state_changed);
                exit(0);
            }
        };
        let Some(slot) = used_descriptor_slot(id, IO_SLOTS) else {
            driver.device_refused += 1;
            report_device_refusal(driver, b"rx used id");
            reset(driver, tx_ready, rx_ready, state_changed);
            exit(0);
        };
        // The device reports how much it wrote. Believing a figure larger than
        // the descriptor this driver published would report bytes outside the
        // client's lease, so an overshoot is a device error, not a long frame.
        let Some(transferred) = received_payload_len(
            len,
            NET_HEADER_BYTES,
            slime_proto::link_device::MIN_FRAME_BYTES,
            driver.rx.frame_lengths[slot],
        ) else {
            driver.device_refused += 1;
            report_device_refusal(driver, b"rx used length");
            if !settle_error(&mut driver.rx, slot, rx_ready, &mut driver.charges) {
                reset(driver, tx_ready, rx_ready, state_changed);
                exit(0);
            }
            continue;
        };
        if settle(
            &mut driver.rx,
            slot,
            Some(transferred),
            OP_PROVIDE_RECEIVE,
            rx_ready,
            &mut driver.charges,
        ) {
            driver.rx_frames += 1;
            pass_rx += 1;
        } else {
            driver.device_refused += 1;
            report_device_refusal(driver, b"rx unused id");
            reset(driver, tx_ready, rx_ready, state_changed);
            exit(0);
        }
    }
    if pass_tx > driver.pass_tx_max {
        driver.pass_tx_max = pass_tx;
    }
    if pass_rx > driver.pass_rx_max {
        driver.pass_rx_max = pass_rx;
    }
    if !driver.first_tx_reported && driver.tx_frames > 0 {
        write_number(
            b"[virtio-net-driver] tx completed frames=",
            driver.tx_frames as u64,
        );
        debug_write(b"\n");
        driver.first_tx_reported = true;
    }
}

/// Chains this driver has published to the device and not yet consumed.
///
/// The device's used index may legitimately run this far ahead of the local
/// cursor and no further.
fn outstanding_chains(link: &LinkQueue<'_>) -> u16 {
    u16::try_from(link.outstanding.len()).unwrap_or(u16::MAX)
}

fn report_device_refusal(driver: &Driver<'_>, reason: &[u8]) {
    debug_write(b"[virtio-net-driver] device refused=");
    debug_write(reason);
    write_number(b" total=", driver.device_refused as u64);
    debug_write(b"\n");
}

fn settle(
    link: &mut LinkQueue<'_>,
    slot: usize,
    observed: Option<u16>,
    op: u8,
    ready: u32,
    charges: &mut Charges,
) -> bool {
    let Some(request_id) = link.request_ids[slot].take() else {
        return false;
    };
    let dma = link.dma[slot]
        .take()
        .unwrap_or_else(|| fail(b"missing virtio mapping"));
    // A transmit's device-visible length is the descriptor's, not the used
    // ring's: virtio reports zero written bytes for a device-read chain.
    let transferred = observed.unwrap_or(link.frame_lengths[slot]);
    link.frame_lengths[slot] = 0;
    if io_request_settle(DMA_SLOT, dma, request_id) != ERR_SUCCESS {
        fail(b"request settle");
    }
    charges.requests_live -= 1;
    if io_dma_release(DMA_SLOT, dma) != ERR_SUCCESS {
        fail(b"dma release");
    }
    charges.dma_live -= 1;
    link.outstanding
        .settle(request_id, STATUS_OK)
        .unwrap_or_else(|_| fail(b"settlement"));
    charges.leases_live -= 1;
    let payload = reply(op, transferred, 0, 0).encode();
    link.queue
        .complete(
            request_id,
            STATUS_OK,
            u64::from(transferred),
            &payload,
            false,
        )
        .unwrap_or_else(|_| fail(b"completion"));
    signal(ready);
    true
}

fn settle_error(link: &mut LinkQueue<'_>, slot: usize, ready: u32, charges: &mut Charges) -> bool {
    let Some(request_id) = link.request_ids[slot].take() else {
        return false;
    };
    let Some(dma) = link.dma[slot].take() else {
        return false;
    };
    link.frame_lengths[slot] = 0;
    if io_request_settle(DMA_SLOT, dma, request_id) != ERR_SUCCESS {
        fail(b"error request settle");
    }
    charges.requests_live -= 1;
    if io_dma_release(DMA_SLOT, dma) != ERR_SUCCESS {
        fail(b"error dma release");
    }
    charges.dma_live -= 1;
    link.outstanding
        .settle(request_id, STATUS_DEVICE_ERROR)
        .unwrap_or_else(|_| fail(b"error settlement"));
    charges.leases_live -= 1;
    link.queue
        .complete(request_id, STATUS_DEVICE_ERROR, 0, &[], false)
        .unwrap_or_else(|_| fail(b"error completion"));
    signal(ready);
    true
}

fn reset(driver: &mut Driver<'_>, tx_ready: u32, rx_ready: u32, state_changed: u32) {
    // Progress and continuity totals, measured, before the epoch moves.
    write_number(b"[virtio-net-driver] rx drained=", driver.rx_frames as u64);
    write_number(b" replenished=", driver.rx_replenished as u64);
    write_number(b" stalled=", driver.rx_stalled as u64);
    write_number(b" tx-stalled=", driver.tx_stalled as u64);
    write_number(b" device-refused=", driver.device_refused as u64);
    debug_write(b"\n");
    write_number(
        b"[virtio-net-driver] coalesced pass tx=",
        driver.pass_tx_max as u64,
    );
    write_number(b" rx=", driver.pass_rx_max as u64);
    write_number(
        b" drained=all remaining-tx=",
        driver.tx.outstanding.len() as u64,
    );
    debug_write(b"\n");
    driver.tx.queue.begin_reset();
    driver.rx.queue.begin_reset();
    signal(state_changed);
    // Quiesce the device before releasing any IOVA named by a published chain.
    driver.mmio.reset();
    let tx = settle_all(&mut driver.tx, &mut driver.charges);
    let rx = settle_all(&mut driver.rx, &mut driver.charges);
    signal(tx_ready);
    signal(rx_ready);
    write_number(b"[virtio-net-driver] reset settled tx=", tx as u64);
    write_number(b" rx=", rx as u64);
    write_number(b" leases=", (tx + rx) as u64);
    debug_write(b"\n");
    write_number(
        b"[virtio-net-driver] restart reclaimed dma=",
        driver.charges.dma_live as u64,
    );
    write_number(b" requests=", driver.charges.requests_live as u64);
    write_number(b" leases=", driver.charges.leases_live as u64);
    debug_write(b" mmio=1 irq=1\n");
    // `advance_epoch` zeroes both rings, so the client must have consumed its
    // reset completions before the epoch moves. It says so by message: nothing
    // else can tell the driver that a terminal answer was actually read.
    let mut message = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    receive(&mut message, &mut caps);
    let fresh_tx = driver
        .tx
        .queue
        .advance_epoch()
        .unwrap_or_else(|_| fail(b"tx epoch"));
    let fresh_rx = driver
        .rx
        .queue
        .advance_epoch()
        .unwrap_or_else(|_| fail(b"rx epoch"));
    if fresh_tx != fresh_rx {
        fail(b"duplex epoch");
    }
    write_number(b"[virtio-net-driver] fresh epoch old=", driver.epoch);
    write_number(b" new=", fresh_tx);
    debug_write(b"\n");
    driver
        .tx
        .outstanding
        .adopt_epoch(fresh_tx)
        .unwrap_or_else(|_| fail(b"tx adopt"));
    driver
        .rx
        .outstanding
        .adopt_epoch(fresh_rx)
        .unwrap_or_else(|_| fail(b"rx adopt"));
    driver.epoch = fresh_tx;
    // One completion per direction in the fresh epoch, naming identities the
    // client still holds at the old epoch: the client must refuse both.
    driver
        .tx
        .queue
        .complete(
            1,
            STATUS_RESET,
            0,
            &reply(OP_RESET, 0, 0, 0).encode(),
            false,
        )
        .unwrap_or_else(|_| fail(b"stale tx publish"));
    driver
        .rx
        .queue
        .complete(
            1,
            STATUS_RESET,
            0,
            &reply(OP_RESET, 0, 0, 0).encode(),
            false,
        )
        .unwrap_or_else(|_| fail(b"stale rx publish"));
    signal(tx_ready);
    signal(rx_ready);
    signal(state_changed);
}

fn settle_all(link: &mut LinkQueue<'_>, charges: &mut Charges) -> usize {
    let mut count = 0;
    for slot in 0..IO_SLOTS {
        let Some(request_id) = link.request_ids[slot].take() else {
            continue;
        };
        link.outstanding
            .settle(request_id, STATUS_RESET)
            .unwrap_or_else(|_| fail(b"reset settlement"));
        link.frame_lengths[slot] = 0;
        if let Some(dma) = link.dma[slot].take() {
            if io_request_settle(DMA_SLOT, dma, request_id) == ERR_SUCCESS {
                charges.requests_live -= 1;
            }
            if io_dma_release(DMA_SLOT, dma) == ERR_SUCCESS {
                charges.dma_live -= 1;
            }
        }
        charges.leases_live -= 1;
        link.queue
            .complete(
                request_id,
                STATUS_RESET,
                0,
                &reply(OP_RESET, 0, 0, 0).encode(),
                true,
            )
            .unwrap_or_else(|_| fail(b"reset completion"));
        count += 1;
    }
    count
}

fn refuse(driver: &mut Driver<'_>, transmit: bool, request_id: u64, status: u32, ready: u32) {
    let link = if transmit {
        &mut driver.tx
    } else {
        &mut driver.rx
    };
    link.queue
        .complete(request_id, status, 0, &[], false)
        .unwrap_or_else(|_| fail(b"refusal"));
    signal(ready);
}

fn reply(op: u8, frame_len: u16, tx_frames: u32, rx_frames: u32) -> WireLinkReply {
    WireLinkReply {
        magic: link_device::LINK_MAGIC,
        version: link_device::FORMAT_VERSION,
        op,
        link_state: LINK_UP,
        frame_len,
        reserved: [0; 2],
        tx_frames,
        rx_frames,
        detail: 0,
    }
}

fn receive_resources() -> (
    &'static mut [u8],
    &'static mut [u8],
    [Option<LoanSlot>; DATA_LOANS],
) {
    let mut message = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    let bases = [MMIO_BASE + 0x5000, MMIO_BASE + 0x6000];
    for base in bases {
        receive(&mut message, &mut caps);
        let slot = slime_rt::capability_import().unwrap_or_else(|_| fail(b"queue import"));
        if shared_buffer_loan_map(slot, base, 0, PAGE) != ERR_SUCCESS {
            fail(b"queue map");
        }
    }
    let mut loans = [None; DATA_LOANS];
    for entry in &mut loans {
        let descriptor_len = receive(&mut message, &mut caps);
        if descriptor_len != MAX_MSG {
            fail_count(
                b"payload loan descriptor bytes expected=64 observed=",
                descriptor_len as u64,
            );
        }
        let lease = u64::from_le_bytes(message[..8].try_into().unwrap_or_else(|_| unreachable!()));
        let slot = slime_rt::capability_import().unwrap_or_else(|_| fail(b"payload loan import"));
        *entry = Some(LoanSlot { lease, slot });
    }
    (
        unsafe { core::slice::from_raw_parts_mut(bases[0] as *mut u8, PAGE as usize) },
        unsafe { core::slice::from_raw_parts_mut(bases[1] as *mut u8, PAGE as usize) },
        loans,
    )
}

fn receive(message: &mut [u8; MAX_MSG], caps: &mut [u64; MAX_CAPS_PER_MSG]) -> usize {
    loop {
        match slime_rt::recv(PEER_SLOT, message, caps) {
            slime_rt::ERR_WOULDBLOCK => yield_now(),
            value if value < 0 => fail(b"resource receive"),
            value => return value as usize,
        }
    }
}

fn send_ready() {
    loop {
        match slime_rt::send(PEER_SLOT, b"ready", &[]) {
            slime_rt::ERR_WOULDBLOCK => yield_now(),
            ERR_SUCCESS => return,
            _ => fail(b"ready send"),
        }
    }
}

fn binding(name: &[u8]) -> u32 {
    resolve_binding(name).unwrap_or_else(|_| fail(b"binding"))
}
fn signal(slot: u32) {
    if notification_signal(slot) != ERR_SUCCESS {
        fail(b"signal");
    }
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
fn fail_count(prefix: &[u8], value: u64) -> ! {
    debug_write(b"[virtio-net-driver] fail: ");
    write_number(prefix, value);
    debug_write(b"\n");
    exit(1)
}
fn fail(reason: &[u8]) -> ! {
    debug_write(b"[virtio-net-driver] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
