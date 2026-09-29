#![no_std]
#![no_main]

//! Declared TCP socket options driven against a scripted frame peer.
//!
//! As `options-baseline` and as `options-tuned` it holds one exact destination
//! per scenario port and runs its three scenarios in the order the plane
//! checker's marker chain names them. The generation declares each holder's
//! options; this probe never names one and proves it cannot change them. The
//! peer judges the wire; this probe reports what the application observed.

use slime_components::network_io::{Connection, NetworkIo, NetworkNotifications, NetworkReply};
use slime_proto::network_service as net;
use slime_rt::{debug_write, exit, monotonic_frequency, monotonic_read, resolve_binding};

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
/// Each small write waits at least this long after the previous one.
const WRITE_GAP_MS: u64 = 25;
/// How often a probe of a mute connection asks for its terminal status.
const TERMINAL_POLL_MS: u64 = 50;
const TERMINAL_POLLS: u32 = 240;
/// An operation code network-service/v1 does not define: a client's attempt
/// to select or change its own socket options.
const OP_SET_OPTION: u8 = 11;

// The scripted streams: byte `i` is `(31 * i + seed) mod 256`.
const BURST_WRITES: usize = 8;
const BURST_WRITE_BYTES: usize = 64;
const BURST: (u8, usize) = (5, BURST_WRITES * BURST_WRITE_BYTES);
const STREAM: (u8, usize) = (17, 4096);
const SILENT: (u8, usize) = (29, 256);
/// The peer's whole stream on a completed scenario.
const DONE: u8 = b'D';

struct Role {
    name: &'static [u8],
    control: &'static [u8],
    provision: &'static [u8],
    done: &'static [u8],
    /// Nagle, the stream or quiet period, and the mute scenario.
    ports: [u16; 3],
}

const BASELINE: Role = Role {
    name: b"baseline",
    control: b"options-baseline-control",
    provision: b"options-baseline-provision",
    done: b"notification:options-baseline-done+wait",
    ports: [4290, 4291, 4292],
};
const TUNED: Role = Role {
    name: b"tuned",
    control: b"options-tuned-control",
    provision: b"options-tuned-provision",
    done: b"notification:options-tuned-done+wait",
    ports: [4293, 4294, 4295],
};

struct Probe<'a> {
    io: NetworkIo<'a>,
    role: &'static Role,
    rate: u64,
    wait: u32,
}

