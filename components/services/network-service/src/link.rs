//! The service's side of one `LinkDevice`: the IO3 client shape, driven by
//! smoltcp instead of a probe script.
//!
//! Two IO0 queues and eight one-page frame buffers are created here, lent to
//! the driver, and never touched by anyone else. Four pages are receive
//! buffers the device owns until it completes them; four are transmit buffers
//! the device owns from submission to completion. Nothing above this module
//! sees a lease, a queue, or a page: smoltcp sees frames.

use slime_components::link_frames::{RxSlots, TxSlots};
use slime_proto::capability_transfer::OBJECT_KIND_SHARED_BUFFER_LOAN;
use slime_proto::io_queue::{
    self, COMPLETION_PAYLOAD_BYTES, DIRECTION_DEVICE_READ, DIRECTION_DEVICE_WRITE, STATUS_OK,
    WireBufferSlice,
};
use slime_proto::io_queue_ring::{Outstanding, Queue, QueueError, format};
use slime_proto::link_device::{
    self, LINK_UP, MAX_FRAME_BYTES, MIN_FRAME_BYTES, OP_PROVIDE_RECEIVE, OP_QUERY_LINK, OP_RESET,
    OP_STATISTICS, OP_TRANSMIT, WireLinkReply, WireLinkRequest,
};
use slime_proto::valid_link_reply;
use slime_rt::{
    CapabilityDisposition, ERR_SUCCESS, MAX_CAPS_PER_MSG, MAX_MSG, capability_delegate,
    notification_signal, shared_buffer_create, shared_buffer_loan, shared_buffer_map, yield_now,
};
use smoltcp::phy::{self, Checksum, ChecksumCapabilities, DeviceCapabilities, Medium};
use smoltcp::time::Instant;

const BASE: u64 = 0x0000_0019_0000_0000;
const PAGE: u64 = 4096;
const SLOTS: usize = 8;
const EPOCH: u64 = 1;
const RIGHT_BUFFER_WRITE: u64 = 1 << 8;
const RIGHT_BUFFER_MAP: u64 = 1 << 9;
pub const RX_FRAMES: usize = 4;
pub const TX_FRAMES: usize = 4;
/// The most yields a control request may wait for its completion. The driver
/// answers a control request without touching the device, so this is a
/// scheduling bound, not a device timeout.
const CONTROL_YIELDS: u32 = 1_000_000;
/// The Ethernet MTU smoltcp is told: a maximal untagged frame. The driver
/// admits `MAX_FRAME_BYTES` for a tagged one, which this stack never emits.
const MTU: usize = 1514;

const ETHERTYPE_IPV4: [u8; 2] = [0x08, 0x00];
const ETHERTYPE_ARP: [u8; 2] = [0x08, 0x06];
const IP_PROTOCOL_ICMP: u8 = 1;
const IP_PROTOCOL_TCP: u8 = 6;

#[derive(Clone, Copy)]
struct Page {
    buffer: u64,
    lease: u64,
    base: u64,
}

impl Page {
    fn bytes(self) -> &'static mut [u8] {
        // The page was mapped at `base` for the life of this task and is lent
        // to the driver only through the queues this module owns; the slot
        // trackers decide who may touch it and when.
        unsafe { core::slice::from_raw_parts_mut(self.base as *mut u8, PAGE as usize) }
    }
}

/// What passed through the link, by what the frame said it was. Counted at
/// the token boundary, so a frame smoltcp received or emitted is counted
/// exactly once, whatever it did with it.
#[derive(Clone, Copy, Default)]
pub struct FrameCounts {
    pub frames: u32,
    pub arp: u32,
    pub icmp: u32,
    pub tcp: u32,
    pub other: u32,
}

impl FrameCounts {
    fn count(&mut self, frame: &[u8]) {
        self.frames += 1;
        match frame.get(12..14) {
            Some(kind) if kind == ETHERTYPE_ARP => self.arp += 1,
            Some(kind) if kind == ETHERTYPE_IPV4 => match frame.get(23) {
                Some(&IP_PROTOCOL_ICMP) => self.icmp += 1,
                Some(&IP_PROTOCOL_TCP) => self.tcp += 1,
                _ => self.other += 1,
            },
            _ => self.other += 1,
        }
    }
}

