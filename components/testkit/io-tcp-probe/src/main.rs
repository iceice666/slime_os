#![no_std]
#![no_main]

use slime_components::network_io::{NetworkIo, NetworkNotifications, NetworkReply};
use slime_components::tick_clock::TickClock;
use slime_proto::network_service;
use slime_rt::{debug_write, exit, monotonic_frequency, monotonic_read, yield_now};

slime_rt::entry!(main);

const SERVICE_SLOT: u32 = 0;
const PROVISION_SLOT: u32 = 1;
const PEER: [u8; 4] = [10, 0, 0, 2];
const ECHO_PORT: u16 = 4242;
const HOLD_MS: i64 = 3000;
const DEADLINE_MS: i64 = 20000;
const RING_BASE: u64 = 0x0000_001b_0000_0000;
const DATA_BASE: u64 = RING_BASE + 4096;
const PAYLOAD_BYTES: usize = 4096;

fn main(_: u32) {
    let rate = monotonic_frequency().unwrap_or_else(|_| fail(b"clock rate"));
    let base = monotonic_read().unwrap_or_else(|_| fail(b"monotonic read"));
    let clock = TickClock::new(rate, base).unwrap_or_else(|_| fail(b"clock rate too slow"));
    write_number(b"[io-tcp-probe] clock rate=", rate);
    debug_write(b"\n");
    let reset_mode = slime_rt::resolve_binding(b"network-reset-mode").is_ok();
    let phase = if reset_mode {
        match slime_rt::lifecycle_parameter_read(slime_rt::PARAMETER_SELF_SLOT, 2) {
            Ok(value) if value <= 1 => value,
            Err(slime_rt::ERR_INVALID_ARG) => 0,
            _ => fail(b"reset phase"),
        }
    } else {
        0
    };
    // SAFETY: these page-aligned regions are reserved solely for this adapter,
    // disjoint from the component image, stack, and each other.
    let mut io = unsafe {
        if reset_mode {
            NetworkIo::attach_with_notifications(
                SERVICE_SLOT,
                PROVISION_SLOT,
                RING_BASE,
                DATA_BASE,
                NetworkNotifications {
                    request_signal: binding(b"notification:reset-network-request+signal"),
                    completion_wait: binding(b"notification:reset-client-done+wait"),
                    timeout_ticks: rate.checked_mul(5).unwrap_or_else(|| fail(b"timer range")),
                },
            )
        } else {
            NetworkIo::attach(SERVICE_SLOT, PROVISION_SLOT, RING_BASE, DATA_BASE)
        }
    }
    .unwrap_or_else(|_| fail(b"application attach"));
    let old = if reset_mode && phase == 1 {
        Some(reject_old_handle(&mut io))
    } else {
        None
    };
    let tcp = io
        .connect_ipv4(PEER, ECHO_PORT)
        .unwrap_or_else(|_| fail(b"tcp connect request"))
        .take_connection()
        .unwrap_or_else(|| fail(b"tcp handshake"));
    if old.is_some_and(|old| old == tcp.id()) {
        fail(b"reset identity reused");
    }
    if reset_mode && phase == 0 {
        for (key, value) in [(2, 1), (3, tcp.id() & u32::MAX as u64), (4, tcp.id() >> 32)] {
            slime_rt::lifecycle_parameter_write(slime_rt::PARAMETER_SELF_SLOT, key, value)
                .unwrap_or_else(|_| fail(b"reset identity write"));
        }
        let mut payload = [0u8; 1024];
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte = (index * 37 + 11) as u8;
        }
        let mut sent = 0;
        while sent < payload.len() {
            let reply = io
                .send(&tcp, &payload[sent..])
                .unwrap_or_else(|_| fail(b"reset send request"));
            if reply.is_success() && reply.transferred > 0 {
                sent += reply.transferred as usize;
            } else {
                check_would_block(&reply);
            }
            if now(&clock) >= DEADLINE_MS {
                fail(b"reset send deadline");
            }
        }
        let mut response = [0u8; 1];
        let reply = io
            .recv(&tcp, &mut response)
            .unwrap_or_else(|_| fail(b"reset receive request"));
        if reply.status_detail != network_service::STATUS_RESET || reply.transferred != 0 {
            fail(b"driver reset terminal");
        }
        let close = io
            .close(tcp)
            .unwrap_or_else(|_| fail(b"reset close request"));
        if close.status_detail != network_service::STATUS_DENIED {
            fail(b"reset stale close");
        }
        debug_write(b"[io-tcp-probe] driver reset sent=1024 terminal=reset transferred=0\n");
        io.finish().unwrap_or_else(|_| fail(b"reset finish"));
        debug_write(b"[io-tcp-probe] driver reset loans returned=2 shutdown=1\n");
        exit(0)
    }
    if reset_mode {
        debug_write(b"[io-tcp-probe] old_handle_refused=1 fresh_identity=1\n");
    }
    debug_write(b"[io-tcp-probe] tcp established=1 rights=connect,send,recv\n");
    let mut payload = [0; PAYLOAD_BYTES];
    for (index, byte) in payload.iter_mut().enumerate() {
        *byte = (index * 37 + 11) as u8;
    }
    let mut received = [0; PAYLOAD_BYTES];
    let mut sent = 0;
    let mut read = 0;
    let mut short_writes = 0;
    let mut would_block = 0;
    while read < PAYLOAD_BYTES || sent < PAYLOAD_BYTES {
        if sent < PAYLOAD_BYTES {
            let offered = PAYLOAD_BYTES - sent;
            let reply = io
                .send(&tcp, &payload[sent..])
                .unwrap_or_else(|_| fail(b"send request"));
            if reply.is_success() {
                if reply.transferred == 0 || reply.transferred as usize > offered {
                    fail(b"send count");
                }
                short_writes += u64::from((reply.transferred as usize) < offered);
                sent += reply.transferred as usize;
            } else {
                check_would_block(&reply);
                would_block += 1;
            }
        }
        if read < PAYLOAD_BYTES {
            let reply = io
                .recv(&tcp, &mut received[read..])
                .unwrap_or_else(|_| fail(b"recv request"));
            if reply.is_success() {
                if reply.transferred == 0 || reply.flags != 0 {
                    fail(b"premature eof");
                }
                read += reply.transferred as usize;
            } else {
                check_would_block(&reply);
                would_block += 1;
            }
        }
        if now(&clock) >= DEADLINE_MS {
            fail(b"payload timeout");
        }
    }
    if received != payload || short_writes == 0 {
        fail(b"payload identity or partial write");
    }
    write_number(b"[io-tcp-probe] stream sent=", sent as u64);
    write_number(b" received=", read as u64);
    debug_write(b" identical=1");
    write_number(b" partial_writes=", short_writes);
    write_number(b" would_block=", would_block);
    debug_write(b"\n");
    if !io
        .close(tcp)
        .unwrap_or_else(|_| fail(b"close request"))
        .is_success()
    {
        fail(b"close");
    }
    debug_write(b"[io-tcp-probe] tcp close completed=1\n");

    let refused = io
        .connect_ipv4(PEER, ECHO_PORT + 1)
        .unwrap_or_else(|_| fail(b"refusal request"));
    if refused.status_detail != network_service::STATUS_REFUSED {
        fail(b"peer refusal");
    }
    let denied = io
        .connect_ipv4([10, 0, 0, 3], ECHO_PORT)
        .unwrap_or_else(|_| fail(b"denial request"));
    if denied.status_detail != network_service::STATUS_DENIED {
        fail(b"destination accepted");
    }
    let port_denied = io
        .connect_ipv4(PEER, ECHO_PORT + 2)
        .unwrap_or_else(|_| fail(b"port denial request"));
    if port_denied.status_detail != network_service::STATUS_DENIED {
        fail(b"port accepted");
    }
    debug_write(b"[io-tcp-probe] peer refused=1 undeclared address=1 port=1\n");
    while now(&clock) < HOLD_MS {
        yield_now();
    }
    write_number(b"[io-tcp-probe] held ms=", HOLD_MS as u64);
    debug_write(b"\n");
    io.finish().unwrap_or_else(|_| fail(b"shutdown"));
    debug_write(b"[io-tcp-probe] closed capabilities=1 shutdown=1\n");
    exit(0)
}

