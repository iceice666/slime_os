//! Fixed-capacity Ethernet loopback with an explicit egress admission boundary.

use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetFrame, EthernetProtocol, IpProtocol, Ipv4Packet, TcpPacket};

pub const FRAME_SLOTS: usize = 4;
pub const MTU: usize = 1514;

#[derive(Default)]
struct Indices {
    slots: [usize; FRAME_SLOTS],
    head: usize,
    len: usize,
}

impl Indices {
    fn push(&mut self, slot: usize) {
        assert!(self.len < FRAME_SLOTS);
        self.slots[(self.head + self.len) % FRAME_SLOTS] = slot;
        self.len += 1;
    }

    fn pop(&mut self) -> Option<usize> {
        if self.len == 0 {
            return None;
        }
        let slot = self.slots[self.head];
        self.head = (self.head + 1) % FRAME_SLOTS;
        self.len -= 1;
        Some(slot)
    }
}

struct Frames {
    bytes: [[u8; MTU]; FRAME_SLOTS],
    lengths: [Option<usize>; FRAME_SLOTS],
    staged: Indices,
    ready: Indices,
}

impl Frames {
    fn free_slot(&self) -> Option<usize> {
        self.lengths.iter().position(Option::is_none)
    }
}

pub struct Loopback {
    frames: Frames,
    scratch: [u8; MTU],
    egress: u64,
    rejected: u64,
    resets: u64,
    syns: u64,
    fins: u64,
}

impl Default for Loopback {
    fn default() -> Self {
        Self::new()
    }
}

impl Loopback {
    pub fn new() -> Self {
        Self {
            frames: Frames {
                bytes: [[0; MTU]; FRAME_SLOTS],
                lengths: [None; FRAME_SLOTS],
                staged: Indices::default(),
                ready: Indices::default(),
            },
            scratch: [0; MTU],
            egress: 0,
            rejected: 0,
            resets: 0,
            syns: 0,
            fins: 0,
        }
    }

    /// Admit staged frames before exposing them to the receive side. Each frame
    /// occupies exactly one pool slot while staged or ready; admission cannot
    /// overflow the receive queue or observe frames that bypassed the guard.
    pub fn flush(&mut self, mut allow: impl FnMut(&[u8]) -> bool) -> bool {
        let mut progress = false;
        while let Some(slot) = self.frames.staged.pop() {
            let length = self.frames.lengths[slot].expect("staged frame owns its slot");
            if allow(&self.frames.bytes[slot][..length]) {
                self.frames.ready.push(slot);
                self.egress = self.egress.saturating_add(1);
                if let Ok(ethernet) = EthernetFrame::new_checked(&self.frames.bytes[slot][..length])
                    && ethernet.ethertype() == EthernetProtocol::Ipv4
                    && let Ok(ip) = Ipv4Packet::new_checked(ethernet.payload())
                    && ip.next_header() == IpProtocol::Tcp
                    && let Ok(tcp) = TcpPacket::new_checked(ip.payload())
                {
                    self.resets = self.resets.saturating_add(u64::from(tcp.rst()));
                    self.syns = self.syns.saturating_add(u64::from(tcp.syn()));
                    self.fins = self.fins.saturating_add(u64::from(tcp.fin()));
                }
            } else {
                self.frames.lengths[slot] = None;
                self.rejected = self.rejected.saturating_add(1);
            }
            progress = true;
        }
        progress
    }

    pub fn ready_count(&self) -> usize {
        self.frames.ready.len
    }

    pub fn staged_count(&self) -> usize {
        self.frames.staged.len
    }

    pub fn egress_count(&self) -> u64 {
        self.egress
    }

    pub fn rejected_count(&self) -> u64 {
        self.rejected
    }

    pub fn reset_count(&self) -> u64 {
        self.resets
    }

    pub fn syn_count(&self) -> u64 {
        self.syns
    }

    pub fn fin_count(&self) -> u64 {
        self.fins
    }
}

pub struct ReceiveToken<'a>(&'a [u8]);
pub struct TransmitToken<'a>(&'a mut Frames);

impl RxToken for ReceiveToken<'_> {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(self.0)
    }
}

impl TxToken for TransmitToken<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, length: usize, f: F) -> R {
        assert!(length <= MTU);
        let slot = self
            .0
            .free_slot()
            .expect("transmit token reserves free capacity");
        let result = f(&mut self.0.bytes[slot][..length]);
        self.0.lengths[slot] = Some(length);
        self.0.staged.push(slot);
        result
    }
}

impl Device for Loopback {
    type RxToken<'a> = ReceiveToken<'a>;
    type TxToken<'a> = TransmitToken<'a>;

