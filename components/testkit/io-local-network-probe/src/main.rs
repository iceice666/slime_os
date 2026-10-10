#![no_std]
#![no_main]

use slime_components::console_line::Line;
use slime_components::network_io::{
    Connection, NetworkError, NetworkIo, NetworkNotifications, NetworkReply,
};
use slime_components::tick_clock::TickClock;
use slime_proto::network_service as net;
use slime_rt::{
    debug_write, exit, monotonic_frequency, monotonic_read, resolve_binding, yield_now,
};

slime_rt::entry!(main);

const ADDRESS: [u8; 4] = [127, 0, 0, 1];
const PORT: u16 = 7447;
const RING_BASE: u64 = 0x0000_001c_0000_0000;
const DATA_BASE: u64 = RING_BASE + 4096;
const DEADLINE_MS: i64 = 15000;
const PUBLISH_BYTES: usize = 4096;
const REPLY_BYTES: usize = 2048;

fn main(_: u32) {
    if let Ok(control) = resolve_binding(b"lifetime-survivor-control") {
        lifetime_survivor(control);
    }
    let publisher = resolve_binding(b"local-publisher-control").ok();
    let (role, control, provision) = if let Some(control) = publisher {
        (
            b"publisher".as_slice(),
            control,
            binding(b"local-publisher-provision"),
        )
    } else {
        (
            b"subscriber".as_slice(),
            binding(b"local-subscriber-control"),
            binding(b"local-subscriber-provision"),
        )
    };
    let rate = monotonic_frequency().unwrap_or_else(|_| fail(b"clock rate"));
    let base = monotonic_read().unwrap_or_else(|_| fail(b"clock read"));
    let clock = TickClock::new(rate, base).unwrap_or_else(|_| fail(b"clock rate too slow"));
    let notifications = NetworkNotifications {
        request_signal: binding(b"notification:local-network-request+signal"),
        completion_wait: binding(if publisher.is_some() {
            b"notification:local-publisher-done+wait"
        } else {
            b"notification:local-subscriber-done+wait"
        }),
        timeout_ticks: rate.checked_mul(5).unwrap_or_else(|| fail(b"timer range")),
    };
    // SAFETY: these distinct page-aligned regions are reserved solely for this
    // adapter, and the two declared endpoints belong to the same service.
    let mut io = unsafe {
        NetworkIo::attach_with_notifications(
            control,
            provision,
            RING_BASE,
            DATA_BASE,
            notifications,
        )
    }
    .unwrap_or_else(|_| fail(b"attach"));
    marker(role, b"attached=1");
    if publisher.is_some() {
        publish(&mut io, &clock, role);
    } else {
        subscribe(&mut io, &clock, role);
    }
    let wakes = io.notification_wakes();
    if wakes == 0 {
        fail(b"no notification waits");
    }
    Line::<128>::new()
        .bytes(b"[io-local-network-probe] role=")
        .bytes(role)
        .bytes(b" notification_wakes=")
        .decimal(wakes as u64)
        .bytes(b"\n")
        .emit();
    io.finish().unwrap_or_else(|_| fail(b"finish"));
    marker(role, b"loans returned=2 shutdown=1");
    exit(0)
}