struct Side<const N: usize> {
    queue: Queue<'static>,
    outstanding: Outstanding<SLOTS>,
    request_ready: u32,
    pages: [Page; N],
    next_request_id: u64,
}

impl<const N: usize> Side<N> {
    fn submit_frame(
        &mut self,
        page: usize,
        op: u8,
        frame_len: usize,
        direction: u32,
    ) -> Result<u64, QueueError> {
        let request_id = self.next_request_id;
        let request = WireLinkRequest {
            magic: link_device::LINK_MAGIC,
            version: link_device::FORMAT_VERSION,
            op,
            flags: 0,
            frame_len: frame_len as u16,
            reserved: [0; 2],
            padding: [0; 44],
        };
        let slice = WireBufferSlice {
            buffer: self.pages[page].buffer,
            lease: self.pages[page].lease,
            offset: 0,
            length: frame_len as u64,
            direction,
            reserved: [0; 4],
        };
        self.queue
            .submit(request_id, &slice, &request.encode(), false, PAGE)?;
        self.outstanding
            .admit(request_id, self.pages[page].lease, frame_len as u64)?;
        self.next_request_id += 2;
        Ok(request_id)
    }
    fn submit_control(&mut self, op: u8) -> Result<u64, QueueError> {
        let request_id = self.next_request_id;
        let request = WireLinkRequest {
            magic: link_device::LINK_MAGIC,
            version: link_device::FORMAT_VERSION,
            op,
            flags: 0,
            frame_len: 0,
            reserved: [0; 2],
            padding: [0; 44],
        };
        let slice = WireBufferSlice {
            buffer: 0,
            lease: 0,
            offset: 0,
            length: 0,
            direction: io_queue::DIRECTION_NONE,
            reserved: [0; 4],
        };
        self.queue
            .submit(request_id, &slice, &request.encode(), false, PAGE)?;
        self.outstanding.admit(request_id, 0, 0)?;
        self.next_request_id += 2;
        Ok(request_id)
    }
    fn signal(&self) {
        if notification_signal(self.request_ready) != ERR_SUCCESS {
            fail(b"link signal");
        }
    }
    /// One completion, settled, with its decoded reply when it carried one.
    fn take(&mut self) -> Option<(u64, u32, Option<WireLinkReply>)> {
        let mut body = [0u8; COMPLETION_PAYLOAD_BYTES];
        loop {
            match self.queue.take_completion(&self.outstanding, &mut body) {
                Ok(completion) => {
                    self.outstanding
                        .settle(completion.request_id, completion.status)
                        .unwrap_or_else(|_| fail(b"link settle"));
                    let reply = WireLinkReply::decode(&body[..completion.payload_len])
                        .filter(valid_link_reply);
                    return Some((completion.request_id, completion.status, reply));
                }
                // A completion for an identity this side never issued: consumed
                // and counted nowhere, exactly as the probe treats it.
                Err(QueueError::Unknown) => continue,
                Err(QueueError::Empty) => return None,
                Err(_) => fail(b"link completion"),
            }
        }
    }
}

pub struct RxSide {
    side: Side<RX_FRAMES>,
    slots: RxSlots<RX_FRAMES>,
    pub counts: FrameCounts,
}

/// The control request in flight on the transmit queue, if any.
#[derive(Clone, Copy)]
struct Control {
    request_id: u64,
    answered: bool,
    reply: Option<WireLinkReply>,
}

pub struct TxSide {
    side: Side<TX_FRAMES>,
    slots: TxSlots<TX_FRAMES>,
    pub counts: FrameCounts,
    control: Option<Control>,
}

pub struct Link {
    pub rx: RxSide,
    pub tx: TxSide,
    peer: u32,
    state_changed: u32,
}

