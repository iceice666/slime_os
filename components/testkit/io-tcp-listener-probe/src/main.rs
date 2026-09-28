#![no_std]
#![no_main]

//! One exact external TCP listener driven by a scripted frame peer.
//!
//! As `listener-session` it holds the listener and a control connection to
//! the peer: it reports each step as a line there and waits on one-byte cues
//! where it must wait for wire events it cannot see. As `intruder-session` it
//! is an external client without listener authority and only proves that it
//! cannot listen. The peer judges the wire; this probe reports what the
//! application observed.

use slime_components::network_io::{
    Connection, Listener, NetworkError, NetworkIo, NetworkNotifications, NetworkReply,
};
use slime_proto::network_service as net;
use slime_rt::{debug_write, exit, monotonic_frequency, resolve_binding};

slime_rt::entry!(main);

const LOCAL: [u8; 4] = [10, 0, 0, 1];
const PEER: [u8; 4] = [10, 0, 0, 2];
const LISTEN_PORT: u16 = 4270;
const CONTROL_PORT: u16 = 4280;
const RING_BASE: u64 = 0x0000_001d_0000_0000;
const DATA_BASE: u64 = RING_BASE + 4096;
// Longer than the four seconds the service retains a blocking request.
const REQUEST_TIMEOUT_SECONDS: u64 = 6;
/// How many four-second service waits one scripted step may take.
const WAIT_ROUNDS: u32 = 16;

// The scripted streams: byte `i` is `(31 * i + seed) mod 256`.
const EXCHANGE: (u8, usize) = (3, 2048);
const LOCAL_HALF_GUEST: (u8, usize) = (41, 1024);
const LOCAL_HALF_PEER: (u8, usize) = (59, 2048);
const PEER_HALF_PEER: (u8, usize) = (73, 2048);
const PEER_HALF_GUEST: (u8, usize) = (97, 1024);
// The peer sends these and the probe leaves them unread; the service reports
// the count it observed at close.
const UNREAD_BYTES: u64 = 512;
const UNSENT: (u8, usize) = (131, 2048);

struct Probe<'a> {
    io: NetworkIo<'a>,
}

fn main(_: u32) {
    let (role, control, provision, request, done) =
        if let Ok(control) = resolve_binding(b"listener-session-control") {
            (
                b"listener".as_slice(),
                control,
                binding(b"listener-session-provision"),
                b"notification:listener-network-request+signal".as_slice(),
                b"notification:listener-session-done+wait".as_slice(),
            )
        } else {
            (
                b"intruder".as_slice(),
                binding(b"intruder-session-control"),
                binding(b"intruder-session-provision"),
                b"notification:listener-network-request+signal".as_slice(),
                b"notification:intruder-session-done+wait".as_slice(),
            )
        };
    let rate = monotonic_frequency().unwrap_or_else(|_| fail(b"clock rate"));
    // SAFETY: these page-aligned regions are reserved solely for this adapter,
    // disjoint from the component image, stack, and each other.
    let io = unsafe {
        NetworkIo::attach_with_notifications(
            control,
            provision,
            RING_BASE,
            DATA_BASE,
            NetworkNotifications {
                request_signal: binding(request),
                completion_wait: binding(done),
                timeout_ticks: rate
                    .checked_mul(REQUEST_TIMEOUT_SECONDS)
                    .unwrap_or_else(|| fail(b"timer range")),
            },
        )
    }
    .unwrap_or_else(|_| fail(b"application attach"));
    marker(role, b"attached=1");
    let mut probe = Probe { io };
    if role == b"intruder" {
        refused(probe.io.listen_ipv4(LOCAL, LISTEN_PORT), net::STATUS_DENIED);
        probe.io.finish().unwrap_or_else(|_| fail(b"shutdown"));
        marker(role, b"listen_refused=1 shutdown=1");
        exit(0)
    }
    probe.run();
    probe.io.finish().unwrap_or_else(|_| fail(b"shutdown"));
    marker(b"listener", b"listener_closed=1 scenarios=5 shutdown=1");
    exit(0)
}

