#![no_std]
#![no_main]

//! Declared TCP socket buffer sizes driven against a scripted frame peer.
//!
//! As `buffers-large` and as `buffers-baseline` it holds one exact destination
//! per scenario port and runs its three scenarios in the order the plane
//! checker's marker chain names them. The generation declares each holder's
//! buffer sizes; this probe never names one. `buffers-baseline` starts its
//! first scenario only after `buffers-large` signals that its last connection
//! closed, so a pooled socket that served the large row must be rebuilt before
//! it serves the baseline row. The peer judges the wire; this probe reports
//! what the application observed.

use slime_components::console_line::Line;
use slime_components::network_io::{Connection, NetworkIo, NetworkNotifications, NetworkReply};
use slime_proto::network_service as net;
use slime_rt::{exit, monotonic_frequency, monotonic_read, resolve_binding};

slime_rt::entry!(main);

const PEER: [u8; 4] = [10, 0, 0, 2];
const RING_BASE: u64 = 0x0000_001e_0000_0000;
const DATA_BASE: u64 = RING_BASE + 4096;
// Longer than the four seconds the service retains a blocking request.
const REQUEST_TIMEOUT_SECONDS: u64 = 6;
/// How many four-second service waits one scripted step may take.
const WAIT_ROUNDS: u32 = 8;
// TIME-WAIT keeps a closed socket in the service's fixed pool for ten seconds,
// so a later scenario may find the pool exhausted until one of them expires.
const CONNECT_RETRY_MS: u64 = 250;
const CONNECT_ATTEMPTS: u32 = 80;
/// The bytes one send or receive request moves at most.
const CHUNK: usize = 4096;

// The scripted streams: byte `i` is `(31 * i + seed) mod 256`.
const TX_STREAM: (u8, usize) = (7, 262_144);
const THROUGHPUT_STREAM: (u8, usize) = (11, 1_048_576);
/// The receive scenario's stall before the probe reads anything.
const RECEIVE_STALL_MS: u64 = 2000;
/// The peer's whole stream on a completed transmit scenario.
const DONE: u8 = b'D';

struct Role {
    name: &'static [u8],
    control: &'static [u8],
    provision: &'static [u8],
    done: &'static [u8],
    /// The transmit, receive and throughput scenario ports.
    ports: [u16; 3],
    /// The peer's receive-scenario stream: twice the declared receive buffer.
    rx_stream: (u8, usize),
}

const LARGE: Role = Role {
    name: b"large",
    control: b"buffers-large-control",
    provision: b"buffers-large-provision",
    done: b"notification:buffers-large-done+wait",
    ports: [4300, 4301, 4302],
    rx_stream: (13, 2 * 65_536),
};
const BASELINE: Role = Role {
    name: b"baseline",
    control: b"buffers-baseline-control",
    provision: b"buffers-baseline-provision",
    done: b"notification:buffers-baseline-done+wait",
    ports: [4310, 4311, 4312],
    rx_stream: (14, 2 * 2048),
};
const HANDOFF_SIGNAL: &[u8] = b"notification:buffers-handoff+signal";
const HANDOFF_WAIT: &[u8] = b"notification:buffers-handoff+wait";

struct Probe<'a> {
    io: NetworkIo<'a>,
    role: &'static Role,
    rate: u64,
    wait: u32,
}