impl Link {
    /// Create the queues and frame pages, lend them to the driver over
    /// `peer_slot`, and wait for its ready message.
    pub fn attach(factory_slot: u32, peer_slot: u32) -> Self {
        let tx_request_ready = binding(b"notification:io-link-tx-request-ready+signal");
        let rx_request_ready = binding(b"notification:io-link-rx-request-ready+signal");
        let state_changed = binding(b"notification:io-link-state-changed+wait");
        let tx_buffer = shared_buffer_create(factory_slot, 1, true)
            .unwrap_or_else(|_| fail(b"tx queue create"));
        let rx_buffer = shared_buffer_create(factory_slot, 1, true)
            .unwrap_or_else(|_| fail(b"rx queue create"));
        if shared_buffer_map(tx_buffer.slot, BASE, 0, PAGE, true) != ERR_SUCCESS
            || shared_buffer_map(rx_buffer.slot, BASE + PAGE, 0, PAGE, true) != ERR_SUCCESS
        {
            fail(b"queue map");
        }
        let tx_bytes = unsafe { core::slice::from_raw_parts_mut(BASE as *mut u8, PAGE as usize) };
        let rx_bytes =
            unsafe { core::slice::from_raw_parts_mut((BASE + PAGE) as *mut u8, PAGE as usize) };
        format(tx_bytes, SLOTS, EPOCH).unwrap_or_else(|_| fail(b"tx format"));
        format(rx_bytes, SLOTS, EPOCH).unwrap_or_else(|_| fail(b"rx format"));
        delegate_queue(tx_buffer.slot, tx_buffer.id, peer_slot);
        delegate_queue(rx_buffer.slot, rx_buffer.id, peer_slot);

        // The driver takes exactly eight payload loans, in this order: the
        // first four are this side's receive pages, the rest its transmit pages.
        let mut pages = [Page {
            buffer: 0,
            lease: 0,
            base: 0,
        }; RX_FRAMES + TX_FRAMES];
        for (index, page) in pages.iter_mut().enumerate() {
            let buffer = shared_buffer_create(factory_slot, 1, true)
                .unwrap_or_else(|_| fail(b"frame create"));
            let base = BASE + (2 + index as u64) * PAGE;
            if shared_buffer_map(buffer.slot, base, 0, PAGE, true) != ERR_SUCCESS {
                fail(b"frame map");
            }
            let loan = shared_buffer_loan(buffer.slot, peer_slot, 0, PAGE, true)
                .unwrap_or_else(|_| fail(b"frame loan"));
            delegate_loan(loan.slot, loan.id, peer_slot);
            *page = Page {
                buffer: buffer.id,
                lease: loan.id,
                base,
            };
        }
        await_ready(peer_slot);
        let mut rx_pages = [pages[0]; RX_FRAMES];
        rx_pages.copy_from_slice(&pages[..RX_FRAMES]);
        let mut tx_pages = [pages[0]; TX_FRAMES];
        tx_pages.copy_from_slice(&pages[RX_FRAMES..]);
        Self {
            rx: RxSide {
                side: Side {
                    queue: Queue::attach(rx_bytes, SLOTS).unwrap_or_else(|_| fail(b"rx attach")),
                    outstanding: Outstanding::new(EPOCH),
                    request_ready: rx_request_ready,
                    pages: rx_pages,
                    // Even ids on the receive queue, odd on transmit: both
                    // queues charge one DMA account, whose request ids must
                    // be unique across them.
                    next_request_id: 2,
                },
                slots: RxSlots::new(),
                counts: FrameCounts::default(),
            },
            tx: TxSide {
                side: Side {
                    queue: Queue::attach(tx_bytes, SLOTS).unwrap_or_else(|_| fail(b"tx attach")),
                    outstanding: Outstanding::new(EPOCH),
                    request_ready: tx_request_ready,
                    pages: tx_pages,
                    next_request_id: 1,
                },
                slots: TxSlots::new(),
                counts: FrameCounts::default(),
                control: None,
            },
            peer: peer_slot,
            state_changed,
        }
    }

    /// Ask the driver for its link state and wait for the answer.
    pub fn link_up(&mut self) -> bool {
        let reply = self.control(OP_QUERY_LINK);
        reply.op == OP_QUERY_LINK && reply.link_state == LINK_UP
    }

    /// The driver's own frame counters, as `(transmitted, received)`.
    pub fn statistics(&mut self) -> (u32, u32) {
        let reply = self.control(OP_STATISTICS);
        (reply.tx_frames, reply.rx_frames)
    }

    fn control(&mut self, op: u8) -> WireLinkReply {
        self.submit_control(op);
        let control = self.await_control();
        control.reply.unwrap_or_else(|| fail(b"control reply"))
    }

    fn submit_control(&mut self, op: u8) {
        let request_id = self
            .tx
            .side
            .submit_control(op)
            .unwrap_or_else(|_| fail(b"control submit"));
        self.tx.control = Some(Control {
            request_id,
            answered: false,
            reply: None,
        });
        self.tx.side.signal();
    }