fn lifetime_survivor(control: u32) -> ! {
    let service_fault = resolve_binding(b"service-fault-survivor").is_ok();
    let phase = match slime_rt::lifecycle_parameter_read(slime_rt::PARAMETER_SELF_SLOT, 2) {
        Ok(value) => value,
        Err(slime_rt::ERR_INVALID_ARG) => 0,
        Err(_) => fail(b"phase read"),
    };
    if phase > 1 {
        fail(b"phase range");
    }
    let rate = monotonic_frequency().unwrap_or_else(|_| fail(b"clock rate"));
    let base = monotonic_read().unwrap_or_else(|_| fail(b"clock read"));
    let clock = TickClock::new(rate, base).unwrap_or_else(|_| fail(b"clock rate too slow"));
    let notifications = NetworkNotifications {
        request_signal: binding(b"notification:lifetime-network-request+signal"),
        completion_wait: binding(b"notification:lifetime-survivor-done+wait"),
        timeout_ticks: rate.checked_mul(5).unwrap_or_else(|| fail(b"timer range")),
    };
    // SAFETY: these disjoint mappings and declared peer endpoints have the same
    // exclusive session ownership as the ordinary local probe above.
    let mut io = unsafe {
        NetworkIo::attach_with_notifications(
            control,
            binding(b"lifetime-survivor-provision"),
            RING_BASE,
            DATA_BASE,
            notifications,
        )
    }
    .unwrap_or_else(|_| fail(b"lifetime attach"));
    if service_fault && phase == 0 {
        slime_rt::lifecycle_parameter_write(slime_rt::PARAMETER_SELF_SLOT, 2, 1)
            .unwrap_or_else(|_| fail(b"service fault phase write"));
    }
    let previous = previous_connection(&mut io, phase);
    let listener = io
        .listen_ipv4(ADDRESS, PORT)
        .unwrap_or_else(|_| fail(b"listen request"))
        .take_listener()
        .unwrap_or_else(|| fail(b"listen"));
    let connection = loop {
        let mut reply = io
            .accept(&listener)
            .unwrap_or_else(|_| fail(b"accept request"));
        if let Some(connection) = reply.take_connection() {
            break connection;
        }
        would_block(&reply);
        bounded_yield(&clock);
    };
    remember_connection(&connection, previous, b"survivor");
    let mut bytes = [0u8; 1024];
    recv_all(&mut io, &connection, &mut bytes, &clock);
    if bytes
        .iter()
        .enumerate()
        .any(|(index, byte)| *byte != (index * 29 + 7) as u8)
    {
        fail(b"lifetime payload identity");
    }
    if service_fault && phase == 0 {
        debug_write(b"[io-network-lifetime-probe] role=survivor service_fault_pending=1\n");
        let mut extra = [0u8; 1];
        match io.recv(&connection, &mut extra) {
            Err(NetworkError::Lost | NetworkError::Malformed) => {
                debug_write(b"[io-network-lifetime-probe] role=survivor service_invalidated=1\n");
                exit(1)
            }
            _ => fail(b"service fault request returned"),
        }
    }
    send_all(&mut io, &connection, &[0x5a; 16], &clock);
    let mut extra = [0u8; 1];
    loop {
        let reply = io
            .recv(&connection, &mut extra)
            .unwrap_or_else(|_| fail(b"terminal recv"));
        if reply.status_detail == net::STATUS_WOULD_BLOCK {
            bounded_yield(&clock);
            continue;
        }
        if phase == 0 {
            if reply.status_detail != net::STATUS_RESET || reply.transferred != 0 {
                fail(b"missing peer reset");
            }
            debug_write(
                b"[io-network-lifetime-probe] role=survivor received=1024 identical=1 reset=1\n",
            );
        } else {
            if !reply.is_success()
                || reply.flags != net::FLAG_END_OF_STREAM
                || reply.transferred != 0
            {
                fail(b"missing peer eof");
            }
            debug_write(b"[io-network-lifetime-probe] role=survivor received=1024 identical=1 eof=1 fresh=1\n");
        }
        break;
    }
    let closed = io
        .close(connection)
        .unwrap_or_else(|_| fail(b"survivor close request"));
    if !(closed.is_success() || phase == 0 && closed.status_detail == net::STATUS_RESET) {
        fail(b"survivor close");
    }
    if !io
        .close_listener(listener)
        .unwrap_or_else(|_| fail(b"listener close request"))
        .is_success()
    {
        fail(b"listener close");
    }
    slime_rt::lifecycle_parameter_write(slime_rt::PARAMETER_SELF_SLOT, 2, 1)
        .unwrap_or_else(|_| fail(b"phase write"));
    io.finish().unwrap_or_else(|_| fail(b"lifetime finish"));
    debug_write(b"[io-network-lifetime-probe] role=survivor clean=1 loans_returned=2\n");
    exit(0)
}

