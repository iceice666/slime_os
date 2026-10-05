#![no_std]
#![no_main]

//! The demo's subscriber node: a Zenoh Profile 0 listener that declares the demo key, validates
//! each received `Counter` sample, and tears the session down by undeclaring before it closes.
//! The exchange itself is `slime_components::zenoh_node`, which the host tests run; this crate
//! holds only what needs the component runtime: its bindings, the service connection and the
//! console.

use slime_components::network_io::{
    Connection as ServiceConnection, NetworkError, NetworkIo, NetworkNotifications, NetworkReply,
};
use slime_components::tick_clock::TickClock;
use slime_components::zenoh_link::{Link, LinkError};
use slime_components::zenoh_node::{
    self as node, Connection, Host, NodeError, Reply, Step, Stream, Subscriber,
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
        completion_wait: binding(b"notification:zenoh-subscriber-done+wait"),
        timeout_ticks: rate.checked_mul(5).unwrap_or_else(|| fail(b"timer range")),
    };
    // SAFETY: these distinct page-aligned regions are reserved solely for this
    // adapter, and the two declared endpoints belong to the same service.
    let mut io = unsafe {
        NetworkIo::attach_with_notifications(
            binding(b"zenoh-subscriber-control"),
            binding(b"zenoh-subscriber-provision"),
            RING_BASE,
            DATA_BASE,
            notifications,
        )
    }
    .unwrap_or_else(|_| fail(b"attach"));
    let mut host = Component { clock: &clock };

    denials(&mut io, &mut host);
    let listener = io
        .listen_ipv4(node::ENDPOINT, node::PORT)
        .unwrap_or_else(|_| fail(b"listen request"))
        .take_listener()
        .unwrap_or_else(|| fail(b"listen"));
    let connection = accept(&mut io, &listener, &mut host);
    let received = {
        let wire = Wire {
            io: &mut io,
            connection: &connection,
        };
        let mut link = Link::new(
            Stream::new(wire),
            Role::Listener,
            &node::subscriber_config(),
            1,
        )
        .unwrap_or_else(|_| fail(b"session config"));
        let mut subscriber = Subscriber::new();
        loop {
            match subscriber.step(&mut link, &mut host) {
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
    if received != node::SAMPLES.len() {
        fail(b"samples received");
    }
    if !io
        .close(connection)
        .unwrap_or_else(|_| fail(b"close request"))
        .is_success()
        || !io
            .close_listener(listener)
            .unwrap_or_else(|_| fail(b"listener close request"))
            .is_success()
    {
        fail(b"close");
    }
    io.finish().unwrap_or_else(|_| fail(b"finish"));
    host.write(node::SUCCESS);
    exit(0)
}

/// Requests the generation did not grant, each checked to be a denial before it is reported.
fn denials(io: &mut NetworkIo<'_>, host: &mut Component<'_>) {
    expect_denied(io.listen_ipv4(node::ENDPOINT, node::PORT + 1));
    expect_denied(io.connect_ipv4(node::ENDPOINT, node::PORT));
    host.write(node::denial_line(node::SUBSCRIBER, b"connect").finish());
}

fn expect_denied(reply: Result<NetworkReply, NetworkError>) {
    let reply = reply.unwrap_or_else(|_| fail(b"authority request transport"));
    if reply.capability_kind != net::CAPABILITY_NONE
        || reply.capability != 0
        || !node::is_denial(&reply_of(&reply))
    {
        fail(b"authority refusal result");
    }
}

fn accept(
    io: &mut NetworkIo<'_>,
    listener: &slime_components::network_io::Listener,
    host: &mut Component<'_>,
) -> ServiceConnection {
    loop {
        let mut reply = io
            .accept(listener)
            .unwrap_or_else(|_| fail(b"accept request"));
        if let Some(connection) = reply.take_connection() {
            return connection;
        }
        if reply.status_detail != net::STATUS_WOULD_BLOCK {
            fail(b"accept status");
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
    line.put(node::SUBSCRIBER)
        .put(b"fail: ")
        .put(reason)
        .put(b"\n");
    debug_write(line.finish());
    exit(1)
}
