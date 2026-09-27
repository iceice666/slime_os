//! One service-owned DNS UDP exchange; never an application UDP capability.

use crate::{dns, tcp::Status};
use boot_contracts::network_destination::{
    self, Address, NetworkDestinations, RIGHT_RECV, RIGHT_SEND, Transport,
};
use boot_contracts::sha256::Sha256;
use smoltcp::iface::{SocketHandle, SocketSet};
use smoltcp::socket::udp;
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address, Ipv4Packet, UdpPacket};

pub const BUFFER_BYTES: usize = dns::MAX_PACKET_BYTES;
const RETRY_TIME: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
struct Query {
    name: [u8; 24],
    len: usize,
    id: u16,
    port: u16,
    retry_at: Instant,
    retries: u32,
    emitted: bool,
}

pub struct Resolver {
    handle: SocketHandle,
    endpoint: IpEndpoint,
    seed: [u8; 32],
    counter: u64,
    retry_limit: u32,
    record_limit: usize,
    query: Option<Query>,
}

impl Resolver {
    pub fn new(
        handle: SocketHandle,
        destinations: &NetworkDestinations<'_>,
        seed: [u8; 32],
    ) -> Result<Self, Status> {
        if seed == [0; 32] {
            return Err(Status::Denied);
        }
        let holder = network_destination::holder_identity("network-service");
        let mut selected = None;
        for index in 0..destinations.destination_count() {
            let row = destinations.destination(index).ok_or(Status::Denied)?;
            if row.holder_identity != holder || row.transport != Transport::Udp {
                continue;
            }
            let Address::Ipv4(address) = row.address else {
                return Err(Status::Denied);
            };
            if selected.is_some()
                || !matches!(row.port, 53 | 1053)
                || row.rights & (RIGHT_SEND | RIGHT_RECV) != RIGHT_SEND | RIGHT_RECV
                || row.dns_record_limit == 0
                || row.queue_depth == 0
                || row.timer_budget == 0
                || row.byte_budget < (2 * BUFFER_BYTES) as u32
                || row.socket_limit == 0
            {
                return Err(Status::Denied);
            }
            selected = Some((
                IpEndpoint::new(IpAddress::Ipv4(Ipv4Address::from(address)), row.port),
                row.retry_limit.min(2),
                row.dns_record_limit as usize,
            ));
        }
        let (endpoint, retry_limit, record_limit) = selected.ok_or(Status::Denied)?;
        Ok(Self {
            handle,
            endpoint,
            seed,
            counter: 0,
            retry_limit,
            record_limit,
            query: None,
        })
    }

    pub fn controlled(&self) -> bool {
        self.endpoint.port == 1053
    }

    pub fn start(
        &mut self,
        sockets: &mut SocketSet<'_>,
        name: &[u8],
        now: Instant,
    ) -> Result<(), Status> {
        if self.query.is_some() || name.is_empty() || name.len() > 24 {
            return Err(Status::Exhausted);
        }
        let mut query = Query {
            name: [0; 24],
            len: name.len(),
            id: 0,
            port: 0,
            retry_at: now,
            retries: 0,
            emitted: false,
        };
        query.name[..name.len()].copy_from_slice(name);
        self.send(sockets, query, now)?;
        Ok(())
    }

    fn send(
        &mut self,
        sockets: &mut SocketSet<'_>,
        mut query: Query,
        now: Instant,
    ) -> Result<(), Status> {
        self.counter = self.counter.checked_add(1).ok_or(Status::Exhausted)?;
        let mut hash = Sha256::new();
        hash.update(b"slime-dns-query-v1");
        hash.update(&self.seed);
        hash.update(&self.counter.to_le_bytes());
        let random = hash.finalize();
        query.id = u16::from_le_bytes([random[0], random[1]]);
        query.port = 49152 + (u16::from_le_bytes([random[2], random[3]]) & 0x3fff);
        query.retry_at = now + RETRY_TIME;
        query.emitted = false;
        let mut bytes = [0; BUFFER_BYTES];
        let len = dns::encode_query(&query.name[..query.len], query.id, &mut bytes)
            .map_err(|_| Status::Malformed)?;
        let socket = sockets.get_mut::<udp::Socket>(self.handle);
        socket.close();
        socket.bind(query.port).map_err(|_| Status::Exhausted)?;
        socket
            .send_slice(&bytes[..len], self.endpoint)
            .map_err(|_| Status::Exhausted)?;
        self.query = Some(query);
        Ok(())
    }

    pub fn poll(
        &mut self,
        sockets: &mut SocketSet<'_>,
        now: Instant,
    ) -> Option<Result<dns::Answer, Status>> {
        let mut query = self.query?;
        let socket = sockets.get_mut::<udp::Socket>(self.handle);
        if let Ok((bytes, metadata)) = socket.recv()
            && metadata.endpoint == self.endpoint
        {
            match dns::parse_response(bytes, query.id, &query.name[..query.len]) {
                Ok(answer) => {
                    if answer.count > self.record_limit {
                        return Some(Err(Status::Exhausted));
                    }
                    return Some(Ok(answer));
                }
                Err(dns::Error::Mismatch) => (),
                Err(dns::Error::NxDomain | dns::Error::NoAddress) => {
                    return Some(Err(Status::Refused));
                }
                Err(_) => return Some(Err(Status::Malformed)),
            }
        }
        if now >= query.retry_at {
            if query.retries >= self.retry_limit {
                return Some(Err(Status::Timeout));
            }
            query.retries += 1;
            if let Err(error) = self.send(sockets, query, now) {
                return Some(Err(error));
            }
        }
        None
    }

    pub fn cancel(&mut self, sockets: &mut SocketSet<'_>) {
        self.query = None;
        sockets.get_mut::<udp::Socket>(self.handle).close();
    }

    pub fn permit_egress(&mut self, ip: &Ipv4Packet<&[u8]>) -> bool {
        let Some(query) = self.query.as_mut() else {
            return false;
        };
        let Ok(udp) = UdpPacket::new_checked(ip.payload()) else {
            return false;
        };
        let mut bytes = [0; BUFFER_BYTES];
        let Ok(len) = dns::encode_query(&query.name[..query.len], query.id, &mut bytes) else {
            return false;
        };
        if query.emitted
            || ip.more_frags()
            || ip.frag_offset() != 0
            || IpAddress::Ipv4(ip.dst_addr()) != self.endpoint.addr
            || udp.dst_port() != self.endpoint.port
            || udp.src_port() != query.port
            || udp.payload() != &bytes[..len]
        {
            return false;
        }
        query.emitted = true;
        true
    }
}