impl Probe<'_> {
    fn run(&mut self) {
        // Only the exact declared endpoint may be listened on.
        refused(
            self.io.listen_ipv4(LOCAL, LISTEN_PORT + 1),
            net::STATUS_DENIED,
        );
        refused(
            self.io.listen_ipv4([10, 0, 0, 9], LISTEN_PORT),
            net::STATUS_DENIED,
        );
        marker(b"listener", b"listen_refusals=2");
        let listener = self
            .io
            .listen_ipv4(LOCAL, LISTEN_PORT)
            .unwrap_or_else(|_| fail(b"listen request"))
            .take_listener()
            .unwrap_or_else(|| fail(b"listen"));
        debug_write(b"[io-tcp-listener-probe] role=listener listening=1 address=10.0.0.1");
        write_number(b" port=", u64::from(LISTEN_PORT));
        debug_write(b"\n");
        let control = self.connect_control();
        self.line(&control, b"ready\n");

        // Accept, then echo exactly what the peer sent.
        let exchange = self.accept(&listener);
        self.line(&control, b"accepted 1\n");
        let mut echoed = [0u8; 2048];
        self.receive_exact(&exchange, &mut echoed);
        verify(&echoed, EXCHANGE);
        self.send_all(&exchange, &echoed);
        debug_write(b"[io-tcp-listener-probe] role=listener accepted=1");
        write_number(b" sent=", echoed.len() as u64);
        write_number(b" received=", echoed.len() as u64);
        debug_write(b" identical=1\n");

        // The peer's third connection found the backlog slot held; only then
        // is the held connection accepted, and then the accepted limit holds.
        self.cue(&control, b'B');
        let held = self.accept(&listener);
        self.line(&control, b"accepted 2\n");
        marker(b"listener", b"accepted=2 held=1");

        // Local half-close: FIN after our bytes, then receive to the peer's FIN.
        self.cue(&control, b'H');
        let bytes = stream(LOCAL_HALF_GUEST);
        self.send_all(&exchange, &bytes[..LOCAL_HALF_GUEST.1]);
        let reply = self
            .io
            .shutdown(&exchange)
            .unwrap_or_else(|_| fail(b"shutdown request"));
        if !reply.is_success() {
            status(b"shutdown", &reply);
        }
        let received = self.receive_to_eof(&exchange, LOCAL_HALF_PEER);
        let stale = self.close(exchange);
        self.line(&control, b"done local-half-close\n");
        scenario(b"local-half-close");
        write_number(b" sent=", LOCAL_HALF_GUEST.1 as u64);
        write_number(b" received=", received);
        debug_write(b" eof=1");
        closed(stale);

        // Peer half-close: the peer's bytes and FIN first, then ours.
        let received = self.receive_to_eof(&held, PEER_HALF_PEER);
        let bytes = stream(PEER_HALF_GUEST);
        self.send_all(&held, &bytes[..PEER_HALF_GUEST.1]);
        let stale = self.close(held);
        self.line(&control, b"done peer-half-close\n");
        scenario(b"peer-half-close");
        write_number(b" received=", received);
        debug_write(b" eof=1");
        write_number(b" sent=", PEER_HALF_GUEST.1 as u64);
        closed(stale);

        // Simultaneous close: our FIN crosses the peer's.
        let crossing = self.accept(&listener);
        self.line(&control, b"accepted 3\n");
        let stale = self.close(crossing);
        self.line(&control, b"done simultaneous-close\n");
        scenario(b"simultaneous-close");
        closed(stale);

        // Close with the peer's bytes still unread.
        let unread = self.accept(&listener);
        self.line(&control, b"accepted 4\n");
        self.cue(&control, b'U');
        let stale = self.close(unread);
        self.line(&control, b"done unread-close\n");
        scenario(b"unread-close");
        write_number(b" unread=", UNREAD_BYTES);
        closed(stale);

        // Close with bytes queued behind the peer's zero window.
        let unsent = self.accept(&listener);
        self.line(&control, b"accepted 5\n");
        let bytes = stream(UNSENT);
        let queued = self.queue(&unsent, &bytes[..UNSENT.1]);
        self.line(&control, b"queued unsent-close\n");
        let stale = self.close(unsent);
        self.line(&control, b"done unsent-close\n");
        scenario(b"unsent-close");
        write_number(b" queued=", queued as u64);
        closed(stale);

        let reply = self
            .io
            .close_listener(listener)
            .unwrap_or_else(|_| fail(b"listener close request"));
        if !reply.is_success() {
            status(b"listener close", &reply);
        }
        self.line(&control, b"finished\n");
        let reply = self
            .io
            .close(control)
            .unwrap_or_else(|_| fail(b"control close request"));
        if !reply.is_success() {
            status(b"control close", &reply);
        }
    }

    fn connect_control(&mut self) -> Connection {
        let mut reply = self
            .io
            .connect_ipv4(PEER, CONTROL_PORT)
            .unwrap_or_else(|_| fail(b"control connect request"));
        reply
            .take_connection()
            .unwrap_or_else(|| status(b"control connect", &reply))
    }

    fn accept(&mut self, listener: &Listener) -> Connection {
        for _ in 0..WAIT_ROUNDS {
            let mut reply = self
                .io
                .accept(listener)
                .unwrap_or_else(|_| fail(b"accept request"));
            if let Some(connection) = reply.take_connection() {
                return connection;
            }
            waiting(b"accept", &reply);
        }
        fail(b"accept rounds")
    }

    fn line(&mut self, control: &Connection, text: &[u8]) {
        self.send_all(control, text);
    }

    fn cue(&mut self, control: &Connection, expected: u8) {
        let mut byte = [0u8; 1];
        self.receive_exact(control, &mut byte);
        if byte[0] != expected {
            fail(b"cue");
        }
    }

    fn send_all(&mut self, connection: &Connection, bytes: &[u8]) {
        let mut sent = 0;
        let mut rounds = 0;
        while sent < bytes.len() {
            let reply = self
                .io
                .send(connection, &bytes[sent..])
                .unwrap_or_else(|_| fail(b"send request"));
            if reply.is_success() && reply.transferred != 0 {
                sent += reply.transferred as usize;
                continue;
            }
            waiting(b"send", &reply);
            rounds += 1;
            if rounds > WAIT_ROUNDS {
                fail(b"send rounds");
            }
        }
    }

    /// Queue bytes without waiting for the peer's window; the service's
    /// transmit buffer holds them.
    fn queue(&mut self, connection: &Connection, bytes: &[u8]) -> usize {
        let mut queued = 0;
        while queued < bytes.len() {
            let reply = self
                .io
                .send_nonblocking(connection, &bytes[queued..])
                .unwrap_or_else(|_| fail(b"queue request"));
            if !reply.is_success() || reply.transferred == 0 {
                status(b"queue", &reply);
            }
            queued += reply.transferred as usize;
        }
        queued
    }

    fn receive_exact(&mut self, connection: &Connection, bytes: &mut [u8]) {
        let mut read = 0;
        let mut rounds = 0;
        while read < bytes.len() {
            let reply = self
                .io
                .recv(connection, &mut bytes[read..])
                .unwrap_or_else(|_| fail(b"recv request"));
            if reply.is_success() && reply.transferred != 0 {
                read += reply.transferred as usize;
                continue;
            }
            if reply.is_success() {
                fail(b"premature eof");
            }
            waiting(b"recv", &reply);
            rounds += 1;
            if rounds > WAIT_ROUNDS {
                fail(b"recv rounds");
            }
        }
    }

    /// Receive the declared stream, then the peer's FIN as end of stream.
    fn receive_to_eof(&mut self, connection: &Connection, declared: (u8, usize)) -> u64 {
        let mut bytes = [0u8; 2048];
        self.receive_exact(connection, &mut bytes[..declared.1]);
        verify(&bytes[..declared.1], declared);
        let mut rounds = 0;
        loop {
            let mut trailing = [0u8; 1];
            let reply = self
                .io
                .recv(connection, &mut trailing)
                .unwrap_or_else(|_| fail(b"eof request"));
            if reply.is_success() {
                if reply.transferred != 0 || reply.flags != net::FLAG_END_OF_STREAM {
                    fail(b"bytes after the declared stream");
                }
                return declared.1 as u64;
            }
            waiting(b"eof", &reply);
            rounds += 1;
            if rounds > WAIT_ROUNDS {
                fail(b"eof rounds");
            }
        }
    }

    /// Close a connection, then prove its handle is refused. Returns whether
    /// the stale handle was refused.
    fn close(&mut self, connection: Connection) -> bool {
        let id = connection.id();
        let reply = self
            .io
            .close(connection)
            .unwrap_or_else(|_| fail(b"close request"));
        if !reply.is_success() {
            status(b"close", &reply);
        }
        let mut stale = net::WireNetworkRequest {
            magic: net::NETWORK_MAGIC,
            version: net::FORMAT_VERSION,
            op: net::OP_RECV,
            transport: net::TRANSPORT_NONE,
            flags: 0,
            port: 0,
            name_len: 0,
            capability: id,
            address_kind: net::ADDRESS_NONE,
            reserved: [0; 7],
            endpoint: [0; 24],
        };
        stale.flags = net::FLAG_NONBLOCKING;
        let reply = self
            .io
            .transact_raw(stale, slime_proto::io_queue::DIRECTION_DEVICE_WRITE, 1)
            .unwrap_or_else(|_| fail(b"stale request"));
        reply.status_detail == net::STATUS_DENIED
    }
}

