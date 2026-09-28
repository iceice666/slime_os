#![no_std]
#![no_main]

//! The TCP impairment scenarios, one destination port each, in the order the
//! plane checker's marker chain names them. The scripted peer decides what the
//! wire does; this probe only reports what the application observed.

use slime_components::network_io::{Connection, NetworkIo, NetworkNotifications, NetworkReply};
use slime_proto::network_service;
use slime_rt::{debug_write, exit, monotonic_frequency, monotonic_read};

slime_rt::entry!(main);

const SERVICE_SLOT: u32 = 0;
const PROVISION_SLOT: u32 = 1;
const PEER: [u8; 4] = [10, 0, 0, 2];
const REORDER_PORT: u16 = 4260;
const LOSS_PORT: u16 = 4261;
const SILENT_PORT: u16 = 4262;
const RECEIVE_WINDOW_PORT: u16 = 4263;
const SEND_WINDOW_PORT: u16 = 4264;
const STREAM_BYTES: usize = 4096;
const STALL_MS: u64 = 2000;
// TIME-WAIT keeps a closed socket in the service's fixed pool for ten seconds,
// so a later scenario may find the pool exhausted until one of them expires.
const CONNECT_RETRY_MS: u64 = 250;
const CONNECT_ATTEMPTS: u32 = 80;
// Longer than the four seconds the service retains a blocking request.
const REQUEST_TIMEOUT_SECONDS: u64 = 6;
const RING_BASE: u64 = 0x0000_001b_0000_0000;
const DATA_BASE: u64 = RING_BASE + 4096;

struct Probe<'a> {
    io: NetworkIo<'a>,
    rate: u64,
    wait: u32,
}

fn main(_: u32) {
    let rate = monotonic_frequency().unwrap_or_else(|_| fail(b"clock rate"));
    let request_signal = binding(b"notification:impairment-network-request+signal");
    let wait = binding(b"notification:impairment-client-done+wait");
    // SAFETY: these page-aligned regions are reserved solely for this adapter,
    // disjoint from the component image, stack, and each other.
    let io = unsafe {
        NetworkIo::attach_with_notifications(
            SERVICE_SLOT,
            PROVISION_SLOT,
            RING_BASE,
            DATA_BASE,
            NetworkNotifications {
                request_signal,
                completion_wait: wait,
                timeout_ticks: rate
                    .checked_mul(REQUEST_TIMEOUT_SECONDS)
                    .unwrap_or_else(|| fail(b"timer range")),
            },
        )
    }
    .unwrap_or_else(|_| fail(b"application attach"));
    let mut probe = Probe { io, rate, wait };
    let payload = stream();

    let tcp = probe.connect(REORDER_PORT);
    probe.echo(&tcp, &payload, 0);
    probe.close(tcp);
    report(b"reorder", None, None);

    let tcp = probe.connect(LOSS_PORT);
    probe.echo(&tcp, &payload, 0);
    probe.close(tcp);
    report(b"loss", None, None);

    probe.silent(&payload);

    let tcp = probe.connect(RECEIVE_WINDOW_PORT);
    probe.send_all(&tcp, &payload, 0);
    probe.sleep_ms(STALL_MS);
    let mut received = [0u8; STREAM_BYTES];
    probe.receive_all(&tcp, &mut received, 0);
    if received != payload {
        fail(b"receive-window stream identity");
    }
    probe.close(tcp);
    report(b"receive-window", Some((b" stalled_ms=", STALL_MS)), None);

    let tcp = probe.connect(SEND_WINDOW_PORT);
    let queued = probe.fill_closed_window(&tcp, &payload);
    probe.echo(&tcp, &payload, queued);
    probe.close(tcp);
    report(
        b"send-window",
        Some((b" queued=", queued as u64)),
        Some((b" backpressure=", 1)),
    );

    probe.io.finish().unwrap_or_else(|_| fail(b"shutdown"));
    debug_write(b"[io-tcp-impairment-probe] scenarios=5 shutdown=1\n");
    exit(0)
}