fn previous_connection(io: &mut NetworkIo<'_>, phase: u64) -> Option<u64> {
    if phase == 0 {
        return None;
    }
    let low = slime_rt::lifecycle_parameter_read(slime_rt::PARAMETER_SELF_SLOT, 3)
        .unwrap_or_else(|_| fail(b"old identity low"));
    let high = slime_rt::lifecycle_parameter_read(slime_rt::PARAMETER_SELF_SLOT, 4)
        .unwrap_or_else(|_| fail(b"old identity high"));
    if low > u32::MAX as u64 || high > u32::MAX as u64 {
        fail(b"old identity bounds");
    }
    let old = low | (high << 32);
    if old == 0 {
        fail(b"old identity absent");
    }
    let request = net::WireNetworkRequest {
        magic: net::NETWORK_MAGIC,
        version: net::FORMAT_VERSION,
        op: net::OP_SEND,
        transport: net::TRANSPORT_NONE,
        flags: 0,
        port: 0,
        name_len: 0,
        capability: old,
        address_kind: net::ADDRESS_NONE,
        reserved: [0; 7],
        endpoint: [0; 24],
    };
    let reply = io
        .transact_raw(request, slime_proto::io_queue::DIRECTION_DEVICE_READ, 1)
        .unwrap_or_else(|_| fail(b"old handle request"));
    if reply.status_detail != net::STATUS_DENIED
        || reply.transferred != 0
        || reply.capability_kind != net::CAPABILITY_NONE
        || reply.capability != 0
    {
        fail(b"old handle accepted");
    }
    Some(old)
}

fn remember_connection(connection: &Connection, old: Option<u64>, role: &[u8]) {
    if let Some(old) = old {
        if connection.id() == old {
            fail(b"connection identity reused");
        }
        Line::<128>::new()
            .bytes(b"[io-network-lifetime-probe] role=")
            .bytes(role)
            .bytes(b" old_handle_refused=1 fresh_identity=1\n")
            .emit();
    } else {
        for (key, value) in [
            (3, connection.id() & u32::MAX as u64),
            (4, connection.id() >> 32),
        ] {
            slime_rt::lifecycle_parameter_write(slime_rt::PARAMETER_SELF_SLOT, key, value)
                .unwrap_or_else(|_| fail(b"remember identity"));
        }
    }
}

fn binding(name: &[u8]) -> u32 {
    resolve_binding(name).unwrap_or_else(|_| fail(b"binding"))
}

fn publish(io: &mut NetworkIo<'_>, clock: &TickClock, role: &[u8]) {
    refused(io.listen_ipv4(ADDRESS, PORT), net::STATUS_DENIED);
    refused(io.connect_ipv4([127, 0, 0, 2], PORT), net::STATUS_DENIED);
    refused(io.connect_ipv4(ADDRESS, PORT + 1), net::STATUS_DENIED);
    refused(
        io.transact_raw(
            capability_request(net::OP_SEND, 1),
            slime_proto::io_queue::DIRECTION_DEVICE_READ,
            1,
        ),
        net::STATUS_DENIED,
    );
    marker(role, b"authority_refusals=4");
    let mut attempts = 0;
    let connection = loop {
        attempts += 1;
        let mut reply = io
            .connect_ipv4(ADDRESS, PORT)
            .unwrap_or_else(|_| fail(b"connect request"));
        if let Some(connection) = reply.take_connection() {
            break connection;
        }
        if !matches!(
            reply.status_detail,
            net::STATUS_REFUSED | net::STATUS_WOULD_BLOCK
        ) || attempts >= 64
        {
            fail(b"connect refused");
        }
        bounded_yield(clock);
    };
    marker(role, b"connected=1");
    let mut payload = [0u8; PUBLISH_BYTES];
    fixture(&mut payload, 37, 11);
    send_all(io, &connection, &payload, clock);
    let mut answer = [0u8; REPLY_BYTES];
    recv_response_after_stall(io, &connection, &mut answer, clock);
    verify(&answer, 53, 19);
    marker(role, b"sent=4096 received=2048 identical=1");
    if !io
        .close(connection)
        .unwrap_or_else(|_| fail(b"close request"))
        .is_success()
    {
        fail(b"close");
    }
    marker(role, b"connections closed=1");
}