fn main(_: u32) {
    let (role, control) = if let Ok(control) = resolve_binding(LARGE.control) {
        (&LARGE, control)
    } else {
        (&BASELINE, binding(BASELINE.control))
    };
    let rate = monotonic_frequency().unwrap_or_else(|_| fail(b"clock rate"));
    let wait = binding(role.done);
    // SAFETY: these page-aligned regions are reserved solely for this adapter,
    // disjoint from the component image, stack, and each other.
    let io = unsafe {
        NetworkIo::attach_with_notifications(
            control,
            binding(role.provision),
            RING_BASE,
            DATA_BASE,
            NetworkNotifications {
                request_signal: binding(b"notification:buffers-network-request+signal"),
                completion_wait: wait,
                timeout_ticks: rate
                    .checked_mul(REQUEST_TIMEOUT_SECONDS)
                    .unwrap_or_else(|| fail(b"timer range")),
            },
        )
    }
    .unwrap_or_else(|_| fail(b"application attach"));
    let mut probe = Probe {
        io,
        role,
        rate,
        wait,
    };
    marker(role.name, b"attached=1");
    if role.name == BASELINE.name {
        // The large holder signals once, after its last close settled.
        slime_rt::notification_wait(binding(HANDOFF_WAIT)).unwrap_or_else(|_| fail(b"handoff"));
    }
    probe.transmit(b"tx-flight", role.ports[0], TX_STREAM);
    probe.receive();
    probe.transmit(b"throughput", role.ports[2], THROUGHPUT_STREAM);
    probe.io.finish().unwrap_or_else(|_| fail(b"shutdown"));
    marker(role.name, b"scenarios=3 shutdown=1");
    if role.name == LARGE.name
        && slime_rt::notification_signal(binding(HANDOFF_SIGNAL)) != slime_rt::ERR_SUCCESS
    {
        fail(b"handoff signal");
    }
    exit(0)
}

impl Probe<'_> {
    /// Stream `declared` as fast as the service accepts it, then read the
    /// peer's completion byte and close gracefully.
    fn transmit(&mut self, name: &[u8], port: u16, declared: (u8, usize)) {
        let tcp = self.connect(port);
        let mut offset = 0;
        let mut chunk = [0u8; CHUNK];
        while offset < declared.1 {
            let length = CHUNK.min(declared.1 - offset);
            fill(&mut chunk[..length], declared.0, offset);
            self.send_all(&tcp, &chunk[..length]);
            offset += length;
        }
        self.complete(tcp);
        let mut line = scenario(self.role.name, name);
        line.bytes(b" sent=")
            .decimal(declared.1 as u64)
            .bytes(b" done=1\n")
            .emit();
    }

    /// Stall without reading while the peer fills the receive buffer, then
    /// read and check the peer's whole stream, and close gracefully.
    fn receive(&mut self) {
        let tcp = self.connect(self.role.ports[1]);
        self.sleep_ms(RECEIVE_STALL_MS);
        let declared = self.role.rx_stream;
        let mut received = 0;
        let mut identical = true;
        let mut rounds = 0;
        let mut chunk = [0u8; CHUNK];
        let mut expected = [0u8; CHUNK];
        while received < declared.1 {
            let wanted = CHUNK.min(declared.1 - received);
            let reply = self
                .io
                .recv(&tcp, &mut chunk[..wanted])
                .unwrap_or_else(|_| fail(b"receive request"));
            if reply.is_success() && reply.transferred != 0 {
                let count = reply.transferred as usize;
                fill(&mut expected[..count], declared.0, received);
                identical &= chunk[..count] == expected[..count];
                received += count;
                rounds = 0;
                continue;
            }
            waiting(b"receive", &reply);
            rounds += 1;
            if rounds > WAIT_ROUNDS {
                fail(b"receive rounds");
            }
        }
        if !identical {
            fail(b"receive stream bytes");
        }
        self.close(tcp);
        let mut line = scenario(self.role.name, b"rx-window");
        line.bytes(b" stalled_ms=")
            .decimal(RECEIVE_STALL_MS)
            .bytes(b" received=")
            .decimal(received as u64)
            .bytes(b" identical=1\n")
            .emit();
    }

    /// Connect to one scenario port, waiting out a pool held by TIME-WAIT.
    fn connect(&mut self, port: u16) -> Connection {
        for _ in 0..CONNECT_ATTEMPTS {
            let mut reply = self
                .io
                .connect_ipv4(PEER, port)
                .unwrap_or_else(|_| fail(b"connect request"));
            if let Some(connection) = reply.take_connection() {
                return connection;
            }
            if reply.status_detail != net::STATUS_EXHAUSTED {
                status(b"connect", &reply);
            }
            self.sleep_ms(CONNECT_RETRY_MS);
        }
        fail(b"connect attempts")
    }

    /// Read the peer's one completion byte, then close gracefully.
    fn complete(&mut self, tcp: Connection) {
        let mut rounds = 0;
        loop {
            let mut byte = [0u8; 1];
            let reply = self
                .io
                .recv(&tcp, &mut byte)
                .unwrap_or_else(|_| fail(b"completion request"));
            if reply.is_success() && reply.transferred == 1 {
                if byte[0] != DONE {
                    fail(b"completion byte");
                }
                break;
            }
            waiting(b"completion", &reply);
            rounds += 1;
            if rounds > WAIT_ROUNDS {
                fail(b"completion rounds");
            }
        }
        self.close(tcp);
    }

    fn close(&mut self, tcp: Connection) {
        let reply = self
            .io
            .close(tcp)
            .unwrap_or_else(|_| fail(b"close request"));
        if !reply.is_success() {
            status(b"close", &reply);
        }
    }

    fn send_all(&mut self, tcp: &Connection, bytes: &[u8]) {
        let mut sent = 0;
        let mut rounds = 0;
        while sent < bytes.len() {
            let reply = self
                .io
                .send(tcp, &bytes[sent..])
                .unwrap_or_else(|_| fail(b"send request"));
            if reply.is_success() && reply.transferred != 0 {
                sent += reply.transferred as usize;
                rounds = 0;
                continue;
            }
            waiting(b"send", &reply);
            rounds += 1;
            if rounds > WAIT_ROUNDS {
                fail(b"send rounds");
            }
        }
    }

    /// Block on the completion notification until a one-shot timer expires.
    /// A service completion may wake it early; only the deadline ends it.
    fn sleep_ms(&mut self, ms: u64) {
        let ticks = (self.rate / 1000)
            .checked_mul(ms)
            .unwrap_or_else(|| fail(b"sleep range"))
            .max(1);
        let deadline = now()
            .checked_add(ticks)
            .unwrap_or_else(|| fail(b"sleep deadline"));
        let timer = slime_rt::timer_arm(ticks).unwrap_or_else(|_| fail(b"sleep timer"));
        while now() < deadline {
            slime_rt::notification_wait(self.wait).unwrap_or_else(|_| fail(b"sleep wait"));
        }
        let cancelled = slime_rt::timer_cancel(timer);
        if cancelled != slime_rt::ERR_SUCCESS && cancelled != slime_rt::ERR_BAD_CAP {
            fail(b"sleep timer cancellation");
        }
    }
}