fn main(_: u32) {
    let (role, control) = if let Ok(control) = resolve_binding(BASELINE.control) {
        (&BASELINE, control)
    } else {
        (&TUNED, binding(TUNED.control))
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
                request_signal: binding(b"notification:options-network-request+signal"),
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
    probe.refuse_mutation();
    marker(role.name, b"attached=1 mutation_refused=1");
    if role.name == BASELINE.name {
        probe.burst(b"nagle-on");
        probe.stream();
        probe.mute(b"silent-timeout", true);
    } else {
        probe.burst(b"nagle-off");
        probe.quiet();
        probe.mute(b"dead-peer", false);
    }
    probe.io.finish().unwrap_or_else(|_| fail(b"shutdown"));
    marker(role.name, b"scenarios=3 shutdown=1");
    exit(0)
}

impl Probe<'_> {
    /// The service holds no operation that selects or changes a socket
    /// option; asking for one is refused as unsupported and creates nothing.
    fn refuse_mutation(&mut self) {
        let request = net::WireNetworkRequest {
            magic: net::NETWORK_MAGIC,
            version: net::FORMAT_VERSION,
            op: OP_SET_OPTION,
            transport: net::TRANSPORT_NONE,
            flags: 0,
            port: 0,
            name_len: 0,
            capability: 0,
            address_kind: net::ADDRESS_NONE,
            reserved: [0; 7],
            endpoint: [0; 24],
        };
        let reply = self
            .io
            .transact_raw(request, slime_proto::io_queue::DIRECTION_NONE, 0)
            .unwrap_or_else(|_| fail(b"mutation request"));
        if reply.queue_status != slime_proto::io_queue::STATUS_OK
            || reply.status_detail != net::STATUS_UNSUPPORTED
            || reply.capability_kind != net::CAPABILITY_NONE
            || reply.capability != 0
            || reply.transferred != 0
        {
            status(b"mutation", &reply);
        }
    }

    /// Small writes, each accepted before the next and spaced apart, then the
    /// peer's completion byte and a graceful close.
    fn burst(&mut self, name: &[u8]) {
        let tcp = self.connect(self.role.ports[0]);
        let bytes = pattern(BURST);
        for chunk in bytes[..BURST.1].chunks(BURST_WRITE_BYTES) {
            self.send_all(&tcp, chunk);
            self.sleep_ms(WRITE_GAP_MS);
        }
        self.complete(tcp);
        scenario(self.role.name, name);
        write_number(b" writes=", BURST_WRITES as u64);
        write_number(b" sent=", BURST.1 as u64);
        debug_write(b" done=1\n");
    }

    /// A stream twice the transmit buffer, written as fast as it is accepted.
    fn stream(&mut self) {
        let tcp = self.connect(self.role.ports[1]);
        let bytes = pattern(STREAM);
        self.send_all(&tcp, &bytes[..STREAM.1]);
        self.complete(tcp);
        scenario(self.role.name, b"loss");
        write_number(b" sent=", STREAM.1 as u64);
        debug_write(b" done=1\n");
    }

    /// Nothing to send: the connection idles until the peer completes it.
    fn quiet(&mut self) {
        let tcp = self.connect(self.role.ports[1]);
        self.complete(tcp);
        scenario(self.role.name, b"keepalive");
        debug_write(b" done=1\n");
    }

    /// The peer never speaks after its SYN-ACK; only the declared idle timeout
    /// ends the connection, and close reports that same typed timeout.
    fn mute(&mut self, name: &[u8], data: bool) {
        let tcp = self.connect(self.role.ports[2]);
        if data {
            let bytes = pattern(SILENT);
            let reply = self
                .io
                .send(&tcp, &bytes[..SILENT.1])
                .unwrap_or_else(|_| fail(b"silent send request"));
            if !reply.is_success() || reply.transferred as usize != SILENT.1 {
                status(b"silent send", &reply);
            }
        }
        let mut polls = 0;
        loop {
            let mut byte = [0u8; 1];
            let reply = self
                .io
                .recv_nonblocking(&tcp, &mut byte)
                .unwrap_or_else(|_| fail(b"terminal request"));
            if reply.status_detail == net::STATUS_TIMEOUT && reply.transferred == 0 {
                break;
            }
            if reply.status_detail != net::STATUS_WOULD_BLOCK || reply.transferred != 0 {
                status(b"terminal", &reply);
            }
            polls += 1;
            if polls > TERMINAL_POLLS {
                fail(b"terminal polls");
            }
            self.sleep_ms(TERMINAL_POLL_MS);
        }
        let reply = self
            .io
            .close(tcp)
            .unwrap_or_else(|_| fail(b"mute close request"));
        if reply.status_detail != net::STATUS_TIMEOUT {
            status(b"mute close", &reply);
        }
        scenario(self.role.name, name);
        debug_write(b" terminal=timeout\n");
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

fn pattern(declared: (u8, usize)) -> [u8; 4096] {
    let mut bytes = [0u8; 4096];
    for (index, byte) in bytes.iter_mut().enumerate().take(declared.1) {
        *byte = (index * 31 + declared.0 as usize) as u8;
    }
    bytes
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

fn scenario(role: &[u8], name: &[u8]) {
    debug_write(b"[io-tcp-options-probe] role=");
    debug_write(role);
    debug_write(b" scenario=");
    debug_write(name);
}

fn marker(role: &[u8], message: &[u8]) {
    debug_write(b"[io-tcp-options-probe] role=");
    debug_write(role);
    debug_write(b" ");
    debug_write(message);
    debug_write(b"\n");
}

fn binding(name: &[u8]) -> u32 {
    resolve_binding(name).unwrap_or_else(|_| fail(b"binding"))
}

fn status(operation: &[u8], reply: &NetworkReply) -> ! {
    debug_write(b"[io-tcp-options-probe] ");
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
    debug_write(b"[io-tcp-options-probe] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
