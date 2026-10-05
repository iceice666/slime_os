#![no_std]
#![no_main]

//! The demo's publisher node: a Zenoh Profile 0 connector that publishes the four declared
//! `Counter` samples once the subscriber has declared the demo key. The exchange itself is
//! `slime_components::zenoh_node`, which the host tests run; this crate holds only what needs the
//! component runtime: its bindings, the service connection and the console.

use slime_components::network_io::{
    Connection as ServiceConnection, NetworkIo, NetworkNotifications, NetworkReply,
};
use slime_components::tick_clock::TickClock;
use slime_components::zenoh_link::{Link, LinkError};
use slime_components::zenoh_node::{
    self as node, Connection, Host, NodeError, Publisher, Reply, Step, Stream,
};
use slime_components::zenoh_profile0::session::Role;
use slime_proto::network_service as net;
use slime_rt::{
    debug_write, exit, monotonic_frequency, monotonic_read, resolve_binding, yield_now,
};

slime_rt::entry!(main);

const RING_BASE: u64 = 0x0000_001c_0000_0000;
const DATA_BASE: u64 = RING_BASE + 4096;
const DEADLINE_MS: i64 = 20_000;

/// One connection of the network service, driven without blocking.
struct Wire<'a, 'b> {
    io: &'a mut NetworkIo<'b>,
    connection: &'a ServiceConnection,
}

fn reply_of(reply: &NetworkReply) -> Reply {
    Reply {
        success: reply.is_success(),
        status: reply.status_detail,
        transferred: reply.transferred,
        end_of_stream: reply.flags == net::FLAG_END_OF_STREAM,
    }
}

impl Connection for Wire<'_, '_> {
    fn send(&mut self, bytes: &[u8]) -> Result<Reply, LinkError> {
        self.io
            .send_nonblocking(self.connection, bytes)
            .map(|reply| reply_of(&reply))
            .map_err(|_| LinkError::Reset)
    }

    fn recv(&mut self, bytes: &mut [u8]) -> Result<Reply, LinkError> {
        self.io
            .recv_nonblocking(self.connection, bytes)
            .map(|reply| reply_of(&reply))
            .map_err(|_| LinkError::Reset)
    }
}

struct Component<'a> {
    clock: &'a TickClock,
}

impl Host for Component<'_> {
    fn now_ms(&mut self) -> u64 {
        let ticks = monotonic_read().unwrap_or_else(|_| fail(b"clock read"));
        self.clock.millis(ticks) as u64
    }

    fn idle(&mut self) -> bool {
        let ticks = monotonic_read().unwrap_or_else(|_| fail(b"clock read"));
        if self.clock.millis(ticks) >= DEADLINE_MS {
            return false;
        }
        yield_now();
        true
    }

    fn write(&mut self, line: &[u8]) {
        debug_write(line);
    }
}

fn main(_: u32) {
    let rate = monotonic_frequency().unwrap_or_else(|_| fail(b"clock rate"));
    let base = monotonic_read().unwrap_or_else(|_| fail(b"clock read"));
    let clock = TickClock::new(rate, base).unwrap_or_else(|_| fail(b"clock rate too slow"));
    let notifications = NetworkNotifications {
        request_signal: binding(b"notification:zenoh-network-request+signal"),
        completion_wait: binding(b"notification:zenoh-publisher-done+wait"),
        timeout_ticks: rate.checked_mul(5).unwrap_or_else(|| fail(b"timer range")),
    };
    // SAFETY: these distinct page-aligned regions are reserved solely for this
    // adapter, and the two declared endpoints belong to the same service.
    let mut io = unsafe {
        NetworkIo::attach_with_notifications(
            binding(b"zenoh-publisher-control"),
            binding(b"zenoh-publisher-provision"),
            RING_BASE,
            DATA_BASE,
            notifications,
        )
    }
    .unwrap_or_else(|_| fail(b"attach"));
    let mut host = Component { clock: &clock };

    denials(&mut io, &mut host);
    let connection = connect(&mut io, &mut host);
    let published = {
        let wire = Wire {
            io: &mut io,
            connection: &connection,
        };
        let mut link = Link::new(
            Stream::new(wire),
            Role::Connector,
            &node::publisher_config(),
            1,
        )
        .unwrap_or_else(|_| fail(b"session config"));
        let mut publisher = Publisher::new();
        loop {
            match publisher.step(&mut link, &mut host) {
                Ok(Step::Done(count)) => break count,
                Ok(Step::Continue) => {
                    if !host.idle() {
                        fail(b"deadline");
                    }
                }
                Err(error) => fail_node(error),
            }
        }
    };
    if published != node::SAMPLES.len() {
        fail(b"samples published");
    }
    if !io
        .close(connection)
        .unwrap_or_else(|_| fail(b"close request"))
        .is_success()
    {
        fail(b"close");
    }
    io.finish().unwrap_or_else(|_| fail(b"finish"));
    host.write(b"[ros2-demo-publisher] session closed samples=4\n");
    exit(0)
}