impl Probe<'_> {
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
            if reply.status_detail != network_service::STATUS_EXHAUSTED {
                refused(b"connect", &reply);
            }
            self.sleep_ms(CONNECT_RETRY_MS);
        }
        fail(b"connect attempts")
    }

    /// Exchange the whole stream, starting after `sent` bytes already queued.
    fn echo(&mut self, tcp: &Connection, payload: &[u8; STREAM_BYTES], mut sent: usize) {
        let mut received = [0u8; STREAM_BYTES];
        let mut read = 0;
        while read < STREAM_BYTES || sent < STREAM_BYTES {
            if sent < STREAM_BYTES {
                sent += self.send(tcp, &payload[sent..]);
            }
            if read < STREAM_BYTES {
                read += self.receive(tcp, &mut received[read..]);
            }
        }
        if received != *payload {
            fail(b"echo stream identity");
        }
    }

    fn send_all(&mut self, tcp: &Connection, payload: &[u8; STREAM_BYTES], mut sent: usize) {
        while sent < STREAM_BYTES {
            sent += self.send(tcp, &payload[sent..]);
        }
    }

    fn receive_all(
        &mut self,
        tcp: &Connection,
        received: &mut [u8; STREAM_BYTES],
        mut read: usize,
    ) {
        while read < STREAM_BYTES {
            read += self.receive(tcp, &mut received[read..]);
        }
    }

    fn send(&mut self, tcp: &Connection, bytes: &[u8]) -> usize {
        let reply = self
            .io
            .send(tcp, bytes)
            .unwrap_or_else(|_| fail(b"send request"));
        if !reply.is_success() || reply.transferred == 0 || reply.transferred as usize > bytes.len()
        {
            refused(b"send", &reply);
        }
        reply.transferred as usize
    }

    fn receive(&mut self, tcp: &Connection, bytes: &mut [u8]) -> usize {
        let reply = self
            .io
            .recv(tcp, bytes)
            .unwrap_or_else(|_| fail(b"recv request"));
        if !reply.is_success() || reply.transferred == 0 || reply.flags != 0 {
            refused(b"recv", &reply);
        }
        reply.transferred as usize
    }

    fn close(&mut self, tcp: Connection) {
        let reply = self
            .io
            .close(tcp)
            .unwrap_or_else(|_| fail(b"close request"));
        if !reply.is_success() {
            refused(b"close", &reply);
        }
    }

    /// The peer acknowledges a prefix and falls silent. The typed timeout must
    /// end the connection, its close must report the same, and a fresh
    /// connection to the same single-connection destination must then succeed.
    fn silent(&mut self, payload: &[u8; STREAM_BYTES]) {
        let tcp = self.connect(SILENT_PORT);
        let mut sent = 0;
        let terminal = loop {
            if sent == STREAM_BYTES {
                fail(b"silent peer accepted the whole stream");
            }
            let reply = self
                .io
                .send(&tcp, &payload[sent..])
                .unwrap_or_else(|_| fail(b"silent send request"));
            if reply.is_success() && reply.transferred != 0 {
                sent += reply.transferred as usize;
                continue;
            }
            break reply;
        };
        if terminal.status_detail != network_service::STATUS_TIMEOUT {
            refused(b"silent terminal", &terminal);
        }
        debug_write(b"[io-tcp-impairment-probe] scenario=silent terminal=timeout\n");
        let reply = self
            .io
            .close(tcp)
            .unwrap_or_else(|_| fail(b"silent close request"));
        if reply.status_detail != network_service::STATUS_TIMEOUT {
            refused(b"silent close", &reply);
        }
        let fresh = self.connect(SILENT_PORT);
        self.echo(&fresh, payload, 0);
        self.close(fresh);
        debug_write(b"[io-tcp-impairment-probe] scenario=silent fresh");
        write_number(b" sent=", STREAM_BYTES as u64);
        write_number(b" received=", STREAM_BYTES as u64);
        debug_write(b" identical=1\n");
    }

    /// Queue bytes behind the peer's zero window until the service reports
    /// backpressure; return how many bytes it accepted before refusing more.
    fn fill_closed_window(&mut self, tcp: &Connection, payload: &[u8; STREAM_BYTES]) -> usize {
        let mut queued = 0;
        loop {
            let reply = self
                .io
                .send_nonblocking(tcp, &payload[queued..])
                .unwrap_or_else(|_| fail(b"window send request"));
            if reply.is_success() && reply.transferred != 0 {
                queued += reply.transferred as usize;
                if queued == STREAM_BYTES {
                    fail(b"zero window accepted the whole stream");
                }
                continue;
            }
            if reply.status_detail != network_service::STATUS_WOULD_BLOCK || reply.transferred != 0
            {
                refused(b"window backpressure", &reply);
            }
            return queued;
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

fn stream() -> [u8; STREAM_BYTES] {
    let mut payload = [0u8; STREAM_BYTES];
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte = (index * 37 + 11) as u8;
    }
    payload
}

fn report(scenario: &[u8], first: Option<(&[u8], u64)>, second: Option<(&[u8], u64)>) {
    debug_write(b"[io-tcp-impairment-probe] scenario=");
    debug_write(scenario);
    for (label, value) in [first, second].into_iter().flatten() {
        write_number(label, value);
    }
    write_number(b" sent=", STREAM_BYTES as u64);
    write_number(b" received=", STREAM_BYTES as u64);
    debug_write(b" identical=1\n");
}

fn now() -> u64 {
    monotonic_read().unwrap_or_else(|_| fail(b"monotonic read"))
}

fn binding(name: &[u8]) -> u32 {
    slime_rt::resolve_binding(name).unwrap_or_else(|_| fail(b"notification binding"))
}

fn refused(operation: &[u8], reply: &NetworkReply) -> ! {
    debug_write(b"[io-tcp-impairment-probe] ");
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
    debug_write(b"[io-tcp-impairment-probe] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