fn binding(name: &[u8]) -> u32 {
    slime_rt::resolve_binding(name).unwrap_or_else(|_| fail(b"reset binding"))
}

fn reject_old_handle(io: &mut NetworkIo<'_>) -> u64 {
    let low = slime_rt::lifecycle_parameter_read(slime_rt::PARAMETER_SELF_SLOT, 3)
        .unwrap_or_else(|_| fail(b"reset identity low"));
    let high = slime_rt::lifecycle_parameter_read(slime_rt::PARAMETER_SELF_SLOT, 4)
        .unwrap_or_else(|_| fail(b"reset identity high"));
    if low > u32::MAX as u64 || high > u32::MAX as u64 {
        fail(b"reset identity bounds");
    }
    let old = low | (high << 32);
    let request = network_service::WireNetworkRequest {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        op: network_service::OP_SEND,
        transport: network_service::TRANSPORT_NONE,
        flags: 0,
        port: 0,
        name_len: 0,
        capability: old,
        address_kind: network_service::ADDRESS_NONE,
        reserved: [0; 7],
        endpoint: [0; 24],
    };
    let reply = io
        .transact_raw(request, slime_proto::io_queue::DIRECTION_DEVICE_READ, 1)
        .unwrap_or_else(|_| fail(b"reset old request"));
    if old == 0
        || reply.status_detail != network_service::STATUS_DENIED
        || reply.transferred != 0
        || reply.capability_kind != network_service::CAPABILITY_NONE
        || reply.capability != 0
    {
        fail(b"reset old handle accepted");
    }
    old
}

fn now(clock: &TickClock) -> i64 {
    clock.millis(monotonic_read().unwrap_or_else(|_| fail(b"monotonic read")))
}

fn check_would_block(reply: &NetworkReply) {
    if reply.status_detail != network_service::STATUS_WOULD_BLOCK || reply.transferred != 0 {
        write_number(
            b"[io-tcp-probe] stream error code=",
            u64::from(reply.status_detail.unsigned_abs()),
        );
        write_number(b" queue=", u64::from(reply.queue_status));
        debug_write(b"\n");
        fail(b"stream error");
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

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[io-tcp-probe] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