fn subscribe(io: &mut NetworkIo<'_>, clock: &TickClock, role: &[u8]) {
    refused(io.listen_ipv4(ADDRESS, PORT + 1), net::STATUS_DENIED);
    refused(
        io.transact_raw(
            capability_request(net::OP_ACCEPT, u64::MAX - 1),
            slime_proto::io_queue::DIRECTION_NONE,
            0,
        ),
        net::STATUS_DENIED,
    );
    let listener = io
        .listen_ipv4(ADDRESS, PORT)
        .unwrap_or_else(|_| fail(b"listen request"))
        .take_listener()
        .unwrap_or_else(|| fail(b"listen"));
    refused(io.listen_ipv4(ADDRESS, PORT), net::STATUS_EXHAUSTED);
    marker(role, b"authority_refusals=3");
    marker(role, b"listening=1");
    let connection = loop {
        let mut reply = io
            .accept(&listener)
            .unwrap_or_else(|_| fail(b"accept request"));
        if let Some(connection) = reply.take_connection() {
            break connection;
        }
        would_block(&reply);
        bounded_yield(clock);
    };
    marker(role, b"accepted=1");
    let mut received = [0u8; PUBLISH_BYTES];
    recv_all(io, &connection, &mut received, clock);
    verify(&received, 37, 11);
    let before = clock.millis(monotonic_read().unwrap_or_else(|_| fail(b"timeout clock")));
    let mut absent = [0u8; 1];
    refused(io.recv(&connection, &mut absent), net::STATUS_TIMEOUT);
    let after = clock.millis(monotonic_read().unwrap_or_else(|_| fail(b"timeout clock")));
    if after - before < 3000 {
        fail(b"premature timeout");
    }
    let mut response = [0u8; REPLY_BYTES];
    fixture(&mut response, 53, 19);
    send_all(io, &connection, &response, clock);
    marker(role, b"timeout=1 resumed=1");
    marker(role, b"sent=2048 received=4096 identical=1");
    loop {
        let mut trailing = [0u8; 1];
        let reply = io
            .recv(&connection, &mut trailing)
            .unwrap_or_else(|_| fail(b"eof request"));
        if reply.is_success() {
            if reply.transferred != 0 || reply.flags != net::FLAG_END_OF_STREAM {
                fail(b"unexpected trailing byte");
            }
            break;
        }
        would_block(&reply);
        bounded_yield(clock);
    }
    if !io
        .close(connection)
        .unwrap_or_else(|_| fail(b"accepted close request"))
        .is_success()
        || !io
            .close_listener(listener)
            .unwrap_or_else(|_| fail(b"listener close request"))
            .is_success()
    {
        fail(b"close");
    }
    marker(role, b"eof=1 connections closed=1 listeners closed=1");
}

fn capability_request(op: u8, capability: u64) -> net::WireNetworkRequest {
    net::WireNetworkRequest {
        magic: net::NETWORK_MAGIC,
        version: net::FORMAT_VERSION,
        op,
        transport: if op == net::OP_ACCEPT {
            net::TRANSPORT_TCP
        } else {
            net::TRANSPORT_NONE
        },
        flags: 0,
        port: 0,
        name_len: 0,
        capability,
        address_kind: net::ADDRESS_NONE,
        reserved: [0; 7],
        endpoint: [0; 24],
    }
}

fn refused(reply: Result<NetworkReply, NetworkError>, expected: i32) {
    let reply = reply.unwrap_or_else(|_| fail(b"authority request transport"));
    if reply.queue_status != slime_proto::io_queue::STATUS_OK
        || reply.status_detail != expected
        || reply.transferred != 0
        || reply.flags != 0
        || reply.capability_kind != net::CAPABILITY_NONE
        || reply.capability != 0
    {
        fail(b"authority refusal result");
    }
}

// These opaque fixtures match externally specified length-prefixed batches;
// network-service treats the prefix and payload alike as uninterpreted bytes.
fn fixture(bytes: &mut [u8], multiplier: usize, addend: usize) {
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = expected(index, multiplier, addend);
    }
}

fn expected(index: usize, multiplier: usize, addend: usize) -> u8 {
    match index % 1024 {
        0 => 0xfe,
        1 => 0x03,
        _ => index.wrapping_mul(multiplier).wrapping_add(addend) as u8,
    }
}

