#![no_std]
#![no_main]

//! IO11 client: the one component granted a TCP destination on the `sel4-io-tcp`
//! plane. It opens its declared destination through the network service, keeps
//! the service resident long enough for the plane's peer to exercise the link,
//! and closes everything it opened. Every count in a marker is observed.

use slime_components::tick_clock::TickClock;
use slime_proto::network_service::{self, WireNetworkCompletion, WireNetworkRequest};
use slime_proto::valid_network_completion;
use slime_rt::{
    ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, debug_write, exit, monotonic_frequency,
    monotonic_read, yield_now,
};

slime_rt::entry!(main);

const SERVICE_SLOT: u32 = 0;
const SHUTDOWN_CAPABILITY: u64 = u64::MAX;
const PEER: [u8; 4] = [10, 0, 0, 2];
const ECHO_PORT: u16 = 4242;
/// How long the service stays bound to the link after the destination is
/// open: the plane's peer needs a resident stack to address its ARP and ICMP
/// requests to, and a peer cannot tell a client when it is done.
const HOLD_MS: u64 = 3000;

fn main(_: u32) {
    let rate = monotonic_frequency().unwrap_or_else(|_| fail(b"clock rate"));
    let base = monotonic_read().unwrap_or_else(|_| fail(b"monotonic read"));
    let clock = TickClock::new(rate, base).unwrap_or_else(|_| fail(b"clock rate too slow"));
    write_number(b"[io-tcp-probe] clock rate=", rate);
    debug_write(b"\n");

    let tcp = call(destination(network_service::OP_CONNECT, PEER, ECHO_PORT));
    if tcp.status_detail != 0 || tcp.capability_kind != network_service::CAPABILITY_TCP_CONNECTION {
        fail(b"tcp connect");
    }
    debug_write(b"[io-tcp-probe] tcp capabilities=1 rights=connect,send,recv\n");

    while clock.millis(monotonic_read().unwrap_or_else(|_| fail(b"monotonic read")))
        < HOLD_MS as i64
    {
        yield_now();
    }
    write_number(b"[io-tcp-probe] held ms=", HOLD_MS);
    debug_write(b"\n");

    let closed = call(capability(network_service::OP_CLOSE, tcp.capability));
    if closed.status_detail != 0 {
        fail(b"close");
    }
    let shutdown = call(capability(network_service::OP_CLOSE, SHUTDOWN_CAPABILITY));
    if shutdown.status_detail != 0 {
        fail(b"shutdown");
    }
    write_number(b"[io-tcp-probe] closed capabilities=", 1);
    write_number(b" shutdown=", u64::from(shutdown.status_detail == 0));
    debug_write(b"\n");
    exit(0)
}

fn destination(op: u8, address: [u8; 4], port: u16) -> WireNetworkRequest {
    let mut endpoint = [0; 24];
    endpoint[..4].copy_from_slice(&address);
    WireNetworkRequest {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        op,
        transport: network_service::TRANSPORT_TCP,
        flags: 0,
        port,
        name_len: 0,
        capability: 0,
        address_kind: network_service::ADDRESS_IPV4,
        reserved: [0; 7],
        endpoint,
    }
}

fn capability(op: u8, id: u64) -> WireNetworkRequest {
    WireNetworkRequest {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        op,
        transport: network_service::TRANSPORT_NONE,
        flags: 0,
        port: 0,
        name_len: 0,
        capability: id,
        address_kind: network_service::ADDRESS_NONE,
        reserved: [0; 7],
        endpoint: [0; 24],
    }
}

fn call(request: WireNetworkRequest) -> WireNetworkCompletion {
    let bytes = request.encode();
    loop {
        match slime_rt::send(SERVICE_SLOT, &bytes, &[]) {
            ERR_WOULDBLOCK => yield_now(),
            ERR_SUCCESS => break,
            _ => fail(b"send"),
        }
    }
    let mut out = [0; MAX_MSG];
    let mut caps = [0; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(SERVICE_SLOT, &mut out, &mut caps) {
            ERR_WOULDBLOCK => yield_now(),
            result if result < 0 => fail(b"recv"),
            result => {
                let reply = WireNetworkCompletion::decode(&out[..result as usize])
                    .unwrap_or_else(|| fail(b"reply"));
                if !valid_network_completion(&reply) {
                    fail(b"invalid completion");
                }
                return reply;
            }
        }
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