    fn receive(&mut self, _: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let slot = self.frames.ready.pop()?;
        let length = self.frames.lengths[slot]
            .take()
            .expect("ready frame owns its slot");
        // The scratch borrow keeps this frame stable while the paired transmit
        // token reuses the freed pool slot, even when the pool was completely full.
        self.scratch[..length].copy_from_slice(&self.frames.bytes[slot][..length]);
        Some((
            ReceiveToken(&self.scratch[..length]),
            TransmitToken(&mut self.frames),
        ))
    }

    fn transmit(&mut self, _: Instant) -> Option<Self::TxToken<'_>> {
        self.frames.free_slot()?;
        Some(TransmitToken(&mut self.frames))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut capabilities = DeviceCapabilities::default();
        capabilities.medium = Medium::Ethernet;
        capabilities.max_transmission_unit = MTU;
        capabilities.max_burst_size = Some(FRAME_SLOTS);
        capabilities
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use smoltcp::iface::{Config, Interface, PollIngressSingleResult, SocketSet, SocketStorage};
    use smoltcp::socket::tcp::{Socket, SocketBuffer, State};
    use smoltcp::wire::{
        EthernetAddress, EthernetFrame, EthernetProtocol, HardwareAddress, IpAddress, IpCidr,
        IpProtocol, Ipv4Packet, TcpPacket,
    };

    #[test]
    fn full_pool_preserves_fifo_and_paired_response_capacity() {
        let mut device = Loopback::new();
        for value in 0..FRAME_SLOTS {
            device
                .transmit(Instant::ZERO)
                .unwrap()
                .consume(1, |bytes| bytes[0] = value as u8);
        }
        assert_eq!(device.staged_count(), FRAME_SLOTS);
        assert_eq!(device.ready_count(), 0);
        assert!(device.receive(Instant::ZERO).is_none());
        assert!(device.transmit(Instant::ZERO).is_none());
        assert!(device.flush(|_| true));
        let (rx, tx) = device.receive(Instant::ZERO).unwrap();
        tx.consume(1, |bytes| bytes[0] = 4);
        assert_eq!(rx.consume(|bytes| bytes[0]), 0);
        assert_eq!(device.ready_count(), 3);
        assert_eq!(device.staged_count(), 1);
        device.flush(|_| true);
        for expected in 1..=4 {
            let (rx, _) = device.receive(Instant::ZERO).unwrap();
            assert_eq!(rx.consume(|bytes| bytes[0]), expected);
        }
        assert_eq!(device.egress_count(), 5);
        assert_eq!(device.rejected_count(), 0);
        assert!(device.receive(Instant::ZERO).is_none());
        assert!(device.transmit(Instant::ZERO).is_some());
    }

    #[test]
    fn denied_frames_are_never_reflected_and_slots_are_reusable() {
        let mut device = Loopback::new();
        for round in 0..8 {
            for value in 0..FRAME_SLOTS {
                device
                    .transmit(Instant::ZERO)
                    .unwrap()
                    .consume(MTU, |bytes| bytes.fill(value as u8));
            }
            device.flush(|frame| frame[0] % 2 == 0);
            assert_eq!(device.ready_count(), 2);
            for expected in [0, 2] {
                let (rx, _) = device.receive(Instant::ZERO).unwrap();
                rx.consume(|frame| assert!(frame.iter().all(|byte| *byte == expected)));
            }
            assert_eq!(device.egress_count(), (round + 1) * 2);
            assert_eq!(device.rejected_count(), (round + 1) * 2);
            assert!(!device.flush(|_| panic!("no staged frames")));
        }
    }

    fn poll(
        iface: &mut Interface,
        device: &mut Loopback,
        sockets: &mut SocketSet<'_>,
        millis: i64,
        syns: &mut usize,
        fins: &mut usize,
    ) {
        let now = Instant::from_millis(millis);
        iface.poll_maintenance(now);
        for _ in 0..FRAME_SLOTS {
            if iface.poll_ingress_single(now, device, sockets) == PollIngressSingleResult::None {
                break;
            }
        }
        iface.poll_egress(now, device, sockets);
        device.flush(|bytes| {
            let ethernet = EthernetFrame::new_checked(bytes).unwrap();
            if ethernet.ethertype() == EthernetProtocol::Ipv4 {
                let ip = Ipv4Packet::new_checked(ethernet.payload()).unwrap();
                if ip.next_header() == IpProtocol::Tcp {
                    let tcp = TcpPacket::new_checked(ip.payload()).unwrap();
                    *syns += usize::from(tcp.syn());
                    *fins += usize::from(tcp.fin());
                }
            }
            true
        });
    }