fn verify(bytes: &[u8], multiplier: usize, addend: usize) {
    if bytes
        .iter()
        .enumerate()
        .any(|(index, byte)| *byte != expected(index, multiplier, addend))
    {
        fail(b"stream identity");
    }
}

fn send_all(io: &mut NetworkIo<'_>, connection: &Connection, bytes: &[u8], clock: &TickClock) {
    let mut offset = 0;
    let mut request_index = 0;
    while offset < bytes.len() {
        let offered = [1, 17, 511, 2048][request_index % 4].min(bytes.len() - offset);
        let reply = io
            .send(connection, &bytes[offset..offset + offered])
            .unwrap_or_else(|_| fail(b"send request"));
        if reply.is_success() {
            if reply.transferred == 0 || reply.transferred > offered as u64 {
                fail(b"send count");
            }
            offset += reply.transferred as usize;
            request_index += 1;
        } else {
            would_block(&reply);
        }
        bounded_yield(clock);
    }
}

fn recv_all(io: &mut NetworkIo<'_>, connection: &Connection, bytes: &mut [u8], clock: &TickClock) {
    let mut offset = 0;
    while offset < bytes.len() {
        let count = (bytes.len() - offset).min(733);
        let reply = io
            .recv(connection, &mut bytes[offset..offset + count])
            .unwrap_or_else(|_| fail(b"recv request"));
        if reply.is_success() {
            if reply.transferred == 0 || reply.flags != 0 {
                fail(b"premature eof");
            }
            offset += reply.transferred as usize;
        } else {
            would_block(&reply);
        }
        bounded_yield(clock);
    }
}

fn recv_response_after_stall(
    io: &mut NetworkIo<'_>,
    connection: &Connection,
    bytes: &mut [u8],
    clock: &TickClock,
) {
    let mut offset = 0;
    let mut timeouts = 0;
    while offset < bytes.len() {
        let count = (bytes.len() - offset).min(733);
        let reply = io
            .recv(connection, &mut bytes[offset..offset + count])
            .unwrap_or_else(|_| fail(b"response recv request"));
        if reply.is_success() {
            if reply.transferred == 0 || reply.flags != 0 {
                fail(b"response premature eof");
            }
            offset += reply.transferred as usize;
        } else if reply.status_detail == net::STATUS_TIMEOUT {
            refused(Ok(reply), net::STATUS_TIMEOUT);
            timeouts += 1;
            if timeouts > 1 {
                fail(b"response repeated timeout");
            }
        } else {
            would_block(&reply);
        }
        bounded_yield(clock);
    }
    Line::<128>::new()
        .bytes(b"[io-local-network-probe] role=publisher timeout_retries=")
        .decimal(timeouts)
        .bytes(b"\n")
        .emit();
}

fn would_block(reply: &NetworkReply) {
    if reply.status_detail != net::STATUS_WOULD_BLOCK || reply.transferred != 0 {
        Line::<128>::new()
            .bytes(b"[io-local-network-probe] status magnitude=")
            .decimal(u64::from(reply.status_detail.unsigned_abs()))
            .bytes(b" queue=")
            .decimal(u64::from(reply.queue_status))
            .bytes(b" capability_kind=")
            .decimal(u64::from(reply.capability_kind))
            .bytes(b"\n")
            .emit();
        fail(b"unexpected network status");
    }
}

fn bounded_yield(clock: &TickClock) {
    if clock.millis(monotonic_read().unwrap_or_else(|_| fail(b"clock read"))) >= DEADLINE_MS {
        fail(b"deadline");
    }
    yield_now();
}

/// Emit one marker as a single record; an invalid or oversized one is refused.
fn marker(role: &[u8], message: &[u8]) {
    Line::<256>::new()
        .bytes(b"[io-local-network-probe] role=")
        .bytes(role)
        .bytes(b" ")
        .bytes(message)
        .bytes(b"\n")
        .emit();
}

fn fail(reason: &[u8]) -> ! {
    Line::<256>::new()
        .bytes(b"[io-local-network-probe] fail: ")
        .bytes(reason)
        .bytes(b"\n")
        .emit();
    exit(1)
}