/// Requests the generation did not grant. Each is a real request to the service; a denial is
/// reported only for a reply that was checked to be a denial, and any other reply fails the node.
fn denials(io: &mut NetworkIo<'_>, host: &mut Component<'_>) {
    expect_denied(
        io.connect_ipv4([127, 0, 0, 2], node::PORT),
        b"undeclared-endpoint",
        host,
    );
    expect_denied(
        io.connect_ipv4(node::ENDPOINT, node::PORT + 1),
        b"undeclared-port",
        host,
    );
    expect_denied(io.listen_ipv4(node::ENDPOINT, node::PORT), b"listen", host);
    let scouting = io.transact_raw(
        node::scouting_request(),
        slime_proto::io_queue::DIRECTION_NONE,
        0,
    );
    expect_denied(scouting, b"scouting", host);
}

fn expect_denied(
    reply: Result<NetworkReply, slime_components::network_io::NetworkError>,
    class: &[u8],
    host: &mut Component<'_>,
) {
    let reply = reply.unwrap_or_else(|_| fail(b"authority request transport"));
    if reply.capability_kind != net::CAPABILITY_NONE
        || reply.capability != 0
        || !node::is_denial(&reply_of(&reply))
    {
        fail(b"authority refusal result");
    }
    // `undeclared-port` is a second refusal of the same class as `undeclared-endpoint`; the plane
    // arm names one marker per class, so it is checked here and reported once.
    if class != b"undeclared-port" {
        host.write(node::denial_line(node::PUBLISHER, class).finish());
    }
}

fn connect(io: &mut NetworkIo<'_>, host: &mut Component<'_>) -> ServiceConnection {
    let mut attempts = 0;
    loop {
        attempts += 1;
        let mut reply = io
            .connect_ipv4(node::ENDPOINT, node::PORT)
            .unwrap_or_else(|_| fail(b"connect request"));
        if let Some(connection) = reply.take_connection() {
            return connection;
        }
        if !matches!(
            reply.status_detail,
            net::STATUS_REFUSED | net::STATUS_WOULD_BLOCK
        ) || attempts >= 64
        {
            fail(b"connect refused");
        }
        if !host.idle() {
            fail(b"deadline");
        }
    }
}

fn binding(name: &[u8]) -> u32 {
    resolve_binding(name).unwrap_or_else(|_| fail(b"binding"))
}

fn fail_node(error: NodeError) -> ! {
    match error {
        NodeError::Link(_) => fail(b"link"),
        NodeError::Deadline => fail(b"deadline"),
        NodeError::PeerClosedEarly => fail(b"peer closed early"),
        NodeError::LineClipped => fail(b"line clipped"),
        NodeError::Sample(reason) | NodeError::Operation(reason) => fail(reason),
    }
}

fn fail(reason: &[u8]) -> ! {
    let mut line = node::Line::<128>::new();
    line.put(node::PUBLISHER)
        .put(b"fail: ")
        .put(reason)
        .put(b"\n");
    debug_write(line.finish());
    exit(1)
}