fn stream(declared: (u8, usize)) -> [u8; 2048] {
    let mut bytes = [0u8; 2048];
    for (index, byte) in bytes.iter_mut().enumerate().take(declared.1) {
        *byte = (index * 31 + declared.0 as usize) as u8;
    }
    bytes
}

fn verify(bytes: &[u8], declared: (u8, usize)) {
    if bytes != &stream(declared)[..declared.1] {
        fail(b"stream identity");
    }
}

fn scenario(name: &[u8]) {
    debug_write(b"[io-tcp-listener-probe] role=listener scenario=");
    debug_write(name);
}

fn closed(stale: bool) {
    if !stale {
        fail(b"stale handle accepted");
    }
    debug_write(b" close=success stale_refused=1\n");
}

/// A blocking request the service released unfinished: waiting, not failure.
fn waiting(operation: &[u8], reply: &NetworkReply) {
    if reply.transferred != 0
        || !matches!(
            reply.status_detail,
            net::STATUS_WOULD_BLOCK | net::STATUS_TIMEOUT
        )
    {
        status(operation, reply);
    }
}

fn refused(reply: Result<NetworkReply, NetworkError>, expected: i32) {
    let reply = reply.unwrap_or_else(|_| fail(b"authority request transport"));
    if reply.queue_status != slime_proto::io_queue::STATUS_OK
        || reply.status_detail != expected
        || reply.capability_kind != net::CAPABILITY_NONE
        || reply.capability != 0
    {
        status(b"authority refusal", &reply);
    }
}

fn marker(role: &[u8], message: &[u8]) {
    debug_write(b"[io-tcp-listener-probe] role=");
    debug_write(role);
    debug_write(b" ");
    debug_write(message);
    debug_write(b"\n");
}

fn binding(name: &[u8]) -> u32 {
    resolve_binding(name).unwrap_or_else(|_| fail(b"binding"))
}

fn status(operation: &[u8], reply: &NetworkReply) -> ! {
    debug_write(b"[io-tcp-listener-probe] ");
    debug_write(operation);
    write_number(b" status=", u64::from(reply.status_detail.unsigned_abs()));
    write_number(b" queue=", u64::from(reply.queue_status));
    write_number(b" transferred=", reply.transferred);
    debug_write(b"\n");
    fail(operation)
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
    debug_write(b"[io-tcp-listener-probe] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