/// Bytes `[offset, offset + bytes.len())` of the stream with `seed`.
fn fill(bytes: &mut [u8], seed: u8, offset: usize) {
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = ((offset + index) * 31 + seed as usize) as u8;
    }
}

fn now() -> u64 {
    monotonic_read().unwrap_or_else(|_| fail(b"monotonic read"))
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

/// A scenario line under construction. Each line is emitted in one write,
/// because the root and the service log concurrently and a line built from
/// several writes can be interleaved.
fn scenario(role: &[u8], name: &[u8]) -> Line<128> {
    let mut line = Line::new();
    line.bytes(b"[io-tcp-buffers-probe] role=")
        .bytes(role)
        .bytes(b" scenario=")
        .bytes(name);
    line
}

fn marker(role: &[u8], message: &[u8]) {
    Line::<128>::new()
        .bytes(b"[io-tcp-buffers-probe] role=")
        .bytes(role)
        .bytes(b" ")
        .bytes(message)
        .bytes(b"\n")
        .emit();
}

fn binding(name: &[u8]) -> u32 {
    resolve_binding(name).unwrap_or_else(|_| fail(b"binding"))
}

fn status(operation: &[u8], reply: &NetworkReply) -> ! {
    Line::<256>::new()
        .bytes(b"[io-tcp-buffers-probe] ")
        .bytes(operation)
        .bytes(b" status=")
        .decimal(u64::from(reply.status_detail.unsigned_abs()))
        .bytes(b" queue=")
        .decimal(u64::from(reply.queue_status))
        .bytes(b" transferred=")
        .decimal(reply.transferred)
        .bytes(b"\n")
        .emit();
    fail(operation)
}

fn fail(reason: &[u8]) -> ! {
    Line::<256>::new()
        .bytes(b"[io-tcp-buffers-probe] fail: ")
        .bytes(reason)
        .bytes(b"\n")
        .emit();
    exit(1)
}