    fn await_control(&mut self) -> Control {
        let mut yields = 0;
        loop {
            self.drain();
            if let Some(control) = self.tx.control.filter(|control| control.answered) {
                self.tx.control = None;
                return control;
            }
            yields += 1;
            if yields > CONTROL_YIELDS {
                fail(b"control completion");
            }
            yield_now();
        }
    }

    /// Hand the link back the way the IO3 probe does: a reset the driver
    /// settles, an acknowledgement once every settled completion has been
    /// read, and its fresh-epoch signal before this side goes away. Until that
    /// signal the driver still writes the rings this side lent it.
    pub fn release(&mut self) {
        self.submit_control(OP_RESET);
        if slime_rt::notification_wait(self.state_changed).is_err() {
            fail(b"reset notification");
        }
        // The driver settles every frame request with `STATUS_RESET`; the
        // reset request itself gets no completion, because answering it is
        // ending the epoch, so it is settled here once every frame is back.
        let mut yields = 0;
        loop {
            self.drain();
            if self.tx.slots.retained_count() == 0 && self.rx.slots.provisioned_count() == 0 {
                break;
            }
            yields += 1;
            if yields > CONTROL_YIELDS {
                fail(b"reset settlement");
            }
            yield_now();
        }
        if let Some(control) = self.tx.control.take() {
            let _ = self
                .tx
                .side
                .outstanding
                .settle(control.request_id, io_queue::STATUS_RESET);
        }
        loop {
            match slime_rt::send(self.peer, b"reset-ack", &[]) {
                slime_rt::ERR_WOULDBLOCK => yield_now(),
                ERR_SUCCESS => break,
                _ => fail(b"reset ack"),
            }
        }
        if slime_rt::notification_wait(self.state_changed).is_err() {
            fail(b"fresh epoch notification");
        }
    }

    /// Lend every free receive page to the device.
    pub fn replenish(&mut self) -> bool {
        let mut provided = false;
        while self.rx.slots.free_count() > 0 {
            let request_id = self.rx.side.next_request_id;
            let Some(slot) = self.rx.slots.provide(request_id) else {
                break;
            };
            self.rx
                .side
                .submit_frame(
                    slot,
                    OP_PROVIDE_RECEIVE,
                    MAX_FRAME_BYTES,
                    DIRECTION_DEVICE_WRITE,
                )
                .unwrap_or_else(|_| fail(b"receive provision"));
            provided = true;
        }
        if provided {
            self.rx.side.signal();
        }
        provided
    }

    /// Settle every completion the driver published, in both directions.
    pub fn drain(&mut self) -> bool {
        let mut progress = false;
        while let Some((request_id, _status, reply)) = self.tx.side.take() {
            progress = true;
            match self.tx.control {
                Some(control) if control.request_id == request_id && !control.answered => {
                    self.tx.control = Some(Control {
                        request_id,
                        answered: true,
                        reply,
                    });
                }
                _ => {
                    self.tx
                        .slots
                        .release(request_id)
                        .unwrap_or_else(|_| fail(b"transmit release"));
                }
            }
        }
        while let Some((request_id, status, reply)) = self.rx.side.take() {
            progress = true;
            let delivered = reply.filter(|reply| {
                status == STATUS_OK
                    && reply.op == OP_PROVIDE_RECEIVE
                    && (MIN_FRAME_BYTES..=MAX_FRAME_BYTES).contains(&(reply.frame_len as usize))
            });
            match delivered {
                Some(reply) => {
                    self.rx
                        .slots
                        .delivered(request_id, reply.frame_len as usize)
                        .unwrap_or_else(|_| fail(b"receive delivery"));
                }
                None => {
                    self.rx
                        .slots
                        .returned(request_id)
                        .unwrap_or_else(|_| fail(b"receive return"));
                }
            }
        }
        progress
    }

    pub fn rx_provisioned(&self) -> usize {
        self.rx.slots.provisioned_count()
    }
}

pub struct RxToken<'a> {
    rx: &'a mut RxSide,
    slot: usize,
    len: usize,
}

pub struct TxToken<'a> {
    tx: &'a mut TxSide,
}