    #[test]
    fn genuine_loopback_tcp_bidirectional_stream_and_fin() {
        let mut device = Loopback::new();
        let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress([
            2, 0, 0, 0, 0, 1,
        ])));
        config.random_seed = 1;
        let mut iface = Interface::new(config, &mut device, Instant::ZERO);
        let address = IpAddress::v4(127, 0, 0, 1);
        iface.update_ip_addrs(|addresses| {
            addresses.push(IpCidr::new(address, 8)).unwrap();
        });
        let mut storage = [SocketStorage::EMPTY; 2];
        let mut sockets = SocketSet::new(&mut storage[..]);
        let mut client_rx = [0; 1024];
        let mut client_tx = [0; 1024];
        let mut server_rx = [0; 1024];
        let mut server_tx = [0; 1024];
        let mut server = Socket::new(
            SocketBuffer::new(&mut server_rx[..]),
            SocketBuffer::new(&mut server_tx[..]),
        );
        server.listen((address, 7447)).unwrap();
        server.set_ack_delay(None);
        let server_handle = sockets.add(server);
        let mut client = Socket::new(
            SocketBuffer::new(&mut client_rx[..]),
            SocketBuffer::new(&mut client_tx[..]),
        );
        client
            .connect(iface.context(), (address, 7447), 49152)
            .unwrap();
        client.set_ack_delay(None);
        let client_handle = sockets.add(client);
        let mut forward = [0u8; 3072];
        let mut reverse = [0u8; 2051];
        for (index, byte) in forward.iter_mut().enumerate() {
            *byte = (index.wrapping_mul(37) + 11) as u8;
        }
        for (index, byte) in reverse.iter_mut().enumerate() {
            *byte = (index.wrapping_mul(19) + 7) as u8;
        }
        // Opaque application bytes include a split length prefix; the device and
        // TCP transport must neither interpret it nor preserve send boundaries.
        forward[..4].copy_from_slice(&3068u32.to_be_bytes());
        reverse[..4].copy_from_slice(&2047u32.to_be_bytes());
        let mut forward_sent = 0;
        let mut reverse_sent = 0;
        let mut forward_received = 0;
        let mut reverse_received = 0;
        let mut partial_send = false;
        let mut syns = 0;
        let mut fins = 0;
        let mut millis = 0;
        while millis < 5000
            && (forward_received < forward.len() || reverse_received < reverse.len())
        {
            poll(
                &mut iface,
                &mut device,
                &mut sockets,
                millis,
                &mut syns,
                &mut fins,
            );
            let client = sockets.get_mut::<Socket>(client_handle);
            if client.can_send() && forward_sent < forward.len() {
                let count = client.send_slice(&forward[forward_sent..]).unwrap();
                partial_send |= count < forward.len() - forward_sent;
                forward_sent += count;
            }
            if client.can_recv() {
                let mut bytes = [0; 313];
                let count = client.recv_slice(&mut bytes).unwrap();
                assert_eq!(
                    &bytes[..count],
                    &reverse[reverse_received..reverse_received + count]
                );
                reverse_received += count;
            }
            let server = sockets.get_mut::<Socket>(server_handle);
            if server.can_send() && reverse_sent < reverse.len() {
                let end =
                    (reverse_sent + if reverse_sent < 4 { 1 } else { 491 }).min(reverse.len());
                reverse_sent += server.send_slice(&reverse[reverse_sent..end]).unwrap();
            }
            if server.can_recv() {
                let mut bytes = [0; 17];
                let count = server.recv_slice(&mut bytes).unwrap();
                assert_eq!(
                    &bytes[..count],
                    &forward[forward_received..forward_received + count]
                );
                forward_received += count;
            }
            millis += 1;
        }
        assert_eq!(forward_received, forward.len());
        assert_eq!(reverse_received, reverse.len());
        assert!(partial_send);
        assert_eq!(syns, 2);
        sockets.get_mut::<Socket>(client_handle).close();
        let mut peer_closed = false;
        for _ in 0..100 {
            poll(
                &mut iface,
                &mut device,
                &mut sockets,
                millis,
                &mut syns,
                &mut fins,
            );
            let server = sockets.get_mut::<Socket>(server_handle);
            if !server.may_recv() && server.state() == State::CloseWait {
                peer_closed = true;
                server.close();
            }
            millis += 1;
        }
        assert!(peer_closed);
        assert_eq!(
            sockets.get::<Socket>(client_handle).state(),
            State::TimeWait
        );
        assert_eq!(sockets.get::<Socket>(server_handle).state(), State::Closed);
        assert_eq!(fins, 2);
        assert_eq!(device.syn_count(), 2);
        assert_eq!(device.fin_count(), 2);
        assert_eq!(device.reset_count(), 0);
        assert_eq!(device.rejected_count(), 0);
        assert!(device.egress_count() > 10);
    }
}