impl phy::Device for Link {
    type RxToken<'a>
        = RxToken<'a>
    where
        Self: 'a;
    type TxToken<'a>
        = TxToken<'a>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if !self.rx.slots.has_ready() || self.tx.slots.free_slot().is_none() {
            return None;
        }
        let (slot, len) = self.rx.slots.take_ready()?;
        Some((
            RxToken {
                rx: &mut self.rx,
                slot,
                len,
            },
            TxToken { tx: &mut self.tx },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        self.tx
            .slots
            .free_slot()
            .map(|_| TxToken { tx: &mut self.tx })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut capabilities = DeviceCapabilities::default();
        capabilities.medium = Medium::Ethernet;
        capabilities.max_transmission_unit = MTU;
        capabilities.max_burst_size = Some(TX_FRAMES);
        // The driver negotiates no offload, so every checksum is computed and
        // verified here.
        let mut checksum = ChecksumCapabilities::default();
        checksum.ipv4 = Checksum::Both;
        checksum.tcp = Checksum::Both;
        checksum.udp = Checksum::Both;
        checksum.icmpv4 = Checksum::Both;
        capabilities.checksum = checksum;
        capabilities
    }
}

impl phy::RxToken for RxToken<'_> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        let frame = &self.rx.side.pages[self.slot].bytes()[..self.len];
        self.rx.counts.count(frame);
        // The slot is already free; the service's next `replenish` lends the
        // page again after this frame has been read.
        f(frame)
    }
}

impl phy::TxToken for TxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let slot = self
            .tx
            .slots
            .free_slot()
            .unwrap_or_else(|| fail(b"transmit slot"));
        let page = self.tx.side.pages[slot].bytes();
        let len = len.min(MAX_FRAME_BYTES);
        let result = f(&mut page[..len]);
        // smoltcp emits 42-byte ARP frames; the contract's floor is 60, so the
        // tail is zero-filled up to it.
        let frame_len = if len < MIN_FRAME_BYTES {
            page[len..MIN_FRAME_BYTES].fill(0);
            MIN_FRAME_BYTES
        } else {
            len
        };
        self.tx.counts.count(&page[..frame_len]);
        let request_id = self
            .tx
            .side
            .submit_frame(slot, OP_TRANSMIT, frame_len, DIRECTION_DEVICE_READ)
            .unwrap_or_else(|_| fail(b"transmit submit"));
        self.tx
            .slots
            .retain(slot, request_id)
            .unwrap_or_else(|_| fail(b"transmit retain"));
        self.tx.side.signal();
        result
    }
}

fn delegate_queue(slot: u32, id: u64, peer: u32) {
    let loan =
        shared_buffer_loan(slot, peer, 0, PAGE, true).unwrap_or_else(|_| fail(b"queue loan"));
    let mut descriptor = [0u8; 64];
    descriptor[..8].copy_from_slice(&id.to_le_bytes());
    descriptor[8..16].copy_from_slice(&loan.id.to_le_bytes());
    if capability_delegate(
        peer,
        loan.slot,
        CapabilityDisposition::Move,
        OBJECT_KIND_SHARED_BUFFER_LOAN,
        RIGHT_BUFFER_MAP | RIGHT_BUFFER_WRITE,
        &descriptor,
    ) != ERR_SUCCESS
    {
        fail(b"queue delegate");
    }
}

fn delegate_loan(slot: u32, id: u64, peer: u32) {
    let mut descriptor = [0u8; 64];
    descriptor[..8].copy_from_slice(&id.to_le_bytes());
    if capability_delegate(
        peer,
        slot,
        CapabilityDisposition::Move,
        OBJECT_KIND_SHARED_BUFFER_LOAN,
        RIGHT_BUFFER_MAP | RIGHT_BUFFER_WRITE,
        &descriptor,
    ) != ERR_SUCCESS
    {
        fail(b"loan delegate");
    }
}

fn await_ready(peer: u32) {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(peer, &mut bytes, &mut caps) {
            slime_rt::ERR_WOULDBLOCK => yield_now(),
            value if value < 0 => fail(b"driver ready"),
            _ => return,
        }
    }
}

fn binding(name: &[u8]) -> u32 {
    slime_rt::resolve_binding(name).unwrap_or_else(|_| fail(b"link binding"))
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[network-service] link fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}
